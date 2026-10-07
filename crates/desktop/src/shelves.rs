//! The pieces a page is made of: shelf titles, cards, carousels, the quick
//! picks grid, track rows, mood tiles, chips and the search top result. Each
//! is a plain function of its data, so the page list can render any row
//! without holding a view per card.

use std::rc::Rc;

use formalmusic_api::{Chip, Item, Link, Rating, Section, Track, TrackKind};
use formalmusic_core::MusicStore;
use formalmusic_core::format::{byline, duration, names};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions::{self, MenuContext};
use crate::art;
use crate::header::{LinkLine, link_text, pill_button, rating_buttons};
use crate::icons::{Icon, IconName};
use crate::primitives::IconButton;
use crate::theme::{
    CARD_ART, PAGE_INSET, Palette, TRACK_ROW, radius, spacing, tabular, type_scale,
};

/// What every row needs to know about the world beyond its own item.
#[derive(Clone)]
pub struct Env {
    pub palette: Palette,
    pub store: MusicStore,
    /// The current track's video id, and whether it is playing.
    pub playing: Option<(String, bool)>,
    pub menu: MenuContext,
}

/// How a track row spreads its text across the width.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RowLayout {
    /// Title and artists, the duration on the right.
    Narrow,
    /// Narrow plus a column for the album and one for the play count, each
    /// kept on every row of a list where some row has one, so they line up.
    Wide { album: bool, plays: bool },
}

impl Env {
    fn is_current(&self, video_id: &str) -> Option<bool> {
        self.playing
            .as_ref()
            .filter(|(current, _)| current == video_id)
            .map(|(_, playing)| *playing)
    }
}

pub const CARD_GAP: Pixels = px(20.);
pub const QUICK_PICK_WIDTH: Pixels = px(360.);
const ROW_ART: Pixels = px(40.);
pub const MOOD_TILE: Pixels = px(196.);
const HERO_ART: Pixels = px(160.);

/// How many tiles at least `tile` wide fit across a page `width` wide, and
/// the width that makes that many fill it edge to edge.
pub fn tiles(width: Pixels, tile: Pixels) -> (usize, Pixels) {
    let inner = width - PAGE_INSET * 2.;
    let columns = (((inner + CARD_GAP) / (tile + CARD_GAP)).floor() as usize).max(1);
    let fill = (inner - CARD_GAP * (columns - 1) as f32) / columns as f32;
    (columns, fill.max(tile))
}

fn title_text(text: impl Into<SharedString>, palette: &Palette) -> Div {
    div()
        .text_size(type_scale::BODY.font_size)
        .line_height(type_scale::BODY.line_height)
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette.text)
        .truncate()
        .child(text.into())
}

fn sub_text(text: impl Into<SharedString>, palette: &Palette) -> Div {
    div()
        .text_size(type_scale::CAPTION.font_size)
        .line_height(type_scale::CAPTION.line_height)
        .text_color(palette.secondary)
        .truncate()
        .child(text.into())
}

fn explicit_badge(palette: &Palette) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .px(px(4.))
        .rounded(px(2.))
        .bg(palette.press_wash)
        .text_size(px(9.))
        .line_height(px(14.))
        .font_weight(FontWeight::BOLD)
        .text_color(palette.secondary)
        .child("E")
}

pub type Callback = Rc<dyn Fn(&mut App)>;

