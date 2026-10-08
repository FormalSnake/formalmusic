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
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
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
    let output = formalmusic_api::process::async_command("ffprobe")
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
    let mut command = formalmusic_api::process::async_command("ffmpeg");
    command.args(["-v", "error", "-nostdin", "-threads", "1"]);
    // As for music videos, the picture comes down from VA-API as NV12 at the
    // drawn size: a third of the CPU of decoding a 768 px cover in software.
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

/// Where the audio is, in seconds into the video, or `None` once it stops
/// playing this video. Read on the decoder's task for every frame.
pub type Clock = Arc<dyn Fn() -> Option<f64> + Send + Sync>;

/// Frames this close to the audio's place are shown as they come.
const SYNC_TOLERANCE: f64 = 0.040;
/// Further off than this, ffmpeg starts again where the audio is instead of
/// catching up frame by frame.
const RESEEK_AFTER: f64 = 1.5;
/// How far ahead of the audio a fresh ffmpeg starts, to cover opening the
/// stream, until one run has shown how long that takes (see [`Lead`]).
const FIRST_LEAD: f64 = 1.0;
const MIN_LEAD: f64 = 0.2;
const MAX_LEAD: f64 = 8.;
/// Added to a measured start-up, which varies from run to run.
const LEAD_MARGIN: f64 = 0.15;
/// Each frame costs a decode, a copy to the GPU and a redraw of the window;
/// past film rate a player box gains little for that.
const MAX_SYNCED_FPS: f64 = 24.;
/// Starts in a row that produced no frame before giving up.
const MAX_FAILED_STARTS: u32 = 3;

/// How far ahead of the audio ffmpeg starts, learned from how long its first
/// frame took. Kept across runs of one stream, so a picture that starts again
/// (the window shown again, a seek) waits about as long as ffmpeg needs and
/// not the full first guess.
#[derive(Clone)]
pub struct Lead(Arc<AtomicU64>);

impl Default for Lead {
    fn default() -> Self {
        Lead(Arc::new(AtomicU64::new(FIRST_LEAD.to_bits())))
    }
}

impl Lead {
    fn get(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }

    fn set(&self, lead: f64) {
        self.0
            .store(lead.clamp(MIN_LEAD, MAX_LEAD).to_bits(), Ordering::Relaxed);
    }
}

#[derive(Default)]
struct Control {
    /// The running ffmpeg, 0 between runs.
    pid: AtomicU32,
    paused: AtomicBool,
}

/// A music video decoded muted beside the daemon's audio, held to its clock.
pub struct Synced {
    task: JoinHandle<()>,
    control: Arc<Control>,
}

impl Synced {
    /// Decodes `stream` at `(width, height)` from wherever `clock` says,
    /// showing each frame when the audio reaches it, dropping frames that
    /// come late and starting over after a seek. With `hardware`, ffmpeg
    /// decodes through VA-API and falls back to software if that fails. The
    /// channel closes when the stream cannot be read, which usually means
    /// googlevideo refused the URL.
    pub fn start(
        runtime: &tokio::runtime::Handle,
        stream: &VideoStream,
        (width, height): (u32, u32),
        hardware: bool,
        lead: &Lead,
        clock: Clock,
        frames: mpsc::Sender<VideoFrame>,
    ) -> Synced {
        let size = (width.max(2) & !1, height.max(2) & !1);
        let control = Arc::new(Control::default());
        let run = Run {
            stream: stream.clone(),
            size,
            lead: lead.clone(),
            clock,
            frames,
            control: control.clone(),
        };
        let task = runtime.spawn(run_synced(run, hardware));
        Synced { task, control }
    }

    /// Stops ffmpeg where it is, for a picture nobody sees for a moment.
    /// After [`Synced::resume`] its first frames are late and dropped until
    /// it has caught up with the audio, or it starts again at the audio's
    /// place when that is too far.
    pub fn pause(&self) {
        self.control.paused.store(true, Ordering::Relaxed);
        self.control.signal(Signal::Stop);
    }

    pub fn resume(&self) {
        self.control.paused.store(false, Ordering::Relaxed);
        self.control.signal(Signal::Continue);
    }
}

enum Signal {
    Stop,
    Continue,
}

impl Control {
    fn signal(&self, signal: Signal) {
        let pid = self.pid.load(Ordering::Relaxed);
        if pid == 0 {
            return;
        }
        #[cfg(unix)]
        {
            let signal = match signal {
                Signal::Stop => libc::SIGSTOP,
                Signal::Continue => libc::SIGCONT,
            };
            // SAFETY: kill(2) takes plain integers. `pid` is a child that
            // has not been reaped (see `Child`), so it is not reused.
            unsafe {
                libc::kill(pid as libc::pid_t, signal);
            }
        }
        // Windows has no SIGSTOP; ntdll suspends every thread of the process.
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
            use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SUSPEND_RESUME};
            #[link(name = "ntdll")]
            unsafe extern "system" {
                fn NtSuspendProcess(process: HANDLE) -> i32;
                fn NtResumeProcess(process: HANDLE) -> i32;
            }
            // SAFETY: `pid` is a child whose handle `Child` still holds, so
            // the id is not reused; the opened handle is closed here.
            unsafe {
                let process = OpenProcess(PROCESS_SUSPEND_RESUME, 0, pid);
                if !process.is_null() {
                    match signal {
                        Signal::Stop => NtSuspendProcess(process),
                        Signal::Continue => NtResumeProcess(process),
                    };
                    CloseHandle(process);
                }
            }
        }
    }
}

