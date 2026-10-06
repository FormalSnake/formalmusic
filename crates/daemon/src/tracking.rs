//! Playback reporting, the pings that put a play into History and feed
//! recommendations. The parameters follow yt-dlp's `_mark_watched`; the
//! cadence follows the player response itself, which asks for watchtime
//! flushes at `videostatsScheduledFlushWalltimeSeconds` (10, 20 and 30 s) and
//! then every `videostatsDefaultFlushIntervalSeconds` (40 s).

use formalmusic_innertube::PlaybackTracking;
use sha1::{Digest, Sha1};
use std::hash::{BuildHasher, RandomState};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const SCHEDULED_FLUSH_MS: [u64; 3] = [10_000, 20_000, 30_000];
const FLUSH_INTERVAL_MS: u64 = 40_000;
/// Position steps larger than this are seeks, not playback.
const MAX_STEP_MS: u64 = 2_000;
const ORIGIN: &str = "https://music.youtube.com";
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";

/// One play of one track, from start to the moment it stops being current.
#[derive(Debug)]
pub struct Watch {
    tracking: PlaybackTracking,
    cpn: String,
    started: Instant,
    last_ms: u64,
    segment_start_ms: u64,
    /// Played ranges since the last flush, closed by seeks.
    segments: Vec<(u64, u64)>,
    watched_ms: u64,
    flushes: usize,
}

impl Watch {
    pub fn new(tracking: PlaybackTracking, position_ms: u64) -> Self {
        Self {
            tracking,
            cpn: client_playback_nonce(),
            started: Instant::now(),
            last_ms: position_ms,
            segment_start_ms: position_ms,
            segments: Vec::new(),
            watched_ms: 0,
            flushes: 0,
        }
    }

    /// The ping that registers the play.
    pub fn playback_url(&self) -> String {
        format!(
            "{}&ver=2&cpn={}&cmt={}&rt=0",
            self.tracking.playback_url,
            self.cpn,
            secs(self.last_ms)
        )
    }

    /// Feeds a position report; returns a watchtime URL when a flush is due.
    pub fn on_position(&mut self, position_ms: u64) -> Option<String> {
        let step = position_ms.wrapping_sub(self.last_ms);
        if position_ms >= self.last_ms && step <= MAX_STEP_MS {
            self.watched_ms += step;
        } else {
            self.close_segment();
            self.segment_start_ms = position_ms;
        }
        self.last_ms = position_ms;
        if self.watched_ms >= self.next_flush_ms() {
            self.flushes += 1;
            return self.flush("playing");
        }
        None
    }

    /// The last watchtime ping, when the track stops being current.
    pub fn finish(mut self) -> Option<String> {
        self.flush("paused")
    }

    fn next_flush_ms(&self) -> u64 {
        match SCHEDULED_FLUSH_MS.get(self.flushes) {
            Some(ms) => *ms,
            None => {
                SCHEDULED_FLUSH_MS[SCHEDULED_FLUSH_MS.len() - 1]
                    + FLUSH_INTERVAL_MS * (self.flushes + 1 - SCHEDULED_FLUSH_MS.len()) as u64
            }
        }
    }

    fn close_segment(&mut self) {
        if self.last_ms > self.segment_start_ms {
            self.segments.push((self.segment_start_ms, self.last_ms));
        }
    }

    fn flush(&mut self, state: &str) -> Option<String> {
        let base = self.tracking.watchtime_url.clone()?;
        self.close_segment();
        if self.segments.is_empty() {
            return None;
        }
        let list = |pick: fn(&(u64, u64)) -> u64| {
            self.segments
                .iter()
                .map(|s| secs(pick(s)))
                .collect::<Vec<_>>()
                .join(",")
        };
        let url = format!(
            "{base}&ver=2&cpn={}&cmt={}&st={}&et={}&state={state}&rt={}",
            self.cpn,
            secs(self.last_ms),
            list(|s| s.0),
            list(|s| s.1),
            secs(self.started.elapsed().as_millis() as u64),
        );
        self.segments.clear();
        self.segment_start_ms = self.last_ms;
        Some(url)
    }
}

fn secs(ms: u64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

/// The 16-character client playback nonce the web player makes up per play.
fn client_playback_nonce() -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let random = RandomState::new();
    (0..16u64)
        .map(|i| ALPHABET[(random.hash_one(i) & 63) as usize] as char)
        .collect()
}

