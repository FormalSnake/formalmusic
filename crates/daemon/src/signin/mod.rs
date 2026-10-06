//! Sign-in through the user's own browser: open it in a throwaway profile at
//! Google's sign-in page, wait until the profile holds a YouTube session, keep
//! those cookies, then close the browser and delete the profile. The approach
//! (profile isolation, launch flags, the cookies that mean "signed in") is
//! from Kopuz's YouTube Music sign-in. Chromium browsers hand their cookies
//! over the DevTools pipe, which avoids decrypting the cookie store with a
//! key from the system keyring; Firefox browsers keep it unencrypted on disk.

mod browsers;
mod cdp;
mod gecko;

use browsers::{Browser, Engine, Launcher};
use formalmusic_api::{ApiError, Browsers};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Child;
use tokio::sync::oneshot;
use tokio::time::Instant;

const SIGN_IN_URL: &str = "https://accounts.google.com/ServiceLogin?service=youtube&continue=https%3A%2F%2Fmusic.youtube.com%2F";
const TIMEOUT: Duration = Duration::from_secs(300);
const POLL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cookie {
    pub domain: String,
    pub name: String,
    pub value: String,
}

pub struct BrowserSignIn {
    /// Parent of the per-attempt profile directories.
    dir: PathBuf,
    running: Mutex<Option<(u64, oneshot::Sender<()>)>>,
    attempts: std::sync::atomic::AtomicU64,
}

enum Stop {
    Cancelled,
    TimedOut,
    Closed,
    Failed(String),
}

impl BrowserSignIn {
    /// Deletes a profile left behind by a daemon that died mid sign-in.
    pub fn new(dir: PathBuf) -> Self {
        if let Err(e) = std::fs::remove_dir_all(&dir)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(dir = %dir.display(), "could not delete an old sign-in profile: {e}");
        }
        Self {
            dir,
            running: Mutex::new(None),
            attempts: Default::default(),
        }
    }

    pub async fn browsers(&self) -> Browsers {
        let installed = browsers::installed();
        let default = browsers::pick_default(&installed, browsers::system_default().await);
        Browsers {
            installed: installed
                .iter()
                .map(|(b, _)| formalmusic_api::Browser {
                    id: b.id().into(),
                    name: b.name().into(),
                })
                .collect(),
            default: default.map(|b| b.id().into()),
        }
    }

    /// Closes the browser of the running sign-in, if there is one.
    pub fn cancel(&self) -> bool {
        self.running
            .lock()
            .take()
            .is_some_and(|(_, cancel)| cancel.send(()).is_ok())
    }

    /// Runs one sign-in and answers with the `Cookie` header of the session.
    /// Starting one cancels any other still waiting.
    pub async fn run(&self, id: Option<&str>) -> Result<String, ApiError> {
        let (browser, launcher) = resolve(id).await?;
        let attempt = self
            .attempts
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (cancel, mut cancelled) = oneshot::channel();
        if let Some((_, previous)) = self.running.lock().replace((attempt, cancel)) {
            let _ = previous.send(());
        }
        let root = self.dir.join(attempt.to_string());
        let result = run_browser(browser, &launcher, &root, &mut cancelled).await;
        remove_dir(&root).await;
        {
            let mut running = self.running.lock();
            if running.as_ref().is_some_and(|(a, _)| *a == attempt) {
                *running = None;
            }
        }
        result.map_err(|stop| {
            ApiError::BadRequest(match stop {
                Stop::Cancelled => "Sign-in was cancelled.".into(),
                Stop::TimedOut => "Sign-in took longer than 5 minutes. Try again.".into(),
                Stop::Closed => format!(
                    "{} closed before sign-in finished. Try again.",
                    browser.name()
                ),
                Stop::Failed(error) => {
                    tracing::warn!(browser = browser.id(), "browser sign-in failed: {error}");
                    format!("Unable to sign in with {}: {error}", browser.name())
                }
            })
        })
    }
}

async fn resolve(id: Option<&str>) -> Result<(Browser, Launcher), ApiError> {
    let browser = match id {
        Some(id) => Browser::from_id(id)
            .ok_or_else(|| ApiError::BadRequest(format!("unknown browser: {id}")))?,
        None => {
            let installed = browsers::installed();
            browsers::pick_default(&installed, browsers::system_default().await).ok_or_else(
                || {
                    ApiError::BadRequest(
                        "No supported browser found. Install Firefox or Chromium, or paste cookies instead."
                            .into(),
                    )
                },
            )?
        }
    };
    let launcher = browsers::find(browser)
        .ok_or_else(|| ApiError::BadRequest(format!("{} is not installed.", browser.name())))?;
    Ok((browser, launcher))
}

