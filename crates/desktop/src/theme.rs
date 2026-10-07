//! Colour tokens, spacing, radii, type scale and sizes, plus the
//! `~/.config/formalmusic/theme.json` override that drives the app from
//! matugen on Linux.
//!
//! The palette lives in a GPUI [`Global`], not a React-style remount: a file
//! change swaps the colours in place and calls `cx.refresh_windows()` so every
//! open window repaints with the new values.

use gpui_kit::{App, FontFeatures, Global, Hsla, Pixels, SharedString, px, rgb};

// ---------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------

/// `FORMALMUSIC_FONT` overrides the body font on any platform; otherwise SF Pro
/// Text on macOS, the bundled static Noto Sans on Linux (`fonts.rs`), and
/// the system Noto Sans on Windows or when the bundled faces failed to load.
pub fn font_sans() -> SharedString {
    if let Ok(font) = std::env::var("FORMALMUSIC_FONT") {
        return font.into();
    }
    if cfg!(target_os = "macos") {
        "SF Pro Text".into()
    } else if crate::fonts::loaded() {
        crate::fonts::FAMILY.into()
    } else {
        "Noto Sans".into()
    }
}

// ---------------------------------------------------------------------------
// Spacing, radii, sizes: fixed, so plain constants rather than global state
// ---------------------------------------------------------------------------

/// One scale, every gap and inset is a step on it. `X1` is 4px. Nothing in
/// the UI should use a spacing number that is not from here.
pub mod spacing {
    use super::{Pixels, px};
    pub const X1: Pixels = px(4.);
    pub const X2: Pixels = px(8.);
    pub const X3: Pixels = px(12.);
    pub const X4: Pixels = px(16.);
    pub const X5: Pixels = px(20.);
    pub const X6: Pixels = px(24.);
    pub const X8: Pixels = px(32.);
    pub const X10: Pixels = px(40.);
}

/// Radii are concentric: an inner radius plus the padding around it equals the
/// outer one. Menu 10 = item 6 + 4 padding, card 12 = art 8 + 4.
pub mod radius {
    use super::{Pixels, px};
    /// Artwork in rows and the player bar.
    pub const ART_SMALL: Pixels = px(4.);
    /// Artwork on cards and page headers.
    pub const ART: Pixels = px(8.);
    pub const ROW: Pixels = px(8.);
    pub const CONTROL: Pixels = px(6.);
    pub const CARD: Pixels = px(12.);
    pub const MENU: Pixels = px(10.);
    pub const MENU_ITEM: Pixels = px(6.);
    pub const PILL: Pixels = px(9999.);
}

#[derive(Clone, Copy)]
pub struct TypeStyle {
    pub font_size: Pixels,
    /// Line height as an absolute pixel value.
    pub line_height: Pixels,
}

pub mod type_scale {
    use super::TypeStyle;
    use gpui_kit::px;

    /// Album, playlist and artist names in a page header.
    pub const DISPLAY: TypeStyle = TypeStyle {
        font_size: px(30.),
        line_height: px(36.),
    };
    /// Shelf titles and the sign-in heading.
    pub const LARGE: TypeStyle = TypeStyle {
        font_size: px(20.),
        line_height: px(26.),
    };
    /// The now playing title in the expanded player and the current lyric.
    pub const TITLE: TypeStyle = TypeStyle {
        font_size: px(14.),
        line_height: px(18.),
    };
    /// Every list row, menu item and button label. macOS control size.
    pub const BODY: TypeStyle = TypeStyle {
        font_size: px(13.),
        line_height: px(18.),
    };
    /// Lyrics: the one place that reads as content, not as chrome.
    pub const LYRIC: TypeStyle = TypeStyle {
        font_size: px(22.),
        line_height: px(30.),
    };
    pub const CAPTION: TypeStyle = TypeStyle {
        font_size: px(12.),
        line_height: px(16.),
    };
    pub const MICRO: TypeStyle = TypeStyle {
        font_size: px(11.),
        line_height: px(14.),
    };
}

