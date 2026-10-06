//! Demux and decode one track into interleaved f32 at the output rate and
//! channel count.

use std::sync::LazyLock;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::errors::{Error, SeekErrorKind};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{TimeBase, Timestamp};
use symphonia_adapter_libopus::OpusDecoder;

use crate::error::PlayerError;
use crate::fetch::{RangeReader, StreamStatus};
use crate::source::StreamSource;

static CODECS: LazyLock<CodecRegistry> = LazyLock::new(|| {
    let mut registry = CodecRegistry::new();
    symphonia::default::register_enabled_codecs(&mut registry);
    registry.register_audio_decoder::<OpusDecoder>();
    registry
});

const RESAMPLE_CHUNK: usize = 1024;

pub(crate) struct TrackDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    rate: u32,
    channels: usize,
    out_channels: usize,
    duration_ms: Option<u64>,
    /// After a seek, decoded audio before this time (seconds) is dropped so
    /// playback starts on the exact requested frame.
    skip_until: Option<f64>,
    resampler: Option<Resample>,
    stream: Option<StreamStatus>,
    decoded: Vec<f32>,
    mapped: Vec<f32>,
}

impl TrackDecoder {
    pub fn open(
        client: &reqwest::blocking::Client,
        source: &StreamSource,
        out_rate: u32,
        out_channels: usize,
    ) -> Result<Self, PlayerError> {
        let (media, stream): (Box<dyn MediaSource>, _) = match source.local_path() {
            Some(path) => {
                let file = std::fs::File::open(path)
                    .map_err(|e| PlayerError::Unsupported(format!("{}: {e}", path.display())))?;
                (Box::new(file), None)
            }
            None => {
                let (reader, status) = RangeReader::open(client, source)?;
                (Box::new(reader), Some(status))
            }
        };
        let mss = MediaSourceStream::new(media, Default::default());
        let mut hint = Hint::new();
        hint.mime_type(&source.mime);
        if let Some(ext) = source
            .local_path()
            .and_then(|p| p.extension())
            .and_then(|e| e.to_str())
        {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe().probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )?;
        Self::new(format, stream, out_rate, out_channels)
    }

    fn new(
        format: Box<dyn FormatReader>,
        stream: Option<StreamStatus>,
        out_rate: u32,
        out_channels: usize,
    ) -> Result<Self, PlayerError> {
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| PlayerError::Unsupported("no audio track".into()))?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(|| PlayerError::Unsupported("no audio codec parameters".into()))?;
        let rate = params
            .sample_rate
            .ok_or_else(|| PlayerError::Unsupported("unknown sample rate".into()))?;
        let channels = params.channels.as_ref().map_or(2, |c| c.count()).max(1);
        let decoder = CODECS.make_audio_decoder(params, &AudioDecoderOptions::default())?;

        let to_ms = |tb: TimeBase, duration| {
            tb.calc_duration(duration)
                .map(|t| (t.as_secs_f64() * 1000.0) as u64)
        };
        // WebM only stores the duration on the segment, not the track.
        let info = format.media_info();
        let duration_ms = match (track.num_frames, track.duration, track.time_base) {
            (Some(frames), _, _) => Some(frames * 1000 / rate as u64),
            (None, Some(duration), Some(tb)) => to_ms(tb, duration),
            _ => info
                .time_base
                .zip(info.duration)
                .and_then(|(tb, d)| to_ms(tb, d)),
        };
        let resampler = (rate != out_rate)
            .then(|| Resample::new(rate, out_rate, out_channels))
            .transpose()?;

