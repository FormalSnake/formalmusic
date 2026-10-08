//! kopuzd over `kopuz-client`: gRPC on a unix socket, or a named pipe on
//! Windows. The connection comes back with backoff when the daemon goes
//! away, and a missing daemon is started once, since a dev box may have no
//! service for it.
//!
//! FormalMusic is a YouTube Music client, so it names that one service when
//! it sets up its source. Everything past that reads what the source says it
//! can do, never which service it is.

use std::collections::HashSet;
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
    /// The rest of a long list queued behind what started playing.
    filling: Mutex<Option<AbortHandle>>,
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
                filling: Mutex::new(None),
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
        let (id, several) = {
            let known = self.shared.known.read();
            match &known.source {
                Some(source) => (source.id.clone(), source.capabilities.accounts),
                None => return session,
            }
        };
        if session.signed_in && several {
            match call(REQUEST_TIMEOUT, self.api().accounts(id)).await {
                Ok(accounts) => {
                    session.account = accounts.iter().find(|a| a.active).map(account);
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

    /// Plays a page's tracks: those given at once, then the rest of the
    /// list, page by page, appended behind them while they play.
    async fn play_page(
        &self,
        target: BrowseTarget,
        given: Vec<Track>,
        start: usize,
        shuffle: bool,
    ) -> Result<()> {
        let request = self.request(&target).await?;
        let mut queued: Vec<String> = given.iter().map(|track| track.key.clone()).collect();
        let first = if queued.is_empty() {
            let detail = self.detail(request.clone()).await?;
            queued = list_keys(&detail);
            if queued.is_empty() {
                return Err(ClientError::NotFound("nothing to play there".into()));
            }
            Some(detail)
        } else {
            None
        };
        self.queue(
            QueueMode::Replace,
            QueueContext::Tracks {
                keys: queued.clone(),
            },
            Some(start),
            Some(shuffle),
        )
        .await?;
        let backend = KopuzBackend {
            shared: self.shared.clone(),
        };
        let task = tokio::spawn(async move {
            let detail = match first {
                Some(detail) => detail,
                None => match backend.detail(request.clone()).await {
                    Ok(detail) => detail,
                    Err(error) => return tracing::warn!("queue the rest: {error}"),
                },
            };
            let mut keys = list_keys(&detail);
            let mut continuation = detail.continuation;
            let mut skip = queued.len().min(keys.len());
            if keys[..skip] != queued[..skip] {
                skip = 0;
            }
            loop {
                let rest: Vec<String> = keys.drain(skip..).collect();
                if !rest.is_empty()
                    && let Err(error) = backend
                        .queue(
                            QueueMode::Append,
                            QueueContext::Tracks { keys: rest },
                            None,
                            None,
                        )
                        .await
                {
                    return tracing::warn!("queue the rest: {error}");
                }
                skip = 0;
                let Some(token) = continuation.take() else {
                    return;
                };
                let more = api::CatalogDetailRequest {
                    continuation: Some(token),
                    ..request.clone()
                };
                match backend.detail(more).await {
                    Ok(detail) => {
                        keys = list_keys(&detail);
                        continuation = detail.continuation;
                    }
                    Err(error) => return tracing::warn!("queue the rest: {error}"),
                }
            }
        });
        if let Some(previous) = self.shared.filling.lock().replace(task.abort_handle()) {
            previous.abort();
        }
        Ok(())
    }
}

/// The keys of a page's own list: its tracks, or the songs of its list shelves.
fn list_keys(detail: &CatalogDetail) -> Vec<String> {
    if !detail.tracks.is_empty() {
        return detail
            .tracks
            .iter()
            .map(|track| track.key.clone())
            .collect();
    }
    detail
        .shelves
        .iter()
        .filter(|shelf| shelf.layout == api::ShelfLayout::List)
        .flat_map(|shelf| &shelf.items)
        .filter_map(|item| item.track.as_ref().map(|track| track.key.clone()))
        .collect()
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

fn account(account: &api::SourceAccount) -> Account {
    Account {
        name: account.name.clone(),
        handle: account.handle.clone(),
        art: None,
        page_id: account.id.clone(),
        selected: account.active,
    }
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
        if let Some(task) = self.shared.filling.lock().take() {
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
            _ => return Ok(None),
        };
        url.map_err(convert::error)
    }

    async fn play(&self, source: PlaySource, start_index: usize, shuffle: bool) -> Result<()> {
        if let Some(previous) = self.shared.filling.lock().take() {
            previous.abort();
        }
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
            PlaySource::Page { target, tracks } => {
                self.play_page(target, tracks, start_index, shuffle).await
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
            Control::Clear => {
                if let Some(previous) = self.shared.filling.lock().take() {
                    previous.abort();
                }
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

    fn playlists_reorder(&self) -> bool {
        self.shared
            .known
            .read()
            .source
            .as_ref()
            .is_some_and(|source| source.capabilities.playlists == api::PlaylistCapability::Reorder)
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
        let known = self.shared.known.read();
        let field = known
            .source
            .as_ref()
            .and_then(|source| source.settings.iter().find(|field| field.key == "browser"));
        let options = match field.map(|field| &field.kind) {
            Some(
                api::FieldKind::Choice { options, .. } | api::FieldKind::Radio { options, .. },
            ) => options.as_slice(),
            _ => &[],
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
        Ok(accounts.iter().map(account).collect())
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
