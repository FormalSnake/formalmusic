//! The Settings dialog. Playback and Equalizer hold the settings kept in
//! `config.json`; the app hands the audio ones to kopuzd. Scrobbling
//! connects Last.fm, with the user's own API account, and ListenBrainz.

use std::cell::Cell;
use std::rc::Rc;

use formalmusic_core::MusicStore;
use formalmusic_core::equalizer::{EQ_BANDS_HZ, EQ_MAX_DB, EqPreset, Equalizer};
use formalmusic_core::model::{LastFmApp, ScrobbleAccount, ScrobbleService};
use formalmusic_core::settings::Settings as ClientSettings;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::bridge::{Bridge, Topic};
use crate::primitives::{Button, ButtonKind, overlay_shadows};
use crate::theme::{Palette, Theme, radius, spacing, tabular, type_scale};

const LASTFM_CREATE: &str = "https://www.last.fm/api/account/create";
const EQ_TRACK_HEIGHT: Pixels = px(112.);

type OnClose = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

pub struct Settings {
    store: MusicStore,
    token: Entity<InputState>,
    api_key: Entity<InputState>,
    shared_secret: Entity<InputState>,
    /// The ListenBrainz connect panel is open.
    picking: bool,
    /// A ListenBrainz connect is running.
    connecting: bool,
    lastfm_error: Option<SharedString>,
    listenbrainz_error: Option<SharedString>,
    client: ClientSettings,
    client_error: Option<SharedString>,
    /// Each band slider's bounds, for turning a pointer into a gain.
    eq_bounds: [Rc<Cell<Bounds<Pixels>>>; 10],
    /// The band being dragged.
    eq_drag: Option<usize>,
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
            connecting: false,
            lastfm_error: None,
            listenbrainz_error: None,
            client: ClientSettings::load(&formalmusic_core::paths::settings_file()),
            client_error: None,
            eq_bounds: Default::default(),
            eq_drag: None,
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

    fn set_equalizer(&mut self, equalizer: Equalizer, cx: &mut Context<Self>) {
        if equalizer == self.client.equalizer {
            return;
        }
        let value = serde_json::to_value(equalizer).unwrap_or_default();
        if self.save(
            "equalizer",
            value,
            |client| client.equalizer = equalizer,
            cx,
        ) {
            self.store.reload_settings();
        }
    }

    /// Moving a band turns the sliders into the Custom preset, starting from
    /// what they showed.
    fn band_at(&self, band: usize, at: f32) -> Equalizer {
        let mut custom = self.client.equalizer.shown();
        custom[band] = ((0.5 - at) * 2. * EQ_MAX_DB).round() + 0.;
        Equalizer {
            preset: EqPreset::Custom,
            custom,
        }
    }

    /// While a band drags it is only heard; letting go keeps it.
    fn band_moved(&mut self, band: usize, at: f32, cx: &mut Context<Self>) {
        self.eq_drag = Some(band);
        let equalizer = self.band_at(band, at);
        if equalizer != self.client.equalizer {
            self.client.equalizer = equalizer;
            self.store.preview_equalizer(equalizer);
        }
        cx.notify();
    }

    fn band_released(&mut self, band: usize, at: f32, cx: &mut Context<Self>) {
        self.eq_drag = None;
        let equalizer = self.band_at(band, at);
        let value = serde_json::to_value(equalizer).unwrap_or_default();
        if self.save(
            "equalizer",
            value,
            |client| client.equalizer = equalizer,
            cx,
        ) {
            self.store.reload_settings();
        }
        cx.notify();
    }

