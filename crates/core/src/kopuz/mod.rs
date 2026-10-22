//! The Kopuz daemon (`kopuzd`, or the Kopuz app serving the same socket) over its gRPC API.
//! The store speaks [`Command`]s; each one becomes the Kopuz calls that answer it, and
//! Kopuz's event stream becomes the store's [`Event`]s. The daemon's one active source has
//! to be YouTube Music; [`KopuzTransport`] selects it when another is active.

mod convert;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use kopuz_client::GrpcApi;
use formalmusic_api::{
    Account, ApiError, Browser, BrowserProfile, Browsers, Command, Continuation,
    ContinuationPage, EnqueuePosition, Event, Item, LibraryScope, ListenBrainzSource, Page,
    PlaySource, PlaylistEdit, ProfileBrowser, RateTarget, Rating, Reply, Repeat, ScrobbleAccount,
    ScrobbleService, ScrobbleStatus, SearchResults, Section, SectionLayout, SessionInfo,
    Suggestion, Track,
};
use futures_util::StreamExt;
use kopuz_api::{
    self as k, ConfigApi, EventApi, LibraryApi, PlayerApi, PlaylistApi, SourceApi,
};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use crate::transport::{ClientError, ConnectionStatus, Transport, TransportEvent, TransportKind};

pub use convert::artwork_request;

const YTMUSIC: &str = "ytmusic";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Pages, search and starting playback go through InnerTube and stream resolution, which
/// take seconds on a bad network.
const SLOW_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
/// A browser sign-in lasts as long as the user takes.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(320);
const RETRY_MIN: Duration = Duration::from_millis(250);
const RETRY_MAX: Duration = Duration::from_secs(10);
/// A daemon that dies at once would otherwise be restarted on every retry.
const SPAWN_GAP: Duration = Duration::from_secs(30);
const OFFLINE_AFTER: u32 = 4;
/// Row actions remembered for later commands; past this many the oldest page's go.
const ACTIONS_KEPT: usize = 20_000;

/// The daemon's socket, or `FORMALMUSIC_KOPUZ_SOCKET`.
pub fn socket_path() -> PathBuf {
    match std::env::var_os("FORMALMUSIC_KOPUZ_SOCKET") {
        Some(path) => path.into(),
        None => kopuz_client::default_socket_path().expect("a home directory"),
    }
}

/// A continuation token as the store holds it: what to ask Kopuz again, and whether the
/// answer extends one section's rows or the page's sections.
#[derive(Serialize, Deserialize)]
#[serde(tag = "c", rename_all = "snake_case")]
enum Cursor {
    Detail {
        kind: String,
        id: String,
        token: String,
        rows: bool,
        /// Rows before this page, for the positions of an owned playlist's rows.
        offset: usize,
    },
    Search {
        query: String,
        filter: Option<String>,
        token: String,
    },
}

impl Cursor {
    fn wrap(&self) -> Continuation {
        Continuation(serde_json::to_string(self).expect("cursors serialise"))
    }

    fn read(token: &Continuation) -> Result<Self, ClientError> {
        serde_json::from_str(&token.0)
            .map_err(|_| ClientError::Api(ApiError::BadRequest("unknown continuation".into())))
    }
}

fn kind_name(kind: k::CatalogItemKind) -> &'static str {
    use k::CatalogItemKind as K;
    match kind {
        K::Track => "track",
        K::Album => "album",
        K::Playlist => "playlist",
        K::Artist => "artist",
        K::Mood => "mood",
        K::Podcast => "podcast",
        K::Episode => "episode",
        K::Video => "video",
        K::Page => "page",
        K::Unknown => "unknown",
    }
}

fn kind_of(name: &str) -> k::CatalogItemKind {
    use k::CatalogItemKind as K;
    match name {
        "track" => K::Track,
        "album" => K::Album,
        "playlist" => K::Playlist,
        "artist" => K::Artist,
        "mood" => K::Mood,
        "podcast" => K::Podcast,
        "episode" => K::Episode,
        "video" => K::Video,
        "page" => K::Page,
        _ => K::Unknown,
    }
}

/// What the player last said, for the position and buffer events that only carry a delta.
#[derive(Default)]
struct Clock {
    /// Local instant at which the daemon's clock read `daemon_ms`.
    synced: Option<(Instant, u64)>,
    token: Option<u64>,
    duration_ms: Option<u64>,
    position_ms: u64,
    buffered_ms: u64,
}

impl Clock {
    fn daemon_now(&self) -> u64 {
        match self.synced {
            Some((at, daemon_ms)) => daemon_ms + at.elapsed().as_millis() as u64,
            None => 0,
        }
    }
}

struct Shared {
    api: Arc<GrpcApi>,
    socket: PathBuf,
    source: Mutex<Option<String>>,
    liked: RwLock<HashSet<String>>,
    actions: Mutex<(HashMap<String, k::CatalogActions>, Vec<String>)>,
    clock: Mutex<Clock>,
    /// The queue's revision and current index as the last player state had them.
    queue_at: Mutex<Option<(u64, Option<u32>)>>,
    covers: Option<formalmusic_extras::AnimatedCovers>,
    stopped: AtomicBool,
    task: Mutex<Option<AbortHandle>>,
    last_spawn: Mutex<Option<Instant>>,
}

pub struct KopuzTransport {
    shared: Arc<Shared>,
}

impl KopuzTransport {
    pub fn new(api: Arc<GrpcApi>, socket: PathBuf) -> Self {
        let covers = formalmusic_extras::AnimatedCovers::new()
            .inspect_err(|error| tracing::warn!("animated covers off: {error}"))
            .ok();
        Self {
            shared: Arc::new(Shared {
                api,
                socket,
                source: Mutex::new(None),
                liked: RwLock::default(),
                actions: Mutex::default(),
                clock: Mutex::default(),
                queue_at: Mutex::default(),
                covers,
                stopped: AtomicBool::new(false),
                task: Mutex::new(None),
                last_spawn: Mutex::new(None),
            }),
        }
    }
}