/// Shelf heading: the small strapline over the title, "More" and, for
/// shelves that scroll sideways, the arrows.
/// `show_all` replaces "More" with the web app's "Show all" on search shelves.
pub fn shelf_title(
    section: &Section,
    scroll: Option<&ScrollHandle>,
    env: &Env,
    id: usize,
    on_scrolled: Rc<dyn Fn(&mut App)>,
    show_all: Option<Callback>,
) -> AnyElement {
    let palette = env.palette;
    let Some(title) = section.title.clone() else {
        return div().h(spacing::X4).into_any_element();
    };
    let arrows = scroll.map(|handle| {
        let (offset, max) = (handle.offset().x, handle.max_offset().x);
        let can_back = offset < px(-1.);
        let can_forward = max > px(1.) && offset > -max + px(1.);
        let (back, forward) = (handle.clone(), handle.clone());
        let (back_done, forward_done) = (on_scrolled.clone(), on_scrolled.clone());
        div()
            .flex()
            .flex_row()
            .gap(spacing::X2)
            .child(
                IconButton::new(("shelf-back", id), IconName::ChevronLeft, "Scroll back")
                    .hit(px(32.))
                    .disabled(!can_back)
                    .on_click(move |_, _, cx| {
                        page_by(&back, -1.);
                        back_done(cx);
                    }),
            )
            .child(
                IconButton::new(
                    ("shelf-forward", id),
                    IconName::ChevronRight,
                    "Scroll forward",
                )
                .hit(px(32.))
                .disabled(!can_forward)
                .on_click(move |_, _, cx| {
                    page_by(&forward, 1.);
                    forward_done(cx);
                }),
            )
    });
    let more: Option<(&str, Callback)> = match (show_all, section.more.clone()) {
        (Some(show_all), _) => Some(("Show all", show_all)),
        (None, Some(target)) => Some((
            "More",
            Rc::new(move |cx: &mut App| actions::open(target.clone(), cx)),
        )),
        (None, None) => None,
    };
    let more = more.map(|(label, on_click)| {
        div()
            .id(("shelf-more", id))
            .h(px(32.))
            .px(spacing::X3)
            .rounded(radius::PILL)
            .border_1()
            .border_color(palette.separator)
            .flex()
            .items_center()
            .cursor_pointer()
            .hover(move |style| style.bg(palette.hover_wash))
            .text_size(type_scale::CAPTION.font_size)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(palette.text)
            .on_click(move |_, _, cx| on_click(cx))
            .child(label)
    });
    div()
        .w_full()
        .flex()
        .flex_row()
        .items_end()
        .gap(spacing::X3)
        .px(PAGE_INSET)
        .pt(spacing::X10)
        .pb(spacing::X4)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_grow(1.)
                .min_w(px(0.))
                .when_some(section.strapline.clone(), |el, strapline| {
                    el.child(
                        div()
                            .text_size(type_scale::CAPTION.font_size)
                            .line_height(type_scale::CAPTION.line_height)
                            .text_color(palette.secondary)
                            .truncate()
                            .child(strapline),
                    )
                })
                .child(
                    div()
                        .text_size(type_scale::LARGE.font_size)
                        .line_height(type_scale::LARGE.line_height)
                        .font_weight(FontWeight::BOLD)
                        .text_color(palette.text)
                        .truncate()
                        .child(title),
                ),
        )
        .children(more)
        .children(arrows)
        .into_any_element()
}

/// Moves a sideways shelf by most of what it shows, keeping one card of context.
fn page_by(handle: &ScrollHandle, direction: f32) {
    let width = handle.bounds().size.width;
    let step = (width - CARD_ART).max(CARD_ART);
    let offset = handle.offset();
    let max = handle.max_offset().x;
    let x = (offset.x - step * direction).min(px(0.)).max(-max);
    handle.set_offset(point(x, offset.y));
}

pub fn carousel(
    section: &Section,
    scroll: &ScrollHandle,
    env: &Env,
    id: usize,
    card_width: Pixels,
) -> AnyElement {
    div()
        .id(("carousel", id))
        // A flex parent and a non-shrinking row: as a block child the row is
        // squeezed to the viewport, and the scroll area then has nothing to
        // scroll. Same for the track grid and the chips. The wheel's vertical
        // motion stays with the page; only sideways motion moves the shelf.
        .flex()
        .overflow_x_scroll()
        .restrict_scroll_to_axis()
        .track_scroll(scroll)
        .w_full()
        .child(
            div()
                .flex_none()
                .flex()
                .flex_row()
                .gap(CARD_GAP)
                .px(PAGE_INSET)
                .children(section.items.iter().enumerate().map(|(n, item)| {
                    card(
                        item,
                        ElementId::NamedInteger(format!("card-{id}").into(), n as u64),
                        env,
                        card_width,
                    )
                })),
        )
        .into_any_element()
}

