//! The signed-in session: cookies on disk, and the one InnerTube client that
//! acts for them. Continuation tokens only work from the client that fetched
//! the page, so the client is rebuilt only when the account changes.

use crate::config::write_private;
use formalmusic_api::{ApiError, SessionInfo};
use formalmusic_innertube::Client;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stored {
    pub cookies: String,
    pub page_id: Option<String>,
    /// The browser profile the cookies were imported from. That profile and
    /// the daemon then share one Google session, which dies for whichever
    /// side did not rotate it last, so the daemon reads it again from there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<ImportedFrom>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportedFrom {
    pub browser: String,
    pub profile: String,
}

pub struct Session {
    path: PathBuf,
    state: RwLock<State>,
}

struct State {
    /// Bumped whenever the client is replaced.
    generation: u64,
    stored: Option<Stored>,
    client: Client,
    info: SessionInfo,
}

impl Session {
    /// Loads the stored cookies without checking them; [`Session::refresh`]
    /// does that once the daemon is up.
    pub fn load(path: PathBuf) -> anyhow::Result<Self> {
        let stored = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Stored>(&text) {
                Ok(stored) => Some(stored),
                Err(e) => {
                    tracing::warn!(path = %path.display(), "ignoring unreadable session: {e}");
                    None
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(e.into()),
        };
        let client = match &stored {
            Some(s) => Client::signed_in(&s.cookies, s.page_id.clone()).or_else(|e| {
                tracing::warn!("stored session unusable: {e}");
                Client::anonymous()
            })?,
            None => Client::anonymous()?,
        };
        let info = SessionInfo {
            signed_in: client.is_signed_in(),
            ..SessionInfo::default()
        };
        Ok(Self {
            path,
            state: RwLock::new(State {
                generation: 0,
                stored,
                client,
                info,
            }),
        })
    }

    pub fn client(&self) -> Client {
        self.state.read().client.clone()
    }

    pub fn info(&self) -> SessionInfo {
        self.state.read().info.clone()
    }

    /// The cookies as they stand now: YouTube's `Set-Cookie`s and yt-dlp's
    /// updates included, which [`Session::persist`] writes back.
    pub fn cookies(&self) -> Option<String> {
        let state = self.state.read();
        state.stored.as_ref()?;
        state.client.cookies()
    }

    /// Cookies yt-dlp was handed while it acted for this session.
    pub fn merge_cookies(&self, pairs: &[(String, String)]) {
        self.state.read().client.merge_cookies(pairs);
    }

    pub fn imported_from(&self) -> Option<ImportedFrom> {
        self.state.read().stored.as_ref()?.profile.clone()
    }

    /// Writes the cookies back to disk when YouTube or yt-dlp changed them,
    /// so a restart picks up the rotated set rather than the stale one.
    pub fn persist(&self) {
        let mut state = self.state.write();
        let Some(current) = state.client.cookies() else {
            return;
        };
        let Some(stored) = state.stored.as_mut().filter(|s| s.cookies != current) else {
            return;
        };
        stored.cookies = current;
        if let Err(e) = save(&self.path, Some(stored)) {
            tracing::warn!("saving the rotated cookies: {e}");
        }
    }

    /// Asks YouTube to keep the session from lapsing, as the web app does.
    pub async fn keepalive(&self) -> Result<(), ApiError> {
        self.client().keepalive().await
    }

    /// Asks YouTube who the stored cookies belong to.
    pub async fn refresh(&self) -> Result<SessionInfo, ApiError> {
        let (generation, client) = {
            let state = self.state.read();
            (state.generation, state.client.clone())
        };
        let info = client.session().await?;
        let mut state = self.state.write();
        // A sign-in that raced this request owns the state now.
        if state.generation == generation {
            state.info = info.clone();
        }
        Ok(info)
    }

    /// Checks `cookies`, a `Cookie` header or a Netscape cookie file, against
    /// YouTube before keeping them.
    pub async fn sign_in(
        &self,
        cookies: &str,
        page_id: Option<String>,
    ) -> Result<SessionInfo, ApiError> {
        self.sign_in_from(cookies, page_id, None).await
    }

    /// [`Session::sign_in`] with cookies read out of a browser profile.
    pub async fn sign_in_from(
        &self,
        cookies: &str,
        page_id: Option<String>,
        profile: Option<ImportedFrom>,
    ) -> Result<SessionInfo, ApiError> {
        let header = cookie_header(cookies);
        let cookies = header.as_str();
        let client = Client::signed_in(cookies, page_id.clone())?;
        let info = client.session().await?;
        if !info.signed_in {
            return Err(ApiError::BadRequest(
                "YouTube does not consider these cookies signed in. Cookies copied out of a browser stop working once it rotates them, so sign in to music.youtube.com there again and copy them anew.".into(),
            ));
        }
        let stored = Stored {
            cookies: cookies.trim().to_owned(),
            page_id,
            profile,
        };
        save(&self.path, Some(&stored))
            .map_err(|e| ApiError::BadRequest(format!("saving the session: {e}")))?;
        self.replace(Some(stored), client, info.clone());
        Ok(info)
    }

    pub async fn switch_account(&self, page_id: Option<String>) -> Result<SessionInfo, ApiError> {
        let cookies = self.cookies().ok_or(ApiError::SignedOut)?;
        self.sign_in_from(&cookies, page_id, self.imported_from())
            .await
    }

    pub fn sign_out(&self) -> Result<SessionInfo, ApiError> {
        save(&self.path, None)
            .map_err(|e| ApiError::BadRequest(format!("removing the session: {e}")))?;
        let client = Client::anonymous()?;
        self.replace(None, client, SessionInfo::default());
        Ok(SessionInfo::default())
    }
}

impl Session {
    fn replace(&self, stored: Option<Stored>, client: Client, info: SessionInfo) {
        let mut state = self.state.write();
        state.generation += 1;
        state.stored = stored;
        state.client = client;
        state.info = info;
    }
}

fn save(path: &Path, stored: Option<&Stored>) -> std::io::Result<()> {
    match stored {
        Some(stored) => write_private(path, &serde_json::to_vec_pretty(stored)?, 0o600),
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        },
    }
}

