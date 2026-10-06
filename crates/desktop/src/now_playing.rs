//! The expanded player: the cover large on the left over a wash of its own
//! colours, and Up next, Lyrics and Related on the right. The queue reorders
//! by dragging; the lyrics pane lives in `lyrics.rs`.

use std::rc::Rc;

use formalmusic_api::{Item, Status, Track};
use formalmusic_core::MusicStore;
use formalmusic_core::format::{duration, names};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions::{self, MenuContext};
use crate::art;
use crate::bridge::{Bridge, Topic};
use crate::cover_video::CoverVideo;
use crate::icons::{Icon, IconName};
use crate::lyrics::LyricsView;
use crate::primitives::IconButton;
use crate::shelves::{self, Env};
use crate::theme::{Palette, Theme, radius, spacing, tabular, type_scale, with_alpha};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    UpNext,
    Lyrics,
    Related,
}

const PANEL_WIDTH: Pixels = px(440.);
/// The animated cover is drawn up to 640 px wide; past 24 fps the eye gains
/// nothing and the CPU pays for every frame.
const COVER_FPS: f64 = 24.;
const QUEUE_ROW: Pixels = px(56.);

pub struct NowPlaying {
    store: MusicStore,
    cover: Entity<CoverVideo>,
    tab: Tab,
    queue: Entity<QueueView>,
    lyrics: Entity<LyricsView>,
    related_scroll: ScrollHandle,
    related_shelves: std::collections::HashMap<usize, ScrollHandle>,
    watching_related: Option<String>,
}

impl NowPlaying {
    pub fn new(store: MusicStore, tab: Tab, cx: &mut Context<Self>) -> Self {
        let weak = cx.entity().downgrade();
        Bridge::watch(cx, Topic::NowPlaying, weak.clone().into());
        let cover =
            cx.new(|cx| CoverVideo::new(store.clone(), px(400.), radius::CARD, COVER_FPS, cx));
        Bridge::watch(cx, Topic::Player, weak.into());
        let queue = cx.new(|cx| QueueView::new(store.clone(), cx));
        let lyrics = cx.new(|cx| LyricsView::new(store.clone(), cx));
        let mut this = Self {
            store,
            cover,
            tab,
            queue,
            lyrics,
            related_scroll: ScrollHandle::new(),
            related_shelves: Default::default(),
            watching_related: None,
        };
        this.load_tab(cx);
        this
    }

    pub fn tab(&self) -> Tab {
        self.tab
    }

