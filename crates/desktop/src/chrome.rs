//! The sidebar's top row and the conversation header stand in for a title bar,
//! so they move the window, and on Windows and client-decorated Linux the
//! header ends in the caption buttons gpui-kit's `TitleBar` draws.

use gpui_kit::component::{InteractiveElementExt as _, TitleBar};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::theme::TITLEBAR_HEIGHT;

struct DragState {
    pressed: bool,
}

#[cfg(test)]
thread_local! {
    pub static DRAG_HOVERED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// One strip across the top of the window, painted before the panes so it
/// sits beneath them. It lives in the root view because gpui drops the window
/// control areas of a cached view when it reuses that view's frame. Windows
/// reports every hitbox under the cursor to its caption hit test, so anything
/// clickable in a title row has to `occlude()` it, or its clicks turn into
/// window drags.
pub fn drag_layer(id: &'static str, window: &mut Window, cx: &mut App) -> impl IntoElement {
    let state = window.use_keyed_state(ElementId::Name(id.into()), cx, |_, _| DragState {
        pressed: false,
    });
    let linux_menu = cfg!(target_os = "linux")
        && matches!(window.window_decorations(), Decorations::Client { .. });
    div()
        .id(id)
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .window_control_area(WindowControlArea::Drag)
        .on_mouse_down(
            MouseButton::Left,
            window.listener_for(&state, |state, _, _, _| state.pressed = true),
        )
        .on_mouse_up(
            MouseButton::Left,
            window.listener_for(&state, |state, _, _, _| state.pressed = false),
        )
        .on_mouse_down_out(window.listener_for(&state, |state, _, _, _| state.pressed = false))
        .on_mouse_move(window.listener_for(&state, |state, _, window, _| {
            #[cfg(test)]
            DRAG_HOVERED.with(|cell| cell.set(true));
            if state.pressed {
                state.pressed = false;
                window.start_window_move();
            }
        }))
        .on_double_click(|_, window, _| {
            if cfg!(target_os = "macos") {
                window.titlebar_double_click();
            } else {
                window.zoom_window();
            }
        })
        .when(linux_menu, |el| {
            el.on_mouse_down(MouseButton::Right, |event, window, _| {
                window.show_window_menu(event.position)
            })
        })
}

/// Minimize, maximize and close, sized to the title row. Empty on macOS, where
/// the traffic lights are native, and under server-side decorations on Linux.
pub fn caption_buttons() -> impl IntoElement {
    TitleBar::new()
        .h(TITLEBAR_HEIGHT)
        .pl_0()
        .border_0()
        .bg(transparent_black())
}

pub fn has_caption_buttons(window: &Window) -> bool {
    static LOGGED: std::sync::Once = std::sync::Once::new();
    LOGGED.call_once(|| {
        let controls = window.window_controls();
        crate::trace::log_if_enabled(&format!(
            "decorations {:?}, minimize {}, maximize {}, window menu {}",
            window.window_decorations(),
            controls.minimize,
            controls.maximize,
            controls.window_menu
        ));
    });
    if cfg!(target_os = "windows") {
        return true;
    }
    cfg!(target_os = "linux") && matches!(window.window_decorations(), Decorations::Client { .. })
}

/// Width the top-right title row keeps clear for the caption buttons.
pub fn caption_reserve(window: &Window) -> Pixels {
    if !has_caption_buttons(window) {
        return px(0.);
    }
    let controls = window.window_controls();
    let count = 1 + usize::from(controls.minimize) + usize::from(controls.maximize);
    gpui_kit::component::TITLE_BAR_HEIGHT * count as f32
}
