//! Any page from `api::Page` or a search, rendered generically: header, chips,
//! then each shelf by its `SectionLayout`. The whole page is one virtualised
//! `list`, with a 400 track playlist as 400 rows, so only what is on screen
//! is laid out. Grids are cut into rows by the width the list had last frame.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use formalmusic_api::{
    BrowseTarget, Chip, Header, Item, LibraryTab, Link, Page, PlaySource, SearchFilter, Section,
    SectionLayout,
};
use formalmusic_core::{MusicStore, Route, SearchKey};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions::MenuContext;
use crate::bridge::{Bridge, Topic};
use crate::icons::{Icon, IconName};
use crate::primitives::{Button, ButtonKind};
use crate::shelves::{self, CARD_GAP, Env, MOOD_TILE, QUICK_PICK_WIDTH, RowLayout};
use crate::theme::{
    CARD_ART, PAGE_INSET, PAGE_MAX_WIDTH, PLAYER_HEIGHT, Theme, spacing, type_scale,
};

/// Below this page width track rows drop their album and play count
/// columns, and the top result stacks its songs under the card.
const WIDE_ROWS: Pixels = px(860.);
const HERO_SIDE_BY_SIDE: Pixels = px(1000.);
/// Songs beside the top result.
const HERO_SONGS: usize = 4;

/// What a page shows, whichever command fetched it.
#[derive(Clone)]
enum Content {
    Page(Arc<Page>),
    Search(Arc<formalmusic_api::SearchResults>),
}

impl Content {
    fn sections(&self) -> &[Section] {
        match self {
            Content::Page(page) => &page.sections,
            Content::Search(results) => &results.sections,
        }
    }

    fn header(&self) -> Option<&Header> {
        match self {
            Content::Page(page) => page.header.as_ref(),
            Content::Search(_) => None,
        }
    }

