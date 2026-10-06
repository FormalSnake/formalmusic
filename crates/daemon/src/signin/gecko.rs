//! Cookies from a Firefox-family profile. `cookies.sqlite` is not encrypted,
//! but a running browser keeps fresh cookies in the write-ahead log until it
//! checkpoints, so the reader copies the database and its log and opens the
//! copy, which replays the log without touching the browser's files.

use super::Cookie;
use std::path::Path;

/// Prefs read before the first window opens, so a fresh profile goes straight
/// to the sign-in page instead of onboarding tabs.
pub const USER_JS: &str = concat!(
    "user_pref(\"browser.aboutwelcome.enabled\", false);\n",
    "user_pref(\"browser.startup.homepage_override.mstone\", \"ignore\");\n",
    "user_pref(\"browser.shell.checkDefaultBrowser\", false);\n",
    "user_pref(\"datareporting.policy.dataSubmissionEnabled\", false);\n",
    "user_pref(\"browser.sessionstore.resume_from_crash\", false);\n",
);

/// Reads every cookie in `profile`, through a copy made in `scratch`.
pub fn read_cookies(profile: &Path, scratch: &Path) -> Result<Vec<Cookie>, String> {
    let source = profile.join("cookies.sqlite");
    if !source.exists() {
        return Ok(Vec::new());
    }
    std::fs::create_dir_all(scratch).map_err(|e| e.to_string())?;
    for suffix in ["", "-wal"] {
        let from = profile.join(format!("cookies.sqlite{suffix}"));
        let to = scratch.join(format!("cookies.sqlite{suffix}"));
        match std::fs::copy(&from, &to) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let _ = std::fs::remove_file(&to);
            }
            Err(e) => return Err(format!("copying cookies.sqlite{suffix}: {e}")),
        }
    }
    let _ = std::fs::remove_file(scratch.join("cookies.sqlite-shm"));
    let db =
        rusqlite::Connection::open(scratch.join("cookies.sqlite")).map_err(|e| e.to_string())?;
    let mut query = db
        .prepare("SELECT host, name, value FROM moz_cookies")
        .map_err(|e| e.to_string())?;
    let rows = query
        .query_map([], |row| {
            Ok(Cookie {
                domain: row.get(0)?,
                name: row.get(1)?,
                value: row.get(2)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_rows_from_the_database_and_its_log() {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("profile");
        std::fs::create_dir_all(&profile).unwrap();
        let db = rusqlite::Connection::open(profile.join("cookies.sqlite")).unwrap();
        db.pragma_update(None, "journal_mode", "wal").unwrap();
        db.execute_batch(
            "PRAGMA wal_autocheckpoint = 0;
             CREATE TABLE moz_cookies (host TEXT, name TEXT, value TEXT);
             INSERT INTO moz_cookies VALUES ('.youtube.com', 'SID', 'a');",
        )
        .unwrap();
        // Kept open so the insert stays in the log, as in a running browser.
        let cookies = read_cookies(&profile, &dir.path().join("scratch")).unwrap();
        assert_eq!(
            cookies,
            vec![Cookie {
                domain: ".youtube.com".into(),
                name: "SID".into(),
                value: "a".into()
            }]
        );
        drop(db);
        assert!(
            read_cookies(&dir.path().join("none"), dir.path())
                .unwrap()
                .is_empty()
        );
    }
}
