//! `config.json`: the client's settings. The user or Home Manager owns it;
//! the Settings dialog writes back only the switches it shows.

use std::path::Path;

use crate::equalizer::Equalizer;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Play the album's animated cover in the player bar too, not only in
    /// the expanded player.
    pub animated_cover_in_bar: bool,
    /// Leave kopuzd playing when the last window closes, instead of
    /// pausing it.
    pub keep_playing_when_closed: bool,
    /// Show a tray icon while a track is loaded.
    pub show_in_tray: bool,
    /// The keys below are kopuzd's: the app hands them to it on connect and
    /// whenever the Settings dialog changes one.
    pub equalizer: Equalizer,
    /// Level tracks by the loudness YouTube reports for them.
    pub normalisation: bool,
    /// Fade from one track into the next over this long; 0 plays gapless.
    pub crossfade_ms: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            animated_cover_in_bar: true,
            keep_playing_when_closed: false,
            show_in_tray: true,
            equalizer: Equalizer::default(),
            normalisation: true,
            crossfade_ms: 0,
        }
    }
}

impl Settings {
    /// kopuzd fades in whole seconds; any fade at all is at least one.
    pub fn crossfade_seconds(&self) -> u8 {
        self.crossfade_ms.div_ceil(1000).min(u8::MAX as u32) as u8
    }

    /// Defaults when the file is missing or unreadable, with a warning for
    /// the second, so a typo never keeps the window from opening.
    pub fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                tracing::warn!("settings: cannot read {}: {error}", path.display());
                return Self::default();
            }
        };
        serde_json::from_str(&text).unwrap_or_else(|error| {
            tracing::warn!("settings: ignoring {}: {error}", path.display());
            Self::default()
        })
    }

    /// Sets one camelCase key in the file, keeping every other key as it is.
    /// A file Home Manager links in from the store is left alone.
    pub fn write(path: &Path, key: &str, value: serde_json::Value) -> std::io::Result<()> {
        if path.is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("{} is a link, so it is managed elsewhere", path.display()),
            ));
        }
        let mut file = match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(std::io::Error::other)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::Map::new(),
            Err(error) => return Err(error),
        };
        file.insert(key.to_owned(), value);
        let mut text = serde_json::to_string_pretty(&file).map_err(std::io::Error::other)?;
        text.push('\n');
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_keys_keep_their_defaults() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert!(settings.animated_cover_in_bar && settings.normalisation);
        let settings: Settings = serde_json::from_str(r#"{"animatedCoverInBar":false}"#).unwrap();
        assert!(!settings.animated_cover_in_bar);
        assert!(!settings.keep_playing_when_closed);
        assert!(settings.show_in_tray);
        let settings: Settings = serde_json::from_str(r#"{"crossfadeMs":1500}"#).unwrap();
        assert_eq!(settings.crossfade_seconds(), 2);
        assert_eq!(Settings::default().crossfade_seconds(), 0);
    }

    #[test]
    fn writing_a_switch_keeps_the_other_keys() {
        let dir = std::env::temp_dir().join(format!("formalmusic-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{"animatedCoverInBar":false,"unknown":1}"#).unwrap();
        Settings::write(&path, "keepPlayingWhenClosed", true.into()).unwrap();
        let settings = Settings::load(&path);
        assert!(settings.keep_playing_when_closed && !settings.animated_cover_in_bar);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"unknown\": 1"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
