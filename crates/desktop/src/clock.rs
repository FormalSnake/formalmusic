//! Moves the seek bar and the time on between the daemon's position events,
//! which come about once a second. It ticks only while a track plays;
//! paused, nothing runs.
//!
//! While a music video or an animated cover decodes, a tick waits for that
//! picture's next frame and is painted with it, instead of costing a frame
//! of its own between two of the picture's.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use formalmusic_core::model::Status;
use gpui_kit::*;

use crate::bridge::{Bridge, Topic};

/// Four repaints a second: a 600 px bar over a three minute song moves
/// under a pixel per tick.
const TICK: Duration = Duration::from_millis(250);

#[derive(Default)]
pub struct Clock {
    running: bool,
    /// Pictures decoding now, counted by their [`Surface`]s.
    surfaces: Rc<Cell<usize>>,
    /// A tick is waiting for the next picture frame.
    due: bool,
}

impl Global for Clock {}

/// Held by a view while its picture decodes.
pub struct Surface(Rc<Cell<usize>>);

impl Drop for Surface {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

impl Clock {
    pub fn surface(cx: &mut App) -> Surface {
        let count = cx.default_global::<Clock>().surfaces.clone();
        count.set(count.get() + 1);
        Surface(count)
    }

    /// A picture shows a new frame: a tick waiting for it goes out with it.
    pub fn frame(cx: &mut App) {
        let clock = cx.default_global::<Clock>();
        if std::mem::take(&mut clock.due) {
            Bridge::notify(cx, &Topic::Position);
        }
    }

    /// Starts or stops ticking to match the player. Call on every
    /// `NowPlaying`.
    pub fn sync(cx: &mut App) {
        cx.default_global::<Clock>();
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
                        // One tick waits for a picture frame at most; with
                        // none by the next, the picture has stalled.
                        let clock = cx.global_mut::<Clock>();
                        clock.due = clock.surfaces.get() > 0 && !clock.due;
                        if !clock.due {
                            Bridge::notify(cx, &Topic::Position);
                        }
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
