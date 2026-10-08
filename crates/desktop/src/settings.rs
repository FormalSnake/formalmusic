//! The Settings dialog. Playback and Privacy hold the settings kept in
//! `config.json`; the daemon reads the audio ones from there too.
//! Scrobbling has Last.fm and ListenBrainz, each with its account, a switch
//! for scrobbling and one for now playing, and a line for plays still
//! waiting to go out. Last.fm asks for the user's own API account first
//! when the daemon has none.

use formalmusic_api::{
    LastFmApp, ListenBrainzSource, ProfileBrowser, ScrobbleAccount, ScrobbleService,
};
use formalmusic_core::MusicStore;
use formalmusic_core::settings::{AudioQuality, Settings as ClientSettings};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::bridge::{Bridge, Topic};
use crate::icons::{Icon, IconName};
use crate::primitives::{Button, ButtonKind, overlay_shadows};
use crate::theme::{Palette, Theme, radius, spacing, type_scale};

const LASTFM_CREATE: &str = "https://www.last.fm/api/account/create";

type OnClose = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

pub struct Settings {
    store: MusicStore,
    token: Entity<InputState>,
    api_key: Entity<InputState>,
    shared_secret: Entity<InputState>,
    /// The ListenBrainz connect panel is open.
    picking: bool,
    profiles: Vec<ProfileBrowser>,
    /// The profile path or "token" while a ListenBrainz connect runs.
    connecting: Option<String>,
    lastfm_error: Option<SharedString>,
    listenbrainz_error: Option<SharedString>,
    client: ClientSettings,
    client_error: Option<SharedString>,
    on_close: OnClose,
    _subscription: Subscription,
}

