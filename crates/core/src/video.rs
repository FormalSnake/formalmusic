//! Video decoded by the ffmpeg binary into raw BGRA frames at the size they
//! are drawn and handed to the UI through a bounded channel. The same
//! approach as the messages app's `video.rs`, without the sound.
//!
//! Animated covers ([`Loop`]): a looping silent mp4 paced on the wall clock.
//!
//! ffmpeg does the scaling, the crop and the frame rate cap, so the pipe
//! carries only what gets painted: a 96 px bar cover at 12 fps is about
//! 440 KB a second. Stopping is dropping the [`Loop`], which kills ffmpeg.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

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
    let output = crate::process::async_command("ffprobe")
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
    /// on `frames` as its time comes. With `hardware`, ffmpeg decodes and
    /// scales through VA-API and falls back to software if that fails. The
    /// channel closes if ffmpeg fails.
    pub fn start(
        runtime: &tokio::runtime::Handle,
        path: &Path,
        info: &VideoInfo,
        side: u32,
        (max_fps, start): (f64, f64),
        hardware: bool,
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
            (start, rate),
            side.max(2),
            hardware,
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
    (start, rate): (f64, f64),
    side: u32,
    hardware: bool,
    frames: mpsc::Sender<VideoFrame>,
    position: Arc<AtomicU64>,
) {
    let looped = |hardware| loop_from(&path, (start, rate), side, hardware, &frames, &position);
    if hardware && looped(true).await {
        return;
    }
    if hardware {
        tracing::info!("animated cover: VA-API decode failed, decoding in software");
    }
    looped(false).await;
}

/// One ffmpeg run, looping until dropped. False when it gave no frame.
async fn loop_from(
    path: &Path,
    (start, rate): (f64, f64),
    side: u32,
    hardware: bool,
    frames: &mpsc::Sender<VideoFrame>,
    position: &AtomicU64,
) -> bool {
    // One decoder thread and one filter thread: a 768 px H.264 stream needs
    // a fraction of one core, and ffmpeg would otherwise start a thread per
    // core for it.
    let mut command = crate::process::async_command("ffmpeg");
    command.args(["-v", "error", "-nostdin", "-threads", "1"]);
    // The picture comes down from VA-API as NV12 at the drawn size: a third
    // of the CPU of decoding a 768 px cover in software.
    let filter = if hardware {
        command.args(["-hwaccel", "vaapi", "-hwaccel_output_format", "vaapi"]);
        format!(
            "fps={rate:.4},scale_vaapi=w={side}:h={side}:force_original_aspect_ratio=increase:format=nv12,hwdownload,format=nv12,format=bgra,crop={side}:{side}"
        )
    } else {
        // Apple's covers are at most 768 px. At half that or less the scaler
        // averages deblocking artifacts away, so the decoder skips that
        // pass, about a sixth of its time.
        if side <= 384 {
            command.args(["-skip_loop_filter", "all"]);
        }
        format!(
            "fps={rate:.4},scale={side}:{side}:force_original_aspect_ratio=increase:flags=bilinear,crop={side}:{side}"
        )
    };
    let spawned = command
        .args(["-stream_loop", "-1"])
        .args(["-ss", &format!("{start:.3}"), "-i"])
        .arg(path)
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
            return true;
        }
    };
    let Some(mut stdout) = child.stdout.take() else {
        return false;
    };
    let frame_len = side as usize * side as usize * 4;
    let frame_time = Duration::from_secs_f64(1. / rate);
    let began = Instant::now();
    let mut index: u32 = 0;
    loop {
        let Some(bgra) = read_frame(&mut stdout, frame_len).await else {
            return index > 0;
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
            Err(mpsc::error::TrySendError::Closed(_)) => return true,
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
            let status = crate::process::command("ffmpeg")
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
            (12., 1.5),
            false,
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
}