fn wire(error: k::ApiError) -> ClientError {
    use k::ErrorCode as C;
    let message = error.message;
    match error.code {
        C::DaemonGone => ClientError::Disconnected("The Kopuz daemon is not running.".into()),
        C::SourceAuthExpired => ApiError::SignedOut.into(),
        C::NotFound | C::ItemUnavailable => ApiError::NotFound(message).into(),
        C::InvalidInput | C::Unsupported | C::Conflict => ApiError::BadRequest(message).into(),
        C::SourceUnreachable | C::SourceChallenged | C::SourceRefused => {
            ApiError::Network(message).into()
        }
        C::Internal => ApiError::Parse(message).into(),
    }
}

fn unsupported(what: &str) -> ClientError {
    ApiError::BadRequest(format!("Kopuz has no way to {what} yet.")).into()
}

fn timeout_for(command: &Command) -> Duration {
    match command {
        Command::Browse { .. }
        | Command::Continue { .. }
        | Command::Search { .. }
        | Command::Related { .. }
        | Command::Play { .. }
        | Command::SignIn { .. }
        | Command::ImportCookies { .. }
        | Command::ConnectListenBrainz { .. }
        | Command::Lyrics { .. }
        | Command::AnimatedCover { .. }
        | Command::EditPlaylist { .. } => SLOW_REQUEST_TIMEOUT,
        Command::BrowserSignIn { .. } | Command::ConnectLastFm { .. } => SIGN_IN_TIMEOUT,
        _ => REQUEST_TIMEOUT,
    }
}

#[async_trait]
impl Transport for KopuzTransport {
    fn kind(&self) -> TransportKind {
        TransportKind::Daemon
    }

    fn start(&self, events: mpsc::UnboundedSender<TransportEvent>) {
        let task = tokio::spawn(supervise(self.shared.clone(), events));
        *self.shared.task.lock() = Some(task.abort_handle());
    }

    async fn call(&self, command: Command) -> Result<Reply, ClientError> {
        let timeout = timeout_for(&command);
        match tokio::time::timeout(timeout, self.shared.call(command)).await {
            Ok(reply) => reply,
            Err(_) => Err(ClientError::Timeout),
        }
    }

    fn stop(&self) {
        self.shared.stopped.store(true, Ordering::SeqCst);
        if let Some(task) = self.shared.task.lock().take() {
            task.abort();
        }
    }
}

impl Shared {
    fn api(&self) -> &GrpcApi {
        &self.api
    }

    fn remember(&self, id: &str, actions: &k::CatalogActions) {
        if *actions == k::CatalogActions::default() {
            return;
        }
        let mut guard = self.actions.lock();
        let (map, order) = &mut *guard;
        if map.insert(id.to_owned(), actions.clone()).is_none() {
            order.push(id.to_owned());
        }
        if order.len() > ACTIONS_KEPT {
            for old in order.drain(..ACTIONS_KEPT / 2) {
                map.remove(&old);
            }
        }
    }

    fn actions_of(&self, id: &str) -> Option<k::CatalogActions> {
        self.actions.lock().0.get(id).cloned()
    }

    /// Runs `f` with a conversion context over this transport's state.
    fn convert<T>(&self, f: impl FnOnce(&mut convert::Ctx) -> T) -> T {
        let liked = self.liked.read().clone();
        let mut remember = |id: &str, actions: &k::CatalogActions| self.remember(id, actions);
        f(&mut convert::Ctx {
            liked: &liked,
            actions: &mut remember,
        })
    }

    /// The YouTube Music source, made the active one if it is not.
    async fn source(&self) -> Result<k::SourceInfo, ClientError> {
        let sources = self.api().sources().await.map_err(wire)?;
        let ytmusic = |s: &&k::SourceInfo| s.service.id == YTMUSIC;
        let source = match sources.iter().filter(ytmusic).find(|s| s.active) {
            Some(active) => active.clone(),
            None => {
                let id = match sources.iter().find(ytmusic) {
                    Some(found) => found.id.clone(),
                    None => {
                        // Signed out still browses and plays, as music.youtube.com does.
                        let draft = k::SourceDraft {
                            name: "YouTube Music".into(),
                            service: YTMUSIC.into(),
                            values: vec![k::FieldValue {
                                key: "auth_method".into(),
                                value: "anonymous".into(),
                            }],
                            ..Default::default()
                        };
                        self.api().upsert_source(draft).await.map_err(wire)?.id
                    }
                };
                tracing::info!(source = id, "selecting the YouTube Music source");
                self.api().switch_source(id).await.map_err(wire)?
            }
        };
        *self.source.lock() = Some(source.id.clone());
        Ok(source)
    }

    async fn source_id(&self) -> Result<String, ClientError> {
        let known = self.source.lock().clone();
        match known {
            Some(id) => Ok(id),
            None => Ok(self.source().await?.id),
        }
    }

    async fn session(&self) -> Result<SessionInfo, ClientError> {
        let source = match self.source().await {
            Ok(source) => source,
            Err(ClientError::Api(ApiError::SignedOut)) => return Ok(SessionInfo::default()),
            Err(error) => return Err(error),
        };
        let signed_in = source.authenticated && !source.anonymous;
        let account = match signed_in && source.capabilities.accounts {
            true => self
                .api()
                .accounts(source.id.clone())
                .await
                .map_err(wire)?
                .into_iter()
                .find(|account| account.active)
                .map(|account| account_of(account, &source)),
            false => None,
        };
        Ok(SessionInfo {
            signed_in,
            account,
            premium: false,
        })
    }

    async fn refresh_liked(&self) {
        match self.api().favorites().await {
            Ok(view) => *self.liked.write() = view.refs.into_iter().collect(),
            Err(error) => tracing::debug!("favorites: {error}"),
        }
    }

    async fn player_snapshot(&self) -> Result<formalmusic_api::PlayerState, ClientError> {
        let state = self.api().player_state().await.map_err(wire)?;
        Ok(self.on_player_state(&state))
    }

    fn on_player_state(&self, state: &k::PlayerState) -> formalmusic_api::PlayerState {
        let player = convert::player(state, state.now_ms, &self.liked.read());
        let mut clock = self.clock.lock();
        clock.synced = Some((Instant::now(), state.now_ms));
        clock.token = match state.intent {
            k::Intent::Loading { token, .. } | k::Intent::Committed { token } => Some(token),
            k::Intent::Stopped => None,
        };
        clock.duration_ms = player.duration_ms;
        clock.position_ms = player.position_ms;
        clock.buffered_ms = buffered_ms(&state.buffered, player.duration_ms);
        player
    }