impl Settings {
    pub fn new(
        store: MusicStore,
        on_close: impl Fn(&mut Window, &mut App) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let weak = cx.entity().downgrade();
        Bridge::watch(cx, Topic::Scrobbling, weak.into());
        store.load_scrobbling();
        let api_key = cx.new(|cx| InputState::new(window, cx));
        let shared_secret = cx.new(|cx| InputState::new(window, cx));
        let token = cx.new(|cx| {
            InputState::new(window, cx).placeholder("00000000-0000-0000-0000-000000000000")
        });
        let subscription = cx.subscribe_in(
            &token,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => this.connect_token(window, cx),
                InputEvent::Change if this.listenbrainz_error.is_some() => {
                    this.listenbrainz_error = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        Self {
            store,
            token,
            api_key,
            shared_secret,
            picking: false,
            profiles: Vec::new(),
            connecting: None,
            lastfm_error: None,
            listenbrainz_error: None,
            client: ClientSettings::load(&formalmusic_core::paths::settings_file()),
            client_error: None,
            on_close: std::rc::Rc::new(on_close),
            _subscription: subscription,
        }
    }

    /// Writes one switch to `config.json`; `apply` records it here once it
    /// is saved.
    fn set_client(
        &mut self,
        key: &str,
        on: bool,
        apply: fn(&mut ClientSettings, bool),
        cx: &mut Context<Self>,
    ) -> bool {
        self.save(key, on.into(), |client| apply(client, on), cx)
    }

    /// A switch the daemon acts on: saved, then the daemon reads the file again.
    fn set_daemon(
        &mut self,
        key: &str,
        on: bool,
        apply: fn(&mut ClientSettings, bool),
        cx: &mut Context<Self>,
    ) {
        if self.set_client(key, on, apply, cx) {
            self.store.reload_settings();
        }
    }

    fn set_quality(&mut self, quality: AudioQuality, cx: &mut Context<Self>) {
        let value = serde_json::to_value(quality).unwrap_or_default();
        if self.save(
            "audioQuality",
            value,
            |client| client.audio_quality = quality,
            cx,
        ) {
            self.store.reload_settings();
        }
    }

    fn save(
        &mut self,
        key: &str,
        value: serde_json::Value,
        apply: impl FnOnce(&mut ClientSettings),
        cx: &mut Context<Self>,
    ) -> bool {
        let path = formalmusic_core::paths::settings_file();
        let saved = ClientSettings::write(&path, key, value);
        match &saved {
            Ok(()) => {
                apply(&mut self.client);
                self.client_error = None;
            }
            Err(error) => self.client_error = Some(format!("Unable to save: {error}").into()),
        }
        cx.notify();
        saved.is_ok()
    }

    fn playback(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let keep = cx.entity().downgrade();
        let tray = keep.clone();
        let (autoplay, explicit) = (keep.clone(), keep.clone());
        div()
            .flex()
            .flex_col()
            .gap(spacing::X2)
            .child(heading("Playback", palette))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(spacing::X3)
                    .p(spacing::X4)
                    .rounded(radius::CARD)
                    .bg(palette.press_wash)
                    .child(self.quality_picker(palette, cx))
                    .child(switch(
                        "autoplay".into(),
                        "Autoplay similar songs when the queue ends",
                        self.client.autoplay,
                        palette,
                        move |on, cx| {
                            let _ = autoplay.update(cx, |this, cx| {
                                this.set_daemon("autoplay", on, |c, on| c.autoplay = on, cx)
                            });
                        },
                    ))
                    .child(switch(
                        "restrict-explicit".into(),
                        "Skip explicit songs",
                        self.client.restrict_explicit,
                        palette,
                        move |on, cx| {
                            let _ = explicit.update(cx, |this, cx| {
                                this.set_daemon(
                                    "restrictExplicit",
                                    on,
                                    |c, on| c.restrict_explicit = on,
                                    cx,
                                )
                            });
                        },
                    ))
                    .child(switch(
                        "keep-playing-when-closed".into(),
                        "Keep playing after closing the window",
                        self.client.keep_playing_when_closed,
                        palette,
                        move |on, cx| {
                            let _ = keep.update(cx, |this, cx| {
                                this.set_client(
                                    "keepPlayingWhenClosed",
                                    on,
                                    |client, on| client.keep_playing_when_closed = on,
                                    cx,
                                )
                            });
                        },
                    ))
                    // The daemon draws the tray, over StatusNotifierItem on
                    // Linux and in the notification area on Windows.
                    .when(cfg!(any(target_os = "linux", windows)), |el| {
                        el.child(switch(
                            "show-in-tray".into(),
                            "Show in the system tray",
                            self.client.show_in_tray,
                            palette,
                            move |on, cx| {
                                let _ = tray.update(cx, |this, cx| {
                                    if this.set_client(
                                        "showInTray",
                                        on,
                                        |client, on| client.show_in_tray = on,
                                        cx,
                                    ) {
                                        this.store.set_tray(on);
                                    }
                                });
                            },
                        ))
                    })
                    .when_some(self.client_error.clone(), |el, error| {
                        el.child(
                            div()
                                .text_size(type_scale::CAPTION.font_size)
                                .line_height(type_scale::CAPTION.line_height)
                                .text_color(palette.danger)
                                .child(error),
                        )
                    }),
            )
            .into_any_element()
    }

    /// Auto, Low, Normal and High, as the web app offers them.
    fn quality_picker(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let options = [
            (AudioQuality::Auto, "Auto"),
            (AudioQuality::Low, "Low"),
            (AudioQuality::Normal, "Normal"),
            (AudioQuality::High, "High"),
        ];
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(spacing::X3)
            .child(
                div()
                    .flex_grow(1.)
                    .min_w(px(0.))
                    .text_size(type_scale::BODY.font_size)
                    .line_height(px(20.))
                    .text_color(palette.text)
                    .child("Audio quality"),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_shrink_0()
                    .p(px(2.))
                    .gap(px(2.))
                    .rounded(radius::CONTROL)
                    .bg(palette.canvas)
                    .children(options.into_iter().map(|(quality, label)| {
                        let selected = self.client.audio_quality == quality;
                        div()
                            .id(SharedString::from(format!("quality-{label}")))
                            .h(px(24.))
                            .px(spacing::X2)
                            .flex()
                            .items_center()
                            .rounded(radius::CONTROL - px(2.))
                            .text_size(type_scale::CAPTION.font_size)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if selected {
                                palette.text
                            } else {
                                palette.secondary
                            })
                            .when(selected, |el| el.bg(palette.raised_hover))
                            .when(!selected, |el| {
                                el.cursor_pointer()
                                    .hover(move |style| style.text_color(palette.text))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_quality(quality, cx)
                                    }))
                            })
                            .child(label)
                    })),
            )
            .into_any_element()
    }

    fn privacy(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let weak = cx.entity().downgrade();
        div()
            .flex()
            .flex_col()
            .gap(spacing::X2)
            .child(heading("Privacy", palette))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(spacing::X3)
                    .p(spacing::X4)
                    .rounded(radius::CARD)
                    .bg(palette.press_wash)
                    .child(switch(
                        "pause-history".into(),
                        "Pause watch history",
                        self.client.pause_history,
                        palette,
                        move |on, cx| {
                            let _ = weak.update(cx, |this, cx| {
                                this.set_daemon(
                                    "pauseHistory",
                                    on,
                                    |c, on| c.pause_history = on,
                                    cx,
                                )
                            });
                        },
                    ))
                    .child(caption(
                        "Songs you play stay out of History and stop shaping your recommendations.",
                        palette,
                    )),
            )
            .into_any_element()
    }

    fn connect_lastfm(&mut self, needs_app: bool, window: &mut Window, cx: &mut Context<Self>) {
        let app = needs_app.then(|| LastFmApp {
            api_key: self.api_key.read(cx).value().trim().to_owned(),
            shared_secret: self.shared_secret.read(cx).value().trim().to_owned(),
        });
        if app
            .as_ref()
            .is_some_and(|a| a.api_key.is_empty() || a.shared_secret.is_empty())
        {
            self.lastfm_error = Some("Enter both the API key and the shared secret.".into());
            cx.notify();
            return;
        }
        self.lastfm_error = None;
        cx.notify();
        let task = self.store.runtime().spawn({
            let store = self.store.clone();
            async move { store.connect_lastfm(app).await }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Unable to reach Last.fm.".into()));
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = result {
                    this.lastfm_error = Some(error.into());
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_picker(&mut self, cx: &mut Context<Self>) {
        self.picking = true;
        self.listenbrainz_error = None;
        cx.notify();
        let task = self.store.runtime().spawn({
            let store = self.store.clone();
            async move { store.browser_profiles().await }
        });
        cx.spawn(async move |this, cx| {
            let profiles = task.await.ok().and_then(Result::ok).unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                this.profiles = profiles;
                cx.notify();
            });
        })
        .detach();
    }

    fn connect_token(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let token = self.token.read(cx).value().trim().to_owned();
        if token.is_empty() {
            return;
        }
        self.connect_listenbrainz(
            ListenBrainzSource::Token { token },
            "token".into(),
            window,
            cx,
        );
    }

    fn connect_listenbrainz(
        &mut self,
        source: ListenBrainzSource,
        key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.connecting.is_some() {
            return;
        }
        self.connecting = Some(key);
        self.listenbrainz_error = None;
        cx.notify();
        let task = self.store.runtime().spawn({
            let store = self.store.clone();
            async move { store.connect_listenbrainz(source).await }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Unable to reach ListenBrainz.".into()));
            let _ = this.update(cx, |this, cx| {
                this.connecting = None;
                match result {
                    Ok(()) => this.picking = false,
                    Err(error) => this.listenbrainz_error = Some(error.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn service_row(
        &self,
        service: ScrobbleService,
        account: &ScrobbleAccount,
        needs_app: bool,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (name, id) = match service {
            ScrobbleService::LastFm => ("Last.fm", "lastfm"),
            ScrobbleService::ListenBrainz => ("ListenBrainz", "listenbrainz"),
        };
        let local_error = match service {
            ScrobbleService::LastFm => self.lastfm_error.clone(),
            ScrobbleService::ListenBrainz => self.listenbrainz_error.clone(),
        };
        let (detail, detail_color): (SharedString, Hsla) = if let Some(error) = &account.error {
            (error.clone().into(), palette.danger)
        } else if account.connecting {
            (
                "Allow access in your browser to finish connecting.".into(),
                palette.secondary,
            )
        } else if let Some(user) = &account.username {
            (format!("Connected as {user}").into(), palette.secondary)
        } else {
            ("Not connected".into(), palette.secondary)
        };
        let connected = account.username.is_some();
        let store = self.store.clone();
        let action = if connected || account.connecting {
            Button::new(
                SharedString::from(format!("{id}-disconnect")),
                if account.connecting {
                    "Cancel"
                } else {
                    "Disconnect"
                },
            )
            .on_click(move |_, _, _| store.disconnect_scrobbler(service))
            .into_any_element()
        } else {
            Button::new(SharedString::from(format!("{id}-connect")), "Connect")
                .kind(ButtonKind::Primary)
                .disabled(service == ScrobbleService::ListenBrainz && self.picking)
                .on_click(cx.listener(move |this, _, window, cx| match service {
                    ScrobbleService::LastFm => this.connect_lastfm(needs_app, window, cx),
                    ScrobbleService::ListenBrainz => this.open_picker(cx),
                }))
                .into_any_element()
        };
        let (scrobble, now_playing) = (account.scrobble, account.now_playing);
        div()
            .flex()
            .flex_col()
            .gap(spacing::X3)
            .p(spacing::X4)
            .rounded(radius::CARD)
            .bg(palette.press_wash)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(spacing::X3)
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
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_size(type_scale::CAPTION.font_size)
                                    .line_height(type_scale::CAPTION.line_height)
                                    .text_color(detail_color)
                                    .child(detail),
                            ),
                    )
                    .child(action),
            )
            .when(needs_app && !connected && !account.connecting, |el| {
                el.child(self.app_fields(palette))
            })
            .when_some(local_error, |el, error| {
                el.child(
                    div()
                        .text_size(type_scale::CAPTION.font_size)
                        .line_height(type_scale::CAPTION.line_height)
                        .text_color(palette.danger)
                        .child(error),
                )
            })
            .when(connected, |el| {
                let (a, b) = (self.store.clone(), self.store.clone());
                el.child(switch(
                    SharedString::from(format!("{id}-scrobble")),
                    "Scrobble tracks you play",
                    scrobble,
                    palette,
                    move |on, _| a.set_scrobbling(service, on, now_playing),
                ))
                .child(switch(
                    SharedString::from(format!("{id}-now-playing")),
                    "Show what's playing now",
                    now_playing,
                    palette,
                    move |on, _| b.set_scrobbling(service, scrobble, on),
                ))
            })
            .into_any_element()
    }

    /// The user's own Last.fm API account, which Last.fm signs every call with.
    fn app_fields(&self, palette: Palette) -> AnyElement {
        let field = |label: &'static str, input: &Entity<InputState>| {
            div()
                .flex()
                .flex_col()
                .gap(spacing::X1)
                .child(caption(label, palette))
                .child(Input::new(input).bordered(true))
        };
        div()
            .flex()
            .flex_col()
            .gap(spacing::X3)
            .child(
                div()
                    .text_size(type_scale::BODY.font_size)
                    .line_height(px(20.))
                    .text_color(palette.secondary)
                    .child(
                        "Last.fm needs an API account of your own. Create one, then paste its API key and shared secret here.",
                    ),
            )
            .child(
                div().flex().flex_row().child(
                    Button::new("lastfm-create-app", "Create an API account")
                        .on_click(|_, _, cx| cx.open_url(LASTFM_CREATE)),
                ),
            )
            .child(field("API key", &self.api_key))
            .child(field("Shared secret", &self.shared_secret))
            .into_any_element()
    }

    fn picker(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let locked = self.connecting.is_some();
        let rows = self
            .profiles
            .iter()
            .flat_map(|group| group.profiles.iter().map(move |profile| (group, profile)));
        let token_busy = self.connecting.as_deref() == Some("token");
        div()
            .flex()
            .flex_col()
            .gap(spacing::X3)
            .child(caption(
                "Use a browser signed in to listenbrainz.org",
                palette,
            ))
            .child(div().flex().flex_col().gap(spacing::X1).children(rows.map(
                |(group, profile)| {
                    let busy = self.connecting.as_ref() == Some(&profile.path);
                    let (browser, path) = (group.browser.id.clone(), profile.path.clone());
                    div()
                        .id(SharedString::from(format!(
                            "lb-profile-{}-{}",
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
                                    let source = ListenBrainzSource::Profile {
                                        browser: browser.clone(),
                                        profile: path.clone(),
                                    };
                                    this.connect_listenbrainz(source, path.clone(), window, cx)
                                }))
                        })
                        .child(
                            Icon::new(IconName::Account)
                                .size(px(16.))
                                .color(palette.secondary),
                        )
                        .child(
                            div()
                                .flex_grow(1.)
                                .min_w(px(0.))
                                .text_size(type_scale::BODY.font_size)
                                .line_height(px(20.))
                                .text_color(palette.text)
                                .truncate()
                                .child(format!("{} \u{b7} {}", profile.name, group.browser.name)),
                        )
                        .when(busy, |el| {
                            el.child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(type_scale::CAPTION.font_size)
                                    .text_color(palette.secondary)
                                    .child("Connecting\u{2026}"),
                            )
                        })
                },
            )))
            .child(caption(
                "Or paste the user token from listenbrainz.org/settings",
                palette,
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(spacing::X2)
                    .child(
                        div()
                            .flex_grow(1.)
                            .child(Input::new(&self.token).bordered(true)),
                    )
                    .child(
                        Button::new(
                            "listenbrainz-token-connect",
                            if token_busy {
                                "Connecting\u{2026}"
                            } else {
                                "Connect"
                            },
                        )
                        .kind(ButtonKind::Primary)
                        .disabled(locked)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.connect_token(window, cx)),
                        ),
                    ),
            )
            .into_any_element()
    }
}

