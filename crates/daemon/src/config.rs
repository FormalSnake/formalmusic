//! `daemon.json` and the directories the daemon keeps its files in. The
//! daemon only reads the config; Home Manager or the user owns it. It also
//! reads the one key of the app's `config.json` that it acts on.

use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    /// Report plays to YouTube so History and recommendations follow them.
    pub report_history: bool,
    pub normalisation: bool,
    pub crossfade_ms: u32,
    pub preferred_quality: Quality,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            report_history: true,
            normalisation: true,
            crossfade_ms: 0,
            preferred_quality: Quality::High,
        }
    }
}

/// The web app's audio quality setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    /// About 50 kbps.
    Low,
    /// About 130 kbps, never the Premium formats.
    Normal,
    /// The best format offered, Premium included.
    #[default]
    High,
}

impl Config {
    /// Defaults when the file is missing; defaults and a warning when it is
    /// unreadable, so a typo never keeps the music from starting.
    pub fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                tracing::warn!(path = %path.display(), "cannot read config: {e}");
                return Self::default();
            }
        };
        serde_json::from_str(&text).unwrap_or_else(|e| {
            tracing::warn!(path = %path.display(), "ignoring invalid config: {e}");
            Self::default()
        })
    }
}

/// The keys of the app's `config.json` the daemon acts on.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    pub show_in_tray: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self { show_in_tray: true }
    }
}

impl AppSettings {
    /// Defaults when the file is missing or broken; the app warns about it.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
pub struct Paths {
    /// `$XDG_STATE_HOME/formalmusic`: session, cookies, queue.
    pub state: PathBuf,
    pub config: PathBuf,
    /// The app's `config.json`, beside `daemon.json`.
    pub app_settings: PathBuf,
}

impl Paths {
    pub fn from_env() -> Self {
        let state = dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .unwrap_or_else(std::env::temp_dir)
            .join("formalmusic");
        let dir = dirs::config_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("formalmusic");
        Self {
            state,
            config: dir.join("daemon.json"),
            app_settings: dir.join("config.json"),
        }
    }

    pub fn session(&self) -> PathBuf {
        self.state.join("session.json")
    }

    /// Throwaway browser profiles of a running sign-in.
    pub fn signin(&self) -> PathBuf {
        self.state.join("signin")
    }

    pub fn queue(&self) -> PathBuf {
        self.state.join("queue.json")
    }
}

/// Writes `contents` through a temporary file and a rename, so a crash never
/// leaves half a file, with `mode` set before any byte lands.
pub fn write_private(path: &Path, contents: &[u8], mode: u32) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    if let Some(dir) = path.parent() {
        create_private_dir(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(&tmp)?;
    file.write_all(contents)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)
}

pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_config_keeps_defaults() {
        let config: Config =
            serde_json::from_str(r#"{"crossfadeMs": 3000, "preferredQuality": "low"}"#).unwrap();
        assert_eq!(config.crossfade_ms, 3000);
        assert_eq!(config.preferred_quality, Quality::Low);
        assert!(config.report_history && config.normalisation);
    }

    #[test]
    fn missing_or_broken_config_is_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Config::load(&dir.path().join("none.json")),
            Config::default()
        );
        let broken = dir.path().join("broken.json");
        std::fs::write(&broken, "{").unwrap();
        assert_eq!(Config::load(&broken), Config::default());
    }

    #[test]
    fn private_files_are_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/b/session.json");
        write_private(&path, b"{}", 0o600).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let dir_mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(dir_mode & 0o777, 0o700);
    }
}