/// A square cover, its title and a second line, with a play button over the
/// cover on hover.
pub fn card(item: &Item, id: ElementId, env: &Env, width: Pixels) -> AnyElement {
    let palette = env.palette;
    let (title, subtitle, thumbnails, round) = card_text(item);
    match item {
        Item::Mood { .. } => return mood_tile(item, id, env, width),
        Item::Shortcut { .. } => return shortcut_tile(item, id, env),
        _ => {}
    }
    let group: SharedString = format!("{id}").into();
    let target = actions::target_of(item);
    let (play_item, menu_item) = (item.clone(), item.clone());
    let (play_store, menu_store) = (env.store.clone(), env.store.clone());
    let menu_context = env.menu.clone();
    let play = actions::playable(item).then(|| {
        div()
            .id("card-play")
            .absolute()
            .right(spacing::X2)
            .bottom(spacing::X2)
            .size(px(40.))
            .rounded(px(20.))
            .bg(palette.scrim)
            .flex()
            .items_center()
            .justify_center()
            .opacity(0.)
            .group_hover(group.clone(), |style| style.opacity(1.))
            .hover(|style| style.bg(hsla(0., 0., 0., 0.85)))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                actions::play_item(&play_item, &play_store);
            })
            .child(
                Icon::new(IconName::Play)
                    .size(px(18.))
                    .color(palette.on_scrim)
                    .filled(true),
            )
    });
    let current = match item {
        Item::Track(track) => env.is_current(&track.video_id).is_some(),
        _ => false,
    };
    div()
        .id(id)
        .group(group.clone())
        .w(width)
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(spacing::X2)
        .cursor_pointer()
        .on_hover({
            let target = actions::target_of(item);
            move |hovered, _, cx| actions::prefetch_on_hover(target.clone(), *hovered, cx)
        })
        .when_some(target, |el, target| {
            el.on_click(move |_, _, cx| actions::open(target.clone(), cx))
        })
        .when(matches!(item, Item::Track(_)), |el| {
            let (item, store) = (item.clone(), env.store.clone());
            el.on_click(move |_, _, _| actions::play_item(&item, &store))
        })
        .on_mouse_up(MouseButton::Right, move |event, window, cx| {
            actions::open_menu(
                event.position,
                actions::item_menu(&menu_item, &menu_context, &menu_store),
                window,
                cx,
            );
        })
        .child(
            div()
                .relative()
                .child(art::cover(&thumbnails, width, radius::ART, round, &palette))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(if round { width / 2. } else { radius::ART })
                        .group_hover(group, |style| style.bg(hsla(0., 0., 0., 0.18))),
                )
                .children(play),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .when(round, |el| el.items_center())
                .child(
                    div()
                        .text_size(type_scale::BODY.font_size)
                        .line_height(type_scale::BODY.line_height)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if current {
                            palette.now_playing
                        } else {
                            palette.text
                        })
                        .line_clamp(2)
                        .child(title),
                )
                .when_some(subtitle, |el, subtitle| {
                    el.child(sub_text(subtitle, &palette))
                }),
        )
        .into_any_element()
}

fn card_text(
    item: &Item,
) -> (
    String,
    Option<String>,
    Vec<formalmusic_api::Thumbnail>,
    bool,
) {
    match item {
        Item::Track(track) => (
            track.title.clone(),
            Some(names(&track.artists)),
            track.thumbnails.clone(),
            false,
        ),
        Item::Album {
            title,
            album_type,
            artists,
            year,
            thumbnails,
            ..
        } => {
            let parts: Vec<String> = album_type
                .iter()
                .cloned()
                .chain(std::iter::once(names(artists)).filter(|s| !s.is_empty()))
                .chain(year.iter().cloned())
                .collect();
            (
                title.clone(),
                Some(parts.join(" \u{2022} ")),
                thumbnails.clone(),
                false,
            )
        }
        Item::Artist {
            name,
            subtitle,
            thumbnails,
            ..
        } => (name.clone(), subtitle.clone(), thumbnails.clone(), true),
        Item::Playlist {
            title,
            subtitle,
            thumbnails,
            ..
        } => (title.clone(), subtitle.clone(), thumbnails.clone(), false),
        Item::Podcast {
            title,
            subtitle,
            thumbnails,
            ..
        } => (title.clone(), subtitle.clone(), thumbnails.clone(), false),
        Item::Mood { title, .. } | Item::Shortcut { title, .. } => {
            (title.clone(), None, Vec::new(), false)
        }
    }
}

