//! What the window renders. It follows YouTube Music's own page shapes
//! (shelves, chips, header variants) rather than a flat track list, so a page
//! lays out the way the web app does. `crate::convert` fills it from kopuz's
//! wire types; it is serde so `state.json` can paint it before kopuzd answers,
//! and hashable where it keys a cache.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum BrowseTarget {
    Home,
    /// Home filtered by a mood chip, by the chip's page id.
    HomeChip(String),
    Explore,
    NewReleases,
    Charts,
    MoodsAndGenres,
    Library(LibraryTab),
    History,
    Album(String),
    Artist(String),
    Playlist(String),
    Podcast(String),
    Episode(String),
    /// One mood or genre tile.
    Mood(String),
    /// Any other page the source handed out an id for: a shortcut, a
    /// shelf's "more", an artist's "see all".
    Page(String),
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
    /// Songs saved to or removed from the library.
    Songs,
    Playlists,
    Albums,
    Subscriptions,
    History,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Continuation(pub String);

/// A picture kopuzd resolves, by what it belongs to. `version` changes
/// exactly when the picture does, so it is the cache key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Art {
    pub kind: ArtKind,
    pub id: String,
    pub version: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtKind {
    Track,
    Album,
    Artist,
    Playlist,
    Catalog,
    Station,
    /// The signed-in account's picture, by the source's id.
    Account,
    /// Painted locally from `id` as a seed, for the demo set.
    Demo,
}

/// A name that may link somewhere, like an artist in a byline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub text: String,
    pub target: Option<BrowseTarget>,
}

/// What the account has done to an entity and the refs that change it,
/// passed back to kopuzd as they came.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Actions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<Rating>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub follow_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub followed: Option<bool>,
    /// Only a row of the listening history has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    /// kopuz's key for the row, which every queue and library call takes.
    pub key: String,
    pub title: String,
    pub artists: Vec<Link>,
    pub album: Option<Link>,
    pub duration_ms: Option<u64>,
    pub art: Option<Art>,
    #[serde(default)]
    pub explicit: bool,
    /// Not `kind` on the wire: `Item` is tagged by `kind`, and a track inside
    /// it would write the key twice.
    #[serde(rename = "track_kind")]
    pub kind: TrackKind,
    /// "1.2M plays" and similar subtitles the page showed.
    #[serde(default)]
    pub plays: Option<String>,
    /// Rating, library state and history token, where the page said.
    #[serde(default)]
    pub actions: Actions,
    /// The music video of an album track, or the album track of a music
    /// video, which the Song and Video switch swaps the playing row for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counterpart: Option<Box<Counterpart>>,
}

/// The other cut of a [`Track`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counterpart {
    pub key: String,
    /// The counterpart's own cut; the row carrying it is the other one.
    pub version: PlaybackMode,
    pub duration_ms: Option<u64>,
}

/// Which cut of a recording plays: the album track, or the music video with
/// its own sound and picture. One choice for the whole queue, as in the web
/// app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackMode {
    #[default]
    Song,
    Video,
}

