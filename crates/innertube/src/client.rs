use crate::Result;
use crate::auth::{ORIGIN, Session, cookie_value};
use crate::endpoints::{bare_playlist_id, browse_request, search_params};
use crate::parse::{self, decode_percent, missing};
use formalmusic_api::{
    Account, ApiError, BrowseTarget, Continuation, ContinuationPage, Lyrics, Page, PlaylistEdit,
    Privacy, RateTarget, Rating, SearchFilter, SearchResults, SessionInfo, Suggestion,
};
use serde_json::{Value, json};
use std::hash::{BuildHasher, RandomState};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::OnceCell;

pub use crate::parse::next::NextResult;
pub use crate::parse::player::PlaybackTracking;

const API_BASE: &str = "https://music.youtube.com/youtubei/v1/";
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
/// Used when the music.youtube.com page cannot be read. InnerTube accepts
/// client versions for months after the web app moves on.
const FALLBACK_CLIENT_VERSION: &str = "1.20261004.17.00";
/// The player script's `signatureTimestamp`. Without one, `player` answers
/// "The page needs to be reloaded" instead of a playable response.
const FALLBACK_SIGNATURE_TIMESTAMP: u64 = 20726;
/// A long-lived album track whose `player` formats show whether the session
/// has Premium (see [`parse::player::has_premium_audio`]).
const PREMIUM_PROBE_VIDEO: &str = "IluRBvnYMoY";
const ANDROID_MUSIC_VERSION: &str = "7.21.50";
const LANGUAGE: &str = "en";
const LOCATION: &str = "US";
/// The EU consent wall redirects anonymous page loads without this cookie.
const CONSENT_COOKIE: &str = "SOCS=CAI";

/// A YouTube Music API client. Cloning is cheap and clones share the
/// connection pool and the cached web app config.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    session: Option<Session>,
    config: OnceCell<WebConfig>,
    signature_timestamp: OnceCell<u64>,
}

/// What the music.youtube.com page tells its own scripts (`ytcfg`).
#[derive(Debug, Clone)]
struct WebConfig {
    client_version: String,
    visitor_data: Option<String>,
    player_js: Option<String>,
}

#[derive(Clone, Copy)]
enum ClientName {
    WebRemix,
    AndroidMusic,
}

impl Client {
    pub fn anonymous() -> Result<Self> {
        Self::build(None)
    }

    /// A client acting as the account behind `cookies`, a `Cookie` header from
    /// a signed-in music.youtube.com tab. `page_id` selects a brand account.
    pub fn signed_in(cookies: &str, page_id: Option<String>) -> Result<Self> {
        let session = Session::new(cookies, page_id).ok_or_else(|| {
            ApiError::BadRequest(
                "the cookies have no SAPISID; copy them from a signed-in tab".into(),
            )
        })?;
        Self::build(Some(session))
    }

    /// [`Client::signed_in`] from a Netscape cookie file, the format yt-dlp
    /// reads with `--cookies`, so the daemon can hand both the same file.
    pub fn from_cookie_file(
        path: impl AsRef<std::path::Path>,
        page_id: Option<String>,
    ) -> Result<Self> {
        let path = path.as_ref();
        let file = std::fs::read_to_string(path)
            .map_err(|e| ApiError::BadRequest(format!("{}: {e}", path.display())))?;
        Self::signed_in(&crate::auth::cookie_header_from_netscape(&file), page_id)
    }

    fn build(session: Option<Session>) -> Result<Self> {
        // Over HTTP/2, h2's small DATA frame budget runs out after a few dozen
        // music.youtube.com responses on one connection, and the GOAWAY it sends
        // (too_many_data_frames) fails the response in flight.
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .http1_only()
            .build()
            .map_err(network)?;
        Ok(Self {
            inner: Arc::new(Inner {
                http,
                session,
                config: OnceCell::new(),
                signature_timestamp: OnceCell::new(),
            }),
        })
    }

    pub fn is_signed_in(&self) -> bool {
        self.inner.session.is_some()
    }

