//! Last.fm's 2.0 API: desktop authentication, now playing, scrobbles and
//! the album lookup for videos.

use super::Failure;
use super::meta::Song;
use super::queue::Pending;
use formalmusic_api::LastFmApp;
use md5::{Digest, Md5};
use parking_lot::RwLock;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;

const ROOT: &str = "https://ws.audioscrobbler.com/2.0/";
const AUTH_PAGE: &str = "https://www.last.fm/api/auth/";

/// `api_sig`: every parameter but `format` and `callback`, sorted by name,
/// each name followed by its value, then the shared secret, hashed with MD5.
pub fn sign(params: &BTreeMap<&str, String>, secret: &str) -> String {
    let mut text = String::new();
    for (name, value) in params {
        if *name != "format" && *name != "callback" {
            text.push_str(name);
            text.push_str(value);
        }
    }
    text.push_str(secret);
    format!("{:x}", Md5::digest(text.as_bytes()))
}

/// Shares the API account between clones, so one set in Settings reaches
/// every task holding one.
#[derive(Clone)]
pub struct LastFm {
    http: reqwest::Client,
    app: Arc<RwLock<Option<LastFmApp>>>,
}

pub struct Session {
    pub key: String,
    pub username: String,
}

impl LastFm {
    pub fn new(http: reqwest::Client, app: Option<LastFmApp>) -> Self {
        Self {
            http,
            app: Arc::new(RwLock::new(app)),
        }
    }

    pub fn configured(&self) -> bool {
        self.app.read().is_some()
    }

    pub fn set_app(&self, app: LastFmApp) {
        *self.app.write() = Some(app);
    }

    pub fn app(&self) -> Result<LastFmApp, Failure> {
        self.app
            .read()
            .clone()
            .ok_or_else(|| Failure::Permanent("no Last.fm API key".into()))
    }

    pub fn auth_url(&self, token: &str) -> Option<String> {
        let app = self.app.read();
        Some(format!(
            "{AUTH_PAGE}?api_key={}&token={token}",
            app.as_ref()?.api_key
        ))
    }

    /// A token for the browser step, signed with `app`. Answers whether the
    /// key and secret work before anything keeps them.
    pub async fn token_with(&self, app: &LastFmApp) -> Result<String, Failure> {
        let json = self.request(app, "auth.getToken", BTreeMap::new()).await?;
        json["token"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| Failure::Permanent("auth.getToken: no token".into()))
    }

