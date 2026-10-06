//! Moves the seek bar, the time and the lyric line on between the daemon's
//! position events, which come about once a second. It ticks only while a
//! track plays; paused, nothing runs.

use std::time::Duration;

use formalmusic_api::Status;
use formalmusic_core::store::line_at;
use gpui_kit::*;

use crate::bridge::{Bridge, Topic};

/// Four repaints a second: a 600 px bar over a three minute song moves
/// under a pixel per tick.
const TICK: Duration = Duration::from_millis(250);

#[derive(Default)]
pub struct Clock {
    running: bool,
    /// The lyric line at the last tick, so `LyricLine` goes out only when it changes.
    line: Option<usize>,
}

impl Global for Clock {}

impl Clock {
    pub fn line(cx: &App) -> Option<usize> {
        cx.try_global::<Clock>().and_then(|clock| clock.line)
    }

    /// Starts or stops ticking to match the player. Call on every
    /// `NowPlaying`.
    pub fn sync(cx: &mut App) {
        if !cx.has_global::<Clock>() {
            cx.set_global(Clock::default());
        }
        let playing = crate::bridge::store(cx)
            .is_some_and(|store| store.state().player.status == Status::Playing);
        Clock::update_line(cx);
        if !playing || cx.global::<Clock>().running {
            return;
        }
        cx.global_mut::<Clock>().running = true;
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let playing = cx.update(|cx| {
                    let playing = crate::bridge::store(cx)
                        .is_some_and(|store| store.state().player.status == Status::Playing);
                    if playing {
                        Bridge::notify(cx, &Topic::Position);
                        Clock::update_line(cx);
                    } else {
                        cx.global_mut::<Clock>().running = false;
                    }
                    playing
                });
                if !playing {
                    break;
                }
            }
        })
        .detach();
    }

    /// Works the current line out from the interpolated position.
    pub fn update_line(cx: &mut App) {
        let Some(store) = crate::bridge::store(cx) else {
            return;
        };
        let line = {
            let state = store.state();
            state
                .current_video()
                .and_then(|id| state.lyrics.get(id))
                .and_then(|entry| entry.lyrics.as_ref())
                .and_then(|lyrics| line_at(lyrics, state.position_now()))
        };
        if !cx.has_global::<Clock>() {
            cx.set_global(Clock::default());
        }
        if cx.global::<Clock>().line != line {
            cx.global_mut::<Clock>().line = line;
            Bridge::notify(cx, &Topic::LyricLine);
        }
    }
}