        Ok(Self {
            track_id: track.id,
            time_base: track.time_base,
            format,
            decoder,
            rate,
            channels,
            out_channels,
            duration_ms,
            skip_until: None,
            resampler,
            stream,
            decoded: Vec::new(),
            mapped: Vec::new(),
        })
    }

    pub fn duration_ms(&self) -> Option<u64> {
        self.duration_ms
    }

    /// How far into the track the download reaches, in ms.
    pub fn buffered_ms(&self) -> Option<u64> {
        let duration = self.duration_ms?;
        let Some(stream) = &self.stream else {
            return Some(duration);
        };
        Some((stream.buffered_until() as u128 * duration as u128 / stream.len() as u128) as u64)
    }

    /// False while the next packet would block on the network.
    pub fn is_ready(&self) -> bool {
        self.stream
            .as_ref()
            .is_none_or(|s| s.is_ready() || s.error().is_some())
    }

    /// Appends the next packet's audio to `out`. Returns false at the end of
    /// the track, after flushing the resampler.
    pub fn decode_next(&mut self, out: &mut Vec<f32>) -> Result<bool, PlayerError> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) | Err(Error::ResetRequired) => {
                    if let Some(resampler) = &mut self.resampler {
                        resampler.flush(out)?;
                    }
                    return Ok(false);
                }
                Err(e) => return Err(e.into()),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(Error::DecodeError(msg)) => {
                    tracing::debug!(msg, "skipping undecodable packet");
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            let frames = decoded.frames();
            decoded.copy_to_vec_interleaved(&mut self.decoded);

            let mut first = 0;
            if let Some(target) = self.skip_until {
                let start = self.time_base.map_or(0.0, |tb| {
                    ts_to_secs(packet.pts.get() + packet.trim_start.get() as i64, tb)
                });
                first = frames_to_skip(start, target, self.rate, frames);
                if first == frames {
                    continue;
                }
                self.skip_until = None;
            }
            let samples = &self.decoded[first * self.channels..];

            match &mut self.resampler {
                None => map_channels(samples, self.channels, self.out_channels, out),
                Some(resampler) => {
                    self.mapped.clear();
                    map_channels(samples, self.channels, self.out_channels, &mut self.mapped);
                    resampler.process(&self.mapped, out)?;
                }
            }
            return Ok(true);
        }
    }

    /// Seeks so the next decoded frame is the one at `ms`. Returns the
    /// position actually reached, which is only earlier than `ms` when `ms`
    /// is past the end.
    pub fn seek(&mut self, ms: u64) -> Result<u64, PlayerError> {
        let ms = self.duration_ms.map_or(ms, |d| ms.min(d));
        let Some(tb) = self.time_base else {
            return Err(PlayerError::Unsupported("track has no time base".into()));
        };
        let ts = Timestamp::new(ms_to_ts(ms, tb) as i64);
        match self.format.seek(
            SeekMode::Accurate,
            SeekTo::Timestamp {
                ts,
                track_id: self.track_id,
            },
        ) {
            Ok(seeked) => self.skip_until = Some(ts_to_secs(seeked.required_ts.get(), tb)),
            Err(Error::SeekError(SeekErrorKind::OutOfRange)) => {
                self.skip_until = Some(f64::INFINITY)
            }
            Err(e) => return Err(e.into()),
        }
        self.decoder.reset();
        if let Some(resampler) = &mut self.resampler {
            resampler.reset();
        }
        Ok(ms)
    }
}

pub(crate) fn ms_to_ts(ms: u64, tb: TimeBase) -> u64 {
    (ms as u128 * tb.denom.get() as u128 / (tb.numer.get() as u128 * 1000)) as u64
}

pub(crate) fn ts_to_secs(ts: i64, tb: TimeBase) -> f64 {
    ts as f64 * tb.numer.get() as f64 / tb.denom.get() as f64
}

/// Frames to drop from a packet starting at `start` seconds so output begins
/// at `target` seconds. Equal to `frames` when the whole packet is before it.
pub(crate) fn frames_to_skip(start: f64, target: f64, rate: u32, frames: usize) -> usize {
    let skip = ((target - start) * rate as f64).round();
    if skip <= 0.0 {
        0
    } else {
        (skip as usize).min(frames)
    }
}

/// Converts interleaved audio between channel counts: mono is copied to
/// every output channel, stereo fills the first two, and extra input
/// channels are dropped (YouTube audio is never more than stereo).
pub(crate) fn map_channels(input: &[f32], from: usize, to: usize, out: &mut Vec<f32>) {
    if from == to {
        out.extend_from_slice(input);
        return;
    }
    out.reserve(input.len() / from * to);
    for frame in input.chunks_exact(from) {
        match (from, to) {
            (1, _) => out.extend(std::iter::repeat_n(frame[0], to)),
            (_, 1) => out.push((frame[0] + frame[1]) * 0.5),
            _ => {
                out.extend_from_slice(&frame[..2]);
                out.extend(std::iter::repeat_n(0.0, to - 2));
            }
        }
    }
}

/// Fixed-ratio FFT resampling with the filter delay trimmed from the start
/// and the tail padded out on flush, so a resampled track has exactly
/// `round(frames * ratio)` frames and gapless joins stay aligned.
struct Resample {
    inner: Fft<f32>,
    channels: usize,
    ratio: f64,
    input: Vec<f32>,
    output: Vec<f32>,
    delay: usize,
    frames_in: u64,
    frames_out: u64,
}

impl Resample {
    fn new(from: u32, to: u32, channels: usize) -> Result<Self, PlayerError> {
        let inner = Fft::<f32>::new(
            from as usize,
            to as usize,
            RESAMPLE_CHUNK,
            channels,
            FixedSync::Input,
        )
        .map_err(|e| PlayerError::Unsupported(format!("resampler: {e}")))?;
        let output = vec![0.0; inner.output_frames_max() * channels];
        Ok(Self {
            delay: inner.output_delay(),
            ratio: to as f64 / from as f64,
            inner,
            channels,
            input: Vec::new(),
            output,
            frames_in: 0,
            frames_out: 0,
        })
    }