    async fn queue_snapshot(&self) -> Result<formalmusic_api::QueueState, ClientError> {
        let snapshot = self.api().queue_snapshot().await.map_err(wire)?;
        Ok(convert::queue(&snapshot, &self.liked.read()))
    }

    async fn detail(
        &self,
        request: k::CatalogDetailRequest,
    ) -> Result<k::CatalogDetail, ClientError> {
        self.api()
            .catalog_detail(request)
            .await
            .map_err(wire)
    }

    async fn page(&self, target: formalmusic_api::BrowseTarget) -> Result<Page, ClientError> {
        let request = convert::request(&target);
        let detail = self.detail(request.clone()).await?;
        Ok(self.convert(|ctx| {
            let mut page = convert::page(
                target,
                convert::header(&detail, ctx),
                convert::chips(&detail),
            );
            let more = |token: &String, rows: bool, offset: usize| {
                Cursor::Detail {
                    kind: kind_name(request.kind).into(),
                    id: request.id.clone(),
                    token: token.clone(),
                    rows,
                    offset,
                }
                .wrap()
            };
            if !detail.tracks.is_empty() {
                page.sections.push(Section {
                    layout: SectionLayout::List,
                    items: convert::track_rows(&detail, 0, ctx),
                    continuation: detail
                        .continuation
                        .as_ref()
                        .map(|token| more(token, true, detail.tracks.len())),
                    ..Default::default()
                });
            } else {
                page.continuation = detail
                    .continuation
                    .as_ref()
                    .map(|token| more(token, false, 0));
            }
            for shelf in &detail.shelves {
                let continuation = shelf.continuation.as_ref().map(|token| more(token, true, 0));
                page.sections.push(convert::section(shelf, continuation, ctx));
            }
            page
        }))
    }

    async fn more(&self, cursor: Cursor) -> Result<ContinuationPage, ClientError> {
        match cursor {
            Cursor::Detail {
                kind,
                id,
                token,
                rows,
                offset,
            } => {
                let detail = self
                    .detail(k::CatalogDetailRequest {
                        kind: kind_of(&kind),
                        id: id.clone(),
                        continuation: Some(token),
                    })
                    .await?;
                Ok(self.convert(|ctx| {
                    let next = |offset: usize| {
                        detail.continuation.as_ref().map(|token| {
                            Cursor::Detail {
                                kind: kind.clone(),
                                id: id.clone(),
                                token: token.clone(),
                                rows,
                                offset,
                            }
                            .wrap()
                        })
                    };
                    if rows {
                        let mut items = convert::track_rows(&detail, offset, ctx);
                        items.extend(convert::items(&detail.shelves, ctx));
                        ContinuationPage {
                            sections: Vec::new(),
                            items,
                            continuation: next(offset + detail.tracks.len()),
                        }
                    } else {
                        let mut sections = Vec::new();
                        if !detail.tracks.is_empty() {
                            sections.push(Section {
                                layout: SectionLayout::List,
                                items: convert::track_rows(&detail, offset, ctx),
                                ..Default::default()
                            });
                        }
                        sections.extend(
                            detail
                                .shelves
                                .iter()
                                .map(|shelf| convert::section(shelf, None, ctx)),
                        );
                        ContinuationPage {
                            sections,
                            items: Vec::new(),
                            continuation: next(0),
                        }
                    }
                }))
            }
            Cursor::Search {
                query,
                filter,
                token,
            } => {
                let results = self
                    .api()
                    .search(k::SearchRequest {
                        query: query.clone(),
                        filter: filter.clone(),
                        continuation: Some(token),
                    })
                    .await
                    .map_err(wire)?;
                Ok(self.convert(|ctx| {
                    let mut items: Vec<Item> = results
                        .tracks
                        .iter()
                        .map(|row| Item::Track(convert::track(row, ctx.liked)))
                        .collect();
                    items.extend(convert::items(&results.shelves, ctx));
                    ContinuationPage {
                        sections: Vec::new(),
                        items,
                        continuation: results.continuation.map(|token| {
                            Cursor::Search {
                                query,
                                filter,
                                token,
                            }
                            .wrap()
                        }),
                    }
                }))
            }
        }
    }

    async fn search(
        &self,
        query: String,
        filter: Option<formalmusic_api::SearchFilter>,
    ) -> Result<SearchResults, ClientError> {
        let filter_id = filter.map(convert::search_filter_id);
        let results = self
            .api()
            .search(k::SearchRequest {
                query: query.clone(),
                filter: filter_id.clone(),
                continuation: None,
            })
            .await
            .map_err(wire)?;
        let sections = self.convert(|ctx| {
            let mut sections: Vec<Section> = results
                .shelves
                .iter()
                .map(|shelf| convert::section(shelf, None, ctx))
                .collect();
            if sections.is_empty() && !results.tracks.is_empty() {
                sections.push(Section {
                    layout: SectionLayout::List,
                    items: results
                        .tracks
                        .iter()
                        .map(|row| Item::Track(convert::track(row, ctx.liked)))
                        .collect(),
                    ..Default::default()
                });
            }
            sections
        });
        Ok(SearchResults {
            continuation: results.continuation.map(|token| {
                Cursor::Search {
                    query: query.clone(),
                    filter: filter_id,
                    token,
                }
                .wrap()
            }),
            query,
            filter,
            correction: results.correction,
            sections,
        })
    }

    /// Every track key of an album or playlist, walking its pages.
    async fn all_keys(&self, id: &str) -> Result<Vec<String>, ClientError> {
        let kind = match id.starts_with("MPRE") {
            true => k::CatalogItemKind::Album,
            false => k::CatalogItemKind::Playlist,
        };
        let mut keys = Vec::new();
        let mut continuation = None;
        loop {
            let detail = self
                .detail(k::CatalogDetailRequest {
                    kind,
                    id: id.to_owned(),
                    continuation,
                })
                .await?;
            let before = keys.len();
            keys.extend(detail.tracks.into_iter().map(|row| row.key));
            match detail.continuation {
                Some(next) if keys.len() > before => continuation = Some(next),
                _ => return Ok(keys),
            }
        }
    }

