//! The user's own browser profiles, and the YouTube session in one of them.
//! Listing reads `Local State` and `profiles.ini`; importing has yt-dlp copy
//! and decrypt the profile's cookie store. Neither starts, signals or locks
//! the browser, so both work while it is open.

use super::browsers::{Browser, Engine};
use super::{Cookie, signed_in_header};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub browser: Browser,
    pub dir: PathBuf,
    pub name: String,
    pub email: Option<String>,
}

/// Data directories holding a Chromium `Local State` or a Firefox
/// `profiles.ini`, relative to the platform's application data directory.
fn data_dirs(browser: Browser, mac: bool) -> &'static [&'static str] {
    match (browser, mac) {
        (Browser::Helium, false) => &["net.imput.helium"],
        // Helium's macOS cookies are keyed by its own Keychain entry, which
        // yt-dlp has no name for.
        (Browser::Helium, true) => &[],
        (Browser::Chrome, false) => &["google-chrome"],
        (Browser::Chrome, true) => &["Google/Chrome"],
        (Browser::Chromium, false) => &["chromium"],
        (Browser::Chromium, true) => &["Chromium"],
        (Browser::Brave, _) => &["BraveSoftware/Brave-Browser"],
        (Browser::Vivaldi, false) => &["vivaldi"],
        (Browser::Vivaldi, true) => &["Vivaldi"],
        (Browser::Edge, false) => &["microsoft-edge"],
        (Browser::Edge, true) => &["Microsoft Edge"],
        (Browser::Firefox, false) => &["mozilla/firefox"],
        (Browser::Firefox, true) => &["Firefox"],
        (Browser::LibreWolf, false) => &["librewolf/librewolf"],
        (Browser::LibreWolf, true) => &["librewolf"],
        (Browser::Zen, false) => &["zen"],
        (Browser::Zen, true) => &["zen"],
        (Browser::Floorp, false) => &["floorp"],
        (Browser::Floorp, true) => &["Floorp"],
    }
}

/// Firefox and its forks still default to a dot directory in `$HOME` on Linux.
fn home_dirs(browser: Browser) -> &'static [&'static str] {
    match browser {
        Browser::Firefox => &[".mozilla/firefox"],
        Browser::LibreWolf => &[".librewolf"],
        Browser::Zen => &[".zen"],
        Browser::Floorp => &[".floorp"],
        _ => &[],
    }
}

fn flatpak_id(browser: Browser) -> Option<&'static str> {
    match browser {
        Browser::Chrome => Some("com.google.Chrome"),
        Browser::Chromium => Some("org.chromium.Chromium"),
        Browser::Brave => Some("com.brave.Browser"),
        Browser::Vivaldi => Some("com.vivaldi.Vivaldi"),
        Browser::Edge => Some("com.microsoft.Edge"),
        Browser::Firefox => Some("org.mozilla.firefox"),
        Browser::LibreWolf => Some("io.gitlab.librewolf-community"),
        Browser::Zen => Some("app.zen_browser.zen"),
        Browser::Floorp => Some("one.ablaze.floorp"),
        Browser::Helium => None,
    }
}

/// Chromium keeps its `User Data` under `%LOCALAPPDATA%`, Firefox and its
/// forks their `profiles.ini` under `%APPDATA%`.
fn windows_roots(browser: Browser) -> Vec<PathBuf> {
    let (base, dirs): (_, &[&str]) = match browser {
        Browser::Helium => (dirs::data_local_dir(), &[r"imput\Helium\User Data"]),
        Browser::Chrome => (dirs::data_local_dir(), &[r"Google\Chrome\User Data"]),
        Browser::Chromium => (dirs::data_local_dir(), &[r"Chromium\User Data"]),
        Browser::Brave => (
            dirs::data_local_dir(),
            &[r"BraveSoftware\Brave-Browser\User Data"],
        ),
        Browser::Vivaldi => (dirs::data_local_dir(), &[r"Vivaldi\User Data"]),
        Browser::Edge => (dirs::data_local_dir(), &[r"Microsoft\Edge\User Data"]),
        Browser::Firefox => (dirs::data_dir(), &[r"Mozilla\Firefox"]),
        Browser::LibreWolf => (dirs::data_dir(), &["librewolf"]),
        Browser::Zen => (dirs::data_dir(), &["zen"]),
        Browser::Floorp => (dirs::data_dir(), &["Floorp"]),
    };
    base.map(|base| dirs.iter().map(|d| base.join(d)).collect())
        .unwrap_or_default()
}