/// An ffmpeg run's process, published for [`Synced::pause`] while it lives.
struct Child {
    child: tokio::process::Child,
    control: Arc<Control>,
}

impl Child {
    fn new(child: tokio::process::Child, control: Arc<Control>) -> Child {
        control
            .pid
            .store(child.id().unwrap_or(0), Ordering::Relaxed);
        if control.paused.load(Ordering::Relaxed) {
            control.signal(Signal::Stop);
        }
        Child { child, control }
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        self.control.pid.store(0, Ordering::Relaxed);
    }
}

/// What every ffmpeg run of one [`Synced`] shares.
struct Run {
    stream: VideoStream,
    size: (u32, u32),
    lead: Lead,
    clock: Clock,
    frames: mpsc::Sender<VideoFrame>,
    control: Arc<Control>,
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

async fn run_synced(run: Run, mut hardware: bool) {
    let rate = run.stream.fps.clamp(1., MAX_SYNCED_FPS);
    let mut failed = 0;
    loop {
        match decode_from(&run, rate, hardware).await {
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
            Ended::Reseek { late: Some(late) } => run.lead.set(run.lead.get() + late + 0.25),
            Ended::Reseek { late: None } => {}
        }
    }
}

/// One ffmpeg run, from the lead past where the audio is now.
async fn decode_from(run: &Run, rate: f64, hardware: bool) -> Ended {
    let Run {
        stream,
        size: (width, height),
        clock,
        frames,
        ..
    } = run;
    let (width, height) = (*width, *height);
    let lead = run.lead.get();
    let Some(now) = clock() else {
        return Ended::Done;
    };
    let start = now + lead;
    let mut command = formalmusic_api::process::async_command("ffmpeg");
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
        Ok(child) => Child::new(child, run.control.clone()),
        Err(error) => {
            tracing::warn!("music video: ffmpeg: {error}");
            return Ended::Done;
        }
    };
    let Some(mut stdout) = child.child.stdout.take() else {
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
        if first_frame && let Some(now) = clock() {
            // Early by `at - now`, so starting took the rest of the lead.
            let took = lead - (at - now);
            if took < lead {
                run.lead.set(took + LEAD_MARGIN);
            }
        }
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
            let status = formalmusic_api::process::command("ffmpeg")
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

    /// Eight seconds at 30 fps, rendered by ffmpeg under target/ once.
    fn long_fixture() -> Option<VideoStream> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-video");
        let path = dir.join("long.mp4");
        if !path.exists() {
            let made = std::fs::create_dir_all(&dir).is_ok()
                && formalmusic_api::process::command("ffmpeg")
                    .args(["-v", "error", "-nostdin", "-y", "-f", "lavfi"])
                    .args(["-i", "testsrc2=size=160x90:rate=30:duration=8"])
                    .args(["-c:v", "mpeg4", "-f", "mp4"])
                    .arg(&path)
                    .status()
                    .is_ok_and(|s| s.success());
            if !made {
                return None;
            }
        }
        Some(VideoStream {
            url: path.to_string_lossy().into_owned(),
            headers: Vec::new(),
            width: 160,
            height: 90,
            fps: 30.,
            codec: "mp4v".into(),
        })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_restart_or_a_resume_shows_a_frame_without_the_first_lead() {
        let Some(stream) = long_fixture() else { return };
        let began = Instant::now();
        let clock: Clock = Arc::new(move || Some(0.5 + began.elapsed().as_secs_f64()));
        let lead = Lead::default();
        let start = |lead: &Lead| {
            let (tx, rx) = mpsc::channel(2);
            let run = Synced::start(
                &tokio::runtime::Handle::current(),
                &stream,
                (80, 45),
                false,
                lead,
                clock.clone(),
                tx,
            );
            (run, rx)
        };
        let (run, mut rx) = start(&lead);
        rx.recv().await.expect("first run");
        drop(run);
        assert!(lead.get() < 0.5, "learned lead {}", lead.get());

        let (run, mut rx) = start(&lead);
        let restarted = Instant::now();
        rx.recv().await.expect("second run");
        let waited = restarted.elapsed().as_secs_f64();
        assert!(waited < 0.5, "restart took {waited:.2}s");

        run.pause();
        tokio::time::sleep(Duration::from_millis(800)).await;
        while rx.try_recv().is_ok() {}
        run.resume();
        let resumed = Instant::now();
        rx.recv().await.expect("resumed run");
        let waited = resumed.elapsed().as_secs_f64();
        assert!(waited < 0.3, "resume took {waited:.2}s");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn synced_frames_follow_the_clock_and_stop_with_it() {
        let Some(stream) = long_fixture() else { return };
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
            (80, 45),
            false,
            &Lead::default(),
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
