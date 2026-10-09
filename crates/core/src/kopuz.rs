//! kopuzd over `kopuz-client`: gRPC on a unix socket, or a named pipe on
//! Windows. The connection comes back with backoff when the daemon goes
//! away, and a missing daemon is started once, since a dev box may have no
//! service for it.
//!
//! FormalMusic is a YouTube Music client, so it names that one service when
//! it sets up its source. Everything past that reads what the source says it
//! can do, never which service it is.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use api::prelude::*;
use api::{
    ApiEvent, CatalogDetail, FieldValue, Handshake, PageEntry, QueueContext, QueueEdit, QueueMode,
    SetQueueRequest, SourceDraft, SourceInfo, Table,
};
use async_trait::async_trait;
use futures_util::StreamExt;
use parking_lot::{Mutex, RwLock};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use crate::backend::{Backend, BackendKind, ClientError, ConnectionStatus, Control, Event, Result};
use crate::convert::{self, Context};
use crate::equalizer::Equalizer;
use crate::model::*;
use crate::settings::Settings;

/// The service FormalMusic's source speaks, as kopuzd names it.
const SERVICE: &str = "ytmusic";
const SOURCE_NAME: &str = "YouTube Music";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Pages, searches and starting playback go out to YouTube, which takes
/// seconds on a bad network.
const SLOW_TIMEOUT: Duration = Duration::from_secs(45);
/// A browser sign-in lasts as long as the user takes.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(320);
const RETRY_MIN: Duration = Duration::from_millis(250);
const RETRY_MAX: Duration = Duration::from_secs(10);
/// A daemon that dies at once would otherwise be restarted on every retry.
const SPAWN_GAP: Duration = Duration::from_secs(30);
/// Failed attempts in a row before the status says offline rather than connecting.
const OFFLINE_AFTER: u32 = 4;
/// Rows a liked songs page shows.
const LIKED_LIMIT: usize = 5000;

/// `FORMALMUSIC_SOCKET`, else where kopuzd listens by default: the user's
/// runtime dir (the cache dir on macOS), or a per-user named pipe.
pub fn socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("FORMALMUSIC_SOCKET").filter(|path| !path.is_empty()) {
        return path.into();
    }
    default_socket()
}

#[cfg(windows)]
fn default_socket() -> PathBuf {
    proto::pipe::default_name()
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"\\.\pipe\kopuz"))
}

