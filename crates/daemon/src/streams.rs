//! Stream resolution through yt-dlp, cached until the signed URL expires.
//! One run gives the audio the daemon plays and the video-only formats a
//! client may decode beside it.

mod worker;

use crate::config::{Quality, write_private};
use crate::session::{Session, netscape_cookies};
use formalmusic_api::VideoStream;
use formalmusic_player::{Codec, StreamSource};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OnceCell, broadcast};
use worker::Worker;

/// A warm yt-dlp answers in well under two seconds; past this it is stuck.
const TIMEOUT: Duration = Duration::from_secs(60);
/// A URL this close to expiry is resolved again rather than started.
const EXPIRY_MARGIN: Duration = Duration::from_secs(10 * 60);
/// yt-dlp runs per track before giving up on a URL that is not gated.
const MAX_ATTEMPTS: usize = 3;
/// Bytes asked for at the end of the stream to check the whole of it is served.
const PROBE_BYTES: u64 = 1024;

/// Heights a client's video request rounds up to. Past 720 an iGPU spends
/// more on decoding than the player box can show.
const VIDEO_HEIGHTS: [u32; 3] = [360, 480, 720];

#[derive(Clone)]
struct Resolved {
    audio: StreamSource,
    video: Arc<Vec<VideoFormat>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VideoFormat {
    pub stream: VideoStream,
    pub content_length: Option<u64>,
}

type Slot = Arc<OnceCell<Result<Resolved, String>>>;

pub struct Resolver {
    inner: Arc<Inner>,
}

struct Inner {
    worker: Worker,
    state_dir: PathBuf,
    session: Arc<Session>,
    quality: Quality,
    cache: Mutex<HashMap<String, Slot>>,
    /// The private copy of the session cookies yt-dlp reads, and the header
    /// it was written from.
    cookie_file: Mutex<Option<(String, PathBuf)>>,
    /// URLs googlevideo refused the end of.
    gated: Mutex<HashSet<String>>,
    gated_tx: broadcast::Sender<String>,
    http: reqwest::Client,
}

/// `$FORMALMUSIC_YTDLP`, else `yt-dlp` from `PATH`.
pub fn ytdlp_program() -> PathBuf {
    std::env::var_os("FORMALMUSIC_YTDLP")
        .map(PathBuf::from)
        .unwrap_or_else(|| "yt-dlp".into())
}

impl Resolver {
    /// `state_dir` holds the cookie file handed to yt-dlp; its cache (the
    /// player JS and solved signature functions) lives under the user cache.
    pub fn new(state_dir: PathBuf, session: Arc<Session>, quality: Quality) -> Self {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| state_dir.clone())
            .join("formalmusic")
            .join("yt-dlp");
        Self::with_worker(
            Worker::new(worker::python(), cache_dir),
            state_dir,
            session,
            quality,
        )
    }

    fn with_worker(
        worker: Worker,
        state_dir: PathBuf,
        session: Arc<Session>,
        quality: Quality,
    ) -> Self {
        remove_cookie_files(&state_dir);
        Self {
            inner: Arc::new(Inner {
                worker,
                state_dir,
                session,
                quality,
                cache: Mutex::new(HashMap::new()),
                cookie_file: Mutex::new(None),
                gated: Mutex::new(HashSet::new()),
                gated_tx: broadcast::channel(16).0,
                http: reqwest::Client::new(),
            }),
        }
    }

    /// Starts yt-dlp, so the first track does not wait for Python to load it.
    pub async fn warm(&self) {
        self.inner.worker.warm().await;
    }

    /// The stream for `video_id`. Concurrent calls for one id share a single
    /// yt-dlp run; failures are not cached.
    ///
    /// The URL is handed out before it is checked: the check runs beside
    /// playback, and a URL that fails it is announced on [`Resolver::gated`]
    /// while a good one is resolved in its place.
    pub async fn resolve(
        &self,
        video_id: &str,
        cookies: Option<&str>,
    ) -> Result<StreamSource, String> {
        Ok(self.resolved(video_id, cookies).await?.audio)
    }