    async fn edit_playlist(&self, id: String, edits: Vec<PlaylistEdit>) -> Result<(), ClientError> {
        let position = |row: &str| {
            row.parse::<u32>()
                .map_err(|_| ClientError::Api(ApiError::BadRequest(format!("no such row: {row}"))))
        };
        let mut removes = Vec::new();
        let mut details = k::PlaylistEdit::default();
        for edit in edits {
            match edit {
                PlaylistEdit::Add { video_id } => self
                    .api()
                    .add_playlist_tracks(id.clone(), vec![video_id])
                    .await
                    .map_err(wire)?,
                PlaylistEdit::AddPlaylist { playlist_id } => {
                    let keys = self.all_keys(&playlist_id).await?;
                    self.api()
                        .add_playlist_tracks(id.clone(), keys)
                        .await
                        .map_err(wire)?
                }
                PlaylistEdit::Remove { set_video_id, .. } => removes.push(position(&set_video_id)?),
                PlaylistEdit::Move {
                    set_video_id,
                    before_set_video_id,
                } => {
                    let from = position(&set_video_id)?;
                    let to = match before_set_video_id {
                        Some(before) => {
                            let before = position(&before)?;
                            if before > from { before - 1 } else { before }
                        }
                        None => (self.all_keys(&id).await?.len() as u32).saturating_sub(1),
                    };
                    self.api()
                        .reorder_playlist(id.clone(), k::PlaylistReorder { from, to })
                        .await
                        .map_err(wire)?
                }
                PlaylistEdit::Rename { title } => details.name = Some(title),
                PlaylistEdit::Describe { description } => details.description = Some(description),
                PlaylistEdit::SetPrivacy { privacy } => {
                    details.privacy = Some(convert::wire_privacy(privacy))
                }
            }
        }
        // Later rows first, so each position still names the row it did on the page.
        removes.sort_unstable_by(|a, b| b.cmp(a));
        for index in removes {
            self.api()
                .remove_playlist_track(id.clone(), index)
                .await
                .map_err(wire)?;
        }
        if details != k::PlaylistEdit::default() {
            self.api()
                .edit_playlist(id, details)
                .await
                .map_err(wire)?;
        }
        Ok(())
    }

    async fn scrobbling(&self) -> Result<ScrobbleStatus, ClientError> {
        let integrations = self.api().integrations().await.map_err(wire)?;
        let account = |id: &str| {
            let configured = integrations
                .iter()
                .find(|info| info.id == id)
                .is_some_and(|info| info.configured);
            ScrobbleAccount {
                username: configured.then(|| "Connected".to_owned()),
                ..Default::default()
            }
        };
        Ok(ScrobbleStatus {
            lastfm_app: true,
            lastfm: account("lastfm"),
            listenbrainz: account("listenbrainz"),
            queued: 0,
        })
    }

    async fn browser_profiles(&self) -> Result<Vec<ProfileBrowser>, ClientError> {
        let sessions = self
            .api()
            .browser_sessions(YTMUSIC.into())
            .await
            .map_err(wire)?;
        let mut browsers: Vec<ProfileBrowser> = Vec::new();
        for session in sessions {
            let profile = BrowserProfile {
                path: session.id,
                name: session.profile,
                email: session.account,
            };
            match browsers.iter_mut().find(|b| b.browser.id == session.browser) {
                Some(browser) => browser.profiles.push(profile),
                None => browsers.push(ProfileBrowser {
                    browser: Browser {
                        id: session.browser.clone(),
                        name: session.browser,
                    },
                    profiles: vec![profile],
                }),
            }
        }
        Ok(browsers)
    }

    /// Pushes the playback settings in `config.toml` into Kopuz's own, editing only the keys
    /// that differ, so a key pinned in Kopuz's managed settings is not fought over.
    async fn apply_settings(&self) -> Result<(), ClientError> {
        let ours = match formalmusic_api::settings::load() {
            Ok(handle) => handle.get(),
            Err(error) => {
                tracing::warn!("settings: {error}");
                return Ok(());
            }
        };
        let view = self.api().config().await.map_err(wire)?;
        let edits = settings_edits(&ours, &view.settings.playback);
        if edits.is_empty() {
            return Ok(());
        }
        let report = self.api().edit_config(edits).await.map_err(wire)?;
        for outcome in report.outcomes {
            if outcome != k::EditOutcome::Applied {
                tracing::info!("kopuz kept a setting: {outcome:?}");
            }
        }
        Ok(())
    }