    // Browsing

    pub async fn browse(&self, target: BrowseTarget) -> Result<Page> {
        let (browse_id, params) = browse_request(&target);
        let mut body = json!({ "browseId": browse_id });
        if let Some(params) = params {
            body["params"] = params.into();
        }
        let json = self.post("browse", body).await?;
        parse::page::parse_page(target, &json)
    }

    /// More sections or items from a [`Continuation`] that a page, a shelf or
    /// a search result handed out.
    pub async fn continuation(&self, token: &Continuation) -> Result<ContinuationPage> {
        if let Some(search_token) = token.0.strip_prefix(parse::search::CONTINUATION_PREFIX) {
            let json = self
                .post("search", json!({ "continuation": search_token }))
                .await?;
            let mut page = parse::page::parse_continuation(&json)?;
            page.continuation = page.continuation.map(parse::search::tag);
            return Ok(page);
        }
        let json = self
            .post("browse", json!({ "continuation": token.0 }))
            .await?;
        parse::page::parse_continuation(&json)
    }

    pub async fn search(&self, query: &str, filter: Option<SearchFilter>) -> Result<SearchResults> {
        let mut body = json!({ "query": query });
        if let Some(filter) = filter {
            body["params"] = search_params(filter).into();
        }
        let json = self.post("search", body).await?;
        parse::search::parse_search(query, filter, &json)
    }

    pub async fn suggestions(&self, query: &str) -> Result<Vec<Suggestion>> {
        let json = self
            .post("music/get_search_suggestions", json!({ "input": query }))
            .await?;
        Ok(parse::suggestions::parse_suggestions(&json))
    }

    /// The player page for a track or playlist: the up-next queue and the
    /// browse ids of its Lyrics and Related tabs.
    pub async fn next(
        &self,
        video_id: Option<&str>,
        playlist_id: Option<&str>,
    ) -> Result<NextResult> {
        self.next_with(video_id, playlist_id, None).await
    }

    /// The radio the web app's "Start radio" plays for a track.
    pub async fn radio(&self, video_id: &str) -> Result<NextResult> {
        // RDAMVM plus the video id is the track's radio playlist, and `wAEB`
        // the params the web app sends with it.
        self.next_with(
            Some(video_id),
            Some(&format!("RDAMVM{video_id}")),
            Some("wAEB"),
        )
        .await
    }

    async fn next_with(
        &self,
        video_id: Option<&str>,
        playlist_id: Option<&str>,
        params: Option<&str>,
    ) -> Result<NextResult> {
        if video_id.is_none() && playlist_id.is_none() {
            return Err(ApiError::BadRequest(
                "next needs a video id or a playlist id".into(),
            ));
        }
        let mut body = json!({ "isAudioOnly": true, "enablePersistentPlaylistPanel": true });
        if let Some(video_id) = video_id {
            body["videoId"] = video_id.into();
        }
        if let Some(playlist_id) = playlist_id {
            body["playlistId"] = bare_playlist_id(playlist_id).into();
        }
        if let Some(params) = params {
            body["params"] = params.into();
        }
        let json = self.post("next", body).await?;
        parse::next::parse_next(&json)
    }

    /// More of a queue, from [`NextResult::continuation`].
    pub async fn next_continuation(
        &self,
        playlist_id: &str,
        token: &Continuation,
    ) -> Result<NextResult> {
        let body = json!({
            "continuation": token.0,
            "playlistId": bare_playlist_id(playlist_id),
            "isAudioOnly": true,
            "enablePersistentPlaylistPanel": true,
        });
        let json = self.post("next", body).await?;
        parse::next::parse_next_continuation(&json)
    }

    /// Lyrics for a track, synced when YouTube has them synced. `None` when
    /// the track has no lyrics tab or no lyrics in this region.
    pub async fn lyrics(&self, video_id: &str) -> Result<Option<Lyrics>> {
        let next = self.next(Some(video_id), None).await?;
        match next.lyrics_browse_id {
            Some(browse_id) => self.lyrics_by_browse_id(&browse_id).await,
            None => Ok(None),
        }
    }

