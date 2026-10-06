//! Which browsers are installed, which one is the system default, and how to
//! start each in a throwaway profile.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    Chromium,
    Gecko,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Browser {
    Helium,
    Chrome,
    Chromium,
    Brave,
    Vivaldi,
    Edge,
    Firefox,
    LibreWolf,
    Zen,
    Floorp,
}

impl Browser {
    /// The order a sign-in without a default browser tries them in.
    pub const ALL: [Browser; 10] = [
        Browser::Helium,
        Browser::Chrome,
        Browser::Chromium,
        Browser::Brave,
        Browser::Vivaldi,
        Browser::Edge,
        Browser::Firefox,
        Browser::LibreWolf,
        Browser::Zen,
        Browser::Floorp,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Browser::Helium => "helium",
            Browser::Chrome => "chrome",
            Browser::Chromium => "chromium",
            Browser::Brave => "brave",
            Browser::Vivaldi => "vivaldi",
            Browser::Edge => "edge",
            Browser::Firefox => "firefox",
            Browser::LibreWolf => "librewolf",
            Browser::Zen => "zen",
            Browser::Floorp => "floorp",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Browser::Helium => "Helium",
            Browser::Chrome => "Chrome",
            Browser::Chromium => "Chromium",
            Browser::Brave => "Brave",
            Browser::Vivaldi => "Vivaldi",
            Browser::Edge => "Edge",
            Browser::Firefox => "Firefox",
            Browser::LibreWolf => "LibreWolf",
            Browser::Zen => "Zen",
            Browser::Floorp => "Floorp",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.id() == id)
    }

    pub fn engine(self) -> Engine {
        match self {
            Browser::Firefox | Browser::LibreWolf | Browser::Zen | Browser::Floorp => Engine::Gecko,
            _ => Engine::Chromium,
        }
    }

    fn binaries(self) -> &'static [&'static str] {
        match self {
            Browser::Helium => &["helium", "helium-browser"],
            Browser::Chrome => &["google-chrome-stable", "google-chrome", "chrome"],
            Browser::Chromium => &["chromium", "chromium-browser"],
            Browser::Brave => &["brave", "brave-browser"],
            Browser::Vivaldi => &["vivaldi", "vivaldi-stable"],
            Browser::Edge => &["microsoft-edge", "microsoft-edge-stable"],
            Browser::Firefox => &["firefox", "firefox-esr"],
            Browser::LibreWolf => &["librewolf"],
            Browser::Zen => &["zen", "zen-browser", "zen-beta"],
            Browser::Floorp => &["floorp"],
        }
    }

    fn flatpak_ids(self) -> &'static [&'static str] {
        match self {
            Browser::Helium => &[],
            Browser::Chrome => &["com.google.Chrome"],
            Browser::Chromium => &["org.chromium.Chromium"],
            Browser::Brave => &["com.brave.Browser"],
            Browser::Vivaldi => &["com.vivaldi.Vivaldi"],
            Browser::Edge => &["com.microsoft.Edge"],
            Browser::Firefox => &["org.mozilla.firefox"],
            Browser::LibreWolf => &["io.gitlab.librewolf-community"],
            Browser::Zen => &["app.zen_browser.zen"],
            Browser::Floorp => &["one.ablaze.floorp"],
        }
    }

    /// Executables inside the app bundle, relative to an Applications folder.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn mac_apps(self) -> &'static [&'static str] {
        match self {
            Browser::Helium => &["Helium.app/Contents/MacOS/Helium"],
            Browser::Chrome => &["Google Chrome.app/Contents/MacOS/Google Chrome"],
            Browser::Chromium => &["Chromium.app/Contents/MacOS/Chromium"],
            Browser::Brave => &["Brave Browser.app/Contents/MacOS/Brave Browser"],
            Browser::Vivaldi => &["Vivaldi.app/Contents/MacOS/Vivaldi"],
            Browser::Edge => &["Microsoft Edge.app/Contents/MacOS/Microsoft Edge"],
            Browser::Firefox => &["Firefox.app/Contents/MacOS/firefox"],
            Browser::LibreWolf => &["LibreWolf.app/Contents/MacOS/librewolf"],
            Browser::Zen => &[
                "Zen.app/Contents/MacOS/zen",
                "Zen Browser.app/Contents/MacOS/zen",
            ],
            Browser::Floorp => &["Floorp.app/Contents/MacOS/floorp"],
        }
    }
}

/// How to start an installed browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launcher {
    Binary(PathBuf),
    Flatpak(String),
}

impl Launcher {
    /// The command with the profile directory made visible to it; a flatpak
    /// only sees its own data unless told otherwise.
    pub fn command(&self, profile: &Path) -> Command {
        match self {
            Launcher::Binary(path) => Command::new(path),
            Launcher::Flatpak(id) => {
                let mut command = Command::new("flatpak");
                command
                    .arg("run")
                    .arg(format!("--filesystem={}", profile.display()))
                    .arg(id);
                command
            }
        }
    }
}

