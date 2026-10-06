//! The contract between `formalmusicd` and its clients.
//!
//! Wire format: one JSON object per line over a Unix socket at
//! [`socket_path`]. A client writes [`Request`]s; the daemon writes
//! [`ServerMessage`]s, which are either the [`Response`] to a request (matched
//! by `id`) or an unsolicited [`Event`] once the client has sent
//! [`Command::Subscribe`]. JSON lines keep the daemon scriptable from a shell
//! (`socat - UNIX-CONNECT:...`) and from bar widgets in any language.

mod model;
#[cfg(feature = "io")]
pub mod wire;

pub use model::*;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Bumped on any breaking change to the types in this crate. The client
/// refuses to talk to a daemon with a different number, since both ship in
/// the same package and a mismatch means a stale daemon is still running.
pub const PROTOCOL_VERSION: u32 = 1;

/// `$XDG_RUNTIME_DIR/formalmusic/formalmusicd.sock`, or the cache dir on macOS
/// where there is no runtime dir.
pub fn socket_path() -> PathBuf {
    if let Ok(path) = std::env::var("FORMALMUSIC_SOCKET") {
        return path.into();
    }
    dirs::runtime_dir()
        .or_else(dirs::cache_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("formalmusic")
        .join("formalmusicd.sock")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub command: Command,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    Response(Response),
    Event(Event),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    #[serde(flatten)]
    pub result: ResponseResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseResult {
    Ok(Reply),
    Err(ApiError),
}

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum ApiError {
    /// The request needs a signed-in session and there is none.
    #[error("not signed in")]
    SignedOut,
    #[error("not found: {0}")]
    NotFound(String),
    /// YouTube answered with something the parser does not understand. The
    /// weekly maintenance run looks for these in the journal.
    #[error("unexpected response: {0}")]
    Parse(String),
    #[error("network: {0}")]
    Network(String),
    #[error("playback: {0}")]
    Playback(String),
    #[error("bad request: {0}")]
    BadRequest(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", content = "args", rename_all = "snake_case")]
pub enum Command {
    Hello {
        protocol: u32,
    },
    /// Start receiving [`Event`]s on this connection. The daemon answers with
    /// a full [`PlayerState`] and [`QueueState`] snapshot first.
    Subscribe,

    // Session
    Session,
    /// A `Cookie` header copied from a signed-in music.youtube.com tab, or
    /// produced by the sign-in window.
    SignIn {
        cookies: String,
    },
    /// Browsers on this machine that [`Command::BrowserSignIn`] can open.
    Browsers,
    /// Open `browser` (a [`Browser::id`], or the default) in a throwaway
    /// profile at Google's sign-in page and keep the cookies once the user is
    /// signed in. Answers with the session when done, which takes as long as
    /// the user does, up to five minutes.
    BrowserSignIn {
        browser: Option<String>,
    },
    /// Close the browser of a running [`Command::BrowserSignIn`], which then
    /// fails.
    CancelSignIn,
    /// Profiles of the browsers on this machine, for [`Command::ImportCookies`].
    BrowserProfiles,
    /// Take the YouTube session from a profile the user is already signed in
    /// to. The browser can stay open.
    ImportCookies {
        browser: String,
        profile: String,
    },
    SignOut,
    /// Brand accounts and channels under the signed-in Google account.
    Accounts,
    SwitchAccount {
        page_id: Option<String>,
    },

    // Scrobbling
    Scrobbling,
    /// Open Last.fm's "allow access" page in the default browser and wait for
    /// the user there in the background; [`Event::Scrobbling`] reports the
    /// outcome.
    ConnectLastFm,
    ConnectListenBrainz {
        source: ListenBrainzSource,
    },
    DisconnectScrobbler {
        service: ScrobbleService,
    },
    SetScrobbling {
        service: ScrobbleService,
        scrobble: bool,
        now_playing: bool,
    },

    // Browsing
    Browse {
        target: BrowseTarget,
    },
    /// More items for a shelf or page, from a [`Continuation`] token.
    Continue {
        token: Continuation,
    },
    Search {
        query: String,
        filter: Option<SearchFilter>,
    },
    Suggestions {
        query: String,
    },
    Lyrics {
        video_id: String,
    },
    /// Apple Music's looping album video, downloaded by the daemon.
    AnimatedCover {
        artist: String,
        album: String,
    },
    /// The "Related" tab of the player page.
    Related {
        browse_id: String,
    },

    // Library mutations
    Rate {
        target: RateTarget,
        rating: Rating,
    },
    /// Subscribe to or unsubscribe from an artist channel.
    SetSubscribed {
        channel_id: String,
        subscribed: bool,
    },
    CreatePlaylist {
        title: String,
        description: String,
        privacy: Privacy,
        video_ids: Vec<String>,
    },
    EditPlaylist {
        playlist_id: String,
        edits: Vec<PlaylistEdit>,
    },
    DeletePlaylist {
        playlist_id: String,
    },
    /// Save or remove an album or someone else's playlist from the library.
    SetInLibrary {
        playlist_id: String,
        saved: bool,
    },
    RemoveFromHistory {
        feedback_token: String,
    },

    // Playback
    /// Replace the queue and start playing `start_index`. With `radio` the
    /// daemon keeps extending the queue from `next` like the web app's autoplay.
    Play {
        source: PlaySource,
        start_index: usize,
        shuffle: bool,
        radio: bool,
    },
    Enqueue {
        tracks: Vec<Track>,
        position: EnqueuePosition,
    },
    RemoveFromQueue {
        index: usize,
    },
    MoveInQueue {
        from: usize,
        to: usize,
    },
    ClearQueue,
    JumpTo {
        index: usize,
    },
    Toggle,
    Pause,
    Resume,
    Next,
    Previous,
    SeekTo {
        position_ms: u64,
    },
    SetVolume {
        volume: f32,
    },
    SetMuted {
        muted: bool,
    },
    SetRepeat {
        repeat: Repeat,
    },
    SetShuffle {
        shuffle: bool,
    },
    PlayerState,
    QueueState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "reply", content = "data", rename_all = "snake_case")]
pub enum Reply {
    Ok,
    Hello {
        protocol: u32,
        version: String,
    },
    Session(SessionInfo),
    Browsers(Browsers),
    BrowserProfiles(Vec<ProfileBrowser>),
    Accounts(Vec<Account>),
    Scrobbling(ScrobbleStatus),
    Page(Page),
    Continuation(ContinuationPage),
    Search(SearchResults),
    Suggestions(Vec<Suggestion>),
    Lyrics(Option<Lyrics>),
    /// Local path to the mp4; the daemon and client share a machine.
    AnimatedCover(Option<String>),
    PlaylistCreated {
        playlist_id: String,
    },
    Player(PlayerState),
    Queue(QueueState),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum Event {
    Player(PlayerState),
    /// Sent about once a second while playing, and at once on every play,
    /// pause, seek and track change. Clients interpolate in between from
    /// [`PlayerState::status`].
    Position {
        position_ms: u64,
        buffered_ms: u64,
    },
    Queue(QueueState),
    Session(SessionInfo),
    Scrobbling(ScrobbleStatus),
    /// A like, subscription or playlist edit landed; clients drop cached pages
    /// of these kinds and refetch the visible one.
    LibraryChanged {
        scope: LibraryScope,
    },
    /// A non-fatal failure the user should see, such as a track that could not
    /// be resolved and was skipped.
    Notice {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_shape_is_stable() {
        let req = Request {
            id: 7,
            command: Command::SeekTo { position_ms: 1500 },
        };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(
            json,
            r#"{"id":7,"cmd":"seek_to","args":{"position_ms":1500}}"#
        );
        let back: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            back.command,
            Command::SeekTo { position_ms: 1500 }
        ));
    }

    #[test]
    fn unit_commands_need_no_args() {
        let req: Request = serde_json::from_str(r#"{"id":1,"cmd":"toggle"}"#).unwrap();
        assert!(matches!(req.command, Command::Toggle));
    }

    #[test]
    fn items_and_suggestions_round_trip() {
        let track = Track {
            video_id: "abc".into(),
            title: "t".into(),
            artists: vec![],
            album: None,
            duration_ms: None,
            thumbnails: vec![],
            explicit: false,
            kind: TrackKind::Video,
            like: Rating::Like,
            set_video_id: None,
            plays: None,
            feedback_token: None,
        };
        let suggestion = Suggestion::Item(Item::Track(track));
        let json = serde_json::to_string(&suggestion).unwrap();
        assert_eq!(
            serde_json::from_str::<Suggestion>(&json).unwrap(),
            suggestion
        );
    }

    #[test]
    fn response_round_trips() {
        let msg = ServerMessage::Response(Response {
            id: 3,
            result: ResponseResult::Err(ApiError::SignedOut),
        });
        let json = serde_json::to_string(&msg).unwrap();
        let back: ServerMessage = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            back,
            ServerMessage::Response(Response {
                id: 3,
                result: ResponseResult::Err(ApiError::SignedOut)
            })
        ));
    }
}
