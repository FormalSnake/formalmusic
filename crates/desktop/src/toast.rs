//! A non-interactive pill, centred at the bottom, that rises and fades in.

use gpui_kit::*;

use crate::icons::{Icon, IconName};
use crate::theme::{Theme, radius, spacing, type_scale};

/// The toast rises this far as it fades in.
pub const RISE: Pixels = px(8.);
pub const BOTTOM: Pixels = px(72.);
pub const MAX_WIDTH: Pixels = px(480.);

/// The resting pill. `app.rs` animates the bottom padding (rise) and the
/// opacity, and keeps the last message mounted through the exit fade.
pub fn toast(message: &str, cx: &App) -> Div {
    let palette = Theme::get(cx);
    div()
        .absolute()
        .inset_0()
        .flex()
        .flex_row()
        .justify_center()
        .items_end()
        .pb(BOTTOM)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(spacing::X2)
                .px(spacing::X3)
                .py(spacing::X2)
                .rounded(radius::MENU)
                .bg(palette.overlay)
                .border_1()
                .border_color(palette.overlay_border)
                .shadow(crate::primitives::overlay_shadows(&palette))
                .max_w(MAX_WIDTH)
                .child(
                    Icon::new(IconName::Alert)
                        .size(px(14.))
                        .color(palette.danger),
                )
                .child(
                    div()
                        .text_size(type_scale::CAPTION.font_size)
                        .line_height(type_scale::CAPTION.line_height)
                        .text_color(palette.text)
                        .child(message.to_owned()),
                ),
        )
}