/// Every place `browser` may keep its profiles, existing or not.
/// `config` is the XDG config directory, `None` on macOS.
fn roots(browser: Browser, home: &Path, config: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let Some(config) = config else {
        let support = home.join("Library/Application Support");
        roots.extend(data_dirs(browser, true).iter().map(|d| support.join(d)));
        return roots;
    };
    roots.extend(data_dirs(browser, false).iter().map(|d| config.join(d)));
    roots.extend(home_dirs(browser).iter().map(|d| home.join(d)));
    if let Some(id) = flatpak_id(browser) {
        let app = home.join(".var/app").join(id);
        roots.extend(
            data_dirs(browser, false)
                .iter()
                .map(|d| app.join("config").join(d)),
        );
        roots.extend(home_dirs(browser).iter().map(|d| app.join(d)));
    }
    roots
}

pub fn list() -> Vec<Profile> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let config = if cfg!(target_os = "macos") {
        None
    } else {
        dirs::config_dir()
    };
    let roots_of = |browser| {
        if cfg!(windows) {
            windows_roots(browser)
        } else {
            roots(browser, &home, config.as_deref())
        }
    };
    Browser::ALL
        .into_iter()
        .flat_map(|browser| {
            roots_of(browser)
                .into_iter()
                .flat_map(move |root| match browser.engine() {
                    Engine::Chromium => chromium_profiles(browser, &root),
                    Engine::Gecko => gecko_profiles(browser, &root),
                })
        })
        .collect()
}

fn has_chromium_cookies(dir: &Path) -> bool {
    dir.join("Network/Cookies").is_file() || dir.join("Cookies").is_file()
}

/// The profiles `Local State` knows about, in the browser's own order, that
/// still have a cookie store.
fn chromium_profiles(browser: Browser, root: &Path) -> Vec<Profile> {
    let Ok(text) = std::fs::read_to_string(root.join("Local State")) else {
        return Vec::new();
    };
    let Ok(state) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(cache) = state["profile"]["info_cache"].as_object() else {
        return Vec::new();
    };
    let mut keys: Vec<&String> = cache.keys().collect();
    let order: Vec<&str> = state["profile"]["profiles_order"]
        .as_array()
        .map(|o| o.iter().filter_map(|k| k.as_str()).collect())
        .unwrap_or_default();
    keys.sort_by_key(|k| {
        (
            order.iter().position(|o| o == k).unwrap_or(usize::MAX),
            k.as_str() != "Default",
            k.as_str(),
        )
    });
    keys.into_iter()
        .filter(|key| has_chromium_cookies(&root.join(key)))
        .map(|key| {
            let info = &cache[key.as_str()];
            let text = |field: &str| {
                info[field]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            };
            Profile {
                browser,
                dir: root.join(key),
                name: text("name").unwrap_or_else(|| key.clone()),
                email: text("user_name"),
            }
        })
        .collect()
}

/// `[Profile*]` sections of `profiles.ini` whose directory has a cookie store,
/// the default one first.
fn gecko_profiles(browser: Browser, root: &Path) -> Vec<Profile> {
    let Ok(text) = std::fs::read_to_string(root.join("profiles.ini")) else {
        return Vec::new();
    };
    let mut profiles: Vec<(bool, Profile)> = Vec::new();
    let mut section: Option<(Option<String>, Option<String>, bool, bool)> = None;
    let mut finish = |section: Option<(Option<String>, Option<String>, bool, bool)>| {
        let Some((Some(name), Some(path), relative, default)) = section else {
            return;
        };
        let dir = if relative {
            root.join(&path)
        } else {
            PathBuf::from(&path)
        };
        if dir.join("cookies.sqlite").is_file() {
            profiles.push((
                default,
                Profile {
                    browser,
                    dir,
                    name,
                    email: None,
                },
            ));
        }
    };
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            finish(section.take());
            if line.starts_with("[Profile") {
                section = Some((None, None, true, false));
            }
            continue;
        }
        let (Some(current), Some((key, value))) = (section.as_mut(), line.split_once('=')) else {
            continue;
        };
        match key.trim() {
            "Name" => current.0 = Some(value.trim().to_owned()),
            "Path" => current.1 = Some(value.trim().to_owned()),
            "IsRelative" => current.2 = value.trim() == "1",
            "Default" => current.3 = value.trim() == "1",
            _ => {}
        }
    }
    finish(section.take());
    profiles.sort_by_key(|(default, _)| !default);
    profiles.into_iter().map(|(_, p)| p).collect()
}