    fn same(&self, other: &Content) -> bool {
        match (self, other) {
            (Content::Page(a), Content::Page(b)) => Arc::ptr_eq(a, b),
            (Content::Search(a), Content::Search(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Row {
    Header,
    Chips,
    Correction,
    /// A shelf's heading.
    Title(usize),
    Carousel(usize),
    TrackGrid(usize),
    Hero(usize),
    /// One item of a List shelf.
    Item(usize, usize),
    /// Items `start..start + count` of a Grid shelf.
    Grid(usize, usize, usize),
    /// The tail of a shelf with more items to fetch.
    MoreItems(usize),
    /// The end of the page: more sections to fetch, or room above the player bar.
    End,
}

/// Everything the list's row closure reads. Shared with it, since the list
/// renders rows after `render` returns.
struct Rows {
    content: Option<Content>,
    rows: Vec<Row>,
    scrolls: HashMap<usize, ScrollHandle>,
    loading_more: bool,
    has_more: bool,
    env: Option<Env>,
    /// The header's description is expanded past its clamp.
    description_open: bool,
    /// The page's width as of last frame, up to [`PAGE_MAX_WIDTH`].
    width: Pixels,
}

pub struct PageView {
    route: Route,
    store: MusicStore,
    shared: Rc<RefCell<Rows>>,
    list: ListState,
    width: Rc<Cell<Pixels>>,
    columns: (usize, usize),
}

/// The web app's filter chips, then its Library tab, which searches only
/// what you saved.
const SEARCH_FILTERS: [(&str, Option<SearchFilter>); 11] = [
    ("All", None),
    ("Songs", Some(SearchFilter::Songs)),
    ("Videos", Some(SearchFilter::Videos)),
    ("Albums", Some(SearchFilter::Albums)),
    ("Artists", Some(SearchFilter::Artists)),
    (
        "Community playlists",
        Some(SearchFilter::CommunityPlaylists),
    ),
    ("Featured playlists", Some(SearchFilter::FeaturedPlaylists)),
    ("Podcasts", Some(SearchFilter::Podcasts)),
    ("Episodes", Some(SearchFilter::Episodes)),
    ("Profiles", Some(SearchFilter::Profiles)),
    ("Library", Some(SearchFilter::Library)),
];

impl PageView {
    pub fn new(route: Route, store: MusicStore, cx: &mut Context<Self>) -> Self {
        let weak = cx.entity().downgrade();
        let topic = match &route {
            Route::Browse(target) => Topic::Page(target.clone()),
            Route::Search(key) => Topic::Search(key.clone()),
        };
        for topic in [topic, Topic::NowPlaying, Topic::Ratings] {
            Bridge::watch(cx, topic, weak.clone().into());
        }
        let list = ListState::new(0, ListAlignment::Top, px(600.));
        let shared = Rc::new(RefCell::new(Rows {
            content: None,
            rows: Vec::new(),
            scrolls: HashMap::new(),
            loading_more: false,
            has_more: false,
            env: None,
            description_open: false,
            width: px(0.),
        }));
        let this = Self {
            route,
            store,
            shared,
            list,
            width: Rc::default(),
            columns: (0, 0),
        };
        this.fetch();
        this
    }

    pub fn route(&self) -> &Route {
        &self.route
    }

    /// Fetches the page if it is missing or stale; the cached one shows meanwhile.
    pub fn fetch(&self) {
        self.store.set_visible(self.route.clone());
        match &self.route {
            Route::Browse(target) => self.store.open(target.clone()),
            Route::Search(key) => self.store.search(key.clone()),
        }
    }

    fn retry(&self) {
        match &self.route {
            Route::Browse(target) => self.store.refresh(target.clone()),
            Route::Search(key) => self.store.search(key.clone()),
        }
    }

    /// Back to the top, for a second click on the nav item already showing it.
    pub fn scroll_to_top(&self) {
        self.list.scroll_to(ListOffset::default());
    }

    pub fn scroll_to_end(&self) {
        self.list.scroll_to_end();
    }

    /// Cards and mood tiles per grid row at `width`.
    fn columns_for(width: Pixels) -> (usize, usize) {
        let width = width.min(PAGE_MAX_WIDTH);
        (
            shelves::tiles(width, CARD_ART).0,
            shelves::tiles(width, MOOD_TILE).0,
        )
    }

    fn build_rows(
        content: &Content,
        chips: bool,
        columns: (usize, usize),
        has_more: bool,
        correction: bool,
    ) -> Vec<Row> {
        // A filtered search lists albums, artists and playlists as rows; they
        // read better as the card grid the library uses.
        let search = matches!(content, Content::Search(_));
        let mut rows = Vec::new();
        if content.header().is_some() {
            rows.push(Row::Header);
        }
        if chips {
            rows.push(Row::Chips);
        }
        if correction {
            rows.push(Row::Correction);
        }
        for (index, section) in content.sections().iter().enumerate() {
            if section.items.is_empty() {
                continue;
            }
            rows.push(Row::Title(index));
            let layout = match section.layout {
                SectionLayout::List
                    if search && !section.items.iter().any(|i| matches!(i, Item::Track(_))) =>
                {
                    SectionLayout::Grid
                }
                layout => layout,
            };
            match layout {
                SectionLayout::Carousel => rows.push(Row::Carousel(index)),
                SectionLayout::TrackGrid => rows.push(Row::TrackGrid(index)),
                SectionLayout::Hero => rows.push(Row::Hero(index)),
                SectionLayout::List => {
                    rows.extend((0..section.items.len()).map(|item| Row::Item(index, item)))
                }
                SectionLayout::Grid => {
                    let all = |kind: fn(&Item) -> bool| section.items.iter().all(kind);
                    let per_row = if all(|item| matches!(item, Item::Shortcut { .. })) {
                        section.items.len()
                    } else if all(|item| matches!(item, Item::Mood { .. })) {
                        columns.1
                    } else {
                        columns.0
                    };
                    rows.extend((0..section.items.len()).step_by(per_row).map(|start| {
                        Row::Grid(index, start, per_row.min(section.items.len() - start))
                    }));
                }
            }
            if section.continuation.is_some() {
                rows.push(Row::MoreItems(index));
            }
        }
        let _ = has_more;
        rows.push(Row::End);
        rows
    }

    /// Brings the shared rows up to date with the store, splicing the list
    /// so scroll position and measured heights survive an appended page.
    fn sync(&mut self, cx: &mut Context<Self>) -> Status {
        let width = self.width.get();
        let columns = Self::columns_for(width);
        let (content, status, loading_more) = {
            let state = self.store.state();
            match &self.route {
                Route::Browse(target) => match state.pages.get(target) {
                    Some(entry) => (
                        entry.page.clone().map(Content::Page),
                        status_of(entry.page.is_some(), entry.loading, entry.error.clone()),
                        entry.loading_more,
                    ),
                    None => (None, Status::Loading, false),
                },
                Route::Search(key) => match state.searches.get(key) {
                    Some(entry) => (
                        entry.results.clone().map(Content::Search),
                        status_of(entry.results.is_some(), entry.loading, entry.error.clone()),
                        entry.loading_more,
                    ),
                    None => (None, Status::Loading, false),
                },
            }
        };
        let playing = {
            let state = self.store.state();
            state.player.track.as_ref().map(|track| {
                (
                    track.video_id.clone(),
                    state.player.status == formalmusic_api::Status::Playing,
                )
            })
        };
        let editable_playlist = match (&self.route, content.as_ref().and_then(Content::header)) {
            (
                Route::Browse(BrowseTarget::Playlist(id)),
                Some(Header::Detail { editable: true, .. }),
            ) => Some(id.clone()),
            _ => None,
        };
        let env = Env {
            palette: Theme::get(cx),
            store: self.store.clone(),
            playing,
            menu: MenuContext { editable_playlist },
        };
        let mut shared = self.shared.borrow_mut();
        shared.env = Some(env);
        shared.width = width.min(PAGE_MAX_WIDTH);
        shared.loading_more = loading_more;
        let Some(content) = content else {
            if shared.content.take().is_some() {
                shared.rows.clear();
                self.list.reset(0);
            }
            return status;
        };
        let changed = shared
            .content
            .as_ref()
            .is_none_or(|old| !old.same(&content))
            || self.columns != columns;
        if changed {
            let chips = match &content {
                Content::Page(page) => !page.chips.is_empty(),
                Content::Search(_) => true,
            };
            let correction =
                matches!(&content, Content::Search(results) if results.correction.is_some());
            let has_more = match &content {
                Content::Page(page) => page.continuation.is_some(),
                Content::Search(results) => results.continuation.is_some(),
            };
            let next = Self::build_rows(&content, chips, columns, has_more, correction);
            if shared.content.is_none()
                && matches!(
                    self.route,
                    Route::Browse(BrowseTarget::Home | BrowseTarget::Explore)
                )
            {
                self.store.prefetch(first_targets(&content));
            }
            let old = std::mem::take(&mut shared.rows);
            let prefix = old
                .iter()
                .zip(next.iter())
                .take_while(|(a, b)| a == b)
                .count();
            if old.is_empty() {
                self.list.reset(next.len());
            } else {
                self.list.splice(prefix..old.len(), next.len() - prefix);
                // A shelf that stayed in place may still hold new items.
                if prefix > 0 {
                    self.list.remeasure_items(0..prefix);
                }
            }
            shared.rows = next;
            shared.has_more = has_more;
            shared.content = Some(content);
            self.columns = columns;
        }
        Status::Ready
    }

    fn render_row(
        shared: &Rc<RefCell<Rows>>,
        index: usize,
        route: &Route,
        page: &WeakEntity<PageView>,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let mut guard = shared.borrow_mut();
        let Rows {
            content,
            rows,
            scrolls,
            loading_more,
            has_more,
            env,
            description_open,
            width,
        } = &mut *guard;
        let width = *width;
        let (Some(content), Some(env), Some(row)) =
            (content.clone(), env.clone(), rows.get(index).copied())
        else {
            return div().into_any_element();
        };
        let palette = env.palette;
        let sections = content.sections();
        let notify_page = {
            let page = page.clone();
            Rc::new(move |cx: &mut App| {
                let _ = page.update(cx, |_, cx| cx.notify());
            }) as Rc<dyn Fn(&mut App)>
        };
        match row {
            Row::Header => {
                let page = page.clone();
                let description = crate::header::Description {
                    open: *description_open,
                    toggle: Rc::new(move |cx: &mut App| {
                        let _ = page.update(cx, |this, cx| {
                            let open = &mut this.shared.borrow_mut().description_open;
                            *open = !*open;
                            this.list.remeasure_items(index..index + 1);
                            cx.notify();
                        });
                    }),
                };
                crate::header::header(
                    content.header().expect("header row"),
                    &env.store,
                    palette,
                    description,
                    window,
                )
            }
            Row::Chips => {
                let on_select: Rc<dyn Fn(&Chip, &mut App)> = match route {
                    Route::Search(key) => {
                        let query = key.query.clone();
                        Rc::new(move |chip: &Chip, cx: &mut App| {
                            let filter = SEARCH_FILTERS
                                .iter()
                                .find(|(title, _)| *title == chip.title)
                                .and_then(|(_, filter)| *filter);
                            crate::app::navigate_replace(
                                Route::Search(SearchKey {
                                    query: query.clone(),
                                    filter,
                                }),
                                cx,
                            );
                        })
                    }
                    Route::Browse(target) => {
                        let target = target.clone();
                        Rc::new(move |chip: &Chip, cx: &mut App| {
                            crate::app::navigate_replace(
                                Route::Browse(chip_target(&target, chip)),
                                cx,
                            )
                        })
                    }
                };
                let chips = match (&content, route) {
                    (Content::Page(page), _) => page.chips.clone(),
                    (Content::Search(_), Route::Search(key)) => SEARCH_FILTERS
                        .iter()
                        .map(|(title, filter)| Chip {
                            title: (*title).into(),
                            params: String::new(),
                            selected: *filter == key.filter,
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                shelves::chips(&chips, palette, on_select)
            }
            Row::Correction => {
                let Content::Search(results) = &content else {
                    return div().into_any_element();
                };
                div()
                    .px(PAGE_INSET)
                    .pt(spacing::X4)
                    .text_size(type_scale::BODY.font_size)
                    .text_color(palette.secondary)
                    .child(results.correction.clone().unwrap_or_default())
                    .into_any_element()
            }
            Row::Title(section) => {
                let shelf = &sections[section];
                let scroll = matches!(
                    shelf.layout,
                    SectionLayout::Carousel | SectionLayout::TrackGrid
                )
                .then(|| scrolls.entry(section).or_default().clone());
                let show_all = match (route, shelf.filter) {
                    (Route::Search(key), Some(filter)) if key.filter.is_none() => {
                        let query = key.query.clone();
                        Some(Rc::new(move |cx: &mut App| {
                            crate::app::navigate_replace(
                                Route::Search(SearchKey {
                                    query: query.clone(),
                                    filter: Some(filter),
                                }),
                                cx,
                            )
                        }) as shelves::Callback)
                    }
                    _ => None,
                };
                shelves::shelf_title(shelf, scroll.as_ref(), &env, section, notify_page, show_all)
            }
            Row::Carousel(section) => shelves::carousel(
                &sections[section],
                scrolls.entry(section).or_default(),
                &env,
                section,
                shelves::tiles(width, CARD_ART).1,
            ),
            Row::TrackGrid(section) => shelves::track_grid(
                &sections[section],
                scrolls.entry(section).or_default(),
                &env,
                section,
                shelves::tiles(width, QUICK_PICK_WIDTH).1,
            ),
            Row::Hero(section) => {
                let tracks = |shelf: &Section| -> Vec<formalmusic_api::Track> {
                    shelf
                        .items
                        .iter()
                        .filter_map(|item| match item {
                            Item::Track(track) => Some(track.clone()),
                            _ => None,
                        })
                        .take(HERO_SONGS)
                        .collect()
                };
                let mut songs = tracks(&sections[section]);
                if songs.is_empty()
                    && let Some(shelf) = sections
                        .iter()
                        .find(|shelf| shelf.filter == Some(SearchFilter::Songs))
                {
                    songs = tracks(shelf);
                }
                shelves::hero(&sections[section], &songs, &env, width >= HERO_SIDE_BY_SIDE)
            }
            Row::Item(section, item) => {
                let shelf = &sections[section];
                let id = ElementId::NamedInteger(format!("row-{section}").into(), item as u64);
                match &shelf.items[item] {
                    Item::Track(track) => {
                        let album = matches!(route, Route::Browse(BrowseTarget::Album(_)));
                        let playlist = matches!(route, Route::Browse(BrowseTarget::Playlist(_)));
                        let any = |has: fn(&formalmusic_api::Track) -> bool| {
                            shelf
                                .items
                                .iter()
                                .any(|item| matches!(item, Item::Track(track) if has(track)))
                        };
                        let album_artists: Vec<Link> = match content.header() {
                            Some(Header::Detail { subtitle, .. }) if album => subtitle
                                .iter()
                                .filter(|link| matches!(link.target, Some(BrowseTarget::Artist(_))))
                                .cloned()
                                .collect(),
                            _ => Vec::new(),
                        };
                        let on_play = play_from(&content, route, section, item, &env.store);
                        let reorder = (section == 0
                            && env.menu.editable_playlist.is_some()
                            && track.set_video_id.is_some())
                        .then_some(item);
                        shelves::track_row(
                            track,
                            id,
                            album.then_some(item),
                            reorder,
                            &album_artists,
                            &env,
                            on_play,
                            if width >= WIDE_ROWS {
                                RowLayout::Wide {
                                    album: !album && any(|track| track.album.is_some()),
                                    plays: !playlist && any(|track| track.plays.is_some()),
                                }
                            } else {
                                RowLayout::Narrow
                            },
                        )
                    }
                    other => shelves::item_row(other, id, &env),
                }
            }
            Row::Grid(section, start, count) => {
                let shelf = &sections[section];
                let moods = shelf
                    .items
                    .iter()
                    .all(|item| matches!(item, Item::Mood { .. }));
                let tile = shelves::tiles(width, if moods { MOOD_TILE } else { CARD_ART }).1;
                div()
                    .flex()
                    .flex_row()
                    .gap(CARD_GAP)
                    .px(PAGE_INSET)
                    .pb(spacing::X6)
                    .children(shelf.items[start..start + count].iter().enumerate().map(
                        |(n, item)| {
                            shelves::card(
                                item,
                                ElementId::NamedInteger(
                                    format!("grid-{section}").into(),
                                    (start + n) as u64,
                                ),
                                &env,
                                tile,
                            )
                        },
                    ))
                    .into_any_element()
            }
            Row::MoreItems(section) => {
                if let Route::Browse(target) = route {
                    let (store, target) = (env.store.clone(), target.clone());
                    cx.defer(move |_| store.load_more_items(target, section));
                }
                loading_row(palette)
            }
            Row::End => {
                if *has_more {
                    let store = env.store.clone();
                    let route = route.clone();
                    cx.defer(move |_| match route {
                        Route::Browse(target) => store.load_more(target),
                        Route::Search(key) => store.search_more(key),
                    });
                }
                let spinner = (*loading_more || *has_more).then(|| loading_row(palette));
                div()
                    .flex()
                    .flex_col()
                    .children(spinner)
                    .h(PLAYER_HEIGHT + spacing::X10)
                    .into_any_element()
            }
        }
    }
}

/// What a click on the opening shelves most likely opens: the first two
/// cards of each of the first three shelves.
fn first_targets(content: &Content) -> Vec<BrowseTarget> {
    content
        .sections()
        .iter()
        .filter(|section| section.layout == SectionLayout::Carousel)
        .take(3)
        .flat_map(|section| {
            section
                .items
                .iter()
                .filter_map(crate::actions::target_of)
                .take(2)
        })
        .collect()
}

fn loading_row(palette: crate::theme::Palette) -> AnyElement {
    div()
        .h(px(56.))
        .flex()
        .items_center()
        .justify_center()
        .text_size(type_scale::CAPTION.font_size)
        .text_color(palette.tertiary)
        .child("Loading more\u{2026}")
        .into_any_element()
}

/// The page a chip leads to: Home's mood chips filter Home, a selected chip
/// goes back to the plain page, and the library chips switch tabs.
fn chip_target(current: &BrowseTarget, chip: &Chip) -> BrowseTarget {
    match current {
        BrowseTarget::Library(_) => BrowseTarget::Library(match chip.title.as_str() {
            "Songs" => LibraryTab::Songs,
            "Albums" => LibraryTab::Albums,
            "Artists" => LibraryTab::Artists,
            "Subscriptions" => LibraryTab::Subscriptions,
            "Podcasts" => LibraryTab::Podcasts,
            "Uploads" => LibraryTab::Uploads,
            _ => LibraryTab::Playlists,
        }),
        _ if chip.selected => BrowseTarget::Home,
        _ => BrowseTarget::HomeChip {
            params: chip.params.clone(),
        },
    }
}

/// A click on a list row: an album or playlist plays whole from that row, a
/// list anywhere else plays its own tracks from it. The rows already loaded
/// go along, so the daemon starts at once and fetches only the rest.
fn play_from(
    content: &Content,
    route: &Route,
    section: usize,
    item: usize,
    store: &MusicStore,
) -> Rc<dyn Fn(&mut App)> {
    let store = store.clone();
    // The daemon reads the first list as the album or playlist itself.
    let playlist = match (route, content.header()) {
        (
            Route::Browse(BrowseTarget::Album(_) | BrowseTarget::Playlist(_)),
            Some(Header::Detail {
                playlist_id: Some(id),
                ..
            }),
        ) if section == 0 => Some(id.clone()),
        _ => None,
    };
    let content = content.clone();
    Rc::new(move |_| {
        let shelf = &content.sections()[section];
        let tracks: Vec<_> = shelf
            .items
            .iter()
            .filter_map(|item| {
                if let Item::Track(track) = item {
                    Some(track.clone())
                } else {
                    None
                }
            })
            .collect();
        let start = shelf.items[..item]
            .iter()
            .filter(|item| matches!(item, Item::Track(_)))
            .count();
        let source = match &playlist {
            Some(playlist_id) => PlaySource::Playlist {
                playlist_id: playlist_id.clone(),
                tracks,
            },
            None => PlaySource::Tracks { tracks },
        };
        store.play(source, start, false, false);
    })
}

enum Status {
    Ready,
    Loading,
    Failed(String),
}

fn status_of(has_page: bool, loading: bool, error: Option<String>) -> Status {
    match (has_page, error) {
        (true, _) => Status::Ready,
        (false, Some(error)) if !loading => Status::Failed(error),
        _ => Status::Loading,
    }
}

impl Render for PageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace::render("PageView");
        let palette = Theme::get(cx);
        let status = self.sync(cx);
        let width = self.width.clone();
        let probe = canvas(
            move |bounds, _, _| width.set(bounds.size.width),
            |_, _, _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .w_full()
        .h(px(0.));
        // Grids are cut by last frame's width; a resize that changes the
        // column count needs one more pass.
        let columns = Self::columns_for(self.width.get());
        if columns != self.columns && self.shared.borrow().content.is_some() {
            cx.on_next_frame(window, |_, _, cx| cx.notify());
        }
        let body = match status {
            Status::Ready => {
                let shared = self.shared.clone();
                let route = self.route.clone();
                let page = cx.entity().downgrade();
                list(self.list.clone(), move |index, window, cx| {
                    // The artist banner runs the full width; it centres its
                    // own text like the rest.
                    let full_bleed = {
                        let shared = shared.borrow();
                        shared.rows.get(index) == Some(&Row::Header)
                            && matches!(
                                shared.content.as_ref().and_then(Content::header),
                                Some(Header::Artist { .. })
                            )
                    };
                    div()
                        .w_full()
                        .when(!full_bleed, |el| el.max_w(PAGE_MAX_WIDTH).mx_auto())
                        .child(PageView::render_row(
                            &shared, index, &route, &page, window, cx,
                        ))
                        .into_any_element()
                })
                .size_full()
                .into_any_element()
            }
            Status::Loading => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(type_scale::BODY.font_size)
                .text_color(palette.tertiary)
                .child("Loading\u{2026}")
                .into_any_element(),
            Status::Failed(error) => {
                let signed_out = error.starts_with("Sign in");
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(spacing::X3)
                    .px(spacing::X6)
                    .child(
                        Icon::new(IconName::Alert)
                            .size(px(28.))
                            .color(palette.tertiary),
                    )
                    .child(
                        div()
                            .text_size(type_scale::TITLE.font_size)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(palette.text)
                            .child(if signed_out {
                                "This page needs an account"
                            } else {
                                "Could not load this page"
                            }),
                    )
                    .child(
                        div()
                            .text_size(type_scale::BODY.font_size)
                            .text_color(palette.secondary)
                            .text_align(TextAlign::Center)
                            .max_w(px(420.))
                            .child(error),
                    )
                    .child(if signed_out {
                        Button::new("page-sign-in", "Sign in")
                            .kind(ButtonKind::Primary)
                            .on_click(|_, _, cx| crate::app::show_sign_in(cx))
                            .into_any_element()
                    } else {
                        Button::new("page-retry", "Try again")
                            .on_click(cx.listener(|this, _, _, _| this.retry()))
                            .into_any_element()
                    })
                    .into_any_element()
            }
        };
        div()
            .id("page")
            .relative()
            .size_full()
            .bg(palette.canvas)
            .child(probe)
            .child(body)
            .when(false, |el| el)
    }
}
