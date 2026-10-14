//! The settings the daemon plays by and the directories it keeps its files
//! in. Settings come from the `config.toml` the app writes (through
//! `formalmusic_api::settings`); the daemon only reads it.

use formalmusic_api::Equalizer;
pub use formalmusic_api::settings::Quality;
use formalmusic_api::settings::Settings;
use std::path::{Path, PathBuf};

/// The playback settings in force, from `config.toml` (the app's Settings
/// window, Home Manager, `config.d/`), read again whenever the file changes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Preferences {
    pub quality: Quality,
    pub autoplay: bool,
    pub restrict_explicit: bool,
    pub report_history: bool,
    pub normalisation: bool,
    pub crossfade_ms: u32,
    pub equalizer: Equalizer,
    pub show_in_tray: bool,
}

impl From<&Settings> for Preferences {
    fn from(s: &Settings) -> Self {
        Self {
            quality: s.playback.quality,
            autoplay: s.playback.autoplay,
            restrict_explicit: s.playback.restrict_explicit,
            report_history: s.privacy.report_history,
            normalisation: s.playback.normalisation,
            crossfade_ms: s.playback.crossfade_ms,
            equalizer: s.playback.equalizer,
            show_in_tray: s.window.show_in_tray,
        }
    }
}

impl Default for Preferences {
    fn default() -> Self {
        Self::from(&Settings::default())
    }
}

#[derive(Debug, Clone)]
pub struct Paths {
    /// `$XDG_STATE_HOME/formalmusic`: session, cookies, queue.
    pub state: PathBuf,
}

impl Paths {
    pub fn from_env() -> Self {
        let state = dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .unwrap_or_else(std::env::temp_dir)
            .join("formalmusic");
        Self { state }
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