async fn run_browser(
    browser: Browser,
    launcher: &Launcher,
    root: &Path,
    cancelled: &mut oneshot::Receiver<()>,
) -> Result<String, Stop> {
    let profile = root.join("profile");
    remove_dir(root).await;
    crate::config::create_private_dir(&profile).map_err(|e| Stop::Failed(e.to_string()))?;
    let mut command = launcher.command(&profile);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let deadline = Instant::now() + TIMEOUT;
    let spawn_error = |e: std::io::Error| Stop::Failed(format!("could not start it ({e})"));

    match browser.engine() {
        Engine::Chromium => {
            let (mut cdp, ends) = cdp::pipes().map_err(|e| Stop::Failed(e.to_string()))?;
            command
                .arg(format!("--user-data-dir={}", profile.display()))
                .arg("--remote-debugging-pipe")
                .arg("--no-first-run")
                .arg("--no-default-browser-check")
                .arg("--password-store=basic")
                .arg("--disable-extensions");
            if cfg!(target_os = "macos") {
                command.arg("--use-mock-keychain");
            }
            command.arg(format!("--app={SIGN_IN_URL}"));
            ends.attach(&mut command);
            let mut child = command.spawn().map_err(spawn_error)?;
            drop(ends);
            tracing::info!(browser = browser.id(), "browser sign-in started");
            #[cfg(test)]
            {
                let seed = live::SEED.lock().take();
                if let Some(seed) = seed {
                    live::seed(&mut cdp, seed).await;
                }
            }

            let started = Instant::now();
            let mut nudges = [Duration::from_secs(5), Duration::from_secs(15)]
                .into_iter()
                .peekable();
            let poll = async {
                loop {
                    if nudges
                        .next_if(|after| started.elapsed() >= *after)
                        .is_some()
                    {
                        renavigate_if_stalled(&mut cdp).await;
                    }
                    let reply =
                        cdp.call("Storage.getCookies", serde_json::json!({}))
                            .await
                            .map_err(|e| match e.kind() {
                                std::io::ErrorKind::UnexpectedEof
                                | std::io::ErrorKind::BrokenPipe => Stop::Closed,
                                _ => Stop::Failed(e.to_string()),
                            })?;
                    let cookies: Vec<Cookie> = reply["cookies"]
                        .as_array()
                        .map(|list| list.iter().filter_map(cdp_cookie).collect())
                        .unwrap_or_default();
                    if let Some(header) = signed_in_header(&cookies) {
                        return Ok(header);
                    }
                    tokio::time::sleep(POLL).await;
                }
            };
            let result = wait(poll, cancelled, deadline).await;
            let _ = tokio::time::timeout(
                Duration::from_secs(2),
                cdp.call("Browser.close", serde_json::json!({})),
            )
            .await;
            stop_child(&mut child).await;
            result
        }
        Engine::Gecko => {
            std::fs::write(profile.join("user.js"), gecko::USER_JS)
                .map_err(|e| Stop::Failed(e.to_string()))?;
            command
                .arg("--no-remote")
                .arg("--profile")
                .arg(&profile)
                .arg("--new-window")
                .arg(SIGN_IN_URL);
            let mut child = command.spawn().map_err(spawn_error)?;
            tracing::info!(browser = browser.id(), "browser sign-in started");

            let scratch = root.join("snapshot");
            let poll = async {
                loop {
                    let exited = matches!(child.try_wait(), Ok(Some(_)));
                    let (profile, scratch) = (profile.clone(), scratch.clone());
                    let cookies = tokio::task::spawn_blocking(move || {
                        gecko::read_cookies(&profile, &scratch)
                    })
                    .await
                    .map_err(|e| Stop::Failed(e.to_string()))?;
                    match cookies {
                        Ok(cookies) => {
                            if let Some(header) = signed_in_header(&cookies) {
                                return Ok(header);
                            }
                        }
                        Err(e) => tracing::debug!("cookies not readable yet: {e}"),
                    }
                    if exited {
                        return Err(Stop::Closed);
                    }
                    tokio::time::sleep(POLL).await;
                }
            };
            let result = wait(poll, cancelled, deadline).await;
            stop_child(&mut child).await;
            result
        }
    }
}

