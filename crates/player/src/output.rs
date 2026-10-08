//! The audio callback and the devices it runs on.
//!
//! The decode thread writes frames into an `rtrb` ring that the callback
//! drains. Flushing (seek, load, stop) never touches the live ring: the
//! decode thread builds a fresh one and leaves its consumer in a mailbox
//! that the callback swaps in with `try_lock`, handing the old consumer back
//! so it is freed off the audio thread. The callback never allocates, frees
//! or blocks.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use crossbeam_channel::Sender;
use rtrb::{Consumer, Producer, RingBuffer};

use crate::engine::Command;
use crate::equalizer::Equalizer;
use crate::error::PlayerError;
use crate::gain::Ramp;

/// Ring length. The decode thread tops it up every position tick (250 ms),
/// so this leaves 750 ms of slack for a slow packet or a busy laptop.
const RING_SECONDS: f32 = 1.0;
/// Volume, mute and pause ramps take this long across the full range.
const RAMP_SECONDS: f32 = 0.03;
const SCRATCH_FRAMES: usize = 2048;

/// Where audio goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputKind {
    /// The system's default output device through cpal.
    #[default]
    Default,
    /// Discards audio in real time on a timer thread. For tests and for
    /// hosts without a sound card.
    Null { sample_rate: u32, channels: u16 },
}

struct Swap {
    generation: u64,
    consumer: Consumer<f32>,
}

#[derive(Default)]
struct Mailbox {
    incoming: Option<Swap>,
    outgoing: Option<Consumer<f32>>,
}

struct Shared {
    mailbox: Mutex<Mailbox>,
    swap_pending: AtomicBool,
    /// Generation of the ring the callback is draining.
    generation: AtomicU64,
    /// Frames taken from that ring.
    played: AtomicU64,
    volume: AtomicU32,
    muted: AtomicBool,
    paused: AtomicBool,
    /// Equalizer gains in dB, picked up by the callback with `try_lock`
    /// while `eq_pending` is set.
    eq: Mutex<Option<[f32; 10]>>,
    eq_pending: AtomicBool,
}

struct Renderer {
    shared: Arc<Shared>,
    consumer: Option<Consumer<f32>>,
    channels: usize,
    gain: Ramp,
    eq: Equalizer,
}

impl Renderer {
    fn render(&mut self, out: &mut [f32]) {
        let shared = &*self.shared;
        if shared.swap_pending.load(Ordering::Acquire)
            && let Ok(mut mailbox) = shared.mailbox.try_lock()
            && let Some(swap) = mailbox.incoming.take()
        {
            mailbox.outgoing = self.consumer.replace(swap.consumer);
            shared.played.store(0, Ordering::Relaxed);
            shared.generation.store(swap.generation, Ordering::Release);
            shared.swap_pending.store(false, Ordering::Release);
        }
        if shared.eq_pending.load(Ordering::Acquire)
            && let Ok(gains) = shared.eq.try_lock()
        {
            self.eq.set(*gains);
            shared.eq_pending.store(false, Ordering::Release);
        }

        let paused = shared.paused.load(Ordering::Relaxed);
        let target = if paused || shared.muted.load(Ordering::Relaxed) {
            0.0
        } else {
            f32::from_bits(shared.volume.load(Ordering::Relaxed)) * self.eq.preamp()
        };
        self.gain.set_target(target);
        // Hold the ring once a pause has faded out, so position freezes on
        // the last audible frame.
        if paused && self.gain.current() == 0.0 {
            out.fill(0.0);
            return;
        }

        let filled = match &mut self.consumer {
            Some(consumer) => consumer.pop_partial_slice(out).0.len(),
            None => 0,
        };
        out[filled..].fill(0.0);
        self.eq.process(out);
        self.gain.apply(out, self.channels);
        // The preamp covers one band's boost; neighbouring boosts can still
        // add up past full scale.
        if self.eq.is_active() {
            out.iter_mut().for_each(|s| *s = s.clamp(-1.0, 1.0));
        }
        shared
            .played
            .fetch_add((filled / self.channels) as u64, Ordering::Release);
    }
}

enum Backend {
    Cpal(cpal::Stream),
    Null(NullSink),
}

pub(crate) struct Output {
    backend: Backend,
    shared: Arc<Shared>,
    pub rate: u32,
    pub channels: usize,
    generation: u64,
    running: bool,
}

