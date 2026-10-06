//! The queue wired to the audio engine: which entry plays, resolving streams
//! ahead of time, gapless hand-over, radio top-ups, recovery from expired
//! URLs, playback reporting and the persisted queue.
//!
//! All state sits behind one short `parking_lot` lock that is never held
//! across an await. Slow work (yt-dlp, InnerTube) runs in spawned tasks that
//! check [`State::generation`] when they finish, so a skip that happened in
//! the meantime wins.

use crate::config::{Config, Paths, write_private};
use crate::queue::{Queue, RADIO_LOW_WATER, Removed};
use crate::session::Session;
use crate::streams::Resolver;
use crate::tracking::{Reporter, Watch};
use formalmusic_api::{
    ApiError, Continuation, EnqueuePosition, Event, Item, PlaySource, PlayerState, QueueState,
    Repeat, Status, Track,
};
use formalmusic_innertube::{Client, PlaybackTracking};
use formalmusic_player::{Player, PlayerError, PlayerEvent, StreamSource, TrackId};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::runtime::Handle;
use tokio::sync::{Notify, broadcast};

/// Previous restarts the track instead of going back once this far in.
const RESTART_AFTER_MS: u64 = 3_000;
/// Queue entries resolved before they are needed.
const RESOLVE_AHEAD: usize = 2;
/// Position events go out this often while playing, plus on every play,
/// pause, seek and track change; clients interpolate in between.
const POSITION_EVERY: Duration = Duration::from_secs(1);
/// Writes to `queue.json` wait this long for further changes.
const PERSIST_DEBOUNCE: Duration = Duration::from_secs(2);

pub struct Playback {
    state: Mutex<State>,
    player: Player,
    resolver: Resolver,
    session: Arc<Session>,
    config: Config,
    reporter: Reporter,
    events: broadcast::Sender<Event>,
    /// Seek targets, for the MPRIS `Seeked` signal.
    seeked: broadcast::Sender<u64>,
    tracking: Mutex<HashMap<String, PlaybackTracking>>,
    persist: Notify,
    queue_path: PathBuf,
    rt: Handle,
    rng: Mutex<fastrand::Rng>,
}

#[derive(Default)]
struct State {
    queue: Queue,
    source_title: Option<String>,
    radio: Option<Radio>,
    status: Status,
    /// Whether playback should run once the current load finishes.
    want_play: bool,
    position_ms: u64,
    /// When `position_ms` was last reported by the engine.
    position_at: Option<Instant>,
    /// When the last Position event went to clients.
    position_sent: Option<Instant>,
    duration_ms: Option<u64>,
    volume: f32,
    muted: bool,
    stream: Option<String>,
    related_browse_id: Option<String>,
    generation: u64,
    loaded: Option<Loaded>,
    preload: Option<Preload>,
    watch: Option<Watch>,
    /// Consecutive entries that failed to resolve, so a queue of dead tracks
    /// stops instead of spinning.
    failures: usize,
}

#[derive(Debug, Clone, Copy)]
struct Loaded {
    id: TrackId,
    uid: u64,
    /// Already re-resolved once after an expired URL.
    retried: bool,
    /// Resumed after a re-resolve; the play was reported already.
    resumed: bool,
}

#[derive(Debug, Clone)]
struct Preload {
    uid: u64,
    /// `None` while the stream is resolving.
    id: Option<TrackId>,
    label: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct Radio {
    playlist_id: Option<String>,
    continuation: Option<Continuation>,
    fetching: bool,
    /// The queue ran out while a top-up was in flight; play on when it lands.
    resume_when_extended: bool,
}

/// What `queue.json` holds.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Saved {
    queue: Queue,
    source_title: Option<String>,
    radio: bool,
    position_ms: u64,
    volume: f32,
    muted: bool,
}

