//! Video decoded by the ffmpeg binary into raw BGRA frames at the size they
//! are drawn and handed to the UI through a bounded channel. The same
//! approach as the messages app's `video.rs`, without the sound.
//!
//! Two kinds: animated covers ([`Loop`]), a looping silent mp4 paced on the
//! wall clock, and music videos ([`Synced`]), a googlevideo stream paced on
//! the daemon's playback clock so the picture follows the audio.
//!
//! ffmpeg does the scaling, the crop and the frame rate cap, so the pipe
//! carries only what gets painted: a 96 px bar cover at 12 fps is about
//! 440 KB a second. Stopping is dropping the [`Loop`] or [`Synced`], which
//! kills ffmpeg.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use formalmusic_api::VideoStream;
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

#[derive(Clone, Debug, PartialEq)]
pub struct VideoInfo {
    pub fps: f64,
    /// Seconds; zero when the container does not say.
    pub duration: f64,
}

pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Frame rate and duration of the first video stream, or `None` when
/// ffprobe is missing or the file has no picture.
pub async fn probe(path: &Path) -> Option<VideoInfo> {
    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=avg_frame_rate,r_frame_rate:format=duration",
            "-of",
            "json",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_probe(&String::from_utf8_lossy(&output.stdout))
}

fn parse_rate(text: &str) -> Option<f64> {
    let (numer, denom) = text.split_once('/')?;
    let (numer, denom): (f64, f64) = (numer.trim().parse().ok()?, denom.trim().parse().ok()?);
    (numer > 0. && denom > 0.).then(|| numer / denom)
}

fn parse_probe(json: &str) -> Option<VideoInfo> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let video = value.get("streams")?.as_array()?.first()?;
    let fps = ["avg_frame_rate", "r_frame_rate"]
        .iter()
        .find_map(|key| {
            video
                .get(key)
                .and_then(|rate| rate.as_str())
                .and_then(parse_rate)
        })
        .unwrap_or(30.);
    let duration = value
        .get("format")
        .and_then(|format| format.get("duration"))
        .and_then(|duration| duration.as_str())
        .and_then(|text| text.parse().ok())
        .unwrap_or(0.);
    Some(VideoInfo {
        fps: fps.clamp(1., 120.),
        duration,
    })
}

/// One run of ffmpeg from a start position, looping until dropped.
pub struct Loop {
    /// Seconds into the file of the last frame let through.
    position: Arc<AtomicU64>,
    duration: f64,
    task: JoinHandle<()>,
}

impl Loop {
    /// Decodes `path` from `start` seconds into `side` px squares, at the
    /// file's own rate or `max_fps`, whichever is lower, and sends each frame
    /// on `frames` as its time comes. The channel closes if ffmpeg fails.
    pub fn start(
        runtime: &tokio::runtime::Handle,
        path: &Path,
        info: &VideoInfo,
        side: u32,
        max_fps: f64,
        start: f64,
        frames: mpsc::Sender<VideoFrame>,
    ) -> Loop {
        let start = if info.duration > 0. {
            start.rem_euclid(info.duration)
        } else {
            0.
        };
        let position = Arc::new(AtomicU64::new(start.to_bits()));
        let rate = info.fps.min(max_fps);
        let task = runtime.spawn(run(
            path.to_path_buf(),
            start,
            rate,
            side.max(2),
            frames,
            position.clone(),
        ));
        Loop {
            position,
            duration: info.duration,
            task,
        }
    }

    /// Where the next start should pick up.
    pub fn position(&self) -> f64 {
        let position = f64::from_bits(self.position.load(Ordering::Acquire));
        if self.duration > 0. {
            position.rem_euclid(self.duration)
        } else {
            0.
        }
    }
}