impl Output {
    pub fn open(kind: OutputKind, errors: Sender<Command>) -> Result<Self, PlayerError> {
        let shared = Arc::new(Shared {
            mailbox: Mutex::new(Mailbox::default()),
            swap_pending: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            played: AtomicU64::new(0),
            volume: AtomicU32::new(1f32.to_bits()),
            muted: AtomicBool::new(false),
            paused: AtomicBool::new(true),
            eq: Mutex::new(None),
            eq_pending: AtomicBool::new(false),
        });
        let (backend, rate, channels) = match kind {
            OutputKind::Default => open_cpal(&shared, errors)?,
            OutputKind::Null {
                sample_rate,
                channels,
            } => {
                let renderer = renderer(&shared, sample_rate, channels as usize);
                (
                    Backend::Null(NullSink::spawn(renderer, sample_rate, channels as usize)),
                    sample_rate,
                    channels as usize,
                )
            }
        };
        Ok(Self {
            backend,
            shared,
            rate,
            channels,
            generation: 0,
            running: false,
        })
    }

    /// Starts a new, empty ring and returns its producer. Whatever was queued
    /// in the old one is discarded once the callback picks this one up.
    pub fn new_ring(&mut self) -> Producer<f32> {
        self.generation += 1;
        let capacity = (self.rate as f32 * RING_SECONDS) as usize * self.channels;
        let (producer, consumer) = RingBuffer::new(capacity);
        let mut mailbox = self
            .shared
            .mailbox
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        mailbox.outgoing = None;
        mailbox.incoming = Some(Swap {
            generation: self.generation,
            consumer,
        });
        self.shared.swap_pending.store(true, Ordering::Release);
        producer
    }

    /// Frames played from the current ring; zero until the callback has
    /// switched to it.
    pub fn played(&self) -> u64 {
        if self.shared.generation.load(Ordering::Acquire) == self.generation {
            self.shared.played.load(Ordering::Acquire)
        } else {
            0
        }
    }

    pub fn set_volume(&self, volume: f32) {
        self.shared
            .volume
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn set_muted(&self, muted: bool) {
        self.shared.muted.store(muted, Ordering::Relaxed);
    }

    pub fn set_equalizer(&self, gains: Option<[f32; 10]>) {
        *self.shared.eq.lock().unwrap_or_else(|e| e.into_inner()) = gains;
        self.shared.eq_pending.store(true, Ordering::Release);
    }

    /// Fades in and keeps the device running.
    pub fn play(&mut self) -> Result<(), PlayerError> {
        self.shared.paused.store(false, Ordering::Relaxed);
        if !self.running {
            match &self.backend {
                Backend::Cpal(stream) => stream
                    .play()
                    .map_err(|e| PlayerError::Output(e.to_string()))?,
                Backend::Null(sink) => sink.set_running(true),
            }
            self.running = true;
        }
        Ok(())
    }

    /// Fades out; [`Output::suspend`] stops the device once the fade is done.
    pub fn pause(&self) {
        self.shared.paused.store(true, Ordering::Relaxed);
    }

    /// Stops callbacks entirely so a paused player costs no CPU. Some ALSA
    /// devices cannot pause; those keep running and render silence.
    pub fn suspend(&mut self) {
        if !self.running || !self.shared.paused.load(Ordering::Relaxed) {
            return;
        }
        match &self.backend {
            Backend::Cpal(stream) => {
                if let Err(e) = stream.pause() {
                    tracing::debug!(error = %e, "output cannot pause, rendering silence instead");
                    return;
                }
            }
            Backend::Null(sink) => sink.set_running(false),
        }
        self.running = false;
    }
}

fn renderer(shared: &Arc<Shared>, rate: u32, channels: usize) -> Renderer {
    Renderer {
        shared: shared.clone(),
        consumer: None,
        channels,
        gain: Ramp::new(0.0, (rate as f32 * RAMP_SECONDS) as u32),
        eq: Equalizer::new(rate, channels),
    }
}

fn open_cpal(
    shared: &Arc<Shared>,
    errors: Sender<Command>,
) -> Result<(Backend, u32, usize), PlayerError> {
    let device = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| PlayerError::Output("no output device".into()))?;
    let supported = device
        .default_output_config()
        .map_err(|e| PlayerError::Output(e.to_string()))?;
    let config = supported.config();
    let (rate, channels) = (config.sample_rate, config.channels as usize);
    let renderer = renderer(shared, rate, channels);
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build_stream::<f32>(&device, config, renderer, errors),
        SampleFormat::F64 => build_stream::<f64>(&device, config, renderer, errors),
        SampleFormat::I16 => build_stream::<i16>(&device, config, renderer, errors),
        SampleFormat::U16 => build_stream::<u16>(&device, config, renderer, errors),
        SampleFormat::I32 => build_stream::<i32>(&device, config, renderer, errors),
        SampleFormat::U32 => build_stream::<u32>(&device, config, renderer, errors),
        SampleFormat::I8 => build_stream::<i8>(&device, config, renderer, errors),
        SampleFormat::U8 => build_stream::<u8>(&device, config, renderer, errors),
        other => {
            return Err(PlayerError::Output(format!(
                "unsupported sample format {other}"
            )));
        }
    }?;
    tracing::info!(rate, channels, format = %supported.sample_format(), "opened audio output");
    Ok((Backend::Cpal(stream), rate, channels))
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut renderer: Renderer,
    errors: Sender<Command>,
) -> Result<cpal::Stream, PlayerError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = renderer.channels;
    let mut scratch = vec![0.0f32; SCRATCH_FRAMES * channels];
    device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], _| {
                for chunk in data.chunks_mut(scratch.len()) {
                    let samples = &mut scratch[..chunk.len()];
                    renderer.render(samples);
                    for (out, sample) in chunk.iter_mut().zip(samples.iter()) {
                        *out = T::from_sample(*sample);
                    }
                }
            },
            move |err| match err.kind() {
                // An xrun is one audible glitch that the backend recovers from
                // by itself (a CPU stall during a system switch is enough), and
                // a reroute keeps the stream running. Neither ends the track.
                cpal::ErrorKind::Xrun | cpal::ErrorKind::DeviceChanged => {
                    tracing::debug!(%err, "audio output glitch")
                }
                _ => {
                    let _ = errors.send(Command::OutputError(err.to_string()));
                }
            },
            None,
        )
        .map_err(|e| PlayerError::Output(e.to_string()))
}

