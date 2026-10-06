//! `config.json`: the client's settings. The app only reads it; the user or
//! Home Manager owns it.

use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Play the album's animated cover in the player bar too, not only in
    /// the expanded player.
    pub animated_cover_in_bar: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            animated_cover_in_bar: true,
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
    }
}
