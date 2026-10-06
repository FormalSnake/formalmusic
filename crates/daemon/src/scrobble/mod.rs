//! Scrobbling to Last.fm and ListenBrainz. Each play announces itself as
//! now playing, again on resume, and becomes one scrobble and one listen
//! once it passes [`rule::threshold`]. Plays go through the persisted
//! [`queue::Queue`] so nothing is lost offline.
//!
//! `scrobble.json` (0600) holds the Last.fm session key, the ListenBrainz
//! token and the per-service toggles.

pub mod clean;
mod connect;
mod lastfm;
mod listenbrainz;
mod meta;
mod queue;
mod rule;

use crate::config::{Paths, write_private};
use crate::playback::Playback;
use crate::session::Session;
use crate::signin::BrowserSignIn;
use formalmusic_api::{
    ApiError, BrowseTarget, Event, ListenBrainzSource, PlayerState, ScrobbleAccount,
    ScrobbleService, ScrobbleStatus, Status, Track, TrackKind,
};
use lastfm::LastFm;
use listenbrainz::ListenBrainz;
use meta::Song;
use parking_lot::Mutex;
use queue::{LASTFM_BATCH, LISTENBRAINZ_BATCH, Pending, Queue};
use rule::Listen;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Notify, broadcast};

/// The YouTube Music lookups behind now playing: the album page, or a song
/// search for videos.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(5);
/// Last.fm and MusicBrainz lookups for the scrobble itself.
const ENRICH_TIMEOUT: Duration = Duration::from_secs(15);
const AUTH_POLL: Duration = Duration::from_secs(3);
const AUTH_TIMEOUT: Duration = Duration::from_secs(300);
const SERVICES: [ScrobbleService; 2] = [ScrobbleService::LastFm, ScrobbleService::ListenBrainz];

/// A track that started playing, from [`Playback`].
#[derive(Debug, Clone)]
pub struct Play {
    pub track: Track,
    /// Where it started: zero, or the saved position after a restart.
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
}

