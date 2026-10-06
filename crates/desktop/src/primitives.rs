//! IconButton, Button, Chip, Divider and SectionLabel.

use gpui_kit::component::box_shadow;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::icons::{Icon, IconName};
use crate::theme::{Palette, Theme, radius, spacing, type_scale};

/// Always painted, transparent until the row is the keyboard cursor, so
/// taking the cursor never reflows the row. GPUI gives no separate focus
/// event a list row can drive its own ring from, so callers track the cursor
/// themselves and pass it in here.
pub fn ring(active: bool, color: Hsla, transparent: Hsla) -> (Pixels, Hsla) {
    (px(2.), if active { color } else { transparent })
}

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

/// The hairline around a photo: black on light surfaces, white on dark.
pub fn image_outline(palette: &Palette) -> Hsla {
    if palette.is_dark() {
        hsla(0., 0., 1., 0.1)
    } else {
        hsla(0., 0., 0., 0.1)
    }
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

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
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
    Danger,
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
            ButtonKind::Danger => (palette.danger, palette.on_accent),
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

/// A removable token: a recipient in the To: row, a staged attachment. The
/// remove button keeps the 24 px hit target the banners use, so every close
/// control in the composer area is the same size.
#[derive(IntoElement)]
pub struct Chip {
    id: ElementId,
    label: SharedString,
    leading: Option<AnyElement>,
    remove_label: SharedString,
    on_remove: Option<std::rc::Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl Chip {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        let label = label.into();
        Self {
            id: id.into(),
            remove_label: format!("Remove {label}").into(),
            label,
            leading: None,
            on_remove: None,
        }
    }

    pub fn leading(mut self, leading: impl IntoElement) -> Self {
        self.leading = Some(leading.into_any_element());
        self
    }

    pub fn on_remove(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_remove = Some(std::rc::Rc::new(handler));
        self
    }
}

impl RenderOnce for Chip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let palette = Theme::get(cx);
        let remove_id: SharedString = format!("{}-remove", self.id).into();
        div()
            .id(self.id)
            .flex()
            .flex_row()
            .items_center()
            .gap(spacing::X1)
            .h(px(24.))
            .max_w(px(220.))
            .pl(if self.leading.is_some() {
                px(3.)
            } else {
                spacing::X2
            })
            .rounded(radius::PILL)
            .bg(palette.selected_soft)
            .flex_shrink_0()
            .children(self.leading)
            .child(
                div()
                    .min_w(px(0.))
                    .text_size(type_scale::CAPTION.font_size)
                    .line_height(type_scale::CAPTION.line_height)
                    .text_color(palette.accent)
                    .text_ellipsis()
                    .child(self.label),
            )
            .when_some(self.on_remove, |el, handler| {
                el.child(
                    IconButton::new(remove_id, IconName::Close, self.remove_label)
                        .size(px(11.))
                        .hit(px(24.))
                        .color(palette.accent)
                        .on_click(move |event, window, cx| handler(event, window, cx)),
                )
            })
    }
}

pub fn divider(color: Hsla) -> impl IntoElement {
    div().h(px(1.)).flex_shrink_0().bg(color)
}

/// The small all-caps-weight heading above a group of rows.
pub fn section_label(
    label: impl Into<SharedString>,
    color: Hsla,
    inset: Pixels,
) -> impl IntoElement {
    div()
        .text_size(type_scale::MICRO.font_size)
        .line_height(type_scale::MICRO.line_height)
        .text_color(color)
        .pl(inset)
        .pr(inset)
        .pb(spacing::X1)
        .child(label.into())
}