/// yt-dlp's name for the browser, which also picks the keyring entry it
/// decrypts with. Helium keeps upstream Chromium's on Linux.
fn ytdlp_browser(browser: Browser) -> &'static str {
    match browser {
        Browser::Chrome => "chrome",
        Browser::Brave => "brave",
        Browser::Vivaldi => "vivaldi",
        Browser::Edge => "edge",
        Browser::Chromium | Browser::Helium => "chromium",
        Browser::Firefox | Browser::LibreWolf | Browser::Zen | Browser::Floorp => "firefox",
    }
}

/// Keyrings to try in turn. A browser started with `--password-store=basic`
/// encrypts with a fixed key yt-dlp only uses when told `basictext`, and
/// nothing in the profile says which store it used.
fn keyrings(browser: Browser) -> &'static [Option<&'static str>] {
    if browser.engine() == Engine::Chromium && cfg!(target_os = "linux") {
        &[None, Some("basictext")]
    } else {
        &[None]
    }
}

/// The unexpired cookies of a Netscape cookie file, `#HttpOnly_` ones
/// included. A profile keeps expired cookies until the browser purges them,
/// and yt-dlp copies them out with the rest.
pub fn parse_netscape(file: &str, now: u64) -> Vec<Cookie> {
    file.lines()
        .filter_map(|line| {
            let line = line.strip_prefix("#HttpOnly_").unwrap_or(line);
            if line.starts_with('#') {
                return None;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            let [domain, _, _, _, expires, name, value] = fields[..] else {
                return None;
            };
            let expires: u64 = expires.parse().unwrap_or(0);
            if expires != 0 && expires < now {
                return None;
            }
            Some(Cookie {
                domain: domain.to_owned(),
                name: name.to_owned(),
                value: value.trim_end().to_owned(),
            })
        })
        .collect()
}

/// The `Cookie` header of the YouTube session in `profile`.
pub async fn import(profile: &Profile, scratch: &Path) -> Result<String, String> {
    read(profile, scratch, signed_in_header)
        .await?
        .ok_or_else(|| {
            format!(
                "No YouTube Music session in {} ({}). Sign in to music.youtube.com there first.",
                profile.browser.name(),
                profile.name
            )
        })
}

/// The first answer `pick` finds in the profile's cookies, trying each
/// keyring in turn.
pub async fn read<T>(
    profile: &Profile,
    scratch: &Path,
    pick: impl Fn(&[Cookie]) -> Option<T>,
) -> Result<Option<T>, String> {
    crate::config::create_private_dir(scratch).map_err(|e| e.to_string())?;
    for keyring in keyrings(profile.browser) {
        let spec = match keyring {
            Some(keyring) => format!(
                "{}+{keyring}:{}",
                ytdlp_browser(profile.browser),
                profile.dir.display()
            ),
            None => format!(
                "{}:{}",
                ytdlp_browser(profile.browser),
                profile.dir.display()
            ),
        };
        let file = CookieFile::create(scratch).map_err(|e| e.to_string())?;
        // yt-dlp saves the cookies it read before it complains that there is
        // no URL to download, so its exit status says nothing here.
        let output = crate::streams::ytdlp_command()
            .arg("--ignore-config")
            .arg("--cookies-from-browser")
            .arg(&spec)
            .arg("--cookies")
            .arg(&file.path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| format!("could not run yt-dlp ({e})"))?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or_default();
        let cookies = parse_netscape(
            &std::fs::read_to_string(&file.path).unwrap_or_default(),
            now,
        );
        drop(file);
        if let Some(found) = pick(&cookies) {
            return Ok(Some(found));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::info!(
            browser = profile.browser.id(),
            keyring,
            read = cookies.len(),
            "no matching cookies in the profile: {}",
            stderr
                .lines()
                .rfind(|l| !l.contains("provide at least one URL"))
                .unwrap_or_default()
        );
    }
    Ok(None)
}

/// A 0600 file yt-dlp writes every cookie of the profile to, removed when
/// dropped. It starts with the header yt-dlp checks before writing to it.
struct CookieFile {
    path: PathBuf,
}

impl CookieFile {
    fn create(dir: &Path) -> std::io::Result<Self> {
        let path = dir.join(format!("import-{}.txt", fastrand::u64(..)));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&path)?;
        file.write_all(b"# Netscape HTTP Cookie File\n")?;
        Ok(Self { path })
    }
}

impl Drop for CookieFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn lists_chromium_profiles_from_local_state() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join("Local State"),
            r#"{"profile":{"profiles_order":["Profile 1","Default"],"info_cache":{
                "Default":{"name":"Personal","user_name":""},
                "Profile 1":{"name":"Work","user_name":"me@example.com"},
                "Profile 2":{"name":"Deleted"}}}}"#,
        );
        write(&root.join("Default/Network/Cookies"), "");
        write(&root.join("Profile 1/Cookies"), "");
        let profiles = chromium_profiles(Browser::Helium, root);
        let names: Vec<_> = profiles
            .iter()
            .map(|p| (p.name.as_str(), p.email.as_deref()))
            .collect();
        assert_eq!(
            names,
            [("Work", Some("me@example.com")), ("Personal", None)]
        );
        assert_eq!(profiles[1].dir, root.join("Default"));
    }

    #[test]
    fn lists_gecko_profiles_from_profiles_ini() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let elsewhere = root.join("elsewhere");
        write(
            &root.join("profiles.ini"),
            &format!(
                "[General]\nStartWithLastProfile=1\n\n\
                 [Profile1]\nName=dev\nIsRelative=1\nPath=abc.dev\n\n\
                 [Profile0]\nName=default-release\nIsRelative=1\nPath=xyz.default-release\nDefault=1\n\n\
                 [Profile2]\nName=empty\nIsRelative=1\nPath=empty\n\n\
                 [Profile3]\nName=absolute\nIsRelative=0\nPath={}\n\n\
                 [Install4F96D1932A9F858E]\nDefault=xyz.default-release\n",
                elsewhere.display()
            ),
        );
        write(&root.join("abc.dev/cookies.sqlite"), "");
        write(&root.join("xyz.default-release/cookies.sqlite"), "");
        write(&elsewhere.join("cookies.sqlite"), "");
        let profiles = gecko_profiles(Browser::Firefox, root);
        let names: Vec<_> = profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["default-release", "dev", "absolute"]);
        assert_eq!(profiles[2].dir, elsewhere);
    }

    #[test]
    fn roots_cover_nix_flatpak_and_mac_layouts() {
        let home = Path::new("/home/u");
        let config = Some(Path::new("/home/u/.config"));
        let linux = roots(Browser::Helium, home, config);
        assert_eq!(linux, [home.join(".config/net.imput.helium")]);
        let firefox = roots(Browser::Firefox, home, config);
        assert!(firefox.contains(&home.join(".mozilla/firefox")));
        assert!(firefox.contains(&home.join(".var/app/org.mozilla.firefox/.mozilla/firefox")));
        let mac = roots(Browser::Chrome, Path::new("/Users/u"), None);
        assert_eq!(
            mac,
            [PathBuf::from(
                "/Users/u/Library/Application Support/Google/Chrome"
            )]
        );
    }

    #[test]
    fn reads_netscape_files() {
        let cookies = parse_netscape(
            "# Netscape HTTP Cookie File\n\
             #HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t0\tSID\ts\n\
             .youtube.com\tTRUE\t/\tTRUE\t2000\tSAPISID\ta\n\
             .youtube.com\tTRUE\t/\tTRUE\t999\tST-old\tx\n\
             .example.com\tTRUE\t/\tTRUE\t0\tN\tn\n",
            1000,
        );
        assert_eq!(cookies.len(), 3);
        assert_eq!(
            signed_in_header(&cookies).as_deref(),
            Some("SID=s; SAPISID=a")
        );
    }

    #[cfg(unix)]
    #[test]
    fn cookie_files_are_private_and_removed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = CookieFile::create(dir.path()).unwrap();
        let mode = std::fs::metadata(&file.path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let path = file.path.clone();
        drop(file);
        assert!(!path.exists());
    }
}
