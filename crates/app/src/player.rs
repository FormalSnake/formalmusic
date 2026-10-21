//! The bar along the bottom: nd's `player_bar` over the daemon's playback, the rating and
//! track menu beside the playing song, repeat, shuffle, queue and expand at the trailing end. The bar extrapolates the position itself, so
//! a playing track costs one render per position report, not one per frame.

use std::time::Duration;

use formalmusic_api::{Item, Rating, Repeat, Status};
use formalmusic_core::{AppState, StoreEvent};
use nd::prelude::*;

use crate::art;
use crate::bridge;
use crate::menus::{self, MenuContext};
use crate::now_playing::Tab;

pub struct Player {
    /// The expanded player's tab while it is open, which the expand and queue buttons show.
    pub expanded: Option<Tab>,
    /// The song the last notification was for, so each song notifies once.
    announced: Option<String>,
}

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
        Self {
            expanded: None,
            announced: None,
        }
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

impl Player {
    /// A desktop notification for a new song while the window is in the background, when the
    /// setting asks for it.
    fn announce(&mut self, track: Option<&formalmusic_api::Track>, cx: &mut Context<Self>) {
        let id = track.map(|t| t.video_id.clone());
        if self.announced == id {
            return;
        }
        let first = self.announced.is_none();
        self.announced = id;
        let Some(track) = track else { return };
        if first
            || !crate::settings::current(cx).window.notify_song_change
            || nd::system::system().is_active()
        {
            return;
        }
        let notification = nd::system::Notification::new(track.title.clone())
            .body(formalmusic_core::format::byline(track))
            .silent();
        nd::system::spawn(
            nd::system::system().notifications().show(notification),
            |shown, _| {
                if let Err(error) = shown {
                    eprintln!("formalmusic: notification: {error}");
                }
            },
        );
    }
}

impl Render for Player {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = crate::store(cx);
        let state = store.state();
        let (clock, duration) = clock(&state);
        let track = state.player.track.clone();
        let has_track = track.is_some();
        self.announce(track.as_ref(), cx);
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
        let toggle = |on: Rating| if rating == on { Rating::Indifferent } else { on };
        let liked = rating == Rating::Like;
        let disliked = rating == Rating::Dislike;
        let like = icon_button("player.like", Icon::ThumbsUp)
            .tooltip(if liked { "Remove like" } else { "Like" })
            .checked(liked)
            .on_click(rate(toggle(Rating::Like)));
        let dislike = icon_button("player.dislike", Icon::ThumbsDown)
            .tooltip(if disliked { "Remove dislike" } else { "Dislike" })
            .checked(disliked)
            .on_click(rate(toggle(Rating::Dislike)));
        let more_track = track.clone();
        let more = menu_button("player.more", Icon::Ellipsis, move |menu, _, cx| match &more_track {
            Some(track) => menus::item_menu(menu, &Item::Track(track.clone()), &MenuContext::default(), cx),
            None => menu,
        })
        .tooltip("More actions");
        let repeat_button = icon_button(
            "player.repeat",
            if repeat == Repeat::One { Icon::Repeat1 } else { Icon::Repeat },
        )
        .tooltip(match repeat {
            Repeat::Off => "Repeat",
            Repeat::All => "Repeat one",
            Repeat::One => "Turn off repeat",
        })
        .checked(repeat != Repeat::Off)
        .on_click(|_, _, cx| crate::store(cx).cycle_repeat());
        let shuffle_button = icon_button("player.shuffle", Icon::Shuffle)
            .tooltip(if shuffle { "Turn off shuffle" } else { "Shuffle" })
            .checked(shuffle)
            .on_click(|_, _, cx| crate::store(cx).toggle_shuffle());
        let queue_open = self.expanded == Some(Tab::UpNext);
        let queue = icon_button("player.queue", Icon::ListMusic)
            .tooltip(if queue_open { "Hide queue" } else { "Show queue" })
            .checked(queue_open)
            .dispatch(crate::root::ToggleQueue);
        let expand = icon_button(
            "player.expand",
            if self.expanded.is_some() { Icon::ChevronDown } else { Icon::ChevronUp },
        )
        .tooltip(if self.expanded.is_some() { "Close player" } else { "Open player" })
        .on_click(|_, _, cx| crate::root::toggle_expanded(cx));
        let menu_track = track.clone();
        let layout = TransportLayout::platform();
        let bar = player_bar("player.bar", now)
            .layout(layout)
            .on_command(cx.listener(Self::command))
            .on_open(|_, cx| crate::root::toggle_expanded(cx));
        let bar = if layout == TransportLayout::Pill {
            // Music's mini player: shuffle and repeat around the transport, More (with rating)
            // at the end of the song, lyrics and the queue at the trailing end.
            let lyrics_open = self.expanded == Some(Tab::Lyrics);
            let lyrics = icon_button("player.lyrics", Icon::MicVocal)
                .tooltip(if lyrics_open { "Hide lyrics" } else { "Show lyrics" })
                .checked(lyrics_open)
                .dispatch(crate::root::ToggleLyrics);
            let control = |b: Button| b.text_size(px(PILL.control_glyph)).bezel(Bezel::Plain);
            bar.before_transport(shuffle_button.size(px(PILL.toggle)).into_any_element())
                .after_transport(repeat_button.size(px(PILL.toggle)).into_any_element())
                // Music's More is three 4 pt dots spanning about 16 pt: the ellipsis symbol at 30 pt
                // semibold.
                .accessory(
                    more.text_size(px(30.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .bezel(Bezel::Plain)
                        .into_any_element(),
                )
                .trailing(control(lyrics).into_any_element())
                .trailing(control(queue).into_any_element())
        } else {
            bar.accessory(like.into_any_element())
                .accessory(dislike.into_any_element())
                .accessory(more.into_any_element())
                .trailing(repeat_button.into_any_element())
                .trailing(shuffle_button.into_any_element())
                .trailing(queue.into_any_element())
                .trailing(expand.into_any_element())
        }
        .w_full();
        let docked = layout != TransportLayout::Pill;
        div()
            .id("player")
            .w_full()
            .when(docked, |bar| bar.px_4().pt_1().pb_2())
            .child(bar)
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