#[cfg(not(windows))]
fn default_socket() -> PathBuf {
    dirs::runtime_dir()
        .or_else(dirs::cache_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("kopuz")
        .join("kopuzd.sock")
}

/// What the backend learned from the daemon and converts with.
#[derive(Default)]
struct Known {
    source: Option<SourceInfo>,
    favorites: HashSet<String>,
    /// The playing track's length and its buffered byte ranges, for the
    /// buffered time the seek bar shows.
    duration_ms: Option<u64>,
    buffered_ms: u64,
    /// What each of the account's playlists allows, by its id without `VL`,
    /// for those the source tells apart.
    playlists: HashMap<String, api::PlaylistCapability>,
}

impl Known {
    fn pages(&self) -> &[PageEntry] {
        self.source
            .as_ref()
            .map_or(&[], |source| &source.capabilities.pages)
    }
}

struct Shared {
    api: client::GrpcApi,
    socket: PathBuf,
    known: RwLock<Known>,
    /// Set once the source is settled, so its pages can be named.
    settled: tokio::sync::watch::Sender<bool>,
    stopped: AtomicBool,
    task: Mutex<Option<AbortHandle>>,
    last_spawn: Mutex<Option<Instant>>,
}

pub struct KopuzBackend {
    shared: Arc<Shared>,
}

impl KopuzBackend {
    pub fn new(socket: PathBuf) -> Self {
        let api =
            client::GrpcApi::new(socket.clone()).expect("a socket client never fails to build");
        Self {
            shared: Arc::new(Shared {
                api,
                socket,
                known: RwLock::default(),
                settled: tokio::sync::watch::channel(false).0,
                stopped: AtomicBool::new(false),
                task: Mutex::new(None),
                last_spawn: Mutex::new(None),
            }),
        }
    }

    fn api(&self) -> &client::GrpcApi {
        &self.shared.api
    }

    fn source_id(&self) -> Result<String> {
        self.shared
            .known
            .read()
            .source
            .as_ref()
            .map(|source| source.id.clone())
            .ok_or_else(|| ClientError::Disconnected("The music daemon is not running.".into()))
    }

    /// The request for `target`, once the source has said which pages it
    /// has: a page opened while the app is still connecting waits for that.
    async fn request(&self, target: &BrowseTarget) -> Result<api::CatalogDetailRequest> {
        let mut settled = self.shared.settled.subscribe();
        let _ = tokio::time::timeout(SLOW_TIMEOUT, settled.wait_for(|settled| *settled)).await;
        convert::request(self.shared.known.read().pages(), target)
            .ok_or_else(|| ClientError::Unsupported("YouTube Music has no such page here".into()))
    }

    fn convert<T>(&self, f: impl FnOnce(&Context) -> T) -> T {
        let known = self.shared.known.read();
        f(&Context {
            favorites: Some(&known.favorites),
            pages: known.pages(),
        })
    }

    async fn detail(&self, request: api::CatalogDetailRequest) -> Result<CatalogDetail> {
        call(SLOW_TIMEOUT, self.api().catalog_detail(request)).await
    }

    /// Replaces the stored source with what an answer says it is now.
    fn learn_source(&self, source: SourceInfo) -> SessionInfo {
        let session = session_of(&source);
        self.shared.known.write().source = Some(source);
        session
    }

    /// Makes the source usable with an account: one set up to run
    /// anonymously ignores any session it is given.
    async fn signing_in(&self) -> Result<String> {
        let id = self.source_id()?;
        let anonymous = self
            .shared
            .known
            .read()
            .source
            .as_ref()
            .is_some_and(|source| source.anonymous);
        if anonymous {
            let source = call(
                REQUEST_TIMEOUT,
                self.api().upsert_source(draft(Some(id.clone()), "browser")),
            )
            .await?;
            self.learn_source(source);
        }
        Ok(id)
    }

    /// After a sign-in: the account is checked with YouTube before the
    /// session says it is signed in. A session that is not an account is
    /// taken off the source again, which then runs anonymously as before.
    async fn signed_in(&self, source: SourceInfo) -> Result<SessionInfo> {
        let id = source.id.clone();
        self.learn_source(source);
        let state = call(SLOW_TIMEOUT, self.api().validate_source(id.clone())).await?;
        let session = self.refresh_source().await?;
        if state == api::SourceState::Online && session.signed_in {
            return Ok(session);
        }
        self.sign_out().await?;
        Err(ClientError::BadRequest(
            "YouTube Music did not accept that session.".into(),
        ))
    }

    async fn refresh_source(&self) -> Result<SessionInfo> {
        let id = self.source_id()?;
        let sources = call(REQUEST_TIMEOUT, self.api().sources()).await?;
        let source = sources
            .into_iter()
            .find(|source| source.id == id)
            .ok_or_else(|| ClientError::NotFound("the YouTube Music source".into()))?;
        let session = self.learn_source(source);
        Ok(self.with_account(session).await)
    }

    /// The session with the account it acts as, for a source with several
    /// under one sign-in.
    async fn with_account(&self, mut session: SessionInfo) -> SessionInfo {
        let (id, several, avatar) = {
            let known = self.shared.known.read();
            match &known.source {
                Some(source) => (
                    source.id.clone(),
                    source.capabilities.accounts,
                    source.avatar.clone(),
                ),
                None => return session,
            }
        };
        if session.signed_in && several {
            match call(REQUEST_TIMEOUT, self.api().accounts(id)).await {
                Ok(accounts) => {
                    session.account = accounts
                        .iter()
                        .find(|a| a.active)
                        .map(|a| account(a, avatar.as_ref()));
                }
                Err(error) => tracing::debug!("accounts: {error}"),
            }
        }
        session
    }

    async fn integrations(&self) -> Result<ScrobbleStatus> {
        let integrations = call(REQUEST_TIMEOUT, self.api().integrations()).await?;
        let connected = |id: &str| ScrobbleAccount {
            connected: integrations
                .iter()
                .any(|integration| integration.id == id && integration.configured),
            connecting: false,
        };
        Ok(ScrobbleStatus {
            lastfm: connected("lastfm"),
            listenbrainz: connected("listenbrainz"),
        })
    }

    /// Queues `keys` as `mode`, starting at `start` for a replace.
    async fn queue(
        &self,
        mode: QueueMode,
        context: QueueContext,
        start: Option<usize>,
        shuffle: Option<bool>,
    ) -> Result<()> {
        call(
            SLOW_TIMEOUT,
            self.api().set_queue(SetQueueRequest {
                mode,
                context,
                start_index: start.map(|index| index as u32),
                shuffle,
            }),
        )
        .await
        .map(drop)
    }
}

fn draft(id: Option<String>, auth_method: &str) -> SourceDraft {
    SourceDraft {
        id,
        name: SOURCE_NAME.into(),
        service: SERVICE.into(),
        values: vec![FieldValue::new("auth_method", auth_method)],
        secrets: Vec::new(),
    }
}

/// Signed in means an account: the source holds credentials, is not set to
/// run anonymously, and offers what only an account has.
fn session_of(source: &SourceInfo) -> SessionInfo {
    SessionInfo {
        signed_in: source.authenticated && !source.anonymous && source.capabilities.rate,
        account: None,
        premium: false,
    }
}

/// An account as the menu lists it; only the active one has a picture,
/// the one the source is signed in as.
fn account(account: &api::SourceAccount, avatar: Option<&api::ArtworkRef>) -> Account {
    Account {
        name: account.name.clone(),
        handle: account.handle.clone(),
        art: avatar.filter(|_| account.active).map(convert::art),
        page_id: account.id.clone(),
        selected: account.active,
    }
}

/// A playlist's id as the library lists it: a browse id without its `VL`.
fn bare(playlist_id: &str) -> &str {
    playlist_id.strip_prefix("VL").unwrap_or(playlist_id)
}

fn text(text: &api::Text) -> String {
    match text {
        api::Text::Key(key) | api::Text::Literal(key) => key.clone(),
    }
}

async fn call<T>(
    timeout: Duration,
    future: impl Future<Output = std::result::Result<T, api::ApiError>>,
) -> Result<T> {
    match tokio::time::timeout(timeout, future).await {
        Ok(result) => result.map_err(convert::error),
        Err(_) => Err(ClientError::Timeout),
    }
}

fn scope(table: Table) -> Option<LibraryScope> {
    Some(match table {
        Table::Favorites => LibraryScope::Likes,
        Table::Playlists => LibraryScope::Playlists,
        Table::Albums => LibraryScope::Albums,
        Table::Recents => LibraryScope::History,
        _ => return None,
    })
}

impl Shared {
    fn backend(self: &Arc<Self>) -> KopuzBackend {
        KopuzBackend {
            shared: self.clone(),
        }
    }

    fn player(&self, state: &api::PlayerState) -> PlayerState {
        let mut known = self.known.write();
        let shown = state
            .fading
            .as_ref()
            .map(|fading| &fading.track)
            .or(state.track.as_ref());
        known.duration_ms = shown
            .and_then(|track| track.duration_ms)
            .filter(|ms| *ms > 0);
        known.buffered_ms = convert::buffered_ms(&state.buffered, known.duration_ms);
        convert::player(
            state,
            &Context {
                favorites: Some(&known.favorites),
                pages: known.pages(),
            },
        )
    }

    async fn refresh_playlists(&self) {
        if let Ok(catalog) = call(REQUEST_TIMEOUT, self.api.playlists()).await {
            self.learn_playlists(&catalog);
        }
    }

    fn learn_playlists(&self, catalog: &api::PlaylistCatalog) {
        self.known.write().playlists = catalog
            .playlists
            .iter()
            .filter_map(|playlist| Some((bare(&playlist.id).to_owned(), playlist.capability?)))
            .collect();
    }

    async fn queue_event(&self) -> Option<Event> {
        let snapshot = call(REQUEST_TIMEOUT, self.api.queue_snapshot())
            .await
            .ok()?;
        let known = self.known.read();
        Some(Event::Queue(convert::queue(
            &snapshot,
            &Context {
                favorites: Some(&known.favorites),
                pages: known.pages(),
            },
        )))
    }

    async fn refresh_favorites(&self) {
        if let Ok(favorites) = call(REQUEST_TIMEOUT, self.api.favorites()).await {
            self.known.write().favorites = favorites.refs.into_iter().collect();
        }
    }

    /// The source FormalMusic plays from, made active: the active one when
    /// it speaks YouTube Music, else the first that does, else a new one
    /// that works without an account until the user signs in.
    async fn settle_source(&self) -> Result<SourceInfo> {
        let sources = call(REQUEST_TIMEOUT, self.api.sources()).await?;
        let ours = sources
            .iter()
            .find(|source| source.active && source.service.id == SERVICE)
            .or_else(|| sources.iter().find(|source| source.service.id == SERVICE))
            .cloned();
        let source = match ours {
            Some(source) => source,
            None => {
                call(
                    REQUEST_TIMEOUT,
                    self.api.upsert_source(draft(None, "anonymous")),
                )
                .await?
            }
        };
        if source.active {
            return Ok(source);
        }
        call(REQUEST_TIMEOUT, self.api.switch_source(source.id.clone())).await
    }

    /// Everything a fresh connection reports before its events: the
    /// session, the player and the queue.
    async fn snapshot(self: &Arc<Self>, events: &mpsc::UnboundedSender<Event>) -> Result<()> {
        let source = self.settle_source().await?;
        let session = self.backend().learn_source(source);
        self.settled.send_replace(true);
        let session = self.backend().with_account(session).await;
        let _ = events.send(Event::Session(session));
        self.refresh_favorites().await;
        self.refresh_playlists().await;
        if let Ok(state) = call(REQUEST_TIMEOUT, self.api.player_state()).await {
            let _ = events.send(Event::Player(self.player(&state)));
        }
        if let Some(queue) = self.queue_event().await {
            let _ = events.send(queue);
        }
        Ok(())
    }

    async fn apply(self: &Arc<Self>, event: ApiEvent, events: &mpsc::UnboundedSender<Event>) {
        match event {
            ApiEvent::PlayerState(state) => {
                let _ = events.send(Event::Player(self.player(&state)));
            }
            ApiEvent::PlayerPosition { position_ms, .. } => {
                let buffered_ms = self.known.read().buffered_ms;
                let _ = events.send(Event::Position {
                    position_ms,
                    buffered_ms,
                });
            }
            ApiEvent::PlayerBuffered { ranges, .. } => {
                let buffered_ms = {
                    let mut known = self.known.write();
                    known.buffered_ms = convert::buffered_ms(&ranges, known.duration_ms);
                    known.buffered_ms
                };
                let _ = events.send(Event::Buffered { buffered_ms });
            }
            ApiEvent::QueueChanged { .. } => {
                if let Some(queue) = self.queue_event().await {
                    let _ = events.send(queue);
                }
            }
            ApiEvent::LibraryInvalidated { table } => {
                if table == Table::Favorites {
                    self.refresh_favorites().await;
                }
                if table == Table::Playlists {
                    self.refresh_playlists().await;
                }
                if table == Table::Servers
                    && let Ok(session) = self.backend().refresh_source().await
                {
                    let _ = events.send(Event::Session(session));
                }
                if let Some(scope) = scope(table) {
                    let _ = events.send(Event::LibraryChanged { scope });
                }
            }
            ApiEvent::SourceStatus { state, .. } => {
                if matches!(state, api::SourceState::AuthExpired)
                    && let Ok(session) = self.backend().refresh_source().await
                {
                    let _ = events.send(Event::Session(SessionInfo {
                        signed_in: false,
                        ..session
                    }));
                }
            }
            ApiEvent::Notice {
                level: api::NoticeLevel::Warning | api::NoticeLevel::Error,
                message: Some(message),
                ..
            } => {
                let _ = events.send(Event::Notice { message });
            }
            ApiEvent::Resync => {
                let _ = self.snapshot(events).await;
            }
            _ => {}
        }
    }

    /// Starts kopuzd: through its systemd user unit when there is one, else
    /// from beside this binary or `PATH`, in its own process group so it
    /// outlives the window and ignores the terminal's signals.
    fn spawn_daemon(&self) {
        {
            let mut last = self.last_spawn.lock();
            if last.is_some_and(|at| at.elapsed() < SPAWN_GAP) {
                return;
            }
            *last = Some(Instant::now());
        }
        if start_unit() {
            return;
        }
        let beside = std::env::current_exe()
            .ok()
            .and_then(|exe| {
                let name = format!("kopuzd{}", std::env::consts::EXE_SUFFIX);
                exe.parent().map(|dir| dir.join(name))
            })
            .filter(|path| path.is_file());
        let program = beside.unwrap_or_else(|| PathBuf::from("kopuzd"));
        let mut command = crate::process::command(&program);
        if std::env::var_os("FORMALMUSIC_SOCKET").is_some() {
            command.arg("--socket").arg(&self.socket);
        }
        // The media flyout names and badges the session by the process's
        // AppUserModelID, which the installer registers for the app.
        if cfg!(windows) {
            command.args(["--app-id", crate::APP_ID, "--app-name", crate::APP_NAME]);
            let icon = program
                .parent()
                .map(|dir| dir.join("formalmusic.ico"))
                .filter(|icon| icon.is_file());
            if let Some(icon) = icon {
                command.arg("--app-icon").arg(icon);
            }
        }
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        match command.spawn() {
            Ok(mut child) => {
                tracing::info!("started {}", program.display());
                // Reaped here so an early exit does not leave a zombie behind.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(error) => tracing::warn!("could not start {}: {error}", program.display()),
        }
    }
}

/// A daemon started by the window lives in the window's scope and holds the
/// socket, so the unit then fails to start. Asking systemd first keeps the
/// unit the owner; `reset-failed` clears an earlier failure.
#[cfg(target_os = "linux")]
fn start_unit() -> bool {
    let systemctl = |args: &[&str]| {
        std::process::Command::new("systemctl")
            .arg("--user")
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    if !systemctl(&["cat", "kopuzd.service"]) {
        return false;
    }
    let _ = systemctl(&["reset-failed", "kopuzd.service"]);
    let started = systemctl(&["start", "kopuzd.service"]);
    if started {
        tracing::info!("started kopuzd.service");
    }
    started
}

#[cfg(not(target_os = "linux"))]
fn start_unit() -> bool {
    false
}

async fn supervise(shared: Arc<Shared>, events: mpsc::UnboundedSender<Event>) {
    let mut delay = RETRY_MIN;
    let mut failures = 0u32;
    let status = |status, error: Option<String>| {
        let _ = events.send(Event::Connection { status, error });
    };
    while !shared.stopped.load(Ordering::SeqCst) {
        let error = match tokio::time::timeout(CONNECT_TIMEOUT, shared.api.handshake()).await {
            Ok(Ok(Handshake::Ready(_))) => match shared.snapshot(&events).await {
                Ok(()) => {
                    failures = 0;
                    delay = RETRY_MIN;
                    status(ConnectionStatus::Online, None);
                    let mut stream = shared.api.events();
                    while let Some(event) = stream.next().await {
                        shared.apply(event, &events).await;
                    }
                    if shared.stopped.load(Ordering::SeqCst) {
                        return;
                    }
                    "The music daemon closed the connection.".to_owned()
                }
                Err(error) => error.to_string(),
            },
            Ok(Ok(Handshake::Mismatched { daemon, client })) => format!(
                "kopuzd speaks wire revision {daemon} and this build {client}. Update one of them."
            ),
            Ok(Err(error)) if error.code == api::ErrorCode::DaemonGone => {
                shared.spawn_daemon();
                "The music daemon is not running.".to_owned()
            }
            Ok(Err(error)) => format!("Could not reach the music daemon: {}", error.message),
            Err(_) => "The music daemon did not answer.".to_owned(),
        };
        failures += 1;
        status(
            if failures >= OFFLINE_AFTER {
                ConnectionStatus::Offline
            } else {
                ConnectionStatus::Connecting
            },
            Some(error),
        );
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(RETRY_MAX);
    }
}

#[async_trait]
impl Backend for KopuzBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Daemon
    }

    fn start(&self, events: mpsc::UnboundedSender<Event>) {
        let task = tokio::spawn(supervise(self.shared.clone(), events));
        *self.shared.task.lock() = Some(task.abort_handle());
    }

    fn stop(&self) {
        self.shared.stopped.store(true, Ordering::SeqCst);
        if let Some(task) = self.shared.task.lock().take() {
            task.abort();
        }
    }

    async fn session(&self) -> Result<SessionInfo> {
        self.refresh_source().await
    }

    async fn browse(&self, target: &BrowseTarget) -> Result<Page> {
        match target {
            BrowseTarget::Library(LibraryTab::Playlists) => {
                let catalog = call(REQUEST_TIMEOUT, self.api().playlists()).await?;
                self.shared.learn_playlists(&catalog);
                let items = catalog
                    .playlists
                    .iter()
                    .map(|playlist| Item::Playlist {
                        playlist_id: playlist.id.clone(),
                        title: playlist.name.clone(),
                        subtitle: match playlist.track_keys.len() {
                            0 => Some("Playlist".into()),
                            1 => Some("Playlist \u{2022} 1 track".into()),
                            n => Some(format!("Playlist \u{2022} {n} tracks")),
                        },
                        art: playlist.artwork.as_ref().map(convert::art),
                        actions: Actions::default(),
                        web_url: None,
                    })
                    .collect();
                Ok(self.library_page(target, items))
            }
            BrowseTarget::Library(LibraryTab::LikedSongs) => {
                let favorites = call(REQUEST_TIMEOUT, self.api().favorites()).await?;
                let keys: Vec<String> = favorites.refs.into_iter().take(LIKED_LIMIT).collect();
                let rows = call(REQUEST_TIMEOUT, self.api().tracks_by_keys(keys)).await?;
                let items = self.convert(|ctx| {
                    rows.iter()
                        .map(|info| {
                            let liked = Actions {
                                rating: Some(Rating::Like),
                                ..Actions::default()
                            };
                            Item::Track(convert::track(info, TrackKind::Song, liked, ctx))
                        })
                        .collect()
                });
                let mut page = self.library_page(target, items);
                page.sections[0].layout = SectionLayout::List;
                Ok(page)
            }
            _ => {
                let request = self.request(target).await?;
                let detail = self.detail(request).await?;
                let mut page = self.convert(|ctx| convert::page(target.clone(), &detail, ctx));
                if let BrowseTarget::Library(_) = target {
                    page.chips = self.library_chips(target);
                }
                Ok(page)
            }
        }
    }

    async fn more(
        &self,
        target: &BrowseTarget,
        section: Option<usize>,
        token: &Continuation,
    ) -> Result<ContinuationPage> {
        let mut request = self.request(target).await?;
        request.continuation = Some(token.0.clone());
        let detail = self.detail(request).await?;
        Ok(self.convert(|ctx| convert::more(&detail, section, ctx)))
    }

    async fn search(&self, query: &str, filter: Option<SearchFilter>) -> Result<SearchResults> {
        let request = api::SearchRequest {
            query: query.to_owned(),
            filter: Some(filter.map_or("all", SearchFilter::id).to_owned()),
            continuation: None,
        };
        let results = call(SLOW_TIMEOUT, self.api().search(request)).await?;
        Ok(self.convert(|ctx| convert::search(query, filter, &results, ctx)))
    }

    async fn search_more(
        &self,
        query: &str,
        filter: Option<SearchFilter>,
        token: &Continuation,
    ) -> Result<ContinuationPage> {
        let request = api::SearchRequest {
            query: query.to_owned(),
            filter: Some(filter.map_or("all", SearchFilter::id).to_owned()),
            continuation: Some(token.0.clone()),
        };
        let results = call(SLOW_TIMEOUT, self.api().search(request)).await?;
        Ok(self.convert(|ctx| convert::search_more(&results, ctx)))
    }

    async fn suggestions(&self, query: &str) -> Result<Vec<Suggestion>> {
        let suggestions = call(
            REQUEST_TIMEOUT,
            self.api().search_suggestions(query.to_owned()),
        )
        .await?;
        Ok(self.convert(|ctx| {
            suggestions
                .iter()
                .filter_map(|suggestion| convert::suggestion(suggestion, ctx))
                .collect()
        }))
    }

    fn search_filters(&self) -> Vec<SearchFilter> {
        let known = self.shared.known.read();
        known.source.as_ref().map_or_else(Vec::new, |source| {
            source
                .capabilities
                .search_filters
                .iter()
                .filter_map(|filter| SearchFilter::from_id(&filter.id))
                .collect()
        })
    }

    async fn lyrics(&self, key: &str) -> Result<Option<Lyrics>> {
        match call(SLOW_TIMEOUT, self.api().lyrics(key.to_owned())).await {
            Ok(view) => Ok(convert::lyrics(&view)),
            Err(ClientError::NotFound(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn related(&self, key: &str) -> Result<Page> {
        let detail = self
            .detail(api::CatalogDetailRequest::new(
                api::CatalogItemKind::Track,
                key,
            ))
            .await?;
        Ok(self.convert(|ctx| convert::page(BrowseTarget::Page(key.to_owned()), &detail, ctx)))
    }

    async fn artwork(&self, art: &Art, hq: bool) -> Result<Vec<u8>> {
        let target = convert::artwork_target(art)
            .ok_or_else(|| ClientError::NotFound("no such picture".into()))?;
        let data = call(
            SLOW_TIMEOUT,
            self.api().artwork(api::ArtworkRequest { target, hq }),
        )
        .await?;
        Ok(data.bytes)
    }

    async fn share_url(&self, item: &Item) -> Result<Option<String>> {
        let url = match item {
            Item::Track(track) => self.api().track_web_url(track.key.clone()).await,
            // Albums, artists and podcasts all open by the id they are browsed under.
            Item::Album { browse_id, .. }
            | Item::Artist { browse_id, .. }
            | Item::Podcast { browse_id, .. } => self.api().album_web_url(browse_id.clone()).await,
            Item::Playlist {
                web_url: Some(url), ..
            } => return Ok(Some(url.clone())),
            // A library row carries no link; the playlist's page does.
            Item::Playlist { playlist_id, .. } => {
                let request = api::CatalogDetailRequest::new(
                    api::CatalogItemKind::Playlist,
                    playlist_id.clone(),
                );
                return Ok(self.detail(request).await?.web_url);
            }
            _ => return Ok(None),
        };
        url.map_err(convert::error)
    }

    fn features(&self) -> Features {
        let known = self.shared.known.read();
        let Some(source) = &known.source else {
            return Features::default();
        };
        let can = &source.capabilities;
        Features {
            stream_quality: can.stream_quality,
            explicit_flags: can.explicit_flags,
            watch_history: can.watch_history,
            music_videos: can.music_videos,
            track_radio: can.track_radio,
        }
    }

    async fn video(&self, key: &str, start: u64, length: Option<u64>) -> Result<VideoChunk> {
        let request = api::VideoRequest {
            key: key.to_owned(),
            start,
            length,
        };
        let chunk = call(SLOW_TIMEOUT, self.api().video(request)).await?;
        Ok(VideoChunk {
            content_type: chunk.content_type,
            start: chunk.start,
            total: chunk.total,
            bytes: chunk.bytes,
        })
    }

    async fn play(&self, source: PlaySource, start_index: usize, shuffle: bool) -> Result<()> {
        match source {
            PlaySource::Tracks { tracks } => {
                self.queue(
                    QueueMode::Replace,
                    QueueContext::Tracks {
                        keys: tracks.into_iter().map(|track| track.key).collect(),
                    },
                    Some(start_index),
                    Some(shuffle),
                )
                .await
            }
            // kopuzd resolves an album or a playlist by its id, saved or not,
            // and answers NotFound for one it cannot.
            PlaySource::Page { target } => {
                let context = match target {
                    BrowseTarget::Album(id) => QueueContext::Album { id },
                    BrowseTarget::Playlist(id) => QueueContext::Playlist { id },
                    other => {
                        return Err(ClientError::Unsupported(format!(
                            "{other:?} does not play whole"
                        )));
                    }
                };
                self.queue(
                    QueueMode::Replace,
                    context,
                    Some(start_index),
                    Some(shuffle),
                )
                .await
            }
            PlaySource::Radio { key } => {
                self.queue(
                    QueueMode::Replace,
                    QueueContext::TrackRadio { key },
                    None,
                    None,
                )
                .await
            }
            PlaySource::PlaylistRadio { id } => {
                self.queue(
                    QueueMode::Replace,
                    QueueContext::PlaylistRadio { id },
                    None,
                    None,
                )
                .await
            }
            PlaySource::Artist { key } => {
                self.queue(
                    QueueMode::Replace,
                    QueueContext::Artist { artist: key },
                    None,
                    Some(shuffle),
                )
                .await
            }
        }
    }

    async fn enqueue(&self, tracks: Vec<Track>, position: EnqueuePosition) -> Result<()> {
        let mode = match position {
            EnqueuePosition::Next => QueueMode::PlayNext,
            EnqueuePosition::End => QueueMode::Append,
        };
        let keys = tracks.into_iter().map(|track| track.key).collect();
        self.queue(mode, QueueContext::Tracks { keys }, None, None)
            .await
    }

    async fn control(&self, control: Control) -> Result<()> {
        use api::PlayerCommand as C;
        let command = match control {
            Control::Toggle => C::Toggle,
            Control::Pause => C::Pause,
            Control::Next => C::Next,
            Control::Previous => C::Previous,
            Control::Seek(position_ms) => C::Seek { position_ms },
            Control::Volume(volume) => C::SetVolume { volume },
            Control::Muted(muted) => C::SetMuted { muted },
            Control::Repeat(repeat) => C::SetMode {
                shuffle: None,
                loop_mode: Some(convert::loop_mode(repeat)),
            },
            Control::Shuffle(shuffle) => C::SetMode {
                shuffle: Some(shuffle),
                loop_mode: None,
            },
            Control::Jump(index) => {
                let edit = QueueEdit::Jump {
                    index: index as u32,
                };
                return call(REQUEST_TIMEOUT, self.api().queue_edit(edit))
                    .await
                    .map(drop);
            }
            Control::Remove(index) => {
                let edit = QueueEdit::Remove {
                    index: index as u32,
                };
                return call(REQUEST_TIMEOUT, self.api().queue_edit(edit))
                    .await
                    .map(drop);
            }
            Control::Move { from, to } => {
                let edit = QueueEdit::Move {
                    from: from as u32,
                    to: to as u32,
                };
                return call(REQUEST_TIMEOUT, self.api().queue_edit(edit))
                    .await
                    .map(drop);
            }
            Control::Version(mode) => C::SetVersion {
                version: convert::version(mode),
            },
            Control::Clear => {
                let empty = QueueContext::Tracks { keys: Vec::new() };
                return self.queue(QueueMode::Replace, empty, None, None).await;
            }
        };
        call(REQUEST_TIMEOUT, self.api().player_command(command))
            .await
            .map(drop)
    }

    async fn rate(&self, rate_ref: &str, rating: Rating) -> Result<()> {
        call(
            REQUEST_TIMEOUT,
            self.api()
                .rate(rate_ref.to_owned(), convert::api_rating(rating)),
        )
        .await
    }

    async fn follow(&self, follow_ref: &str, follow: bool) -> Result<()> {
        call(
            REQUEST_TIMEOUT,
            self.api().follow(follow_ref.to_owned(), follow),
        )
        .await
    }

    async fn save(&self, save_ref: &str, saved: bool) -> Result<()> {
        call(REQUEST_TIMEOUT, self.api().save(save_ref.to_owned(), saved)).await
    }

    async fn remove_from_history(&self, token: &str) -> Result<()> {
        call(
            REQUEST_TIMEOUT,
            self.api().remove_from_history(token.to_owned()),
        )
        .await
    }

    async fn create_playlist(&self, title: String, keys: Vec<String>) -> Result<String> {
        call(SLOW_TIMEOUT, self.api().create_playlist(title, keys)).await
    }

    async fn add_to_playlist(&self, playlist_id: &str, keys: Vec<String>) -> Result<()> {
        call(
            SLOW_TIMEOUT,
            self.api().add_playlist_tracks(playlist_id.to_owned(), keys),
        )
        .await
    }

    async fn remove_from_playlist(&self, playlist_id: &str, index: usize) -> Result<()> {
        call(
            SLOW_TIMEOUT,
            self.api()
                .remove_playlist_track(playlist_id.to_owned(), index as u32),
        )
        .await
    }

    async fn move_in_playlist(&self, playlist_id: &str, from: usize, to: usize) -> Result<()> {
        let reorder = api::PlaylistReorder {
            from: from as u32,
            to: to as u32,
        };
        call(
            SLOW_TIMEOUT,
            self.api().reorder_playlist(playlist_id.to_owned(), reorder),
        )
        .await
    }

    fn playlist_reorders(&self, playlist_id: &str) -> bool {
        let known = self.shared.known.read();
        let Some(source) = &known.source else {
            return false;
        };
        let capability = known
            .playlists
            .get(bare(playlist_id))
            .copied()
            .unwrap_or(source.capabilities.playlists);
        capability == api::PlaylistCapability::Reorder
    }

    async fn edit_playlist(&self, playlist_id: &str, details: PlaylistDetails) -> Result<()> {
        let edit = api::PlaylistEdit {
            name: details.title,
            description: details.description,
            privacy: details.privacy.map(convert::api_privacy),
        };
        call(
            SLOW_TIMEOUT,
            self.api().edit_playlist(playlist_id.to_owned(), edit),
        )
        .await
    }

    async fn delete_playlist(&self, playlist_id: &str) -> Result<()> {
        call(
            SLOW_TIMEOUT,
            self.api().delete_playlist(playlist_id.to_owned()),
        )
        .await
    }

    async fn sign_in(&self, cookies: String) -> Result<SessionInfo> {
        let id = self.signing_in().await?;
        let source = call(
            SLOW_TIMEOUT,
            self.api().provision_credentials(api::CredentialProvision {
                server_id: id,
                secret: cookies,
                user_id: None,
                browser: None,
            }),
        )
        .await?;
        self.signed_in(source).await
    }

    async fn browsers(&self) -> Result<Browsers> {
        // An anonymous source has no sign-in options of its own; the
        // service's form for a new one lists the same browsers.
        let field =
            |fields: &[api::FieldSpec]| fields.iter().find(|field| field.key == "browser").cloned();
        let mut found = self
            .shared
            .known
            .read()
            .source
            .as_ref()
            .and_then(|source| field(&source.settings));
        if found.is_none() {
            let services = call(REQUEST_TIMEOUT, self.api().services()).await?;
            found = services
                .iter()
                .find(|service| service.id == SERVICE)
                .and_then(|service| field(&service.fields));
        }
        let options = match found.map(|field| field.kind) {
            Some(
                api::FieldKind::Choice { options, .. } | api::FieldKind::Radio { options, .. },
            ) => options,
            _ => Vec::new(),
        };
        let installed: Vec<Browser> = options
            .iter()
            .filter(|option| option.unavailable.is_none() && option.value != "auto")
            .map(|option| Browser {
                id: option.value.clone(),
                name: text(&option.label),
            })
            .collect();
        Ok(Browsers {
            default: installed.first().map(|browser| browser.id.clone()),
            installed,
        })
    }

    async fn browser_sign_in(&self, browser: Option<String>) -> Result<SessionInfo> {
        let id = self.signing_in().await?;
        let choice = FieldValue::new("browser", browser.as_deref().unwrap_or("auto"));
        let source = call(
            REQUEST_TIMEOUT,
            self.api().set_source_settings(id.clone(), vec![choice]),
        )
        .await?;
        self.learn_source(source);
        let source = call(SIGN_IN_TIMEOUT, self.api().authenticate_source(id)).await?;
        self.signed_in(source).await
    }

    async fn browser_profiles(&self) -> Result<Vec<ProfileBrowser>> {
        let sessions = call(
            SLOW_TIMEOUT,
            self.api().browser_sessions(SERVICE.to_owned()),
        )
        .await?;
        let mut browsers: Vec<ProfileBrowser> = Vec::new();
        for session in sessions {
            let profile = BrowserProfile {
                path: session.id,
                name: session.profile,
                email: session.account,
            };
            match browsers
                .iter_mut()
                .find(|entry| entry.browser.name == session.browser)
            {
                Some(entry) => entry.profiles.push(profile),
                None => browsers.push(ProfileBrowser {
                    browser: Browser {
                        id: session.browser.to_lowercase(),
                        name: session.browser,
                    },
                    profiles: vec![profile],
                }),
            }
        }
        Ok(browsers)
    }

    async fn import_profile(&self, profile: &str) -> Result<SessionInfo> {
        let id = self.signing_in().await?;
        let source = call(
            SLOW_TIMEOUT,
            self.api().import_browser_session(id, profile.to_owned()),
        )
        .await?;
        self.signed_in(source).await
    }

    async fn sign_out(&self) -> Result<()> {
        let id = self.source_id()?;
        call(REQUEST_TIMEOUT, self.api().clear_credentials(id.clone())).await?;
        let source = call(
            REQUEST_TIMEOUT,
            self.api().upsert_source(draft(Some(id), "anonymous")),
        )
        .await?;
        self.learn_source(source);
        Ok(())
    }

    async fn accounts(&self) -> Result<Vec<Account>> {
        let several = self
            .shared
            .known
            .read()
            .source
            .as_ref()
            .is_some_and(|source| source.capabilities.accounts);
        if !several {
            return Ok(Vec::new());
        }
        let accounts = call(REQUEST_TIMEOUT, self.api().accounts(self.source_id()?)).await?;
        let avatar = self
            .shared
            .known
            .read()
            .source
            .as_ref()
            .and_then(|source| source.avatar.clone());
        Ok(accounts
            .iter()
            .map(|a| account(a, avatar.as_ref()))
            .collect())
    }

    async fn switch_account(&self, page_id: Option<String>) -> Result<SessionInfo> {
        let source = call(
            SLOW_TIMEOUT,
            self.api().switch_account(self.source_id()?, page_id),
        )
        .await?;
        let session = self.learn_source(source);
        Ok(self.with_account(session).await)
    }

    async fn scrobbling(&self) -> Result<ScrobbleStatus> {
        self.integrations().await
    }

    async fn connect_lastfm(&self, app: Option<LastFmApp>) -> Result<ScrobbleStatus> {
        if let Some(app) = app {
            let values = vec![
                FieldValue::new("api_key", app.api_key),
                FieldValue::new("api_secret", app.shared_secret),
            ];
            call(
                REQUEST_TIMEOUT,
                self.api().set_integration_settings("lastfm".into(), values),
            )
            .await?;
        }
        call(
            SIGN_IN_TIMEOUT,
            self.api().authenticate_integration("lastfm".into()),
        )
        .await?;
        self.integrations().await
    }

    async fn connect_listenbrainz(&self, token: String) -> Result<ScrobbleStatus> {
        call(
            REQUEST_TIMEOUT,
            self.api().set_integration_settings(
                "listenbrainz".into(),
                vec![FieldValue::new("token", token)],
            ),
        )
        .await?;
        self.integrations().await
    }

    async fn disconnect_scrobbler(&self, service: ScrobbleService) -> Result<ScrobbleStatus> {
        let id = match service {
            ScrobbleService::LastFm => "lastfm",
            ScrobbleService::ListenBrainz => "listenbrainz",
        };
        call(REQUEST_TIMEOUT, self.api().clear_integration(id.into())).await?;
        self.integrations().await
    }

    async fn apply_settings(&self, settings: &Settings) -> Result<()> {
        let view = call(REQUEST_TIMEOUT, self.api().config()).await?;
        let mut config = view.config.clone();
        config.equalizer = settings.equalizer.settings();
        config.crossfade_seconds = settings.crossfade_seconds();
        config.replay_gain.normalize_loudness = settings.normalisation;
        config.stream_quality = match settings.audio_quality {
            AudioQuality::Low => config::StreamQuality::Low,
            AudioQuality::Normal => config::StreamQuality::Normal,
            AudioQuality::High => config::StreamQuality::High,
        };
        config.autoplay_radio = settings.autoplay;
        config.skip_explicit = settings.restrict_explicit;
        config.pause_watch_history = settings.pause_history;
        if config == view.config {
            return Ok(());
        }
        call(REQUEST_TIMEOUT, self.api().set_config(config))
            .await
            .map(drop)
    }

    async fn preview_equalizer(&self, equalizer: Equalizer) -> Result<()> {
        call(
            REQUEST_TIMEOUT,
            self.api().preview_equalizer(equalizer.settings()),
        )
        .await
    }
}

impl KopuzBackend {
    /// A library tab built here rather than browsed: the tabs as chips, then
    /// its items in a grid.
    fn library_page(&self, target: &BrowseTarget, items: Vec<Item>) -> Page {
        Page {
            target: target.clone(),
            header: None,
            chips: self.library_chips(target),
            sections: vec![Section {
                layout: SectionLayout::Grid,
                items,
                ..Section::default()
            }],
            continuation: None,
        }
    }

    /// The library's tabs, as the web app shows them over each one.
    fn library_chips(&self, selected: &BrowseTarget) -> Vec<Chip> {
        let known = self.shared.known.read();
        let pages = known.pages();
        [
            ("Playlists", LibraryTab::Playlists),
            ("Songs", LibraryTab::Songs),
            ("Albums", LibraryTab::Albums),
            ("Artists", LibraryTab::Artists),
            ("Subscriptions", LibraryTab::Subscriptions),
            ("Podcasts", LibraryTab::Podcasts),
            ("Uploads", LibraryTab::Uploads),
        ]
        .into_iter()
        .filter(|(_, tab)| {
            *tab == LibraryTab::Playlists
                || convert::page_id(pages, &BrowseTarget::Library(*tab)).is_some()
        })
        .map(|(title, tab)| Chip {
            title: title.into(),
            id: title.to_lowercase(),
            selected: *selected == BrowseTarget::Library(tab),
        })
        .collect()
    }
}
