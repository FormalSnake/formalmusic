//! The client's whole state and every action the UI can take.
//!
//! Threading: the store lives on the tokio runtime the desktop app builds at
//! startup. Every socket call, timer, cache write and JSON parse runs there.
//! The GPUI foreground thread takes the read lock for the length of one render
//! and calls methods that either mutate in memory and return at once, or
//! spawn their I/O.
//!
//! Change notification: every mutation records the `StoreEvent`s it caused
//! and sends them on a broadcast channel after the write lock is released.
//! Events are narrow on purpose: a position tick is `Position` alone, so the
//! desktop repaints the seek bar and nothing else.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use formalmusic_api::{
    Account, BrowseTarget, Command, Continuation, EnqueuePosition, Event, Item, LibraryScope,
    LibraryTab, Lyrics, Page, PlaySource, PlayerState, PlaylistEdit, Privacy, QueueState,
    RateTarget, Rating, Repeat, Reply, SearchFilter, SearchResults, SessionInfo, Status,
    Suggestion, Track,
};
use parking_lot::{Mutex, RwLock, RwLockReadGuard};
use tokio::sync::{broadcast, mpsc};
use tokio::task::AbortHandle;

use crate::art::ArtCache;
use crate::cache::{CachedState, StateCache};
use crate::transport::{ClientError, ConnectionStatus, Transport, TransportEvent, TransportKind};

/// A page older than this is shown and fetched again behind it.
const STALE_AFTER: Duration = Duration::from_secs(5 * 60);
/// Search answers go stale faster: a query typed again usually wants fresh results.
const SEARCH_STALE_AFTER: Duration = Duration::from_secs(60);
/// Typing pauses this long before a suggestions request goes out.
const SUGGEST_DEBOUNCE: Duration = Duration::from_millis(150);
/// Prefetch waits this long, then leaves this much between requests.
const PREFETCH_DELAY: Duration = Duration::from_millis(1200);
const PREFETCH_GAP: Duration = Duration::from_millis(150);
/// How far a seek jumps from the keyboard.
pub const SEEK_STEP_MS: u64 = 10_000;
pub const VOLUME_STEP: f32 = 0.05;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SearchKey {
    pub query: String,
    pub filter: Option<SearchFilter>,
}

/// Where the main pane is. History is a list of these.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Route {
    Browse(BrowseTarget),
    Search(SearchKey),
}

#[derive(Clone, Debug, Default)]
pub struct PageEntry {
    pub page: Option<Arc<Page>>,
    /// Last opened, for evicting the page nobody has looked at longest.
    viewed_at: Option<Instant>,
    pub loading: bool,
    pub loading_more: bool,
    /// The last fetch failed. The page, if there is one, is still the old one.
    pub error: Option<String>,
    fetched_at: Option<Instant>,
}

#[derive(Clone, Debug, Default)]
pub struct SearchEntry {
    pub results: Option<Arc<SearchResults>>,
    pub loading: bool,
    pub loading_more: bool,
    pub error: Option<String>,
    fetched_at: Option<Instant>,
}

#[derive(Clone, Debug, Default)]
pub struct LyricsEntry {
    pub lyrics: Option<Arc<Lyrics>>,
    pub loading: bool,
    /// Loaded, and YouTube has none for this track.
    pub missing: bool,
}

/// An album's animated cover, by `(artist, album)`.
#[derive(Clone, Debug, Default)]
pub struct CoverEntry {
    /// The local mp4.
    pub path: Option<Arc<std::path::Path>>,
    pub loading: bool,
    /// Loaded, and Apple Music has none for this album.
    pub missing: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Suggestions {
    pub query: String,
    pub items: Arc<Vec<Suggestion>>,
}

#[derive(Clone, Debug, Default)]
pub struct AppState {
    pub connection: ConnectionStatus,
    pub connection_error: Option<String>,
    /// `None` until the daemon or the cache has said.
    pub session: Option<SessionInfo>,
    pub accounts: Vec<Account>,
    pub pages: HashMap<BrowseTarget, PageEntry>,
    pub searches: HashMap<SearchKey, SearchEntry>,
    pub suggestions: Suggestions,
    pub player: PlayerState,
    /// Kept apart from `player` because it moves four times a second.
    pub position_ms: u64,
    /// When `position_ms` was true. The daemon sends a position about once a
    /// second, so the UI moves the bar on from here (`position_now`).
    pub position_at: Option<Instant>,
    pub buffered_ms: u64,
    pub queue: Arc<QueueState>,
    pub lyrics: HashMap<String, LyricsEntry>,
    /// Index into the current track's synced lyrics.
    pub lyric_line: Option<usize>,
    pub animated_covers: HashMap<(String, String), CoverEntry>,
    /// The player's "Related" tab, keyed by its browse id.
    pub related: HashMap<String, PageEntry>,
    /// Likes made here, ahead of the pages that still carry the old rating.
    pub ratings: HashMap<String, Rating>,
    /// A failure worth a toast, with a counter so the same text twice is two toasts.
    pub notice: Option<(u64, String)>,
}

impl AppState {
    pub fn page(&self, target: &BrowseTarget) -> Option<&Arc<Page>> {
        self.pages.get(target).and_then(|entry| entry.page.as_ref())
    }

    /// Where playback is now: the last position plus the time since, while
    /// playing, never past the end of the track.
    pub fn position_now(&self) -> u64 {
        let elapsed = match (self.player.status, self.position_at) {
            (Status::Playing, Some(at)) => at.elapsed().as_millis() as u64,
            _ => 0,
        };
        (self.position_ms + elapsed).min(self.player.duration_ms.unwrap_or(u64::MAX))
    }

    pub fn rating(&self, track: &Track) -> Rating {
        self.ratings
            .get(&track.video_id)
            .copied()
            .unwrap_or(track.like)
    }

    pub fn signed_in(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.signed_in)
    }

    /// The video id of the track playing or paused.
    pub fn current_video(&self) -> Option<&str> {
        self.player
            .track
            .as_ref()
            .map(|track| track.video_id.as_str())
    }