pub fn mood_tile(item: &Item, id: ElementId, env: &Env, width: Pixels) -> AnyElement {
    let Item::Mood { title, color, .. } = item else {
        return div().into_any_element();
    };
    let palette = env.palette;
    let stripe = color
        .map(|argb| Hsla::from(rgb(argb & 0xffffff)))
        .unwrap_or(palette.accent);
    let target = actions::target_of(item);
    div()
        .id(id)
        .w(width)
        .h(px(48.))
        .flex_shrink_0()
        .flex()
        .flex_row()
        .items_center()
        .rounded(radius::ROW)
        .overflow_hidden()
        .bg(palette.raised)
        .cursor_pointer()
        .hover(move |style| style.bg(palette.raised_hover))
        .when_some(target, |el, target| {
            el.on_click(move |_, _, cx| actions::open(target.clone(), cx))
        })
        .child(div().w(px(6.)).h_full().bg(stripe))
        .child(
            div()
                .px(spacing::X3)
                .child(title_text(title.clone(), &palette)),
        )
        .into_any_element()
}

/// One of Explore's buttons to New releases, Charts and Moods & genres. They
/// share their row's width, as on the web app.
fn shortcut_tile(item: &Item, id: ElementId, env: &Env) -> AnyElement {
    let Item::Shortcut {
        title,
        target,
        icon,
    } = item
    else {
        return div().into_any_element();
    };
    let palette = env.palette;
    let icon = match icon.as_deref() {
        Some("MUSIC_NEW_RELEASE") => IconName::NewReleases,
        Some("TRENDING_UP") => IconName::Charts,
        Some("STICKER_EMOTICON") => IconName::Moods,
        _ => IconName::Explore,
    };
    let (target, hovered) = (target.clone(), target.clone());
    div()
        .id(id)
        .flex_1()
        .min_w(px(0.))
        .h(px(56.))
        .px(spacing::X4)
        .flex()
        .flex_row()
        .items_center()
        .gap(spacing::X3)
        .rounded(radius::ROW)
        .bg(palette.raised)
        .cursor_pointer()
        .hover(move |style| style.bg(palette.raised_hover))
        .on_hover(move |hovered_now, _, cx| {
            actions::prefetch_on_hover(Some(hovered.clone()), *hovered_now, cx)
        })
        .on_click(move |_, _, cx| actions::open(target.clone(), cx))
        .child(Icon::new(icon).size(px(20.)).color(palette.secondary))
        .child(title_text(title.clone(), &palette))
        .into_any_element()
}

/// Quick picks: columns of four rows, scrolled sideways. Charts' top
/// artists come in the same shelf as rows of artists.
pub fn track_grid(
    section: &Section,
    scroll: &ScrollHandle,
    env: &Env,
    id: usize,
    column_width: Pixels,
) -> AnyElement {
    let columns = section.items.chunks(4).enumerate().map(|(column, items)| {
        div()
            .w(column_width)
            .flex_shrink_0()
            .flex()
            .flex_col()
            .children(items.iter().enumerate().map(|(row, item)| {
                let Item::Track(track) = item else {
                    return compact_item(
                        item,
                        ElementId::NamedInteger(
                            format!("grid-{id}").into(),
                            (column * 4 + row) as u64,
                        ),
                        env,
                    );
                };
                let (store, video_id) = (env.store.clone(), track.video_id.clone());
                compact_track(
                    track,
                    ElementId::NamedInteger(format!("grid-{id}").into(), (column * 4 + row) as u64),
                    env,
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
                )
            }))
    });
    div()
        .id(("track-grid", id))
        .flex()
        .overflow_x_scroll()
        .restrict_scroll_to_axis()
        .track_scroll(scroll)
        .w_full()
        .child(
            div()
                .flex_none()
                .flex()
                .flex_row()
                .gap(CARD_GAP)
                .px(PAGE_INSET)
                .children(columns),
        )
        .into_any_element()
}

/// Art, title and artists in one 56 px row: quick picks and the queue.
pub fn compact_track(
    track: &Track,
    id: ElementId,
    env: &Env,
    on_play: Rc<dyn Fn(&mut App)>,
) -> AnyElement {
    let palette = env.palette;
    let current = env.is_current(&track.video_id);
    let group: SharedString = format!("{id}").into();
    let (menu_track, menu_store, menu_context) =
        (track.clone(), env.store.clone(), env.menu.clone());
    div()
        .id(id)
        .group(group.clone())
        .h(TRACK_ROW)
        .flex()
        .flex_row()
        .items_center()
        .gap(spacing::X3)
        .px(spacing::X2)
        .rounded(radius::ROW)
        .cursor_pointer()
        .hover(move |style| style.bg(palette.hover_wash))
        .on_click(move |_, _, cx| on_play(cx))
        .on_mouse_up(MouseButton::Right, move |event, window, cx| {
            actions::open_menu(
                event.position,
                actions::track_menu(&menu_track, &menu_context, &menu_store),
                window,
                cx,
            );
        })
        .child(row_art(track, current, group, &palette))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_grow(1.)
                .min_w(px(0.))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(spacing::X1)
                        .child(
                            title_text(track.title.clone(), &palette)
                                .when(current.is_some(), |el| el.text_color(palette.now_playing)),
                        )
                        .when(track.explicit, |el| el.child(explicit_badge(&palette))),
                )
                .child(sub_text(byline(track), &palette)),
        )
        .into_any_element()
}

