//! Stream resolution through yt-dlp, cached until the signed URL expires.

use crate::config::{Quality, write_private};
use crate::session::netscape_cookies;
use formalmusic_player::{Codec, StreamSource};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::OnceCell;

/// yt-dlp normally answers in one to three seconds; past this it is stuck.
const TIMEOUT: Duration = Duration::from_secs(60);
/// A URL this close to expiry is resolved again rather than started.
const EXPIRY_MARGIN: Duration = Duration::from_secs(10 * 60);

type Slot = Arc<OnceCell<Result<StreamSource, String>>>;

pub struct Resolver {
    program: PathBuf,
    cookie_dir: PathBuf,
    quality: Quality,
    cache: Mutex<HashMap<String, Slot>>,
    runs: AtomicU64,
}

impl Resolver {
    pub fn new(cookie_dir: PathBuf, quality: Quality) -> Self {
        let program = std::env::var_os("FORMALMUSIC_YTDLP")
            .map(PathBuf::from)
            .unwrap_or_else(|| "yt-dlp".into());
        Self {
            program,
            cookie_dir,
            quality,
            cache: Mutex::new(HashMap::new()),
            runs: AtomicU64::new(0),
        }
    }

    /// The stream for `video_id`. Concurrent calls for one id share a single
    /// yt-dlp run; failures are not cached.
    pub async fn resolve(
        &self,
        video_id: &str,
        cookies: Option<&str>,
    ) -> Result<StreamSource, String> {
        let slot = {
            let mut cache = self.cache.lock();
            cache.retain(|_, slot| match slot.get() {
                Some(Ok(source)) => !source.expires_within(EXPIRY_MARGIN),
                Some(Err(_)) => false,
                None => true,
            });
            cache.entry(video_id.to_owned()).or_default().clone()
        };
        let result = slot
            .get_or_init(|| self.run(video_id, cookies))
            .await
            .clone();
        if result.is_err() {
            let mut cache = self.cache.lock();
            if cache.get(video_id).is_some_and(|s| Arc::ptr_eq(s, &slot)) {
                cache.remove(video_id);
            }
        }
        result
    }

    /// Forgets a URL googlevideo refused, so the next resolve runs yt-dlp.
    pub fn invalidate(&self, video_id: &str) {
        self.cache.lock().remove(video_id);
    }

    /// Drops every cached URL; they were signed for the previous account.
    pub fn clear(&self) {
        self.cache.lock().clear();
    }

    async fn run(&self, video_id: &str, cookies: Option<&str>) -> Result<StreamSource, String> {
        let url = format!("https://music.youtube.com/watch?v={video_id}");
        let mut command = Command::new(&self.program);
        command
            .args(["-J", "--no-playlist", "--no-warnings", "--no-progress"])
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null());
        // yt-dlp writes the jar back when it exits, so each run gets its own copy.
        let cookie_file = match cookies {
            Some(header) => {
                let n = self.runs.fetch_add(1, Ordering::Relaxed);
                let path = self
                    .cookie_dir
                    .join(format!("cookies-{}-{n}.txt", std::process::id()));
                write_private(&path, netscape_cookies(header).as_bytes(), 0o600)
                    .map_err(|e| format!("writing the cookie file: {e}"))?;
                command.arg("--cookies").arg(&path);
                Some(RemoveOnDrop(path))
            }
            None => None,
        };
        command.arg(&url);

        let started = std::time::Instant::now();
        let output = tokio::time::timeout(TIMEOUT, command.output())
            .await
            .map_err(|_| format!("yt-dlp took longer than {}s", TIMEOUT.as_secs()))?
            .map_err(|e| format!("running {}: {e}", self.program.display()))?;
        drop(cookie_file);
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let reason = stderr
                .lines()
                .rev()
                .find(|l| l.starts_with("ERROR"))
                .unwrap_or(stderr.trim());
            return Err(format!("yt-dlp: {reason}"));
        }
        let info: Value = serde_json::from_slice(&output.stdout)
            .map_err(|e| format!("yt-dlp printed invalid JSON: {e}"))?;
        let source = pick_format(&info, self.quality)
            .ok_or_else(|| "no playable audio format".to_owned())?;
        tracing::debug!(video_id, format = %source.label(), elapsed_ms = started.elapsed().as_millis() as u64, "resolved");
        Ok(source)
    }
}

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
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

    #[tokio::test]
    async fn failed_runs_are_not_cached() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("yt-dlp");
        let count = dir.path().join("count");
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho x >> {}\necho 'ERROR: [youtube] x: Video unavailable' >&2\nexit 1\n", count.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let mut resolver = Resolver::new(dir.path().to_owned(), Quality::High);
        resolver.program = script;
        let err = resolver.resolve("abc", Some("SID=1")).await.unwrap_err();
        assert_eq!(err, "yt-dlp: ERROR: [youtube] x: Video unavailable");
        resolver.resolve("abc", None).await.unwrap_err();
        assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 2);
        let leftovers = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("cookies-")
            })
            .count();
        assert_eq!(leftovers, 0);
    }
}