/// Tabular figures for any time or count that repaints in place, so the
/// digits keep their column when one of them changes.
pub fn tabular() -> FontFeatures {
    FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]))
}

pub const SIDEBAR_WIDTH: Pixels = px(232.);
/// Collapsed to icons: the icon column plus its padding.
pub const SIDEBAR_COLLAPSED: Pixels = px(64.);
pub const TITLEBAR_HEIGHT: Pixels = px(52.);
pub const PLAYER_HEIGHT: Pixels = px(76.);
/// Gutter each side of a page. Shelves, headers and rows share it.
pub const PAGE_INSET: Pixels = px(32.);
/// A page stops widening here and centres, gutters included, as the web
/// app's content column does on very wide windows.
pub const PAGE_MAX_WIDTH: Pixels = px(1664.);
pub const TRACK_ROW: Pixels = px(56.);
pub const CARD_ART: Pixels = px(168.);

/// macOS traffic lights sit inside this clearance; other platforms draw their
/// own controls flush right, so there is nothing to clear.
pub fn traffic_light_clearance() -> Pixels {
    if cfg!(target_os = "macos") {
        px(78.)
    } else {
        px(0.)
    }
}

// ---------------------------------------------------------------------------
// Palette: the part `theme.json` can override, so it lives in a Global
// ---------------------------------------------------------------------------

/// Apple's dark-appearance system colours, the same base as the sibling
/// Messages app, so both read as one desktop before matugen themes them.
/// `theme.json` sets the base tokens, and `live_theme.rs` recomputes the
/// washes and accent-following tokens from whichever base tokens it set.
#[derive(Clone, Copy)]
pub struct Palette {
    pub canvas: Hsla,
    pub sidebar: Hsla,
    pub sidebar_border: Hsla,
    pub raised: Hsla,
    pub raised_hover: Hsla,
    pub overlay: Hsla,
    pub overlay_border: Hsla,
    pub separator: Hsla,
    pub text: Hsla,
    pub secondary: Hsla,
    pub tertiary: Hsla,
    pub ghost: Hsla,
    pub accent: Hsla,
    pub on_accent: Hsla,
    pub on_accent_soft: Hsla,
    pub selected: Hsla,
    pub selected_soft: Hsla,
    pub danger: Hsla,
    pub danger_soft: Hsla,
    pub warning: Hsla,
    pub online: Hsla,
    pub offline: Hsla,
    pub focus_ring: Hsla,
    /// Washes for hover and press over a dark surface, so one value works on any fill.
    pub hover_wash: Hsla,
    pub press_wash: Hsla,
    /// The played part of a seek bar, and the rest of it.
    pub progress: Hsla,
    pub progress_track: Hsla,
    /// The row and the lyric line that are playing now.
    pub now_playing: Hsla,
    /// A dark wash laid over artwork, and the text on it. Pictures are any
    /// colour, so these stay dark and light whatever the palette.
    pub scrim: Hsla,
    pub on_scrim: Hsla,
    pub transparent: Hsla,
}

impl Default for Palette {
    fn default() -> Self {
        let text = Hsla::from(rgb(0xf2f2f7));
        let accent = Hsla::from(rgb(0xfa2d48));
        let on_accent = Hsla::from(rgb(0xffffff));
        let danger = Hsla::from(rgb(0xff453a));
        Self {
            canvas: Hsla::from(rgb(0x1c1c1e)),
            sidebar: Hsla::from(rgb(0x18181a)),
            sidebar_border: Hsla::from(rgb(0x2c2c2e)),
            raised: Hsla::from(rgb(0x2c2c2e)),
            raised_hover: Hsla::from(rgb(0x3a3a3c)),
            overlay: Hsla::from(rgb(0x2c2c2e)),
            overlay_border: Hsla::from(rgb(0x48484a)),
            separator: Hsla::from(rgb(0x38383a)),
            text,
            secondary: Hsla::from(rgb(0x98989f)),
            tertiary: Hsla::from(rgb(0x6e6e73)),
            ghost: Hsla::from(rgb(0x48484a)),
            accent,
            on_accent,
            on_accent_soft: with_alpha(on_accent, 0xb8),
            selected: accent,
            selected_soft: with_alpha(accent, 0x33),
            danger,
            danger_soft: with_alpha(danger, 0x26),
            warning: Hsla::from(rgb(0xffd60a)),
            online: Hsla::from(rgb(0x30d158)),
            offline: danger,
            focus_ring: accent,
            hover_wash: with_alpha(text, 0x14),
            press_wash: with_alpha(text, 0x26),
            progress: accent,
            progress_track: with_alpha(text, 0x2e),
            now_playing: accent,
            scrim: with_alpha(Hsla::from(rgb(0x000000)), 0x99),
            on_scrim: Hsla::from(rgb(0xffffff)),
            transparent: Hsla::transparent_black(),
        }
    }
}