impl Drop for Loop {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn run(
    path: PathBuf,
    start: f64,
    rate: f64,
    side: u32,
    frames: mpsc::Sender<VideoFrame>,
    position: Arc<AtomicU64>,
) {
    // One decoder thread and one filter thread: a 768 px H.264 stream needs
    // a fraction of one core, and ffmpeg would otherwise start a thread per
    // core for it.
    let filter = format!(
        "fps={rate:.4},scale={side}:{side}:force_original_aspect_ratio=increase:flags=bilinear,crop={side}:{side}"
    );
    let mut command = tokio::process::Command::new("ffmpeg");
    command.args(["-v", "error", "-nostdin", "-threads", "1"]);
    // Apple's covers are at most 768 px. At half that or less the scaler
    // averages deblocking artifacts away, so the decoder skips that pass,
    // about a sixth of its time.
    if side <= 384 {
        command.args(["-skip_loop_filter", "all"]);
    }
    let spawned = command
        .args(["-stream_loop", "-1"])
        .args(["-ss", &format!("{start:.3}"), "-i"])
        .arg(&path)
        .args([
            "-map",
            "0:v:0",
            "-an",
            "-filter_threads",
            "1",
            "-vf",
            &filter,
        ])
        .args(["-pix_fmt", "bgra", "-f", "rawvideo", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            tracing::warn!("animated cover: ffmpeg: {error}");
            return;
        }
    };
    let Some(mut stdout) = child.stdout.take() else {
        return;
    };
    let frame_len = side as usize * side as usize * 4;
    let frame_time = Duration::from_secs_f64(1. / rate);
    let began = Instant::now();
    let mut index: u32 = 0;
    loop {
        let Some(bgra) = read_frame(&mut stdout, frame_len).await else {
            return;
        };
        let due = began + frame_time * index;
        index += 1;
        let now = Instant::now();
        if now < due {
            tokio::time::sleep_until(due.into()).await;
        } else if now - due > frame_time && index > 1 {
            // Behind by more than a frame: skip it, the next one is on time.
            continue;
        }
        let at = start + (index - 1) as f64 / rate;
        position.store(at.to_bits(), Ordering::Release);
        let frame = VideoFrame {
            width: side,
            height: side,
            bgra,
        };
        match frames.try_send(frame) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
            Err(mpsc::error::TrySendError::Closed(_)) => return,
        }
    }
}

/// One raw frame from ffmpeg. Reads into spare capacity, so the 1-4 MB
/// buffer each frame needs is not zeroed first only to be overwritten.
async fn read_frame(stdout: &mut tokio::process::ChildStdout, len: usize) -> Option<Vec<u8>> {
    let mut frame = Vec::with_capacity(len);
    stdout.take(len as u64).read_to_end(&mut frame).await.ok()?;
    (frame.len() == len).then_some(frame)
}

/// Where the audio is, in seconds into the video, or `None` once it stops
/// playing this video. Read on the decoder's task for every frame.
pub type Clock = Arc<dyn Fn() -> Option<f64> + Send + Sync>;

/// Frames this close to the audio's place are shown as they come.
const SYNC_TOLERANCE: f64 = 0.040;
/// Further off than this, ffmpeg starts again where the audio is instead of
/// catching up frame by frame.
const RESEEK_AFTER: f64 = 1.5;
/// How far ahead of the audio a fresh ffmpeg starts, to cover opening the
/// stream. Grows by however late the first frame was when it was not enough.
const FIRST_LEAD: f64 = 1.0;
const MAX_LEAD: f64 = 8.;
/// Each frame costs a decode, a copy to the GPU and a redraw of the window;
/// past film rate a player box gains little for that.
const MAX_SYNCED_FPS: f64 = 24.;
/// Starts in a row that produced no frame before giving up.
const MAX_FAILED_STARTS: u32 = 3;

/// A music video decoded muted beside the daemon's audio, held to its clock.
pub struct Synced {
    task: JoinHandle<()>,
}

impl Synced {
    /// Decodes `stream` at `width` x `height` from wherever `clock` says,
    /// showing each frame when the audio reaches it, dropping frames that
    /// come late and starting over after a seek. With `hardware`, ffmpeg
    /// decodes through VA-API and falls back to software if that fails. The
    /// channel closes when the stream cannot be read, which usually means
    /// googlevideo refused the URL.
    pub fn start(
        runtime: &tokio::runtime::Handle,
        stream: &VideoStream,
        width: u32,
        height: u32,
        hardware: bool,
        clock: Clock,
        frames: mpsc::Sender<VideoFrame>,
    ) -> Synced {
        let size = (width.max(2) & !1, height.max(2) & !1);
        let task = runtime.spawn(run_synced(stream.clone(), size, hardware, clock, frames));
        Synced { task }
    }
}

impl Drop for Synced {
    fn drop(&mut self) {
        self.task.abort();
    }
}

