//! Sign in by pasting the Cookie header of a signed-in music.youtube.com tab.
//! A WebKit sign-in window replaces this later; the daemon takes the same
//! cookies either way.

use formalmusic_core::MusicStore;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::icons::{Icon, IconName};
use crate::primitives::{Button, ButtonKind, IconButton};
use crate::theme::{Palette, Theme, radius, spacing, type_scale};

const STEPS: [&str; 4] = [
    "Open music.youtube.com in your browser and sign in.",
    "Open the developer tools (F12), then the Network tab, and reload the page.",
    "Select any request to music.youtube.com and find Cookie under Request Headers.",
    "Copy the whole value and paste it below.",
];

pub struct SignIn {
    store: MusicStore,
    input: Entity<TextareaState>,
    error: Option<SharedString>,
    busy: bool,
    on_close: std::rc::Rc<dyn Fn(&mut Window, &mut App)>,
    _subscription: Subscription,
}

impl SignIn {
    pub fn new(
        store: MusicStore,
        on_close: impl Fn(&mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(4, 8)
                .placeholder("SID=...; HSID=...; SAPISID=...; __Secure-3PSID=...")
        });
        let subscription = cx.subscribe(&input, |this: &mut Self, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) && this.error.is_some() {
                this.error = None;
                cx.notify();
            }
        });
        window.focus(&input.focus_handle(cx), cx);
        Self {
            store,
            input,
            error: None,
            busy: false,
            on_close: std::rc::Rc::new(on_close),
            _subscription: subscription,
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let cookies = self.input.read(cx).value().trim().to_owned();
        if cookies.is_empty() {
            self.error = Some("Paste the Cookie header first.".into());
            cx.notify();
            return;
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        let store = self.store.clone();
        let task = store.runtime().spawn({
            let store = store.clone();
            async move { store.sign_in(cookies).await }
        });
        cx.spawn(async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Sign in stopped unexpectedly.".into()));
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                if let Err(error) = result {
                    this.error = Some(error.into());
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn step(n: usize, text: &'static str, palette: Palette) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(spacing::X3)
        .child(
            div()
                .size(px(20.))
                .flex_shrink_0()
                .rounded(px(10.))
                .bg(palette.press_wash)
                .flex()
                .items_center()
                .justify_center()
                .text_size(type_scale::MICRO.font_size)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.text)
                .child((n + 1).to_string()),
        )
        .child(
            div()
                .flex_grow(1.)
                .min_w(px(0.))
                .text_size(type_scale::BODY.font_size)
                .line_height(px(20.))
                .text_color(palette.secondary)
                .child(text),
        )
}

impl Render for SignIn {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::get(cx);
        let on_close = self.on_close.clone();
        let close_button = self.on_close.clone();
        div()
            .id("sign-in")
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p(spacing::X6)
            .bg(palette.canvas)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(spacing::X5)
                    .w(px(520.))
                    .max_w_full()
                    .p(spacing::X8)
                    .rounded(radius::CARD)
                    .bg(palette.sidebar)
                    .border_1()
                    .border_color(palette.sidebar_border)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_start()
                            .gap(spacing::X2)
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(spacing::X1)
                                    .flex_grow(1.)
                                    .min_w(px(0.))
                                    .child(Icon::new(IconName::Music).size(px(28.)).color(palette.accent))
                                    .child(div().pt(spacing::X2).text_size(type_scale::LARGE.font_size).line_height(type_scale::LARGE.line_height).font_weight(FontWeight::BOLD).text_color(palette.text).child("Sign in to YouTube Music"))
                                    .child(div().text_size(type_scale::BODY.font_size).line_height(px(20.)).text_color(palette.secondary).child("Your library, likes and recommendations come from your account. The cookies stay on this computer.")),
                            )
                            .child(IconButton::new("close-sign-in", IconName::Close, "Close (Esc)").on_click(move |_, window, cx| close_button(window, cx))),
                    )
                    .child(div().flex().flex_col().gap(spacing::X3).children(STEPS.iter().enumerate().map(|(n, text)| step(n, text, palette))))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(spacing::X1)
                            .child(div().text_size(type_scale::CAPTION.font_size).text_color(palette.secondary).child("Cookie header"))
                            .child(div().rounded(radius::CONTROL).border_1().border_color(palette.separator).p(spacing::X2).child(Textarea::new(&self.input).appearance(false))),
                    )
                    .when_some(self.error.clone(), |el, error| {
                        el.child(
                            div()
                                .flex()
                                .flex_row()
                                .items_start()
                                .gap(spacing::X2)
                                .p(spacing::X3)
                                .rounded(radius::CONTROL)
                                .bg(palette.danger_soft)
                                .child(Icon::new(IconName::Alert).size(px(14.)).color(palette.danger))
                                .child(div().flex_grow(1.).min_w(px(0.)).text_size(type_scale::CAPTION.font_size).line_height(type_scale::CAPTION.line_height).text_color(palette.text).child(error)),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(spacing::X2)
                            .child(Button::new("sign-in-button", if self.busy { "Signing in\u{2026}" } else { "Sign in" }).kind(ButtonKind::Primary).disabled(self.busy).on_click(cx.listener(|this, _, _, cx| this.submit(cx))))
                            .child(Button::new("browse-signed-out", "Browse without signing in").on_click(move |_, window, cx| on_close(window, cx))),
                    ),
            )
    }
}
