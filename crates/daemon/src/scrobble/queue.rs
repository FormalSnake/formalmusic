//! Plays waiting for a service: written to `scrobble-queue.json` before the
//! first attempt, so a crash or a night offline loses nothing, and sent in
//! batches once the service answers again.

use super::meta::Song;
use crate::config::write_private;
use formalmusic_api::ScrobbleService;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// `track.scrobble` takes at most 50 plays per call.
pub const LASTFM_BATCH: usize = 50;
/// ListenBrainz accepts far more per import; this keeps one request small.
pub const LISTENBRAINZ_BATCH: usize = 100;

const BACKOFF_MIN: Duration = Duration::from_secs(30);
const BACKOFF_MAX: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    pub service: ScrobbleService,
    /// Unix seconds when the play started, which both services take as the
    /// time of the listen.
    pub listened_at: u64,
    pub song: Song,
}

impl Pending {
    fn same_play(&self, other: &Pending) -> bool {
        self.service == other.service
            && self.listened_at == other.listened_at
            && self.song.video_id == other.song.video_id
    }
}

pub struct Queue {
    path: PathBuf,
    items: Vec<Pending>,
    failures: HashMap<ScrobbleService, u32>,
    retry_at: HashMap<ScrobbleService, Instant>,
}

impl Queue {
    pub fn load(path: PathBuf) -> Self {
        let items = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!(path = %path.display(), "ignoring an unreadable scrobble queue: {e}");
                Vec::new()
            }),
            Err(_) => Vec::new(),
        };
        Self {
            path,
            items,
            failures: HashMap::new(),
            retry_at: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Adds plays and writes the queue. A play already queued for the same
    /// service is kept once.
    pub fn push(&mut self, pending: impl IntoIterator<Item = Pending>) {
        for p in pending {
            if !self.items.iter().any(|q| q.same_play(&p)) {
                self.items.push(p);
            }
        }
        self.save();
    }

    /// The oldest plays for `service`, at most `max`, when it is not backing off.
    pub fn batch(&self, service: ScrobbleService, max: usize, now: Instant) -> Vec<Pending> {
        if self.retry_at.get(&service).is_some_and(|at| *at > now) {
            return Vec::new();
        }
        self.items
            .iter()
            .filter(|p| p.service == service)
            .take(max)
            .cloned()
            .collect()
    }

    /// Drops plays the service took or refused for good.
    pub fn remove(&mut self, done: &[Pending]) {
        self.items.retain(|q| !done.iter().any(|d| d.same_play(q)));
        self.save();
    }

    pub fn drop_service(&mut self, service: ScrobbleService) {
        self.items.retain(|q| q.service != service);
        self.failures.remove(&service);
        self.retry_at.remove(&service);
        self.save();
    }

    pub fn succeeded(&mut self, service: ScrobbleService) {
        self.failures.remove(&service);
        self.retry_at.remove(&service);
    }

    /// Waits 30 s after the first failure, doubling up to 30 minutes.
    pub fn failed(&mut self, service: ScrobbleService, now: Instant) -> Duration {
        let n = self.failures.entry(service).or_default();
        let wait = BACKOFF_MIN
            .saturating_mul(1 << (*n).min(16))
            .min(BACKOFF_MAX);
        *n += 1;
        self.retry_at.insert(service, now + wait);
        wait
    }

    /// When the next backed-off service may try again.
    pub fn next_retry(&self) -> Option<Instant> {
        self.retry_at.values().min().copied()
    }

    fn save(&self) {
        let result = if self.items.is_empty() {
            match std::fs::remove_file(&self.path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            }
        } else {
            serde_json::to_vec(&self.items)
                .map_err(std::io::Error::other)
                .and_then(|bytes| write_private(&self.path, &bytes, 0o600))
        };
        if let Err(e) = result {
            tracing::warn!(path = %self.path.display(), "saving the scrobble queue: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play(service: ScrobbleService, video_id: &str, at: u64) -> Pending {
        Pending {
            service,
            listened_at: at,
            song: Song {
                video_id: video_id.into(),
                title: "t".into(),
                artists: vec!["a".into()],
                ..Song::default()
            },
        }
    }

    #[test]
    fn survives_a_restart_and_keeps_one_copy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scrobble-queue.json");
        let mut q = Queue::load(path.clone());
        q.push([
            play(ScrobbleService::LastFm, "a", 1),
            play(ScrobbleService::ListenBrainz, "a", 1),
        ]);
        q.push([play(ScrobbleService::LastFm, "a", 1)]);
        assert_eq!(q.len(), 2);
        let back = Queue::load(path.clone());
        assert_eq!(back.items, q.items);
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn batches_per_service_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut q = Queue::load(dir.path().join("q.json"));
        q.push((0..120).map(|i| play(ScrobbleService::LastFm, "v", i)));
        q.push([play(ScrobbleService::ListenBrainz, "v", 7)]);
        let now = Instant::now();
        let batch = q.batch(ScrobbleService::LastFm, LASTFM_BATCH, now);
        assert_eq!(batch.len(), 50);
        assert_eq!(batch[0].listened_at, 0);
        assert_eq!(batch[49].listened_at, 49);
        q.remove(&batch);
        let batch = q.batch(ScrobbleService::LastFm, LASTFM_BATCH, now);
        assert_eq!(batch[0].listened_at, 50);
        assert_eq!(q.batch(ScrobbleService::ListenBrainz, 100, now).len(), 1);
    }

    #[test]
    fn backs_off_and_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let mut q = Queue::load(dir.path().join("q.json"));
        q.push([play(ScrobbleService::LastFm, "v", 1)]);
        let now = Instant::now();
        assert_eq!(
            q.failed(ScrobbleService::LastFm, now),
            Duration::from_secs(30)
        );
        assert!(q.batch(ScrobbleService::LastFm, 50, now).is_empty());
        assert_eq!(
            q.failed(ScrobbleService::LastFm, now),
            Duration::from_secs(60)
        );
        for _ in 0..20 {
            q.failed(ScrobbleService::LastFm, now);
        }
        assert_eq!(q.failed(ScrobbleService::LastFm, now), BACKOFF_MAX);
        let later = now + BACKOFF_MAX + Duration::from_secs(1);
        assert_eq!(q.batch(ScrobbleService::LastFm, 50, later).len(), 1);
        q.succeeded(ScrobbleService::LastFm);
        assert_eq!(q.next_retry(), None);
    }

    #[test]
    fn empty_queue_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("q.json");
        let mut q = Queue::load(path.clone());
        let p = play(ScrobbleService::LastFm, "v", 1);
        q.push([p.clone()]);
        assert!(path.exists());
        q.remove(&[p]);
        assert!(!path.exists());
    }
}