#[derive(Debug)]
pub enum Failure {
    /// Offline, rate limited or a server error: try again later.
    Transient(String),
    /// The session key or token no longer works.
    Auth(String),
    /// A Last.fm error code not covered above.
    Api(u64, String),
    /// Refused for good.
    Permanent(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Failure::Transient(m) | Failure::Auth(m) | Failure::Permanent(m) => f.write_str(m),
            Failure::Api(code, m) => write!(f, "{m} ({code})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Account {
    username: Option<String>,
    /// The Last.fm session key or the ListenBrainz token.
    key: Option<String>,
    scrobble: bool,
    now_playing: bool,
    error: Option<String>,
}

impl Default for Account {
    fn default() -> Self {
        Self {
            username: None,
            key: None,
            scrobble: true,
            now_playing: true,
            error: None,
        }
    }
}

impl Account {
    /// Connected and its credentials still work.
    fn usable(&self) -> Option<&str> {
        self.key.as_deref().filter(|_| self.error.is_none())
    }
}

/// The last play that was scrobbled, so resuming it after a restart does
/// not count it twice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LastScrobble {
    video_id: String,
    listened_at: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Stored {
    lastfm: Account,
    listenbrainz: Account,
    last: Option<LastScrobble>,
}

impl Stored {
    fn account(&mut self, service: ScrobbleService) -> &mut Account {
        match service {
            ScrobbleService::LastFm => &mut self.lastfm,
            ScrobbleService::ListenBrainz => &mut self.listenbrainz,
        }
    }
}

struct Current {
    generation: u64,
    video_id: String,
    listened_at: u64,
    listen: Listen,
    /// Set once the YouTube Music lookups are done.
    song: Option<Song>,
    /// The Last.fm and MusicBrainz lookups are done too.
    enriched: bool,
    /// Passed the threshold before the lookups finished.
    due: bool,
}

struct Inner {
    stored: Stored,
    queue: Queue,
    current: Option<Current>,
    generation: u64,
    /// The Last.fm authorisation being waited on, by attempt.
    connecting: Option<u64>,
}

pub struct Scrobbler {
    path: PathBuf,
    http: reqwest::Client,
    lastfm: LastFm,
    listenbrainz: ListenBrainz,
    session: Arc<Session>,
    events: broadcast::Sender<Event>,
    inner: Mutex<Inner>,
    flush: Notify,
}

impl Scrobbler {
    pub fn new(
        paths: &Paths,
        session: Arc<Session>,
        events: broadcast::Sender<Event>,
    ) -> anyhow::Result<Arc<Self>> {
        let path = paths.state.join("scrobble.json");
        let stored = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!(path = %path.display(), "ignoring unreadable scrobble settings: {e}");
                Stored::default()
            }),
            Err(_) => Stored::default(),
        };
        let http = reqwest::Client::builder()
            .user_agent(concat!(
                "FormalMusic/",
                env!("CARGO_PKG_VERSION"),
                " ( https://github.com/FormalSnake/formalmusic )"
            ))
            .timeout(Duration::from_secs(20))
            .build()?;
        Ok(Arc::new(Self {
            path,
            lastfm: LastFm::new(http.clone()),
            listenbrainz: ListenBrainz::new(http.clone()),
            http,
            session,
            events,
            inner: Mutex::new(Inner {
                stored,
                queue: Queue::load(paths.state.join("scrobble-queue.json")),
                current: None,
                generation: 0,
                connecting: None,
            }),
            flush: Notify::new(),
        }))
    }

    pub fn status(&self) -> ScrobbleStatus {
        let inner = self.inner.lock();
        let account = |a: &Account, connecting: bool| ScrobbleAccount {
            username: a.key.as_ref().and(a.username.clone()),
            connecting,
            scrobble: a.scrobble,
            now_playing: a.now_playing,
            error: a.error.clone(),
        };
        ScrobbleStatus {
            lastfm: account(&inner.stored.lastfm, inner.connecting.is_some()),
            listenbrainz: account(&inner.stored.listenbrainz, false),
            queued: inner.queue.len(),
        }
    }

    fn changed(&self) {
        let _ = self.events.send(Event::Scrobbling(self.status()));
    }

    fn save(&self, stored: &Stored) {
        let result = serde_json::to_vec_pretty(stored)
            .map_err(std::io::Error::other)
            .and_then(|bytes| write_private(&self.path, &bytes, 0o600));
        if let Err(e) = result {
            tracing::warn!(path = %self.path.display(), "saving scrobble settings: {e}");
        }
    }

    fn update(&self, f: impl FnOnce(&mut Stored)) {
        {
            let mut inner = self.inner.lock();
            f(&mut inner.stored);
            self.save(&inner.stored);
        }
        self.changed();
    }

    // Commands

    /// Asks Last.fm for a token, opens its "allow access" page in the default
    /// browser and waits there in the background.
    pub async fn connect_lastfm(self: &Arc<Self>) -> Result<(), ApiError> {
        if !LastFm::configured() {
            return Err(ApiError::BadRequest(
                "This build has no Last.fm API key, so it cannot connect to Last.fm.".into(),
            ));
        }
        let token = self
            .lastfm
            .token()
            .await
            .map_err(|e| ApiError::Network(format!("Last.fm: {e}")))?;
        open_in_browser(&lastfm::auth_url(&token));
        let attempt = {
            let mut inner = self.inner.lock();
            inner.generation += 1;
            inner.connecting = Some(inner.generation);
            inner.generation
        };
        self.changed();
        let this = self.clone();
        tokio::spawn(async move { this.wait_for_lastfm(attempt, token).await });
        Ok(())
    }

    async fn wait_for_lastfm(&self, attempt: u64, token: String) {
        let started = Instant::now();
        let outcome = loop {
            tokio::time::sleep(AUTH_POLL).await;
            if self.inner.lock().connecting != Some(attempt) {
                return;
            }
            if started.elapsed() > AUTH_TIMEOUT {
                break None;
            }
            match self.lastfm.session(&token).await {
                Ok(Some(session)) => break Some(session),
                Ok(None) | Err(Failure::Transient(_)) => continue,
                Err(e) => {
                    tracing::warn!("last.fm authorisation failed: {e}");
                    break None;
                }
            }
        };
        {
            let mut inner = self.inner.lock();
            if inner.connecting != Some(attempt) {
                return;
            }
            inner.connecting = None;
            if let Some(session) = outcome {
                tracing::info!(user = session.username, "connected to last.fm");
                let account = &mut inner.stored.lastfm;
                account.key = Some(session.key);
                account.username = Some(session.username);
                account.error = None;
                self.save(&inner.stored);
            }
        }
        self.changed();
        self.flush.notify_one();
    }

    pub async fn connect_listenbrainz(
        &self,
        source: ListenBrainzSource,
        signin: &BrowserSignIn,
    ) -> Result<ScrobbleStatus, ApiError> {
        let token = match source {
            ListenBrainzSource::Token { token } => token.trim().to_owned(),
            ListenBrainzSource::Profile { browser, profile } => {
                connect::token_from_profile(signin, &self.http, &browser, &profile).await?
            }
        };
        let username = self
            .listenbrainz
            .validate(&token)
            .await
            .map_err(|e| ApiError::Network(format!("ListenBrainz: {e}")))?
            .ok_or_else(|| {
                ApiError::BadRequest("ListenBrainz does not recognise that token.".into())
            })?;
        tracing::info!(user = username, "connected to listenbrainz");
        self.update(|s| {
            s.listenbrainz.key = Some(token);
            s.listenbrainz.username = Some(username);
            s.listenbrainz.error = None;
        });
        self.flush.notify_one();
        Ok(self.status())
    }

    pub fn disconnect(&self, service: ScrobbleService) -> ScrobbleStatus {
        {
            let mut inner = self.inner.lock();
            if service == ScrobbleService::LastFm {
                inner.connecting = None;
            }
            let account = inner.stored.account(service);
            account.key = None;
            account.username = None;
            account.error = None;
            inner.queue.drop_service(service);
            self.save(&inner.stored);
        }
        self.changed();
        self.status()
    }

    pub fn set(
        &self,
        service: ScrobbleService,
        scrobble: bool,
        now_playing: bool,
    ) -> ScrobbleStatus {
        self.update(|s| {
            let account = s.account(service);
            account.scrobble = scrobble;
            account.now_playing = now_playing;
        });
        self.status()
    }

    // Playback

    pub async fn run(self: Arc<Self>, playback: Arc<Playback>) {
        let mut plays = playback.subscribe_plays();
        let mut events = self.events.subscribe();
        tokio::spawn(self.clone().flush_loop());
        self.flush.notify_one();
        loop {
            let due_in = {
                let inner = self.inner.lock();
                inner
                    .current
                    .as_ref()
                    .and_then(|c| c.listen.due_in(Instant::now()))
            };
            let tick = async {
                match due_in {
                    Some(wait) => tokio::time::sleep(wait).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                play = plays.recv() => match play {
                    Ok(play) => self.on_play(play, &playback.player_state()),
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                event = events.recv() => match event {
                    Ok(Event::Player(state)) => self.on_player(&state),
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                () = tick => self.check_due(),
            }
        }
    }

    fn on_play(self: &Arc<Self>, play: Play, state: &PlayerState) {
        let now_unix = unix_now();
        let playing = state.status == Status::Playing;
        let generation = {
            let mut inner = self.inner.lock();
            inner.generation += 1;
            let generation = inner.generation;
            let mut listen = Listen::new(play.duration_ms, playing, Instant::now());
            let length_s = play.duration_ms.unwrap_or_default() / 1000;
            if play.position_ms > 0
                && inner.stored.last.as_ref().is_some_and(|last| {
                    last.video_id == play.track.video_id
                        && now_unix.saturating_sub(last.listened_at) < length_s + 600
                })
            {
                listen.mark_counted();
            }
            inner.current = Some(Current {
                generation,
                video_id: play.track.video_id.clone(),
                listened_at: now_unix,
                listen,
                song: None,
                enriched: false,
                due: false,
            });
            generation
        };
        let this = self.clone();
        tokio::spawn(async move { this.resolve(generation, play, playing).await });
    }

    fn on_player(self: &Arc<Self>, state: &PlayerState) {
        let resumed_song = {
            let mut inner = self.inner.lock();
            let Some(current) = inner.current.as_mut() else {
                return;
            };
            if state.track.as_ref().map(|t| t.video_id.as_str()) != Some(&current.video_id) {
                return;
            }
            let resumed = current
                .listen
                .set_playing(state.status == Status::Playing, Instant::now());
            resumed.then(|| current.song.clone()).flatten()
        };
        // Spotify's scrobbler announces the track again on resume; both
        // services drop a now-playing entry after a while without one.
        if let Some(song) = resumed_song {
            self.now_playing(&song);
        }
        self.check_due();
    }

    fn check_due(self: &Arc<Self>) {
        let ready = {
            let mut inner = self.inner.lock();
            let Some(current) = inner.current.as_mut() else {
                return;
            };
            if !current.listen.take_due(Instant::now()) {
                return;
            }
            current.due = true;
            take_ready(current)
        };
        if let Some((song, at)) = ready {
            self.submit(song, at);
        }
    }

    async fn resolve(self: Arc<Self>, generation: u64, play: Play, playing: bool) {
        let client = self.session.client();
        let mut song = Song::from_track(&play.track, play.duration_ms);
        if !song.is_reportable() {
            return;
        }
        let youtube_music = async {
            let album = match play.track.album.as_ref().and_then(|a| a.target.clone()) {
                Some(BrowseTarget::Album(id)) => Some(id),
                _ if play.track.kind != TrackKind::Song => {
                    meta::album_track(&client, &song).await.and_then(|t| {
                        song.album = t.album.as_ref().map(|a| a.text.clone());
                        match t.album?.target? {
                            BrowseTarget::Album(id) => Some(id),
                            _ => None,
                        }
                    })
                }
                _ => None,
            };
            if let Some(id) = album {
                meta::album_details(&client, &id, &mut song).await;
            }
        };
        let _ = tokio::time::timeout(RESOLVE_TIMEOUT, youtube_music).await;
        if !self.store_song(generation, &song, false) {
            return;
        }
        if playing {
            self.now_playing(&song);
        }

        let had_album = song.album.is_some();
        let lastfm = LastFm::configured().then_some(&self.lastfm);
        let _ = tokio::time::timeout(
            ENRICH_TIMEOUT,
            meta::enrich(&mut song, lastfm, &self.listenbrainz, &self.http),
        )
        .await;
        tracing::debug!(?song, "scrobble metadata");
        if !self.store_song(generation, &song, true) {
            return;
        }
        let still_playing = self
            .inner
            .lock()
            .current
            .as_ref()
            .is_some_and(|c| c.listen.due_in(Instant::now()).is_some());
        if !had_album && song.album.is_some() && still_playing {
            self.now_playing(&song);
        }
    }

    /// Keeps the looked-up song on the play it belongs to; false once the
    /// play is over. A play that passed the threshold meanwhile goes out now.
    fn store_song(&self, generation: u64, song: &Song, enriched: bool) -> bool {
        let ready = {
            let mut inner = self.inner.lock();
            let Some(current) = inner
                .current
                .as_mut()
                .filter(|c| c.generation == generation)
            else {
                return false;
            };
            current.song = Some(song.clone());
            current.enriched = enriched;
            take_ready(current)
        };
        if let Some((song, at)) = ready {
            self.submit(song, at);
        }
        true
    }

    fn now_playing(&self, song: &Song) {
        let (lastfm, listenbrainz) = {
            let inner = self.inner.lock();
            let pick = |a: &Account| a.usable().filter(|_| a.now_playing).map(str::to_owned);
            (pick(&inner.stored.lastfm), pick(&inner.stored.listenbrainz))
        };
        if let Some(key) = lastfm {
            let (service, song) = (self.lastfm.clone(), song.clone());
            tokio::spawn(async move {
                if let Err(e) = service.now_playing(&key, &song).await {
                    tracing::warn!("last.fm now playing: {e}");
                }
            });
        }
        if let Some(token) = listenbrainz {
            let (service, song) = (self.listenbrainz.clone(), song.clone());
            tokio::spawn(async move {
                if let Err(e) = service.playing_now(&token, &song).await {
                    tracing::warn!("listenbrainz playing now: {e}");
                }
            });
        }
    }

    fn submit(&self, song: Song, listened_at: u64) {
        {
            let mut inner = self.inner.lock();
            let pending: Vec<Pending> = SERVICES
                .into_iter()
                .filter(|s| {
                    let a = inner.stored.account(*s);
                    a.key.is_some() && a.scrobble
                })
                .map(|service| Pending {
                    service,
                    listened_at,
                    song: song.clone(),
                })
                .collect();
            tracing::info!(
                title = song.title,
                artist = song.artist(),
                services = pending.len(),
                "scrobbling"
            );
            inner.stored.last = Some(LastScrobble {
                video_id: song.video_id.clone(),
                listened_at,
            });
            self.save(&inner.stored);
            inner.queue.push(pending);
        }
        self.changed();
        self.flush.notify_one();
    }

    // Delivery

    async fn flush_loop(self: Arc<Self>) {
        loop {
            let retry = self.inner.lock().queue.next_retry();
            match retry {
                Some(at) => {
                    tokio::select! {
                        () = self.flush.notified() => {}
                        () = tokio::time::sleep_until(at.into()) => {}
                    }
                }
                None => self.flush.notified().await,
            }
            for service in SERVICES {
                self.deliver(service).await;
            }
            self.changed();
        }
    }

    async fn deliver(&self, service: ScrobbleService) {
        loop {
            let (key, batch) = {
                let mut inner = self.inner.lock();
                let Some(key) = inner.stored.account(service).usable().map(str::to_owned) else {
                    return;
                };
                let max = match service {
                    ScrobbleService::LastFm => LASTFM_BATCH,
                    ScrobbleService::ListenBrainz => LISTENBRAINZ_BATCH,
                };
                (key, inner.queue.batch(service, max, Instant::now()))
            };
            if batch.is_empty() {
                return;
            }
            let result = match service {
                ScrobbleService::LastFm => self.lastfm.scrobble(&key, &batch).await,
                ScrobbleService::ListenBrainz => self
                    .listenbrainz
                    .listens(&key, &batch)
                    .await
                    .map(|()| batch.clone()),
            };
            let mut inner = self.inner.lock();
            match result {
                Ok(done) => {
                    tracing::info!(?service, count = done.len(), "submitted");
                    inner.queue.succeeded(service);
                    inner.queue.remove(&done);
                }
                Err(Failure::Transient(e)) => {
                    let wait = inner.queue.failed(service, Instant::now());
                    tracing::warn!(?service, "submission failed, retrying in {wait:?}: {e}");
                    return;
                }
                Err(Failure::Auth(e)) => {
                    tracing::warn!(?service, "credentials refused: {e}");
                    inner.stored.account(service).error =
                        Some("Access was revoked. Connect again.".into());
                    self.save(&inner.stored);
                    return;
                }
                Err(e) => {
                    tracing::warn!(
                        ?service,
                        count = batch.len(),
                        "submission refused, dropping: {e}"
                    );
                    inner.queue.remove(&batch);
                }
            }
        }
    }
}

/// The song and start time once the play is due and fully looked up.
fn take_ready(current: &mut Current) -> Option<(Song, u64)> {
    if !(current.due && current.enriched) {
        return None;
    }
    current.due = false;
    Some((current.song.clone()?, current.listened_at))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

fn open_in_browser(url: &str) {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(program)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        tracing::warn!("could not open the browser: {e}");
    }
}