/// Where to look for executables. A user service gets a short `PATH`, so the
/// Nix profile directories are searched even when it leaves them out.
fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    if let Some(user) = std::env::var_os("USER") {
        let mut dir = OsString::from("/etc/profiles/per-user/");
        dir.push(user);
        dirs.push(PathBuf::from(dir).join("bin"));
    }
    dirs.push("/run/current-system/sw/bin".into());
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".nix-profile/bin"));
    }
    dirs
}

fn flatpak_export_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(data) = dirs::data_dir() {
        dirs.push(data.join("flatpak/exports/bin"));
    }
    dirs.push("/var/lib/flatpak/exports/bin".into());
    dirs
}

fn mac_app_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/Applications")];
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join("Applications"));
    }
    dirs
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

pub fn find(browser: Browser) -> Option<Launcher> {
    find_in(
        browser,
        &search_dirs(),
        &flatpak_export_dirs(),
        if cfg!(target_os = "macos") {
            mac_app_dirs()
        } else {
            Vec::new()
        }
        .as_slice(),
    )
}

fn find_in(
    browser: Browser,
    bin_dirs: &[PathBuf],
    flatpak_dirs: &[PathBuf],
    app_dirs: &[PathBuf],
) -> Option<Launcher> {
    let binary = browser
        .binaries()
        .iter()
        .flat_map(|name| bin_dirs.iter().map(move |dir| dir.join(name)))
        .chain(
            browser
                .mac_apps()
                .iter()
                .flat_map(|app| app_dirs.iter().map(move |dir| dir.join(app))),
        )
        .find(|path| is_executable(path));
    if let Some(path) = binary {
        return Some(Launcher::Binary(path));
    }
    browser
        .flatpak_ids()
        .iter()
        .find(|id| flatpak_dirs.iter().any(|dir| dir.join(id).exists()))
        .map(|id| Launcher::Flatpak((*id).to_owned()))
}

pub fn installed() -> Vec<(Browser, Launcher)> {
    Browser::ALL
        .into_iter()
        .filter_map(|b| find(b).map(|launcher| (b, launcher)))
        .collect()
}

/// Maps whatever the platform calls its default handler (a `.desktop` file,
/// a bundle id) to a browser. The more specific names come first so
/// `chromium` never reads as Chrome and `librewolf` never as Firefox.
fn from_handler(handler: &str) -> Option<Browser> {
    const NAMES: [(&str, Browser); 10] = [
        ("librewolf", Browser::LibreWolf),
        ("floorp", Browser::Floorp),
        ("zen", Browser::Zen),
        ("firefox", Browser::Firefox),
        ("helium", Browser::Helium),
        ("chromium", Browser::Chromium),
        ("chrome", Browser::Chrome),
        ("brave", Browser::Brave),
        ("vivaldi", Browser::Vivaldi),
        ("edge", Browser::Edge),
    ];
    let handler = handler.trim().to_ascii_lowercase();
    if handler.is_empty() {
        return None;
    }
    NAMES
        .into_iter()
        .find(|(name, _)| handler.contains(name))
        .map(|(_, browser)| browser)
}

/// The `x-scheme-handler/https` entry of the first `mimeapps.list` that has
/// one, for when `xdg-settings` is missing.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn handler_from_mimeapps(files: &[PathBuf]) -> Option<String> {
    files.iter().find_map(|file| {
        let text = std::fs::read_to_string(file).ok()?;
        let mut in_defaults = false;
        text.lines().find_map(|line| {
            let line = line.trim();
            if line.starts_with('[') {
                in_defaults = line == "[Default Applications]";
                return None;
            }
            let value = line.strip_prefix("x-scheme-handler/https=")?;
            in_defaults
                .then(|| value.split(';').next().unwrap_or_default().to_owned())
                .filter(|v| !v.is_empty())
        })
    })
}

async fn stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
}

#[cfg(not(target_os = "macos"))]
async fn default_handler() -> Option<String> {
    if let Some(handler) = stdout("xdg-settings", &["get", "default-web-browser"]).await {
        return Some(handler);
    }
    let mut files = Vec::new();
    if let Some(config) = dirs::config_dir() {
        files.push(config.join("mimeapps.list"));
    }
    if let Some(data) = dirs::data_dir() {
        files.push(data.join("applications/mimeapps.list"));
    }
    files.push("/etc/xdg/mimeapps.list".into());
    handler_from_mimeapps(&files)
}

/// LaunchServices keeps the handlers in a binary plist; `plutil` turns it
/// into JSON.
#[cfg(target_os = "macos")]
async fn default_handler() -> Option<String> {
    let plist = dirs::home_dir()?
        .join("Library/Preferences/com.apple.LaunchServices/com.apple.launchservices.secure.plist");
    let json = stdout(
        "plutil",
        &["-convert", "json", "-o", "-", &plist.to_string_lossy()],
    )
    .await?;
    let value: serde_json::Value = serde_json::from_str(&json).ok()?;
    value
        .get("LSHandlers")?
        .as_array()?
        .iter()
        .find(|h| h.get("LSHandlerURLScheme").and_then(|s| s.as_str()) == Some("https"))
        .and_then(|h| {
            h.get("LSHandlerRoleAll")
                .or_else(|| h.get("LSHandlerRoleViewer"))
        })
        .and_then(|role| role.as_str())
        .map(str::to_owned)
}