impl Palette {
    /// Light matugen palettes flip this; shadows, scrims and image outlines
    /// read it because black on black and white on white both vanish.
    pub fn is_dark(&self) -> bool {
        self.canvas.l < 0.5
    }
}

pub(crate) fn with_alpha(color: Hsla, alpha_byte: u8) -> Hsla {
    Hsla {
        a: alpha_byte as f32 / 255.0,
        ..color
    }
}

/// The live theme. A [`Global`] rather than a React-style remount: swap
/// `palette` in place and call `cx.refresh_windows()`.
pub struct Theme {
    pub palette: Palette,
}

impl Global for Theme {}

impl Theme {
    pub fn install(cx: &mut App) {
        cx.set_global(Theme {
            palette: crate::live_theme::initial(),
        });
        sync_component_theme(cx);
    }

    pub fn get(cx: &App) -> Palette {
        cx.global::<Theme>().palette
    }

    /// `live_theme.rs` calls this after applying a `theme.json` change. Swaps
    /// the palette in place and repaints every open window: a `Global` change
    /// alone notifies nobody, so without this a theme change would sit in
    /// memory until something else happened to trigger a render.
    pub(crate) fn set(cx: &mut App, palette: Palette) {
        cx.global_mut::<Theme>().palette = palette;
        sync_component_theme(cx);
        cx.refresh_windows();
    }
}

/// Hands the palette to gpui-component, which draws the text fields, the
/// caption buttons and, on client-decorated Linux, the window frame from its
/// own `Theme`. Its mode follows the canvas, because the frame and the input
/// fill pick their colours by `is_dark()` rather than by token, so a light
/// matugen palette needs a light mode and a dark one a dark mode.
fn sync_component_theme(cx: &mut App) {
    use gpui_kit::component::ThemeMode;

    let palette = Theme::get(cx);
    let theme = gpui_kit::component::Theme::global_mut(cx);
    theme.mode = if palette.is_dark() {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    theme.background = palette.canvas;
    theme.foreground = palette.text;
    theme.border = palette.separator;
    theme.input = palette.separator;
    theme.ring = palette.focus_ring;
    theme.caret = palette.accent;
    theme.selection = palette.accent.opacity(0.35);
    theme.muted = palette.raised;
    theme.muted_foreground = palette.tertiary;
    theme.popover = palette.overlay;
    theme.popover_foreground = palette.text;
    theme.accent = palette.hover_wash;
    theme.accent_foreground = palette.text;
    theme.primary = palette.accent;
    theme.primary_foreground = palette.on_accent;
    theme.list_hover = palette.hover_wash;
    theme.list_active = palette.selected_soft;
    theme.scrollbar_thumb = palette.ghost;
    theme.title_bar = palette.canvas;
    theme.title_bar_border = palette.separator;
    theme.window_border = palette.separator;
    theme.secondary_hover = palette.hover_wash;
    theme.secondary_active = palette.press_wash;
    theme.secondary_foreground = palette.text;
    theme.danger = palette.danger;
    theme.danger_active = palette.danger;
    theme.danger_foreground = palette.on_accent;
    theme.font_family = font_sans();
}