    /// Lyrics from a [`NextResult::lyrics_browse_id`]. Only the Android
    /// client gets timed lines, so it goes first and the web client fills in
    /// when it has nothing.
    pub async fn lyrics_by_browse_id(&self, browse_id: &str) -> Result<Option<Lyrics>> {
        let body = json!({ "browseId": browse_id });
        match self
            .post_as(ClientName::AndroidMusic, "browse", body.clone())
            .await
        {
            Ok(json) => {
                if let Some(lyrics) = parse::lyrics::parse_timed(&json) {
                    return Ok(Some(lyrics));
                }
            }
            Err(err) => {
                tracing::debug!(%err, "android lyrics request failed, trying the web client")
            }
        }
        let json = self.post("browse", body).await?;
        Ok(parse::lyrics::parse_plain(&json))
    }

    /// The player page's Related tab, from [`NextResult::related_browse_id`].
    pub async fn related(&self, browse_id: &str) -> Result<Page> {
        self.browse(BrowseTarget::Raw {
            browse_id: browse_id.to_owned(),
            params: None,
        })
        .await
    }

    // Library mutations

    pub async fn rate(&self, target: &RateTarget, rating: Rating) -> Result<()> {
        self.require_session()?;
        let endpoint = match rating {
            Rating::Like => "like/like",
            Rating::Dislike => "like/dislike",
            Rating::Indifferent => "like/removelike",
        };
        let target = match target {
            RateTarget::Track { video_id } => json!({ "videoId": video_id }),
            RateTarget::Playlist { playlist_id } => {
                json!({ "playlistId": bare_playlist_id(playlist_id) })
            }
        };
        self.post(endpoint, json!({ "target": target }))
            .await
            .map(drop)
    }

    /// Saving an album or someone else's playlist is a like on its playlist.
    pub async fn set_in_library(&self, playlist_id: &str, saved: bool) -> Result<()> {
        let rating = if saved {
            Rating::Like
        } else {
            Rating::Indifferent
        };
        self.rate(
            &RateTarget::Playlist {
                playlist_id: playlist_id.to_owned(),
            },
            rating,
        )
        .await
    }

    pub async fn set_subscribed(&self, channel_id: &str, subscribed: bool) -> Result<()> {
        self.require_session()?;
        let endpoint = if subscribed {
            "subscription/subscribe"
        } else {
            "subscription/unsubscribe"
        };
        self.post(endpoint, json!({ "channelIds": [channel_id] }))
            .await
            .map(drop)
    }

    /// Returns the new playlist's id.
    pub async fn create_playlist(
        &self,
        title: &str,
        description: &str,
        privacy: Privacy,
        video_ids: &[String],
    ) -> Result<String> {
        self.require_session()?;
        let mut body = json!({ "title": title, "description": description, "privacyStatus": privacy_name(privacy) });
        if !video_ids.is_empty() {
            body["videoIds"] = json!(video_ids);
        }
        let json = self.post("playlist/create", body).await?;
        json["playlistId"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| missing("playlistId"))
    }

    pub async fn edit_playlist(&self, playlist_id: &str, edits: &[PlaylistEdit]) -> Result<()> {
        self.require_session()?;
        let actions: Vec<Value> = edits.iter().map(edit_action).collect();
        let body = json!({ "playlistId": bare_playlist_id(playlist_id), "actions": actions });
        let json = self.post("browse/edit_playlist", body).await?;
        check_status(&json)
    }

    pub async fn delete_playlist(&self, playlist_id: &str) -> Result<()> {
        self.require_session()?;
        self.post(
            "playlist/delete",
            json!({ "playlistId": bare_playlist_id(playlist_id) }),
        )
        .await
        .map(drop)
    }

