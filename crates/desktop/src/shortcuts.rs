//! music.youtube.com's single-key shortcuts, as its own `?` overlay lists
//! them, and that overlay. `n` and `p` stay as next and previous too, and
//! `Shift+Up` and `Shift+Down` as volume, since nothing on the web uses them.

use formalmusic_core::store::SEEK_STEP_MS;
use gpui_kit::*;

use crate::primitives::overlay_shadows;
use crate::theme::{Palette, radius, spacing, type_scale};

/// A `g` waits this long for the key that says where to go.
pub const CHORD_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shortcut {
    Toggle,
    Next,
    Previous,
    /// Milliseconds, negative to go back.
    Seek(i64),
    /// Steps of the store's volume step.
    Volume(f32),
    Mute,
    Shuffle,
    Repeat,
    Like,
    Dislike,
    Search,
    Queue,
    Expand,
    Help,
    /// A `g`: the next key picks a page.
    Chord,
    Home,
    Explore,
    Library,
    Settings,
}

/// What a key does. `key` is the key's name (`space`, `left`), `typed`
/// the character it produced, which already carries Shift (`N`, `+`, `?`).
/// After a `g`, only the chord's second keys mean anything.
pub fn resolve(after_g: bool, key: &str, typed: &str, shift: bool) -> Option<Shortcut> {
    use Shortcut::*;
    if after_g {
        return match typed {
            "h" => Some(Home),
            "e" => Some(Explore),
            "l" => Some(Library),
            "," => Some(Settings),
            _ => None,
        };
    }
    Some(match (key, typed, shift) {
        ("space", _, _) | (_, ";", _) => Toggle,
        (_, "j" | "n" | "N", _) => Next,
        (_, "k" | "p" | "P", _) => Previous,
        ("right", _, true) | (_, "l", _) => Seek(SEEK_STEP_MS as i64),
        ("left", _, true) | (_, "h", _) => Seek(-(SEEK_STEP_MS as i64)),
        (_, "L", _) => Seek(1_000),
        (_, "H", _) => Seek(-1_000),
        (_, "=", _) | ("up", _, true) => Volume(1.),
        (_, "-", _) | ("down", _, true) => Volume(-1.),
        (_, "m", _) => Mute,
        (_, "s", _) => Shuffle,
        (_, "r", _) => Repeat,
        (_, "+", _) => Like,
        (_, "_", _) => Dislike,
        (_, "/", _) => Search,
        (_, "q", _) => Queue,
        (_, "f", _) => Expand,
        (_, "?", _) => Help,
        (_, "g", _) => Chord,
        _ => return None,
    })
}

const PLAYBACK: [(&str, &str); 12] = [
    ("Space  ;", "Play or pause"),
    ("j  Shift+N", "Next song"),
    ("k  Shift+P", "Previous song"),
    ("l  Shift+\u{2192}", "Forward 10 seconds"),
    ("h  Shift+\u{2190}", "Back 10 seconds"),
    ("Shift+L  Shift+H", "Forward or back 1 second"),
    ("s", "Shuffle"),
    ("r", "Repeat"),
    ("=  -", "Volume up or down"),
    ("m", "Mute"),
    ("+", "Like"),
    ("_", "Dislike"),
];

const NAVIGATION: [(&str, &str); 9] = [
    ("g h", "Home"),
    ("g e", "Explore"),
    ("g l", "Library"),
    ("g ,", "Settings"),
    ("/", "Search"),
    ("q", "Queue"),
    ("f", "Expanded player"),
    ("Alt+\u{2190}  Alt+\u{2192}", "Back or forward"),
    ("?", "Keyboard shortcuts"),
];

/// The `?` overlay: every shortcut, in the web app's two groups.
pub fn overlay(palette: Palette, on_close: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    let group =
        |title: &'static str, rows: &[(&'static str, &'static str)]| {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.))
                .gap(spacing::X2)
                .child(
                    div()
                        .pb(spacing::X1)
                        .text_size(type_scale::TITLE.font_size)
                        .line_height(type_scale::TITLE.line_height)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(palette.text)
                        .child(title),
                )
                .children(rows.iter().map(|(keys, what)| {
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(spacing::X3)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .text_size(type_scale::BODY.font_size)
                                .line_height(type_scale::BODY.line_height)
                                .text_color(palette.secondary)
                                .child(*what),
                        )
                        .child(div().flex().flex_row().gap(spacing::X1).children(
                            keys.split("  ").map(|key| {
                                div()
                                    .px(spacing::X2)
                                    .rounded(radius::CONTROL)
                                    .bg(palette.press_wash)
                                    .text_size(type_scale::CAPTION.font_size)
                                    .line_height(px(22.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(palette.text)
                                    .child(key)
                            }),
                        ))
                }))
        };
    div()
        .id("shortcuts-layer")
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(palette.scrim)
        .occlude()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| on_close(window, cx))
        .child(
            div()
                .id("shortcuts-dialog")
                .w(px(680.))
                .max_w(relative(0.9))
                .max_h(relative(0.85))
                .overflow_y_scroll()
                .p(spacing::X6)
                .flex()
                .flex_col()
                .gap(spacing::X5)
                .rounded(radius::CARD)
                .bg(palette.overlay)
                .border_1()
                .border_color(palette.overlay_border)
                .shadow(overlay_shadows(&palette))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .text_size(type_scale::LARGE.font_size)
                        .line_height(type_scale::LARGE.line_height)
                        .font_weight(FontWeight::BOLD)
                        .text_color(palette.text)
                        .child("Keyboard shortcuts"),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(spacing::X8)
                        .child(group("Playback", &PLAYBACK))
                        .child(group("Navigation", &NAVIGATION)),
                )
                .into_any_element(),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::Shortcut::*;
    use super::*;

    #[::core::prelude::v1::test]
    fn the_web_apps_keys() {
        let plain = |typed| resolve(false, typed, typed, false);
        let shifted = |typed| resolve(false, typed, typed, true);
        assert_eq!(resolve(false, "space", " ", false), Some(Toggle));
        assert_eq!(plain(";"), Some(Toggle));
        assert_eq!(plain("j"), Some(Next));
        assert_eq!(plain("k"), Some(Previous));
        assert_eq!(shifted("N"), Some(Next));
        assert_eq!(shifted("P"), Some(Previous));
        assert_eq!(plain("l"), Some(Seek(10_000)));
        assert_eq!(shifted("H"), Some(Seek(-1_000)));
        assert_eq!(resolve(false, "right", "", true), Some(Seek(10_000)));
        assert_eq!(resolve(false, "right", "", false), None);
        assert_eq!(shifted("+"), Some(Like));
        assert_eq!(shifted("_"), Some(Dislike));
        assert_eq!(plain("="), Some(Volume(1.)));
        assert_eq!(shifted("?"), Some(Help));
        assert_eq!(plain("x"), None);
    }

    #[::core::prelude::v1::test]
    fn g_chords_go_places_and_nothing_else() {
        assert_eq!(resolve(false, "g", "g", false), Some(Chord));
        assert_eq!(resolve(true, "h", "h", false), Some(Home));
        assert_eq!(resolve(true, "e", "e", false), Some(Explore));
        assert_eq!(resolve(true, "l", "l", false), Some(Library));
        assert_eq!(resolve(true, ",", ",", false), Some(Settings));
        assert_eq!(resolve(true, "j", "j", false), None);
    }
}
