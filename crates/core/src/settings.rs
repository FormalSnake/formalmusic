//! `config.json`: the client's settings. The user or Home Manager owns it;
//! the Settings dialog writes back only the switches it shows.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Play the album's animated cover in the player bar too, not only in
    /// the expanded player.
    pub animated_cover_in_bar: bool,
    /// Leave the daemon playing when the last window closes, instead of
    /// pausing it.
    pub keep_playing_when_closed: bool,
    /// Show the daemon's tray icon while a track is loaded (Linux).
    pub show_in_tray: bool,
    /// The keys below are the daemon's: it reads them on start and on
    /// `ReloadSettings`, over its own `daemon.json`.
    pub audio_quality: AudioQuality,
    /// Radio from the last track once a list runs out.
    pub autoplay: bool,
    pub restrict_explicit: bool,
    /// Stop reporting plays to YouTube's watch history.
    pub pause_history: bool,
}

/// The web app's audio quality setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioQuality {
    #[default]
    Auto,
    Low,
    Normal,
    High,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            animated_cover_in_bar: true,
            keep_playing_when_closed: false,
            show_in_tray: true,
            audio_quality: AudioQuality::Auto,
            autoplay: true,
            restrict_explicit: false,
            pause_history: false,
        }
    }
}

impl Settings {
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
        assert!(settings.animated_cover_in_bar);
        let settings: Settings = serde_json::from_str(r#"{"animatedCoverInBar":false}"#).unwrap();
        assert!(!settings.animated_cover_in_bar);
        assert!(!settings.keep_playing_when_closed);
        assert!(settings.show_in_tray && settings.autoplay);
        let settings: Settings = serde_json::from_str(r#"{"audioQuality":"low"}"#).unwrap();
        assert_eq!(settings.audio_quality, AudioQuality::Low);
        assert_eq!(
            serde_json::to_value(AudioQuality::Normal).unwrap(),
            "normal"
        );
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
