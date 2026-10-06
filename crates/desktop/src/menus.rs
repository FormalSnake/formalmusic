//! The `MenuItem` data model screens build for their rows, and the
//! `ContextMenu` entity that renders it. Menus open through `AppRoot`.

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::icons::{Icon, IconName};
use crate::theme::{Palette, Theme, radius, spacing, type_scale};

pub enum MenuItem {
    Item {
        label: SharedString,
        icon: Option<IconName>,
        /// `Rc`, not `Box`: `ContextMenu` keeps the request in its own state
        /// and re-renders it on every notify, so the handler has to be
        /// cloneable into each render's click/keydown closures.
        on_select: std::rc::Rc<dyn Fn(&mut Window, &mut App)>,
        danger: bool,
        disabled: bool,
        /// Right-aligned hint, already formatted for the platform by `shortcut()`.
        shortcut: Option<SharedString>,
    },
    Separator,
    Header(SharedString),
    /// A line of information the pointer does nothing with.
    Note(SharedString),
}

impl MenuItem {
    pub fn item(
        label: impl Into<SharedString>,
        on_select: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        MenuItem::Item {
            label: label.into(),
            icon: None,
            on_select: std::rc::Rc::new(on_select),
            danger: false,
            disabled: false,
            shortcut: None,
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        if let MenuItem::Item { icon: slot, .. } = &mut self {
            *slot = Some(icon);
        }
        self
    }

    pub fn danger(mut self) -> Self {
        if let MenuItem::Item { danger, .. } = &mut self {
            *danger = true;
        }
        self
    }

    fn is_selectable(&self) -> bool {
        matches!(self, MenuItem::Item { .. })
    }
}

pub struct MenuRequest {
    pub position: Point<Pixels>,
    pub items: Vec<MenuItem>,
    /// Rests the menu's bottom edge on the point instead of hanging it
    /// below, for a menu opened from the player bar.
    pub above: bool,
}

impl MenuRequest {
    pub fn at(position: Point<Pixels>, items: Vec<MenuItem>) -> Self {
        Self {
            position,
            items,
            above: false,
        }
    }

    pub fn above(mut self) -> Self {
        self.above = true;
        self
    }

    fn anchor(&self) -> Anchor {
        if self.above {
            Anchor::BottomLeft
        } else {
            Anchor::TopLeft
        }
    }
}

/// The rendered menu: focused on open, Escape closes, Down/Up wrap through
/// the selectable items, Home/End jump to the ends, Enter/Space activates.
/// No typeahead is implemented.
pub struct ContextMenu {
    request: MenuRequest,
    highlighted: Option<usize>,
    focus_handle: FocusHandle,
    on_close: std::rc::Rc<dyn Fn(&mut Window, &mut App)>,
}

impl ContextMenu {
    pub fn open(
        request: MenuRequest,
        on_close: impl Fn(&mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| Self::new(request, on_close, window, cx))
    }

    fn new(
        request: MenuRequest,
        on_close: impl Fn(&mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);
        Self {
            request,
            highlighted: None,
            focus_handle,
            on_close: std::rc::Rc::new(on_close),
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.request.items.len()
    }

    fn selectable(&self) -> Vec<usize> {
        self.request
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.is_selectable())
            .map(|(index, _)| index)
            .collect()
    }

    fn step(&mut self, delta: i32) {
        let selectable = self.selectable();
        if selectable.is_empty() {
            return;
        }
        let len = selectable.len() as i32;
        let current = match self
            .highlighted
            .and_then(|h| selectable.iter().position(|&index| index == h))
        {
            Some(position) => position as i32,
            None => {
                if delta > 0 {
                    -1
                } else {
                    0
                }
            }
        };
        let next = (current + delta).rem_euclid(len);
        self.highlighted = Some(selectable[next as usize]);
    }

    fn jump_start(&mut self) {
        self.highlighted = self.selectable().first().copied();
    }

    fn jump_end(&mut self) {
        self.highlighted = self.selectable().last().copied();
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(MenuItem::Item {
            on_select,
            disabled,
            ..
        }) = self.request.items.get(index)
        else {
            return;
        };
        if *disabled {
            return;
        }
        let on_select = on_select.clone();
        let on_close = self.on_close.clone();
        on_select(window, cx);
        on_close(window, cx);
    }

    fn activate_highlighted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.highlighted {
            self.activate(index, window, cx);
        }
    }
}

impl Render for ContextMenu {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::get(cx);
        let highlighted = self.highlighted;
        let on_close = self.on_close.clone();
        let close_for_outside = on_close.clone();
        let close_for_escape = on_close.clone();

