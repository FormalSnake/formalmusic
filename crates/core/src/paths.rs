//! Where the client keeps things on disk.

use std::path::PathBuf;

/// `$XDG_CACHE_HOME/formalmusic`: `state.json` and the artwork cache.
pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("formalmusic")
}

pub fn art_dir() -> PathBuf {
    cache_dir().join("art")
}

/// `$XDG_CONFIG_HOME/formalmusic`, falling back to `~/.config` on every
/// platform, since matugen writes `theme.json` there on Linux and the same
/// path keeps a Mac dev run themable.
pub fn config_dir() -> PathBuf {
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    config_home.join("formalmusic")
}

pub fn theme_file() -> PathBuf {
    config_dir().join("theme.json")
}
