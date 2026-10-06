//! ListenBrainz: playing now, listens, token checks and the MusicBrainz
//! mapping lookup. Listens carry the same `additional_info` keys as the
//! ones ListenBrainz records from Spotify, so pages that read a user's
//! listens treat both alike.

use super::Failure;
use super::meta::Song;
use super::queue::Pending;
use serde_json::{Map, Value, json};

const ROOT: &str = "https://api.listenbrainz.org/1";

#[derive(Clone)]
pub struct ListenBrainz {
    http: reqwest::Client,
}

/// What `/1/metadata/lookup` matched a name to.
#[derive(Debug, Default, PartialEq)]
pub struct Mapping {
    pub recording_mbid: Option<String>,
    pub release_mbid: Option<String>,
    pub recording_name: String,
    pub artist_credit_name: String,
    pub release_name: Option<String>,
}

impl ListenBrainz {
    pub fn new(http: reqwest::Client) -> Self {
        Self { http }
    }

    /// The user the token belongs to, or `None` for a token ListenBrainz
    /// does not know.
    pub async fn validate(&self, token: &str) -> Result<Option<String>, Failure> {
        let response = self
            .http
            .get(format!("{ROOT}/validate-token"))
            .header("Authorization", format!("Token {token}"))
            .send()
            .await
            .map_err(|e| Failure::Transient(e.to_string()))?;
        if response.status().is_server_error() {
            return Err(Failure::Transient(format!(
                "listenbrainz answered {}",
                response.status()
            )));
        }
        let json: Value = response.json().await.unwrap_or(Value::Null);
        Ok(json["valid"]
            .as_bool()
            .unwrap_or(false)
            .then(|| json["user_name"].as_str().unwrap_or_default().to_owned()))
    }

    pub async fn playing_now(&self, token: &str, song: &Song) -> Result<(), Failure> {
        let body = json!({
            "listen_type": "playing_now",
            "payload": [{ "track_metadata": track_metadata(song) }],
        });
        self.submit(token, &body).await
    }

    /// One play as `single`, more as `import`, which is what ListenBrainz
    /// asks of clients sending a backlog.
    pub async fn listens(&self, token: &str, plays: &[Pending]) -> Result<(), Failure> {
        let payload: Vec<Value> = plays.iter().map(listen).collect();
        let body = json!({
            "listen_type": if plays.len() == 1 { "single" } else { "import" },
            "payload": payload,
        });
        self.submit(token, &body).await
    }

    pub async fn lookup(&self, artist: &str, title: &str, album: Option<&str>) -> Option<Mapping> {
        let mut query = vec![("artist_name", artist), ("recording_name", title)];
        if let Some(album) = album {
            query.push(("release_name", album));
        }
        let response = self
            .http
            .get(format!("{ROOT}/metadata/lookup/"))
            .query(&query)
            .send()
            .await
            .ok()?;
        let json: Value = response.json().await.ok()?;
        let text = |key: &str| json[key].as_str().map(str::to_owned);
        Some(Mapping {
            recording_mbid: text("recording_mbid"),
            release_mbid: text("release_mbid"),
            recording_name: text("recording_name")?,
            artist_credit_name: text("artist_credit_name")?,
            release_name: text("release_name"),
        })
    }

    async fn submit(&self, token: &str, body: &Value) -> Result<(), Failure> {
        let response = self
            .http
            .post(format!("{ROOT}/submit-listens"))
            .header("Authorization", format!("Token {token}"))
            .json(body)
            .send()
            .await
            .map_err(|e| Failure::Transient(e.to_string()))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let message = response
            .json::<Value>()
            .await
            .ok()
            .and_then(|j| j["error"].as_str().map(str::to_owned))
            .unwrap_or_else(|| format!("listenbrainz answered {status}"));
        Err(match status.as_u16() {
            401 => Failure::Auth(message),
            429 => Failure::Transient(message),
            s if s >= 500 => Failure::Transient(message),
            _ => Failure::Permanent(message),
        })
    }
}

fn listen(play: &Pending) -> Value {
    json!({
        "listened_at": play.listened_at,
        "track_metadata": track_metadata(&play.song),
    })
}