    async fn call(&self, command: Command) -> Result<Reply, ClientError> {
        let api = self.api();
        let ack = |result: Result<k::CommandAck, k::ApiError>| result.map(|_| Reply::Ok).map_err(wire);
        let unit = |result: Result<(), k::ApiError>| result.map(|_| Reply::Ok).map_err(wire);
        Ok(match command {
            Command::Session => Reply::Session(self.session().await?),
            Command::SignIn { cookies } => {
                let source = self.source().await?;
                let id = source.id.clone();
                // Kopuz keeps a source made anonymous anonymous through pasted credentials (only
                // its browser sign-ins clear the flag), so the session would read signed out.
                if source.anonymous {
                    api.upsert_source(k::SourceDraft {
                        id: Some(id.clone()),
                        name: source.name.clone(),
                        service: YTMUSIC.into(),
                        values: vec![k::FieldValue {
                            key: "auth_method".into(),
                            value: "browser".into(),
                        }],
                        ..Default::default()
                    })
                    .await
                    .map_err(wire)?;
                }
                api.provision_credentials(k::CredentialProvision {
                    server_id: id,
                    secret: cookies,
                    user_id: None,
                    browser: None,
                })
                .await
                .map_err(wire)?;
                Reply::Session(self.session().await?)
            }
            Command::Browsers => Reply::Browsers(Browsers {
                installed: vec![Browser {
                    id: "default".into(),
                    name: "Your browser".into(),
                }],
                default: Some("default".into()),
            }),
            Command::BrowserSignIn { .. } => {
                let id = self.source_id().await?;
                api.authenticate_source(id).await.map_err(wire)?;
                Reply::Session(self.session().await?)
            }
            Command::CancelSignIn => return Err(unsupported("cancel a sign-in")),
            Command::BrowserProfiles => Reply::BrowserProfiles(self.browser_profiles().await?),
            Command::ImportCookies { profile, .. } => {
                let id = self.source_id().await?;
                api.import_browser_session(id, profile)
                    .await
                    .map_err(wire)?;
                Reply::Session(self.session().await?)
            }
            Command::SignOut => {
                api.clear_credentials(self.source_id().await?)
                    .await
                    .map_err(wire)?;
                Reply::Ok
            }
            Command::Accounts => {
                let source = self.source().await?;
                let accounts = api.accounts(source.id.clone()).await.map_err(wire)?;
                Reply::Accounts(
                    accounts
                        .into_iter()
                        .map(|account| account_of(account, &source))
                        .collect(),
                )
            }
            Command::SwitchAccount { page_id } => {
                api.switch_account(self.source_id().await?, page_id)
                    .await
                    .map_err(wire)?;
                Reply::Session(self.session().await?)
            }

            Command::Scrobbling => Reply::Scrobbling(self.scrobbling().await?),
            Command::ConnectLastFm { app } => {
                if let Some(app) = app {
                    api.set_integration_settings(
                        "lastfm".into(),
                        vec![
                            k::FieldValue {
                                key: "api_key".into(),
                                value: app.api_key,
                            },
                            k::FieldValue {
                                key: "api_secret".into(),
                                value: app.shared_secret,
                            },
                        ],
                    )
                    .await
                    .map_err(wire)?;
                }
                api.authenticate_integration("lastfm".into())
                    .await
                    .map_err(wire)?;
                Reply::Scrobbling(self.scrobbling().await?)
            }
            Command::ConnectListenBrainz { source } => {
                match source {
                    ListenBrainzSource::Token { token } => {
                        api.set_integration_settings(
                            "listenbrainz".into(),
                            vec![k::FieldValue {
                                key: "token".into(),
                                value: token,
                            }],
                        )
                        .await
                        .map_err(wire)?;
                    }
                    ListenBrainzSource::Profile { profile, .. } => {
                        // The profiles listed are YouTube Music sessions, `<browser>:<dir>`;
                        // an integration takes the profile directory alone.
                        let dir = profile.split_once(':').map_or(profile.as_str(), |(_, dir)| dir);
                        api.connect_integration_from_browser("listenbrainz".into(), dir.into())
                            .await
                            .map_err(wire)?;
                    }
                }
                Reply::Scrobbling(self.scrobbling().await?)
            }
            Command::DisconnectScrobbler { service } => {
                let id = match service {
                    ScrobbleService::LastFm => "lastfm",
                    ScrobbleService::ListenBrainz => "listenbrainz",
                };
                api.clear_integration(id.into()).await.map_err(wire)?;
                Reply::Scrobbling(self.scrobbling().await?)
            }
            Command::SetScrobbling { .. } => {
                return Err(unsupported("turn scrobbling or now playing off per service"));
            }

            Command::Browse { target } => Reply::Page(self.page(target).await?),
            Command::Continue { token } => Reply::Continuation(self.more(Cursor::read(&token)?).await?),
            Command::Search { query, filter } => Reply::Search(self.search(query, filter).await?),
            Command::Suggestions { query } => {
                let suggestions = api.search_suggestions(query).await.map_err(wire)?;
                Reply::Suggestions(self.convert(|ctx| {
                    suggestions
                        .into_iter()
                        .map(|suggestion| {
                            let item = suggestion.item.as_ref().and_then(|tile| {
                                let shelf = k::CatalogShelf {
                                    items: vec![tile.clone()],
                                    ..Default::default()
                                };
                                convert::items(std::slice::from_ref(&shelf), ctx).pop()
                            });
                            match item {
                                Some(item) => Suggestion::Item(Box::new(item)),
                                None => Suggestion::Query {
                                    text: suggestion.text,
                                    from_history: suggestion.from_history,
                                },
                            }
                        })
                        .collect()
                }))
            }
            Command::Lyrics { video_id } => match api.lyrics(video_id).await {
                Ok(view) => Reply::Lyrics(convert::lyrics(view)),
                Err(error) if error.code == k::ErrorCode::NotFound => Reply::Lyrics(None),
                Err(error) => return Err(wire(error)),
            },
            Command::AnimatedCover { artist, album } => {
                let Some(covers) = &self.covers else {
                    return Ok(Reply::AnimatedCover(None));
                };
                let path = covers
                    .animated_cover(&artist, &album)
                    .await
                    .map_err(|error| ClientError::Api(ApiError::Network(error.to_string())))?;
                Reply::AnimatedCover(path.map(|path| path.to_string_lossy().into_owned()))
            }
            Command::VideoStream { .. } => return Err(unsupported("hand out a music video stream")),
            Command::Related { browse_id } => {
                let target = formalmusic_api::BrowseTarget::Raw {
                    browse_id: browse_id.clone(),
                    params: None,
                };
                let detail = self
                    .detail(k::CatalogDetailRequest::new(k::CatalogItemKind::Track, &browse_id))
                    .await?;
                Reply::Page(self.convert(|ctx| {
                    let mut page = convert::page(target, None, Vec::new());
                    page.sections = detail
                        .shelves
                        .iter()
                        .map(|shelf| convert::section(shelf, None, ctx))
                        .collect();
                    page
                }))
            }

            Command::Rate { target, rating } => {
                let (id, fallback) = match &target {
                    RateTarget::Track { video_id } => (video_id, Some(video_id.clone())),
                    RateTarget::Playlist { playlist_id } => (playlist_id, None),
                };
                let item_ref = self
                    .actions_of(id)
                    .and_then(|actions| actions.rate_ref)
                    .or(fallback)
                    .ok_or_else(|| unsupported("rate this"))?;
                let rating = match rating {
                    Rating::Like => k::Rating::Like,
                    Rating::Dislike => k::Rating::Dislike,
                    Rating::Indifferent => k::Rating::None,
                };
                unit(api.rate(item_ref, rating).await)?
            }
            Command::SetSubscribed {
                channel_id,
                subscribed,
            } => unit(api.follow(channel_id, subscribed).await)?,
            Command::CreatePlaylist {
                title,
                description,
                privacy,
                video_ids,
            } => {
                let id = api
                    .create_playlist(title.clone(), video_ids)
                    .await
                    .map_err(wire)?;
                if !description.is_empty() || privacy != formalmusic_api::Privacy::Private {
                    api.edit_playlist(
                        id.clone(),
                        k::PlaylistEdit {
                            name: Some(title),
                            description: Some(description),
                            privacy: Some(convert::wire_privacy(privacy)),
                        },
                    )
                    .await
                    .map_err(wire)?;
                }
                Reply::PlaylistCreated { playlist_id: id }
            }
            Command::EditPlaylist { playlist_id, edits } => {
                self.edit_playlist(playlist_id, edits).await?;
                Reply::Ok
            }
            Command::DeletePlaylist { playlist_id } => unit(api.delete_playlist(playlist_id).await)?,
            Command::SetInLibrary { playlist_id, saved } => {
                let save_ref = self
                    .actions_of(&playlist_id)
                    .and_then(|actions| actions.save_ref)
                    .ok_or_else(|| unsupported("save this to the library"))?;
                unit(api.save(save_ref, saved).await)?
            }
            Command::RemoveFromHistory { feedback_token } => {
                unit(api.remove_from_history(feedback_token).await)?
            }
            Command::SetSongInLibrary { feedback_token } => {
                let (save_ref, saved) = convert::read_library_token(&feedback_token)
                    .ok_or_else(|| ClientError::Api(ApiError::BadRequest(feedback_token.clone())))?;
                unit(api.save(save_ref.to_owned(), saved).await)?
            }

            Command::Play {
                source,
                start_index,
                shuffle,
                radio,
            } => {
                let (context, start_on) = play_context(source, start_index, shuffle, radio);
                let reply = ack(api
                    .set_queue(k::SetQueueRequest {
                        mode: k::QueueMode::Replace,
                        context,
                        start_index: Some(start_index as u32),
                        shuffle: Some(shuffle),
                    })
                    .await)?;
                if let Some(key) = start_on {
                    let snapshot = api.queue_snapshot().await.map_err(wire)?;
                    if let Some(index) = start_correction(&snapshot, &key) {
                        ack(api.queue_edit(k::QueueEdit::Jump { index }).await)?;
                    }
                }
                reply
            }
            Command::Enqueue { tracks, position } => ack(api
                .set_queue(k::SetQueueRequest {
                    mode: match position {
                        EnqueuePosition::Next => k::QueueMode::PlayNext,
                        EnqueuePosition::End => k::QueueMode::Append,
                    },
                    context: k::QueueContext::Tracks {
                        keys: tracks.into_iter().map(|track| track.video_id).collect(),
                    },
                    start_index: None,
                    shuffle: None,
                })
                .await)?,
            Command::RemoveFromQueue { index } => {
                ack(api.queue_edit(k::QueueEdit::Remove { index: index as u32 }).await)?
            }
            Command::MoveInQueue { from, to } => ack(api
                .queue_edit(k::QueueEdit::Move {
                    from: from as u32,
                    to: to as u32,
                })
                .await)?,
            Command::ClearQueue => ack(api.queue_edit(k::QueueEdit::ClearUpcoming).await)?,
            Command::JumpTo { index } => {
                ack(api.queue_edit(k::QueueEdit::Jump { index: index as u32 }).await)?
            }
            Command::Toggle => ack(api.player_command(k::PlayerCommand::Toggle).await)?,
            Command::Pause => ack(api.player_command(k::PlayerCommand::Pause).await)?,
            Command::Resume => ack(api.player_command(k::PlayerCommand::Play).await)?,
            Command::Next => ack(api.player_command(k::PlayerCommand::Next).await)?,
            Command::Previous => ack(api.player_command(k::PlayerCommand::Previous).await)?,
            Command::SeekTo { position_ms } => {
                ack(api.player_command(k::PlayerCommand::Seek { position_ms }).await)?
            }
            Command::SetVolume { volume } => {
                ack(api.player_command(k::PlayerCommand::SetVolume { volume }).await)?
            }
            Command::SetMuted { muted } => {
                ack(api.player_command(k::PlayerCommand::SetMuted { muted }).await)?
            }
            Command::SetRepeat { repeat } => ack(api
                .player_command(k::PlayerCommand::SetMode {
                    shuffle: None,
                    loop_mode: Some(match repeat {
                        Repeat::Off => k::LoopMode::None,
                        Repeat::All => k::LoopMode::Queue,
                        Repeat::One => k::LoopMode::Track,
                    }),
                })
                .await)?,
            Command::SetShuffle { shuffle } => ack(api
                .player_command(k::PlayerCommand::SetMode {
                    shuffle: Some(shuffle),
                    loop_mode: None,
                })
                .await)?,
            Command::SetMode { mode } => ack(api
                .player_command(k::PlayerCommand::SetVersion {
                    version: match mode {
                        formalmusic_api::PlaybackMode::Song => k::TrackVersion::Song,
                        formalmusic_api::PlaybackMode::Video => k::TrackVersion::Video,
                    },
                })
                .await)?,
            Command::PlayerState => Reply::Player(self.player_snapshot().await?),
            Command::QueueState => Reply::Queue(self.queue_snapshot().await?),
            Command::ReloadSettings => {
                self.apply_settings().await?;
                Reply::Ok
            }
        })
    }
}

