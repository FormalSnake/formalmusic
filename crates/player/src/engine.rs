//! The decode thread: owns the decoders and the ring producer, runs the
//! commands, and turns callback progress into events.

use std::collections::VecDeque;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use formalmusic_api::Status;
use rtrb::Producer;
use tokio::sync::broadcast;

use crate::decode::TrackDecoder;
use crate::error::PlayerError;
use crate::gain::{Ramp, crossfade_weights, normalisation_gain};
use crate::output::{Output, OutputKind};
use crate::source::StreamSource;
use crate::{PlayerEvent, TrackId};

const POSITION_INTERVAL: Duration = Duration::from_millis(250);
/// How long a pause fade gets before the device is suspended.
const SUSPEND_AFTER: Duration = Duration::from_millis(100);
const STARVED_POLL: Duration = Duration::from_millis(50);
const NEEDS_NEXT_MS: u64 = 15_000;
const MIN_WRITE_FRAMES: usize = 256;
const MAX_WRITE_FRAMES: usize = 4096;
const NORMALISATION_RAMP_SECONDS: f32 = 0.05;
/// Overlap when the playing track hands over to another recording of the
/// same music. Masters differ in level and phase; a short equal-power fade
/// hides the seam without the two being heard as an echo.
const SWITCH_FADE_MS: u64 = 120;

pub(crate) enum Command {
    Load {
        id: TrackId,
        source: StreamSource,
        start_ms: u64,
        loudness_db: Option<f32>,
    },
    PreloadNext {
        id: TrackId,
        source: StreamSource,
        loudness_db: Option<f32>,
    },
    Switch {
        id: TrackId,
        source: StreamSource,
        start_ms: u64,
        offset_ms: i64,
        loudness_db: Option<f32>,
    },
    Opened {
        id: TrackId,
        result: Result<Box<TrackDecoder>, PlayerError>,
    },
    Play,
    Pause,
    Seek(u64),
    Stop,
    SetVolume(f32),
    SetMuted(bool),
    SetNormalisation(bool),
    SetCrossfade(u32),
    SetEqualizer(Option<[f32; 10]>),
    OutputError(String),
    Shutdown,
}

struct Pending {
    id: TrackId,
    start_ms: u64,
    loudness_db: Option<f32>,
}

struct Active {
    id: TrackId,
    decoder: Box<TrackDecoder>,
    loudness_db: Option<f32>,
    gain: Ramp,
    fifo: Vec<f32>,
    /// Track position, in output frames, of the next frame to be queued.
    cursor: u64,
    eof: bool,
    needs_next_sent: bool,
}

impl Active {
    fn frames(&self, channels: usize) -> usize {
        self.fifo.len() / channels
    }

    fn remaining(&self, rate: u32) -> Option<u64> {
        let total = self.decoder.duration_ms()? * rate as u64 / 1000;
        Some(total.saturating_sub(self.cursor))
    }

    /// Decodes until `frames` are queued or the track ends. False when the
    /// network has not caught up yet.
    fn fill(&mut self, frames: usize, channels: usize) -> Result<bool, PlayerError> {
        while self.frames(channels) < frames && !self.eof {
            if !self.decoder.is_ready() {
                return Ok(false);
            }
            if !self.decoder.decode_next(&mut self.fifo)? {
                self.eof = true;
            }
        }
        Ok(true)
    }

    fn consume(&mut self, frames: usize, channels: usize) {
        self.fifo.drain(..frames * channels);
        self.cursor += frames as u64;
    }
}

/// Another recording of the current track, waiting to take over at the
/// sample where `offset` lines the two up.
struct Incoming {
    /// The track it replaces; dropped if that one ends first.
    of: TrackId,
    track: Active,
    /// Frames to add to a position in the current track to get the same
    /// moment in this one.
    offset: i64,
}

enum Step {
    Idle,
    /// Push no more than this many frames of the current track.
    Ahead(usize),
    Mixed,
    Starved,
}

enum Align {
    /// Both tracks sit at the same moment and the incoming one has the
    /// whole fade decoded.
    Ready,
    /// The incoming track starts this many frames ahead of the current one.
    Ahead(usize),
    /// Still downloading or decoding up to the current moment.
    NotYet,
}