    /// Removes one History row, by the token from [`formalmusic_api::Track::feedback_token`].
    pub async fn remove_from_history(&self, feedback_token: &str) -> Result<()> {
        self.require_session()?;
        let json = self
            .post("feedback", json!({ "feedbackTokens": [feedback_token] }))
            .await?;
        let processed = json["feedbackResponses"]
            .as_array()
            .is_some_and(|r| r.iter().all(|r| r["isProcessed"] == true));
        if processed {
            Ok(())
        } else {
            Err(ApiError::BadRequest(
                "YouTube did not accept the history removal".into(),
            ))
        }
    }

    // Session

    /// The Google account and the brand accounts under it.
    pub async fn accounts(&self) -> Result<Vec<Account>> {
        self.require_session()?;
        let json = self.post("account/accounts_list", json!({})).await?;
        parse::account::parse_accounts(&json)
    }

    pub async fn session(&self) -> Result<SessionInfo> {
        let Some(session) = &self.inner.session else {
            return Ok(SessionInfo::default());
        };
        let player_body = self.player_body(PREMIUM_PROBE_VIDEO).await;
        let (menu, player) = tokio::join!(
            self.post("account/account_menu", json!({})),
            self.post("player", player_body),
        );
        let mut info = parse::account::parse_session(&menu?)?;
        if let Some(account) = &mut info.account {
            account.page_id = session.page_id.clone();
        }
        match player {
            Ok(player) => info.premium = parse::player::has_premium_audio(&player),
            Err(err) => tracing::warn!(%err, "could not check for Premium formats"),
        }
        Ok(info)
    }

    // Playback reporting

    /// The URLs that put a play into History, and the track's loudness. Asked
    /// as the signed-in session so the play counts for that account.
    pub async fn playback_tracking(&self, video_id: &str) -> Result<PlaybackTracking> {
        let body = self.player_body(video_id).await;
        let json = self.post("player", body).await?;
        parse::player::parse_player(video_id, &json)
    }

    /// Sends one of the playback-tracking pings, a [`PlaybackTracking`] URL
    /// with the caller's `cpn`, `cmt`, `st`/`et` and friends appended, with
    /// the headers the player response asks for (`USER_AUTH`, `VISITOR_ID`,
    /// `PLUS_PAGE_ID`). The playback ping is what lands a play in History.
    pub async fn report_tracking(&self, url: &str) -> Result<()> {
        let config = self.config().await;
        let request = self.inner.http.get(url);
        let response = self
            .with_session_headers(request, &config)
            .send()
            .await
            .map_err(network)?;
        self.absorb(&response);
        if response.status().is_success() {
            Ok(())
        } else {
            Err(ApiError::Network(format!(
                "playback report answered {}",
                response.status()
            )))
        }
    }

    /// The unparsed response of any `WEB_REMIX` endpoint, with the context
    /// and session headers filled in. For recording fixtures.
    pub async fn raw(&self, endpoint: &str, body: Value) -> Result<Value> {
        self.post(endpoint, body).await
    }

    /// The `player` body [`Client::playback_tracking`] sends, for recording.
    pub async fn player_body(&self, video_id: &str) -> Value {
        json!({
            "videoId": video_id,
            "playbackContext": { "contentPlaybackContext": {
                "signatureTimestamp": self.signature_timestamp().await,
                "html5Preference": "HTML5_PREF_WANTS",
            }},
            "contentCheckOk": true,
            "racyCheckOk": true,
        })
    }

    /// The session's cookies as they stand now, `Set-Cookie`s taken in.
    pub fn cookies(&self) -> Option<String> {
        self.inner.session.as_ref().map(Session::cookie)
    }

    /// Cookies another client of the same session (yt-dlp) was handed.
    pub fn merge_cookies(&self, pairs: &[(String, String)]) {
        if let Some(session) = &self.inner.session {
            session.merge(pairs);
        }
    }