    /// The user's playlists for the sidebar, from the library page.
    pub fn library_playlists(&self) -> Vec<&Item> {
        self.page(&BrowseTarget::Library(LibraryTab::Playlists))
            .map(|page| {
                page.sections
                    .iter()
                    .flat_map(|section| section.items.iter())
                    .filter(|item| matches!(item, Item::Playlist { .. }))
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum StoreEvent {
    Connection,
    Session,
    Accounts,
    Page(BrowseTarget),
    Search(SearchKey),
    Suggestions,
    /// Track, status, volume, repeat or shuffle.
    Player,
    /// Only which track is current, or whether it plays: what a row that
    /// marks the playing track needs, without every volume change.
    NowPlaying,
    /// Only `position_ms` and `buffered_ms`.
    Position,
    Queue,
    /// Cached pages of this kind were dropped.
    Library(LibraryScope),
    Lyrics(String),
    LyricLine,
    /// An animated cover finished loading, whether there was one or not.
    AnimatedCover,
    Related(String),
    Ratings,
    Notice,
}

impl StoreEvent {
    fn cacheable(&self) -> bool {
        match self {
            StoreEvent::Page(target) => cacheable_target(target),
            StoreEvent::Player | StoreEvent::Queue | StoreEvent::Session => true,
            _ => false,
        }
    }
}

fn cacheable_target(target: &BrowseTarget) -> bool {
    matches!(
        target,
        BrowseTarget::Home | BrowseTarget::Explore | BrowseTarget::Library(_)
    )
}

/// The library pages a `LibraryChanged` scope makes stale.
fn scope_covers(scope: LibraryScope, target: &BrowseTarget) -> bool {
    match scope {
        LibraryScope::Likes => matches!(
            target,
            BrowseTarget::Library(LibraryTab::LikedSongs)
                | BrowseTarget::Library(LibraryTab::Songs)
        ),
        LibraryScope::Playlists => matches!(
            target,
            BrowseTarget::Library(LibraryTab::Playlists) | BrowseTarget::Playlist(_)
        ),
        LibraryScope::Albums => matches!(
            target,
            BrowseTarget::Library(LibraryTab::Albums) | BrowseTarget::Album(_)
        ),
        LibraryScope::Subscriptions => {
            matches!(
                target,
                BrowseTarget::Library(LibraryTab::Artists)
                    | BrowseTarget::Library(LibraryTab::Subscriptions)
                    | BrowseTarget::Artist(_)
            )
        }
        LibraryScope::History => matches!(target, BrowseTarget::History),
    }
}

pub struct StoreOptions {
    /// Last known state, painted before the daemon answers.
    pub cache: Option<Arc<StateCache>>,
    pub art: Arc<ArtCache>,
    /// The cache already read (`StateCache::load_blocking` on a thread that
    /// ran beside window setup), so the first frame has it.
    pub preloaded: Option<CachedState>,
}

/// Cheap to clone; every clone is the same store.
#[derive(Clone)]
pub struct MusicStore {
    inner: Arc<Inner>,
}

struct Inner {
    transport: Arc<dyn Transport>,
    runtime: tokio::runtime::Handle,
    state: RwLock<AppState>,
    events: broadcast::Sender<StoreEvent>,
    cache: Option<Arc<StateCache>>,
    art: Arc<ArtCache>,
    private: Mutex<Private>,
    me: Weak<Inner>,
}

#[derive(Default)]
struct Private {
    /// The cache was painted at construction; `start` does not read it again.
    painted: bool,
    suggest: Option<AbortHandle>,
    event_loop: Option<AbortHandle>,
    notices: u64,
    visible: Option<Route>,
}

fn message(error: &ClientError) -> String {
    use formalmusic_api::ApiError;
    match error {
        ClientError::Api(ApiError::SignedOut) => "Sign in to see this.".into(),
        ClientError::Api(ApiError::NotFound(_)) => "YouTube Music could not find that.".into(),
        ClientError::Api(ApiError::Network(_)) => {
            "YouTube Music is not answering. Check your connection.".into()
        }
        ClientError::Api(ApiError::Parse(_)) => {
            "YouTube Music sent something this version cannot read.".into()
        }
        ClientError::Api(ApiError::Playback(text)) => format!("Could not play that: {text}"),
        ClientError::Api(ApiError::BadRequest(text)) => text.clone(),
        other => other.to_string(),
    }
}

/// The active line for `position_ms`: the last one that has started.
pub fn line_at(lyrics: &Lyrics, position_ms: u64) -> Option<usize> {
    if !lyrics.synced {
        return None;
    }
    let after = lyrics
        .lines
        .partition_point(|line| line.start_ms <= position_ms);
    after.checked_sub(1)
}

impl MusicStore {
    /// Builds the store; nothing runs until `start`.
    pub fn new(
        transport: Arc<dyn Transport>,
        options: StoreOptions,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        let (events, _) = broadcast::channel(1024);
        let inner = Arc::new_cyclic(|me| Inner {
            transport,
            runtime,
            state: RwLock::new(AppState::default()),
            events,
            cache: options.cache,
            art: options.art,
            private: Mutex::new(Private::default()),
            me: me.clone(),
        });
        let store = Self { inner };
        if let Some(cached) = options.preloaded {
            store.paint_cached(cached);
            store.inner.private.lock().painted = true;
        }
        store
    }

    /// Capacity is large enough that only a stalled UI lags.
    pub fn events(&self) -> broadcast::Receiver<StoreEvent> {
        self.inner.events.subscribe()
    }

    /// Hold the guard for a render at most, never across an await.
    pub fn state(&self) -> RwLockReadGuard<'_, AppState> {
        self.inner.state.read()
    }

    pub fn art(&self) -> &Arc<ArtCache> {
        &self.inner.art
    }

    pub fn kind(&self) -> TransportKind {
        self.inner.transport.kind()
    }

    pub fn runtime(&self) -> &tokio::runtime::Handle {
        &self.inner.runtime
    }

    /// Runs `future` on the store's runtime. The UI uses this for every async method.
    pub fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.inner.runtime.spawn(future);
    }

    /// Paints from the cache, then connects and keeps applying events.
    pub async fn start(&self) {
        let painted = self.inner.private.lock().painted;
        if !painted
            && let Some(cache) = &self.inner.cache
            && let Some(cached) = cache.load().await
        {
            self.paint_cached(cached);
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let store = self.clone();
        let task = self
            .inner
            .runtime
            .spawn(async move { store.event_loop(rx).await });
        self.inner.private.lock().event_loop = Some(task.abort_handle());
        let _entered = self.inner.runtime.enter();
        self.inner.transport.start(tx);
    }

    pub async fn stop(&self) {
        self.inner.transport.stop();
        if let Some(task) = self.inner.private.lock().event_loop.take() {
            task.abort();
        }
        if let Some(cache) = &self.inner.cache {
            cache.flush().await;
        }
    }

    fn paint_cached(&self, cached: CachedState) {
        self.inner.update(|state, events| {
            for (target, page) in cached.pages {
                events.push(StoreEvent::Page(target.clone()));
                state.pages.insert(
                    target,
                    PageEntry {
                        page: Some(page),
                        ..PageEntry::default()
                    },
                );
            }
            let mut player = cached.player;
            if player.status != Status::Stopped {
                player.status = Status::Paused;
            }
            state.position_ms = player.position_ms;
            state.position_at = Some(Instant::now());
            state.player = player;
            state.queue = cached.queue;
            state.session = cached.session;
            events.extend([
                StoreEvent::Player,
                StoreEvent::NowPlaying,
                StoreEvent::Position,
                StoreEvent::Queue,
                StoreEvent::Session,
            ]);
        });
    }

    async fn event_loop(&self, mut rx: mpsc::UnboundedReceiver<TransportEvent>) {
        while let Some(event) = rx.recv().await {
            match event {
                TransportEvent::Connection { status, error } => {
                    let came_up = self.inner.update(|state, events| {
                        let came_up = status == ConnectionStatus::Online
                            && state.connection != ConnectionStatus::Online;
                        if state.connection != status || state.connection_error != error {
                            state.connection = status;
                            state.connection_error = error;
                            events.push(StoreEvent::Connection);
                        }
                        came_up
                    });
                    if came_up {
                        let store = self.clone();
                        self.spawn(async move { store.on_online().await });
                    }
                }
                TransportEvent::Event(event) => self.apply(event),
            }
        }
    }

    /// Applies one daemon event.
    pub fn apply(&self, event: Event) {
        match event {
            Event::Player(player) => self.inner.update(|state, events| {
                let changed_track = state.player.track.as_ref().map(|t| &t.video_id)
                    != player.track.as_ref().map(|t| &t.video_id);
                if changed_track || state.player.status != player.status {
                    events.push(StoreEvent::NowPlaying);
                }
                state.position_ms = player.position_ms;
                state.position_at = Some(Instant::now());
                state.player = player;
                events.extend([StoreEvent::Player, StoreEvent::Position]);
                if changed_track {
                    state.buffered_ms = 0;
                }
                update_lyric_line(state, events);
            }),
            Event::Position {
                position_ms,
                buffered_ms,
            } => self.inner.update(|state, events| {
                if state.position_ms != position_ms || state.buffered_ms != buffered_ms {
                    state.position_ms = position_ms;
                    state.position_at = Some(Instant::now());
                    state.buffered_ms = buffered_ms;
                    events.push(StoreEvent::Position);
                    update_lyric_line(state, events);
                }
            }),
            Event::Queue(queue) => self.inner.update(|state, events| {
                state.queue = Arc::new(queue);
                events.push(StoreEvent::Queue);
            }),
            Event::Session(session) => {
                let changed = self.inner.update(|state, events| {
                    let changed = state
                        .session
                        .as_ref()
                        .map(|s| (s.signed_in, s.account.as_ref().map(|a| a.page_id.clone())))
                        != Some((
                            session.signed_in,
                            session.account.as_ref().map(|a| a.page_id.clone()),
                        ));
                    state.session = Some(session);
                    events.push(StoreEvent::Session);
                    changed
                });
                if changed {
                    self.account_changed();
                }
            }
            Event::LibraryChanged { scope } => {
                self.inner.update(|state, events| {
                    for (target, entry) in state.pages.iter_mut() {
                        if scope_covers(scope, target) {
                            entry.fetched_at = None;
                        }
                    }
                    if scope == LibraryScope::Likes {
                        state.ratings.clear();
                        events.push(StoreEvent::Ratings);
                    }
                    events.push(StoreEvent::Library(scope));
                });
                self.revalidate_visible();
                if scope == LibraryScope::Playlists {
                    self.refresh(BrowseTarget::Library(LibraryTab::Playlists));
                }
            }
            Event::Notice { message } => self.notice(message),
        }
    }

    async fn on_online(&self) {
        let session = match self.inner.transport.call(Command::Session).await {
            Ok(Reply::Session(session)) => session,
            _ => return,
        };
        let signed_in = session.signed_in;
        self.inner.update(|state, events| {
            state.session = Some(session);
            events.push(StoreEvent::Session);
        });
        if signed_in {
            self.refresh(BrowseTarget::Library(LibraryTab::Playlists));
            self.revalidate_visible();
        }
    }

    /// Signed in, out, or onto another account: every page is someone else's now.
    fn account_changed(&self) {
        self.inner.update(|state, events| {
            for (target, entry) in state.pages.iter_mut() {
                entry.fetched_at = None;
                events.push(StoreEvent::Page(target.clone()));
            }
            state.searches.clear();
            state.related.clear();
            state.ratings.clear();
        });
        if self.state().signed_in() {
            self.refresh(BrowseTarget::Library(LibraryTab::Playlists));
            self.revalidate_visible();
        }
    }

    /// The main pane says what it shows, so a library change refetches it.
    pub fn set_visible(&self, route: Route) {
        self.inner.private.lock().visible = Some(route);
    }

    fn revalidate_visible(&self) {
        let visible = self.inner.private.lock().visible.clone();
        match visible {
            Some(Route::Browse(target)) => self.open(target),
            Some(Route::Search(key)) => self.search(key),
            None => {}
        }
    }

    // ---------------------------------------------------------------------
    // Pages
    // ---------------------------------------------------------------------

    /// Shows a page: the cached one at once if there is one, fetched again
    /// behind it when stale.
    pub fn open(&self, target: BrowseTarget) {
        self.inner
            .state
            .write()
            .pages
            .entry(target.clone())
            .or_default()
            .viewed_at = Some(Instant::now());
        if !self.claim(&target) {
            return;
        }
        let store = self.clone();
        self.spawn(async move { store.fetch_page(target).await });
    }

    /// Fetches the first targets of a page ahead of a click, one at a time
    /// and after a pause, so they never compete with what is on screen.
    pub fn prefetch(&self, targets: Vec<BrowseTarget>) {
        let store = self.clone();
        self.spawn(async move {
            tokio::time::sleep(PREFETCH_DELAY).await;
            for target in targets {
                if store.claim(&target) {
                    store.fetch_page(target).await;
                    tokio::time::sleep(PREFETCH_GAP).await;
                }
            }
        });
    }

    /// Marks `target` loading when it is missing or stale; false when a
    /// fetch is already out or the cached page is fresh.
    fn claim(&self, target: &BrowseTarget) -> bool {
        self.inner.update(|state, events| {
            let entry = state.pages.entry(target.clone()).or_default();
            if entry.loading
                || (entry.page.is_some()
                    && entry
                        .fetched_at
                        .is_some_and(|at| at.elapsed() < STALE_AFTER))
            {
                return false;
            }
            entry.loading = true;
            events.push(StoreEvent::Page(target.clone()));
            true
        })
    }

    async fn fetch_page(&self, target: BrowseTarget) {
        let store = self;
        {
            let result = store
                .inner
                .transport
                .call(Command::Browse {
                    target: target.clone(),
                })
                .await;
            store.inner.update(|state, events| {
                let entry = state.pages.entry(target.clone()).or_default();
                entry.loading = false;
                match result {
                    Ok(Reply::Page(page)) => {
                        entry.page = Some(Arc::new(page));
                        entry.error = None;
                        entry.fetched_at = Some(Instant::now());
                    }
                    Ok(_) => entry.error = Some(message(&ClientError::UnexpectedReply)),
                    Err(error) => entry.error = Some(message(&error)),
                }
                events.push(StoreEvent::Page(target));
                evict_pages(state);
            });
        }
    }

    /// `open`, ignoring how fresh the page is.
    pub fn refresh(&self, target: BrowseTarget) {
        self.inner.update(|state, _| {
            if let Some(entry) = state.pages.get_mut(&target) {
                entry.fetched_at = None;
            }
        });
        self.open(target);
    }

    /// Appends the page's next sections (Home keeps going as you scroll).
    pub fn load_more(&self, target: BrowseTarget) {
        let token = self.inner.update(|state, events| {
            let entry = state.pages.get_mut(&target)?;
            if entry.loading_more {
                return None;
            }
            let token = entry.page.as_ref()?.continuation.clone()?;
            entry.loading_more = true;
            events.push(StoreEvent::Page(target.clone()));
            Some(token)
        });
        let Some(token) = token else { return };
        let store = self.clone();
        self.spawn(async move {
            let result = store.continuation(token).await;
            store.inner.update(|state, events| {
                let Some(entry) = state.pages.get_mut(&target) else {
                    return;
                };
                entry.loading_more = false;
                match result {
                    Ok(more) => {
                        if let Some(page) = &entry.page {
                            let mut page = (**page).clone();
                            page.sections.extend(more.sections);
                            page.continuation = more.continuation;
                            entry.page = Some(Arc::new(page));
                        }
                    }
                    // Dropping the token stops the end of the page from asking again
                    // every frame; a refresh brings it back.
                    Err(error) => {
                        entry.error = Some(message(&error));
                        if let Some(page) = &entry.page {
                            let mut page = (**page).clone();
                            page.continuation = None;
                            entry.page = Some(Arc::new(page));
                        }
                    }
                }
                events.push(StoreEvent::Page(target));
            });
        });
    }

    /// Appends one section's next items (a long playlist).
    pub fn load_more_items(&self, target: BrowseTarget, section: usize) {
        let token = self.inner.update(|state, events| {
            let entry = state.pages.get_mut(&target)?;
            if entry.loading_more {
                return None;
            }
            let token = entry
                .page
                .as_ref()?
                .sections
                .get(section)?
                .continuation
                .clone()?;
            entry.loading_more = true;
            events.push(StoreEvent::Page(target.clone()));
            Some(token)
        });
        let Some(token) = token else { return };
        let store = self.clone();
        self.spawn(async move {
            let result = store.continuation(token).await;
            store.inner.update(|state, events| {
                let Some(entry) = state.pages.get_mut(&target) else {
                    return;
                };
                entry.loading_more = false;
                match result {
                    Ok(more) => {
                        if let Some(page) = &entry.page {
                            let mut page = (**page).clone();
                            if let Some(shelf) = page.sections.get_mut(section) {
                                shelf.items.extend(more.items);
                                shelf.continuation = more.continuation;
                            }
                            entry.page = Some(Arc::new(page));
                        }
                    }
                    Err(error) => {
                        entry.error = Some(message(&error));
                        if let Some(page) = &entry.page {
                            let mut page = (**page).clone();
                            if let Some(shelf) = page.sections.get_mut(section) {
                                shelf.continuation = None;
                            }
                            entry.page = Some(Arc::new(page));
                        }
                    }
                }
                events.push(StoreEvent::Page(target));
            });
        });
    }

    async fn continuation(
        &self,
        token: Continuation,
    ) -> Result<formalmusic_api::ContinuationPage, ClientError> {
        match self
            .inner
            .transport
            .call(Command::Continue { token })
            .await?
        {
            Reply::Continuation(page) => Ok(page),
            _ => Err(ClientError::UnexpectedReply),
        }
    }

    // ---------------------------------------------------------------------
    // Search
    // ---------------------------------------------------------------------

    pub fn search(&self, key: SearchKey) {
        let fetch = self.inner.update(|state, events| {
            let entry = state.searches.entry(key.clone()).or_default();
            if entry.loading
                || (entry.results.is_some()
                    && entry
                        .fetched_at
                        .is_some_and(|at| at.elapsed() < SEARCH_STALE_AFTER))
            {
                return false;
            }
            entry.loading = true;
            events.push(StoreEvent::Search(key.clone()));
            true
        });
        if !fetch {
            return;
        }
        let store = self.clone();
        self.spawn(async move {
            let result = store
                .inner
                .transport
                .call(Command::Search {
                    query: key.query.clone(),
                    filter: key.filter,
                })
                .await;
            store.inner.update(|state, events| {
                let entry = state.searches.entry(key.clone()).or_default();
                entry.loading = false;
                match result {
                    Ok(Reply::Search(results)) => {
                        entry.results = Some(Arc::new(results));
                        entry.error = None;
                        entry.fetched_at = Some(Instant::now());
                    }
                    Ok(_) => entry.error = Some(message(&ClientError::UnexpectedReply)),
                    Err(error) => entry.error = Some(message(&error)),
                }
                events.push(StoreEvent::Search(key));
            });
        });
    }

    pub fn search_more(&self, key: SearchKey) {
        let token = self.inner.update(|state, events| {
            let entry = state.searches.get_mut(&key)?;
            if entry.loading_more {
                return None;
            }
            let token = entry.results.as_ref()?.continuation.clone()?;
            entry.loading_more = true;
            events.push(StoreEvent::Search(key.clone()));
            Some(token)
        });
        let Some(token) = token else { return };
        let store = self.clone();
        self.spawn(async move {
            let result = store.continuation(token).await;
            store.inner.update(|state, events| {
                let Some(entry) = state.searches.get_mut(&key) else {
                    return;
                };
                entry.loading_more = false;
                match result {
                    Ok(more) => {
                        if let Some(results) = &entry.results {
                            let mut results = (**results).clone();
                            // A filtered search continues its one shelf; the
                            // unfiltered page continues with whole sections.
                            if more.sections.is_empty() {
                                if let Some(last) = results.sections.last_mut() {
                                    last.items.extend(more.items);
                                }
                            } else {
                                results.sections.extend(more.sections);
                            }
                            results.continuation = more.continuation;
                            entry.results = Some(Arc::new(results));
                        }
                    }
                    // Dropping the token stops the end of the list from asking again
                    // every frame; a refresh brings it back.
                    Err(error) => {
                        entry.error = Some(message(&error));
                        if let Some(results) = &entry.results {
                            let mut results = (**results).clone();
                            results.continuation = None;
                            entry.results = Some(Arc::new(results));
                        }
                    }
                }
                events.push(StoreEvent::Search(key));
            });
        });
    }

    /// Live suggestions as the search field changes. Debounced: only the
    /// query typing settled on goes out.
    pub fn suggest(&self, query: &str) {
        let query = query.trim().to_owned();
        if let Some(previous) = self.inner.private.lock().suggest.take() {
            previous.abort();
        }
        if query.is_empty() {
            self.inner.update(|state, events| {
                if !state.suggestions.items.is_empty() || !state.suggestions.query.is_empty() {
                    state.suggestions = Suggestions::default();
                    events.push(StoreEvent::Suggestions);
                }
            });
            return;
        }
        let store = self.clone();
        let task = self.inner.runtime.spawn(async move {
            tokio::time::sleep(SUGGEST_DEBOUNCE).await;
            let Ok(Reply::Suggestions(items)) = store
                .inner
                .transport
                .call(Command::Suggestions {
                    query: query.clone(),
                })
                .await
            else {
                return;
            };
            store.inner.update(|state, events| {
                state.suggestions = Suggestions {
                    query,
                    items: Arc::new(items),
                };
                events.push(StoreEvent::Suggestions);
            });
        });
        self.inner.private.lock().suggest = Some(task.abort_handle());
    }

    // ---------------------------------------------------------------------
    // Player
    // ---------------------------------------------------------------------

    /// Sends a command whose effect comes back as daemon events; a refusal
    /// becomes a toast.
    fn send(&self, command: Command) {
        let store = self.clone();
        self.spawn(async move {
            if let Err(error) = store.inner.transport.call(command).await {
                store.notice(message(&error));
            }
        });
    }

    pub fn play(&self, source: PlaySource, start_index: usize, shuffle: bool, radio: bool) {
        self.send(Command::Play {
            source,
            start_index,
            shuffle,
            radio,
        });
    }

    pub fn toggle(&self) {
        self.inner.update(|state, events| {
            let next = match state.player.status {
                Status::Playing | Status::Loading => Status::Paused,
                Status::Paused => Status::Playing,
                Status::Stopped => return,
            };
            state.position_ms = state.position_now();
            state.position_at = Some(Instant::now());
            state.player.status = next;
            events.extend([StoreEvent::Player, StoreEvent::NowPlaying]);
        });
        self.send(Command::Toggle);
    }

    pub fn next(&self) {
        self.send(Command::Next);
    }

    pub fn previous(&self) {
        self.send(Command::Previous);
    }

    pub fn seek(&self, position_ms: u64) {
        let position_ms = self.inner.update(|state, events| {
            let end = state.player.duration_ms.unwrap_or(u64::MAX);
            state.position_ms = position_ms.min(end);
            state.position_at = Some(Instant::now());
            events.push(StoreEvent::Position);
            update_lyric_line(state, events);
            state.position_ms
        });
        self.send(Command::SeekTo { position_ms });
    }

    /// Seeks by `delta_ms` from where the player is.
    pub fn seek_by(&self, delta_ms: i64) {
        let target = {
            let state = self.state();
            if state.player.track.is_none() {
                return;
            }
            (state.position_now() as i64 + delta_ms).max(0) as u64
        };
        self.seek(target);
    }

    pub fn set_volume(&self, volume: f32) {
        let volume = volume.clamp(0., 1.);
        self.inner.update(|state, events| {
            state.player.volume = volume;
            if volume > 0. {
                state.player.muted = false;
            }
            events.push(StoreEvent::Player);
        });
        self.send(Command::SetVolume { volume });
    }

    pub fn set_muted(&self, muted: bool) {
        self.inner.update(|state, events| {
            state.player.muted = muted;
            events.push(StoreEvent::Player);
        });
        self.send(Command::SetMuted { muted });
    }

    /// Off, then all, then one, the order the web app's button steps through.
    pub fn cycle_repeat(&self) {
        let repeat = self.inner.update(|state, events| {
            state.player.repeat = match state.player.repeat {
                Repeat::Off => Repeat::All,
                Repeat::All => Repeat::One,
                Repeat::One => Repeat::Off,
            };
            events.push(StoreEvent::Player);
            state.player.repeat
        });
        self.send(Command::SetRepeat { repeat });
    }

    pub fn toggle_shuffle(&self) {
        let shuffle = self.inner.update(|state, events| {
            state.player.shuffle = !state.player.shuffle;
            events.push(StoreEvent::Player);
            state.player.shuffle
        });
        self.send(Command::SetShuffle { shuffle });
    }

    pub fn enqueue(&self, tracks: Vec<Track>, position: EnqueuePosition) {
        self.send(Command::Enqueue { tracks, position });
    }

    pub fn jump_to(&self, index: usize) {
        self.send(Command::JumpTo { index });
    }

    pub fn remove_from_queue(&self, index: usize) {
        self.inner.update(|state, events| {
            if index >= state.queue.tracks.len() {
                return;
            }
            let queue = Arc::make_mut(&mut state.queue);
            queue.tracks.remove(index);
            queue.current = queue.current.and_then(|current| match current.cmp(&index) {
                std::cmp::Ordering::Greater => Some(current - 1),
                std::cmp::Ordering::Equal => None,
                std::cmp::Ordering::Less => Some(current),
            });
            events.push(StoreEvent::Queue);
        });
        self.send(Command::RemoveFromQueue { index });
    }

    /// Moves a queue row so it ends up at `to`, as a drag drops it.
    pub fn move_in_queue(&self, from: usize, to: usize) {
        let moved = self.inner.update(|state, events| {
            let len = state.queue.tracks.len();
            if from >= len || to >= len || from == to {
                return false;
            }
            let queue = Arc::make_mut(&mut state.queue);
            let track = queue.tracks.remove(from);
            queue.tracks.insert(to, track);
            queue.current = queue.current.map(|current| moved_index(current, from, to));
            events.push(StoreEvent::Queue);
            true
        });
        if moved {
            self.send(Command::MoveInQueue { from, to });
        }
    }

    pub fn clear_queue(&self) {
        self.send(Command::ClearQueue);
    }

    // ---------------------------------------------------------------------
    // Lyrics and related
    // ---------------------------------------------------------------------

    pub fn load_lyrics(&self, video_id: String) {
        let fetch = self.inner.update(|state, events| {
            let entry = state.lyrics.entry(video_id.clone()).or_default();
            if entry.loading || entry.lyrics.is_some() || entry.missing {
                return false;
            }
            entry.loading = true;
            events.push(StoreEvent::Lyrics(video_id.clone()));
            true
        });
        if !fetch {
            return;
        }
        let store = self.clone();
        self.spawn(async move {
            let result = store
                .inner
                .transport
                .call(Command::Lyrics {
                    video_id: video_id.clone(),
                })
                .await;
            store.inner.update(|state, events| {
                let entry = state.lyrics.entry(video_id.clone()).or_default();
                entry.loading = false;
                match result {
                    Ok(Reply::Lyrics(Some(lyrics))) => entry.lyrics = Some(Arc::new(lyrics)),
                    Ok(Reply::Lyrics(None)) => entry.missing = true,
                    _ => {}
                }
                events.push(StoreEvent::Lyrics(video_id));
                update_lyric_line(state, events);
            });
        });
    }

    /// Asks the daemon for the album's animated cover once. The daemon has
    /// usually warmed it already, so this is a cache read.
    pub fn load_animated_cover(&self, key: (String, String)) {
        let fetch = self.inner.update(|state, _| {
            let entry = state.animated_covers.entry(key.clone()).or_default();
            if entry.loading || entry.path.is_some() || entry.missing {
                return false;
            }
            entry.loading = true;
            true
        });
        if !fetch {
            return;
        }
        let store = self.clone();
        self.spawn(async move {
            let (artist, album) = key.clone();
            let result = store
                .inner
                .transport
                .call(Command::AnimatedCover { artist, album })
                .await;
            store.inner.update(|state, events| {
                let entry = state.animated_covers.entry(key).or_default();
                entry.loading = false;
                match result {
                    Ok(Reply::AnimatedCover(Some(path))) => {
                        entry.path = Some(std::path::PathBuf::from(path).into())
                    }
                    // A failed lookup is not asked again this session: the
                    // static cover is a fine answer, and the bar renders often.
                    _ => entry.missing = true,
                }
                events.push(StoreEvent::AnimatedCover);
            });
        });
    }

    pub fn load_related(&self, browse_id: String) {
        let fetch = self.inner.update(|state, events| {
            let entry = state.related.entry(browse_id.clone()).or_default();
            if entry.loading || entry.page.is_some() {
                return false;
            }
            entry.loading = true;
            events.push(StoreEvent::Related(browse_id.clone()));
            true
        });
        if !fetch {
            return;
        }
        let store = self.clone();
        self.spawn(async move {
            let result = store
                .inner
                .transport
                .call(Command::Related {
                    browse_id: browse_id.clone(),
                })
                .await;
            store.inner.update(|state, events| {
                let entry = state.related.entry(browse_id.clone()).or_default();
                entry.loading = false;
                match result {
                    Ok(Reply::Page(page)) => entry.page = Some(Arc::new(page)),
                    Ok(_) => entry.error = Some(message(&ClientError::UnexpectedReply)),
                    Err(error) => entry.error = Some(message(&error)),
                }
                events.push(StoreEvent::Related(browse_id));
            });
        });
    }

    // ---------------------------------------------------------------------
    // Library
    // ---------------------------------------------------------------------

    pub fn rate(&self, track: &Track, rating: Rating) {
        let video_id = track.video_id.clone();
        self.inner.update(|state, events| {
            state.ratings.insert(video_id.clone(), rating);
            if let Some(current) = state
                .player
                .track
                .as_mut()
                .filter(|current| current.video_id == video_id)
            {
                current.like = rating;
                events.push(StoreEvent::Player);
            }
            events.push(StoreEvent::Ratings);
        });
        self.send(Command::Rate {
            target: RateTarget::Track { video_id },
            rating,
        });
    }

    pub fn set_subscribed(&self, channel_id: String, subscribed: bool) {
        self.send(Command::SetSubscribed {
            channel_id,
            subscribed,
        });
    }

    pub fn set_in_library(&self, playlist_id: String, saved: bool) {
        self.send(Command::SetInLibrary { playlist_id, saved });
    }

    pub fn add_to_playlist(&self, playlist_id: String, video_ids: Vec<String>) {
        let edits = video_ids
            .into_iter()
            .map(|video_id| PlaylistEdit::Add { video_id })
            .collect();
        self.send(Command::EditPlaylist { playlist_id, edits });
    }

    /// Takes the row off the cached page at once; the daemon's
    /// `LibraryChanged` brings the real page back after.
    pub fn remove_from_playlist(&self, playlist_id: String, track: &Track) {
        let Some(set_video_id) = track.set_video_id.clone() else {
            return;
        };
        let video_id = track.video_id.clone();
        let target = BrowseTarget::Playlist(playlist_id.clone());
        self.inner.update(|state, events| {
            let Some(page) = state.pages.get_mut(&target).and_then(|entry| entry.page.as_mut()) else { return };
            let mut next = (**page).clone();
            for section in &mut next.sections {
                section.items.retain(|item| !matches!(item, Item::Track(row) if row.set_video_id.as_deref() == Some(set_video_id.as_str())));
            }
            *page = Arc::new(next);
            events.push(StoreEvent::Page(target.clone()));
        });
        self.send(Command::EditPlaylist {
            playlist_id,
            edits: vec![PlaylistEdit::Remove {
                video_id,
                set_video_id,
            }],
        });
    }

    pub async fn create_playlist(
        &self,
        title: String,
        video_ids: Vec<String>,
    ) -> Result<String, String> {
        let command = Command::CreatePlaylist {
            title,
            description: String::new(),
            privacy: Privacy::Private,
            video_ids,
        };
        match self.inner.transport.call(command).await {
            Ok(Reply::PlaylistCreated { playlist_id }) => {
                self.refresh(BrowseTarget::Library(LibraryTab::Playlists));
                Ok(playlist_id)
            }
            Ok(_) => Err(message(&ClientError::UnexpectedReply)),
            Err(error) => Err(message(&error)),
        }
    }

    // ---------------------------------------------------------------------
    // Session
    // ---------------------------------------------------------------------

    pub async fn sign_in(&self, cookies: String) -> Result<(), String> {
        match self.inner.transport.call(Command::SignIn { cookies }).await {
            Ok(Reply::Session(session)) if session.signed_in => {
                self.apply(Event::Session(session));
                Ok(())
            }
            Ok(Reply::Session(_)) | Ok(Reply::Ok) => Err("Those cookies did not sign in. Copy them again from a signed-in music.youtube.com tab.".into()),
            Ok(_) => Err(message(&ClientError::UnexpectedReply)),
            Err(error) => Err(message(&error)),
        }
    }

    pub fn sign_out(&self) {
        let store = self.clone();
        self.spawn(async move {
            match store.inner.transport.call(Command::SignOut).await {
                Ok(_) => store.apply(Event::Session(SessionInfo::default())),
                Err(error) => store.notice(message(&error)),
            }
        });
    }

    pub fn load_accounts(&self) {
        let store = self.clone();
        self.spawn(async move {
            if let Ok(Reply::Accounts(accounts)) =
                store.inner.transport.call(Command::Accounts).await
            {
                store.inner.update(|state, events| {
                    state.accounts = accounts;
                    events.push(StoreEvent::Accounts);
                });
            }
        });
    }

    pub fn switch_account(&self, page_id: Option<String>) {
        let store = self.clone();
        self.spawn(async move {
            match store
                .inner
                .transport
                .call(Command::SwitchAccount { page_id })
                .await
            {
                Ok(Reply::Session(session)) => store.apply(Event::Session(session)),
                Ok(_) => {
                    if let Ok(Reply::Session(session)) =
                        store.inner.transport.call(Command::Session).await
                    {
                        store.apply(Event::Session(session));
                    }
                }
                Err(error) => store.notice(message(&error)),
            }
        });
    }

    // ---------------------------------------------------------------------
    // Notices
    // ---------------------------------------------------------------------

    pub fn notice(&self, text: String) {
        let seq = {
            let mut private = self.inner.private.lock();
            private.notices += 1;
            private.notices
        };
        self.inner.update(|state, events| {
            state.notice = Some((seq, text));
            events.push(StoreEvent::Notice);
        });
    }

    /// Clears the toast, unless a newer one replaced it meanwhile.
    pub fn clear_notice(&self, seq: u64) {
        self.inner.update(|state, events| {
            if state
                .notice
                .as_ref()
                .is_some_and(|(shown, _)| *shown == seq)
            {
                state.notice = None;
                events.push(StoreEvent::Notice);
            }
        });
    }
}

/// Pages kept in memory before the least recently viewed go. A page is tens
/// to hundreds of kilobytes; a 1000 track playlist is about a megabyte.
const KEPT_PAGES: usize = 32;
const KEPT_SEARCHES: usize = 12;

/// Home, Explore and the library tabs are always kept: they paint at launch
/// and sit one click away in the sidebar.
fn pinned(target: &BrowseTarget) -> bool {
    matches!(
        target,
        BrowseTarget::Home | BrowseTarget::Explore | BrowseTarget::Library(_)
    )
}

fn evict_pages(state: &mut AppState) {
    while state.pages.len() > KEPT_PAGES {
        let oldest = state
            .pages
            .iter()
            .filter(|(target, entry)| !pinned(target) && !entry.loading && !entry.loading_more)
            .min_by_key(|(_, entry)| entry.viewed_at)
            .map(|(target, _)| target.clone());
        let Some(oldest) = oldest else { break };
        state.pages.remove(&oldest);
    }
    while state.searches.len() > KEPT_SEARCHES {
        let oldest = state
            .searches
            .iter()
            .filter(|(_, entry)| !entry.loading)
            .min_by_key(|(_, entry)| entry.fetched_at)
            .map(|(key, _)| key.clone());
        let Some(oldest) = oldest else { break };
        state.searches.remove(&oldest);
    }
    while state.related.len() > 4 {
        let Some(key) = state.related.keys().next().cloned() else {
            break;
        };
        state.related.remove(&key);
    }
}

/// Where the row at `index` ends up after the row at `from` moves to `to`.
fn moved_index(index: usize, from: usize, to: usize) -> usize {
    if index == from {
        to
    } else if from < index && index <= to {
        index - 1
    } else if to <= index && index < from {
        index + 1
    } else {
        index
    }
}

fn update_lyric_line(state: &mut AppState, events: &mut Vec<StoreEvent>) {
    let line = state
        .player
        .track
        .as_ref()
        .and_then(|track| state.lyrics.get(&track.video_id))
        .and_then(|entry| entry.lyrics.as_ref())
        .and_then(|lyrics| line_at(lyrics, state.position_now()));
    if line != state.lyric_line {
        state.lyric_line = line;
        events.push(StoreEvent::LyricLine);
    }
}

impl Inner {
    /// Applies `mutate` under the write lock and sends the events it recorded
    /// once the lock is released.
    fn update<R>(&self, mutate: impl FnOnce(&mut AppState, &mut Vec<StoreEvent>) -> R) -> R {
        let mut events = Vec::new();
        let result = {
            let mut state = self.state.write();
            mutate(&mut state, &mut events)
        };
        if !events.is_empty() {
            self.send(events);
        }
        result
    }

    fn send(&self, mut events: Vec<StoreEvent>) {
        let mut seen = Vec::with_capacity(events.len());
        events.retain(|event| {
            if seen.contains(event) {
                false
            } else {
                seen.push(event.clone());
                true
            }
        });
        let cacheable = events.iter().any(StoreEvent::cacheable);
        for event in events {
            let _ = self.events.send(event);
        }
        if !cacheable {
            return;
        }
        let (Some(cache), Some(me)) = (&self.cache, self.me.upgrade()) else {
            return;
        };
        let _entered = self.runtime.enter();
        cache.schedule(Box::new(move || {
            let state = me.state.read();
            let pages = state
                .pages
                .iter()
                .filter(|(target, _)| cacheable_target(target))
                .filter_map(|(target, entry)| entry.page.clone().map(|page| (target.clone(), page)))
                .collect();
            let mut player = state.player.clone();
            player.position_ms = state.position_ms;
            CachedState {
                pages,
                player,
                queue: state.queue.clone(),
                session: state.session.clone(),
                ..CachedState::new()
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::DemoTransport;
    use formalmusic_api::LyricLine;

    fn store() -> MusicStore {
        let dir =
            std::env::temp_dir().join(format!("formalmusic-store-art-{}", std::process::id()));
        let art = Arc::new(ArtCache::new(dir, reqwest::Client::new()));
        MusicStore::new(
            Arc::new(DemoTransport::new()),
            StoreOptions {
                cache: None,
                art,
                preloaded: None,
            },
            tokio::runtime::Handle::current(),
        )
    }

    async fn settle(store: &MusicStore, until: impl Fn(&AppState) -> bool) {
        for _ in 0..200 {
            if until(&store.state()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("store never settled");
    }

    #[test]
    fn the_active_lyric_line_is_the_last_one_that_started() {
        let line = |start_ms| LyricLine {
            start_ms,
            end_ms: None,
            text: String::new(),
            ..LyricLine::default()
        };
        let lyrics = Lyrics {
            source: None,
            lines: vec![line(1000), line(3000), line(5000)],
            synced: true,
            word_synced: false,
        };
        assert_eq!(line_at(&lyrics, 0), None);
        assert_eq!(line_at(&lyrics, 1000), Some(0));
        assert_eq!(line_at(&lyrics, 4999), Some(1));
        assert_eq!(line_at(&lyrics, 90_000), Some(2));
        assert_eq!(
            line_at(
                &Lyrics {
                    synced: false,
                    ..lyrics
                },
                4000
            ),
            None
        );
    }

    #[test]
    fn a_moved_row_carries_the_current_index_with_it() {
        assert_eq!(moved_index(2, 2, 5), 5);
        assert_eq!(moved_index(3, 1, 4), 2);
        assert_eq!(moved_index(3, 5, 1), 4);
        assert_eq!(moved_index(0, 2, 4), 0);
    }

    #[tokio::test]
    async fn a_stale_page_stays_on_screen_while_it_is_fetched_again() {
        let store = store();
        store.start().await;
        store.open(BrowseTarget::Home);
        assert!(store.state().pages[&BrowseTarget::Home].loading);
        settle(&store, |state| state.page(&BrowseTarget::Home).is_some()).await;
        let first = store.state().page(&BrowseTarget::Home).cloned().unwrap();
        store.refresh(BrowseTarget::Home);
        {
            let state = store.state();
            let entry = &state.pages[&BrowseTarget::Home];
            assert!(entry.loading);
            assert!(Arc::ptr_eq(entry.page.as_ref().unwrap(), &first));
        }
        settle(&store, |state| !state.pages[&BrowseTarget::Home].loading).await;
        // Fresh now, so opening again does not refetch.
        store.open(BrowseTarget::Home);
        assert!(!store.state().pages[&BrowseTarget::Home].loading);
    }

    #[tokio::test]
    async fn continuations_append_to_the_page() {
        let store = store();
        store.start().await;
        store.open(BrowseTarget::Home);
        settle(&store, |state| state.page(&BrowseTarget::Home).is_some()).await;
        let before = store
            .state()
            .page(&BrowseTarget::Home)
            .unwrap()
            .sections
            .len();
        store.load_more(BrowseTarget::Home);
        settle(&store, |state| {
            state.page(&BrowseTarget::Home).unwrap().sections.len() > before
        })
        .await;
        assert!(!store.state().pages[&BrowseTarget::Home].loading_more);
    }

    #[tokio::test(start_paused = true)]
    async fn suggestions_wait_for_typing_to_settle() {
        let store = store();
        store.start().await;
        let mut events = store.events();
        for query in ["h", "ha", "har"] {
            store.suggest(query);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(store.state().suggestions.query, "har");
        let mut answers = 0;
        while let Ok(event) = events.try_recv() {
            answers += usize::from(event == StoreEvent::Suggestions);
        }
        assert_eq!(answers, 1);
    }

    #[tokio::test]
    async fn position_ticks_move_the_lyric_line_and_nothing_else() {
        let store = store();
        let track = crate::demo::catalog().albums[0].tracks[0].clone();
        store.apply(Event::Player(PlayerState {
            track: Some(track.clone()),
            status: Status::Paused,
            ..PlayerState::default()
        }));
        store.inner.update(|state, _| {
            let line = |start_ms| LyricLine {
                start_ms,
                end_ms: None,
                text: String::new(),
                ..LyricLine::default()
            };
            state.lyrics.insert(
                track.video_id.clone(),
                LyricsEntry {
                    lyrics: Some(Arc::new(Lyrics {
                        source: None,
                        lines: vec![line(0), line(4000)],
                        synced: true,
                        word_synced: false,
                    })),
                    ..LyricsEntry::default()
                },
            );
        });
        let mut events = store.events();
        store.apply(Event::Position {
            position_ms: 4500,
            buffered_ms: 9000,
        });
        assert_eq!(events.try_recv().unwrap(), StoreEvent::Position);
        assert_eq!(events.try_recv().unwrap(), StoreEvent::LyricLine);
        assert!(events.try_recv().is_err());
        store.apply(Event::Position {
            position_ms: 4750,
            buffered_ms: 9000,
        });
        assert_eq!(events.try_recv().unwrap(), StoreEvent::Position);
        assert!(events.try_recv().is_err());
        assert_eq!(store.state().lyric_line, Some(1));
    }

    #[test]
    fn eviction_keeps_pinned_pages_and_drops_the_least_recently_viewed() {
        let mut state = AppState::default();
        let page = |n| PageEntry {
            viewed_at: Some(Instant::now() + Duration::from_secs(n)),
            ..PageEntry::default()
        };
        state.pages.insert(BrowseTarget::Home, PageEntry::default());
        for n in 0..(KEPT_PAGES as u64 + 5) {
            state
                .pages
                .insert(BrowseTarget::Album(n.to_string()), page(n));
        }
        evict_pages(&mut state);
        assert_eq!(state.pages.len(), KEPT_PAGES);
        assert!(state.pages.contains_key(&BrowseTarget::Home));
        assert!(!state.pages.contains_key(&BrowseTarget::Album("0".into())));
        assert!(
            state
                .pages
                .contains_key(&BrowseTarget::Album((KEPT_PAGES as u64 + 4).to_string()))
        );
    }

    #[test]
    fn the_position_moves_on_between_ticks_only_while_playing() {
        let mut state = AppState::default();
        state.player.duration_ms = Some(10_000);
        state.position_ms = 1_000;
        state.position_at = Some(Instant::now() - Duration::from_millis(500));
        assert_eq!(state.position_now(), 1_000);
        state.player.status = Status::Playing;
        assert!(state.position_now() >= 1_500);
        state.position_at = Some(Instant::now() - Duration::from_secs(60));
        assert_eq!(state.position_now(), 10_000);
    }

    #[tokio::test]
    async fn queue_moves_and_removals_keep_the_current_track() {
        let store = store();
        let tracks: Vec<Track> = crate::demo::catalog().albums[0]
            .tracks
            .iter()
            .take(4)
            .cloned()
            .collect();
        store.apply(Event::Queue(QueueState {
            tracks: tracks.clone(),
            current: Some(1),
            source_title: None,
            radio: false,
        }));
        store.move_in_queue(1, 3);
        assert_eq!(store.state().queue.current, Some(3));
        assert_eq!(store.state().queue.tracks[3].video_id, tracks[1].video_id);
        store.remove_from_queue(0);
        assert_eq!(store.state().queue.current, Some(2));
    }

    #[tokio::test]
    async fn a_library_change_marks_its_pages_stale() {
        let store = store();
        store.start().await;
        let target = BrowseTarget::Library(LibraryTab::Playlists);
        store.open(target.clone());
        settle(&store, |state| state.page(&target).is_some()).await;
        assert!(store.state().pages[&target].fetched_at.is_some());
        store.apply(Event::LibraryChanged {
            scope: LibraryScope::Likes,
        });
        assert!(store.state().pages[&target].fetched_at.is_some());
        store.apply(Event::LibraryChanged {
            scope: LibraryScope::Playlists,
        });
        settle(&store, |state| {
            state.pages[&target].fetched_at.is_some() && !state.pages[&target].loading
        })
        .await;
    }
}