/// Brings `incoming` to the moment `current` is about to push: drops what
/// lies before it, or seeks back when the current track moved behind it.
fn align(
    current: &Active,
    incoming: &mut Incoming,
    fade: usize,
    channels: usize,
    rate: u32,
) -> Result<Align, PlayerError> {
    let target = current.cursor as i64 + incoming.offset;
    let track = &mut incoming.track;
    if track.cursor as i64 > target + rate as i64 {
        let ms = target.max(0) as u64 * 1000 / rate as u64;
        let reached = track.decoder.seek(ms)?;
        track.fifo.clear();
        track.eof = false;
        track.cursor = reached * rate as u64 / 1000;
    }
    if track.cursor as i64 > target {
        return Ok(Align::Ahead((track.cursor as i64 - target) as usize));
    }
    loop {
        let behind = (target - track.cursor as i64) as usize;
        let frames = track.frames(channels);
        if behind > 0 && frames > 0 {
            track.consume(behind.min(frames), channels);
            continue;
        }
        if behind == 0 && frames >= fade {
            return Ok(Align::Ready);
        }
        if track.eof {
            return Err(PlayerError::Decode(
                "the other version ends before this point".into(),
            ));
        }
        let goal = if behind > 0 {
            behind.min(MAX_WRITE_FRAMES)
        } else {
            fade
        };
        if !track.fill(goal, channels)? {
            return Ok(Align::NotYet);
        }
    }
}

/// Where a track begins inside the current ring, so callback progress maps
/// back to a track and a position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Segment {
    pub id: TrackId,
    pub duration_ms: Option<u64>,
    pub ring_frame: u64,
    pub track_frame: u64,
    /// Takes over from the segment before it in a switch, so that one does
    /// not end.
    pub continues: bool,
}

/// The segment playing after `played` ring frames, and its position in ms.
pub(crate) fn locate(segments: &VecDeque<Segment>, played: u64, rate: u32) -> Option<(usize, u64)> {
    let index = segments.iter().rposition(|s| s.ring_frame <= played)?;
    let seg = segments[index];
    let frame = seg.track_frame + (played - seg.ring_frame);
    Some((index, frame * 1000 / rate as u64))
}

pub(crate) struct Engine {
    commands: Receiver<Command>,
    internal: Sender<Command>,
    events: broadcast::Sender<PlayerEvent>,
    output_kind: OutputKind,
    client: Option<reqwest::blocking::Client>,
    output: Option<Output>,
    producer: Option<Producer<f32>>,
    current: Option<Active>,
    next: Option<Active>,
    pending: Option<Pending>,
    pending_next: Option<Pending>,
    pending_switch: Option<(Pending, i64, TrackId)>,
    incoming: Option<Incoming>,
    /// Ring frame where a running switch fade ends.
    switch_end: Option<u64>,
    segments: VecDeque<Segment>,
    pushed: u64,
    /// Ring frame where the running crossfade ends.
    fade_end: Option<u64>,
    fade_len: u64,
    status: Status,
    want_play: bool,
    draining: bool,
    starved: bool,
    suspend_at: Option<Instant>,
    last_position: Instant,
    volume: f32,
    muted: bool,
    normalise: bool,
    crossfade_ms: u32,
    equalizer: Option<[f32; 10]>,
}

impl Engine {
    pub fn spawn(
        output_kind: OutputKind,
        commands: Receiver<Command>,
        internal: Sender<Command>,
        events: broadcast::Sender<PlayerEvent>,
    ) -> std::io::Result<()> {
        let engine = Self {
            commands,
            internal,
            events,
            output_kind,
            client: None,
            output: None,
            producer: None,
            current: None,
            next: None,
            pending: None,
            pending_next: None,
            pending_switch: None,
            incoming: None,
            switch_end: None,
            segments: VecDeque::new(),
            pushed: 0,
            fade_end: None,
            fade_len: 0,
            status: Status::Stopped,
            want_play: false,
            draining: false,
            starved: false,
            suspend_at: None,
            last_position: Instant::now(),
            volume: 1.0,
            muted: false,
            normalise: true,
            crossfade_ms: 0,
            equalizer: None,
        };
        thread::Builder::new()
            .name("formalmusic-engine".into())
            .spawn(move || engine.run())?;
        Ok(())
    }

