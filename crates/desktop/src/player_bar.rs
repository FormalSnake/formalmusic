//! The bar along the bottom: transport, the seek bar, the current track and
//! its rating, volume, repeat, shuffle and the button that opens the
//! expanded player. The seek bar and the time are entities of their own
//! watching `Position`, so the four ticks a second repaint them and not the
//! rest of the bar.

use std::cell::Cell;
use std::rc::Rc;

use formalmusic_api::{Repeat, Status};
use formalmusic_core::MusicStore;
use formalmusic_core::format::duration;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions::{self, MenuContext};
use crate::art;
use crate::bridge::{Bridge, Topic};
use crate::header::{link_text, rating_buttons};
use crate::icons::{Icon, IconName};
use crate::primitives::IconButton;
use crate::theme::{PLAYER_HEIGHT, Palette, Theme, radius, spacing, tabular, type_scale};

/// Fraction of `bounds` the pointer is at, clamped to the bar.
fn fraction(bounds: Bounds<Pixels>, x: Pixels) -> f32 {
    let width = f32::from(bounds.size.width).max(1.);
    (f32::from(x - bounds.origin.x) / width).clamp(0., 1.)
}

/// A horizontal bar dragged with the pointer. While a drag is on, the move
/// and release listeners are window-wide, so the drag keeps going when the
/// pointer leaves the bar.
fn drag_listeners<V: 'static>(
    bounds: Rc<Cell<Bounds<Pixels>>>,
    dragging: bool,
    entity: WeakEntity<V>,
    on_move: fn(&mut V, f32, &mut Context<V>),
    on_release: fn(&mut V, f32, &mut Context<V>),
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
                    let at = fraction(move_bounds.get(), event.position.x);
                    let _ = moving.update(cx, |view, cx| on_move(view, at, cx));
                }
            });
            window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                if phase == DispatchPhase::Bubble && event.button == MouseButton::Left {
                    let at = fraction(release_bounds.get(), event.position.x);
                    let _ = releasing.update(cx, |view, cx| on_release(view, at, cx));
                }
            });
        },
    )
    .absolute()
    .inset_0()
}

pub struct SeekBar {
    store: MusicStore,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    /// Where a drag is, as a fraction; the position shown until release.
    drag: Option<f32>,
}

impl SeekBar {
    pub fn new(store: MusicStore, cx: &mut Context<Self>) -> Self {
        let weak = cx.entity().downgrade();
        Bridge::watch(cx, Topic::Position, weak.clone().into());
        Bridge::watch(cx, Topic::Player, weak.into());
        Self {
            store,
            bounds: Rc::default(),
            drag: None,
        }
    }

    fn moved(&mut self, at: f32, cx: &mut Context<Self>) {
        self.drag = Some(at);
        cx.notify();
    }

    fn released(&mut self, at: f32, cx: &mut Context<Self>) {
        self.drag = None;
        let total = self.store.state().player.duration_ms.unwrap_or(0);
        if total > 0 {
            self.store.seek((total as f32 * at) as u64);
        }
        cx.notify();
    }
}

impl Render for SeekBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace::render("SeekBar");
        let palette = Theme::get(cx);
        let (position, buffered, total, live) = {
            let state = self.store.state();
            (
                state.position_now(),
                state.buffered_ms,
                state.player.duration_ms.unwrap_or(0),
                state.player.track.is_some(),
            )
        };
        let played = self
            .drag
            .unwrap_or_else(|| {
                if total > 0 {
                    position as f32 / total as f32
                } else {
                    0.
                }
            })
            .clamp(0., 1.);
        let loaded = if total > 0 {
            (buffered as f32 / total as f32).clamp(played, 1.)
        } else {
            0.
        };
        let dragging = self.drag.is_some();
        let entity = cx.entity().downgrade();
        div()
            .id("seek-bar")
            .group("seek")
            .relative()
            .w_full()
            .h(px(14.))
            .flex()
            .items_center()
            .when(live, |el| el.cursor_pointer())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    if live {
                        cx.stop_propagation();
                        this.moved(fraction(this.bounds.get(), event.position.x), cx);
                    }
                }),
            )
            .child(drag_listeners(
                self.bounds.clone(),
                dragging,
                entity,
                Self::moved,
                Self::released,
            ))
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(3.))
                    .when(dragging, |el| el.h(px(5.)))
                    .group_hover("seek", |style| style.h(px(5.)))
                    .bg(palette.progress_track)
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .w(relative(loaded))
                            .bg(palette.progress_track),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left_0()
                            .w(relative(played))
                            .bg(palette.progress),
                    ),
            )
            .when(live, |el| {
                el.child(
                    div()
                        .absolute()
                        .left(relative(played))
                        .ml(px(-6.))
                        .size(px(12.))
                        .rounded(px(6.))
                        .bg(palette.progress)
                        .opacity(if dragging { 1. } else { 0. })
                        .group_hover("seek", |style| style.opacity(1.)),
                )
            })
    }
}

/// "1:03 / 3:12", in tabular figures so the digits hold still.
pub struct TimeLabel {
    store: MusicStore,
}