/// A `Cookie` header from either a header or a Netscape cookie file, which
/// is what yt-dlp and the browser export extensions write.
pub fn cookie_header(input: &str) -> String {
    if !input.lines().any(|l| l.split('\t').count() == 7) {
        return input.trim().trim_start_matches("Cookie:").trim().to_owned();
    }
    input
        .lines()
        .filter_map(|line| {
            // curl marks HttpOnly cookies with this prefix, which would
            // otherwise read as a comment.
            let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
            if line.starts_with('#') {
                return None;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            let [domain, _, _, _, _, name, value] = fields[..] else {
                return None;
            };
            domain
                .trim_start_matches('.')
                .ends_with("youtube.com")
                .then(|| format!("{name}={}", value.trim_end()))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// A Netscape cookie file for yt-dlp's `--cookies`, from a `Cookie` header.
/// The header carries no attributes, so every cookie is scoped to
/// `.youtube.com`, secure, and valid for a year.
pub fn netscape_cookies(header: &str) -> String {
    let expires = (SystemTime::now() + Duration::from_secs(365 * 24 * 3600))
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let mut out = String::from("# Netscape HTTP Cookie File\n");
    for pair in header.split(';') {
        let Some((name, value)) = pair.trim().split_once('=') else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        out.push_str(&format!(
            ".youtube.com\tTRUE\t/\tTRUE\t{expires}\t{name}\t{value}\n"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_in_takes_headers_and_cookie_files() {
        assert_eq!(
            cookie_header("Cookie: SID=1; SAPISID=2\n"),
            "SID=1; SAPISID=2"
        );
        let file = "# Netscape HTTP Cookie File\n\
            .youtube.com\tTRUE\t/\tTRUE\t1893456000\tSAPISID\tabc\n\
            #HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t1893456000\tSID\tdef\n\
            .google.com\tTRUE\t/\tTRUE\t1893456000\tNID\tzzz\n";
        assert_eq!(cookie_header(file), "SAPISID=abc; SID=def");
        assert_eq!(cookie_header(&netscape_cookies("A=1; B=2")), "A=1; B=2");
    }

    #[tokio::test]
    async fn rotated_cookies_reach_the_session_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let stored = Stored {
            cookies: "SAPISID=a; SIDCC=old".into(),
            page_id: None,
            profile: Some(ImportedFrom {
                browser: "firefox".into(),
                profile: "/p".into(),
            }),
        };
        save(&path, Some(&stored)).unwrap();
        let session = Session::load(path.clone()).unwrap();
        session.persist();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            serde_json::to_vec_pretty(&stored).unwrap()
        );

        session.merge_cookies(&[("SIDCC".into(), "new".into()), ("YSC".into(), "y".into())]);
        assert_eq!(
            session.cookies().as_deref(),
            Some("SAPISID=a; SIDCC=new; YSC=y")
        );
        session.persist();
        let saved: Stored = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.cookies, "SAPISID=a; SIDCC=new; YSC=y");
        assert_eq!(saved.profile, stored.profile);
    }

    #[test]
    fn cookie_header_to_netscape() {
        let file = netscape_cookies("SID=abc; __Secure-3PAPISID=x=y; junk; =v");
        let lines: Vec<&str> = file.lines().collect();
        assert_eq!(lines[0], "# Netscape HTTP Cookie File");
        assert_eq!(lines.len(), 3);
        let fields: Vec<&str> = lines[2].split('\t').collect();
        assert_eq!(fields[0], ".youtube.com");
        assert_eq!(fields[5], "__Secure-3PAPISID");
        assert_eq!(fields[6], "x=y");
    }
}
