//! IconButton, Button, Chip, Divider and SectionLabel.

use gpui_kit::component::box_shadow;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::icons::{Icon, IconName};
use crate::theme::{Palette, Theme, radius, spacing, type_scale};

/// A wide soft shadow plus a tight contact one, so a floating surface reads
/// as lifted rather than smudged. Light palettes need far less of both.
pub fn overlay_shadows(palette: &Palette) -> Vec<BoxShadow> {
    let (wide, tight) = if palette.is_dark() {
        (0.5, 0.3)
    } else {
        (0.16, 0.08)
    };
    vec![
        box_shadow(px(0.), px(12.), px(32.), px(0.), hsla(0., 0., 0., wide)),
        box_shadow(px(0.), px(1.), px(3.), px(0.), hsla(0., 0., 0., tight)),
    ]
}

#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: IconName,
    label: SharedString,
    size: Pixels,
    hit: Pixels,
    color: Option<Hsla>,
    active: bool,
    disabled: bool,
    strong: bool,
    filled: bool,
    on_click: Option<std::rc::Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

/// IconButton tooltips wait a beat longer than the app-wide
/// 500 ms default.
pub const TOOLTIP_DELAY: std::time::Duration = std::time::Duration::from_millis(600);

impl IconButton {
    pub fn new(id: impl Into<ElementId>, icon: IconName, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            icon,
            label: label.into(),
            size: px(16.),
            hit: px(28.),
            color: None,
            active: false,
            disabled: false,
            strong: false,
            filled: false,
            on_click: None,
        }
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }

    pub fn hit(mut self, hit: Pixels) -> Self {
        self.hit = hit;
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn strong(mut self, strong: bool) -> Self {
        self.strong = strong;
        self
    }

    /// The filled glyph, for an on state that colour alone would not carry.
    pub fn filled(mut self, filled: bool) -> Self {
        self.filled = filled;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(std::rc::Rc::new(handler));
        self
    }
}

impl RenderOnce for IconButton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = Theme::get(cx);
        let fg = if self.active {
            palette.accent
        } else {
            self.color.unwrap_or(palette.secondary)
        };
        let disabled = self.disabled;
        let label = self.label.clone();
        let selector = self.id.to_string();

        div()
            .id(self.id)
            .debug_selector(|| selector)
            .w(self.hit)
            .h(self.hit)
            .occlude()
            .rounded(radius::CONTROL)
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .when(self.disabled, |el| el.opacity(0.4))
            .when(self.active, |el| el.bg(palette.selected_soft))
            .when(!disabled, |el| {
                el.hover(move |style| {
                    style.bg(if self.active {
                        palette.selected_soft
                    } else {
                        palette.hover_wash
                    })
                })
                .active(move |style| {
                    style.bg(if self.active {
                        palette.selected_soft
                    } else {
                        palette.press_wash
                    })
                })
                .when_some(self.on_click, |el, handler| {
                    let on_key = handler.clone();
                    el.tab_index(0)
                        .border_2()
                        .border_color(palette.transparent)
                        .focus_visible(move |style| style.border_color(palette.focus_ring))
                        .on_click(move |event, window, cx| handler(event, window, cx))
                        .on_key_down(move |event: &KeyDownEvent, window, cx| {
                            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                on_key(&ClickEvent::default(), window, cx);
                            }
                        })
                })
            })
            .tooltip(move |window, cx| Tooltip::new(label.clone()).build(window, cx))
            .tooltip_show_delay(TOOLTIP_DELAY)
            .child(
                Icon::new(self.icon)
                    .size(self.size)
                    .color(fg)
                    .strong(self.strong)
                    .filled(self.filled),
            )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Secondary,
}

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    kind: ButtonKind,
    disabled: bool,
    on_click: Option<std::rc::Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind: ButtonKind::Secondary,
            disabled: false,
            on_click: None,
        }
    }

    pub fn kind(mut self, kind: ButtonKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(std::rc::Rc::new(handler));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = Theme::get(cx);
        let (fill, fg) = match self.kind {
            ButtonKind::Primary => (palette.accent, palette.on_accent),
            ButtonKind::Secondary => (palette.press_wash, palette.text),
        };
        let disabled = self.disabled;
        let kind = self.kind;
        let selector = self.id.to_string();

        div()
            .id(self.id)
            .debug_selector(|| selector)
            .h(px(30.))
            .px(spacing::X3)
            .rounded(radius::CONTROL)
            .bg(fill)
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .when(disabled, |el| el.opacity(0.4))
            .when(!disabled, |el| {
                el.hover(|style| style.opacity(0.88))
                    .active(|style| style.opacity(0.7))
                    .when_some(self.on_click, |el, handler| {
                        let on_key = handler.clone();
                        el.tab_index(0)
                            .border_2()
                            .border_color(palette.transparent)
                            .focus_visible(move |style| {
                                style.border_color(if kind == ButtonKind::Secondary {
                                    palette.focus_ring
                                } else {
                                    palette.text
                                })
                            })
                            .on_click(move |event, window, cx| handler(event, window, cx))
                            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                    on_key(&ClickEvent::default(), window, cx);
                                }
                            })
                    })
            })
            .child(
                div()
                    .text_color(fg)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(type_scale::BODY.font_size)
                    .line_height(type_scale::BODY.line_height)
                    .child(self.label),
            )
    }
}