    fn equalizer(&self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let equalizer = self.client.equalizer;
        let gains = equalizer.shown();
        let off = equalizer.gains().is_none();
        div()
            .flex()
            .flex_col()
            .gap(spacing::X2)
            .child(heading("Equalizer", palette))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(spacing::X4)
                    .p(spacing::X4)
                    .rounded(radius::CARD)
                    .bg(palette.press_wash)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap(spacing::X1)
                            .children(EqPreset::ALL.into_iter().map(|preset| {
                                let selected = equalizer.preset == preset;
                                div()
                                    .id(SharedString::from(format!("eq-preset-{preset:?}")))
                                    .h(px(28.))
                                    .px(spacing::X3)
                                    .flex()
                                    .items_center()
                                    .rounded(radius::PILL)
                                    .text_size(type_scale::CAPTION.font_size)
                                    .font_weight(FontWeight::MEDIUM)
                                    .map(|el| {
                                        if selected {
                                            el.bg(palette.accent).text_color(palette.on_accent)
                                        } else {
                                            el.bg(palette.canvas)
                                                .text_color(palette.secondary)
                                                .cursor_pointer()
                                                .hover(move |style| style.text_color(palette.text))
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    let custom = this.client.equalizer.custom;
                                                    this.set_equalizer(
                                                        Equalizer { preset, custom },
                                                        cx,
                                                    )
                                                }))
                                        }
                                    })
                                    .child(preset.label())
                            })),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .when(off, |el| el.opacity(0.5))
                            .children(
                                (0..EQ_BANDS_HZ.len())
                                    .map(|band| self.band_slider(band, gains[band], palette, cx)),
                            ),
                    )
                    .child(caption(
                        "Boosts lower the overall level so loud passages don't distort.",
                        palette,
                    )),
            )
            .into_any_element()
    }

    /// One band: its gain above, a vertical slider with 0 dB in the middle,
    /// and its frequency below.
    fn band_slider(
        &self,
        band: usize,
        gain: f32,
        palette: Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let bounds = self.eq_bounds[band].clone();
        let dragging = self.eq_drag == Some(band);
        // Fractions from the top of the track.
        let at = (0.5 - gain / (2. * EQ_MAX_DB)).clamp(0., 1.);
        let (fill_top, fill_bottom) = if at < 0.5 { (at, 0.5) } else { (0.5, at) };
        let hz = EQ_BANDS_HZ[band];
        let label = if hz >= 1000. {
            format!("{}k", hz / 1000.)
        } else {
            format!("{hz}")
        };
        let value = match gain {
            g if g > 0. => format!("+{g}"),
            g => format!("{g}"),
        };
        let group = SharedString::from(format!("eq-band-{band}"));
        div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .items_center()
            .gap(spacing::X1)
            .text_size(type_scale::CAPTION.font_size)
            .line_height(type_scale::CAPTION.line_height)
            .font_features(tabular())
            .child(
                div()
                    .text_color(if gain == 0. {
                        palette.secondary
                    } else {
                        palette.text
                    })
                    .child(value),
            )
            .child(
                div()
                    .id(group.clone())
                    .group(group.clone())
                    .relative()
                    .w_full()
                    .h(EQ_TRACK_HEIGHT)
                    .flex()
                    .justify_center()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let at = fraction_y(this.eq_bounds[band].get(), event.position.y);
                            this.band_moved(band, at, cx);
                        }),
                    )
                    .child(band_drag(bounds, band, dragging, cx.entity().downgrade()))
                    .child(
                        div()
                            .relative()
                            .w(px(3.))
                            .h_full()
                            .rounded(px(2.))
                            .bg(palette.progress_track)
                            .child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .right_0()
                                    .top(relative(fill_top))
                                    .h(relative(fill_bottom - fill_top))
                                    .bg(palette.accent),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(relative(at))
                            .mt(px(-6.))
                            .size(px(12.))
                            .rounded(px(6.))
                            .bg(palette.accent)
                            .opacity(if dragging { 1. } else { 0.85 })
                            .group_hover(group, |style| style.opacity(1.)),
                    ),
            )
            .child(div().text_color(palette.secondary).child(label))
            .into_any_element()
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
                    // The tray is drawn over StatusNotifierItem on Linux and
                    // in the notification area on Windows.
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
                                        crate::tray::set_shown(on, cx);
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

    /// Connects with the API account typed in, or with the one kopuzd
    /// already holds when both fields are left empty.
    fn connect_lastfm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let api_key = self.api_key.read(cx).value().trim().to_owned();
        let shared_secret = self.shared_secret.read(cx).value().trim().to_owned();
        if api_key.is_empty() != shared_secret.is_empty() {
            self.lastfm_error = Some("Enter both the API key and the shared secret.".into());
            cx.notify();
            return;
        }
        let app = (!api_key.is_empty()).then_some(LastFmApp {
            api_key,
            shared_secret,
        });
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
    }

    fn connect_token(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let token = self.token.read(cx).value().trim().to_owned();
        if token.is_empty() || self.connecting {
            return;
        }
        self.connecting = true;
        self.listenbrainz_error = None;
        cx.notify();
        let task = self.store.runtime().spawn({
            let store = self.store.clone();
            async move { store.connect_listenbrainz(token).await }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task
                .await
                .unwrap_or_else(|_| Err("Unable to reach ListenBrainz.".into()));
            let _ = this.update(cx, |this, cx| {
                this.connecting = false;
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
        let detail: SharedString = if account.connecting {
            "Allow access in your browser to finish connecting.".into()
        } else if account.connected {
            "Connected".into()
        } else {
            "Not connected".into()
        };
        let connected = account.connected;
        let needs_app = service == ScrobbleService::LastFm;
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
                    ScrobbleService::LastFm => this.connect_lastfm(window, cx),
                    ScrobbleService::ListenBrainz => this.open_picker(cx),
                }))
                .into_any_element()
        };
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
                                    .text_color(palette.secondary)
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
                        "Last.fm needs an API account of your own. Create one, then paste its API key and shared secret here, or leave both empty to use the one already saved.",
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
        let busy = self.connecting;
        div()
            .flex()
            .flex_col()
            .gap(spacing::X3)
            .child(caption(
                "Paste the user token from listenbrainz.org/settings",
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
                            if busy {
                                "Connecting\u{2026}"
                            } else {
                                "Connect"
                            },
                        )
                        .kind(ButtonKind::Primary)
                        .disabled(busy)
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
        let listenbrainz_open = self.picking && !status.listenbrainz.connected;
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
                    .child(self.equalizer(palette, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(spacing::X2)
                            .child(heading("Scrobbling", palette))
                            .child(self.service_row(
                                ScrobbleService::LastFm,
                                &status.lastfm,
                                palette,
                                cx,
                            ))
                            .child(self.service_row(
                                ScrobbleService::ListenBrainz,
                                &status.listenbrainz,
                                palette,
                                cx,
                            ))
                            .when(listenbrainz_open, |el| el.child(self.picker(palette, cx))),
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

fn fraction_y(bounds: Bounds<Pixels>, y: Pixels) -> f32 {
    let height = f32::from(bounds.size.height).max(1.);
    (f32::from(y - bounds.origin.y) / height).clamp(0., 1.)
}

/// Records a band slider's bounds, and while it is dragged follows the
/// pointer window-wide, so the drag keeps going outside the slider.
fn band_drag(
    bounds: Rc<Cell<Bounds<Pixels>>>,
    band: usize,
    dragging: bool,
    entity: WeakEntity<Settings>,
) -> impl IntoElement {
    canvas(
        {
            let bounds = bounds.clone();
            move |area, _, _| bounds.set(area)
        },
        move |_, _, window, _| {
            if !dragging {
                return;
            }
            let (moving, releasing) = (entity.clone(), entity.clone());
            let (move_bounds, release_bounds) = (bounds.clone(), bounds.clone());
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                if phase == DispatchPhase::Bubble && event.pressed_button == Some(MouseButton::Left)
                {
                    let at = fraction_y(move_bounds.get(), event.position.y);
                    let _ = moving.update(cx, |view, cx| view.band_moved(band, at, cx));
                }
            });
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                    let at = fraction_y(release_bounds.get(), event.position.y);
                    let _ = releasing.update(cx, |view, cx| view.band_released(band, at, cx));
                }
            });
        },
    )
    .absolute()
    .inset_0()
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