enum Ended {
    /// ffmpeg stopped; whether it got a frame out first.
    Stopped(bool),
    /// The audio moved too far from the picture. `late` is how far behind
    /// the first frame came out, when it was that one.
    Reseek { late: Option<f64> },
    /// The clock stopped or the UI went away.
    Done,
}

async fn run_synced(
    stream: VideoStream,
    size: (u32, u32),
    mut hardware: bool,
    clock: Clock,
    frames: mpsc::Sender<VideoFrame>,
) {
    let rate = stream.fps.clamp(1., MAX_SYNCED_FPS);
    let mut lead = FIRST_LEAD;
    let mut failed = 0;
    loop {
        match decode_from(&stream, lead, rate, size, hardware, &clock, &frames).await {
            Ended::Done => return,
            Ended::Stopped(true) => failed = 0,
            Ended::Stopped(false) if hardware => {
                tracing::info!("music video: VA-API decode failed, decoding in software");
                hardware = false;
            }
            Ended::Stopped(false) => {
                failed += 1;
                if failed >= MAX_FAILED_STARTS {
                    return;
                }
            }
            Ended::Reseek { late: Some(late) } => lead = (lead + late + 0.25).min(MAX_LEAD),
            Ended::Reseek { late: None } => {}
        }
    }
}