    /// What the web app does every few minutes to keep its session from
    /// lapsing: `verify_session`, which answers with fresh `SIDCC` cookies.
    pub async fn keepalive(&self) -> Result<()> {
        self.require_session()?;
        let config = self.config().await;
        let request = self.inner.http.get(format!("{ORIGIN}/verify_session"));
        let response = self
            .with_session_headers(request, &config)
            .send()
            .await
            .map_err(network)?;
        self.absorb(&response);
        match response.status().as_u16() {
            200..=299 => Ok(()),
            401 | 403 => Err(ApiError::SignedOut),
            status => Err(ApiError::Network(format!(
                "verify_session answered {status}"
            ))),
        }
    }

    // Plumbing

    fn absorb(&self, response: &reqwest::Response) {
        if let Some(session) = &self.inner.session {
            session.absorb(response);
        }
    }

    fn require_session(&self) -> Result<&Session> {
        self.inner.session.as_ref().ok_or(ApiError::SignedOut)
    }

    async fn post(&self, endpoint: &str, body: Value) -> Result<Value> {
        self.post_as(ClientName::WebRemix, endpoint, body).await
    }

    async fn post_as(&self, client: ClientName, endpoint: &str, mut body: Value) -> Result<Value> {
        let config = self.config().await;
        let mut context_client = match client {
            ClientName::WebRemix => {
                json!({ "clientName": "WEB_REMIX", "clientVersion": config.client_version })
            }
            ClientName::AndroidMusic => json!({
                "clientName": "ANDROID_MUSIC",
                "clientVersion": ANDROID_MUSIC_VERSION,
                "androidSdkVersion": 34,
            }),
        };
        context_client["hl"] = LANGUAGE.into();
        context_client["gl"] = LOCATION.into();
        if let Some(visitor) = &config.visitor_data {
            context_client["visitorData"] = visitor.as_str().into();
        }
        body["context"] = json!({ "client": context_client });
        if let Some(page_id) = self
            .inner
            .session
            .as_ref()
            .and_then(|s| s.page_id.as_deref())
        {
            body["context"]["user"] = json!({ "onBehalfOfUser": page_id });
        }

        let url = format!("{API_BASE}{endpoint}");
        let mut request = self
            .inner
            .http
            .post(url)
            .query(&[("prettyPrint", "false")])
            .json(&body);
        request = match client {
            ClientName::WebRemix => self.with_session_headers(request, &config),
            ClientName::AndroidMusic => request.header(
                reqwest::header::USER_AGENT,
                format!("com.google.android.apps.youtube.music/{ANDROID_MUSIC_VERSION} (Linux; U; Android 14) gzip"),
            ),
        };

        let response = request.send().await.map_err(network)?;
        self.absorb(&response);
        let status = response.status();
        let bytes = response.bytes().await.map_err(network)?;
        let json: Option<Value> = serde_json::from_slice(&bytes).ok();
        if status.is_success() {
            return json.ok_or_else(|| {
                ApiError::Parse(format!("{endpoint} answered something that is not JSON"))
            });
        }
        let message = json
            .as_ref()
            .and_then(|j| j["error"]["message"].as_str())
            .unwrap_or("no message")
            .to_owned();
        tracing::warn!(endpoint, %status, message, "innertube request failed");
        Err(match status.as_u16() {
            401 | 403 => ApiError::SignedOut,
            404 => ApiError::NotFound(format!("{endpoint}: {message}")),
            400..=499 => ApiError::BadRequest(format!("{endpoint}: {message}")),
            _ => ApiError::Network(format!("{endpoint}: HTTP {status}: {message}")),
        })
    }