struct NullSink {
    thread: Option<JoinHandle<()>>,
    control: Arc<(AtomicBool, AtomicBool)>,
}

impl NullSink {
    fn spawn(mut renderer: Renderer, rate: u32, channels: usize) -> Self {
        let control = Arc::new((AtomicBool::new(false), AtomicBool::new(false)));
        let flags = control.clone();
        let thread = thread::Builder::new()
            .name("formalmusic-null".into())
            .spawn(move || {
                let (running, stop) = &*flags;
                let period = Duration::from_millis(10);
                let mut buffer = vec![0.0f32; (rate / 100) as usize * channels];
                let mut deadline = Instant::now();
                while !stop.load(Ordering::Acquire) {
                    if !running.load(Ordering::Acquire) {
                        thread::park();
                        deadline = Instant::now();
                        continue;
                    }
                    renderer.render(&mut buffer);
                    deadline += period;
                    thread::sleep(deadline.saturating_duration_since(Instant::now()));
                }
            })
            .expect("spawn null sink");
        Self {
            thread: Some(thread),
            control,
        }
    }

    fn set_running(&self, running: bool) {
        self.control.0.store(running, Ordering::Release);
        if let Some(thread) = &self.thread {
            thread.thread().unpark();
        }
    }
}

impl Drop for NullSink {
    fn drop(&mut self) {
        self.control.1.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared() -> Arc<Shared> {
        Arc::new(Shared {
            mailbox: Mutex::new(Mailbox::default()),
            swap_pending: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            played: AtomicU64::new(0),
            volume: AtomicU32::new(1f32.to_bits()),
            muted: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            eq: Mutex::new(None),
            eq_pending: AtomicBool::new(false),
        })
    }

    #[test]
    fn renderer_swaps_rings_and_counts_frames() {
        let shared = shared();
        let mut renderer = Renderer {
            shared: shared.clone(),
            consumer: None,
            channels: 2,
            gain: Ramp::new(1.0, 1),
            eq: Equalizer::new(48_000, 2),
        };
        let (mut producer, consumer) = RingBuffer::new(16);
        shared.mailbox.lock().unwrap().incoming = Some(Swap {
            generation: 1,
            consumer,
        });
        shared.swap_pending.store(true, Ordering::Release);
        let _ = producer.push_partial_slice(&[0.5; 6]);

        let mut out = [9.0f32; 8];
        renderer.render(&mut out);
        assert_eq!(out, [0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.0, 0.0]);
        assert_eq!(shared.generation.load(Ordering::Acquire), 1);
        assert_eq!(shared.played.load(Ordering::Acquire), 3);
        assert!(!shared.swap_pending.load(Ordering::Acquire));
    }

    #[test]
    fn pause_fades_then_holds_the_ring() {
        let shared = shared();
        let (mut producer, consumer) = RingBuffer::new(64);
        let mut renderer = Renderer {
            shared: shared.clone(),
            consumer: Some(consumer),
            channels: 1,
            gain: Ramp::new(1.0, 4),
            eq: Equalizer::new(48_000, 1),
        };
        let _ = producer.push_partial_slice(&[1.0; 64]);
        shared.paused.store(true, Ordering::Relaxed);

        let mut out = [0.0f32; 8];
        renderer.render(&mut out);
        assert_eq!(&out[..5], [0.75, 0.5, 0.25, 0.0, 0.0]);
        let played = shared.played.load(Ordering::Acquire);
        renderer.render(&mut out);
        assert_eq!(out, [0.0; 8]);
        assert_eq!(
            shared.played.load(Ordering::Acquire),
            played,
            "paused output holds its position"
        );
    }
}
