//! Moves the seek bar and the time on between the daemon's position events,
//! which come about once a second. It ticks only while a track plays;
//! paused, nothing runs.

use std::time::Duration;

use formalmusic_api::Status;
use gpui_kit::*;

use crate::bridge::{Bridge, Topic};

/// Four repaints a second: a 600 px bar over a three minute song moves
/// under a pixel per tick.
const TICK: Duration = Duration::from_millis(250);

#[derive(Default)]
pub struct Clock {
    running: bool,
}

impl Global for Clock {}

impl Clock {
    /// Starts or stops ticking to match the player. Call on every
    /// `NowPlaying`.
    pub fn sync(cx: &mut App) {
        if !cx.has_global::<Clock>() {
            cx.set_global(Clock::default());
        }
        let playing = crate::bridge::store(cx)
            .is_some_and(|store| store.state().player.status == Status::Playing);
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
}