/// One ffmpeg run, from `lead` seconds past where the audio is now.
async fn decode_from(
    stream: &VideoStream,
    lead: f64,
    rate: f64,
    (width, height): (u32, u32),
    hardware: bool,
    clock: &Clock,
    frames: &mpsc::Sender<VideoFrame>,
) -> Ended {
    let Some(now) = clock() else {
        return Ended::Done;
    };
    let start = now + lead;
    let mut command = tokio::process::Command::new("ffmpeg");
    command.args(["-v", "error", "-nostdin"]);
    // One decoder thread keeps 480p H.264 well under real time; taller
    // streams get a second so a slow core does not fall behind the audio.
    command.args(["-threads", if height > 480 { "2" } else { "1" }]);
    // Frames stay on the GPU through decode and scaling, and come down as
    // NV12 at the drawn size for swscale to turn into BGRA. Reading BGRA
    // back from the GPU instead is a slow uncached copy per frame: on an
    // Arrow Lake iGPU it cost five times this, on an N305 about the same.
    let mut filter = if hardware {
        command.args(["-hwaccel", "vaapi", "-hwaccel_output_format", "vaapi"]);
        format!("scale_vaapi=w={width}:h={height}:format=nv12,hwdownload,format=nv12")
    } else {
        format!("scale={width}:{height}:flags=bilinear")
    };
    let headers: String = stream
        .headers
        .iter()
        .map(|(name, value)| format!("{name}: {value}\r\n"))
        .collect();
    if !headers.is_empty() {
        command.args(["-headers", &headers]);
    }
    if stream.fps > rate + 0.5 {
        filter = format!("fps={rate:.4},{filter}");
    }
    let spawned = command
        .args(["-ss", &format!("{start:.3}"), "-i", &stream.url])
        .args([
            "-map",
            "0:v:0",
            "-an",
            "-filter_threads",
            "1",
            "-vf",
            &filter,
        ])
        .args(["-pix_fmt", "bgra", "-f", "rawvideo", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            tracing::warn!("music video: ffmpeg: {error}");
            return Ended::Done;
        }
    };
    let Some(mut stdout) = child.stdout.take() else {
        return Ended::Stopped(false);
    };
    let frame_len = width as usize * height as usize * 4;
    let mut index: u32 = 0;
    loop {
        let Some(bgra) = read_frame(&mut stdout, frame_len).await else {
            return Ended::Stopped(index > 0);
        };
        let at = start + index as f64 / rate;
        let first_frame = index == 0;
        index += 1;
        let ahead = loop {
            let Some(now) = clock() else {
                return Ended::Done;
            };
            let ahead = at - now;
            // The first frame is meant to be early, by up to the lead.
            let early = if first_frame { lead } else { 0. } + RESEEK_AFTER;
            if ahead > early {
                return Ended::Reseek { late: None };
            }
            if ahead < -RESEEK_AFTER {
                return Ended::Reseek {
                    late: first_frame.then_some(-ahead),
                };
            }
            if ahead <= SYNC_TOLERANCE {
                break ahead;
            }
            // Hold the frame, looking at the clock again at least every
            // 100 ms in case the audio paused or jumped.
            tokio::time::sleep(Duration::from_secs_f64(ahead.min(0.1))).await;
        };
        if ahead < -SYNC_TOLERANCE {
            continue;
        }
        let frame = VideoFrame {
            width,
            height,
            bgra,
        };
        match frames.try_send(frame) {
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
            Err(mpsc::error::TrySendError::Closed(_)) => return Ended::Done,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_reads_rate_and_duration() {
        let json = r#"{"streams":[{"avg_frame_rate":"24000/1001","r_frame_rate":"24000/1001"}],"format":{"duration":"12.512000"}}"#;
        let info = parse_probe(json).unwrap();
        assert!((info.fps - 23.976).abs() < 0.01);
        assert!((info.duration - 12.512).abs() < 0.001);
    }

    #[test]
    fn a_file_without_a_picture_is_not_a_video() {
        assert!(parse_probe(r#"{"streams":[],"format":{}}"#).is_none());
    }

    /// Two seconds at 30 fps, rendered by ffmpeg under target/ once.
    fn fixture() -> Option<PathBuf> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-video");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join("pattern.mp4");
        if !path.exists() {
            let status = std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-nostdin", "-y", "-f", "lavfi"])
                .args(["-i", "testsrc2=size=160x120:rate=30:duration=2"])
                .args(["-c:v", "mpeg4", "-f", "mp4"])
                .arg(&path)
                .status()
                .ok()?;
            if !status.success() {
                return None;
            }
        }
        Some(path)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn loops_past_the_end_at_the_capped_rate_and_size() {
        let Some(path) = fixture() else { return };
        let Some(info) = probe(&path).await else {
            return;
        };
        assert!((info.fps - 30.).abs() < 0.01 && (info.duration - 2.).abs() < 0.1);
        let (tx, mut rx) = mpsc::channel(2);
        let started = Instant::now();
        let run = Loop::start(
            &tokio::runtime::Handle::current(),
            &path,
            &info,
            48,
            12.,
            1.5,
            tx,
        );
        let mut count = 0;
        while count < 12 {
            let frame = rx.recv().await.expect("ffmpeg stopped");
            assert_eq!(
                (frame.width, frame.height, frame.bgra.len()),
                (48, 48, 48 * 48 * 4)
            );
            count += 1;
        }
        let elapsed = started.elapsed().as_secs_f64();
        assert!(elapsed > 0.85, "12 frames at 12 fps came in {elapsed:.2}s");
        // Started 1.5 s into a 2 s file, a second of frames wrapped round.
        assert!(run.position() < 1.5, "at {}", run.position());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn synced_frames_follow_the_clock_and_stop_with_it() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-video");
        let path = dir.join("long.mp4");
        if !path.exists() {
            let made = std::fs::create_dir_all(&dir).is_ok()
                && std::process::Command::new("ffmpeg")
                    .args(["-v", "error", "-nostdin", "-y", "-f", "lavfi"])
                    .args(["-i", "testsrc2=size=160x90:rate=30:duration=8"])
                    .args(["-c:v", "mpeg4", "-f", "mp4"])
                    .arg(&path)
                    .status()
                    .is_ok_and(|s| s.success());
            if !made {
                return;
            }
        }
        let stream = VideoStream {
            url: path.to_string_lossy().into_owned(),
            headers: Vec::new(),
            width: 160,
            height: 90,
            fps: 30.,
            codec: "mp4v".into(),
        };
        let began = Instant::now();
        let stop_at = 3.0;
        let clock: Clock = Arc::new(move || {
            let at = 0.5 + began.elapsed().as_secs_f64();
            (at < 0.5 + stop_at).then_some(at)
        });
        let (tx, mut rx) = mpsc::channel(2);
        let _run = Synced::start(
            &tokio::runtime::Handle::current(),
            &stream,
            80,
            45,
            false,
            clock,
            tx,
        );
        let mut count = 0;
        while let Some(frame) = rx.recv().await {
            assert_eq!(
                (frame.width, frame.height, frame.bgra.len()),
                (80, 44, 80 * 44 * 4)
            );
            count += 1;
        }
        let elapsed = began.elapsed().as_secs_f64();
        // The first second goes to the start lead, then frames come at the
        // 24 fps cap until the clock stops.
        assert!((stop_at..stop_at + 0.5).contains(&elapsed), "{elapsed:.2}s");
        assert!((36..=54).contains(&count), "{count} frames");
    }
}