    fn with_session_headers(
        &self,
        request: reqwest::RequestBuilder,
        config: &WebConfig,
    ) -> reqwest::RequestBuilder {
        let mut request = request
            .header("Origin", ORIGIN)
            .header("X-Origin", ORIGIN)
            .header("Referer", format!("{ORIGIN}/"));
        if let Some(visitor) = &config.visitor_data {
            request = request.header("X-Goog-Visitor-Id", visitor);
        }
        request = request.header(reqwest::header::COOKIE, self.cookie_header());
        let Some(session) = &self.inner.session else {
            return request;
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        request = request
            .header(reqwest::header::AUTHORIZATION, session.authorization(now))
            .header("X-Goog-AuthUser", "0");
        if let Some(page_id) = &session.page_id {
            request = request.header("X-Goog-PageId", page_id);
        }
        request
    }

    fn cookie_header(&self) -> String {
        match &self.inner.session {
            Some(session) => {
                let cookie = session.cookie();
                if cookie_value(&cookie, "SOCS").is_some() {
                    cookie
                } else {
                    format!("{}; {CONSENT_COOKIE}", cookie.trim_end_matches(';'))
                }
            }
            None => CONSENT_COOKIE.to_owned(),
        }
    }

    /// The web app config, read from music.youtube.com once. A failed read
    /// falls back to known values and is retried on the next request.
    async fn config(&self) -> WebConfig {
        if let Some(config) = self.inner.config.get() {
            return config.clone();
        }
        match self.fetch_config().await {
            Ok(config) => self
                .inner
                .config
                .get_or_init(|| async { config })
                .await
                .clone(),
            Err(err) => {
                tracing::warn!(%err, "could not read the music.youtube.com config, using fallbacks");
                WebConfig {
                    client_version: FALLBACK_CLIENT_VERSION.into(),
                    visitor_data: None,
                    player_js: None,
                }
            }
        }
    }

    async fn fetch_config(&self) -> Result<WebConfig> {
        let html = self
            .inner
            .http
            .get(ORIGIN)
            .header(reqwest::header::ACCEPT_LANGUAGE, "en-US,en")
            .header(reqwest::header::COOKIE, self.cookie_header())
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(network)?
            .text()
            .await
            .map_err(network)?;
        let client_version = ytcfg_value(&html, "INNERTUBE_CLIENT_VERSION")
            .ok_or_else(|| missing("ytcfg INNERTUBE_CLIENT_VERSION"))?;
        Ok(WebConfig {
            client_version,
            visitor_data: ytcfg_value(&html, "VISITOR_DATA"),
            player_js: ytcfg_value(&html, "jsUrl"),
        })
    }

    async fn signature_timestamp(&self) -> u64 {
        if let Some(sts) = self.inner.signature_timestamp.get() {
            return *sts;
        }
        match self.fetch_signature_timestamp().await {
            Ok(sts) => {
                *self
                    .inner
                    .signature_timestamp
                    .get_or_init(|| async { sts })
                    .await
            }
            Err(err) => {
                tracing::warn!(%err, "could not read the player script, using a fallback signature timestamp");
                FALLBACK_SIGNATURE_TIMESTAMP
            }
        }
    }

    async fn fetch_signature_timestamp(&self) -> Result<u64> {
        let path = self
            .config()
            .await
            .player_js
            .ok_or_else(|| missing("ytcfg jsUrl"))?;
        let script = self
            .inner
            .http
            .get(format!("{ORIGIN}{path}"))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(network)?
            .text()
            .await
            .map_err(network)?;
        let start = script
            .find("signatureTimestamp:")
            .ok_or_else(|| missing("signatureTimestamp in player script"))?;
        let digits: String = script[start + "signatureTimestamp:".len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        digits
            .parse()
            .map_err(|_| missing("signatureTimestamp value"))
    }
}

/// A `"KEY":"value"` string from the inline `ytcfg.set({...})` blob.
fn ytcfg_value(html: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":\"");
    let start = html.find(&needle)? + needle.len();
    let end = html[start..].find('"')?;
    let raw = html[start..start + end]
        .replace("\\/", "/")
        .replace("\\u0026", "&");
    Some(decode_percent(&raw)).filter(|v| !v.is_empty())
}

fn network(err: reqwest::Error) -> ApiError {
    ApiError::Network(err.to_string())
}

fn check_status(json: &Value) -> Result<()> {
    match json["status"].as_str() {
        None | Some("STATUS_SUCCEEDED") => Ok(()),
        Some(status) => Err(ApiError::BadRequest(format!("YouTube answered {status}"))),
    }
}

fn privacy_name(privacy: Privacy) -> &'static str {
    match privacy {
        Privacy::Public => "PUBLIC",
        Privacy::Unlisted => "UNLISTED",
        Privacy::Private => "PRIVATE",
    }
}