    /// `None` while the user has not allowed access yet.
    pub async fn session(&self, token: &str) -> Result<Option<Session>, Failure> {
        let mut params = BTreeMap::new();
        params.insert("token", token.to_owned());
        match self.signed("auth.getSession", params).await {
            Ok(json) => {
                let session = &json["session"];
                Ok(Some(Session {
                    key: session["key"].as_str().unwrap_or_default().to_owned(),
                    username: session["name"].as_str().unwrap_or_default().to_owned(),
                }))
            }
            // 14: not authorised yet.
            Err(Failure::Api(14, _)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub async fn now_playing(&self, session_key: &str, song: &Song) -> Result<(), Failure> {
        let mut params = BTreeMap::new();
        params.insert("sk", session_key.to_owned());
        for (name, value) in song_params(song) {
            params.insert(name, value);
        }
        self.signed("track.updateNowPlaying", params)
            .await
            .map(drop)
    }

    /// Submits up to 50 plays. Answers with the plays Last.fm took or
    /// ignored for good; anything else stays queued.
    pub async fn scrobble(
        &self,
        session_key: &str,
        plays: &[Pending],
    ) -> Result<Vec<Pending>, Failure> {
        let mut params: BTreeMap<&str, String> = BTreeMap::new();
        params.insert("sk", session_key.to_owned());
        // BTreeMap keys borrow; the indexed names live as long as the call.
        let mut names: Vec<(String, String)> = Vec::new();
        for (i, play) in plays.iter().enumerate() {
            names.push((format!("timestamp[{i}]"), play.listened_at.to_string()));
            for (name, value) in song_params(&play.song) {
                names.push((format!("{name}[{i}]"), value));
            }
        }
        for (name, value) in &names {
            params.insert(name.as_str(), value.clone());
        }
        let json = self.signed("track.scrobble", params).await?;
        let results = match &json["scrobbles"]["scrobble"] {
            Value::Array(list) => list.clone(),
            one @ Value::Object(_) => vec![one.clone()],
            _ => Vec::new(),
        };
        for (play, result) in plays.iter().zip(&results) {
            let code = &result["ignoredMessage"]["code"];
            if code.as_str().is_some_and(|c| c != "0") || code.as_u64().is_some_and(|c| c != 0) {
                tracing::info!(
                    title = play.song.title,
                    "last.fm ignored a scrobble: {}",
                    result["ignoredMessage"]["#text"]
                        .as_str()
                        .unwrap_or_default()
                );
            }
        }
        Ok(plays.to_vec())
    }

    /// The album Last.fm files a track under, with its artist.
    pub async fn album(&self, artist: &str, title: &str) -> Option<(String, Option<String>)> {
        let app = self.app().ok()?;
        let response = self
            .http
            .get(ROOT)
            .query(&[
                ("method", "track.getInfo"),
                ("api_key", app.api_key.as_str()),
                ("artist", artist),
                ("track", title),
                ("autocorrect", "1"),
                ("format", "json"),
            ])
            .send()
            .await
            .ok()?;
        let json: Value = response.json().await.ok()?;
        let album = &json["track"]["album"];
        let name = album["title"].as_str().filter(|s| !s.is_empty())?;
        Some((name.to_owned(), album["artist"].as_str().map(str::to_owned)))
    }

    async fn signed(&self, method: &str, params: BTreeMap<&str, String>) -> Result<Value, Failure> {
        let app = self.app()?;
        self.request(&app, method, params).await
    }

    async fn request(
        &self,
        app: &LastFmApp,
        method: &str,
        mut params: BTreeMap<&str, String>,
    ) -> Result<Value, Failure> {
        params.insert("method", method.to_owned());
        params.insert("api_key", app.api_key.clone());
        let sig = sign(&params, &app.shared_secret);
        params.insert("api_sig", sig);
        params.insert("format", "json".to_owned());
        let response = self
            .http
            .post(ROOT)
            .form(&params)
            .send()
            .await
            .map_err(|e| Failure::Transient(e.to_string()))?;
        let status = response.status();
        let json: Value = response.json().await.unwrap_or(Value::Null);
        if let Some(code) = json["error"].as_u64() {
            let message = json["message"].as_str().unwrap_or_default().to_owned();
            return Err(match code {
                // Invalid or revoked session key.
                9 => Failure::Auth(message),
                // Invalid API key, or a signature the secret does not match.
                10 | 13 | 26 => Failure::App(message),
                // Service offline, temporarily unavailable, rate limited.
                11 | 16 | 29 => Failure::Transient(message),
                code => Failure::Api(code, message),
            });
        }
        if status.is_server_error() {
            return Err(Failure::Transient(format!("last.fm answered {status}")));
        }
        if !status.is_success() {
            return Err(Failure::Permanent(format!("last.fm answered {status}")));
        }
        Ok(json)
    }
}

/// The song as `track.updateNowPlaying` and `track.scrobble` take it. The
/// artist is the main one alone, as Spotify sends it, since Last.fm keeps
/// one artist per track.
fn song_params(song: &Song) -> Vec<(&'static str, String)> {
    let mut params = vec![
        ("artist", song.artist().to_owned()),
        ("track", song.title.clone()),
    ];
    if let Some(album) = &song.album {
        params.push(("album", album.clone()));
        if let Some(album_artist) = song.album_artists.first() {
            params.push(("albumArtist", album_artist.clone()));
        }
    }
    if let Some(ms) = song.duration_ms {
        params.push(("duration", (ms / 1000).to_string()));
    }
    if let Some(n) = song.track_number {
        params.push(("trackNumber", n.to_string()));
    }
    if let Some(mbid) = &song.recording_mbid {
        params.push(("mbid", mbid.clone()));
    }
    params
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The worked example from https://www.last.fm/api/authspec, section 8:
    /// `api_keyxxxxxxxxxxmethodauth.getSessiontokenxxxxxxx` plus the secret.
    #[test]
    fn signs_like_the_documented_example() {
        let mut params = BTreeMap::new();
        params.insert("method", "auth.getSession".to_owned());
        params.insert("token", "xxxxxxx".to_owned());
        params.insert("api_key", "xxxxxxxxxx".to_owned());
        params.insert("format", "json".to_owned());
        assert_eq!(
            sign(&params, "mysecret"),
            "e2c6398b3e8417a9662d82953b04a47a"
        );
    }

    #[test]
    fn indexed_names_sort_as_strings() {
        let mut params = BTreeMap::new();
        params.insert("track[10]", "b".to_owned());
        params.insert("track[2]", "a".to_owned());
        params.insert("artist[0]", "c".to_owned());
        let manual = format!("{:x}", Md5::digest(b"artist[0]ctrack[10]btrack[2]as"));
        assert_eq!(sign(&params, "s"), manual);
    }

    #[test]
    fn sends_the_main_artist_and_album_artist() {
        let song = Song {
            video_id: "v".into(),
            title: "Die With A Smile".into(),
            artists: vec!["Lady Gaga".into(), "Bruno Mars".into()],
            album: Some("Die With A Smile".into()),
            album_artists: vec!["Lady Gaga".into(), "Bruno Mars".into()],
            duration_ms: Some(251_000),
            track_number: Some(1),
            ..Song::default()
        };
        let params = song_params(&song);
        let get = |k: &str| {
            params
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("artist"), Some("Lady Gaga"));
        assert_eq!(get("albumArtist"), Some("Lady Gaga"));
        assert_eq!(get("duration"), Some("251"));
        assert_eq!(get("trackNumber"), Some("1"));
        assert_eq!(get("mbid"), None);
    }
}