/// The 40 px cover with a play glyph over it on hover, or the playing marker.
fn row_art(
    track: &Track,
    current: Option<bool>,
    group: SharedString,
    palette: &Palette,
) -> AnyElement {
    let marker = match current {
        Some(true) => Some(IconName::Volume),
        Some(false) => Some(IconName::Pause),
        None => None,
    };
    div()
        .relative()
        .flex_shrink_0()
        .child(art::cover(
            &track.thumbnails,
            ROW_ART,
            radius::ART_SMALL,
            false,
            palette,
        ))
        .child(
            div()
                .absolute()
                .inset_0()
                .rounded(radius::ART_SMALL)
                .flex()
                .items_center()
                .justify_center()
                .when(marker.is_some(), |el| el.bg(palette.scrim))
                .when(marker.is_none(), |el| {
                    el.opacity(0.)
                        .group_hover(group, |style| style.opacity(1.).bg(palette.scrim))
                })
                .child(
                    Icon::new(marker.unwrap_or(IconName::Play))
                        .size(px(16.))
                        .color(palette.on_scrim)
                        .filled(marker.is_none()),
                ),
        )
        .into_any_element()
}

/// The value a playlist row carries while it is dragged to a new place.
#[derive(Clone)]
struct RowDrag {
    playlist_id: String,
    from: usize,
    title: SharedString,
}

/// What follows the pointer while a row is dragged: its title in a pill.
struct RowGhost {
    title: SharedString,
}

impl Render for RowGhost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::theme::Theme::get(cx);
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