    fn run(mut self) {
        loop {
            let command = match self.wake_in() {
                None => self
                    .commands
                    .recv()
                    .map_err(|_| RecvTimeoutError::Disconnected),
                Some(timeout) => self.commands.recv_timeout(timeout),
            };
            match command {
                Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(command) => {
                    self.handle(command);
                    while let Ok(command) = self.commands.try_recv() {
                        if matches!(command, Command::Shutdown) {
                            return;
                        }
                        self.handle(command);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.pump();
            self.report();
            if self.suspend_at.is_some_and(|at| Instant::now() >= at) {
                self.suspend_at = None;
                if let Some(output) = &mut self.output {
                    output.suspend();
                }
            }
        }
    }

    /// How long to sleep before the next fill or report. `None` sleeps until
    /// a command arrives, which is what keeps a paused player at zero CPU.
    fn wake_in(&self) -> Option<Duration> {
        let mut wake: Option<Duration> = None;
        if self.want_play && (self.current.is_some() || self.draining) {
            let since = self.last_position.elapsed();
            wake = Some(
                POSITION_INTERVAL
                    .saturating_sub(since)
                    .max(Duration::from_millis(5)),
            );
            if self.starved {
                wake = wake.map(|w| w.min(STARVED_POLL));
            }
        }
        if let Some(at) = self.suspend_at {
            let until = at.saturating_duration_since(Instant::now());
            wake = Some(wake.map_or(until, |w| w.min(until)));
        }
        wake
    }

    fn emit(&self, event: PlayerEvent) {
        let _ = self.events.send(event);
    }

    fn set_status(&mut self, status: Status) {
        if self.status != status {
            self.status = status;
            self.emit(PlayerEvent::StateChanged(status));
        }
    }

    fn handle(&mut self, command: Command) {
        match command {
            Command::Load {
                id,
                source,
                start_ms,
                loudness_db,
            } => self.load(id, source, start_ms, loudness_db),
            Command::PreloadNext {
                id,
                source,
                loudness_db,
            } => {
                self.next = None;
                if self.ensure_output().is_ok() {
                    self.pending_next = Some(Pending {
                        id,
                        start_ms: 0,
                        loudness_db,
                    });
                    self.open(id, source, 0);
                }
            }
            Command::Switch {
                id,
                source,
                start_ms,
                offset_ms,
                loudness_db,
            } => {
                self.incoming = None;
                self.switch_end = None;
                let of = self.current.as_ref().map(|c| c.id);
                if let Some(of) = of.or(self.pending.as_ref().map(|p| p.id)) {
                    self.pending_switch = Some((
                        Pending {
                            id,
                            start_ms,
                            loudness_db,
                        },
                        offset_ms,
                        of,
                    ));
                    self.open(id, source, start_ms);
                }
            }
            Command::Opened { id, result } => self.opened(id, result),
            Command::Play => {
                self.want_play = true;
                if self.current.is_some() {
                    self.resume_output();
                    self.set_status(Status::Playing);
                } else if self.pending.is_some() {
                    self.set_status(Status::Loading);
                }
            }
            Command::Pause => {
                self.want_play = false;
                if let Some(output) = &self.output {
                    output.pause();
                }
                self.suspend_at = Some(Instant::now() + SUSPEND_AFTER);
                if self.current.is_some() || self.pending.is_some() {
                    self.set_status(Status::Paused);
                }
            }
            Command::Seek(ms) => self.seek(ms),
            Command::Stop => self.stop(),
            Command::SetVolume(volume) => {
                self.volume = volume;
                if let Some(output) = &self.output {
                    output.set_volume(volume);
                }
            }
            Command::SetMuted(muted) => {
                self.muted = muted;
                if let Some(output) = &self.output {
                    output.set_muted(muted);
                }
            }
            Command::SetNormalisation(on) => {
                self.normalise = on;
                for track in self.current.iter_mut().chain(self.next.iter_mut()) {
                    track.gain.set_target(track_gain(on, track.loudness_db));
                }
            }
            Command::SetCrossfade(ms) => self.crossfade_ms = ms,
            Command::SetEqualizer(gains) => {
                self.equalizer = gains;
                if let Some(output) = &self.output {
                    output.set_equalizer(gains);
                }
            }
            Command::OutputError(message) => {
                tracing::warn!(%message, "audio output error");
                let track = self.current.as_ref().map(|t| t.id);
                self.emit(PlayerEvent::Error {
                    track,
                    error: PlayerError::Output(message),
                });
            }
            Command::Shutdown => {}
        }
    }

    fn ensure_output(&mut self) -> Result<(), PlayerError> {
        if self.output.is_some() {
            return Ok(());
        }
        let result = crate::fetch::http_client().and_then(|client| {
            let output = Output::open(self.output_kind, self.internal.clone())?;
            output.set_volume(self.volume);
            output.set_muted(self.muted);
            output.set_equalizer(self.equalizer);
            Ok((client, output))
        });
        match result {
            Ok((client, output)) => {
                self.client = Some(client);
                self.output = Some(output);
                Ok(())
            }
            Err(error) => {
                self.emit(PlayerEvent::Error {
                    track: None,
                    error: error.clone(),
                });
                Err(error)
            }
        }
    }

    fn open(&self, id: TrackId, source: StreamSource, start_ms: u64) {
        let (Some(client), Some(output)) = (self.client.clone(), self.output.as_ref()) else {
            return;
        };
        let (rate, channels) = (output.rate, output.channels);
        let reply = self.internal.clone();
        let spawned = thread::Builder::new()
            .name("formalmusic-open".into())
            .spawn(move || {
                let result =
                    TrackDecoder::open(&client, &source, rate, channels).and_then(|mut decoder| {
                        if start_ms > 0 {
                            decoder.seek(start_ms)?;
                        }
                        Ok(Box::new(decoder))
                    });
                let _ = reply.send(Command::Opened { id, result });
            });
        if let Err(e) = spawned {
            self.emit(PlayerEvent::Error {
                track: Some(id),
                error: PlayerError::Decode(e.to_string()),
            });
        }
    }

    fn load(&mut self, id: TrackId, source: StreamSource, start_ms: u64, loudness_db: Option<f32>) {
        self.clear();
        self.want_play = true;
        if self.ensure_output().is_err() {
            self.set_status(Status::Stopped);
            return;
        }
        self.pending = Some(Pending {
            id,
            start_ms,
            loudness_db,
        });
        self.set_status(Status::Loading);
        self.open(id, source, start_ms);
    }

    fn opened(&mut self, id: TrackId, result: Result<Box<TrackDecoder>, PlayerError>) {
        let is_current = self.pending.as_ref().is_some_and(|p| p.id == id);
        let is_next = self.pending_next.as_ref().is_some_and(|p| p.id == id);
        let is_switch = self
            .pending_switch
            .as_ref()
            .is_some_and(|(p, _, _)| p.id == id);
        if !is_current && !is_next && !is_switch {
            return;
        }
        let mut switch = (0, id);
        let pending = if is_current {
            self.pending.take()
        } else if is_next {
            self.pending_next.take()
        } else {
            self.pending_switch.take().map(|(pending, offset, of)| {
                switch = (offset, of);
                pending
            })
        };
        let Some(pending) = pending else { return };
        let decoder = match result {
            Ok(decoder) => decoder,
            Err(error) => {
                tracing::warn!(%error, ?id, "could not open track");
                self.emit(PlayerEvent::Error {
                    track: Some(id),
                    error,
                });
                if is_current {
                    self.set_status(Status::Stopped);
                }
                return;
            }
        };
        let rate = self.output.as_ref().map_or(48_000, |o| o.rate);
        let gain = track_gain(self.normalise, pending.loudness_db);
        let duration_ms = decoder.duration_ms();
        let active = Active {
            id,
            decoder,
            loudness_db: pending.loudness_db,
            gain: Ramp::new(gain, (rate as f32 * NORMALISATION_RAMP_SECONDS) as u32),
            fifo: Vec::new(),
            cursor: pending.start_ms * rate as u64 / 1000,
            eof: false,
            needs_next_sent: false,
        };
        if is_next {
            self.next = Some(active);
            return;
        }
        if is_switch {
            self.incoming = Some(Incoming {
                of: switch.1,
                track: active,
                offset: switch.0 * rate as i64 / 1000,
            });
            return;
        }
        let track_frame = active.cursor;
        self.current = Some(active);
        self.reset_ring(id, duration_ms, track_frame);
        self.emit(PlayerEvent::TrackStarted {
            track: id,
            duration_ms,
        });
        self.emit_position();
        // Stays Loading until `report` sees audio queued.
        if self.want_play {
            self.resume_output();
        } else {
            self.set_status(Status::Paused);
        }
    }

    fn resume_output(&mut self) {
        self.suspend_at = None;
        if let Some(output) = &mut self.output
            && let Err(error) = output.play()
        {
            self.emit(PlayerEvent::Error {
                track: self.current.as_ref().map(|t| t.id),
                error,
            });
        }
    }

    /// Drops everything queued and starts a fresh ring at `track_frame`.
    fn reset_ring(&mut self, id: TrackId, duration_ms: Option<u64>, track_frame: u64) {
        self.producer = self.output.as_mut().map(Output::new_ring);
        self.segments.clear();
        self.segments.push_back(Segment {
            id,
            duration_ms,
            ring_frame: 0,
            track_frame,
            continues: false,
        });
        self.pushed = 0;
        self.fade_end = None;
        self.switch_end = None;
        self.draining = false;
        self.starved = false;
    }

    fn clear(&mut self) {
        self.current = None;
        self.next = None;
        self.pending = None;
        self.pending_next = None;
        self.pending_switch = None;
        self.incoming = None;
        self.switch_end = None;
        self.segments.clear();
        self.fade_end = None;
        self.draining = false;
        self.starved = false;
        if let Some(output) = &mut self.output {
            self.producer = Some(output.new_ring());
        }
    }

    fn stop(&mut self) {
        self.clear();
        self.want_play = false;
        if let Some(output) = &self.output {
            output.pause();
        }
        self.suspend_at = Some(Instant::now() + SUSPEND_AFTER);
        self.set_status(Status::Stopped);
    }

    fn seek(&mut self, ms: u64) {
        if let Some(pending) = &mut self.pending {
            pending.start_ms = ms;
            return;
        }
        // A switch fading across starts over from the new place.
        self.switch_end = None;
        if let Some((pending, offset, _)) = &mut self.pending_switch {
            pending.start_ms = (ms as i64 + *offset).max(0) as u64;
        }
        if self.current.is_none() {
            return;
        }
        let rate = self.output.as_ref().map_or(48_000, |o| o.rate);
        if let Some(incoming) = &mut self.incoming {
            let at = ms as i64 + incoming.offset * 1000 / rate as i64;
            let track = &mut incoming.track;
            track.fifo.clear();
            track.eof = false;
            match track.decoder.seek(at.max(0) as u64) {
                Ok(reached) => track.cursor = reached * rate as u64 / 1000,
                Err(error) => {
                    let id = track.id;
                    self.incoming = None;
                    self.emit(PlayerEvent::Error {
                        track: Some(id),
                        error,
                    });
                }
            }
        }
        let Some(current) = &mut self.current else {
            return;
        };
        let result = current.decoder.seek(ms);
        current.fifo.clear();
        current.eof = false;
        let reached = match result {
            Ok(reached) => reached,
            Err(error) => {
                let id = current.id;
                self.fail_current(id, error);
                return;
            }
        };
        current.cursor = reached * rate as u64 / 1000;
        let (id, duration_ms, cursor) = (current.id, current.decoder.duration_ms(), current.cursor);
        // A seek during a crossfade restarts the incoming track from the top.
        if self.fade_end.is_some()
            && let Some(next) = &mut self.next
        {
            next.fifo.clear();
            next.eof = false;
            next.cursor = 0;
            if let Err(error) = next.decoder.seek(0) {
                let next_id = next.id;
                self.next = None;
                self.emit(PlayerEvent::Error {
                    track: Some(next_id),
                    error,
                });
            }
        }
        self.reset_ring(id, duration_ms, cursor);
        self.emit_position();
    }

    fn fail_current(&mut self, id: TrackId, error: PlayerError) {
        tracing::warn!(%error, ?id, "playback failed");
        self.emit(PlayerEvent::Error {
            track: Some(id),
            error,
        });
        self.clear();
        self.set_status(Status::Stopped);
    }

    fn crossfade_frames(&self, rate: u32) -> u64 {
        self.crossfade_ms as u64 * rate as u64 / 1000
    }

    /// Tops up the ring from the current track, switching to the preloaded
    /// one gaplessly or through a crossfade.
    fn pump(&mut self) {
        if !self.want_play || self.current.is_none() {
            return;
        }
        let Some(output) = &self.output else { return };
        let (rate, channels) = (output.rate, output.channels);
        let xfade = self.crossfade_frames(rate);
        self.starved = false;
        loop {
            let Some(producer) = &mut self.producer else {
                return;
            };
            let free = producer.slots() / channels;
            if free < MIN_WRITE_FRAMES {
                return;
            }
            let want = free.min(MAX_WRITE_FRAMES);
            let Some(current) = &mut self.current else {
                return;
            };
            match current.fill(want, channels) {
                Ok(true) => {}
                Ok(false) => {
                    self.starved = true;
                    return;
                }
                Err(error) => {
                    let id = current.id;
                    self.fail_current(id, error);
                    return;
                }
            }

            let mut limit = usize::MAX;
            if self.fade_end.is_none() {
                match self.switch(want, channels, rate) {
                    Step::Mixed => continue,
                    Step::Starved => {
                        self.starved = true;
                        return;
                    }
                    Step::Ahead(frames) => limit = frames,
                    Step::Idle => {}
                }
            }
            let (Some(producer), Some(current)) = (&mut self.producer, &mut self.current) else {
                return;
            };
            let fading = self.fade_end.is_some()
                || (xfade > 0
                    && self.next.is_some()
                    && current.remaining(rate).is_some_and(|r| r <= xfade));
            if fading && current.frames(channels) > 0 {
                let Some(next) = &mut self.next else {
                    self.fade_end = None;
                    continue;
                };
                match next.fill(want, channels) {
                    Ok(true) => {}
                    Ok(false) => {
                        self.starved = true;
                        return;
                    }
                    Err(error) => {
                        let id = next.id;
                        self.next = None;
                        self.fade_end = None;
                        if self.segments.len() > 1
                            && self.segments.back().is_some_and(|s| s.id == id)
                        {
                            self.segments.pop_back();
                        }
                        self.emit(PlayerEvent::Error {
                            track: Some(id),
                            error,
                        });
                        continue;
                    }
                }
                if next.frames(channels) == 0 {
                    // The incoming track is shorter than the fade.
                    self.fade_end = None;
                    self.current = self.next.take();
                    continue;
                }
                let fade_end = match self.fade_end {
                    Some(end) => end,
                    None => {
                        self.fade_len = current.remaining(rate).unwrap_or(0).max(1);
                        self.segments.push_back(Segment {
                            id: next.id,
                            duration_ms: next.decoder.duration_ms(),
                            ring_frame: self.pushed,
                            track_frame: next.cursor,
                            continues: false,
                        });
                        *self.fade_end.insert(self.pushed + self.fade_len)
                    }
                };
                let n = want
                    .min(current.frames(channels))
                    .min(next.frames(channels))
                    .min(fade_end.saturating_sub(self.pushed).max(1) as usize);
                let outgoing = &mut current.fifo[..n * channels];
                current.gain.apply(outgoing, channels);
                let incoming = &mut next.fifo[..n * channels];
                next.gain.apply(incoming, channels);
                for (i, (out, inc)) in outgoing
                    .chunks_exact_mut(channels)
                    .zip(incoming.chunks_exact(channels))
                    .enumerate()
                {
                    let left = fade_end.saturating_sub(self.pushed + i as u64);
                    let (w_out, w_in) = crossfade_weights(1.0 - left as f32 / self.fade_len as f32);
                    for (o, x) in out.iter_mut().zip(inc) {
                        *o = *o * w_out + x * w_in;
                    }
                }
                let _ = producer.push_partial_slice(outgoing);
                current.consume(n, channels);
                next.consume(n, channels);
                self.pushed += n as u64;
                if self.pushed >= fade_end {
                    self.fade_end = None;
                    self.current = self.next.take();
                }
                continue;
            }

            if current.frames(channels) == 0 && current.eof {
                if self.fade_end.take().is_some() || self.next.is_some() {
                    let Some(next) = self.next.take() else { return };
                    if self.segments.back().is_none_or(|s| s.id != next.id) {
                        self.segments.push_back(Segment {
                            id: next.id,
                            duration_ms: next.decoder.duration_ms(),
                            ring_frame: self.pushed,
                            track_frame: next.cursor,
                            continues: false,
                        });
                    }
                    self.current = Some(next);
                    continue;
                }
                if self.pending_next.is_some() {
                    self.starved = true;
                    return;
                }
                self.draining = true;
                self.current = None;
                return;
            }

            let n = want.min(current.frames(channels)).min(limit);
            let block = &mut current.fifo[..n * channels];
            current.gain.apply(block, channels);
            let _ = producer.push_partial_slice(block);
            current.consume(n, channels);
            self.pushed += n as u64;
        }
    }

    /// Runs a pending switch for one block: lines the incoming version up
    /// with the current one, then fades across and makes it current.
    fn switch(&mut self, want: usize, channels: usize, rate: u32) -> Step {
        let xfade = self.crossfade_frames(rate);
        let (Some(current), Some(incoming)) = (&mut self.current, &mut self.incoming) else {
            return Step::Idle;
        };
        if incoming.of != current.id {
            self.incoming = None;
            self.switch_end = None;
            return Step::Idle;
        }
        let fade = (SWITCH_FADE_MS * rate as u64 / 1000) as usize;
        let fade_end = match self.switch_end {
            Some(end) => end,
            None => {
                // Too close to the end to bother: the next track takes over.
                let ending = current.eof
                    || current
                        .remaining(rate)
                        .is_some_and(|r| r <= xfade + fade as u64);
                if ending {
                    return Step::Idle;
                }
                match align(current, incoming, fade, channels, rate) {
                    Ok(Align::Ready) => {}
                    Ok(Align::Ahead(frames)) => return Step::Ahead(frames),
                    Ok(Align::NotYet) => return Step::Idle,
                    Err(error) => {
                        let id = incoming.track.id;
                        self.incoming = None;
                        self.emit(PlayerEvent::Error {
                            track: Some(id),
                            error,
                        });
                        return Step::Idle;
                    }
                }
                self.segments.push_back(Segment {
                    id: incoming.track.id,
                    duration_ms: incoming.track.decoder.duration_ms(),
                    ring_frame: self.pushed,
                    track_frame: incoming.track.cursor,
                    continues: true,
                });
                *self.switch_end.insert(self.pushed + fade as u64)
            }
        };
        let Some(producer) = &mut self.producer else {
            return Step::Idle;
        };
        let incoming = &mut incoming.track;
        let n = want
            .min(current.frames(channels))
            .min(incoming.frames(channels))
            .min(fade_end.saturating_sub(self.pushed) as usize);
        if n == 0 && self.pushed < fade_end {
            return Step::Starved;
        }
        let outgoing = &mut current.fifo[..n * channels];
        current.gain.apply(outgoing, channels);
        let block = &mut incoming.fifo[..n * channels];
        incoming.gain.apply(block, channels);
        for (i, (out, inc)) in outgoing
            .chunks_exact_mut(channels)
            .zip(block.chunks_exact(channels))
            .enumerate()
        {
            let left = fade_end.saturating_sub(self.pushed + i as u64);
            let (w_out, w_in) = crossfade_weights(1.0 - left as f32 / fade as f32);
            for (o, x) in out.iter_mut().zip(inc) {
                *o = *o * w_out + x * w_in;
            }
        }
        let _ = producer.push_partial_slice(outgoing);
        current.consume(n, channels);
        incoming.consume(n, channels);
        self.pushed += n as u64;
        if self.pushed >= fade_end {
            self.switch_end = None;
            self.current = self.incoming.take().map(|incoming| incoming.track);
        }
        Step::Mixed
    }

    fn report(&mut self) {
        let Some(output) = &self.output else { return };
        let rate = output.rate;
        let played = output.played();

        while self.segments.len() > 1 && self.segments[1].ring_frame <= played {
            if let Some(ended) = self.segments.pop_front()
                && !self.segments.front().is_some_and(|s| s.continues)
            {
                self.emit(PlayerEvent::TrackEnded { track: ended.id });
            }
            if let Some(&Segment {
                id, duration_ms, ..
            }) = self.segments.front()
            {
                self.emit(PlayerEvent::TrackStarted {
                    track: id,
                    duration_ms,
                });
            }
            self.emit_position();
        }

        if self.draining && played >= self.pushed {
            self.draining = false;
            if let Some(ended) = self.segments.pop_front() {
                self.emit(PlayerEvent::TrackEnded { track: ended.id });
            }
            self.want_play = false;
            if let Some(output) = &self.output {
                output.pause();
            }
            self.suspend_at = Some(Instant::now() + SUSPEND_AFTER);
            self.set_status(Status::Stopped);
            return;
        }

        if self.want_play && !self.segments.is_empty() {
            let buffering = self.starved
                && self
                    .producer
                    .as_ref()
                    .is_some_and(|p| p.slots() == p.buffer().capacity());
            if buffering {
                self.set_status(Status::Loading);
            } else if self.status == Status::Loading && self.pending.is_none() {
                self.set_status(Status::Playing);
            }
        }

        if self.want_play && self.last_position.elapsed() >= POSITION_INTERVAL {
            self.emit_position();
        }

        let Some((index, position_ms)) = locate(&self.segments, played, rate) else {
            return;
        };
        let id = self.segments[index].id;
        let wants_next = self.next.is_none() && self.pending_next.is_none();
        if let Some(current) = self.current.as_mut().filter(|c| c.id == id)
            && wants_next
            && !current.needs_next_sent
            && current
                .decoder
                .duration_ms()
                .is_some_and(|d| position_ms + NEEDS_NEXT_MS >= d)
        {
            current.needs_next_sent = true;
            self.emit(PlayerEvent::NeedsNext { track: id });
        }
    }

    fn track(&self, id: TrackId) -> Option<&Active> {
        self.current
            .iter()
            .chain(self.next.iter())
            .find(|t| t.id == id)
    }

    fn emit_position(&mut self) {
        self.last_position = Instant::now();
        let Some(output) = &self.output else { return };
        let Some((index, position_ms)) = locate(&self.segments, output.played(), output.rate)
        else {
            return;
        };
        let Segment {
            id, duration_ms, ..
        } = self.segments[index];
        // A track no longer in `current` or `next` is fully decoded.
        let buffered_ms = match self.track(id) {
            Some(track) => track.decoder.buffered_ms(),
            None => duration_ms,
        };
        let buffered_ms = buffered_ms.unwrap_or(position_ms).max(position_ms);
        self.emit(PlayerEvent::Position {
            track: id,
            position_ms,
            buffered_ms,
        });
    }
}

fn track_gain(normalise: bool, loudness_db: Option<f32>) -> f32 {
    match loudness_db {
        Some(db) if normalise => normalisation_gain(db),
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_follows_segments() {
        let a = TrackId(1);
        let b = TrackId(2);
        // Track a was seeked to 60 s at 48 kHz; b starts 2 s into the ring.
        let segments = VecDeque::from([
            Segment {
                id: a,
                duration_ms: None,
                ring_frame: 0,
                track_frame: 60 * 48_000,
                continues: false,
            },
            Segment {
                id: b,
                duration_ms: None,
                ring_frame: 96_000,
                track_frame: 0,
                continues: false,
            },
        ]);
        assert_eq!(locate(&segments, 0, 48_000), Some((0, 60_000)));
        assert_eq!(locate(&segments, 24_000, 48_000), Some((0, 60_500)));
        assert_eq!(locate(&segments, 96_000, 48_000), Some((1, 0)));
        assert_eq!(locate(&segments, 96_000 + 4_800, 48_000), Some((1, 100)));
        assert_eq!(locate(&VecDeque::new(), 10, 48_000), None);
    }

    #[test]
    fn normalisation_toggle() {
        assert_eq!(track_gain(false, Some(6.0)), 1.0);
        assert_eq!(track_gain(true, None), 1.0);
        assert!((track_gain(true, Some(6.0)) - 0.501).abs() < 1e-3);
    }
}