impl TimeLabel {
    pub fn new(store: MusicStore, cx: &mut Context<Self>) -> Self {
        let weak = cx.entity().downgrade();
        Bridge::watch(cx, Topic::Position, weak.clone().into());
        Bridge::watch(cx, Topic::Player, weak.into());
        Self { store }
    }
}

impl Render for TimeLabel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::get(cx);
        let state = self.store.state();
        let text = match (&state.player.track, state.player.duration_ms) {
            (Some(_), Some(total)) => {
                format!("{} / {}", duration(state.position_now()), duration(total))
            }
            (Some(_), None) => duration(state.position_now()),
            (None, _) => String::new(),
        };
        div()
            .text_size(type_scale::CAPTION.font_size)
            .text_color(palette.secondary)
            .font_features(tabular())
            .whitespace_nowrap()
            .child(text)
    }
}

pub struct PlayerBar {
    store: MusicStore,
    seek: Entity<SeekBar>,
    time: Entity<TimeLabel>,
    volume_bounds: Rc<Cell<Bounds<Pixels>>>,
    volume_drag: bool,
    expanded: bool,
}

impl PlayerBar {
    pub fn new(store: MusicStore, cx: &mut Context<Self>) -> Self {
        let weak = cx.entity().downgrade();
        Bridge::watch(cx, Topic::Player, weak.into());
        let seek = cx.new(|cx| SeekBar::new(store.clone(), cx));
        let time = cx.new(|cx| TimeLabel::new(store.clone(), cx));
        Self {
            store,
            seek,
            time,
            volume_bounds: Rc::default(),
            volume_drag: false,
            expanded: false,
        }
    }

    pub fn set_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        if self.expanded != expanded {
            self.expanded = expanded;
            cx.notify();
        }
    }

    fn volume_moved(&mut self, at: f32, _cx: &mut Context<Self>) {
        self.volume_drag = true;
        self.store.set_volume(at);
    }

    fn volume_released(&mut self, at: f32, cx: &mut Context<Self>) {
        self.volume_drag = false;
        self.store.set_volume(at);
        cx.notify();
    }

    fn volume(
        &self,
        palette: Palette,
        volume: f32,
        muted: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let shown = if muted { 0. } else { volume };
        let icon = if muted || volume == 0. {
            IconName::Muted
        } else if volume < 0.5 {
            IconName::VolumeLow
        } else {
            IconName::Volume
        };
        let store = self.store.clone();
        let entity = cx.entity().downgrade();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(spacing::X1)
            .child(
                IconButton::new("mute", icon, if muted { "Unmute (m)" } else { "Mute (m)" })
                    .on_click(move |_, _, _| store.set_muted(!muted)),
            )
            .child(
                div()
                    .id("volume")
                    .group("volume")
                    .relative()
                    .w(px(96.))
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let at = fraction(this.volume_bounds.get(), event.position.x);
                            this.volume_moved(at, cx);
                            cx.notify();
                        }),
                    )
                    .child(drag_listeners(
                        self.volume_bounds.clone(),
                        self.volume_drag,
                        entity,
                        Self::volume_moved,
                        Self::volume_released,
                    ))
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .h(px(3.))
                            .rounded(px(2.))
                            .bg(palette.progress_track)
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left_0()
                                    .rounded(px(2.))
                                    .w(relative(shown))
                                    .bg(palette.text),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(relative(shown))
                            .ml(px(-6.))
                            .size(px(12.))
                            .rounded(px(6.))
                            .bg(palette.text)
                            .opacity(if self.volume_drag { 1. } else { 0. })
                            .group_hover("volume", |style| style.opacity(1.)),
                    ),
            )
    }
}