/// One row of an album, playlist or song list. On an album page `number`
/// replaces the cover with the track number, and the artists show only when
/// they are not the album's own, as the web app does. `reorder` is the row's
/// place in a playlist you own, which makes it draggable.
#[allow(clippy::too_many_arguments)]
pub fn track_row(
    track: &Track,
    id: ElementId,
    number: Option<usize>,
    reorder: Option<usize>,
    album_artists: &[Link],
    env: &Env,
    on_play: Rc<dyn Fn(&mut App)>,
    layout: RowLayout,
) -> AnyElement {
    let palette = env.palette;
    let drag = reorder.zip(env.menu.editable_playlist.clone());
    let current = env.is_current(&track.video_id);
    let rating = env.store.state().rating(track);
    let group: SharedString = format!("{id}").into();
    let (menu_track, menu_store, menu_context) =
        (track.clone(), env.store.clone(), env.menu.clone());
    let (more_track, more_store, more_context) =
        (track.clone(), env.store.clone(), env.menu.clone());
    let (rate_track, rate_store) = (track.clone(), env.store.clone());
    let lead = match number {
        Some(n) => {
            let marker = match current {
                Some(true) => Some(IconName::Volume),
                Some(false) => Some(IconName::Pause),
                None => None,
            };
            div()
                .w(ROW_ART)
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .relative()
                .child(match marker {
                    Some(icon) => Icon::new(icon)
                        .size(px(16.))
                        .color(palette.now_playing)
                        .into_any_element(),
                    None => div()
                        .text_size(type_scale::BODY.font_size)
                        .text_color(palette.secondary)
                        .font_features(tabular())
                        .group_hover(group.clone(), |style| style.opacity(0.))
                        .child((n + 1).to_string())
                        .into_any_element(),
                })
                .when(marker.is_none(), |el| {
                    el.child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .opacity(0.)
                            .group_hover(group.clone(), |style| style.opacity(1.))
                            .child(
                                Icon::new(IconName::Play)
                                    .size(px(16.))
                                    .color(palette.text)
                                    .filled(true),
                            ),
                    )
                })
                .into_any_element()
        }
        None => row_art(track, current, group.clone(), &palette),
    };
    let line = LinkLine::new(SharedString::from(format!("{id}-byline")), palette.text)
        .names(&track.artists);
    let (album_column, plays_column) = match layout {
        RowLayout::Wide { album, plays } => (album, plays),
        RowLayout::Narrow => (false, false),
    };
    let artists = div()
        .min_w(px(0.))
        .text_size(type_scale::BODY.font_size)
        .line_height(type_scale::BODY.line_height)
        .text_color(palette.secondary)
        .child(line);
    let album = album_column.then(|| {
        div()
            .w(relative(0.28))
            .flex_shrink_0()
            .min_w(px(0.))
            .truncate()
            .text_size(type_scale::BODY.font_size)
            .text_color(palette.secondary)
            .children(track.album.as_ref().map(|album| {
                link_text(
                    album,
                    SharedString::from(format!("{id}-album")),
                    palette.text,
                )
            }))
    });
    div()
        .id(id.clone())
        .group(group.clone())
        .h(TRACK_ROW)
        .mx(PAGE_INSET - spacing::X2)
        .px(spacing::X2)
        .flex()
        .flex_row()
        .items_center()
        .gap(spacing::X4)
        .rounded(radius::ROW)
        .cursor_pointer()
        .hover(move |style| style.bg(palette.hover_wash))
        .on_click(move |_, _, cx| on_play(cx))
        .on_mouse_up(MouseButton::Right, move |event, window, cx| {
            actions::open_menu(
                event.position,
                actions::track_menu(&menu_track, &menu_context, &menu_store),
                window,
                cx,
            );
        })
        .when_some(drag, |el, (index, playlist_id)| {
            let store = env.store.clone();
            let here = playlist_id.clone();
            el.on_drag(
                RowDrag {
                    playlist_id,
                    from: index,
                    title: track.title.clone().into(),
                },
                |drag, _, _, cx| {
                    cx.new(|_| RowGhost {
                        title: drag.title.clone(),
                    })
                },
            )
            .drag_over::<RowDrag>(move |style, _, _, _| {
                style.border_t_2().border_color(palette.accent)
            })
            .on_drop(move |drag: &RowDrag, _, _| {
                if drag.playlist_id == here {
                    store.move_in_playlist(here.clone(), drag.from, index)
                }
            })
            // In the page margin, so a row you can drag lines up with the rest.
            .relative()
            .child(
                div()
                    .absolute()
                    .left(px(-20.))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .opacity(0.)
                    .group_hover(group.clone(), |style| style.opacity(1.))
                    .child(
                        Icon::new(IconName::Grip)
                            .size(px(14.))
                            .color(palette.tertiary),
                    ),
            )
        })
        .child(lead)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_grow(1.)
                .flex_basis(px(0.))
                .min_w(px(0.))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(spacing::X1)
                        .min_w(px(0.))
                        .child(
                            title_text(track.title.clone(), &palette)
                                .when(current.is_some(), |el| el.text_color(palette.now_playing)),
                        )
                        .when(track.explicit, |el| el.child(explicit_badge(&palette))),
                )
                .when(number.is_none() || track.artists != album_artists, |el| {
                    el.child(artists)
                }),
        )
        .children(album)
        .when(plays_column, |el| {
            el.child(
                div()
                    .w(px(120.))
                    .flex_shrink_0()
                    .text_size(type_scale::CAPTION.font_size)
                    .text_color(palette.secondary)
                    .truncate()
                    .child(track.plays.clone().unwrap_or_default()),
            )
        })
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .flex_shrink_0()
                .opacity(if rating == Rating::Indifferent {
                    0.
                } else {
                    1.
                })
                .group_hover(group.clone(), |style| style.opacity(1.))
                .child(rating_buttons(
                    &format!("{id}"),
                    rating,
                    palette,
                    move |rating, _| rate_store.rate(&rate_track, rating),
                )),
        )
        .child(
            div()
                .w(px(44.))
                .flex_shrink_0()
                .text_size(type_scale::BODY.font_size)
                .text_color(palette.secondary)
                .font_features(tabular())
                .text_align(TextAlign::Right)
                .child(track.duration_ms.map(duration).unwrap_or_default()),
        )
        .child(
            div()
                .opacity(0.)
                .group_hover(group, |style| style.opacity(1.))
                .child(
                    IconButton::new(
                        SharedString::from(format!("{id}-more")),
                        IconName::More,
                        "More actions",
                    )
                    .on_click(move |event, window, cx| {
                        cx.stop_propagation();
                        actions::open_menu(
                            event.position(),
                            actions::track_menu(&more_track, &more_context, &more_store),
                            window,
                            cx,
                        );
                    }),
                ),
        )
        .into_any_element()
}

