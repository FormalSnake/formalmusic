//! Durations, easing curves and the presence helpers.
//!
//! gpui-pre's `Animation` (`elements/animation.rs`) only ships linear, quadratic,
//! ease-in-out and a quint ease-out; it has no cubic-bezier curve, which the
//! easings here need. `cubic_bezier` below solves one, the same curve CSS
//! `cubic-bezier()` and gpuix's `MotionEase` describe.
//!
//! GPUI animations are one-shot and keyed by element id, so `Presence` bumps a
//! generation on every open/close flip and the helpers fold it into the id:
//! each flip replays from its start, and nothing animates while idle.

use std::time::Duration;

use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, Context, ElementId, IntoElement, SharedString,
};

/// Everything stays under 300ms: in a music player motion is feedback, never a show.
pub const DURATION_FAST: Duration = Duration::from_millis(120);
/// Enter transitions and small reveals.
pub const DURATION_BASE: Duration = Duration::from_millis(180);
/// The expanded player sliding up.
pub const DURATION_PANEL: Duration = Duration::from_millis(280);

/// A strong ease-out: fast start, long settle. Fades and small reveals use it.
pub const EASE_OUT: (f32, f32, f32, f32) = (0.23, 1.0, 0.32, 1.0);

/// Builds a `Fn(f32) -> f32` for `Animation::with_easing` from the four
/// control points of a `cubic-bezier(x1, y1, x2, y2)` curve. Solves for the
/// bezier parameter `t` at a given `x` by bisection (8 iterations is enough
/// for the sub-pixel precision an animation needs), then evaluates `y` at `t`.
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32) -> impl Fn(f32) -> f32 {
    move |x: f32| {
        let bezier = |t: f32, p1: f32, p2: f32| {
            let mt = 1.0 - t;
            3.0 * mt * mt * t * p1 + 3.0 * mt * t * t * p2 + t * t * t
        };

        let mut lo = 0.0f32;
        let mut hi = 1.0f32;
        let mut t = x;
        for _ in 0..8 {
            let guess = bezier(t, x1, x2);
            if guess < x {
                lo = t;
            } else {
                hi = t;
            }
            t = (lo + hi) / 2.0;
        }
        bezier(t, y1, y2)
    }
}

fn curve((x1, y1, x2, y2): (f32, f32, f32, f32)) -> impl Fn(f32) -> f32 {
    cubic_bezier(x1, y1, x2, y2)
}

/// `usePresence` plus `useHeld`: whether something is shown, whether it is
/// still mounted while it animates out, and the last value it showed, so
/// content on its way out keeps painting instead of blanking.
pub struct Presence<T> {
    held: Option<T>,
    open: bool,
    mounted: bool,
    generation: u64,
    exit: Duration,
}

impl<T: Clone + 'static> Presence<T> {
    pub fn new(exit: Duration) -> Self {
        Self {
            held: None,
            open: false,
            mounted: false,
            generation: 0,
            exit,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// What to paint: the live value while open, the last one while leaving.
    pub fn current(&self) -> Option<&T> {
        if self.mounted {
            self.held.as_ref()
        } else {
            None
        }
    }

    pub fn id(&self, name: &'static str) -> ElementId {
        ElementId::NamedInteger(SharedString::new_static(name), self.generation)
    }

    /// Feeds the value the owner wants shown (`None` to hide). `field` finds
    /// this presence again inside the owner when the exit timer fires.
    pub fn set<V: 'static>(
        &mut self,
        value: Option<T>,
        field: fn(&mut V) -> &mut Presence<T>,
        cx: &mut Context<V>,
    ) {
        let open = value.is_some();
        if let Some(value) = value {
            self.held = Some(value);
        }
        if open == self.open {
            return;
        }
        self.open = open;
        self.generation += 1;
        if open {
            self.mounted = true;
            return;
        }
        let generation = self.generation;
        let exit = self.exit;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(exit).await;
            let _ = this.update(cx, |view, cx| {
                let presence = field(view);
                if presence.generation == generation {
                    presence.mounted = false;
                    presence.held = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }
}

/// Anything else that moves between two states: `t` runs 0 to 1 towards
/// `open`, on EASE_OUT, `enter` long opening and `exit` long closing.
pub fn toward<E: IntoElement + 'static>(
    element: E,
    id: ElementId,
    open: bool,
    enter: Duration,
    exit: Duration,
    animator: impl Fn(E, f32) -> E + 'static,
) -> AnyElement {
    let duration = if open { enter } else { exit };
    element
        .with_animation(
            id,
            Animation::new(duration).with_easing(curve(EASE_OUT)),
            move |element, t| animator(element, if open { t } else { 1. - t }),
        )
        .into_any_element()
}

/// `FORMALMUSIC_STILL=1` (the screenshot run) jumps every animation to its end.
pub fn install(cx: &mut App) {
    if std::env::var("FORMALMUSIC_STILL").ok().as_deref() == Some("1") {
        cx.set_reduce_motion(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn bezier_hits_both_ends() {
        let ease = curve(EASE_OUT);
        assert!(ease(0.).abs() < 0.01);
        assert!((ease(1.) - 1.).abs() < 0.01);
        assert!(ease(0.5) > 0.5);
    }
}