    pub fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if tab == Tab::Lyrics && self.tab != Tab::Lyrics {
            self.lyrics.update(cx, |lyrics, cx| lyrics.resume(cx));
        }
        self.tab = tab;
        self.load_tab(cx);
        cx.notify();
    }

    /// Asks for what the open tab shows of the current track, once.
    fn load_tab(&mut self, cx: &mut Context<Self>) {
        let (video_id, related) = {
            let state = self.store.state();
            (
                state.current_video().map(str::to_owned),
                state.player.related_browse_id.clone(),
            )
        };
        match self.tab {
            Tab::Lyrics => {
                if let Some(video_id) = video_id {
                    self.store.load_lyrics(video_id);
                }
            }
            Tab::Related => {
                if let Some(browse_id) = related {
                    if self.watching_related.as_ref() != Some(&browse_id) {
                        let weak = cx.entity().downgrade();
                        if let Some(old) = self.watching_related.take() {
                            Bridge::unwatch(cx, &Topic::Related(old), &weak.clone().into());
                        }
                        Bridge::watch(cx, Topic::Related(browse_id.clone()), weak.into());
                        self.watching_related = Some(browse_id.clone());
                    }
                    self.store.load_related(browse_id);
                }
            }
            Tab::UpNext => {}
        }
    }

    fn related(&mut self, palette: Palette, cx: &mut Context<Self>) -> AnyElement {
        let page = self.watching_related.as_ref().and_then(|id| {
            self.store
                .state()
                .related
                .get(id)
                .and_then(|entry| entry.page.clone())
        });
        let Some(page) = page else {
            return placeholder("Loading related music\u{2026}", palette);
        };
        let playing = {
            let state = self.store.state();
            state.player.track.as_ref().map(|track| {
                (
                    track.video_id.clone(),
                    state.player.status == Status::Playing,
                )
            })
        };
        let env = Env {
            palette,
            store: self.store.clone(),
            playing,
            menu: MenuContext::default(),
        };
        let noop: Rc<dyn Fn(&mut App)> = {
            let weak = cx.entity().downgrade();
            Rc::new(move |cx: &mut App| {
                let _ = weak.update(cx, |_, cx| cx.notify());
            })
        };
        div()
            .id("related")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.related_scroll)
            .children(page.sections.iter().enumerate().map(|(n, section)| {
                let scroll = self.related_shelves.entry(n).or_default().clone();
                let body = match section.layout {
                    formalmusic_api::SectionLayout::TrackGrid
                    | formalmusic_api::SectionLayout::List => div()
                        .flex()
                        .flex_col()
                        .px(spacing::X2)
                        .children(section.items.iter().enumerate().filter_map(|(row, item)| {
                            let Item::Track(track) = item else {
                                return None;
                            };
                            let (store, video_id) = (self.store.clone(), track.video_id.clone());
                            Some(shelves::compact_track(
                                track,
                                ElementId::NamedInteger(format!("related-{n}").into(), row as u64),
                                &env,
                                Rc::new(move |_| {
                                    store.play(
                                        formalmusic_api::PlaySource::Radio {
                                            video_id: video_id.clone(),
                                        },
                                        0,
                                        false,
                                        true,
                                    )
                                }),
                            ))
                        }))
                        .into_any_element(),
                    _ => shelves::carousel(section, &scroll, &env, 100 + n),
                };
                div()
                    .flex()
                    .flex_col()
                    .child(shelves::shelf_title(
                        section,
                        None,
                        &env,
                        100 + n,
                        noop.clone(),
                    ))
                    .child(body)
            }))
            .into_any_element()
    }
}

pub(crate) fn placeholder(text: &'static str, palette: Palette) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(type_scale::BODY.font_size)
        .text_color(palette.secondary)
        .child(text)
        .into_any_element()
}

impl Render for NowPlaying {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace::render("NowPlaying");
        let palette = Theme::get(cx);
        let track = self.store.state().player.track.clone();
        if self.tab != Tab::UpNext {
            self.load_tab(cx);
        }
        let viewport = window.viewport_size();
        let art_size = (viewport.height - crate::theme::PLAYER_HEIGHT - px(160.))
            .min(viewport.width - PANEL_WIDTH - px(160.))
            .clamp(px(200.), px(640.));
        self.cover
            .update(cx, |cover, cx| cover.set_size(art_size, cx));
        let tabs = [
            (Tab::UpNext, "Up next"),
            (Tab::Lyrics, "Lyrics"),
            (Tab::Related, "Related"),
        ];
        let tab_row = div()
            .flex()
            .flex_row()
            .border_b_1()
            .border_color(with_alpha(palette.text, 0x26))
            .children(tabs.into_iter().map(|(tab, label)| {
                let active = self.tab == tab;
                div()
                    .id(label)
                    .flex_grow(1.)
                    .h(px(44.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .border_b_2()
                    .border_color(if active {
                        palette.text
                    } else {
                        palette.transparent
                    })
                    .text_size(type_scale::BODY.font_size)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if active {
                        palette.text
                    } else {
                        palette.secondary
                    })
                    .hover(move |style| style.text_color(palette.text))
                    .on_click(cx.listener(move |this, _, _, cx| this.set_tab(tab, cx)))
                    .child(label)
            }));
        let body = match self.tab {
            // Cached: the animated cover repaints this view up to 24 times
            // a second, and the queue has nothing new to draw for it.
            Tab::UpNext => AnyView::from(self.queue.clone())
                .cached(StyleRefinement::default().size_full())
                .into_any_element(),
            Tab::Lyrics => self.lyrics.clone().into_any_element(),
            Tab::Related => self.related(palette, cx),
        };
        let backdrop = track
            .as_ref()
            .and_then(|track| art::backdrop(&track.thumbnails));
        let caption = track.as_ref().map(|track| {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(spacing::X1)
                .pt(spacing::X6)
                .max_w(art_size)
                .child(
                    div()
                        .text_size(type_scale::LARGE.font_size)
                        .line_height(type_scale::LARGE.line_height)
                        .font_weight(FontWeight::BOLD)
                        .text_color(palette.text)
                        .text_align(TextAlign::Center)
                        .line_clamp(2)
                        .child(track.title.clone()),
                )
                .child(
                    div()
                        .text_size(type_scale::BODY.font_size)
                        .text_color(palette.secondary)
                        .text_align(TextAlign::Center)
                        .truncate()
                        .child(match &track.album {
                            Some(album) => {
                                format!("{} \u{2022} {}", names(&track.artists), album.text)
                            }
                            None => names(&track.artists),
                        }),
                )
        });
        div()
            .id("now-playing")
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(palette.canvas)
            .occlude()
            .when_some(backdrop, |el, source| {
                el.child(
                    img(source)
                        .absolute()
                        .inset_0()
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                )
            })
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(with_alpha(palette.canvas, 0xc8)),
            )
            .child(
                div()
                    .relative()
                    .size_full()
                    .flex()
                    .flex_row()
                    .gap(spacing::X10)
                    .px(spacing::X10)
                    .pt(px(64.))
                    .pb(spacing::X8)
                    .child(
                        div()
                            .flex_grow(1.)
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .when(track.is_some(), |el| {
                                el.child(
                                    div()
                                        .rounded(radius::CARD)
                                        .shadow(crate::primitives::overlay_shadows(&palette))
                                        .child(self.cover.clone()),
                                )
                            })
                            .children(caption),
                    )
                    .child(
                        div()
                            .w(PANEL_WIDTH)
                            .flex_shrink_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .child(tab_row)
                            .child(
                                div()
                                    .flex_grow(1.)
                                    .min_h(px(0.))
                                    .pt(spacing::X2)
                                    .child(body),
                            ),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top(spacing::X4)
                    .left(spacing::X4)
                    .occlude()
                    .child(
                        IconButton::new(
                            "collapse-player",
                            IconName::Collapse,
                            "Close player (Esc)",
                        )
                        .hit(px(36.))
                        .size(px(20.))
                        .color(palette.text)
                        .on_click(|_, window, cx| crate::app::toggle_expanded(window, cx)),
                    ),
            )
    }
}

