//! `config.toml`: the settings the app's Settings window writes and the daemon plays by. Both
//! binaries load it through nd-settings, so Home Manager (`programs.formalmusic.settings`, typed
//! options generated from this schema), `config.d/` drop-ins and `FORMALMUSIC_*` variables
//! layer over it the same way for each.
//!
//! The JSON files it replaces (`config.json`, written by the old window, and `daemon.json`) are
//! read once by [`migrate_legacy`] when no `config.toml` exists yet.

use std::path::Path;

use nd_settings::{Options, SettingsHandle};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Equalizer;

/// The config directory name; nd-settings puts it under `~/.config` on Linux and under
/// `~/Library/Application Support` on macOS.
pub const APP: &str = "formalmusic";

#[derive(nd_settings::Settings, Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    #[settings(section)]
    pub playback: Playback,
    #[settings(section)]
    pub privacy: Privacy,
    #[settings(section)]
    pub window: Window,
}

/// Playback.
#[derive(nd_settings::Settings, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Playback {
    /// Audio quality. Auto and High take the best format the account gets, Premium's Opus
    /// included; Normal stays near 130 kbps and Low near 50.
    pub quality: Quality,
    /// Play similar songs when the queue ends.
    pub autoplay: bool,
    /// Skip songs YouTube marks explicit.
    pub restrict_explicit: bool,
    /// Even out loudness with YouTube's own per-track values.
    pub normalisation: bool,
    /// Crossfade between tracks, in milliseconds. 0 plays them gapless.
    #[settings(range = 0.0..=12000.0, step = 500.0)]
    pub crossfade_ms: u32,
    pub equalizer: Equalizer,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            quality: Quality::Auto,
            autoplay: true,
            restrict_explicit: false,
            normalisation: true,
            crossfade_ms: 0,
            equalizer: Equalizer::default(),
        }
    }
}

/// Privacy.
#[derive(nd_settings::Settings, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Privacy {
    /// Report plays to YouTube, so History and recommendations follow what you play.
    pub report_history: bool,
}

impl Default for Privacy {
    fn default() -> Self {
        Self {
            report_history: true,
        }
    }
}

/// Window.
#[derive(nd_settings::Settings, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Window {
    /// Keep playing after the window closes, instead of pausing.
    pub keep_playing_when_closed: bool,
    /// Play the album's motion artwork in the player bar too, not only in the expanded player.
    pub animated_cover_in_bar: bool,
    /// Show a desktop notification when the song changes while the window is in the background.
    pub notify_song_change: bool,
    /// Show FormalMusic in the system tray (the menu bar on macOS), running on with the window closed.
    pub show_in_tray: bool,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            keep_playing_when_closed: false,
            animated_cover_in_bar: true,
            notify_song_change: false,
            show_in_tray: true,
        }
    }
}

/// The web app's audio quality setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    #[default]
    Auto,
    Low,
    Normal,
    High,
}

impl Quality {
    pub const ALL: [Quality; 4] = [Quality::Auto, Quality::Low, Quality::Normal, Quality::High];

    pub fn label(self) -> &'static str {
        match self {
            Quality::Auto => "Auto",
            Quality::Low => "Low",
            Quality::Normal => "Normal",
            Quality::High => "High",
        }
    }
}

/// Loads the layered settings, after importing the old JSON files the first time.
pub fn load() -> Result<SettingsHandle<Settings>, nd_settings::LoadError> {
    let dir = nd_settings::config_dir(APP);
    if let Err(e) = migrate_legacy(&dir, &legacy_dir()) {
        eprintln!("formalmusic: settings from config.json not imported: {e}");
    }
    SettingsHandle::load(Options::new(APP))
}

/// Where the JSON files lived: `$XDG_CONFIG_HOME/formalmusic` (or `~/.config/formalmusic`) on
/// Linux and macOS, `%APPDATA%\formalmusic` on Windows.
pub fn legacy_dir() -> std::path::PathBuf {
    if cfg!(windows) {
        return dirs::config_dir().unwrap_or_default().join(APP);
    }
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .unwrap_or_default()
        .join(APP)
}

