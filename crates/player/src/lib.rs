//! Streaming decode and audio output for `formalmusicd`.
//!
//! The daemon resolves a track with yt-dlp, turns the chosen format into a
//! [`StreamSource`] and hands it to a [`Player`]. The player downloads the
//! stream in 10 MB range requests (googlevideo throttles or rejects larger
//! ones), decodes on its own thread, and feeds a lock-free ring that the
//! audio callback drains.
//!
//! Codec choice: Opus first (itag 774 for Premium at ~256 kbps, 251 at
//! ~160 kbps), AAC (141, 140) when no Opus format is offered. YouTube's Opus
//! streams are the higher quality at equal bitrate and are what the web
//! player picks. Symphonia demuxes both containers and decodes AAC, but has
//! no Opus decoder, so Opus goes through `symphonia-adapter-libopus`, which
//! plugs the reference libopus into symphonia's codec registry. It is built
//! without the bundled feature and links the system `libopus` (nixpkgs
//! `libopus`), so Nix packaging needs only `libopus` and `alsa-lib`, with no
//! cmake build of a vendored copy. On a machine without libopus on the
//! linker path, point `OPUS_LIB_DIR` at it.
//!
//! Threads: one engine thread per [`Player`], one fetcher per open stream, a
//! short-lived opener per load, and the cpal callback. A paused player
//! suspends the output stream and blocks the engine on its command channel,
//! so it costs no CPU.

mod decode;
mod engine;
mod error;
mod fetch;
mod gain;
mod output;
mod source;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crossbeam_channel::Sender;
pub use formalmusic_api::Status;
use tokio::sync::broadcast;

use crate::engine::{Command, Engine};
pub use crate::error::PlayerError;
pub use crate::gain::{crossfade_weights, normalisation_gain};
pub use crate::output::OutputKind;
pub use crate::source::{Codec, StreamSource};

/// Identifies one `load` or `preload_next` call in events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrackId(pub u64);

#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    StateChanged(Status),
    /// A track became audible: after `load`, or at the exact sample a
    /// preloaded track took over.
    TrackStarted { track: TrackId, duration_ms: Option<u64> },
    /// About four times a second while playing, and after every seek.
    /// `buffered_ms` is how far into the track the download reaches.
    Position { track: TrackId, position_ms: u64, buffered_ms: u64 },
    /// The playing track is within 15 s of its end and nothing is preloaded.
    NeedsNext { track: TrackId },
    TrackEnded { track: TrackId },
    /// `track` is `None` for output failures not tied to a track. After
    /// [`PlayerError::Expired`] on the current track, resolve it again and
    /// `load` at the last reported position.
    Error { track: Option<TrackId>, error: PlayerError },
}

/// Handle to the audio engine. Cheap to clone; the engine thread stops when
/// the last clone is dropped.
#[derive(Clone)]
pub struct Player {
    inner: Arc<Inner>,
}

struct Inner {
    commands: Sender<Command>,
    events: broadcast::Sender<PlayerEvent>,
    next_id: AtomicU64,
}

impl Drop for Inner {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
    }
}

impl Player {
    /// A player on the default output device, opened on the first `load`.
    pub fn new() -> std::io::Result<Self> {
        Self::with_output(OutputKind::Default)
    }

    pub fn with_output(output: OutputKind) -> std::io::Result<Self> {
        let (commands, receiver) = crossbeam_channel::unbounded();
        let (events, _) = broadcast::channel(256);
        Engine::spawn(output, receiver, commands.clone(), events.clone())?;
        Ok(Self { inner: Arc::new(Inner { commands, events, next_id: AtomicU64::new(1) }) })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PlayerEvent> {
        self.inner.events.subscribe()
    }

    fn send(&self, command: Command) {
        let _ = self.inner.commands.send(command);
    }

    fn next_id(&self) -> TrackId {
        TrackId(self.inner.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Replaces whatever is playing and starts `source` at `start_ms`.
    /// `loudness_db` is the player response's `loudnessDb`.
    pub fn load(&self, source: StreamSource, start_ms: u64, loudness_db: Option<f32>) -> TrackId {
        let id = self.next_id();
        self.send(Command::Load { id, source, start_ms, loudness_db });
        id
    }

    /// Opens and buffers the track that follows the current one, so it starts
    /// on the sample after the current track's last (or crossfades into it).
    /// Replaces an earlier preload.
    pub fn preload_next(&self, source: StreamSource, loudness_db: Option<f32>) -> TrackId {
        let id = self.next_id();
        self.send(Command::PreloadNext { id, source, loudness_db });
        id
    }

    pub fn play(&self) {
        self.send(Command::Play);
    }

    pub fn pause(&self) {
        self.send(Command::Pause);
    }

    pub fn seek(&self, position_ms: u64) {
        self.send(Command::Seek(position_ms));
    }

    pub fn stop(&self) {
        self.send(Command::Stop);
    }

    /// Linear amplitude, `0.0..=1.0`. Changes ramp over 30 ms.
    pub fn set_volume(&self, volume: f32) {
        self.send(Command::SetVolume(volume));
    }

    pub fn set_muted(&self, muted: bool) {
        self.send(Command::SetMuted(muted));
    }

    /// Loudness normalisation from `loudnessDb`, on by default.
    pub fn set_normalisation(&self, enabled: bool) {
        self.send(Command::SetNormalisation(enabled));
    }

    /// Crossfade length into a preloaded track; 0 (the default) is gapless.
    pub fn set_crossfade(&self, ms: u32) {
        self.send(Command::SetCrossfade(ms));
    }
}