/// A list row for anything that is not a track, as library artists list.
pub fn item_row(item: &Item, id: ElementId, env: &Env) -> AnyElement {
    div()
        .mx(PAGE_INSET - spacing::X2)
        .child(compact_item(item, id, env))
        .into_any_element()
}

/// Art, title and subtitle of an album, artist or playlist in one 56 px row.
pub fn compact_item(item: &Item, id: ElementId, env: &Env) -> AnyElement {
    let palette = env.palette;
    let (title, subtitle, thumbnails, round) = card_text(item);
    let target = actions::target_of(item);
    let (menu_item, menu_store, menu_context) = (item.clone(), env.store.clone(), env.menu.clone());
    div()
        .id(id)
        .h(TRACK_ROW)
        .px(spacing::X2)
        .flex()
        .flex_row()
        .items_center()
        .gap(spacing::X4)
        .rounded(radius::ROW)
        .cursor_pointer()
        .on_hover({
            let target = actions::target_of(item);
            move |hovered, _, cx| actions::prefetch_on_hover(target.clone(), *hovered, cx)
        })
        .hover(move |style| style.bg(palette.hover_wash))
        .when_some(target, |el, target| {
            el.on_click(move |_, _, cx| actions::open(target.clone(), cx))
        })
        .on_mouse_up(MouseButton::Right, move |event, window, cx| {
            actions::open_menu(
                event.position,
                actions::item_menu(&menu_item, &menu_context, &menu_store),
                window,
                cx,
            );
        })
        .child(art::cover(
            &thumbnails,
            ROW_ART,
            radius::ART_SMALL,
            round,
            &palette,
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_grow(1.)
                .min_w(px(0.))
                .child(title_text(title, &palette))
                .when_some(subtitle, |el, subtitle| {
                    el.child(sub_text(subtitle, &palette))
                }),
        )
        .into_any_element()
}

/// What music.youtube.com calls a track in search results.
fn kind(track: &Track) -> &'static str {
    match track.kind {
        TrackKind::Song | TrackKind::Upload => "Song",
        TrackKind::Video => "Video",
        TrackKind::Episode => "Episode",
    }
}