/// A fresh Helium profile can hold the first navigation forever while its
/// built-in content blocker starts up, leaving a blank window; navigating
/// again once it is up loads the page at once.
async fn renavigate_if_stalled(cdp: &mut cdp::Cdp) {
    let Ok(targets) = cdp.call("Target.getTargets", serde_json::json!({})).await else {
        return;
    };
    let stalled = targets["targetInfos"].as_array().and_then(|targets| {
        targets.iter().find(|t| {
            t["type"] == "page"
                && t["url"] == SIGN_IN_URL
                && t["title"].as_str().is_none_or(str::is_empty)
        })
    });
    let Some(target) = stalled else {
        return;
    };
    let attach = serde_json::json!({ "targetId": target["targetId"], "flatten": true });
    let Ok(attached) = cdp.call("Target.attachToTarget", attach).await else {
        return;
    };
    let Some(session) = attached["sessionId"].as_str().map(str::to_owned) else {
        return;
    };
    tracing::debug!("sign-in page stalled, navigating again");
    let _ = cdp
        .call_in(
            Some(&session),
            "Page.navigate",
            serde_json::json!({ "url": SIGN_IN_URL }),
        )
        .await;
    let _ = cdp
        .call(
            "Target.detachFromTarget",
            serde_json::json!({ "sessionId": session }),
        )
        .await;
}

async fn wait(
    poll: impl Future<Output = Result<String, Stop>>,
    cancelled: &mut oneshot::Receiver<()>,
    deadline: Instant,
) -> Result<String, Stop> {
    tokio::select! {
        result = poll => result,
        _ = cancelled => Err(Stop::Cancelled),
        _ = tokio::time::sleep_until(deadline) => Err(Stop::TimedOut),
    }
}

/// Waits briefly for a browser asked to close, then kills it, so nothing is
/// still writing to the profile when it is deleted.
async fn stop_child(child: &mut Child) {
    if tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
    }
}

/// Helper processes can still be flushing files for a moment after the
/// browser exits, which makes a single `remove_dir_all` fail.
async fn remove_dir(dir: &Path) {
    for _ in 0..10 {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) => tokio::time::sleep(Duration::from_millis(200)).await,
        }
    }
    tracing::warn!(dir = %dir.display(), "could not delete the sign-in profile");
}

fn cdp_cookie(value: &serde_json::Value) -> Option<Cookie> {
    Some(Cookie {
        domain: value["domain"].as_str()?.to_owned(),
        name: value["name"].as_str()?.to_owned(),
        value: value["value"].as_str()?.to_owned(),
    })
}

fn is_youtube(domain: &str) -> bool {
    let host = domain.strip_prefix('.').unwrap_or(domain);
    host == "youtube.com" || host.ends_with(".youtube.com")
}

/// Bytes a `Cookie` header can carry without quoting.
fn header_safe(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| (0x20..0x7f).contains(&b) && b != b';' && b != b',')
}

