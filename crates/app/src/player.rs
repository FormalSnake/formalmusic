//! The bar along the bottom: nd's `player_bar` over the daemon's playback, with the rating,
//! repeat, shuffle and expand controls beside it. The bar extrapolates the position itself, so
//! a playing track costs one render per position report, not one per frame.

use std::time::Duration;

use formalmusic_api::{Item, Rating, Repeat, Status};
use formalmusic_core::{AppState, StoreEvent};
use nd::prelude::*;

use crate::art;
use crate::bridge;
use crate::menus::{self, MenuContext};

pub struct Player;

/// The clock and length nd's media widgets read, from the store's last report.
pub fn clock(state: &AppState) -> (PlaybackClock, Option<Duration>) {
    let playing = state.player.status == Status::Playing;
    let position = Duration::from_millis(state.position_ms);
    let at = state.position_at.unwrap_or_else(std::time::Instant::now);
    let clock = PlaybackClock {
        position,
        at,
        playing,
        rate: 1.,
    };
    (clock, state.player.duration_ms.map(Duration::from_millis))
}

impl Player {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let id = cx.entity_id();
        for topic in [
            StoreEvent::Player,
            StoreEvent::NowPlaying,
            StoreEvent::Position,
            StoreEvent::Ratings,
        ] {
            bridge::watch(cx, topic, id);
        }
        Self
    }

    fn command(
        &mut self,
        command: &TransportCommand,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = crate::store(cx);
        match *command {
            TransportCommand::TogglePlay => store.toggle(),
            TransportCommand::Previous => store.previous(),
            TransportCommand::Next => store.next(),
            TransportCommand::Seek(at) => store.seek(at.as_millis() as u64),
            TransportCommand::Scrub(_) => {}
            TransportCommand::SetVolume(v) => {
                store.set_volume(v);
                if store.state().player.muted {
                    store.set_muted(false);
                }
            }
            TransportCommand::ToggleMute => {
                let muted = store.state().player.muted;
                store.set_muted(!muted);
            }
        }
    }
}

impl Render for Player {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = crate::store(cx);
        let state = store.state();
        let (clock, duration) = clock(&state);
        let track = state.player.track.clone();
        let has_track = track.is_some();
        let mut now = NowPlaying::new(
            track
                .as_ref()
                .map_or("Not playing".to_owned(), |t| t.title.clone()),
        );
        if let Some(track) = &track {
            now = now.subtitle(formalmusic_core::format::byline(track));
            let motion = crate::settings::current(cx)
                .window
                .animated_cover_in_bar
                .then(|| crate::video::cover_frame(cx.entity_id(), cx))
                .flatten();
            if let Some(source) = motion.or_else(|| art::source(&track.thumbnails, 44.)) {
                now = now.artwork(source);
            }
        }
        now.clock = clock;
        now.duration = duration;
        now.buffered = (state.buffered_ms > 0).then(|| Duration::from_millis(state.buffered_ms));
        now = now.volume(state.player.volume, state.player.muted);
        now.can_previous = has_track;
        now.can_next = has_track;
        let rating = track
            .as_ref()
            .map_or(Rating::Indifferent, |t| state.rating(t));
        let (repeat, shuffle) = (state.player.repeat, state.player.shuffle);
        drop(state);

        let rate = |next: Rating| {
            let track = track.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                if let Some(track) = &track {
                    crate::store(cx).rate(track, next);
                }
            }
        };
        let extras = h_flex()
            .id("player.extras")
            .gap_1()
            .flex_none()
            .child(
                button(
                    "player.like",
                    if rating == Rating::Like {
                        "Liked"
                    } else {
                        "Like"
                    },
                )
                .borderless()
                .disabled(!has_track)
                .on_click(rate(if rating == Rating::Like {
                    Rating::Indifferent
                } else {
                    Rating::Like
                })),
            )
            .child(
                button(
                    "player.dislike",
                    if rating == Rating::Dislike {
                        "Disliked"
                    } else {
                        "Dislike"
                    },
                )
                .borderless()
                .disabled(!has_track)
                .on_click(rate(if rating == Rating::Dislike {
                    Rating::Indifferent
                } else {
                    Rating::Dislike
                })),
            )
            .child(
                button(
                    "player.repeat",
                    match repeat {
                        Repeat::Off => "Repeat off",
                        Repeat::All => "Repeat all",
                        Repeat::One => "Repeat one",
                    },
                )
                .borderless()
                .on_click(|_, _, cx| crate::store(cx).cycle_repeat()),
            )
            .child(
                button(
                    "player.shuffle",
                    if shuffle { "Shuffle on" } else { "Shuffle off" },
                )
                .borderless()
                .on_click(|_, _, cx| crate::store(cx).toggle_shuffle()),
            )
            .child(
                icon_button("player.expand", Icon::PanelRight)
                    .tooltip("Open player")
                    .disabled(!has_track)
                    .on_click(|_, _, cx| crate::root::toggle_expanded(cx)),
            );
        let menu_track = track.clone();
        h_flex()
            .id("player")
            .w_full()
            .px_4()
            .py_2()
            .gap_4()
            .child(
                player_bar("player.bar", now)
                    .on_command(cx.listener(Self::command))
                    .flex_1()
                    .min_w_0(),
            )
            .child(extras)
            .when_some(menu_track, |bar, track| {
                bar.context_menu(move |menu, _, cx| {
                    menus::item_menu(
                        menu,
                        &Item::Track(track.clone()),
                        &MenuContext::default(),
                        cx,
                    )
                })
            })
    }
}
