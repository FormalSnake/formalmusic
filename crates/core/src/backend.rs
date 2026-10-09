//! What the store talks to: kopuzd through `kopuz-client`, or the demo set.

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::model::*;
use crate::settings::Settings;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ConnectionStatus {
    #[default]
    Connecting,
    Online,
    Offline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    Daemon,
    Demo,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ClientError {
    /// The request needs a signed-in session and there is none.
    #[error("not signed in")]
    SignedOut,
    #[error("not found: {0}")]
    NotFound(String),
    #[error("network: {0}")]
    Network(String),
    #[error("{0}")]
    BadRequest(String),
    /// The source cannot do this at all.
    #[error("not supported: {0}")]
    Unsupported(String),
    #[error("{0}")]
    Failed(String),
    /// No daemon, or it went away before the answer came.
    #[error("{0}")]
    Disconnected(String),
    #[error("the music daemon did not answer in time")]
    Timeout,
}

/// What the backend tells the store without being asked.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Connection {
        status: ConnectionStatus,
        error: Option<String>,
    },
    Player(PlayerState),
    /// At once on every play, pause, seek and track change. The store
    /// interpolates in between from [`PlayerState::status`].
    Position {
        position_ms: u64,
        buffered_ms: u64,
    },
    /// How far ahead of the position the track is downloaded.
    Buffered {
        buffered_ms: u64,
    },
    Queue(QueueState),
    Session(SessionInfo),
    Scrobbling(ScrobbleStatus),
    /// A like, subscription or playlist edit landed; cached pages of these
    /// kinds are dropped and the visible one fetched again.
    LibraryChanged {
        scope: LibraryScope,
    },
    /// A non-fatal failure the user should see.
    Notice {
        message: String,
    },
}

/// Transport and queue commands whose effect comes back as events.
#[derive(Clone, Debug, PartialEq)]
pub enum Control {
    Toggle,
    Pause,
    Next,
    Previous,
    Seek(u64),
    Volume(f32),
    Muted(bool),
    Repeat(Repeat),
    Shuffle(bool),
    Jump(usize),
    Remove(usize),
    Move {
        from: usize,
        to: usize,
    },
    Clear,
    /// Swaps the playing track for its other cut, at the same place.
    Version(PlaybackMode),
}

pub type Result<T> = std::result::Result<T, ClientError>;

#[async_trait]
pub trait Backend: Send + Sync {
    fn kind(&self) -> BackendKind;

    /// Starts connecting. Connection changes and events go to `events` for
    /// as long as the backend lives, the opening player and queue snapshots
    /// included.
    fn start(&self, events: mpsc::UnboundedSender<Event>);

    fn stop(&self);

    async fn session(&self) -> Result<SessionInfo>;

    // Pages
    async fn browse(&self, target: &BrowseTarget) -> Result<Page>;
    /// More of a page (`section` none) or of one of its sections.
    async fn more(
        &self,
        target: &BrowseTarget,
        section: Option<usize>,
        token: &Continuation,
    ) -> Result<ContinuationPage>;
    async fn search(&self, query: &str, filter: Option<SearchFilter>) -> Result<SearchResults>;
    async fn search_more(
        &self,
        query: &str,
        filter: Option<SearchFilter>,
        token: &Continuation,
    ) -> Result<ContinuationPage>;
    async fn suggestions(&self, query: &str) -> Result<Vec<Suggestion>>;
    /// The filters the source's search takes, in the order to offer them.
    fn search_filters(&self) -> Vec<SearchFilter>;
    async fn lyrics(&self, key: &str) -> Result<Option<Lyrics>>;
    /// The player's "Related" tab for a track.
    async fn related(&self, key: &str) -> Result<Page>;
    async fn artwork(&self, art: &Art, hq: bool) -> Result<Vec<u8>>;
    /// The web app's link for a track, an album or a page.
    async fn share_url(&self, item: &Item) -> Result<Option<String>>;
    /// What the source can do past browsing and playing.
    fn features(&self) -> Features;
    /// A byte range of the picture of the queued music video `key`.
    async fn video(&self, key: &str, start: u64, length: Option<u64>) -> Result<VideoChunk>;

    // Playback
    async fn play(&self, source: PlaySource, start_index: usize, shuffle: bool) -> Result<()>;
    async fn enqueue(&self, tracks: Vec<Track>, position: EnqueuePosition) -> Result<()>;
    async fn control(&self, control: Control) -> Result<()>;

    // Library
    async fn rate(&self, rate_ref: &str, rating: Rating) -> Result<()>;
    async fn follow(&self, follow_ref: &str, follow: bool) -> Result<()>;
    async fn save(&self, save_ref: &str, saved: bool) -> Result<()>;
    async fn remove_from_history(&self, token: &str) -> Result<()>;
    async fn create_playlist(&self, title: String, keys: Vec<String>) -> Result<String>;
    async fn add_to_playlist(&self, playlist_id: &str, keys: Vec<String>) -> Result<()>;
    async fn remove_from_playlist(&self, playlist_id: &str, index: usize) -> Result<()>;
    async fn move_in_playlist(&self, playlist_id: &str, from: usize, to: usize) -> Result<()>;
    /// Whether rows of this playlist, one the account owns, can be moved.
    fn playlist_reorders(&self, playlist_id: &str) -> bool;
    async fn edit_playlist(&self, playlist_id: &str, details: PlaylistDetails) -> Result<()>;
    async fn delete_playlist(&self, playlist_id: &str) -> Result<()>;

    // Session
    async fn sign_in(&self, cookies: String) -> Result<SessionInfo>;
    async fn browsers(&self) -> Result<Browsers>;
    async fn browser_sign_in(&self, browser: Option<String>) -> Result<SessionInfo>;
    async fn browser_profiles(&self) -> Result<Vec<ProfileBrowser>>;
    async fn import_profile(&self, profile: &str) -> Result<SessionInfo>;
    async fn sign_out(&self) -> Result<()>;
    async fn accounts(&self) -> Result<Vec<Account>>;
    async fn switch_account(&self, page_id: Option<String>) -> Result<SessionInfo>;

    // Scrobbling
    async fn scrobbling(&self) -> Result<ScrobbleStatus>;
    async fn connect_lastfm(&self, app: Option<LastFmApp>) -> Result<ScrobbleStatus>;
    async fn connect_listenbrainz(&self, token: String) -> Result<ScrobbleStatus>;
    async fn disconnect_scrobbler(&self, service: ScrobbleService) -> Result<ScrobbleStatus>;

    // Settings
    /// Hands the daemon's keys of `config.json` to it.
    async fn apply_settings(&self, settings: &Settings) -> Result<()>;
    /// Plays an equalizer setting without keeping it.
    async fn preview_equalizer(&self, equalizer: crate::equalizer::Equalizer) -> Result<()>;
}