/// Writes `dir/config.toml` from the old `config.json` (the window's keys) over `daemon.json`
/// (the daemon's), once: nothing happens when `config.toml` exists or neither old file does.
/// The old files stay where they are.
pub fn migrate_legacy(dir: &Path, legacy: &Path) -> std::io::Result<bool> {
    let target = dir.join("config.toml");
    if target.exists() {
        return Ok(false);
    }
    let read = |name: &str| -> Option<serde_json::Map<String, serde_json::Value>> {
        let text = std::fs::read_to_string(legacy.join(name)).ok()?;
        serde_json::from_str(&text).ok()
    };
    let (app, daemon) = (read("config.json"), read("daemon.json"));
    if app.is_none() && daemon.is_none() {
        return Ok(false);
    }
    let (app, daemon) = (app.unwrap_or_default(), daemon.unwrap_or_default());
    // The window's file won over the daemon's for the keys both carried.
    let key = |name: &str| app.get(name).or_else(|| daemon.get(name));
    let bool_of = |name: &str| key(name).and_then(serde_json::Value::as_bool);
    let mut s = Settings::default();
    if let Some(q) = app
        .get("audioQuality")
        .or_else(|| daemon.get("preferredQuality"))
    {
        s.playback.quality = serde_json::from_value(q.clone()).unwrap_or_default();
    }
    if let Some(v) = bool_of("autoplay") {
        s.playback.autoplay = v;
    }
    if let Some(v) = bool_of("restrictExplicit") {
        s.playback.restrict_explicit = v;
    }
    if let Some(v) = daemon
        .get("normalisation")
        .and_then(serde_json::Value::as_bool)
    {
        s.playback.normalisation = v;
    }
    if let Some(v) = daemon
        .get("crossfadeMs")
        .and_then(serde_json::Value::as_u64)
    {
        s.playback.crossfade_ms = v.min(u64::from(u32::MAX)) as u32;
    }
    if let Some(eq) = key("equalizer") {
        s.playback.equalizer = serde_json::from_value(eq.clone()).unwrap_or_default();
    }
    match (
        app.get("pauseHistory").and_then(serde_json::Value::as_bool),
        daemon.get("reportHistory"),
    ) {
        (Some(paused), _) => s.privacy.report_history = !paused,
        (None, Some(v)) => s.privacy.report_history = v.as_bool().unwrap_or(true),
        _ => {}
    }
    if let Some(v) = app
        .get("keepPlayingWhenClosed")
        .and_then(serde_json::Value::as_bool)
    {
        s.window.keep_playing_when_closed = v;
    }
    if let Some(v) = app.get("showInTray").and_then(serde_json::Value::as_bool) {
        s.window.show_in_tray = v;
    }
    if let Some(v) = app
        .get("animatedCoverInBar")
        .and_then(serde_json::Value::as_bool)
    {
        s.window.animated_cover_in_bar = v;
    }
    let text = toml::to_string(&s).map_err(std::io::Error::other)?;
    std::fs::create_dir_all(dir)?;
    let tmp = target.with_extension("toml.tmp");
    std::fs::write(
        &tmp,
        format!("# Imported from config.json and daemon.json.\n{text}"),
    )?;
    std::fs::rename(&tmp, &target)?;
    Ok(true)
}

/// The JSON Schema `nd-hm-gen` turns into `nix/settings-options.nix`.
pub fn schema_json() -> serde_json::Value {
    nd_settings::schema_json::<Settings>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_json_files_become_config_toml() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("old");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(
            legacy.join("config.json"),
            r#"{"audioQuality":"low","pauseHistory":true,"keepPlayingWhenClosed":true,"showInTray":false,
                "equalizer":{"preset":"rock","custom":[1,2,3,4,5,6,7,8,9,10]},"unknown":1}"#,
        )
        .unwrap();
        std::fs::write(
            legacy.join("daemon.json"),
            r#"{"crossfadeMs":3000,"autoplay":false,"normalisation":false}"#,
        )
        .unwrap();
        let new = dir.path().join("new");
        assert!(migrate_legacy(&new, &legacy).unwrap());
        let text = std::fs::read_to_string(new.join("config.toml")).unwrap();
        let s: Settings = toml::from_str(&text).unwrap();
        assert_eq!(s.playback.quality, Quality::Low);
        assert!(!s.playback.autoplay && !s.playback.normalisation && !s.privacy.report_history);
        assert_eq!(s.playback.crossfade_ms, 3000);
        assert_eq!(s.playback.equalizer.preset, crate::EqPreset::Rock);
        assert_eq!(s.playback.equalizer.custom[9], 10.0);
        assert!(s.window.keep_playing_when_closed);
        assert!(!s.window.show_in_tray);
        // Once written, the old files are not read again.
        assert!(!migrate_legacy(&new, &legacy).unwrap());
    }

    #[test]
    fn nothing_to_import_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!migrate_legacy(dir.path(), &dir.path().join("missing")).unwrap());
        assert!(!dir.path().join("config.toml").exists());
    }
}

#[cfg(test)]
mod nix {
    /// `nix/settings-options.nix` is generated from this schema; `FORMALMUSIC_REGEN=1` rewrites it.
    #[test]
    fn home_manager_options_are_current() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../nix/settings-options.nix"
        );
        let module = nd_settings::nix::options_module(&super::schema_json());
        if std::env::var("FORMALMUSIC_REGEN").as_deref() == Ok("1") {
            std::fs::write(path, &module).unwrap();
        }
        let current = std::fs::read_to_string(path).unwrap_or_default();
        assert!(
            current == module,
            "{path} is out of date; run with FORMALMUSIC_REGEN=1"
        );
    }

    /// `fspec.star`, the fspec module, comes from the same schema; `FORMALMUSIC_REGEN=1` rewrites it.
    #[test]
    fn fspec_module_is_current() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fspec.star");
        // symphonia-adapter-libopus links the system libopus.
        let app = nd_settings::fspec::App::new("formalmusic", "https://github.com/FormalStudio/formalmusic")
            .display_name("FormalMusic")
            .build_deps(&["libopus-devel"]);
        let module = nd_settings::fspec::star_module(&app, &super::schema_json());
        if std::env::var("FORMALMUSIC_REGEN").as_deref() == Ok("1") {
            std::fs::write(path, &module).unwrap();
        }
        let current = std::fs::read_to_string(path).unwrap_or_default();
        assert!(
            current == module,
            "{path} is out of date; run with FORMALMUSIC_REGEN=1"
        );
    }
}
