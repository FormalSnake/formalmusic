//! The settings window: `config.toml` through nd-settings, so a key Home Manager set shows
//! read-only, and the daemon picks up every write from the file. Scrobbling lives in the
//! daemon's state and goes through the store.

use formalmusic_api::settings::{Quality, Settings};
use formalmusic_api::{
    EQ_BANDS_HZ, EQ_MAX_DB, EqPreset, LastFmApp, ListenBrainzSource, ScrobbleAccount,
    ScrobbleService,
};
use formalmusic_core::StoreEvent;
use nd::prelude::*;
use nd_settings::{KeyState, SettingsHandle};
use serde::Serialize;

use crate::bridge;

pub struct SettingsWindow {
    lastfm_key: SharedString,
    lastfm_secret: SharedString,
    error: Option<String>,
}

pub fn open(cx: &mut App) {
    nd::chrome::open_settings(cx, |_, cx| cx.new(SettingsWindow::new));
}

fn handle(cx: &App) -> Option<SettingsHandle<Settings>> {
    crate::music(cx).settings.clone()
}

/// The settings in force, or the defaults when none could be read.
pub fn current(cx: &App) -> Settings {
    handle(cx).map(|h| (*h.get()).clone()).unwrap_or_default()
}

fn write<V: Serialize>(key: &str, value: V, cx: &mut App) -> Option<String> {
    let handle = handle(cx)?;
    let result = handle.set(key, &value);
    cx.refresh_windows();
    // The daemon watches the file too; this makes a running old daemon reread it at once.
    crate::store(cx).reload_settings();
    result.err().map(|e| format!("Unable to save: {e:?}"))
}

fn editable(key: &str, cx: &App) -> bool {
    handle(cx).is_some_and(|h| h.key_state(key) == KeyState::Editable)
}

impl SettingsWindow {
    fn new(cx: &mut Context<Self>) -> Self {
        let id = cx.entity_id();
        bridge::watch(cx, StoreEvent::Scrobbling, id);
        crate::store(cx).load_scrobbling();
        Self {
            lastfm_key: SharedString::default(),
            lastfm_secret: SharedString::default(),
            error: None,
        }
    }

    fn switch(
        &self,
        key: &'static str,
        title: &'static str,
        on: bool,
        cx: &mut Context<Self>,
    ) -> nd::components::forms::SwitchRow {
        switch_row(key, title)
            .checked(on)
            .disabled(!editable(key, cx))
            .on_toggle(cx.listener(move |this, e: &Toggled, _, cx| {
                this.error = write(key, e.checked, cx);
                cx.notify();
            }))
    }