    /// A video-only stream of `video_id` at most `max_height` tall, rounded
    /// up to a step in [`VIDEO_HEIGHTS`], from the same yt-dlp run as the
    /// audio. A gated URL is resolved again, as for audio.
    pub async fn video(
        &self,
        video_id: &str,
        cookies: Option<&str>,
        max_height: u32,
    ) -> Result<VideoStream, String> {
        let cap = VIDEO_HEIGHTS
            .into_iter()
            .find(|h| *h >= max_height)
            .unwrap_or(VIDEO_HEIGHTS[VIDEO_HEIGHTS.len() - 1]);
        for _ in 0..MAX_ATTEMPTS {
            let resolved = self.resolved(video_id, cookies).await?;
            let format = pick_video(&resolved.video, cap)
                .ok_or_else(|| "no video-only format".to_owned())?;
            let stream = &format.stream;
            if !self
                .inner
                .gates(
                    video_id,
                    &stream.url,
                    &stream.headers,
                    format.content_length,
                )
                .await
            {
                return Ok(format.stream.clone());
            }
            self.invalidate(video_id);
        }
        Err(format!(
            "googlevideo refused the end of the video {MAX_ATTEMPTS} times"
        ))
    }

    async fn resolved(&self, video_id: &str, cookies: Option<&str>) -> Result<Resolved, String> {
        let inner = &self.inner;
        let slot = {
            let mut cache = inner.cache.lock();
            cache.retain(|_, slot| match slot.get() {
                Some(Ok(resolved)) => !resolved.audio.expires_within(EXPIRY_MARGIN),
                Some(Err(_)) => false,
                None => true,
            });
            cache.entry(video_id.to_owned()).or_default().clone()
        };
        let mut fresh = false;
        let result = slot
            .get_or_init(|| {
                fresh = true;
                inner.run_once(video_id, cookies)
            })
            .await
            .clone();
        match &result {
            Ok(resolved) if fresh => {
                tokio::spawn(Inner::check(
                    inner.clone(),
                    video_id.to_owned(),
                    cookies.map(str::to_owned),
                    slot,
                    resolved.audio.clone(),
                ));
            }
            Ok(_) => {}
            Err(_) => {
                let mut cache = inner.cache.lock();
                if cache.get(video_id).is_some_and(|s| Arc::ptr_eq(s, &slot)) {
                    cache.remove(video_id);
                }
            }
        }
        result
    }

    /// Video ids whose URL googlevideo gates; the next resolve of one waits
    /// for a URL that passed the check.
    pub fn gated(&self) -> broadcast::Receiver<String> {
        self.inner.gated_tx.subscribe()
    }

    /// True when `source` failed the check after it was handed out.
    pub fn is_gated(&self, source: &StreamSource) -> bool {
        self.inner.gated.lock().contains(&source.url)
    }

    /// Forgets a URL googlevideo refused, so the next resolve runs yt-dlp.
    /// A resolve still running, such as the one replacing a gated URL, is
    /// newer than the refused URL and stays.
    pub fn invalidate(&self, video_id: &str) {
        let mut cache = self.inner.cache.lock();
        if cache.get(video_id).is_some_and(|slot| slot.initialized()) {
            cache.remove(video_id);
        }
    }