/// The value a queue row carries while it is dragged.
#[derive(Clone)]
struct QueueDrag {
    from: usize,
    title: SharedString,
}

/// What follows the pointer during a drag: the row's title in a pill.
struct DragGhost {
    title: SharedString,
}

impl Render for DragGhost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = Theme::get(cx);
        div()
            .px(spacing::X3)
            .py(spacing::X2)
            .rounded(radius::ROW)
            .bg(palette.overlay)
            .border_1()
            .border_color(palette.overlay_border)
            .shadow(crate::primitives::overlay_shadows(&palette))
            .text_size(type_scale::BODY.font_size)
            .text_color(palette.text)
            .child(self.title.clone())
    }
}

pub struct QueueView {
    store: MusicStore,
    scroll: UniformListScrollHandle,
    revealed: Option<usize>,
}

impl QueueView {
    fn new(store: MusicStore, cx: &mut Context<Self>) -> Self {
        let weak = cx.entity().downgrade();
        Bridge::watch(cx, Topic::Queue, weak.clone().into());
        Bridge::watch(cx, Topic::NowPlaying, weak.into());
        Self {
            store,
            scroll: UniformListScrollHandle::new(),
            revealed: None,
        }
    }
}

impl Render for QueueView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace::render("QueueView");
        let palette = Theme::get(cx);
        let (queue, playing) = {
            let state = self.store.state();
            (state.queue.clone(), state.player.status == Status::Playing)
        };
        if queue.tracks.is_empty() {
            return placeholder("Nothing in the queue. Play something to fill it.", palette);
        }
        // The current row scrolls into view when it changes, not on every repaint.
        if queue.current != self.revealed {
            self.revealed = queue.current;
            if let Some(current) = queue.current {
                self.scroll.scroll_to_item(current, ScrollStrategy::Top);
            }
        }
        let store = self.store.clone();
        let caption = queue.source_title.clone().map(|title| {
            div()
                .px(spacing::X3)
                .pb(spacing::X2)
                .text_size(type_scale::CAPTION.font_size)
                .text_color(palette.secondary)
                .child(if queue.radio {
                    format!("Playing from {title}, with radio after it")
                } else {
                    format!("Playing from {title}")
                })
        });
        let entity = cx.entity().downgrade();
        let list = uniform_list("queue", queue.tracks.len(), move |range, _window, _cx| {
            range
                .map(|index| {
                    let track: &Track = &queue.tracks[index];
                    let current = queue.current == Some(index);
                    let group: SharedString = format!("queue-{index}").into();
                    let (jump, remove, drop_store) = (store.clone(), store.clone(), store.clone());
                    let (menu_store, menu_track) = (store.clone(), track.clone());
                    let _ = &entity;
                    div()
                        .id(("queue-row", index))
                        .group(group.clone())
                        .w_full()
                        .h(QUEUE_ROW)
                        .px(spacing::X2)
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(spacing::X3)
                        .rounded(radius::ROW)
                        .cursor_pointer()
                        .when(current, |el| el.bg(with_alpha(palette.text, 0x1a)))
                        .hover(move |style| style.bg(palette.hover_wash))
                        .on_click(move |_, _, _| jump.jump_to(index))
                        .on_mouse_up(MouseButton::Right, move |event, window, cx| {
                            actions::open_menu(
                                event.position,
                                actions::track_menu(
                                    &menu_track,
                                    &MenuContext::default(),
                                    &menu_store,
                                ),
                                window,
                                cx,
                            );
                        })
                        .on_drag(
                            QueueDrag {
                                from: index,
                                title: track.title.clone().into(),
                            },
                            |drag, _, _, cx| {
                                cx.new(|_| DragGhost {
                                    title: drag.title.clone(),
                                })
                            },
                        )
                        .drag_over::<QueueDrag>(move |style, _, _, _| {
                            style.border_t_2().border_color(palette.accent)
                        })
                        .on_drop(move |drag: &QueueDrag, _, _| {
                            drop_store.move_in_queue(drag.from, index)
                        })
                        .child(
                            Icon::new(IconName::Grip)
                                .size(px(14.))
                                .color(palette.tertiary),
                        )
                        .child(
                            div()
                                .relative()
                                .child(art::cover(
                                    &track.thumbnails,
                                    px(40.),
                                    radius::ART_SMALL,
                                    false,
                                    &palette,
                                ))
                                .when(current, |el| {
                                    el.child(
                                        div()
                                            .absolute()
                                            .inset_0()
                                            .rounded(radius::ART_SMALL)
                                            .bg(palette.scrim)
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                Icon::new(if playing {
                                                    IconName::Volume
                                                } else {
                                                    IconName::Pause
                                                })
                                                .size(px(16.))
                                                .color(palette.on_scrim),
                                            ),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_grow(1.)
                                .min_w(px(0.))
                                .child(
                                    div()
                                        .text_size(type_scale::BODY.font_size)
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(if current {
                                            palette.now_playing
                                        } else {
                                            palette.text
                                        })
                                        .truncate()
                                        .child(track.title.clone()),
                                )
                                .child(
                                    div()
                                        .text_size(type_scale::CAPTION.font_size)
                                        .text_color(palette.secondary)
                                        .truncate()
                                        .child(names(&track.artists)),
                                ),
                        )
                        .child(
                            div()
                                .text_size(type_scale::CAPTION.font_size)
                                .text_color(palette.secondary)
                                .font_features(tabular())
                                .group_hover(group.clone(), |style| style.opacity(0.))
                                .child(track.duration_ms.map(duration).unwrap_or_default()),
                        )
                        .child(
                            div()
                                .absolute()
                                .right(spacing::X2)
                                .opacity(0.)
                                .group_hover(group, |style| style.opacity(1.))
                                .child(
                                    IconButton::new(
                                        ("queue-remove", index),
                                        IconName::Close,
                                        "Remove from queue",
                                    )
                                    .on_click(
                                        move |_, _, cx| {
                                            cx.stop_propagation();
                                            remove.remove_from_queue(index)
                                        },
                                    ),
                                ),
                        )
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&self.scroll)
        .flex_grow(1.)
        .min_h(px(0.));
        div()
            .size_full()
            .flex()
            .flex_col()
            .children(caption)
            .child(list)
            .into_any_element()
    }
}
