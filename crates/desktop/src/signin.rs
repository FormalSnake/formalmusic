//! Sign in to YouTube Music. In order of preference: take the session from a
//! browser profile that is already signed in, sign in to Google in a new
//! browser window the daemon opens, or paste the Cookie header of a signed-in
//! music.youtube.com tab.

use formalmusic_core::MusicStore;
use formalmusic_core::model::{Browsers, ProfileBrowser};
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
    paste: bool,
    busy: bool,
    /// `None` until the daemon has listed them.
    browsers: Option<Browsers>,
    browser: Option<String>,
    profiles: Vec<ProfileBrowser>,
    /// The profile path being imported from.
    importing: Option<String>,
    /// The browser the user is signing in with, while the daemon waits.
    waiting: Option<SharedString>,
    cancelling: bool,
    /// The browser sign-in's call to kopuzd, dropped to cancel it.
    signing_in: Option<tokio::task::AbortHandle>,
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
        let task = store.runtime().spawn({
            let store = store.clone();
            async move { (store.browsers().await, store.browser_profiles().await) }
        });
        cx.spawn(async move |this, cx| {
            let (browsers, profiles) = task.await.unwrap_or_else(|_| {
                let lost = || "Sign in stopped unexpectedly.".to_owned();
                (Err(lost()), Err(lost()))
            });
            let browsers = browsers.unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.browser = browsers.default.clone();
                this.browsers = Some(browsers);
                this.profiles = profiles.unwrap_or_default();
                cx.notify();
            });
        })
        .detach();
        Self {
            store,
            input,
            error: None,
            paste: false,
            busy: false,
            browsers: None,
            browser: None,
            profiles: Vec::new(),
            importing: None,
            waiting: None,
            cancelling: false,
            signing_in: None,
            on_close: std::rc::Rc::new(on_close),
            _subscription: subscription,
        }
    }

    fn browser_name(&self) -> Option<SharedString> {
        let id = self.browser.as_ref()?;
        self.browsers
            .as_ref()?
            .installed
            .iter()
            .find(|b| &b.id == id)
            .map(|b| b.name.clone().into())
    }

    /// Names the account that signed in, so a wrong profile is easy to spot,
    /// and closes the screen.
    fn signed_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self
            .store
            .state()
            .session
            .as_ref()
            .and_then(|s| s.account.as_ref())
            .map(|a| a.name.clone());
        if let Some(name) = name {
            self.store.notice(format!("Signed in as {name}"));
        }
        (self.on_close)(window, cx);
    }

    fn import(&mut self, profile: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.importing.is_some() || self.waiting.is_some() {
            return;
        }
        self.importing = Some(profile.clone());
        self.error = None;
        cx.notify();
        let task = self.store.runtime().spawn({
            let store = self.store.clone();
            async move { store.import_cookies(profile).await }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Sign in stopped unexpectedly.".into()));
            let _ = this.update_in(cx, |this, window, cx| {
                this.importing = None;
                match result {
                    Ok(_) => this.signed_in(window, cx),
                    Err(error) => this.error = Some(error.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn sign_in_with_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.browser_name() else {
            return;
        };
        self.waiting = Some(name);
        self.cancelling = false;
        self.error = None;
        cx.notify();
        let browser = self.browser.clone();
        let task = self.store.runtime().spawn({
            let store = self.store.clone();
            async move { store.browser_sign_in(browser).await }
        });
        self.signing_in = Some(task.abort_handle());
        cx.spawn_in(window, async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Sign in stopped unexpectedly.".into()));
            let _ = this.update_in(cx, |this, window, cx| {
                this.waiting = None;
                match result {
                    Ok(()) => this.signed_in(window, cx),
                    Err(error) if !this.cancelling => this.error = Some(error.into()),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.cancelling = true;
        if let Some(task) = self.signing_in.take() {
            task.abort();
        }
        cx.notify();
    }

    fn show_paste(&mut self, paste: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.waiting.is_some() {
            self.cancel(cx);
        }
        self.paste = paste;
        self.error = None;
        if paste {
            window.focus(&self.input.focus_handle(cx), cx);
        }
        cx.notify();
    }

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        cx.spawn_in(window, async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Sign in stopped unexpectedly.".into()));
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(()) => this.signed_in(window, cx),
                    Err(error) => this.error = Some(error.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// One row per profile of every browser the user may already be signed
    /// in to; a click takes the session from it.
    fn profile_list(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.profiles.is_empty() {
            return None;
        }
        let locked = self.importing.is_some() || self.waiting.is_some();
        let rows = self
            .profiles
            .iter()
            .flat_map(|group| group.profiles.iter().map(move |profile| (group, profile)));
        Some(
            div()
                .flex()
                .flex_col()
                .gap(spacing::X2)
                .child(caption(
                    "Use a browser you're already signed in to",
                    palette,
                ))
                .child(div().flex().flex_col().gap(spacing::X1).children(rows.map(
                    |(group, profile)| {
                        let busy = self.importing.as_ref() == Some(&profile.path);
                        let detail = match &profile.email {
                            Some(email) => format!("{} \u{b7} {email}", group.browser.name),
                            None => group.browser.name.clone(),
                        };
                        let path = profile.path.clone();
                        div()
                            .id(SharedString::from(format!(
                                "profile-{}-{}",
                                group.browser.id, profile.path
                            )))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(spacing::X3)
                            .px(spacing::X3)
                            .py(spacing::X2)
                            .rounded(radius::CONTROL)
                            .bg(palette.press_wash)
                            .when(locked && !busy, |el| el.opacity(0.4))
                            .when(!locked, |el| {
                                el.cursor_pointer()
                                    .hover(move |style| style.bg(palette.raised_hover))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.import(path.clone(), window, cx)
                                    }))
                            })
                            .child(
                                Icon::new(IconName::Account)
                                    .size(px(16.))
                                    .color(palette.secondary),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_grow(1.)
                                    .min_w(px(0.))
                                    .child(
                                        div()
                                            .text_size(type_scale::BODY.font_size)
                                            .line_height(px(20.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(palette.text)
                                            .truncate()
                                            .child(profile.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(type_scale::CAPTION.font_size)
                                            .line_height(type_scale::CAPTION.line_height)
                                            .text_color(palette.secondary)
                                            .truncate()
                                            .child(detail),
                                    ),
                            )
                            .when(busy, |el| {
                                el.child(
                                    div()
                                        .flex_shrink_0()
                                        .text_size(type_scale::CAPTION.font_size)
                                        .text_color(palette.secondary)
                                        .child("Signing in\u{2026}"),
                                )
                            })
                    },
                )))
                .into_any_element(),
        )
    }

    fn browser_picker(&self, palette: Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let installed = &self.browsers.as_ref()?.installed;
        if installed.len() < 2 {
            return None;
        }
        let locked = self.waiting.is_some() || self.importing.is_some();
        Some(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap(spacing::X2)
                .children(installed.iter().map(|browser| {
                    let selected = self.browser.as_ref() == Some(&browser.id);
                    let id = browser.id.clone();
                    div()
                        .id(SharedString::from(format!("browser-{}", browser.id)))
                        .h(px(28.))
                        .px(spacing::X3)
                        .flex()
                        .items_center()
                        .rounded(radius::CONTROL)
                        .bg(if selected {
                            palette.text
                        } else {
                            palette.press_wash
                        })
                        .text_size(type_scale::BODY.font_size)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if selected {
                            palette.canvas
                        } else {
                            palette.text
                        })
                        .when(locked && !selected, |el| el.opacity(0.4))
                        .when(!locked && !selected, |el| {
                            el.cursor_pointer()
                                .hover(move |style| style.bg(palette.raised_hover))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.browser = Some(id.clone());
                                    cx.notify();
                                }))
                        })
                        .child(browser.name.clone())
                }))
                .into_any_element(),
        )
    }

    fn new_window(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let no_browser = self
            .browsers
            .as_ref()
            .is_some_and(|b| b.installed.is_empty());
        let kind = if self.profiles.is_empty() {
            ButtonKind::Primary
        } else {
            ButtonKind::Secondary
        };
        let actions = if self.waiting.is_some() {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(spacing::X2)
                .child(
                    Button::new("browser-sign-in", "Waiting for sign-in\u{2026}")
                        .kind(kind)
                        .disabled(true),
                )
                .child(
                    Button::new("cancel-sign-in", "Cancel")
                        .disabled(self.cancelling)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                )
        } else {
            div().flex().flex_row().child(
                Button::new("browser-sign-in", "Sign in with a new browser window")
                    .kind(kind)
                    .disabled(self.browser.is_none() || self.importing.is_some())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.sign_in_with_browser(window, cx)),
                    ),
            )
        };
        div()
            .flex()
            .flex_col()
            .gap(spacing::X2)
            .when(!self.profiles.is_empty(), |el| el.child(caption("Or sign in to Google", palette)))
            .children(self.browser_picker(palette, cx))
            .when(no_browser, |el| {
                el.child(div().text_size(type_scale::BODY.font_size).line_height(px(20.)).text_color(palette.secondary).child("No supported browser found. Install Firefox, Chromium or Helium, or paste cookies instead."))
            })
            .when_some(self.waiting.clone(), |el, name| {
                el.child(div().text_size(type_scale::BODY.font_size).line_height(px(20.)).text_color(palette.text).child(format!("Finish signing in in the {name} window.")))
            })
            .child(actions)
            .into_any_element()
    }
}

impl Drop for SignIn {
    /// Closing the screen closes the sign-in browser with it.
    fn drop(&mut self) {
        if let Some(task) = self.signing_in.take() {
            task.abort();
        }
    }
}

fn caption(text: &'static str, palette: Palette) -> impl IntoElement {
    div()
        .text_size(type_scale::CAPTION.font_size)
        .text_color(palette.secondary)
        .child(text)
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

fn text_link(
    id: &'static str,
    label: &'static str,
    palette: Palette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .cursor_pointer()
        .text_size(type_scale::CAPTION.font_size)
        .line_height(type_scale::CAPTION.line_height)
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.secondary)
        .hover(move |style| style.text_color(palette.text).underline())
        .on_click(on_click)
        .child(label)
        .into_any_element()
}

impl Render for SignIn {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::get(cx);
        let on_close = self.on_close.clone();
        let close_button = self.on_close.clone();
        let browse = self.on_close.clone();
        let subtitle = if self.paste {
            "Your library, likes and recommendations come from your account. The cookies stay on this computer."
        } else {
            "Your library, likes and recommendations come from your account. The session stays on this computer."
        };

        let body = if self.paste {
            div()
                .flex()
                .flex_col()
                .gap(spacing::X5)
                .child(
                    div().flex().flex_col().gap(spacing::X3).children(
                        STEPS
                            .iter()
                            .enumerate()
                            .map(|(n, text)| step(n, text, palette)),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(spacing::X1)
                        .child(caption("Cookie header", palette))
                        .child(
                            div()
                                .rounded(radius::CONTROL)
                                .border_1()
                                .border_color(palette.separator)
                                .p(spacing::X2)
                                .child(Textarea::new(&self.input).appearance(false)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(spacing::X2)
                        .child(
                            Button::new(
                                "sign-in-button",
                                if self.busy {
                                    "Signing in\u{2026}"
                                } else {
                                    "Sign in"
                                },
                            )
                            .kind(ButtonKind::Primary)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                        )
                        .child(
                            Button::new("browse-signed-out", "Browse without signing in")
                                .on_click(move |_, window, cx| on_close(window, cx)),
                        ),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap(spacing::X5)
                .children(self.profile_list(palette, cx))
                .child(self.new_window(palette, cx))
                .into_any_element()
        };

        let paste = self.paste;
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
                    .id("sign-in-card")
                    .flex()
                    .flex_col()
                    .gap(spacing::X5)
                    .w(px(520.))
                    .max_w_full()
                    .max_h_full()
                    .overflow_y_scroll()
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
                                    .child(
                                        Icon::new(IconName::Music)
                                            .size(px(28.))
                                            .color(palette.accent),
                                    )
                                    .child(
                                        div()
                                            .pt(spacing::X2)
                                            .text_size(type_scale::LARGE.font_size)
                                            .line_height(type_scale::LARGE.line_height)
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(palette.text)
                                            .child("Sign in to YouTube Music"),
                                    )
                                    .child(
                                        div()
                                            .text_size(type_scale::BODY.font_size)
                                            .line_height(px(20.))
                                            .text_color(palette.secondary)
                                            .child(subtitle),
                                    ),
                            )
                            .child(
                                IconButton::new("close-sign-in", IconName::Close, "Close (Esc)")
                                    .on_click(move |_, window, cx| close_button(window, cx)),
                            ),
                    )
                    .child(body)
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
                                .child(
                                    Icon::new(IconName::Alert)
                                        .size(px(14.))
                                        .color(palette.danger),
                                )
                                .child(
                                    div()
                                        .flex_grow(1.)
                                        .min_w(px(0.))
                                        .text_size(type_scale::CAPTION.font_size)
                                        .line_height(type_scale::CAPTION.line_height)
                                        .text_color(palette.text)
                                        .child(error),
                                ),
                        )
                    })
                    .child(if paste {
                        text_link(
                            "use-browser",
                            "Sign in with your browser instead",
                            palette,
                            cx.listener(|this, _, window, cx| this.show_paste(false, window, cx)),
                        )
                    } else {
                        div()
                            .flex()
                            .flex_row()
                            .gap(spacing::X4)
                            .child(text_link(
                                "paste-cookies",
                                "Paste cookies instead",
                                palette,
                                cx.listener(|this, _, window, cx| {
                                    this.show_paste(true, window, cx)
                                }),
                            ))
                            .child(text_link(
                                "browse-signed-out",
                                "Browse without signing in",
                                palette,
                                move |_, window, cx| browse(window, cx),
                            ))
                            .into_any_element()
                    }),
            )
    }
}
