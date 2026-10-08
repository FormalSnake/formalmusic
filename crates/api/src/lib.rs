//! The contract between `formalmusicd` and its clients.
//!
//! Wire format: one JSON object per line over a Unix socket (a named pipe on
//! Windows) at [`socket_path`]. A client writes [`Request`]s; the daemon writes
//! [`ServerMessage`]s, which are either the [`Response`] to a request (matched
//! by `id`) or an unsolicited [`Event`] once the client has sent
//! [`Command::Subscribe`]. JSON lines keep the daemon scriptable from a shell
//! (`socat - UNIX-CONNECT:...`) and from bar widgets in any language.

mod equalizer;
#[cfg(feature = "io")]
pub mod local;
mod model;
pub mod process;
#[cfg(feature = "io")]
pub mod wire;

pub use equalizer::*;
pub use model::*;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Bumped on any breaking change to the types in this crate. The client
/// refuses to talk to a daemon with a different number, since both ship in
/// the same package and a mismatch means a stale daemon is still running.
pub const PROTOCOL_VERSION: u32 = 2;

/// `$XDG_RUNTIME_DIR/formalmusic/formalmusicd.sock`, or the cache dir on macOS
/// where there is no runtime dir. On Windows a named pipe per user.
pub fn socket_path() -> PathBuf {
    if let Ok(path) = std::env::var("FORMALMUSIC_SOCKET") {
        return path.into();
    }
    if cfg!(windows) {
        let user = std::env::var("USERNAME").unwrap_or_default();
        return format!(r"\\.\pipe\formalmusicd-{user}").into();
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
    /// outcome. `app` sets the API account first, kept only if Last.fm
    /// accepts it.
    ConnectLastFm {
        app: Option<LastFmApp>,
    },
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
    /// A video-only stream of `video_id` no taller than `max_height`, from
    /// the same yt-dlp run as its audio. `refresh` resolves it again, after
    /// googlevideo refused the last URL.
    VideoStream {
        video_id: String,
        max_height: u32,
        #[serde(default)]
        refresh: bool,
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
    /// Save a song to the library or remove it, with the
    /// [`LibraryToggle`] token for that direction.
    SetSongInLibrary {
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
    /// Song or Video for every track that has both; the playing one
    /// switches at the same place in the song.
    SetMode {
        mode: PlaybackMode,
    },
    PlayerState,
    QueueState,

    // Desktop
    /// Read the playback settings again from the app's `config.json`, which
    /// the Settings dialog just wrote.
    ReloadSettings,
    /// Show or hide the tray icon, which the daemon owns so it outlives the
    /// window. Linux only; elsewhere the daemon takes it and does nothing.
    SetTray {
        shown: bool,
    },
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
    VideoStream(VideoStream),
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
    /// The daemon is exiting because the user quit FormalMusic (the tray's
    /// Quit). Windows close instead of reconnecting or starting a new one.
    Quit,
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
            like: Some(Rating::Like),
            set_video_id: None,
            plays: None,
            feedback_token: None,
            library: None,
            counterpart: None,
        };
        let suggestion = Suggestion::Item(Item::Track(track));
        let json = serde_json::to_string(&suggestion).unwrap();
        assert_eq!(
            serde_json::from_str::<Suggestion>(&json).unwrap(),
            suggestion
        );
    }

    fn album_track() -> Track {
        Track {
            video_id: "song".into(),
            title: "t".into(),
            artists: vec![],
            album: None,
            duration_ms: Some(222_000),
            thumbnails: vec![],
            explicit: false,
            kind: TrackKind::Song,
            like: None,
            set_video_id: None,
            plays: None,
            feedback_token: None,
            library: None,
            counterpart: Some(Box::new(Counterpart {
                video_id: "video".into(),
                kind: TrackKind::Video,
                thumbnails: vec![],
                duration_ms: Some(260_000),
                segments: vec![
                    SharedSegment {
                        start_ms: 0,
                        counterpart_start_ms: 22_498,
                        duration_ms: 127_378,
                    },
                    SharedSegment {
                        start_ms: 127_881,
                        counterpart_start_ms: 153_000,
                        duration_ms: 69_884,
                    },
                ],
            })),
        }
    }

    #[test]
    fn modes_pick_the_version() {
        let track = album_track();
        assert_eq!(track.version(PlaybackMode::Song), "song");
        assert_eq!(track.version(PlaybackMode::Video), "video");
        assert_eq!(track.video_version(), Some("video"));
        let mut lone = track.clone();
        lone.counterpart = None;
        assert_eq!(lone.version(PlaybackMode::Video), "song");
        assert_eq!(lone.video_version(), None);
        lone.kind = TrackKind::Video;
        assert_eq!(lone.version(PlaybackMode::Song), "song");
        assert_eq!(lone.video_version(), Some("song"));
    }

    #[test]
    fn positions_map_through_the_shared_stretches() {
        let track = album_track();
        assert_eq!(track.map_position("song", "video", 10_000), 32_498);
        assert_eq!(track.map_position("song", "video", 130_000), 155_119);
        assert_eq!(track.map_position("video", "song", 32_498), 10_000);
        // The video's intro has no song under it: the song starts over.
        assert_eq!(track.map_position("video", "song", 5_000), 0);
        assert_eq!(track.map_position("song", "song", 7), 7);
        assert_eq!(track.map_position("song", "elsewhere", 7), 7);
    }

    #[test]
    fn older_player_states_still_parse() {
        let state: PlayerState = serde_json::from_str(
            r#"{"status":"playing","track":null,"position_ms":0,"duration_ms":null,"volume":1.0,"muted":false,"repeat":"off","shuffle":false,"stream":null}"#,
        )
        .unwrap();
        assert_eq!((state.mode, state.playing_id), (PlaybackMode::Song, None));
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
