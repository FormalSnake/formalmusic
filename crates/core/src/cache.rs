//! The last known Home, library, queue and player on disk, so the window
//! paints before the daemon answers.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use formalmusic_api::{BrowseTarget, Page, PlayerState, QueueState, SessionInfo};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::task::AbortHandle;

const SAVE_DELAY: Duration = Duration::from_millis(2000);
const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CachedState {
    pub version: u32,
    /// Pages worth painting at once: Home, Explore and the library tabs.
    /// A list of pairs, since a JSON object key cannot be a `BrowseTarget`.
    #[serde(default)]
    pub pages: Vec<(BrowseTarget, Arc<Page>)>,
    #[serde(default)]
    pub player: PlayerState,
    #[serde(default)]
    pub queue: Arc<QueueState>,
    #[serde(default)]
    pub session: Option<SessionInfo>,
}

impl CachedState {
    pub fn new() -> Self {
        Self {
            version: VERSION,
            pages: Vec::new(),
            player: PlayerState::default(),
            queue: Arc::default(),
            session: None,
        }
    }
}

impl Default for CachedState {
    fn default() -> Self {
        Self::new()
    }
}

type Snapshot = Box<dyn FnOnce() -> CachedState + Send>;

struct Shared {
    file: PathBuf,
    temp: PathBuf,
    pending: Mutex<Option<Snapshot>>,
    timer: Mutex<Option<AbortHandle>>,
    /// Held for the length of a write, so `flush` can wait out one in flight.
    writing: tokio::sync::Mutex<()>,
}

/// Writes are debounced (2 s, snapshot taken when the timer fires) and atomic
/// (a per-instance temp file, then rename). Serialization and I/O run on the
/// runtime's blocking pool, never on the caller.
pub struct StateCache {
    shared: Arc<Shared>,
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

impl StateCache {
    pub fn new(cache_dir: &Path) -> Self {
        let file = cache_dir.join("state.json");
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
        let temp = cache_dir.join(format!("state.json.{}-{n}.tmp", std::process::id()));
        Self {
            shared: Arc::new(Shared {
                file,
                temp,
                pending: Mutex::new(None),
                timer: Mutex::new(None),
                writing: tokio::sync::Mutex::new(()),
            }),
        }
    }

    pub fn file(&self) -> &Path {
        &self.shared.file
    }

    /// None when missing, unparsable or another version.
    pub async fn load(&self) -> Option<CachedState> {
        let file = self.shared.file.clone();
        let bytes = match tokio::fs::read(&file).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => {
                tracing::warn!("cache: ignoring {}: {error}", file.display());
                return None;
            }
        };
        let parsed =
            tokio::task::spawn_blocking(move || serde_json::from_slice::<CachedState>(&bytes))
                .await
                .ok()?;
        match parsed {
            Ok(state) if state.version == VERSION => Some(state),
            Ok(_) => None,
            Err(error) => {
                tracing::warn!("cache: ignoring {}: {error}", file.display());
                None
            }
        }
    }

    /// Schedules a write. `snapshot` runs when the timer fires, not now.
    /// Must be called inside the runtime's context.
    pub fn schedule(&self, snapshot: Snapshot) {
        *self.shared.pending.lock() = Some(snapshot);
        let mut timer = self.shared.timer.lock();
        if timer.is_some() {
            return;
        }
        let shared = self.shared.clone();
        let task = tokio::spawn(async move {
            tokio::time::sleep(SAVE_DELAY).await;
            shared.timer.lock().take();
            flush(&shared).await;
        });
        *timer = Some(task.abort_handle());
    }

    /// Writes anything pending and waits for an in-flight write.
    pub async fn flush(&self) {
        if let Some(timer) = self.shared.timer.lock().take() {
            timer.abort();
        }
        flush(&self.shared).await;
    }
}

async fn flush(shared: &Arc<Shared>) {
    let snapshot = shared.pending.lock().take();
    let _writing = shared.writing.lock().await;
    let Some(snapshot) = snapshot else { return };
    let file = shared.file.clone();
    let temp = shared.temp.clone();
    let result = tokio::task::spawn_blocking(move || write(&file, &temp, &snapshot())).await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::warn!("cache: {error}"),
        Err(error) => tracing::warn!("cache: {error}"),
    }
}

fn write(file: &Path, temp: &Path, state: &CachedState) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let bytes = serde_json::to_vec(state).map_err(std::io::Error::other)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut out = options.open(temp)?;
    out.write_all(&bytes)?;
    drop(out);
    std::fs::rename(temp, file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("formalmusic-cache-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn ignores_a_missing_corrupt_or_foreign_file() {
        let dir = temp_dir("corrupt");
        let cache = StateCache::new(&dir);
        assert!(cache.load().await.is_none());
        std::fs::write(cache.file(), b"{ not json").unwrap();
        assert!(cache.load().await.is_none());
        std::fs::write(cache.file(), br#"{"version":2}"#).unwrap();
        assert!(cache.load().await.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test(start_paused = true)]
    async fn debounces_writes_and_flushes_what_is_pending() {
        let dir = temp_dir("debounce");
        let cache = StateCache::new(&dir);
        let snapshot = |volume: f32| -> Snapshot {
            Box::new(move || CachedState {
                player: PlayerState {
                    volume,
                    ..PlayerState::default()
                },
                ..CachedState::new()
            })
        };
        cache.schedule(snapshot(0.1));
        cache.schedule(snapshot(0.2));
        assert!(!cache.file().exists());
        tokio::time::sleep(Duration::from_millis(2100)).await;
        cache.flush().await;
        assert_eq!(cache.load().await.unwrap().player.volume, 0.2);
        cache.schedule(snapshot(0.3));
        cache.flush().await;
        assert_eq!(cache.load().await.unwrap().player.volume, 0.3);
        std::fs::remove_dir_all(&dir).ok();
    }
}