        anchored()
            .position(self.request.position)
            .anchor(self.request.anchor())
            .snap_to_window_with_margin(spacing::X2)
            .child(deferred(
                div()
                    .id("context-menu")
                    .track_focus(&self.focus_handle)
                    .flex()
                    .flex_col()
                    .min_w(px(196.))
                    .p(spacing::X1)
                    .rounded(radius::MENU)
                    .bg(palette.overlay)
                    .border_1()
                    .border_color(palette.overlay_border)
                    .shadow(crate::primitives::overlay_shadows(&palette))
                    .on_mouse_down_out(move |_, window, cx| close_for_outside(window, cx))
                    .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                        match event.keystroke.key.as_str() {
                            "escape" => close_for_escape(window, cx),
                            "down" => {
                                this.step(1);
                                cx.notify();
                            }
                            "up" => {
                                this.step(-1);
                                cx.notify();
                            }
                            "home" => {
                                this.jump_start();
                                cx.notify();
                            }
                            "end" => {
                                this.jump_end();
                                cx.notify();
                            }
                            "enter" | "space" => this.activate_highlighted(window, cx),
                            _ => {}
                        }
                    }))
                    .children((0..self.request.items.len()).map(|index| {
                        render_item(index, &self.request.items[index], palette, highlighted, cx)
                    })),
            ))
    }
}

fn render_item(
    index: usize,
    item: &MenuItem,
    palette: Palette,
    highlighted: Option<usize>,
    cx: &Context<ContextMenu>,
) -> AnyElement {
    match item {
        MenuItem::Separator => div()
            .h(px(1.))
            .bg(palette.separator)
            .mt(spacing::X1)
            .mb(spacing::X1)
            .mx(spacing::X2)
            .into_any_element(),
        MenuItem::Header(label) => div()
            .text_size(type_scale::MICRO.font_size)
            .line_height(type_scale::MICRO.line_height)
            .text_color(palette.tertiary)
            .px(spacing::X2)
            .py(spacing::X1)
            .child(label.clone())
            .into_any_element(),
        MenuItem::Note(label) => div()
            .text_size(type_scale::BODY.font_size)
            .line_height(px(28.))
            .text_color(palette.text)
            .px(spacing::X2)
            .h(px(28.))
            .child(label.clone())
            .into_any_element(),
        MenuItem::Item {
            label,
            icon,
            danger,
            disabled,
            shortcut,
            ..
        } => {
            let (label, icon, danger, disabled, shortcut) =
                (label.clone(), *icon, *danger, *disabled, shortcut.clone());
            let active = highlighted == Some(index) && !disabled;
            let fg = if disabled {
                palette.secondary
            } else if danger && !active {
                palette.danger
            } else if active {
                palette.on_accent
            } else {
                palette.text
            };
            let id = ElementId::Name(format!("menu-{label}").into());
            div()
                .id(id)
                .flex()
                .flex_row()
                .items_center()
                .gap(spacing::X2)
                .h(px(28.))
                .px(spacing::X2)
                .rounded(radius::MENU_ITEM)
                .when(active, |el| el.bg(palette.accent))
                .when(disabled, |el| el.opacity(0.4))
                .when(!disabled, |el| {
                    el.on_click(
                        cx.listener(move |this, _, window, cx| this.activate(index, window, cx)),
                    )
                    .on_hover(cx.listener(
                        move |this, hovered, _window, cx| {
                            if *hovered {
                                this.highlighted = Some(index);
                            } else if this.highlighted == Some(index) {
                                this.highlighted = None;
                            }
                            cx.notify();
                        },
                    ))
                })
                .child(
                    div()
                        .w(px(14.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_shrink_0()
                        .when_some(icon, |el, icon| {
                            el.child(Icon::new(icon).size(px(14.)).color(fg))
                        }),
                )
                .child(
                    div()
                        .flex_grow(1.)
                        .min_w(px(0.))
                        .text_size(type_scale::BODY.font_size)
                        .line_height(type_scale::BODY.line_height)
                        .text_color(fg)
                        .child(label),
                )
                .when_some(shortcut, |el, shortcut| {
                    el.child(
                        div()
                            .text_size(type_scale::BODY.font_size)
                            .line_height(type_scale::BODY.line_height)
                            .text_color(if active {
                                palette.on_accent_soft
                            } else {
                                palette.tertiary
                            })
                            .pl(spacing::X3)
                            .child(shortcut),
                    )
                })
                .into_any_element()
        }
    }
}
