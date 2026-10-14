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