impl Playback {
    pub fn new(
        player: Player,
        session: Arc<Session>,
        config: Config,
        paths: &Paths,
        events: broadcast::Sender<Event>,
    ) -> anyhow::Result<Arc<Self>> {
        player.set_normalisation(config.normalisation);
        player.set_crossfade(config.crossfade_ms);
        let queue_path = paths.queue();
        let mut state = State {
            volume: 1.0,
            ..State::default()
        };
        match std::fs::read(&queue_path) {
            Ok(bytes) => match serde_json::from_slice::<Saved>(&bytes) {
                Ok(saved) => {
                    state.queue = saved.queue;
                    state.source_title = saved.source_title;
                    state.radio = saved.radio.then(Radio::default);
                    state.position_ms = saved.position_ms;
                    state.volume = saved.volume.clamp(0.0, 1.0);
                    state.muted = saved.muted;
                    if let Some(entry) = state.queue.current() {
                        state.status = Status::Paused;
                        state.duration_ms = entry.track.duration_ms;
                    }
                }
                Err(e) => tracing::warn!("ignoring unreadable queue.json: {e}"),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("cannot read queue.json: {e}"),
        }
        player.set_volume(state.volume);
        player.set_muted(state.muted);
        let (seeked, _) = broadcast::channel(16);
        let this = Arc::new(Self {
            state: Mutex::new(state),
            player,
            resolver: Resolver::new(paths.state.clone(), config.preferred_quality),
            session,
            config,
            reporter: Reporter::new()?,
            events,
            seeked,
            tracking: Mutex::new(HashMap::new()),
            persist: Notify::new(),
            queue_path,
            rt: Handle::current(),
            rng: Mutex::new(fastrand::Rng::new()),
        });
        this.rt.spawn(this.clone().player_events());
        this.rt.spawn(this.clone().persist_loop());
        Ok(this)
    }

    // Snapshots

    pub fn player_state(&self) -> PlayerState {
        player_state(&self.state.lock())
    }

    pub fn queue_state(&self) -> QueueState {
        queue_state(&self.state.lock())
    }

    /// The entry that plays after the current one, for warming its extras.
    pub fn next_track(&self) -> Option<Track> {
        let st = self.state.lock();
        st.queue.upcoming(1).first().map(|e| e.track.clone())
    }

    /// A queued track with this id, so lookups need not ask YouTube for it.
    pub fn queued_track(&self, video_id: &str) -> Option<Track> {
        let st = self.state.lock();
        st.queue
            .entries()
            .iter()
            .find(|e| e.track.video_id == video_id)
            .map(|e| e.track.clone())
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    fn emit_player(&self, st: &State) {
        self.emit(Event::Player(player_state(st)));
    }

    fn emit_position(&self, st: &mut State, buffered_ms: u64) {
        st.position_sent = Some(Instant::now());
        self.emit(Event::Position {
            position_ms: st.position_ms,
            buffered_ms,
        });
    }

    fn emit_queue(&self, st: &State) {
        self.emit(Event::Queue(queue_state(st)));
        self.persist.notify_one();
    }

    // Commands

    pub async fn play(
        self: &Arc<Self>,
        source: PlaySource,
        start: usize,
        shuffle: bool,
        radio: bool,
    ) -> Result<(), ApiError> {
        let client = self.session.client();
        let (tracks, title, radio_state) = match source {
            PlaySource::Tracks { tracks } => (tracks, None, None),
            PlaySource::Playlist { playlist_id } => {
                let (title, tracks) = fetch_playlist(&client, &playlist_id).await?;
                (tracks, title, None)
            }
            PlaySource::Radio { video_id } => {
                let next = client.radio(&video_id).await?;
                let title = next.tracks.first().map(|t| format!("{} radio", t.title));
                let radio = Radio {
                    playlist_id: next.playlist_id,
                    continuation: next.continuation,
                    ..Radio::default()
                };
                (next.tracks, title, Some(radio))
            }
        };
        if tracks.is_empty() {
            return Err(ApiError::NotFound("nothing playable in this source".into()));
        }
        let mut st = self.state.lock();
        st.queue
            .replace(tracks, start, shuffle, &mut self.rng.lock());
        st.source_title = title;
        st.radio = radio_state.or_else(|| radio.then(Radio::default));
        st.failures = 0;
        self.start_current(&mut st, 0, true);
        self.emit_queue(&st);
        Ok(())
    }

    pub fn enqueue(self: &Arc<Self>, tracks: Vec<Track>, position: EnqueuePosition) {
        let mut st = self.state.lock();
        if st.queue.enqueue(tracks, position) {
            self.start_current(&mut st, 0, true);
        }
        self.queue_changed(&mut st);
    }

    pub fn remove(self: &Arc<Self>, index: usize) -> Result<(), ApiError> {
        let mut st = self.state.lock();
        match st.queue.remove(index).ok_or_else(|| out_of_range(index))? {
            Removed::Current if st.queue.current().is_some() => {
                let play = st.want_play;
                self.start_current(&mut st, 0, play);
            }
            Removed::Current => self.stop(&mut st),
            Removed::Other => {}
        }
        self.queue_changed(&mut st);
        Ok(())
    }

    pub fn move_entry(self: &Arc<Self>, from: usize, to: usize) -> Result<(), ApiError> {
        let mut st = self.state.lock();
        if !st.queue.move_entry(from, to) {
            return Err(out_of_range(from.max(to)));
        }
        self.queue_changed(&mut st);
        Ok(())
    }

    pub fn clear(self: &Arc<Self>) {
        let mut st = self.state.lock();
        st.queue.clear();
        self.queue_changed(&mut st);
    }

    pub fn jump(self: &Arc<Self>, index: usize) -> Result<(), ApiError> {
        let mut st = self.state.lock();
        if !st.queue.jump(index) {
            return Err(out_of_range(index));
        }
        st.failures = 0;
        self.start_current(&mut st, 0, true);
        self.emit_queue(&st);
        Ok(())
    }

    pub fn toggle(self: &Arc<Self>) {
        let playing = matches!(self.state.lock().status, Status::Playing | Status::Loading);
        if playing { self.pause() } else { self.resume() }
    }

    pub fn pause(&self) {
        let mut st = self.state.lock();
        st.want_play = false;
        if st.loaded.is_some() {
            self.player.pause();
        }
        if st.status != Status::Stopped {
            st.status = Status::Paused;
        }
        self.emit_player(&st);
        self.persist.notify_one();
    }

    pub fn resume(self: &Arc<Self>) {
        let mut st = self.state.lock();
        st.want_play = true;
        if st.loaded.is_some() {
            self.player.play();
            st.status = Status::Playing;
            self.emit_player(&st);
        } else if st.queue.current().is_some() && st.status != Status::Loading {
            // Restored or stopped at the end: nothing is loaded yet.
            let position = if st.status == Status::Stopped {
                0
            } else {
                st.position_ms
            };
            st.failures = 0;
            self.start_current(&mut st, position, true);
        }
    }

    pub fn next(self: &Arc<Self>) {
        let mut st = self.state.lock();
        if let Some(index) = st.queue.next_index(false) {
            st.queue.jump(index);
            st.failures = 0;
            self.start_current(&mut st, 0, true);
            self.emit_queue(&st);
        }
    }

    pub fn previous(self: &Arc<Self>) {
        let mut st = self.state.lock();
        match st.queue.previous_index() {
            Some(index) if st.position_ms <= RESTART_AFTER_MS => {
                st.queue.jump(index);
                st.failures = 0;
                self.start_current(&mut st, 0, true);
                self.emit_queue(&st);
            }
            _ => self.seek_locked(&mut st, 0),
        }
    }

    pub fn seek(&self, position_ms: u64) {
        let mut st = self.state.lock();
        self.seek_locked(&mut st, position_ms);
    }

    fn seek_locked(&self, st: &mut State, position_ms: u64) {
        let position_ms = st.duration_ms.map_or(position_ms, |d| position_ms.min(d));
        if st.loaded.is_some() {
            self.player.seek(position_ms);
        }
        st.position_ms = position_ms;
        st.position_at = Some(Instant::now());
        self.emit_position(st, 0);
        let _ = self.seeked.send(position_ms);
    }

    pub fn set_volume(&self, volume: f32) {
        let volume = volume.clamp(0.0, 1.0);
        self.player.set_volume(volume);
        let mut st = self.state.lock();
        st.volume = volume;
        self.emit_player(&st);
        self.persist.notify_one();
    }

    pub fn set_muted(&self, muted: bool) {
        self.player.set_muted(muted);
        let mut st = self.state.lock();
        st.muted = muted;
        self.emit_player(&st);
        self.persist.notify_one();
    }

    pub fn set_repeat(self: &Arc<Self>, repeat: Repeat) {
        let mut st = self.state.lock();
        st.queue.repeat = repeat;
        self.emit_player(&st);
        self.queue_changed(&mut st);
    }

    pub fn set_shuffle(self: &Arc<Self>, shuffle: bool) {
        let mut st = self.state.lock();
        st.queue.set_shuffle(shuffle, &mut self.rng.lock());
        self.emit_player(&st);
        self.queue_changed(&mut st);
    }

    /// Cached streams and radio tokens belong to the previous account.
    pub fn session_changed(&self) {
        self.resolver.clear();
        self.tracking.lock().clear();
        if let Some(radio) = &mut self.state.lock().radio {
            radio.continuation = None;
        }
    }

    /// Writes the queue with the live position, for shutdown.
    pub fn save_now(&self) {
        let saved = self.saved();
        if let Err(e) = self.write(&saved) {
            tracing::warn!("saving the queue: {e}");
        }
        self.player.stop();
    }

    // Loading

    /// Plays the current entry from `start_ms`, dropping whatever was loaded.
    fn start_current(self: &Arc<Self>, st: &mut State, start_ms: u64, play: bool) {
        self.finish_watch(st);
        st.generation += 1;
        st.loaded = None;
        st.preload = None;
        st.want_play = play;
        st.position_ms = start_ms;
        st.stream = None;
        st.related_browse_id = None;
        let Some(entry) = st.queue.current().cloned() else {
            self.stop(st);
            return;
        };
        self.player.stop();
        st.duration_ms = entry.track.duration_ms;
        st.status = Status::Loading;
        self.emit_player(st);

        let generation = st.generation;
        let this = self.clone();
        self.rt.spawn(async move {
            let prepared = this.prepare(&entry.track.video_id).await;
            this.on_prepared(generation, entry.uid, prepared, start_ms, false);
        });
    }

    fn stop(&self, st: &mut State) {
        self.finish_watch(st);
        st.generation += 1;
        st.loaded = None;
        st.preload = None;
        st.want_play = false;
        st.status = Status::Stopped;
        st.position_ms = 0;
        st.stream = None;
        self.player.stop();
        self.emit_player(st);
    }

    /// Resolves a stream and the loudness to play it at.
    async fn prepare(&self, video_id: &str) -> Result<(StreamSource, Option<f32>), String> {
        let cookies = self.session.cookies();
        let (source, tracking) = tokio::join!(
            self.resolver.resolve(video_id, cookies.as_deref()),
            self.playback_tracking(video_id)
        );
        Ok((
            source?,
            tracking.and_then(|t| t.loudness_db).map(|db| db as f32),
        ))
    }

    async fn playback_tracking(&self, video_id: &str) -> Option<PlaybackTracking> {
        if let Some(tracking) = self.tracking.lock().get(video_id) {
            return Some(tracking.clone());
        }
        match self.session.client().playback_tracking(video_id).await {
            Ok(tracking) => {
                let mut cache = self.tracking.lock();
                if cache.len() > 64 {
                    cache.clear();
                }
                cache.insert(video_id.to_owned(), tracking.clone());
                Some(tracking)
            }
            Err(e) => {
                tracing::debug!(video_id, "no playback tracking: {e}");
                None
            }
        }
    }

    fn on_prepared(
        self: &Arc<Self>,
        generation: u64,
        uid: u64,
        prepared: Result<(StreamSource, Option<f32>), String>,
        start_ms: u64,
        resumed: bool,
    ) {
        let mut st = self.state.lock();
        if st.generation != generation {
            return;
        }
        match prepared {
            Ok((source, loudness)) => {
                st.stream = Some(source.label());
                let id = self.player.load(source, start_ms, loudness);
                if !st.want_play {
                    self.player.pause();
                }
                st.loaded = Some(Loaded {
                    id,
                    uid,
                    retried: resumed,
                    resumed,
                });
                st.failures = 0;
            }
            Err(message) => self.skip_failed(&mut st, &message),
        }
    }

    /// Tells the user and moves on to the next entry.
    fn skip_failed(self: &Arc<Self>, st: &mut State, message: &str) {
        let title = st
            .queue
            .current()
            .map(|e| e.track.title.clone())
            .unwrap_or_default();
        tracing::warn!(%title, "skipping: {message}");
        self.emit(Event::Notice {
            message: format!("Skipped \"{title}\": {message}"),
        });
        st.failures += 1;
        match st.queue.next_index(false) {
            Some(index) if st.failures < st.queue.len() => {
                st.queue.jump(index);
                let play = st.want_play;
                self.start_current(st, 0, play);
                self.emit_queue(st);
            }
            _ => self.stop(st),
        }
    }

    /// Resolves the next entries ahead of time, so a track change never
    /// waits for yt-dlp.
    fn resolve_ahead(self: &Arc<Self>, st: &State) {
        for entry in st.queue.upcoming(RESOLVE_AHEAD) {
            let this = self.clone();
            let video_id = entry.track.video_id.clone();
            self.rt.spawn(async move {
                let _ = this.prepare(&video_id).await;
            });
        }
    }

    /// Hands the engine the entry that follows `current`, for a gapless start.
    fn preload_next(self: &Arc<Self>, st: &mut State, current: TrackId) {
        let Some(index) = st.queue.next_index(true) else {
            st.preload = None;
            return;
        };
        let entry = st.queue.entries()[index].clone();
        st.preload = Some(Preload {
            uid: entry.uid,
            id: None,
            label: None,
        });
        let generation = st.generation;
        let this = self.clone();
        self.rt.spawn(async move {
            let prepared = this.prepare(&entry.track.video_id).await;
            let mut st = this.state.lock();
            let still_wanted = st.generation == generation
                && st.loaded.is_some_and(|l| l.id == current)
                && st
                    .preload
                    .as_ref()
                    .is_some_and(|p| p.uid == entry.uid && p.id.is_none());
            if !still_wanted {
                return;
            }
            match prepared {
                Ok((source, loudness)) => {
                    let label = source.label();
                    let id = this.player.preload_next(source, loudness);
                    st.preload = Some(Preload {
                        uid: entry.uid,
                        id: Some(id),
                        label: Some(label),
                    });
                }
                // The track end loads it the slow way and reports the failure.
                Err(_) => st.preload = None,
            }
        });
    }

    fn queue_changed(self: &Arc<Self>, st: &mut State) {
        // The preloaded entry may no longer be the next one.
        if let (Some(preload), Some(loaded)) = (st.preload.clone(), st.loaded) {
            let next_uid = st.queue.next_index(true).map(|i| st.queue.entries()[i].uid);
            if next_uid != Some(preload.uid) && next_uid.is_some() {
                self.preload_next(st, loaded.id);
            }
        }
        self.emit_queue(st);
        self.resolve_ahead(st);
        self.extend_radio(st);
    }

    // Radio

    fn extend_radio(self: &Arc<Self>, st: &mut State) {
        let Some(radio) = &mut st.radio else { return };
        if radio.fetching || st.queue.remaining() >= RADIO_LOW_WATER || st.queue.len() == 0 {
            return;
        }
        radio.fetching = true;
        let request = (
            radio.playlist_id.clone(),
            radio.continuation.clone(),
            st.queue.entries().last().map(|e| e.track.video_id.clone()),
        );
        let this = self.clone();
        self.rt.spawn(async move {
            let client = this.session.client();
            let result = match request {
                (Some(playlist_id), Some(token), _) => {
                    match client.next_continuation(&playlist_id, &token).await {
                        Ok(next) => Ok(next),
                        // Tokens die with the client that made them, after a sign-in.
                        Err(_) if request.2.is_some() => {
                            client.radio(request.2.as_deref().unwrap_or_default()).await
                        }
                        Err(e) => Err(e),
                    }
                }
                (_, _, Some(seed)) => client.radio(&seed).await,
                _ => return,
            };
            let mut st = this.state.lock();
            let Some(radio) = &mut st.radio else { return };
            radio.fetching = false;
            let next = match result {
                Ok(next) => next,
                Err(e) => {
                    tracing::warn!("radio top-up failed: {e}");
                    return;
                }
            };
            if next.playlist_id.is_some() {
                radio.playlist_id = next.playlist_id;
            }
            radio.continuation = next.continuation;
            let resume = std::mem::take(&mut radio.resume_when_extended);
            let added = st.queue.append_radio(next.tracks);
            tracing::debug!(added, "radio extended the queue");
            if added == 0 {
                return;
            }
            if resume && let Some(index) = st.queue.next_index(false) {
                st.queue.jump(index);
                this.start_current(&mut st, 0, true);
            }
            this.queue_changed(&mut st);
        });
    }

    // Player events

    async fn player_events(self: Arc<Self>) {
        let mut events = self.player.subscribe();
        loop {
            match events.recv().await {
                Ok(event) => self.on_player_event(event),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(n, "missed player events")
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }

    fn on_player_event(self: &Arc<Self>, event: PlayerEvent) {
        let mut st = self.state.lock();
        let loaded = st.loaded;
        let is_loaded = |track: TrackId| loaded.is_some_and(|l| l.id == track);
        match event {
            PlayerEvent::StateChanged(status) => {
                if loaded.is_none() || st.status == status {
                    return;
                }
                // The engine pauses itself when a load was asked to stay paused.
                if status == Status::Playing && !st.want_play {
                    return;
                }
                st.status = status;
                self.emit_player(&st);
                self.emit_position(&mut st, 0);
                if status != Status::Playing {
                    self.persist.notify_one();
                }
            }
            PlayerEvent::TrackStarted { track, duration_ms } => {
                if let Some(preload) = st.preload.clone().filter(|p| p.id == Some(track)) {
                    // Gapless hand-over to the preloaded entry.
                    self.finish_watch(&mut st);
                    let Some(index) = st.queue.index_of(preload.uid) else {
                        // Removed from the queue after it was preloaded.
                        self.stop(&mut st);
                        return;
                    };
                    st.queue.jump(index);
                    st.loaded = Some(Loaded {
                        id: track,
                        uid: preload.uid,
                        retried: false,
                        resumed: false,
                    });
                    st.preload = None;
                    st.stream = preload.label;
                    st.related_browse_id = None;
                    st.position_ms = 0;
                    self.emit_queue(&st);
                } else if !is_loaded(track) {
                    return;
                }
                if duration_ms.is_some() {
                    st.duration_ms = duration_ms;
                }
                if st.want_play {
                    st.status = Status::Playing;
                }
                self.emit_player(&st);
                self.emit_position(&mut st, 0);
                self.track_started(&mut st);
            }
            PlayerEvent::Position {
                track,
                position_ms,
                buffered_ms,
            } => {
                if !is_loaded(track) {
                    return;
                }
                st.position_ms = position_ms;
                st.position_at = Some(Instant::now());
                if st
                    .position_sent
                    .is_none_or(|at| at.elapsed() >= POSITION_EVERY)
                {
                    self.emit_position(&mut st, buffered_ms);
                }
                if let Some(url) = st.watch.as_mut().and_then(|w| w.on_position(position_ms)) {
                    self.send_report(url);
                }
            }
            PlayerEvent::NeedsNext { track } => {
                if is_loaded(track) && st.preload.is_none() {
                    self.preload_next(&mut st, track);
                }
            }
            PlayerEvent::TrackEnded { track } => {
                if !is_loaded(track) || st.preload.as_ref().is_some_and(|p| p.id.is_some()) {
                    return;
                }
                self.finish_watch(&mut st);
                match st.queue.next_index(true) {
                    Some(index) => {
                        st.queue.jump(index);
                        self.start_current(&mut st, 0, true);
                        self.emit_queue(&st);
                    }
                    None => {
                        if let Some(radio) = &mut st.radio {
                            radio.resume_when_extended = true;
                        }
                        self.stop(&mut st);
                        self.extend_radio(&mut st);
                    }
                }
            }
            PlayerEvent::Error { track, error } => {
                if let Some(track) = track
                    && st.preload.as_ref().is_some_and(|p| p.id == Some(track))
                {
                    st.preload = None;
                    return;
                }
                match (track, loaded) {
                    (Some(track), Some(l)) if l.id == track => {
                        if error == PlayerError::Expired && !l.retried {
                            self.reresolve(&mut st, l);
                        } else {
                            self.skip_failed(&mut st, &error.to_string());
                        }
                    }
                    (None, _) => {
                        tracing::warn!("audio output: {error}");
                        self.emit(Event::Notice {
                            message: format!("Audio output failed: {error}"),
                        });
                    }
                    _ => {}
                }
            }
        }
    }

    /// googlevideo refused the URL: resolve once more and resume in place.
    fn reresolve(self: &Arc<Self>, st: &mut State, loaded: Loaded) {
        let Some(entry) = st.queue.current().cloned().filter(|e| e.uid == loaded.uid) else {
            return;
        };
        tracing::info!(
            video_id = entry.track.video_id,
            "stream url expired, resolving again"
        );
        self.resolver.invalidate(&entry.track.video_id);
        st.generation += 1;
        st.loaded = None;
        st.preload = None;
        st.status = Status::Loading;
        self.emit_player(st);
        let (generation, start_ms) = (st.generation, st.position_ms);
        let this = self.clone();
        self.rt.spawn(async move {
            let prepared = this.prepare(&entry.track.video_id).await;
            this.on_prepared(generation, entry.uid, prepared, start_ms, true);
        });
    }

    /// Reporting, the Related tab, resolving ahead and radio, once a track is audible.
    fn track_started(self: &Arc<Self>, st: &mut State) {
        let (Some(loaded), Some(entry)) = (st.loaded, st.queue.current().cloned()) else {
            return;
        };
        self.persist.notify_one();
        self.resolve_ahead(st);
        self.extend_radio(st);
        if loaded.resumed {
            return;
        }
        let video_id = entry.track.video_id;
        let report = self.config.report_history && self.session.info().signed_in;
        let this = self.clone();
        self.rt.spawn(async move {
            let client = this.session.client();
            let (next, tracking) = tokio::join!(client.next(Some(&video_id), None), async {
                if report {
                    this.playback_tracking(&video_id).await
                } else {
                    None
                }
            });
            let mut st = this.state.lock();
            if !st.loaded.is_some_and(|l| l.id == loaded.id) {
                return;
            }
            if let Ok(next) = next {
                st.related_browse_id = next.related_browse_id;
                this.emit_player(&st);
            }
            if let Some(tracking) = tracking {
                let watch = Watch::new(tracking, 0);
                this.send_report(watch.playback_url());
                st.watch = Some(watch);
            }
        });
    }

    fn finish_watch(&self, st: &mut State) {
        if let Some(url) = st.watch.take().and_then(Watch::finish) {
            self.send_report(url);
        }
    }

    fn send_report(&self, url: String) {
        let Some(cookies) = self.session.cookies() else {
            return;
        };
        let page_id = self.session.page_id();
        let reporter = self.reporter.clone();
        self.rt
            .spawn(async move { reporter.ping(&url, &cookies, page_id.as_deref()).await });
    }

    // Persistence

    fn saved(&self) -> Saved {
        let st = self.state.lock();
        Saved {
            queue: st.queue.clone(),
            source_title: st.source_title.clone(),
            radio: st.radio.is_some(),
            position_ms: st.position_ms,
            volume: st.volume,
            muted: st.muted,
        }
    }

    fn write(&self, saved: &Saved) -> std::io::Result<()> {
        write_private(&self.queue_path, &serde_json::to_vec(saved)?, 0o600)
    }

    async fn persist_loop(self: Arc<Self>) {
        loop {
            self.persist.notified().await;
            tokio::time::sleep(PERSIST_DEBOUNCE).await;
            let saved = self.saved();
            let this = self.clone();
            let result = tokio::task::spawn_blocking(move || this.write(&saved)).await;
            if let Ok(Err(e)) = result {
                tracing::warn!("saving the queue: {e}");
            }
        }
    }
}

#[cfg(target_os = "linux")]
impl Playback {
    /// The current entry's uid, which MPRIS uses as the track id.
    pub fn current_uid(&self) -> Option<u64> {
        self.state.lock().queue.current().map(|e| e.uid)
    }

    pub fn can_go_next(&self) -> bool {
        let st = self.state.lock();
        st.queue.next_index(false).is_some() || st.radio.is_some()
    }

    pub fn subscribe_seeks(&self) -> broadcast::Receiver<u64> {
        self.seeked.subscribe()
    }

    /// The position now, run on from the engine's last report while playing,
    /// since clients only get an event a second.
    pub fn live_position(&self) -> u64 {
        let st = self.state.lock();
        let ran = match (st.status, st.position_at) {
            (Status::Playing, Some(at)) => at.elapsed().as_millis() as u64,
            _ => 0,
        };
        let position = st.position_ms + ran;
        st.duration_ms.map_or(position, |d| position.min(d))
    }
}

fn player_state(st: &State) -> PlayerState {
    PlayerState {
        status: st.status,
        track: st.queue.current().map(|e| e.track.clone()),
        position_ms: st.position_ms,
        duration_ms: st.duration_ms,
        volume: st.volume,
        muted: st.muted,
        repeat: st.queue.repeat,
        shuffle: st.queue.shuffled(),
        stream: st.stream.clone(),
        related_browse_id: st.related_browse_id.clone(),
    }
}

fn queue_state(st: &State) -> QueueState {
    QueueState {
        tracks: st.queue.tracks(),
        current: st.queue.current_index(),
        source_title: st.source_title.clone(),
        radio: st.radio.is_some(),
    }
}

fn out_of_range(index: usize) -> ApiError {
    ApiError::BadRequest(format!("no queue entry at {index}"))
}

/// A playlist or album in full. Album playlists (`OLAK5uy_...`) have no
/// browse page of their own, so those come from the player's queue panel.
async fn fetch_playlist(
    client: &Client,
    playlist_id: &str,
) -> Result<(Option<String>, Vec<Track>), ApiError> {
    match browse_playlist(client, playlist_id).await {
        Ok(found) if !found.1.is_empty() => Ok(found),
        Ok(_) | Err(ApiError::Parse(_) | ApiError::NotFound(_)) => {
            queue_playlist(client, playlist_id).await
        }
        Err(e) => Err(e),
    }
}

/// The playlist page's track list, following its continuations.
async fn browse_playlist(
    client: &Client,
    playlist_id: &str,
) -> Result<(Option<String>, Vec<Track>), ApiError> {
    let page = client
        .browse(formalmusic_api::BrowseTarget::Playlist(
            playlist_id.to_owned(),
        ))
        .await?;
    let title = match &page.header {
        Some(formalmusic_api::Header::Detail { title, .. }) => Some(title.clone()),
        _ => None,
    };
    let Some(list) = page.sections.into_iter().next() else {
        return Ok((title, Vec::new()));
    };
    let mut tracks: Vec<Track> = tracks_of(list.items);
    let mut token = list.continuation;
    while let Some(next) = token {
        let more = client.continuation(&next).await?;
        let before = tracks.len();
        tracks.extend(tracks_of(more.items));
        if tracks.len() == before {
            break;
        }
        token = more.continuation;
    }
    Ok((title, tracks))
}

/// The up-next panel the web app shows for a playlist, which also covers
/// albums. Titled after the album when the tracks share one.
async fn queue_playlist(
    client: &Client,
    playlist_id: &str,
) -> Result<(Option<String>, Vec<Track>), ApiError> {
    let first = client.next(None, Some(playlist_id)).await?;
    let panel_id = first.playlist_id.unwrap_or_else(|| playlist_id.to_owned());
    let mut tracks = first.tracks;
    let mut token = first.continuation;
    while let Some(next) = token {
        let more = client.next_continuation(&panel_id, &next).await?;
        let fresh: Vec<Track> = more
            .tracks
            .into_iter()
            .filter(|t| !tracks.iter().any(|known| known.video_id == t.video_id))
            .collect();
        if fresh.is_empty() {
            break;
        }
        tracks.extend(fresh);
        token = more.continuation;
    }
    let album = tracks.first().and_then(|t| t.album.as_ref()).map(|a| a.text.clone());
    let title = album.filter(|album| {
        tracks
            .iter()
            .all(|t| t.album.as_ref().is_some_and(|a| &a.text == album))
    });
    Ok((title, tracks))
}

fn tracks_of(items: Vec<Item>) -> Vec<Track> {
    items
        .into_iter()
        .filter_map(|item| match item {
            Item::Track(track) => Some(track),
            _ => None,
        })
        .collect()
}
