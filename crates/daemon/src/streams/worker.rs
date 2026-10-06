//! One long-lived Python process running yt-dlp (`ytdlp_worker.py`), so a
//! track does not pay for interpreter start-up, extractor imports and the
//! player JS download. Requests and answers are JSON lines matched by id;
//! the process is started again on the next request after it exits.

use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;

const SCRIPT: &str = include_str!("ytdlp_worker.py");

/// Answers still owed by one process; `None` once its stdout has closed.
type Pending = Arc<Mutex<Option<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>>;

pub struct Worker {
    python: PathBuf,
    cache_dir: PathBuf,
    process: tokio::sync::Mutex<Option<Process>>,
    next_id: AtomicU64,
}

struct Process {
    child: Child,
    stdin: ChildStdin,
    pending: Pending,
}

/// `$FORMALMUSIC_YTDLP_PYTHON`, an interpreter that can import `yt_dlp`
/// (the Nix package points it at one built with the pinned yt-dlp), else
/// `python3` from `PATH`.
pub fn python() -> PathBuf {
    std::env::var_os("FORMALMUSIC_YTDLP_PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|| "python3".into())
}

impl Worker {
    pub fn new(python: PathBuf, cache_dir: PathBuf) -> Self {
        Self {
            python,
            cache_dir,
            process: tokio::sync::Mutex::new(None),
            next_id: AtomicU64::new(0),
        }
    }

    /// Starts the process ahead of the first request.
    pub async fn warm(&self) {
        if let Err(e) = self.running().await {
            tracing::warn!("{e}");
        }
    }

    /// yt-dlp's info for `video_id`, holding only its `formats`.
    pub async fn info(
        &self,
        video_id: &str,
        cookies: Option<&Path>,
        premium: bool,
        timeout: Duration,
    ) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut line = json!({
            "id": id,
            "video_id": video_id,
            "cookies": cookies,
            "premium": premium,
        })
        .to_string();
        line.push('\n');
        let (tx, rx) = oneshot::channel();
        {
            let mut guard = self.running().await?;
            let process = guard.as_mut().expect("running() starts the process");
            if let Some(pending) = process.pending.lock().as_mut() {
                pending.insert(id, tx);
            }
            if let Err(e) = process.stdin.write_all(line.as_bytes()).await {
                *guard = None;
                return Err(format!("writing to the yt-dlp worker: {e}"));
            }
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(_)) => Err("the yt-dlp worker exited".into()),
            Err(_) => Err(format!("yt-dlp took longer than {}s", timeout.as_secs())),
        }
    }

    async fn running(&self) -> Result<tokio::sync::MutexGuard<'_, Option<Process>>, String> {
        let mut guard = self.process.lock().await;
        let exited = match guard.as_mut() {
            Some(process) => {
                process.pending.lock().is_none()
                    || process.child.try_wait().ok().flatten().is_some()
            }
            None => true,
        };
        if exited {
            *guard = Some(self.spawn()?);
        }
        Ok(guard)
    }

    fn spawn(&self) -> Result<Process, String> {
        let mut child = Command::new(&self.python)
            .arg("-c")
            .arg(SCRIPT)
            .arg(&self.cache_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("running {}: {e}", self.python.display()))?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let pending: Pending = Arc::new(Mutex::new(Some(HashMap::new())));
        let last_error = Arc::new(Mutex::new(String::new()));
        tokio::spawn({
            let last_error = last_error.clone();
            async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "formalmusicd::ytdlp", "{line}");
                    if !line.trim().is_empty() {
                        *last_error.lock() = line;
                    }
                }
            }
        });
        tokio::spawn({
            let pending = pending.clone();
            async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let Ok(answer) = serde_json::from_str::<Value>(&line) else {
                        tracing::debug!(target: "formalmusicd::ytdlp", "{line}");
                        continue;
                    };
                    let Some(id) = answer["id"].as_u64() else {
                        continue;
                    };
                    let result = match (answer.get("info"), answer["error"].as_str()) {
                        (Some(info), _) => Ok(info.clone()),
                        (None, Some(error)) => Err(error.to_owned()),
                        (None, None) => Err("the yt-dlp worker answered nothing".into()),
                    };
                    if let Some(tx) = pending.lock().as_mut().and_then(|p| p.remove(&id)) {
                        let _ = tx.send(result);
                    }
                }
                // Give stderr a moment to deliver the traceback that explains the exit.
                tokio::time::sleep(Duration::from_millis(100)).await;
                let reason = last_error.lock().clone();
                let owed = pending.lock().take().unwrap_or_default();
                for (_, tx) in owed {
                    let _ = tx.send(Err(if reason.is_empty() {
                        "the yt-dlp worker exited".to_owned()
                    } else {
                        format!("the yt-dlp worker exited: {reason}")
                    }));
                }
            }
        });
        Ok(Process {
            child,
            stdin,
            pending,
        })
    }
}
