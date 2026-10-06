//! Data model. It follows YouTube Music's own page shapes (shelves, chips,
//! header variants) instead of flattening everything to tracks, so the client
//! can lay a page out the way the web app does.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum BrowseTarget {
    Home,
    /// Home filtered by a mood chip; `params` comes from [`Chip::params`].
    HomeChip { params: String },
    Explore,
    NewReleases,
    Charts,
    MoodsAndGenres,
    /// One mood or genre tile from [`BrowseTarget::MoodsAndGenres`].
    MoodCategory { params: String },
    Library(LibraryTab),
    History,
    Album(String),
    Artist(String),
    Playlist(String),
    Podcast(String),
    Episode(String),
    /// An artist's "see all" page (all songs, albums, singles, videos).
    ArtistShelf { browse_id: String, params: String },
    /// Any other `browseId` + `params` pair a page linked to.
    Raw { browse_id: String, params: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryTab {
    Playlists,
    Songs,
    Albums,
    Artists,
    Subscriptions,
    Podcasts,
    Uploads,
    LikedSongs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LibraryScope {
    Likes,
    Playlists,
    Albums,
    Subscriptions,
    History,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Continuation(pub String);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Thumbnail {
    pub url: String,
    pub width: u32,
    pub height: u32,
}

/// Every size YouTube offered, smallest first. Clients pick with [`best`].
pub type Thumbnails = Vec<Thumbnail>;

/// The smallest thumbnail at least `min_px` wide, or the largest there is.
pub fn best(thumbnails: &[Thumbnail], min_px: u32) -> Option<&Thumbnail> {
    thumbnails
        .iter()
        .find(|t| t.width >= min_px)
        .or_else(|| thumbnails.last())
}

/// A name that may link somewhere, like an artist in a byline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub text: String,
    pub target: Option<BrowseTarget>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub video_id: String,
    pub title: String,
    pub artists: Vec<Link>,
    pub album: Option<Link>,
    pub duration_ms: Option<u64>,
    pub thumbnails: Thumbnails,
    pub explicit: bool,
    /// Music videos and user uploads to YouTube rather than album tracks.
    /// Not `kind` on the wire: `Item` is tagged by `kind`, and a track inside
    /// it would write the key twice.
    #[serde(rename = "track_kind")]
    pub kind: TrackKind,
    pub like: Rating,
    /// Present in playlists you own; needed to remove or move the row.
    pub set_video_id: Option<String>,
    /// "1.2M plays" and similar subtitles the page showed.
    pub plays: Option<String>,
    /// From the History page, needed for [`crate::Command::RemoveFromHistory`].
    pub feedback_token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    #[default]
    Song,
    Video,
    Episode,
    Upload,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Item {
    Track(Track),
    Album {
        browse_id: String,
        /// The `OLAK5uy_...` playlist that plays the album.
        playlist_id: Option<String>,
        title: String,
        /// "Album", "Single", "EP".
        album_type: Option<String>,
        artists: Vec<Link>,
        year: Option<String>,
        thumbnails: Thumbnails,
        explicit: bool,
    },
    Artist {
        browse_id: String,
        name: String,
        subtitle: Option<String>,
        thumbnails: Thumbnails,
    },
    Playlist {
        playlist_id: String,
        title: String,
        /// "Playlist • YouTube Music • 50 songs"
        subtitle: Option<String>,
        thumbnails: Thumbnails,
    },
    Podcast {
        browse_id: String,
        title: String,
        subtitle: Option<String>,
        thumbnails: Thumbnails,
    },
    /// A mood or genre tile.
    Mood {
        title: String,
        params: String,
        /// The tile's accent colour from YouTube, as `0xAARRGGBB`.
        color: Option<u32>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub title: Option<String>,
    /// Small caption over the title ("Similar to", an artist name).
    pub strapline: Option<String>,
    pub layout: SectionLayout,
    pub items: Vec<Item>,
    /// Where "More" goes, if the shelf has one.
    pub more: Option<BrowseTarget>,
    pub continuation: Option<Continuation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionLayout {
    /// Horizontal row of square cards.
    Carousel,
    /// The "Quick picks" grid of track rows, four rows per column.
    TrackGrid,
    /// Vertical track list, as on album and playlist pages.
    List,
    /// Wrapping grid of tiles (moods, library albums).
    Grid,
    /// One large card ("Top result" in search).
    Hero,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chip {
    pub title: String,
    pub params: String,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Header {
    /// Album, single, playlist, podcast: artwork left, actions under the title.
    Detail {
        title: String,
        subtitle: Vec<Link>,
        second_subtitle: Option<String>,
        description: Option<String>,
        thumbnails: Thumbnails,
        /// Playlist to hand to Play for the whole page.
        playlist_id: Option<String>,
        /// Present when you own the playlist.
        editable: bool,
        saved: Option<bool>,
        privacy: Option<Privacy>,
    },
    /// Artist: full-bleed banner.
    Artist {
        name: String,
        description: Option<String>,
        thumbnails: Thumbnails,
        channel_id: Option<String>,
        subscribed: Option<bool>,
        subscribers: Option<String>,
        /// "Shuffle" and "Radio" playlist ids.
        shuffle_playlist_id: Option<String>,
        radio_playlist_id: Option<String>,
        monthly_listeners: Option<String>,
    },
    /// A plain title, as on Explore sub-pages and moods.
    Title { title: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page {
    pub target: BrowseTarget,
    pub header: Option<Header>,
    pub chips: Vec<Chip>,
    pub sections: Vec<Section>,
    /// More sections for the page (Home keeps loading as you scroll).
    pub continuation: Option<Continuation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContinuationPage {
    /// Further sections for a page, or further items for one section; the
    /// client knows which it asked for.
    pub sections: Vec<Section>,
    pub items: Vec<Item>,
    pub continuation: Option<Continuation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchFilter {
    Songs,
    Videos,
    Albums,
    Artists,
    CommunityPlaylists,
    FeaturedPlaylists,
    Podcasts,
    Episodes,
    Profiles,
    /// Your own library only.
    Library,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResults {
    pub query: String,
    pub filter: Option<SearchFilter>,
    /// "Did you mean" or "Showing results for".
    pub correction: Option<String>,
    pub sections: Vec<Section>,
    pub continuation: Option<Continuation>,
}

/// Tagged by `type`, since the `Item` inside is tagged by `kind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Suggestion {
    Query { text: String, from_history: bool },
    Item(Item),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lyrics {
    pub source: Option<String>,
    pub lines: Vec<LyricLine>,
    /// False when only plain text was available; `start_ms` is then zero.
    pub synced: bool,
    /// Some line carries per-word or per-syllable timing in
    /// [`LyricLine::words`].
    #[serde(default)]
    pub word_synced: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LyricLine {
    pub start_ms: u64,
    pub end_ms: Option<u64>,
    pub text: String,
    /// Empty when the source only timed the line as a whole.
    #[serde(default)]
    pub words: Vec<LyricWord>,
    /// Backing vocals, drawn smaller and lit alongside the main line.
    #[serde(default)]
    pub background: bool,
    /// Who sings the line in a duet, as the provider names the voice ("v1").
    #[serde(default)]
    pub agent: Option<String>,
    /// The line belongs on the other side of the lane from the first singer.
    #[serde(default)]
    pub opposite_turn: bool,
}

/// One timed chunk of a line. Providers that stamp syllables yield several
/// chunks per word; `joins_next` says the next chunk follows with no space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LyricWord {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    #[serde(default)]
    pub joins_next: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RateTarget {
    Track { video_id: String },
    Playlist { playlist_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rating {
    Like,
    Dislike,
    #[default]
    Indifferent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Privacy {
    Public,
    Unlisted,
    Private,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlaylistEdit {
    Add { video_id: String },
    /// Add every track of another playlist or album.
    AddPlaylist { playlist_id: String },
    Remove { video_id: String, set_video_id: String },
    /// Move a row so it sits before `before_set_video_id`, or last when `None`.
    Move { set_video_id: String, before_set_video_id: Option<String> },
    Rename { title: String },
    Describe { description: String },
    SetPrivacy { privacy: Privacy },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlaySource {
    /// Exactly these tracks, as on a page the client already has.
    Tracks { tracks: Vec<Track> },
    /// A playlist or album fetched in full by the daemon.
    Playlist { playlist_id: String },
    /// Start radio from one track (the web app's "Start radio").
    Radio { video_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnqueuePosition {
    /// "Play next".
    Next,
    /// "Add to queue".
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Stopped,
    /// Resolving or buffering before sound comes out.
    Loading,
    Playing,
    Paused,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PlayerState {
    pub status: Status,
    pub track: Option<Track>,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub volume: f32,
    pub muted: bool,
    pub repeat: Repeat,
    pub shuffle: bool,
    /// Codec and bitrate actually playing, such as "opus 160 kbps".
    pub stream: Option<String>,
    /// The `browseId` of the current track's "Related" tab, for
    /// [`crate::Command::Related`]. It comes with the `next` response.
    #[serde(default)]
    pub related_browse_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct QueueState {
    pub tracks: Vec<Track>,
    pub current: Option<usize>,
    /// Where the queue came from, for the "Playing from" caption.
    pub source_title: Option<String>,
    /// Radio is on: the daemon appends tracks as the queue runs out.
    pub radio: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SessionInfo {
    pub signed_in: bool,
    pub account: Option<Account>,
    /// Premium unlocks the higher bitrate streams.
    pub premium: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub name: String,
    pub handle: Option<String>,
    pub thumbnails: Thumbnails,
    /// `None` for the main Google account, else the brand account page id.
    pub page_id: Option<String>,
    pub selected: bool,
}