impl Render for Settings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::get(cx);
        let status = self.store.state().scrobbling.clone().unwrap_or_default();
        let on_close = self.on_close.clone();
        let outside = self.on_close.clone();
        let queued = match status.queued {
            0 => None,
            1 => Some("1 play is waiting to be sent.".to_owned()),
            n => Some(format!("{n} plays are waiting to be sent.")),
        };
        let listenbrainz_open = self.picking && status.listenbrainz.username.is_none();
        div()
            .id("settings-layer")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(palette.scrim)
            .occlude()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| outside(window, cx))
            .child(
                div()
                    .id("settings-dialog")
                    .w(px(460.))
                    .max_h(relative(0.85))
                    .overflow_y_scroll()
                    .p(spacing::X6)
                    .flex()
                    .flex_col()
                    .gap(spacing::X4)
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
                            .child("Settings"),
                    )
                    .child(self.playback(palette, cx))
                    .child(self.privacy(palette, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(spacing::X2)
                            .child(heading("Scrobbling", palette))
                            .child(self.service_row(
                                ScrobbleService::LastFm,
                                &status.lastfm,
                                !status.lastfm_app,
                                palette,
                                cx,
                            ))
                            .child(self.service_row(
                                ScrobbleService::ListenBrainz,
                                &status.listenbrainz,
                                false,
                                palette,
                                cx,
                            ))
                            .when(listenbrainz_open, |el| el.child(self.picker(palette, cx)))
                            .when_some(queued, |el, text| el.child(caption(text, palette))),
                    )
                    .child(
                        div().flex().flex_row().justify_end().child(
                            Button::new("settings-done", "Done")
                                .on_click(move |_, window, cx| on_close(window, cx)),
                        ),
                    ),
            )
    }
}