fn edit_action(edit: &PlaylistEdit) -> Value {
    match edit {
        PlaylistEdit::Add { video_id } => {
            json!({ "action": "ACTION_ADD_VIDEO", "addedVideoId": video_id })
        }
        PlaylistEdit::AddPlaylist { playlist_id } => {
            json!({ "action": "ACTION_ADD_PLAYLIST", "addedFullListId": bare_playlist_id(playlist_id) })
        }
        PlaylistEdit::Remove {
            video_id,
            set_video_id,
        } => {
            json!({ "action": "ACTION_REMOVE_VIDEO", "removedVideoId": video_id, "setVideoId": set_video_id })
        }
        PlaylistEdit::Move {
            set_video_id,
            before_set_video_id,
        } => {
            let mut action =
                json!({ "action": "ACTION_MOVE_VIDEO_BEFORE", "setVideoId": set_video_id });
            if let Some(successor) = before_set_video_id {
                action["movedSetVideoIdSuccessor"] = successor.as_str().into();
            }
            action
        }
        PlaylistEdit::Rename { title } => {
            json!({ "action": "ACTION_SET_PLAYLIST_NAME", "playlistName": title })
        }
        PlaylistEdit::Describe { description } => {
            json!({ "action": "ACTION_SET_PLAYLIST_DESCRIPTION", "playlistDescription": description })
        }
        PlaylistEdit::SetPrivacy { privacy } => {
            json!({ "action": "ACTION_SET_PLAYLIST_PRIVACY", "playlistPrivacy": privacy_name(*privacy) })
        }
    }
}

/// The 16-character client playback nonce the web player makes up per play.
/// Every tracking ping of one play carries the same one.
pub fn client_playback_nonce() -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let random = RandomState::new();
    (0..16u64)
        .map(|i| ALPHABET[(random.hash_one(i) & 63) as usize] as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ytcfg_values() {
        let html = r#"ytcfg.set({"INNERTUBE_CLIENT_VERSION":"1.20261004.17.00","VISITOR_DATA":"Cgt%3D%3D","jsUrl":"\/s\/player\/x\/base.js"});"#;
        assert_eq!(
            ytcfg_value(html, "INNERTUBE_CLIENT_VERSION").as_deref(),
            Some("1.20261004.17.00")
        );
        assert_eq!(ytcfg_value(html, "VISITOR_DATA").as_deref(), Some("Cgt=="));
        assert_eq!(
            ytcfg_value(html, "jsUrl").as_deref(),
            Some("/s/player/x/base.js")
        );
        assert_eq!(ytcfg_value(html, "MISSING"), None);
    }

    #[test]
    fn edits_become_actions() {
        let action = edit_action(&PlaylistEdit::Move {
            set_video_id: "a".into(),
            before_set_video_id: Some("b".into()),
        });
        assert_eq!(
            action,
            json!({"action": "ACTION_MOVE_VIDEO_BEFORE", "setVideoId": "a", "movedSetVideoIdSuccessor": "b"})
        );
        let action = edit_action(&PlaylistEdit::AddPlaylist {
            playlist_id: "VLPL1".into(),
        });
        assert_eq!(action["addedFullListId"], "PL1");
    }

    #[test]
    fn nonce_shape() {
        let cpn = client_playback_nonce();
        assert_eq!(cpn.len(), 16);
        assert!(
            cpn.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
    }

    #[tokio::test]
    async fn mutations_need_a_session() {
        let client = Client::anonymous().unwrap();
        let err = client.delete_playlist("PL1").await.unwrap_err();
        assert!(matches!(err, ApiError::SignedOut));
        assert!(matches!(
            client.session().await,
            Ok(SessionInfo {
                signed_in: false,
                ..
            })
        ));
    }
}