/// The song in the shape of a Spotify listen: artists joined with ", " in
/// `artist_name` and listed in `artist_names`, the same for the album's
/// artists, and the watch URL as the origin.
pub fn track_metadata(song: &Song) -> Value {
    let mut info = Map::new();
    let mut put = |key: &str, value: Value| {
        info.insert(key.to_owned(), value);
    };
    put("media_player", "FormalMusic".into());
    put("submission_client", "FormalMusic".into());
    put(
        "submission_client_version",
        env!("CARGO_PKG_VERSION").into(),
    );
    put("music_service", "music.youtube.com".into());
    put(
        "origin_url",
        format!("https://music.youtube.com/watch?v={}", song.video_id).into(),
    );
    put("artist_names", song.artists.clone().into());
    if let Some(ms) = song.duration_ms {
        put("duration_ms", ms.into());
    }
    if !song.album_artists.is_empty() {
        put("release_artist_name", song.album_artists.join(", ").into());
        put("release_artist_names", song.album_artists.clone().into());
    }
    if let Some(n) = song.track_number {
        put("tracknumber", n.into());
    }
    if let Some(isrc) = &song.isrc {
        put("isrc", isrc.clone().into());
    }
    if let Some(mbid) = &song.recording_mbid {
        put("recording_mbid", mbid.clone().into());
    }
    if let Some(mbid) = &song.release_mbid {
        put("release_mbid", mbid.clone().into());
    }
    let mut metadata = json!({
        "artist_name": song.artists.join(", "),
        "track_name": song.title,
        "additional_info": info,
    });
    if let Some(album) = &song.album {
        metadata["release_name"] = album.clone().into();
    }
    metadata
}

#[cfg(test)]
mod tests {
    use super::*;
    use formalmusic_api::ScrobbleService;

    fn song() -> Song {
        Song {
            video_id: "lp-EO5I60KA".into(),
            title: "Thinking Out Loud".into(),
            artists: vec!["Ed Sheeran".into()],
            album: Some("x (Wembley Edition)".into()),
            album_artists: vec!["Ed Sheeran".into()],
            duration_ms: Some(281_000),
            track_number: Some(11),
            isrc: Some("GBAHS1400099".into()),
            recording_mbid: Some("0d1ea0b0-0000-4000-8000-000000000000".into()),
            release_mbid: None,
        }
    }

    #[test]
    fn listens_look_like_spotify_listens() {
        let m = track_metadata(&song());
        assert_eq!(m["artist_name"], "Ed Sheeran");
        assert_eq!(m["track_name"], "Thinking Out Loud");
        assert_eq!(m["release_name"], "x (Wembley Edition)");
        let info = &m["additional_info"];
        assert_eq!(info["music_service"], "music.youtube.com");
        assert_eq!(
            info["origin_url"],
            "https://music.youtube.com/watch?v=lp-EO5I60KA"
        );
        assert_eq!(info["media_player"], "FormalMusic");
        assert_eq!(info["submission_client"], "FormalMusic");
        assert_eq!(info["duration_ms"], 281_000);
        assert_eq!(info["artist_names"], json!(["Ed Sheeran"]));
        assert_eq!(info["release_artist_name"], "Ed Sheeran");
        assert_eq!(info["release_artist_names"], json!(["Ed Sheeran"]));
        assert_eq!(info["tracknumber"], 11);
        assert_eq!(info["isrc"], "GBAHS1400099");
        assert!(info.get("release_mbid").is_none());
    }

    #[test]
    fn joins_several_artists_like_spotify() {
        let mut s = song();
        s.artists = vec!["Delinquent".into(), "KCAT".into()];
        assert_eq!(track_metadata(&s)["artist_name"], "Delinquent, KCAT");
    }

    #[test]
    fn listens_carry_the_start_time() {
        let play = Pending {
            service: ScrobbleService::ListenBrainz,
            listened_at: 1_791_300_000,
            song: song(),
        };
        assert_eq!(listen(&play)["listened_at"], 1_791_300_000);
    }
}