/// Search's "Top result": the card with its own play buttons, and beside it
/// the first few songs, or under it when the page is too narrow for both.
pub fn hero(section: &Section, songs: &[Track], env: &Env, side_by_side: bool) -> AnyElement {
    let palette = env.palette;
    let Some(item) = section.items.first() else {
        return div().into_any_element();
    };
    let (title, subtitle, thumbnails, round) = card_text(item);
    let kind = match item {
        Item::Track(track) => kind(track),
        Item::Album { .. } => "Album",
        Item::Artist { .. } => "Artist",
        Item::Playlist { .. } => "Playlist",
        Item::Podcast { .. } => "Podcast",
        Item::Mood { .. } => "Mood",
        Item::Shortcut { .. } => "Page",
    };
    let target = actions::target_of(item);
    let subtitle = match (item, subtitle) {
        (Item::Track(track), _) => [kind.to_owned(), byline(track)]
            .into_iter()
            .chain(track.duration_ms.map(duration))
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" \u{2022} "),
        (_, Some(subtitle)) if subtitle.starts_with(kind) => subtitle,
        (_, Some(subtitle)) => format!("{kind} \u{2022} {subtitle}"),
        (_, None) => kind.to_owned(),
    };
    let playable = actions::playable(item);
    let play_playlist = |playlist_id: String, shuffle: bool, radio: bool, store: MusicStore| {
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            cx.stop_propagation();
            store.play(
                formalmusic_api::PlaySource::Playlist {
                    playlist_id: playlist_id.clone(),
                    tracks: Vec::new(),
                },
                0,
                shuffle,
                radio,
            )
        }
    };
    let (play_item, play_store) = (item.clone(), env.store.clone());
    let buttons = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(spacing::X2)
        .pt(spacing::X3)
        .when(playable, |el| {
            el.child(pill_button(
                "top-result-play",
                IconName::Play,
                "Play",
                true,
                palette,
                move |_, _, cx| {
                    cx.stop_propagation();
                    actions::play_item(&play_item, &play_store)
                },
            ))
        })
        .when_some(section.shuffle_playlist_id.clone(), |el, playlist_id| {
            el.child(pill_button(
                "top-result-shuffle",
                IconName::Shuffle,
                "Shuffle",
                !playable,
                palette,
                play_playlist(playlist_id, true, false, env.store.clone()),
            ))
        })
        .when_some(section.radio_playlist_id.clone(), |el, playlist_id| {
            el.child(pill_button(
                "top-result-radio",
                IconName::Radio,
                "Radio",
                false,
                palette,
                play_playlist(playlist_id, false, true, env.store.clone()),
            ))
        });
    let card = div()
        .id("top-result")
        .mx(spacing::X2)
        .p(spacing::X4)
        .flex()
        .flex_row()
        .items_center()
        .gap(spacing::X6)
        .rounded(radius::CARD)
        .bg(palette.raised)
        .cursor_pointer()
        .hover(move |style| style.bg(palette.raised_hover))
        .when(side_by_side, |el| el.flex_1().min_w(px(0.)))
        .when_some(target, |el, target| {
            el.on_click(move |_, _, cx| actions::open(target.clone(), cx))
        })
        .child(art::cover(
            &thumbnails,
            HERO_ART,
            radius::ART,
            round,
            &palette,
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(spacing::X1)
                .min_w(px(0.))
                .child(
                    div()
                        .text_size(type_scale::DISPLAY.font_size)
                        .line_height(type_scale::DISPLAY.line_height)
                        .font_weight(FontWeight::BOLD)
                        .text_color(palette.text)
                        .line_clamp(2)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(type_scale::BODY.font_size)
                        .line_height(type_scale::BODY.line_height)
                        .text_color(palette.secondary)
                        .truncate()
                        .child(subtitle),
                )
                .child(buttons),
        );
    let list = (!songs.is_empty()).then(|| {
        let tracks: Vec<Track> = songs.to_vec();
        div()
            .flex()
            .flex_col()
            .when(side_by_side, |el| el.flex_1().min_w(px(0.)))
            .children(songs.iter().enumerate().map(|(n, track)| {
                let (store, tracks) = (env.store.clone(), tracks.clone());
                compact_track(
                    track,
                    ElementId::NamedInteger("top-result-song".into(), n as u64),
                    env,
                    Rc::new(move |_| {
                        store.play(
                            formalmusic_api::PlaySource::Tracks {
                                tracks: tracks.clone(),
                            },
                            n,
                            false,
                            false,
                        )
                    }),
                )
            }))
    });
    div()
        .px(PAGE_INSET - spacing::X2)
        .flex()
        .gap(spacing::X6)
        .map(|el| {
            if side_by_side {
                el.flex_row()
            } else {
                el.flex_col()
            }
        })
        .child(card)
        .children(list)
        .into_any_element()
}

/// The row of filter pills under a page title.
pub fn chips(
    chips: &[Chip],
    palette: Palette,
    on_select: Rc<dyn Fn(&Chip, &mut App)>,
) -> AnyElement {
    div()
        .id("chips")
        .flex()
        .overflow_x_scroll()
        .restrict_scroll_to_axis()
        .w_full()
        .child(
            div()
                .flex_none()
                .flex()
                .flex_row()
                .gap(spacing::X2)
                .px(PAGE_INSET)
                .pt(spacing::X6)
                .children(chips.iter().enumerate().map(|(n, chip)| {
                    let selected = chip.selected;
                    let on_select = on_select.clone();
                    let chip_value = chip.clone();
                    div()
                        .id(("chip", n))
                        .h(px(32.))
                        .px(spacing::X3)
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .rounded(radius::CONTROL)
                        .bg(if selected {
                            palette.text
                        } else {
                            palette.press_wash
                        })
                        .text_size(type_scale::BODY.font_size)
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if selected {
                            palette.canvas
                        } else {
                            palette.text
                        })
                        .cursor_pointer()
                        .when(!selected, |el| {
                            el.hover(move |style| style.bg(palette.raised_hover))
                        })
                        .on_click(move |_, _, cx| on_select(&chip_value, cx))
                        .child(chip.title.clone())
                })),
        )
        .into_any_element()
}