/// The queue context for a play, and the track it has to start on when the
/// daemon builds the list itself and may not line up with the rows shown.
///
/// A mix (`RD...`) is answered afresh by every fetch, so one the page already
/// lists plays those rows: asking the daemon for it again starts some other
/// mix. Without rows (a card on Home) it starts the mix as the daemon has it.
fn play_context(
    source: PlaySource,
    start_index: usize,
    shuffle: bool,
    radio: bool,
) -> (k::QueueContext, Option<String>) {
    let keys = |tracks: Vec<Track>| tracks.into_iter().map(|track| track.video_id).collect();
    match source {
        PlaySource::Tracks { tracks } => (k::QueueContext::Tracks { keys: keys(tracks) }, None),
        PlaySource::Radio { video_id } => (k::QueueContext::TrackRadio { key: video_id }, None),
        PlaySource::Playlist { playlist_id, .. } if radio => {
            (k::QueueContext::PlaylistRadio { id: playlist_id }, None)
        }
        PlaySource::Playlist {
            playlist_id,
            tracks,
        } if playlist_id.starts_with("RD") && !tracks.is_empty() => {
            (k::QueueContext::Tracks { keys: keys(tracks) }, None)
        }
        PlaySource::Playlist {
            playlist_id,
            tracks,
        } => {
            let start_on = (!shuffle)
                .then(|| tracks.get(start_index).map(|track| track.video_id.clone()))
                .flatten();
            (playlist_context(playlist_id), start_on)
        }
    }
}