pub async fn system_default() -> Option<Browser> {
    let handler = default_handler().await?;
    let browser = from_handler(&handler);
    tracing::debug!(handler, browser = ?browser.map(Browser::id), "system default browser");
    browser
}

/// The browser a sign-in without an explicit choice opens: the system default
/// when it is installed, else the first installed one.
pub fn pick_default(installed: &[(Browser, Launcher)], system: Option<Browser>) -> Option<Browser> {
    system
        .filter(|s| installed.iter().any(|(b, _)| b == s))
        .or_else(|| installed.first().map(|(b, _)| *b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn touch(path: &Path, mode: u32) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn handlers_name_their_browser() {
        assert_eq!(from_handler("helium.desktop\n"), Some(Browser::Helium));
        assert_eq!(from_handler("net.imput.helium"), Some(Browser::Helium));
        assert_eq!(
            from_handler("org.chromium.Chromium.desktop"),
            Some(Browser::Chromium)
        );
        assert_eq!(from_handler("google-chrome.desktop"), Some(Browser::Chrome));
        assert_eq!(from_handler("com.google.chrome"), Some(Browser::Chrome));
        assert_eq!(from_handler("org.mozilla.firefox"), Some(Browser::Firefox));
        assert_eq!(from_handler("librewolf.desktop"), Some(Browser::LibreWolf));
        assert_eq!(from_handler("app.zen_browser.zen"), Some(Browser::Zen));
        assert_eq!(from_handler("com.microsoft.edgemac"), Some(Browser::Edge));
        assert_eq!(from_handler("com.apple.safari"), None);
        assert_eq!(from_handler(""), None);
    }

    #[test]
    fn mimeapps_default_comes_from_the_defaults_section() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mimeapps.list");
        std::fs::write(
            &file,
            "[Added Associations]\nx-scheme-handler/https=firefox.desktop;\n\
             [Default Applications]\ntext/html=helium.desktop\n\
             x-scheme-handler/https=helium.desktop;firefox.desktop;\n",
        )
        .unwrap();
        let missing = dir.path().join("none.list");
        assert_eq!(
            handler_from_mimeapps(&[missing, file]).as_deref(),
            Some("helium.desktop")
        );
    }

    #[test]
    fn finds_binaries_then_flatpaks() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let flatpak = dir.path().join("flatpak");
        touch(&bin.join("helium"), 0o755);
        // Not executable, so not a browser.
        touch(&bin.join("chromium"), 0o644);
        touch(&flatpak.join("org.chromium.Chromium"), 0o755);
        touch(&flatpak.join("org.mozilla.firefox"), 0o755);
        touch(&bin.join("firefox"), 0o755);
        let (bins, flatpaks) = (vec![bin.clone()], vec![flatpak]);

        assert_eq!(
            find_in(Browser::Helium, &bins, &flatpaks, &[]),
            Some(Launcher::Binary(bin.join("helium")))
        );
        assert_eq!(
            find_in(Browser::Chromium, &bins, &flatpaks, &[]),
            Some(Launcher::Flatpak("org.chromium.Chromium".into()))
        );
        assert_eq!(
            find_in(Browser::Firefox, &bins, &flatpaks, &[]),
            Some(Launcher::Binary(bin.join("firefox")))
        );
        assert_eq!(find_in(Browser::Brave, &bins, &flatpaks, &[]), None);
    }

    #[test]
    fn finds_mac_app_bundles() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir
            .path()
            .join("Google Chrome.app/Contents/MacOS/Google Chrome");
        touch(&exe, 0o755);
        assert_eq!(
            find_in(Browser::Chrome, &[], &[], &[dir.path().to_owned()]),
            Some(Launcher::Binary(exe))
        );
    }

    #[test]
    fn default_falls_back_to_the_first_installed() {
        let installed = vec![
            (Browser::Chromium, Launcher::Binary("/bin/chromium".into())),
            (Browser::Firefox, Launcher::Binary("/bin/firefox".into())),
        ];
        assert_eq!(
            pick_default(&installed, Some(Browser::Firefox)),
            Some(Browser::Firefox)
        );
        assert_eq!(
            pick_default(&installed, Some(Browser::Brave)),
            Some(Browser::Chromium)
        );
        assert_eq!(pick_default(&installed, None), Some(Browser::Chromium));
        assert_eq!(pick_default(&[], Some(Browser::Brave)), None);
    }

    #[test]
    fn ids_round_trip() {
        for browser in Browser::ALL {
            assert_eq!(Browser::from_id(browser.id()), Some(browser));
        }
    }
}