/// The youtube.com cookies as a `Cookie` header, once they hold a signed-in
/// session: `SID` and `SAPISID` only appear after Google redirects back to
/// YouTube. A name set on both `.youtube.com` and a subdomain keeps the
/// `.youtube.com` value, which is the one every YouTube host receives.
pub fn signed_in_header(cookies: &[Cookie]) -> Option<String> {
    let mut kept: Vec<&Cookie> = Vec::new();
    let mut youtube: Vec<&Cookie> = cookies
        .iter()
        .filter(|c| is_youtube(&c.domain) && header_safe(&c.name) && header_safe(&c.value))
        .collect();
    youtube.sort_by_key(|c| c.domain != ".youtube.com");
    for cookie in youtube {
        if !kept.iter().any(|k| k.name == cookie.name) {
            kept.push(cookie);
        }
    }
    let has = |name: &str| kept.iter().any(|c| c.name == name);
    if !(has("SID") && has("SAPISID")) {
        return None;
    }
    Some(
        kept.iter()
            .map(|c| format!("{}={}", c.name, c.value))
            .collect::<Vec<_>>()
            .join("; "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(domain: &str, name: &str, value: &str) -> Cookie {
        Cookie {
            domain: domain.into(),
            name: name.into(),
            value: value.into(),
        }
    }

    #[test]
    fn keeps_youtube_cookies_once_signed_in() {
        let cookies = [
            cookie("music.youtube.com", "SID", "sub"),
            cookie(".youtube.com", "SID", "s"),
            cookie(".youtube.com", "SAPISID", "a/b"),
            cookie("youtube.com", "PREF", "f6=40000000&tz=Europe.Lisbon"),
            cookie(".google.com", "NID", "n"),
            cookie("notyoutube.com", "HSID", "x"),
            cookie("youtube.com.evil.test", "SSID", "x"),
            cookie(".youtube.com", "EMPTY", ""),
            cookie(".youtube.com", "LIST", "a,b"),
        ];
        assert_eq!(
            signed_in_header(&cookies).as_deref(),
            Some("SID=s; SAPISID=a/b; PREF=f6=40000000&tz=Europe.Lisbon")
        );
    }

    #[test]
    fn waits_for_both_session_cookies() {
        assert_eq!(signed_in_header(&[]), None);
        // Google's own cookies arrive first; YouTube's follow the redirect.
        let google_only = [
            cookie(".google.com", "SID", "s"),
            cookie(".google.com", "SAPISID", "a"),
        ];
        assert_eq!(signed_in_header(&google_only), None);
        let visitor = [
            cookie(".youtube.com", "VISITOR_INFO1_LIVE", "v"),
            cookie(".youtube.com", "SID", "s"),
        ];
        assert_eq!(signed_in_header(&visitor), None);
    }

    #[test]
    fn reads_devtools_cookies() {
        let value = serde_json::json!({
            "name": "SAPISID", "value": "x", "domain": ".youtube.com", "path": "/", "secure": true
        });
        assert_eq!(
            cdp_cookie(&value),
            Some(cookie(".youtube.com", "SAPISID", "x"))
        );
        assert_eq!(cdp_cookie(&serde_json::json!({ "name": "x" })), None);
    }
}

/// Runs against a real browser: `FORMALMUSIC_LIVE_BROWSER` picks it (the
/// default otherwise) and `FORMALMUSIC_LIVE_COOKIES` is a cookie file of a
/// signed-in session, which stands in for typing a password: it goes into
/// the sign-in browser over the same pipe the cookies come back on.
#[cfg(test)]
mod live {
    use super::*;
    use serde_json::json;

    /// Cookies to sign the browser in with; an empty list only checks that
    /// Google's page opened.
    pub static SEED: Mutex<Option<Vec<Cookie>>> = Mutex::new(None);
    static PAGE_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    async fn page_urls(cdp: &mut cdp::Cdp) -> Vec<String> {
        let targets = cdp.call("Target.getTargets", json!({})).await.unwrap();
        targets["targetInfos"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["type"] == "page")
            .filter(|t| {
                t["title"].as_str().is_some_and(|title| {
                    !title.is_empty() && !t["url"].as_str().unwrap_or_default().contains(title)
                })
            })
            .filter_map(|t| Some(format!("{} ({})", t["url"].as_str()?, t["title"].as_str()?)))
            .collect()
    }

    /// Checks that Google's sign-in page opened, then signs the profile in
    /// and opens YouTube Music with it.
    pub async fn seed(cdp: &mut cdp::Cdp, cookies: Vec<Cookie>) {
        let mut opened = false;
        for n in 0..60 {
            // The same schedule as the sign-in loop, which only starts after this.
            if n == 10 || n == 30 {
                renavigate_if_stalled(cdp).await;
            }
            if page_urls(cdp)
                .await
                .iter()
                .any(|u| u.starts_with("https://accounts.google.com/"))
            {
                opened = true;
                break;
            }
            tokio::time::sleep(POLL).await;
        }
        assert!(
            opened,
            "Google's sign-in page never opened: {}",
            cdp.call("Target.getTargets", json!({})).await.unwrap()
        );
        eprintln!(
            "live: Google sign-in page is open: {:?}",
            page_urls(cdp).await
        );
        PAGE_OPEN.store(true, std::sync::atomic::Ordering::Relaxed);
        if cookies.is_empty() {
            return;
        }
        let cookies: Vec<_> = cookies
            .iter()
            .map(|c| json!({ "name": c.name, "value": c.value, "domain": c.domain, "path": "/", "secure": true }))
            .collect();
        cdp.call("Storage.setCookies", json!({ "cookies": cookies }))
            .await
            .unwrap();
        cdp.call(
            "Target.createTarget",
            json!({ "url": "https://music.youtube.com/" }),
        )
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_secs(8)).await;
        eprintln!("live: pages open: {:?}", page_urls(cdp).await);
    }

    fn files_under(dir: &Path) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .flat_map(|e| {
                let path = e.path();
                if path.is_dir() {
                    files_under(&path)
                } else {
                    vec![path]
                }
            })
            .collect()
    }

    #[tokio::test]
    #[ignore = "opens a browser and needs FORMALMUSIC_LIVE_COOKIES"]
    async fn signs_in_with_the_seeded_session() {
        let file = std::env::var("FORMALMUSIC_LIVE_COOKIES").expect("FORMALMUSIC_LIVE_COOKIES");
        let header = crate::session::cookie_header(&std::fs::read_to_string(file).unwrap());
        let seed = header
            .split("; ")
            .filter_map(|pair| pair.split_once('='))
            .map(|(name, value)| Cookie {
                domain: ".youtube.com".into(),
                name: name.into(),
                value: value.into(),
            })
            .collect();
        *SEED.lock() = Some(seed);

        let state = tempfile::tempdir().unwrap();
        let signin = BrowserSignIn::new(state.path().join("signin"));
        let browser = std::env::var("FORMALMUSIC_LIVE_BROWSER")
            .ok()
            .filter(|b| !b.is_empty());
        let cookies = signin.run(browser.as_deref()).await.unwrap();
        assert!(files_under(state.path()).is_empty(), "profile left behind");

        let session = crate::session::Session::load(state.path().join("session.json")).unwrap();
        let info = session.sign_in(&cookies, None).await.unwrap();
        eprintln!(
            "live: signed_in={} premium={} account={:?}",
            info.signed_in,
            info.premium,
            info.account.as_ref().map(|a| &a.name)
        );
        assert!(info.signed_in && info.premium);
        assert_eq!(
            files_under(state.path()),
            vec![state.path().join("session.json")]
        );
    }

    #[tokio::test]
    #[ignore = "opens a browser"]
    async fn cancel_closes_the_browser_and_deletes_the_profile() {
        let state = tempfile::tempdir().unwrap();
        let signin = std::sync::Arc::new(BrowserSignIn::new(state.path().join("signin")));
        let browser = std::env::var("FORMALMUSIC_LIVE_BROWSER")
            .ok()
            .filter(|b| !b.is_empty());
        *SEED.lock() = Some(Vec::new());
        let run = tokio::spawn({
            let signin = signin.clone();
            async move { signin.run(browser.as_deref()).await }
        });
        // Chromium reports its pages over the pipe. Firefox cannot, but Google
        // sets cookies on the sign-in page, so they show it loaded.
        let mut google = false;
        for _ in 0..60 {
            tokio::time::sleep(POLL).await;
            google = PAGE_OPEN.load(std::sync::atomic::Ordering::Relaxed)
                || files_under(state.path()).iter().any(|f| {
                    f.file_name()
                        .is_some_and(|n| n == "Cookies" || n == "cookies.sqlite")
                }) && profile_has_google_cookies(state.path());
            if google {
                break;
            }
        }
        assert!(signin.cancel());
        let error = run.await.unwrap().unwrap_err();
        eprintln!("live: google page loaded={google}, after cancel: {error}");
        assert!(google, "Google's sign-in page never loaded");
        assert!(matches!(error, ApiError::BadRequest(m) if m.contains("cancelled")));
        assert!(files_under(state.path()).is_empty(), "profile left behind");
    }

    /// The Chromium store is encrypted, but its host column is not.
    fn profile_has_google_cookies(state: &Path) -> bool {
        files_under(state)
            .iter()
            .filter(|f| {
                f.file_name()
                    .is_some_and(|n| n == "Cookies" || n == "cookies.sqlite")
            })
            .any(|db| {
                let scratch = tempfile::tempdir().unwrap();
                let copy = scratch.path().join("db");
                std::fs::copy(db, &copy).is_ok()
                    && [("-wal", "db-wal")].iter().all(|(s, to)| {
                        let wal = db.with_file_name(format!(
                            "{}{s}",
                            db.file_name().unwrap().to_string_lossy()
                        ));
                        !wal.exists() || std::fs::copy(&wal, scratch.path().join(to)).is_ok()
                    })
                    && rusqlite::Connection::open(&copy).is_ok_and(|c| {
                        let table = if db.ends_with("Cookies") {
                            "SELECT host_key FROM cookies"
                        } else {
                            "SELECT host FROM moz_cookies"
                        };
                        c.prepare(table).is_ok_and(|mut q| {
                            q.query_map([], |r| r.get::<_, String>(0))
                                .is_ok_and(|rows| rows.flatten().any(|h| h.ends_with("google.com")))
                        })
                    })
            })
    }
}