/// A labelled on/off switch; the label says what happens when it is on.
fn switch(
    id: SharedString,
    label: &'static str,
    on: bool,
    palette: Palette,
    toggle: impl Fn(bool, &mut App) + 'static,
) -> AnyElement {
    let track = if on { palette.accent } else { palette.ghost };
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(spacing::X3)
        .cursor_pointer()
        .on_click(move |_, _, cx| toggle(!on, cx))
        .child(
            div()
                .flex_grow(1.)
                .min_w(px(0.))
                .text_size(type_scale::BODY.font_size)
                .line_height(px(20.))
                .text_color(palette.text)
                .child(label),
        )
        .child(
            div()
                .w(px(32.))
                .h(px(18.))
                .flex_shrink_0()
                .p(px(2.))
                .rounded(radius::PILL)
                .bg(track)
                .flex()
                .flex_row()
                .when(on, |el| el.justify_end())
                .child(
                    div()
                        .size(px(14.))
                        .rounded(radius::PILL)
                        .bg(palette.on_accent),
                ),
        )
        .into_any_element()
}

fn heading(text: &'static str, palette: Palette) -> impl IntoElement {
    div()
        .text_size(type_scale::TITLE.font_size)
        .line_height(type_scale::TITLE.line_height)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(palette.text)
        .child(text)
}

fn caption(text: impl Into<SharedString>, palette: Palette) -> impl IntoElement {
    div()
        .text_size(type_scale::CAPTION.font_size)
        .line_height(type_scale::CAPTION.line_height)
        .text_color(palette.secondary)
        .child(text.into())
}