/// The play-order index to jump to when the queue the daemon built does not
/// start on `key`: its list can drop a repeated song or keep another order
/// than the page, which moves every row after it.
fn start_correction(snapshot: &k::QueueSnapshot, key: &str) -> Option<u32> {
    let position = snapshot.position? as usize;
    if snapshot.items.get(position).is_some_and(|item| item.key == key) {
        return None;
    }
    snapshot
        .items
        .iter()
        .position(|item| item.key == key)
        .map(|index| index as u32)
}

/// The queue context that plays a playlist id the pages handed out.
fn playlist_context(id: String) -> k::QueueContext {
    if id.starts_with("MPRE") {
        k::QueueContext::Album { id }
    } else if id.starts_with("RD") {
        k::QueueContext::PlaylistRadio { id }
    } else {
        k::QueueContext::Playlist { id }
    }
}

fn account_of(account: k::SourceAccount, source: &k::SourceInfo) -> Account {
    Account {
        name: account.name,
        handle: account.handle,
        thumbnails: match account.active {
            true => convert::thumbnails(source.avatar.as_ref()),
            false => Vec::new(),
        },
        page_id: account.id,
        selected: account.active,
    }
}

/// How far into the track the daemon holds audio, from its byte ranges.
fn buffered_ms(ranges: &[k::BufferedRange], duration_ms: Option<u64>) -> u64 {
    let (Some(duration), Some(range)) = (duration_ms, ranges.iter().max_by_key(|r| r.end)) else {
        return 0;
    };
    match range.total {
        Some(total) if total > 0 => duration.saturating_mul(range.end) / total,
        _ => 0,
    }
}

fn settings_edits(
    ours: &formalmusic_api::settings::Settings,
    theirs: &kopuz_config::Playback,
) -> Vec<k::SettingsEdit> {
    use formalmusic_api::settings::Quality;
    use kopuz_config::{PlaybackEdit as E, ReplayGainMode, StreamQuality};
    let playback = &ours.playback;
    let mut edits = Vec::new();
    let mut edit = |differs: bool, edit: E| {
        if differs {
            edits.push(k::SettingsEdit::Playback(edit));
        }
    };
    let quality = match playback.quality {
        Quality::Auto | Quality::High => StreamQuality::High,
        Quality::Normal => StreamQuality::Normal,
        Quality::Low => StreamQuality::Low,
    };
    edit(theirs.stream_quality != quality, E::StreamQuality(quality));
    edit(
        theirs.autoplay_radio != playback.autoplay,
        E::AutoplayRadio(playback.autoplay),
    );
    edit(
        theirs.skip_explicit != playback.restrict_explicit,
        E::SkipExplicit(playback.restrict_explicit),
    );
    let paused = !ours.privacy.report_history;
    edit(theirs.pause_watch_history != paused, E::PauseWatchHistory(paused));
    let crossfade = (playback.crossfade_ms.div_ceil(1000)).min(u8::MAX as u32) as u8;
    edit(theirs.crossfade_seconds != crossfade, E::CrossfadeSeconds(crossfade));
    let mut gain = theirs.replay_gain;
    gain.mode = match playback.normalisation {
        true => ReplayGainMode::Auto,
        false => ReplayGainMode::Off,
    };
    edit(theirs.replay_gain != gain, E::ReplayGain(gain));
    let mut equalizer = theirs.equalizer.clone();
    let gains = playback.equalizer.gains();
    equalizer.enabled = gains.is_some_and(|gains| gains.iter().any(|db| *db != 0.0));
    if let Some(gains) = gains {
        equalizer.preset = kopuz_config::EqPreset::Custom;
        equalizer.bands = gains;
    }
    edit(theirs.equalizer != equalizer, E::Equalizer(equalizer));
    edits
}