/// Sends the pings with the headers the player response asks for
/// (`USER_AUTH`, `PLUS_PAGE_ID`): the session cookies, a SAPISIDHASH and the
/// brand account.
#[derive(Clone)]
pub struct Reporter {
    http: reqwest::Client,
}

impl Reporter {
    pub fn new() -> anyhow::Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder().user_agent(USER_AGENT).build()?,
        })
    }

    pub async fn ping(&self, url: &str, cookies: &str, page_id: Option<&str>) {
        let mut request = self
            .http
            .get(url)
            .header(reqwest::header::COOKIE, cookies)
            .header("Origin", ORIGIN)
            .header("Referer", format!("{ORIGIN}/"))
            .header("X-Goog-AuthUser", "0");
        if let Some(auth) = authorization(cookies) {
            request = request.header(reqwest::header::AUTHORIZATION, auth);
        }
        if let Some(page_id) = page_id {
            request = request.header("X-Goog-PageId", page_id);
        }
        match request.send().await {
            Ok(response) if response.status().is_success() => {
                tracing::debug!(
                    url = url.split('?').next().unwrap_or(url),
                    "reported playback"
                )
            }
            Ok(response) => tracing::warn!(status = %response.status(), "playback report refused"),
            Err(e) => tracing::warn!("playback report failed: {e}"),
        }
    }
}

/// `SAPISIDHASH <ts>_<sha1("<ts> <SAPISID> <origin>")>`.
fn authorization(cookies: &str) -> Option<String> {
    let value = |name: &str| {
        cookies.split(';').find_map(|pair| {
            let (key, value) = pair.trim().split_once('=')?;
            (key == name).then_some(value)
        })
    };
    let sapisid = value("SAPISID").or_else(|| value("__Secure-3PAPISID"))?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let digest = Sha1::digest(format!("{now} {sapisid} {ORIGIN}").as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    Some(format!("SAPISIDHASH {now}_{hex}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watch() -> Watch {
        Watch::new(
            PlaybackTracking {
                video_id: "v".into(),
                playback_url: "https://s.youtube.com/api/stats/playback?docid=v&len=200".into(),
                watchtime_url: Some(
                    "https://s.youtube.com/api/stats/watchtime?docid=v&len=200".into(),
                ),
                loudness_db: None,
            },
            0,
        )
    }

    fn param<'a>(url: &'a str, key: &str) -> &'a str {
        url.split(['?', '&'])
            .find_map(|p| p.strip_prefix(&format!("{key}=")))
            .unwrap()
    }

    #[test]
    fn flushes_at_10_20_30_then_every_40_seconds() {
        let mut w = watch();
        let mut flushed_at = Vec::new();
        for ms in (250..=120_000).step_by(250) {
            if let Some(url) = w.on_position(ms) {
                flushed_at.push(ms);
                assert_eq!(param(&url, "state"), "playing");
            }
        }
        assert_eq!(flushed_at, [10_000, 20_000, 30_000, 70_000, 110_000]);
    }

    #[test]
    fn seeks_split_the_watched_ranges() {
        let mut w = watch();
        for ms in (250..=4_000).step_by(250) {
            assert!(w.on_position(ms).is_none());
        }
        w.on_position(60_000);
        for ms in (60_250..=63_000).step_by(250) {
            w.on_position(ms);
        }
        let url = w.finish().unwrap();
        assert_eq!(param(&url, "st"), "0.000,60.000");
        assert_eq!(param(&url, "et"), "4.000,63.000");
        assert_eq!(param(&url, "cmt"), "63.000");
        assert_eq!(param(&url, "ver"), "2");
        assert_eq!(param(&url, "cpn").len(), 16);
        assert_eq!(param(&url, "state"), "paused");
    }

    #[test]
    fn playback_ping_shares_the_nonce() {
        let mut w = watch();
        let start = w.playback_url();
        assert_eq!(param(&start, "docid"), "v");
        for ms in (250..=10_000).step_by(250) {
            if let Some(url) = w.on_position(ms) {
                assert_eq!(param(&url, "cpn"), param(&start, "cpn"));
            }
        }
    }

    #[test]
    fn hash_needs_sapisid() {
        assert!(authorization("SID=1").is_none());
        assert!(
            authorization("SID=1; SAPISID=abc")
                .unwrap()
                .starts_with("SAPISIDHASH ")
        );
    }
}
