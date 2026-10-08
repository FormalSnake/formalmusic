//! `daemon.json` and the directories the daemon keeps its files in. The
//! daemon only reads the config; Home Manager or the user owns it. It also
//! reads the keys of the app's `config.json` that it acts on: the tray, and
//! the playback settings the Settings dialog writes, which win over
//! `daemon.json`'s.

use formalmusic_api::Equalizer;
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
    /// Radio from the last track once a list runs out, as the web app's
    /// autoplay does.
    pub autoplay: bool,
    /// Leave tracks YouTube marks explicit out of the queue.
    pub restrict_explicit: bool,
    pub equalizer: Equalizer,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            report_history: true,
            normalisation: true,
            crossfade_ms: 0,
            preferred_quality: Quality::Auto,
            autoplay: true,
            restrict_explicit: false,
            equalizer: Equalizer::default(),
        }
    }
}

/// The web app's audio quality setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    /// The web app adapts to the connection as it plays; one format plays
    /// a whole track here, so Auto takes the best one, as `High` does.
    #[default]
    Auto,
    /// About 50 kbps.
    Low,
    /// About 130 kbps, never the Premium formats.
    Normal,
    /// The best format offered, Premium included.
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

/// The keys of the app's `config.json` the daemon acts on. A playback key
/// left out keeps `daemon.json`'s value.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppSettings {
    pub show_in_tray: bool,
    pub audio_quality: Option<Quality>,
    pub autoplay: Option<bool>,
    pub restrict_explicit: Option<bool>,
    pub pause_history: Option<bool>,
    pub equalizer: Option<Equalizer>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            show_in_tray: true,
            audio_quality: None,
            autoplay: None,
            restrict_explicit: None,
            pause_history: None,
            equalizer: None,
        }
    }
}

/// The playback settings in force: `daemon.json`, overridden by the app's
/// `config.json`. Read again on [`formalmusic_api::Command::ReloadSettings`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Preferences {
    pub quality: Quality,
    pub autoplay: bool,
    pub restrict_explicit: bool,
    pub report_history: bool,
    pub equalizer: Equalizer,
}

impl Preferences {
    pub fn new(config: &Config, app: &AppSettings) -> Self {
        Self {
            quality: app.audio_quality.unwrap_or(config.preferred_quality),
            autoplay: app.autoplay.unwrap_or(config.autoplay),
            restrict_explicit: app.restrict_explicit.unwrap_or(config.restrict_explicit),
            report_history: app
                .pause_history
                .map_or(config.report_history, |paused| !paused),
            equalizer: app.equalizer.unwrap_or(config.equalizer),
        }
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
/// leaves half a file, with `mode` set before any byte lands. Windows has no
/// modes; the profile directories these live in are the user's alone.
pub fn write_private(path: &Path, contents: &[u8], mode: u32) -> std::io::Result<()> {
    use std::io::Write;

    if let Some(dir) = path.parent() {
        create_private_dir(dir)?;
    }
    let tmp = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, mode);
    #[cfg(not(unix))]
    let _ = mode;
    let mut file = options.open(&tmp)?;
    file.write_all(contents)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)
}

#[cfg(not(unix))]
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

#[cfg(unix)]
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
    fn the_apps_settings_win_over_daemon_json() {
        let config: Config = serde_json::from_str(
            r#"{"preferredQuality": "low", "reportHistory": false, "autoplay": false}"#,
        )
        .unwrap();
        let untouched = Preferences::new(&config, &AppSettings::default());
        assert_eq!(
            untouched,
            Preferences {
                quality: Quality::Low,
                autoplay: false,
                restrict_explicit: false,
                report_history: false,
                equalizer: Equalizer::default(),
            }
        );
        let app: AppSettings = serde_json::from_str(
            r#"{"audioQuality": "high", "pauseHistory": false, "restrictExplicit": true}"#,
        )
        .unwrap();
        let chosen = Preferences::new(&config, &app);
        assert_eq!(chosen.quality, Quality::High);
        assert!(chosen.report_history && chosen.restrict_explicit && !chosen.autoplay);
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

    #[cfg(unix)]
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