async fn supervise(shared: Arc<Shared>, events: mpsc::UnboundedSender<TransportEvent>) {
    let mut delay = RETRY_MIN;
    let mut failures = 0u32;
    while !shared.stopped.load(Ordering::SeqCst) {
        match shared.api().handshake().await {
            Ok(k::Handshake::Ready(status)) => {
                failures = 0;
                delay = RETRY_MIN;
                tracing::info!(version = status.version, "attached to kopuz");
                attached(&shared, &events).await;
                if shared.stopped.load(Ordering::SeqCst) {
                    return;
                }
                let _ = events.send(TransportEvent::Connection {
                    status: ConnectionStatus::Connecting,
                    error: Some("The Kopuz daemon closed the connection.".into()),
                });
            }
            Ok(k::Handshake::Mismatched { daemon, client }) => {
                failures = OFFLINE_AFTER;
                let _ = events.send(TransportEvent::Connection {
                    status: ConnectionStatus::Offline,
                    error: Some(format!(
                        "The running Kopuz daemon speaks wire revision {daemon}, this build {client}. Update one of them."
                    )),
                });
                delay = RETRY_MAX;
            }
            Err(error) => {
                failures += 1;
                if error.code == k::ErrorCode::DaemonGone {
                    maybe_spawn(&shared);
                }
                let status = match failures >= OFFLINE_AFTER {
                    true => ConnectionStatus::Offline,
                    false => ConnectionStatus::Connecting,
                };
                let _ = events.send(TransportEvent::Connection {
                    status,
                    error: Some(match error.code {
                        k::ErrorCode::DaemonGone => "The Kopuz daemon is not running.".into(),
                        _ => format!("Could not reach the Kopuz daemon: {}", error.message),
                    }),
                });
            }
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(RETRY_MAX);
    }
}

/// One attachment: subscribe, send the snapshots, then follow the events until the stream
/// ends with the daemon.
async fn attached(shared: &Arc<Shared>, events: &mpsc::UnboundedSender<TransportEvent>) {
    let mut stream = shared.api().events();
    shared.refresh_liked().await;
    let send = |event: Event| {
        let _ = events.send(TransportEvent::Event(event));
    };
    let resync = async |shared: &Arc<Shared>| {
        if let Ok(player) = shared.player_snapshot().await {
            send(Event::Player(Box::new(player)));
        }
        if let Ok(queue) = shared.queue_snapshot().await {
            send(Event::Queue(queue));
        }
    };
    resync(shared).await;
    let _ = events.send(TransportEvent::Connection {
        status: ConnectionStatus::Online,
        error: None,
    });
    if let Err(error) = shared.apply_settings().await {
        tracing::warn!("settings not applied: {error}");
    }
    while let Some(event) = stream.next().await {
        match event {
            k::ApiEvent::PlayerState(state) => {
                send(Event::Player(Box::new(shared.on_player_state(&state))));
                // Next, previous and a track ending move the queue's index without a
                // QueueChanged; only the player state says so.
                let at = Some((state.queue.rev, state.queue.index));
                let moved = std::mem::replace(&mut *shared.queue_at.lock(), at) != at;
                if moved && let Ok(queue) = shared.queue_snapshot().await {
                    send(Event::Queue(queue));
                }
            }
            k::ApiEvent::PlayerPosition {
                token,
                position_ms,
                at_ms,
                playing,
            } => {
                let (position_ms, buffered_ms) = {
                    let mut clock = shared.clock.lock();
                    if clock.token.is_some_and(|current| current != token) {
                        continue;
                    }
                    let now = clock.daemon_now();
                    clock.position_ms = match playing {
                        true => position_ms + now.saturating_sub(at_ms),
                        false => position_ms,
                    };
                    (clock.position_ms, clock.buffered_ms)
                };
                send(Event::Position {
                    position_ms,
                    buffered_ms,
                });
            }
            k::ApiEvent::PlayerBuffered { token, ranges } => {
                let (position_ms, buffered_ms) = {
                    let mut clock = shared.clock.lock();
                    if clock.token.is_some_and(|current| current != token) {
                        continue;
                    }
                    clock.buffered_ms = buffered_ms(&ranges, clock.duration_ms);
                    (clock.position_ms, clock.buffered_ms)
                };
                send(Event::Position {
                    position_ms,
                    buffered_ms,
                });
            }
            k::ApiEvent::QueueChanged { .. } => {
                if let Ok(queue) = shared.queue_snapshot().await {
                    send(Event::Queue(queue));
                }
            }
            k::ApiEvent::LibraryInvalidated { table } => {
                let scope = match table {
                    k::Table::Favorites => {
                        shared.refresh_liked().await;
                        LibraryScope::Likes
                    }
                    k::Table::Playlists => LibraryScope::Playlists,
                    k::Table::Tracks => LibraryScope::Songs,
                    k::Table::Albums => LibraryScope::Albums,
                    k::Table::Artists => LibraryScope::Subscriptions,
                    k::Table::Recents => LibraryScope::History,
                    _ => continue,
                };
                send(Event::LibraryChanged { scope });
            }
            k::ApiEvent::Notice { code, message, .. } => send(Event::Notice {
                message: message.unwrap_or(code),
            }),
            k::ApiEvent::SourceStatus { .. } => {
                if let Ok(session) = shared.session().await {
                    send(Event::Session(session));
                }
            }
            k::ApiEvent::Resync => {
                shared.refresh_liked().await;
                resync(shared).await;
                if let Ok(session) = shared.session().await {
                    send(Event::Session(session));
                }
            }
            _ => {}
        }
    }
}

/// Starts `kopuzd` from `PATH` in its own process group, so it outlives the window. A
/// daemon run by a service manager or the Kopuz app is found on the socket first.
fn maybe_spawn(shared: &Shared) {
    {
        let mut last = shared.last_spawn.lock();
        if last.is_some_and(|at| at.elapsed() < SPAWN_GAP) {
            return;
        }
        *last = Some(Instant::now());
    }
    let mut command = formalmusic_api::process::command("kopuzd");
    command.arg("--socket").arg(&shared.socket);
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
            tracing::info!("started kopuzd");
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(error) => tracing::warn!("could not start kopuzd: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracks(ids: &str) -> Vec<Track> {
        ids.split(' ')
            .map(|id| Track {
                video_id: id.into(),
                ..crate::demo::catalog().albums[0].tracks[0].clone()
            })
            .collect()
    }

    fn keys(context: &k::QueueContext) -> Vec<&str> {
        match context {
            k::QueueContext::Tracks { keys } => keys.iter().map(String::as_str).collect(),
            other => panic!("not a track list: {other:?}"),
        }
    }

    fn mix(rows: &str) -> PlaySource {
        PlaySource::Playlist {
            playlist_id: "RDTMAK5uy_replay".into(),
            tracks: if rows.is_empty() { Vec::new() } else { tracks(rows) },
        }
    }

    #[test]
    fn a_listed_mix_plays_its_rows_from_the_one_clicked() {
        let (context, start_on) = play_context(mix("925 b c"), 0, false, false);
        assert_eq!(keys(&context), ["925", "b", "c"]);
        assert_eq!(start_on, None);
        let (context, _) = play_context(mix("925 b c"), 2, false, false);
        assert_eq!(keys(&context), ["925", "b", "c"]);
    }

    #[test]
    fn a_mix_starts_as_a_radio_only_without_rows_or_when_asked() {
        let (context, _) = play_context(mix(""), 0, false, false);
        assert!(matches!(context, k::QueueContext::PlaylistRadio { .. }));
        let (context, start_on) = play_context(mix("a b"), 0, false, true);
        assert!(matches!(context, k::QueueContext::PlaylistRadio { .. }));
        assert_eq!(start_on, None);
    }

    #[test]
    fn a_playlist_starts_on_the_row_clicked() {
        let source = |rows| PlaySource::Playlist {
            playlist_id: "PLmine".into(),
            tracks: tracks(rows),
        };
        let (context, start_on) = play_context(source("a b a c"), 3, false, false);
        assert_eq!(context, k::QueueContext::Playlist { id: "PLmine".into() });
        assert_eq!(start_on.as_deref(), Some("c"));
        let (_, start_on) = play_context(source("a b"), 0, true, false);
        assert_eq!(start_on, None);
    }

    #[test]
    fn the_queue_moves_to_the_row_clicked_when_the_daemon_listed_it_elsewhere() {
        let snapshot = |ids: &str, position| k::QueueSnapshot {
            items: ids
                .split(' ')
                .map(|key| k::TrackInfo {
                    key: key.into(),
                    ..Default::default()
                })
                .collect(),
            position: Some(position),
            ..Default::default()
        };
        // The page shows "a b a c" and the daemon drops the repeat, so index 3 is past "c".
        assert_eq!(start_correction(&snapshot("a b c", 2), "c"), None);
        assert_eq!(start_correction(&snapshot("a b c", 1), "c"), Some(2));
        assert_eq!(start_correction(&snapshot("a b", 1), "c"), None);
    }
}