    fn scrobbler(
        &self,
        service: ScrobbleService,
        title: &'static str,
        account: Option<&ScrobbleAccount>,
        cx: &mut Context<Self>,
    ) -> FormSection {
        let id = match service {
            ScrobbleService::LastFm => "lastfm",
            ScrobbleService::ListenBrainz => "listenbrainz",
        };
        let status = match account {
            None => "Not connected".to_owned(),
            Some(a) if a.connecting => {
                "Allow access in your browser to finish connecting.".to_owned()
            }
            Some(a) => match &a.error {
                Some(e) => e.clone(),
                None => format!("Connected as {}", a.username.clone().unwrap_or_default()),
            },
        };
        let connected = account.is_some_and(|a| !a.connecting && a.username.is_some());
        let mut section =
            settings_group(title).child(row("Status").child(label(status).secondary()));
        if let Some(a) = account.filter(|_| connected) {
            let (scrobble, now_playing) = (a.scrobble, a.now_playing);
            section = section
                .child(
                    switch_row(format!("{id}.scrobble"), "Scrobble tracks you play")
                        .checked(scrobble)
                        .on_toggle(move |e: &Toggled, _, cx| {
                            crate::store(cx).set_scrobbling(service, e.checked, now_playing)
                        }),
                )
                .child(
                    switch_row(format!("{id}.now-playing"), "Show what's playing now")
                        .checked(now_playing)
                        .on_toggle(move |e: &Toggled, _, cx| {
                            crate::store(cx).set_scrobbling(service, scrobble, e.checked)
                        }),
                );
        }
        let action = if connected || account.is_some_and(|a| a.connecting) {
            button(
                format!("{id}.disconnect"),
                if connected { "Disconnect" } else { "Cancel" },
            )
            .on_click(move |_, _, cx| crate::store(cx).disconnect_scrobbler(service))
        } else {
            button(format!("{id}.connect"), "Connect")
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.connect(service, cx)))
        };
        section.child(row("").child(action))
    }

    fn connect(&mut self, service: ScrobbleService, cx: &mut Context<Self>) {
        let store = crate::store(cx);
        let page = cx.weak_entity();
        let done = move |result: Result<(), String>, cx: &mut App| {
            page.update(cx, |this, cx| {
                this.error = result.err();
                cx.notify();
            });
        };
        match service {
            ScrobbleService::LastFm => {
                let has_app = store
                    .state()
                    .scrobbling
                    .as_ref()
                    .is_some_and(|s| s.lastfm_app);
                let app = if has_app {
                    None
                } else {
                    let (key, secret) = (
                        self.lastfm_key.trim().to_owned(),
                        self.lastfm_secret.trim().to_owned(),
                    );
                    if key.is_empty() || secret.is_empty() {
                        self.error = Some("Enter both the API key and the shared secret.".into());
                        cx.notify();
                        return;
                    }
                    Some(LastFmApp {
                        api_key: key,
                        shared_secret: secret,
                    })
                };
                bridge::task(cx, async move { store.connect_lastfm(app).await }, done);
            }
            ScrobbleService::ListenBrainz => {
                // The first browser profile signed in to listenbrainz.org; the daemon reads its session.
                let probe = store.clone();
                bridge::task(
                    cx,
                    async move {
                        let profiles = probe.browser_profiles().await?;
                        let (browser, profile) = profiles
                            .iter()
                            .find_map(|b| {
                                b.profiles
                                    .first()
                                    .map(|p| (b.browser.id.clone(), p.path.clone()))
                            })
                            .ok_or_else(|| {
                                "Use a browser signed in to listenbrainz.org".to_owned()
                            })?;
                        probe
                            .connect_listenbrainz(ListenBrainzSource::Profile { browser, profile })
                            .await
                    },
                    done,
                );
            }
        }
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = current(cx);
        let quality = Quality::ALL
            .iter()
            .position(|q| *q == s.playback.quality)
            .unwrap_or(0);
        let eq = s.playback.equalizer;
        let preset = EqPreset::ALL
            .iter()
            .position(|p| *p == eq.preset)
            .unwrap_or(0);
        let shown = eq.shown();
        let scrobbling = crate::store(cx)
            .state()
            .scrobbling
            .clone()
            .unwrap_or_default();

        let playback = settings_group("Playback")
            .child(
                row("Audio quality").child(
                    select("playback.quality", Quality::ALL.map(Quality::label))
                        .selected(quality)
                        .disabled(!editable("playback.quality", cx))
                        .on_change(cx.listener(|this, e: &SelectChanged, _, cx| {
                            this.error =
                                write("playback.quality", Quality::ALL[e.index.min(3)], cx);
                            cx.notify();
                        })),
                ),
            )
            .child(self.switch(
                "playback.autoplay",
                "Autoplay similar songs when the queue ends",
                s.playback.autoplay,
                cx,
            ))
            .child(self.switch(
                "playback.restrict_explicit",
                "Skip explicit songs",
                s.playback.restrict_explicit,
                cx,
            ))
            .child(self.switch(
                "playback.normalisation",
                "Even out loudness",
                s.playback.normalisation,
                cx,
            ))
            .child(
                row(format!(
                    "Crossfade: {:.1} s",
                    f64::from(s.playback.crossfade_ms) / 1000.
                ))
                .child(
                    slider(
                        "playback.crossfade_ms",
                        s.playback.crossfade_ms as f32 / 12000.,
                    )
                    .label("Crossfade")
                    .w(px(200.))
                    .on_change(cx.listener(
                        |this, e: &SliderChanged, _, cx| {
                            let ms = ((e.value * 24.).round() * 500.) as u32;
                            this.error = write("playback.crossfade_ms", ms, cx);
                            cx.notify();
                        },
                    )),
                ),
            )
            .child(self.switch(
                "window.keep_playing_when_closed",
                "Keep playing after closing the window",
                s.window.keep_playing_when_closed,
                cx,
            ))
            .when(!cfg!(target_os = "macos"), |group| {
                group.child(self.switch(
                    "window.show_in_tray",
                    "Show in the system tray",
                    s.window.show_in_tray,
                    cx,
                ))
            })
            .child(self.switch(
                "window.animated_cover_in_bar",
                "Animated cover in the player bar",
                s.window.animated_cover_in_bar,
                cx,
            ));

        let mut equalizer = settings_group("Equalizer")
            .child(
                row("Preset").child(
                    select("eq.preset", EqPreset::ALL.map(EqPreset::label))
                        .selected(preset)
                        .on_change(cx.listener(move |this, e: &SelectChanged, _, cx| {
                            let mut eq = eq;
                            eq.preset = EqPreset::ALL[e.index.min(8)];
                            this.error = write("playback.equalizer", eq, cx);
                            cx.notify();
                        })),
                ),
            )
            .footer("Boosts lower the overall level so loud passages don't distort.");
        for (band, hz) in EQ_BANDS_HZ.iter().enumerate() {
            let name = if *hz >= 1000. {
                format!("{} kHz", hz / 1000.)
            } else {
                format!("{hz} Hz")
            };
            equalizer = equalizer.child(
                row(format!("{name}: {:+.0} dB", shown[band])).child(
                    slider(
                        format!("eq.band{band}"),
                        (shown[band] + EQ_MAX_DB) / (2. * EQ_MAX_DB),
                    )
                    .label(name)
                    .w(px(200.))
                    .on_change(cx.listener(
                        move |this, e: &SliderChanged, _, cx| {
                            // Moving a band starts Custom from the gains shown.
                            let mut eq = eq;
                            eq.custom = shown;
                            eq.preset = EqPreset::Custom;
                            eq.custom[band] = (e.value * 2. * EQ_MAX_DB - EQ_MAX_DB).round();
                            this.error = write("playback.equalizer", eq, cx);
                            cx.notify();
                        },
                    )),
                ),
            );
        }

        let privacy = settings_group("Privacy").child(
            switch_row("privacy.pause_history", "Pause watch history")
                .checked(!s.privacy.report_history)
                .disabled(!editable("privacy.report_history", cx))
                .on_toggle(cx.listener(|this, e: &Toggled, _, cx| {
                    this.error = write("privacy.report_history", !e.checked, cx);
                    cx.notify();
                })),
        );

        let mut lastfm = self.scrobbler(
            ScrobbleService::LastFm,
            "Last.fm",
            Some(&scrobbling.lastfm)
                .filter(|a| a.username.is_some() || a.connecting || a.error.is_some()),
            cx,
        );
        if !scrobbling.lastfm_app && scrobbling.lastfm.username.is_none() {
            lastfm = lastfm
                .child(
                    row("API key").child(
                        text_field("lastfm.key")
                            .text(self.lastfm_key.clone())
                            .w(px(260.))
                            .on_change(cx.listener(|this, e: &TextChanged, _, cx| {
                                this.lastfm_key = e.text.clone();
                                cx.notify();
                            })),
                    ),
                )
                .child(
                    row("Shared secret").child(
                        secure_field("lastfm.secret")
                            .text(self.lastfm_secret.clone())
                            .w(px(260.))
                            .on_change(cx.listener(|this, e: &TextChanged, _, cx| {
                                this.lastfm_secret = e.text.clone();
                                cx.notify();
                            })),
                    ),
                )
                .child(
                    row("").child(
                        link("lastfm.create", "Create an API account")
                            .url("https://www.last.fm/api/account/create"),
                    ),
                );
        }
        let listenbrainz = self.scrobbler(
            ScrobbleService::ListenBrainz,
            "ListenBrainz",
            Some(&scrobbling.listenbrainz)
                .filter(|a| a.username.is_some() || a.connecting || a.error.is_some()),
            cx,
        );

        v_flex()
            .id("settings")
            .size_full()
            .overflow_y_scroll()
            .child(
                form()
                    .child(playback)
                    .child(equalizer)
                    .child(privacy)
                    .child(lastfm)
                    .child(listenbrainz)
                    .when(scrobbling.queued > 0, |f| {
                        f.child(settings_group("").child(row(format!(
                            "{} plays waiting to be sent",
                            scrobbling.queued
                        ))))
                    })
                    .when_some(self.error.clone(), |f, e| {
                        f.child(settings_group("").child(row(e)))
                    }),
            )
    }
}