impl Render for PlayerBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace::render("PlayerBar");
        let palette = Theme::get(cx);
        let (track, status, volume, muted, repeat, shuffle) = {
            let state = self.store.state();
            let player = &state.player;
            (
                player.track.clone(),
                player.status,
                player.volume,
                player.muted,
                player.repeat,
                player.shuffle,
            )
        };
        let rating = track
            .as_ref()
            .map(|track| self.store.state().rating(track))
            .unwrap_or_default();
        let playing = matches!(status, Status::Playing | Status::Loading);
        let has_track = track.is_some();
        let (toggle, next, previous) = (self.store.clone(), self.store.clone(), self.store.clone());
        let (repeat_store, shuffle_store) = (self.store.clone(), self.store.clone());

        let transport = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(spacing::X2)
            .w(px(300.))
            .flex_shrink_0()
            .child(
                IconButton::new("previous", IconName::Previous, "Previous (p)")
                    .size(px(20.))
                    .hit(px(36.))
                    .color(palette.text)
                    .disabled(!has_track)
                    .filled(true)
                    .on_click(move |_, _, _| previous.previous()),
            )
            .child(
                div()
                    .id("play-pause")
                    .size(px(44.))
                    .rounded(px(22.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .occlude()
                    .cursor_pointer()
                    .hover(move |style| style.bg(palette.hover_wash))
                    .active(move |style| style.bg(palette.press_wash))
                    .tab_index(0)
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(if playing {
                            "Pause (space)"
                        } else {
                            "Play (space)"
                        })
                        .build(window, cx)
                    })
                    .on_click(move |_, _, _| toggle.toggle())
                    .child(
                        Icon::new(if playing {
                            IconName::Pause
                        } else {
                            IconName::Play
                        })
                        .size(px(28.))
                        .color(palette.text)
                        .filled(true),
                    ),
            )
            .child(
                IconButton::new("next", IconName::Next, "Next (n)")
                    .size(px(20.))
                    .hit(px(36.))
                    .color(palette.text)
                    .disabled(!has_track)
                    .filled(true)
                    .on_click(move |_, _, _| next.next()),
            )
            .child(div().pl(spacing::X2).child(self.time.clone()));

        let now = track.as_ref().map(|track| {
            let (rate_store, rate_track) = (self.store.clone(), track.clone());
            let (menu_store, menu_track) = (self.store.clone(), track.clone());
            let mut byline = div()
                .flex()
                .flex_row()
                .min_w(px(0.))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(type_scale::CAPTION.font_size)
                .line_height(type_scale::CAPTION.line_height)
                .text_color(palette.secondary);
            for (n, artist) in track.artists.iter().enumerate() {
                if n > 0 {
                    byline = byline.child(div().child(", "));
                }
                byline = byline.child(link_text(artist, ("bar-artist", n), palette.text));
            }
            if let Some(album) = &track.album {
                byline = byline
                    .child(div().px(spacing::X1).child("\u{2022}"))
                    .child(link_text(album, "bar-album", palette.text));
            }
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(spacing::X3)
                .min_w(px(0.))
                .child(art::cover(
                    &track.thumbnails,
                    px(48.),
                    radius::ART_SMALL,
                    false,
                    &palette,
                ))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w(px(0.))
                        .child(
                            div()
                                .text_size(type_scale::BODY.font_size)
                                .line_height(type_scale::BODY.line_height)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(palette.text)
                                .truncate()
                                .child(track.title.clone()),
                        )
                        .child(byline),
                )
                .child(div().flex_shrink_0().occlude().child(rating_buttons(
                    "bar",
                    rating,
                    palette,
                    move |rating, _| rate_store.rate(&rate_track, rating),
                )))
                .child(
                    IconButton::new("bar-more", IconName::More, "More actions").on_click(
                        move |event, window, cx| {
                            actions::open_menu(
                                event.position(),
                                actions::track_menu(
                                    &menu_track,
                                    &MenuContext::default(),
                                    &menu_store,
                                ),
                                window,
                                cx,
                            );
                        },
                    ),
                )
        });

        let expanded = self.expanded;
        let controls = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .gap(spacing::X1)
            .w(px(300.))
            .flex_shrink_0()
            .child(self.volume(palette, volume, muted, cx))
            .child(
                IconButton::new(
                    "repeat",
                    if repeat == Repeat::One {
                        IconName::RepeatOne
                    } else {
                        IconName::Repeat
                    },
                    match repeat {
                        Repeat::Off => "Repeat all (r)",
                        Repeat::All => "Repeat one (r)",
                        Repeat::One => "Repeat off (r)",
                    },
                )
                .color(if repeat == Repeat::Off {
                    palette.secondary
                } else {
                    palette.text
                })
                .strong(repeat != Repeat::Off)
                .on_click(move |_, _, _| repeat_store.cycle_repeat()),
            )
            .child(
                IconButton::new(
                    "shuffle",
                    IconName::Shuffle,
                    if shuffle {
                        "Shuffle off (s)"
                    } else {
                        "Shuffle on (s)"
                    },
                )
                .color(if shuffle {
                    palette.text
                } else {
                    palette.secondary
                })
                .strong(shuffle)
                .on_click(move |_, _, _| shuffle_store.toggle_shuffle()),
            )
            .child(
                IconButton::new(
                    "expand",
                    if expanded {
                        IconName::Collapse
                    } else {
                        IconName::Expand
                    },
                    if expanded {
                        "Close player (Esc)"
                    } else {
                        "Open player"
                    },
                )
                .color(palette.text)
                .disabled(!has_track)
                .on_click(|_, window, cx| crate::app::toggle_expanded(window, cx)),
            );

        div()
            .id("player-bar")
            .relative()
            .h(PLAYER_HEIGHT)
            .w_full()
            .flex_shrink_0()
            .bg(palette.sidebar)
            .border_t_1()
            .border_color(palette.sidebar_border)
            .when(has_track, |el| {
                el.cursor_pointer()
                    .on_click(|_, window, cx| crate::app::toggle_expanded(window, cx))
            })
            .child(
                div()
                    .absolute()
                    .top(px(-7.))
                    .left_0()
                    .right_0()
                    .occlude()
                    .child(self.seek.clone()),
            )
            .child(
                div()
                    .size_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(spacing::X4)
                    .px(spacing::X4)
                    .child(transport)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_center()
                            .flex_grow(1.)
                            .min_w(px(0.))
                            .children(now),
                    )
                    .child(controls),
            )
    }
}