    fn reset(&mut self) {
        self.inner.reset();
        self.input.clear();
        self.delay = self.inner.output_delay();
        self.frames_in = 0;
        self.frames_out = 0;
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<(), PlayerError> {
        self.input.extend_from_slice(input);
        self.frames_in += (input.len() / self.channels) as u64;
        let mut consumed = 0;
        loop {
            let need = self.inner.input_frames_next();
            if self.input.len() / self.channels - consumed < need {
                break;
            }
            let produced = self.run(consumed, need, None)?;
            consumed += need;
            self.emit(produced, u64::MAX, out);
        }
        self.input.drain(..consumed * self.channels);
        Ok(())
    }

    fn flush(&mut self, out: &mut Vec<f32>) -> Result<(), PlayerError> {
        let expected = (self.frames_in as f64 * self.ratio).round() as u64;
        while self.frames_out < expected {
            let pending = self.input.len() / self.channels;
            let need = self.inner.input_frames_next();
            if self.input.len() < need * self.channels {
                self.input.resize(need * self.channels, 0.0);
            }
            let produced = self.run(0, need, Some(pending))?;
            self.input.clear();
            self.emit(produced, expected, out);
        }
        Ok(())
    }

    fn run(
        &mut self,
        offset: usize,
        frames: usize,
        partial: Option<usize>,
    ) -> Result<usize, PlayerError> {
        let input =
            InterleavedSlice::new(&self.input[offset * self.channels..], self.channels, frames)
                .map_err(|e| PlayerError::Decode(e.to_string()))?;
        let max = self.output.len() / self.channels;
        let mut output = InterleavedSlice::new_mut(&mut self.output[..], self.channels, max)
            .map_err(|e| PlayerError::Decode(e.to_string()))?;
        let indexing = Indexing {
            partial_len: partial,
            ..Default::default()
        };
        let (_, produced) = self
            .inner
            .process_into_buffer(&input, &mut output, Some(&indexing))
            .map_err(|e| PlayerError::Decode(e.to_string()))?;
        Ok(produced)
    }

    /// Moves `produced` frames from the scratch buffer to `out`, dropping the
    /// filter delay and anything past `limit` total frames.
    fn emit(&mut self, produced: usize, limit: u64, out: &mut Vec<f32>) {
        let skip = self.delay.min(produced);
        self.delay -= skip;
        let available = (produced - skip) as u64;
        let take = available.min(limit.saturating_sub(self.frames_out)) as usize;
        let start = skip * self.channels;
        out.extend_from_slice(&self.output[start..start + take * self.channels]);
        self.frames_out += take as u64;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZero;

    fn tb(numer: u32, denom: u32) -> TimeBase {
        TimeBase {
            numer: NonZero::new(numer).unwrap(),
            denom: NonZero::new(denom).unwrap(),
        }
    }

    #[test]
    fn ms_converts_to_timestamps() {
        assert_eq!(ms_to_ts(1500, tb(1, 48_000)), 72_000);
        assert_eq!(ms_to_ts(1500, tb(1, 1000)), 1500);
        assert_eq!(ms_to_ts(1, tb(1, 44_100)), 44);
        assert_eq!(ms_to_ts(3_600_000, tb(1, 1_000_000_000)), 3_600_000_000_000);
        assert!((ts_to_secs(72_000, tb(1, 48_000)) - 1.5).abs() < 1e-12);
    }

    #[test]
    fn seek_skips_to_the_exact_frame() {
        // A 20 ms opus packet at 10.000 s, seeking to 10.005 s.
        assert_eq!(frames_to_skip(10.0, 10.005, 48_000, 960), 240);
        assert_eq!(frames_to_skip(10.0, 9.0, 48_000, 960), 0);
        assert_eq!(frames_to_skip(10.0, 10.5, 48_000, 960), 960);
        assert_eq!(frames_to_skip(0.0, f64::INFINITY, 48_000, 960), 960);
    }

    #[test]
    fn channels_map_up_and_down() {
        let mut out = Vec::new();
        map_channels(&[0.5, -0.5], 1, 2, &mut out);
        assert_eq!(out, [0.5, 0.5, -0.5, -0.5]);
        out.clear();
        map_channels(&[1.0, 0.0], 2, 1, &mut out);
        assert_eq!(out, [0.5]);
        out.clear();
        map_channels(&[0.1, 0.2], 2, 4, &mut out);
        assert_eq!(out, [0.1, 0.2, 0.0, 0.0]);
    }

    #[test]
    fn resampler_output_length_is_exact() {
        let mut resample = Resample::new(44_100, 48_000, 2).unwrap();
        let mut out = Vec::new();
        let input: Vec<f32> = (0..44_100 * 2)
            .map(|i| ((i / 2) as f32 * 0.01).sin())
            .collect();
        for chunk in input.chunks(2048 * 2) {
            resample.process(chunk, &mut out).unwrap();
        }
        resample.flush(&mut out).unwrap();
        assert_eq!(out.len(), 48_000 * 2);
    }
}