    /// Drops every cached URL and the cookie copy; they belong to the
    /// previous account.
    pub fn clear(&self) {
        self.inner.cache.lock().clear();
        self.inner.gated.lock().clear();
        if let Some((_, path)) = self.inner.cookie_file.lock().take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl Inner {
    /// Some of the URLs yt-dlp's default client (`c=VISIONOS` as of
    /// 2026.08.19) gets for a signed-out session are gated behind a GVS PO
    /// token that yt-dlp does not know is needed: googlevideo serves the
    /// first ~65 s of audio (about 1.1 MB) and answers 403 to any range past
    /// it. About one run in ten is gated, at random, so asking again works.
    /// A gated URL is replaced in the cache by one that passed this check
    /// before anyone sees it.
    async fn check(
        self: Arc<Self>,
        video_id: String,
        cookies: Option<String>,
        slot: Slot,
        source: StreamSource,
    ) {
        if !self
            .gates(
                &video_id,
                &source.url,
                &source.headers,
                source.content_length,
            )
            .await
        {
            return;
        }
        let retry: Slot = Arc::default();
        {
            let mut gated = self.gated.lock();
            if gated.len() > 64 {
                gated.clear();
            }
            gated.insert(source.url.clone());
            let mut cache = self.cache.lock();
            match cache.get(&video_id) {
                Some(current) if Arc::ptr_eq(current, &slot) => {
                    cache.insert(video_id.clone(), retry.clone());
                }
                _ => return,
            }
        }
        let _ = self.gated_tx.send(video_id.clone());
        let result = retry
            .get_or_init(|| async {
                for _ in 1..MAX_ATTEMPTS {
                    let resolved = self.run_once(&video_id, cookies.as_deref()).await?;
                    let audio = &resolved.audio;
                    if !self
                        .gates(&video_id, &audio.url, &audio.headers, audio.content_length)
                        .await
                    {
                        return Ok(resolved);
                    }
                }
                Err(format!(
                    "googlevideo refused the end of the stream {MAX_ATTEMPTS} times"
                ))
            })
            .await;
        if result.is_err() {
            let mut cache = self.cache.lock();
            if cache.get(&video_id).is_some_and(|s| Arc::ptr_eq(s, &retry)) {
                cache.remove(&video_id);
            }
        }
    }

    /// True when googlevideo refuses the last bytes of the stream. A probe
    /// that fails for any other reason passes: the player retries its own
    /// network errors.
    async fn gates(
        &self,
        video_id: &str,
        url: &str,
        headers: &[(String, String)],
        content_length: Option<u64>,
    ) -> bool {
        let Some(len) = content_length.filter(|len| *len > PROBE_BYTES) else {
            return false;
        };
        let mut request = self.http.get(url).header(
            reqwest::header::RANGE,
            format!("bytes={}-{}", len - PROBE_BYTES, len - 1),
        );
        for (name, value) in headers {
            request = request.header(name, value);
        }
        match request.send().await {
            Ok(response) => {
                let status = response.status();
                let gated =
                    status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::GONE;
                if gated {
                    tracing::info!(
                        video_id,
                        "googlevideo gates this url after the first megabyte, resolving again"
                    );
                }
                gated
            }
            Err(e) => {
                tracing::debug!(video_id, "could not probe the stream: {e}");
                false
            }
        }
    }

    async fn run_once(&self, video_id: &str, cookies: Option<&str>) -> Result<Resolved, String> {
        let cookie_file = cookies.map(|header| self.cookie_file(header)).transpose()?;
        let premium = cookies.is_some() && self.session.info().premium;
        let started = std::time::Instant::now();
        let answer = self
            .worker
            .info(video_id, cookie_file.as_deref(), premium, TIMEOUT)
            .await
            .map_err(|e| format!("yt-dlp: {e}"))?;
        if !answer.cookies.is_empty() {
            self.session.merge_cookies(&answer.cookies);
        }
        let source = pick_format(&answer.info, self.quality)
            .ok_or_else(|| "no playable audio format".to_owned())?;
        tracing::debug!(video_id, premium, format = %source.label(), elapsed_ms = started.elapsed().as_millis() as u64, "resolved");
        Ok(Resolved {
            audio: source,
            video: Arc::new(video_formats(&answer.info)),
        })
    }

    /// The daemon's own 0600 copy of the session cookies for yt-dlp, written
    /// once per session. yt-dlp rewrites any jar it is given, so it never
    /// sees the session file or a file the user handed over.
    fn cookie_file(&self, header: &str) -> Result<PathBuf, String> {
        let mut current = self.cookie_file.lock();
        if let Some((written, path)) = current.as_ref()
            && written == header
            && path.exists()
        {
            return Ok(path.clone());
        }
        if let Some((_, old)) = current.take() {
            let _ = std::fs::remove_file(old);
        }
        let path = self
            .state_dir
            .join(format!("ytdlp-cookies-{:016x}.txt", fastrand::u64(..)));
        write_private(&path, netscape_cookies(header).as_bytes(), 0o600)
            .map_err(|e| format!("writing the cookie file: {e}"))?;
        *current = Some((header.to_owned(), path.clone()));
        Ok(path)
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        remove_cookie_files(&self.state_dir);
    }
}

/// Cookie copies left by an earlier run that did not exit cleanly.
fn remove_cookie_files(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("ytdlp-cookies-") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// The player crate ranks formats for the best quality; lower settings pick
/// the closest Opus (or AAC) bitrate below a cap instead.
pub fn pick_format(info: &Value, quality: Quality) -> Option<StreamSource> {
    let cap = match quality {
        Quality::High => return StreamSource::best_from_ytdlp(info),
        Quality::Normal => 160,
        Quality::Low => 64,
    };
    let formats = info.get("formats")?.as_array()?;
    let candidates: Vec<StreamSource> = formats
        .iter()
        .filter_map(StreamSource::from_ytdlp_format)
        .collect();
    let rank = |s: &StreamSource| (s.codec == Codec::Opus, s.bitrate_kbps.unwrap_or(0));
    candidates
        .iter()
        .filter(|s| s.codec != Codec::Other && s.bitrate_kbps.is_some_and(|b| b <= cap))
        .max_by_key(|s| rank(s))
        .or_else(|| {
            candidates
                .iter()
                .min_by_key(|s| s.bitrate_kbps.unwrap_or(u32::MAX))
        })
        .cloned()
}

/// The video-only https formats of a `yt-dlp -J` document.
pub fn video_formats(info: &Value) -> Vec<VideoFormat> {
    let formats = info.get("formats").and_then(Value::as_array);
    formats
        .into_iter()
        .flatten()
        .filter_map(|format| {
            let str_field = |key: &str| format.get(key).and_then(Value::as_str);
            let uint = |key: &str| format.get(key).and_then(Value::as_u64);
            let codec = str_field("vcodec").filter(|v| *v != "none")?;
            if str_field("acodec").is_some_and(|a| a != "none")
                || str_field("protocol") != Some("https")
                || format.get("has_drm").and_then(Value::as_bool) == Some(true)
            {
                return None;
            }
            let headers = format
                .get("http_headers")
                .and_then(Value::as_object)
                .map(|h| {
                    h.iter()
                        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                        .collect()
                })
                .unwrap_or_default();
            Some(VideoFormat {
                stream: VideoStream {
                    url: str_field("url")?.to_owned(),
                    headers,
                    width: uint("width")? as u32,
                    height: uint("height")? as u32,
                    fps: format.get("fps").and_then(Value::as_f64).unwrap_or(30.),
                    codec: codec.to_owned(),
                },
                content_length: uint("filesize"),
            })
        })
        .collect()
}

/// The tallest H.264 format within `cap`, since it decodes in hardware or
/// cheaply in software everywhere; any codec within `cap` otherwise, and the
/// shortest format there is when all are taller.
pub fn pick_video(formats: &[VideoFormat], cap: u32) -> Option<&VideoFormat> {
    let avc = |f: &VideoFormat| f.stream.codec.starts_with("avc1");
    let rank = |f: &&VideoFormat| (avc(f), f.stream.height);
    formats
        .iter()
        .filter(|f| f.stream.height <= cap)
        .max_by_key(rank)
        .or_else(|| formats.iter().min_by_key(|f| f.stream.height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../fixtures/ytdlp_IluRBvnYMoY.json")).unwrap()
    }

    #[test]
    fn best_quality_from_recorded_output() {
        let source = pick_format(&fixture(), Quality::High).unwrap();
        assert_eq!(source.codec, Codec::Opus);
        assert!(source.url.contains("itag=251"), "{}", source.url);
        assert_eq!(source.mime, "audio/webm");
        assert_eq!(source.bitrate_kbps, Some(135));
        assert!(source.expires_at.is_some());
        assert!(source.headers.iter().any(|(k, _)| k == "User-Agent"));
    }

    #[test]
    fn lower_qualities_cap_the_bitrate() {
        let low = pick_format(&fixture(), Quality::Low).unwrap();
        assert!(low.url.contains("itag=249"), "{}", low.url);
        let normal = pick_format(&fixture(), Quality::Normal).unwrap();
        assert!(normal.url.contains("itag=251"), "{}", normal.url);
    }

    #[test]
    fn premium_formats_win_only_at_high() {
        let mut info = fixture();
        let mut premium = info["formats"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["format_id"] == "251")
            .unwrap()
            .clone();
        premium["format_id"] = json!("774");
        premium["abr"] = json!(256.0);
        premium["url"] = json!(
            "https://rr1---sn-redacted.googlevideo.com/videoplayback?expire=1791306028&itag=774"
        );
        info["formats"].as_array_mut().unwrap().push(premium);
        assert!(
            pick_format(&info, Quality::High)
                .unwrap()
                .url
                .contains("itag=774")
        );
        assert!(
            pick_format(&info, Quality::Normal)
                .unwrap()
                .url
                .contains("itag=251")
        );
    }

    fn video_info() -> Value {
        let format = |id: &str, vcodec: &str, height: u32| {
            json!({
                "format_id": id,
                "vcodec": vcodec,
                "acodec": "none",
                "protocol": "https",
                "width": height * 16 / 9,
                "height": height,
                "fps": 24,
                "filesize": 1_000_000,
                "url": format!("https://rr1---sn-redacted.googlevideo.com/videoplayback?expire=1791306028&itag={id}"),
                "http_headers": { "User-Agent": "Mozilla/5.0" },
            })
        };
        let mut info = fixture();
        let formats = info["formats"].as_array_mut().unwrap();
        formats.extend([
            format("134", "avc1.4D401E", 360),
            format("135", "avc1.4D401E", 480),
            format("244", "vp9", 480),
            format("136", "avc1.4D401F", 720),
            format("247", "vp9", 720),
            format("137", "avc1.640028", 1080),
            format("248", "vp9", 1080),
        ]);
        // A muxed format has audio and is no use beside the daemon's.
        let mut muxed = format("18", "avc1.42001E", 360);
        muxed["acodec"] = json!("mp4a.40.2");
        formats.push(muxed);
        info
    }

    #[test]
    fn video_formats_skip_audio_and_muxed_streams() {
        let formats = video_formats(&video_info());
        // The recorded run's own 144p format does not say how tall it is.
        assert_eq!(formats.len(), 7);
        assert!(formats.iter().all(|f| !f.stream.url.contains("itag=18")));
        let first = formats.iter().find(|f| f.stream.height == 480).unwrap();
        assert_eq!((first.stream.width, first.stream.fps), (853, 24.));
        assert!(first.stream.headers.iter().any(|(k, _)| k == "User-Agent"));
    }

    #[test]
    fn video_prefers_h264_at_the_tallest_step_within_the_cap() {
        let formats = video_formats(&video_info());
        let itag = |cap| {
            let url = &pick_video(&formats, cap).unwrap().stream.url;
            url.rsplit('=').next().unwrap().to_owned()
        };
        assert_eq!(itag(480), "135");
        assert_eq!(itag(720), "136");
        assert_eq!(itag(360), "134");
        assert_eq!(itag(100), "134");
        let vp9_only: Vec<_> = formats
            .iter()
            .filter(|f| f.stream.codec == "vp9")
            .cloned()
            .collect();
        assert!(
            pick_video(&vp9_only, 720)
                .unwrap()
                .stream
                .url
                .ends_with("247")
        );
    }

    /// A stand-in for the Python worker: `answer` is a shell snippet that
    /// prints one JSON line for the request in `$line` with id `$id`.
    fn fake_worker(dir: &Path, answer: &str) -> Resolver {
        let script = dir.join("python");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nwhile read -r line; do\n  echo \"$line\" >> {log}\n  id=$(echo \"$line\" | sed 's/.*\"id\":\\([0-9]*\\).*/\\1/')\n  n=$(wc -l < {log} | tr -d ' ')\n  {answer}\ndone\n",
                log = dir.join("requests").display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let state = dir.join("state");
        std::fs::create_dir_all(&state).unwrap();
        let session = Arc::new(Session::load(state.join("session.json")).unwrap());
        Resolver::with_worker(
            Worker::new(script, dir.join("cache")),
            state,
            session,
            Quality::High,
        )
    }

    fn requests(dir: &Path) -> Vec<Value> {
        std::fs::read_to_string(dir.join("requests"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn failed_runs_are_not_cached() {
        let dir = tempfile::tempdir().unwrap();
        let resolver = fake_worker(
            dir.path(),
            r#"echo "{\"id\":$id,\"error\":\"ERROR: [youtube] x: Video unavailable\"}""#,
        );
        let err = resolver.resolve("abc", Some("SID=1")).await.unwrap_err();
        assert_eq!(err, "yt-dlp: ERROR: [youtube] x: Video unavailable");
        resolver.resolve("abc", None).await.unwrap_err();
        let requests = requests(dir.path());
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1]["cookies"], Value::Null);

        // yt-dlp gets a 0600 copy in the state dir, gone with the resolver.
        let cookies = PathBuf::from(requests[0]["cookies"].as_str().unwrap());
        assert_eq!(cookies.parent(), Some(dir.path().join("state").as_path()));
        let mode = std::fs::metadata(&cookies).unwrap().permissions();
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777,
            0o600
        );
        assert!(
            std::fs::read_to_string(&cookies)
                .unwrap()
                .contains("\tSID\t1")
        );
        drop(resolver);
        assert!(!cookies.exists());
    }

    /// Serves `bytes=` ranges of a 4 KiB file, refusing the end of `/gated`.
    async fn googlevideo() -> std::net::SocketAddr {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = vec![0; 4096];
                let n = socket.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..n]);
                let status = if request.starts_with("GET /gated") {
                    "403 Forbidden"
                } else {
                    "206 Partial Content"
                };
                let _ = socket
                    .write_all(
                        format!(
                            "HTTP/1.1 {status}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .await;
            }
        });
        addr
    }

    #[tokio::test]
    async fn gated_urls_are_handed_out_then_replaced() {
        let addr = googlevideo().await;
        let dir = tempfile::tempdir().unwrap();
        // The first run gets a gated URL, every later one a good URL.
        let format = |path: &str| {
            json!({ "id": "ID", "info": { "formats": [{
                "format_id": "251", "ext": "webm", "acodec": "opus", "vcodec": "none",
                "protocol": "https", "abr": 130.0, "filesize": 4096,
                "url": format!("http://{addr}/{path}"),
            }]}})
            .to_string()
            .replace('"', "\\\"")
            .replace("\\\"ID\\\"", "$id")
        };
        let resolver = fake_worker(
            dir.path(),
            &format!(
                "if [ \"$n\" = 1 ]; then echo \"{}\"; else echo \"{}\"; fi",
                format("gated"),
                format("good")
            ),
        );
        let mut gated = resolver.gated();
        let first = resolver.resolve("abc", None).await.unwrap();
        assert!(first.url.ends_with("/gated"), "{}", first.url);
        let video_id = tokio::time::timeout(Duration::from_secs(5), gated.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(video_id, "abc");
        assert!(resolver.is_gated(&first));
        let second = resolver.resolve("abc", None).await.unwrap();
        assert!(second.url.ends_with("/good"), "{}", second.url);
        assert!(!resolver.is_gated(&second));
        assert_eq!(requests(dir.path()).len(), 2);
    }
}

/// Against the real YouTube: resolves a radio's worth of tracks the way the
/// daemon does (two ahead of the one starting) and opens each in the player.
/// `cargo test -p formalmusicd -- --ignored --nocapture live_`
#[cfg(test)]
mod live {
    use super::*;
    use formalmusic_player::{OutputKind, Player, PlayerEvent};
    use std::sync::Arc;

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "runs yt-dlp and streams from googlevideo"]
    async fn live_fifty_tracks_open_without_403() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("formalmusic_player=warn,formalmusicd=debug")
            .with_test_writer()
            .try_init();
        let client = formalmusic_innertube::Client::anonymous().unwrap();
        let mut ids: Vec<String> = Vec::new();
        let mut radio = client.radio("IluRBvnYMoY").await.unwrap();
        while ids.len() < 50 {
            ids.extend(radio.tracks.iter().map(|t| t.video_id.clone()));
            ids.dedup();
            let (Some(playlist), Some(token)) = (&radio.playlist_id, &radio.continuation) else {
                break;
            };
            radio = client.next_continuation(playlist, token).await.unwrap();
        }
        ids.truncate(50);

        let dir = tempfile::tempdir().unwrap();
        let session = Arc::new(Session::load(dir.path().join("session.json")).unwrap());
        let resolver = Arc::new(Resolver::new(dir.path().to_owned(), session, Quality::High));
        let player = Player::with_output(OutputKind::Null {
            sample_rate: 48_000,
            channels: 2,
        })
        .unwrap();
        let mut events = player.subscribe();
        let mut failures = Vec::new();
        for (i, id) in ids.iter().enumerate() {
            for ahead in ids.iter().skip(i + 1).take(2) {
                let ahead = ahead.clone();
                let resolver = resolver.clone();
                tokio::spawn(async move { resolver.resolve(&ahead, None).await });
            }
            let source = match resolver.resolve(id, None).await {
                Ok(source) => source,
                Err(e) => {
                    failures.push(format!("{id}: resolve: {e}"));
                    continue;
                }
            };
            let track = player.load(source, 0, None);
            let outcome = tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    match events.recv().await {
                        Ok(PlayerEvent::TrackStarted { track: t, .. }) if t == track => {
                            return Ok(());
                        }
                        Ok(PlayerEvent::Error {
                            track: Some(t),
                            error,
                        }) if t == track => return Err(error.to_string()),
                        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(e) => return Err(e.to_string()),
                    }
                }
            })
            .await
            .unwrap_or_else(|_| Err("no start within 30 s".into()));
            eprintln!(
                "{:2} {id} {}",
                i + 1,
                outcome.as_ref().map_or_else(|e| e.as_str(), |_| "ok")
            );
            if let Err(e) = outcome {
                failures.push(format!("{id}: {e}"));
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} failed: {failures:#?}",
            failures.len(),
            ids.len()
        );
    }
}