impl Track {
    /// Which cut this row is, known only when it has a counterpart.
    pub fn version(&self) -> Option<PlaybackMode> {
        self.counterpart.as_ref().map(|other| match other.version {
            PlaybackMode::Song => PlaybackMode::Video,
            PlaybackMode::Video => PlaybackMode::Song,
        })
    }
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
        title: String,
        /// "Album", "Single", "EP".
        album_type: Option<String>,
        artists: Vec<Link>,
        year: Option<String>,
        art: Option<Art>,
        explicit: bool,
        #[serde(default)]
        actions: Actions,
    },
    Artist {
        browse_id: String,
        name: String,
        subtitle: Option<String>,
        art: Option<Art>,
        #[serde(default)]
        actions: Actions,
    },
    Playlist {
        playlist_id: String,
        title: String,
        /// "Playlist • YouTube Music • 50 songs"
        subtitle: Option<String>,
        art: Option<Art>,
        #[serde(default)]
        actions: Actions,
        /// The web app's link to it, for Share.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        web_url: Option<String>,
    },
    Podcast {
        browse_id: String,
        title: String,
        subtitle: Option<String>,
        art: Option<Art>,
        #[serde(default)]
        actions: Actions,
    },
    /// A mood or genre tile.
    Mood {
        title: String,
        id: String,
        /// The tile's accent colour, as `0xRRGGBB`.
        color: Option<u32>,
    },
    /// A button to another page, as Explore's New releases, Charts and
    /// Moods & genres.
    Shortcut {
        title: String,
        target: BrowseTarget,
        icon: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Section {
    pub title: Option<String>,
    /// Small caption over the title ("Similar to", an artist name).
    pub strapline: Option<String>,
    pub layout: SectionLayout,
    pub items: Vec<Item>,
    /// Where "More" goes, if the shelf has one.
    pub more: Option<BrowseTarget>,
    pub continuation: Option<Continuation>,
    /// On a search's All tab, the filter whose chip shows every result of
    /// this shelf's kind, for its "Show all".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<SearchFilter>,
    /// The search top result's Shuffle and Mix buttons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shuffle: Option<PlaySource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radio: Option<PlaySource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SectionLayout {
    /// Horizontal row of square cards.
    Carousel,
    /// The "Quick picks" grid of track rows, four rows per column.
    TrackGrid,
    /// Vertical track list, as on album and playlist pages.
    #[default]
    List,
    /// Wrapping grid of tiles (moods, library albums).
    Grid,
    /// One large card ("Top result" in search).
    Hero,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chip {
    pub title: String,
    /// The page the chip opens.
    pub id: String,
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
        art: Option<Art>,
        /// What Play and Shuffle start for the whole page.
        play: Option<PlaySource>,
        /// You own the playlist.
        editable: bool,
        privacy: Option<Privacy>,
        #[serde(default)]
        actions: Actions,
    },
    /// Artist: full-bleed banner.
    Artist {
        name: String,
        description: Option<String>,
        art: Option<Art>,
        subscribers: Option<String>,
        monthly_listeners: Option<String>,
        shuffle: Option<PlaySource>,
        radio: Option<PlaySource>,
        #[serde(default)]
        actions: Actions,
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

/// kopuzd names a filter by the same snake_case id serde writes here.
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

impl SearchFilter {
    pub const ALL: [SearchFilter; 10] = [
        SearchFilter::Songs,
        SearchFilter::Videos,
        SearchFilter::Albums,
        SearchFilter::Artists,
        SearchFilter::CommunityPlaylists,
        SearchFilter::FeaturedPlaylists,
        SearchFilter::Podcasts,
        SearchFilter::Episodes,
        SearchFilter::Profiles,
        SearchFilter::Library,
    ];

    pub fn id(self) -> &'static str {
        match self {
            SearchFilter::Songs => "songs",
            SearchFilter::Videos => "videos",
            SearchFilter::Albums => "albums",
            SearchFilter::Artists => "artists",
            SearchFilter::CommunityPlaylists => "community_playlists",
            SearchFilter::FeaturedPlaylists => "featured_playlists",
            SearchFilter::Podcasts => "podcasts",
            SearchFilter::Episodes => "episodes",
            SearchFilter::Profiles => "profiles",
            SearchFilter::Library => "library",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|filter| filter.id() == id)
    }
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

/// A playlist's name, description and privacy, each `None` to keep.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlaylistDetails {
    pub title: Option<String>,
    pub description: Option<String>,
    pub privacy: Option<Privacy>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlaySource {
    /// Exactly these tracks, as on a page the client already has.
    Tracks { tracks: Vec<Track> },
    /// An album or a playlist whole, which kopuzd resolves by its id.
    Page { target: BrowseTarget },
    /// Start radio from one track (the web app's "Start radio").
    Radio { key: String },
    /// A mix made from a playlist.
    PlaylistRadio { id: String },
    /// Every song kopuzd finds by an artist.
    Artist { key: String },
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
    /// Bitrate actually playing, such as "160 kbps".
    pub stream: Option<String>,
    /// The Song and Video switch, which the store holds for the queue.
    #[serde(default)]
    pub mode: PlaybackMode,
    /// How far the sound runs behind the position, for a picture kept to it.
    #[serde(default)]
    pub output_latency_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct QueueState {
    pub tracks: Vec<Track>,
    pub current: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SessionInfo {
    pub signed_in: bool,
    pub account: Option<Account>,
    /// Premium unlocks the higher bitrate streams.
    pub premium: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Browser {
    /// Stable id for a browser sign-in, such as `firefox`.
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Browsers {
    pub installed: Vec<Browser>,
    /// The id a browser sign-in uses when given none.
    pub default: Option<String>,
}

/// A browser and its profiles signed in to YouTube Music.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileBrowser {
    pub browser: Browser,
    pub profiles: Vec<BrowserProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserProfile {
    /// What kopuzd takes the session by.
    pub path: String,
    pub name: String,
    /// The Google account the browser itself is signed in to, when it says.
    pub email: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub name: String,
    pub handle: Option<String>,
    pub art: Option<Art>,
    /// `None` for the signed-in account itself, else the one to switch to.
    pub page_id: Option<String>,
    pub selected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrobbleService {
    LastFm,
    ListenBrainz,
}

/// A Last.fm API account: Last.fm signs every call with both, and gives
/// each user their own at last.fm/api/account/create.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastFmApp {
    pub api_key: String,
    pub shared_secret: String,
}

impl std::fmt::Debug for LastFmApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LastFmApp").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ScrobbleStatus {
    pub lastfm: ScrobbleAccount,
    pub listenbrainz: ScrobbleAccount,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ScrobbleAccount {
    pub connected: bool,
    /// Waiting for the user to allow access in the browser.
    pub connecting: bool,
}

/// What the source can do past browsing and playing, which decides the
/// settings and switches the window offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Features {
    /// It picks among several formats per track by the audio quality.
    pub stream_quality: bool,
    /// Its rows say which are explicit, so explicit songs can be skipped.
    pub explicit_flags: bool,
    /// It keeps a watch history that plays are reported to.
    pub watch_history: bool,
    /// A row can have a music video cut, with a picture to show.
    pub music_videos: bool,
    /// It can start a radio from a track, which autoplay runs on.
    pub track_radio: bool,
}

/// The web app's audio quality setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioQuality {
    /// The smallest format, to save data.
    Low,
    /// Standard formats, never the ones Premium unlocks.
    Normal,
    /// The best format the account is offered; `auto` in an older
    /// config.json reads as this.
    #[default]
    #[serde(alias = "auto")]
    High,
}

/// A music video's picture for the expanded player, decoded muted beside
/// kopuzd's sound.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoStream {
    pub url: String,
    /// Sent with every request to `url`.
    pub headers: Vec<(String, String)>,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub codec: String,
}

/// A byte range of a queued music video's picture, as kopuzd serves it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VideoChunk {
    pub content_type: String,
    pub start: u64,
    /// The whole stream's length, when known.
    pub total: Option<u64>,
    pub bytes: Vec<u8>,
}
