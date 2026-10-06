//! Polls `~/.config/formalmusic/theme.json` (matugen's output on Linux) and
//! applies it over the baked-in palette in `theme.rs`.
//!
//! The poll runs on the background executor and only touches the foreground
//! (a `cx.update` plus `refresh_windows`) when the file's mtime actually
//! moved, so an untouched theme file costs one background `stat` a second and
//! nothing on the window's render loop, matching the idle-CPU budget in
//! `docs/rust-architecture.md`.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use gpui_kit::{App, Hsla, rgb, rgba};
use serde::Deserialize;

use crate::theme::{Palette, Theme, with_alpha};

/// The subset of `Palette` a theme file sets directly; everything else is
/// derived from these.
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PaletteFile {
    canvas: Option<String>,
    sidebar: Option<String>,
    sidebar_border: Option<String>,
    raised: Option<String>,
    raised_hover: Option<String>,
    overlay: Option<String>,
    overlay_border: Option<String>,
    separator: Option<String>,
    text: Option<String>,
    secondary: Option<String>,
    tertiary: Option<String>,
    ghost: Option<String>,
    accent: Option<String>,
    on_accent: Option<String>,
    danger: Option<String>,
    warning: Option<String>,
    online: Option<String>,
    progress: Option<String>,
    progress_track: Option<String>,
    now_playing: Option<String>,
    scrim: Option<String>,
}

/// Applies a `theme.json` file's base tokens over `base`, then recomputes
/// the derived ones. Unknown keys and anything but `#rrggbb`/`#rrggbbaa`
/// are ignored, and a field the file omits keeps `base`'s value, so a
/// missing or unreadable file's caller can pass `Palette::default()` to
/// reset every token.
fn apply(base: Palette, file: &PaletteFile) -> Palette {
    let mut palette = base;
    macro_rules! set {
        ($field:ident) => {
            if let Some(color) = file.$field.as_deref().and_then(parse_hex) {
                palette.$field = color;
            }
        };
    }
    set!(canvas);
    set!(sidebar);
    set!(sidebar_border);
    set!(raised);
    set!(raised_hover);
    set!(overlay);
    set!(overlay_border);
    set!(separator);
    set!(text);
    set!(secondary);
    set!(tertiary);
    set!(ghost);
    set!(accent);
    set!(on_accent);
    set!(danger);
    set!(warning);
    set!(online);

    palette.selected = palette.accent;
    palette.selected_soft = with_alpha(palette.accent, 0x33);
    palette.focus_ring = palette.accent;
    palette.progress = palette.accent;
    palette.progress_track = with_alpha(palette.text, 0x2e);
    palette.now_playing = palette.accent;
    set!(progress);
    set!(progress_track);
    set!(now_playing);
    set!(scrim);
    palette.danger_soft = with_alpha(palette.danger, 0x26);
    palette.on_accent_soft = with_alpha(palette.on_accent, 0xb8);
    palette.hover_wash = with_alpha(palette.text, 0x14);
    palette.press_wash = with_alpha(palette.text, 0x26);
    palette.offline = palette.danger;
    palette
}

fn parse_hex(s: &str) -> Option<Hsla> {
    let hex = s.strip_prefix('#')?;
    match hex.len() {
        6 => u32::from_str_radix(hex, 16)
            .ok()
            .map(|v| Hsla::from(rgb(v))),
        8 => u32::from_str_radix(hex, 16)
            .ok()
            .map(|v| Hsla::from(rgba(v))),
        _ => None,
    }
}

fn theme_path() -> PathBuf {
    formalmusic_core::paths::theme_file()
}

/// The palette for the first frame, read on the spot: a small file, and
/// painting the defaults first would flash the wrong colours at every launch.
pub fn initial() -> Palette {
    let file: PaletteFile = std::fs::read_to_string(theme_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    apply(Palette::default(), &file)
}

/// Starts the 1 s poll. Call once, after `Theme::install`.
pub fn watch(cx: &mut App) {
    cx.spawn(async move |cx| {
        let path = theme_path();
        // `initial` already applied the file as it is now.
        let mut last_mtime: Option<Option<SystemTime>> =
            Some(std::fs::metadata(&path).and_then(|m| m.modified()).ok());
        loop {
            let poll_path = path.clone();
            let mtime = cx
                .background_executor()
                .spawn(async move {
                    std::fs::metadata(&poll_path)
                        .and_then(|m| m.modified())
                        .ok()
                })
                .await;

            if last_mtime != Some(mtime) {
                last_mtime = Some(mtime);
                match mtime {
                    // Missing or unreadable resets to defaults.
                    None => {
                        let _ = cx.update(|cx| Theme::set(cx, Palette::default()));
                    }
                    Some(_) => {
                        let read_path = path.clone();
                        let contents = cx
                            .background_executor()
                            .spawn(async move { std::fs::read_to_string(&read_path).ok() })
                            .await;
                        let file: PaletteFile = contents
                            .as_deref()
                            .and_then(|text| serde_json::from_str(text).ok())
                            .unwrap_or_default();
                        let _ = cx.update(|cx| Theme::set(cx, apply(Palette::default(), &file)));
                    }
                }
            }

            cx.background_executor().timer(Duration::from_secs(1)).await;
        }
    })
    .detach();
}
