//! Any page or search, rendered generically: header, chips, then each shelf by its layout. The
//! page is one virtual list keyed by row, so a 1000 track playlist is 1000 rows and an
//! appended continuation never moves what is on screen.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use formalmusic_api::{
    BrowseTarget, Chip, Header, Item, PlaySource, SearchFilter, SearchResults, Section,
    SectionLayout,
};
use formalmusic_core::{MusicStore, Route, SearchKey, StoreEvent};
use nd::prelude::*;

use crate::bridge;
use crate::items::{self, RowEnv};
use crate::menus::{self, MenuContext};
use crate::root;

/// Cards per grid row; the list rows wrap them at the page's width.
const GRID_CHUNK: usize = 12;

/// The web app's filter chips, then its Library tab, which searches only what you saved.
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

#[derive(Clone)]
enum Content {
    Page(Arc<formalmusic_api::Page>),
    Search(Arc<SearchResults>),
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

    fn has_more(&self) -> bool {
        match self {
            Content::Page(page) => page.continuation.is_some(),
            Content::Search(results) => results.continuation.is_some(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Row {
    LargeTitle,
    Header,
    Chips,
    Correction,
    Title(usize),
    Carousel(usize),
    TrackGrid(usize),
    Hero(usize),
    Item(usize, usize),
    Grid(usize, usize),
    MoreItems(usize),
    End,
}

enum Status {
    Ready,
    Loading,
    Failed(String),
}

pub struct PageView {
    route: Route,
    store: MusicStore,
    list: ListState<Row>,
    content: Option<Content>,
    shelves: HashMap<usize, ShelfHandle>,
    chips: ShelfHandle,
    description_open: bool,
    loading_more: bool,
    wide: bool,
}

impl PageView {
    pub fn new(route: Route, cx: &mut Context<Self>) -> Self {
        let store = crate::store(cx);
        let topic = match &route {
            Route::Browse(target) => StoreEvent::Page(target.clone()),
            Route::Search(key) => StoreEvent::Search(key.clone()),
        };
        let id = cx.entity_id();
        for topic in [topic, StoreEvent::NowPlaying, StoreEvent::Ratings] {
            bridge::watch(cx, topic, id);
        }
        let list = ListState::new(cx)
            .follow(Follow::None)
            .estimated_row_height(px(56.));
        let this = Self {
            route,
            store,
            list,
            content: None,
            shelves: HashMap::new(),
            chips: ShelfHandle::new(),
            description_open: false,
            loading_more: false,
            wide: true,
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

    pub fn scroll_to_top(&mut self, cx: &mut App) {
        self.list.scroll_to_top(cx);
    }

    pub fn scroll_to_end(&mut self, cx: &mut App) {
        self.list.scroll_to_end(cx);
    }

    fn rows(content: &Content) -> Vec<Row> {
        // A search lists albums, artists and playlists as rows; they read better as cards.
        let search = matches!(content, Content::Search(_));
        let mut rows = Vec::new();
        if large_title(content).is_some() {
            rows.push(Row::LargeTitle);
        }
        if content.header().is_some() {
            rows.push(Row::Header);
        }
        let chips = match content {
            Content::Page(page) => !page.chips.is_empty(),
            Content::Search(results) => {
                if results.correction.is_some() {
                    rows.push(Row::Correction);
                }
                true
            }
        };
        if chips {
            let above = content.header().is_some() || large_title(content).is_some();
            rows.insert(usize::from(above), Row::Chips);
        }
        for (index, section) in content.sections().iter().enumerate() {
            if section.items.is_empty() {
                continue;
            }
            let layout = match section.layout {
                SectionLayout::List
                    if search && !section.items.iter().any(|i| matches!(i, Item::Track(_))) =>
                {
                    SectionLayout::Grid
                }
                layout => layout,
            };
            // Carousels and the quick picks carry their title as the shelf's own label row. A
            // shelf with nothing to say there (an album's or playlist's own songs) gets none.
            let titled = section.title.is_some()
                || section.strapline.is_some()
                || section.more.is_some()
                || section.filter.is_some();
            if titled {
                rows.push(Row::Title(index));
            }
            match layout {
                SectionLayout::Carousel => rows.push(Row::Carousel(index)),
                SectionLayout::TrackGrid => rows.push(Row::TrackGrid(index)),
                SectionLayout::Hero => rows.push(Row::Hero(index)),
                SectionLayout::List => {
                    rows.extend((0..section.items.len()).map(|item| Row::Item(index, item)))
                }
                SectionLayout::Grid => rows.extend(
                    (0..section.items.len())
                        .step_by(GRID_CHUNK)
                        .map(|start| Row::Grid(index, start)),
                ),
            }
            if section.continuation.is_some() {
                rows.push(Row::MoreItems(index));
            }
        }
        rows.push(Row::End);
        rows
    }

    /// Reads the store and, when the page changed, feeds the list its rows. Keyed rows keep
    /// scroll and measured heights across an appended continuation.
    fn sync(&mut self, cx: &mut Context<Self>) -> Status {
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
        self.loading_more = loading_more;
        let Some(content) = content else {
            self.content = None;
            return status;
        };
        if self.content.as_ref().is_none_or(|old| !old.same(&content)) {
            if self.content.is_none()
                && matches!(
                    self.route,
                    Route::Browse(BrowseTarget::Home | BrowseTarget::Explore)
                )
            {
                self.store.prefetch(first_targets(&content));
            }
            // Rows of a changed page re-render; the version is its pointer.
            let version = match &content {
                Content::Page(p) => Arc::as_ptr(p) as u64,
                Content::Search(s) => Arc::as_ptr(s) as u64,
            };
            self.list
                .set_items_versioned(Self::rows(&content).into_iter().map(|r| (r, version)), cx);
            self.content = Some(content);
        }
        Status::Ready
    }

    fn env(&self, row: usize) -> RowEnv {
        let state = self.store.state();
        let playing = state.player.track.as_ref().map(|t| {
            (
                t.video_id.clone(),
                state.player.status == formalmusic_api::Status::Playing,
            )
        });
        let header = self.content.as_ref().and_then(Content::header);
        let editable_playlist = match (&self.route, header) {
            (
                Route::Browse(BrowseTarget::Playlist(id)),
                Some(Header::Detail { editable: true, .. }),
            ) => Some((id.clone(), row)),
            _ => None,
        };
        let hide_artist = match (&self.route, header) {
            (Route::Browse(BrowseTarget::Album(_)), Some(Header::Detail { subtitle, .. })) => {
                subtitle
                    .iter()
                    .find(|l| l.target.is_some())
                    .map(|l| l.text.clone())
            }
            _ => None,
        };
        RowEnv {
            playing,
            menu: MenuContext {
                editable_playlist,
                queue_index: None,
            },
            hide_artist,
            wide: self.wide,
        }
    }

    fn row(&mut self, row: Row, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(content) = self.content.clone() else {
            return div().into_any_element();
        };
        let sections = content.sections();
        let id = |kind: &str, a: usize| format!("{kind}-{a}");
        let body = match row {
            Row::LargeTitle => label(large_title(&content).unwrap_or_default())
                .text_style(TextStyle::LargeTitle)
                .into_any_element(),
            Row::Header => {
                let Some(header) = content.header() else {
                    return div().into_any_element();
                };
                let page = cx.weak_entity();
                items::header(
                    header,
                    self.description_open,
                    Rc::new(move |_, cx| {
                        page.update(cx, |this, cx| {
                            this.description_open = !this.description_open;
                            this.list.invalidate(&Row::Header, cx);
                            cx.notify();
                        });
                    }),
                    cx,
                )
            }
            Row::Chips => {
                let (chips, on_select) = self.chips_of(&content);
                items::chips("chips", &chips, &self.chips, on_select)
            }
            Row::Correction => match &content {
                Content::Search(results) => label(format!(
                    "Showing results for {}",
                    results.correction.clone().unwrap_or_default()
                ))
                .secondary()
                .into_any_element(),
                Content::Page(_) => div().into_any_element(),
            },
            Row::Title(section) => {
                let shelf = &sections[section];
                let more = shelf.more.clone();
                let filter = shelf.filter;
                let route = self.route.clone();
                v_flex()
                    .when_some(shelf.strapline.clone(), |col, s| {
                        col.child(label(s).secondary())
                    })
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                label(shelf.title.clone().unwrap_or_default())
                                    .text_style(TextStyle::Title2)
                                    .flex_1(),
                            )
                            .when(more.is_some() || filter.is_some(), |row| {
                                row.child(
                                    button(
                                        id("more", section),
                                        "Show all",
                                    )
                                    .borderless()
                                    .on_click(
                                        move |_, _, cx| match (&route, filter, &more) {
                                            (Route::Search(key), Some(filter), _) => {
                                                root::navigate_replace(
                                                    Route::Search(SearchKey {
                                                        query: key.query.clone(),
                                                        filter: Some(filter),
                                                    }),
                                                    cx,
                                                )
                                            }
                                            (_, _, Some(target)) => {
                                                root::navigate(Route::Browse(target.clone()), cx)
                                            }
                                            _ => {}
                                        },
                                    ),
                                )
                            }),
                    )
                    .into_any_element()
            }
            Row::Carousel(section) => {
                let handle = self.shelves.entry(section).or_default().clone();
                items::carousel(id("shelf", section), &sections[section], &handle, cx)
            }
            Row::TrackGrid(section) => {
                let handle = self.shelves.entry(section).or_default().clone();
                let env = self.env(0);
                items::track_grid(id("picks", section), &sections[section], &handle, &env, cx)
            }
            Row::Hero(section) => {
                let env = self.env(0);
                items::hero(id("top", section), &sections[section], &env, cx)
            }
            Row::Item(section, item) => {
                let shelf = &sections[section];
                match &shelf.items[item] {
                    Item::Track(track) => {
                        let env = self.env(item);
                        items::track_row(format!("row-{section}-{item}"), track, &env, None, cx)
                    }
                    other => items::item_row(format!("row-{section}-{item}"), other, false, cx),
                }
            }
            Row::Grid(section, start) => {
                let shelf = &sections[section];
                let end = (start + GRID_CHUNK).min(shelf.items.len());
                h_flex()
                    .flex_wrap()
                    .gap_4()
                    .pb_4()
                    .children(
                        (start..end).map(|i| {
                            items::card(format!("grid-{section}-{i}"), &shelf.items[i], cx)
                        }),
                    )
                    .into_any_element()
            }
            Row::MoreItems(section) => {
                if let Route::Browse(target) = &self.route {
                    self.store.load_more_items(target.clone(), section);
                }
                loading_row()
            }
            Row::End => {
                if content.has_more() || self.loading_more {
                    loading_row()
                } else {
                    div()
                        .h(px(32. + bottom_accessory_clearance()))
                        .into_any_element()
                }
            }
        };
        // Content sits on a 32 pt gutter; list rows start 12 pt further out so their hover wash
        // surrounds the art while art and titles still line up with the headings.
        let (inset, top, bottom) = match row {
            Row::LargeTitle => (PAGE_INSET, 20., 4.),
            Row::Header => (PAGE_INSET, 24., 24.),
            Row::Chips => (PAGE_INSET, 16., 0.),
            Row::Title(_) => (PAGE_INSET, 28., 8.),
            Row::Item(..) => (PAGE_INSET - ROW_INSET, 0., 0.),
            _ => (PAGE_INSET, 0., 0.),
        };
        div()
            .w_full()
            .px(px(inset))
            .pt(px(top))
            .pb(px(bottom))
            .child(body)
            .into_any_element()
    }

    fn chips_of(&self, content: &Content) -> (Vec<Chip>, Rc<dyn Fn(&Chip, &mut Window, &mut App)>) {
        match (content, &self.route) {
            (Content::Search(_), Route::Search(key)) => {
                let query = key.query.clone();
                let chips = SEARCH_FILTERS
                    .iter()
                    .map(|(title, filter)| Chip {
                        title: (*title).into(),
                        params: String::new(),
                        selected: *filter == key.filter,
                    })
                    .collect();
                (
                    chips,
                    Rc::new(move |chip: &Chip, _, cx: &mut App| {
                        let filter = SEARCH_FILTERS
                            .iter()
                            .find(|(t, _)| *t == chip.title)
                            .and_then(|(_, f)| *f);
                        root::navigate_replace(
                            Route::Search(SearchKey {
                                query: query.clone(),
                                filter,
                            }),
                            cx,
                        );
                    }),
                )
            }
            (Content::Page(page), Route::Browse(target)) => {
                let target = target.clone();
                (
                    page.chips.clone(),
                    Rc::new(move |chip: &Chip, _, cx: &mut App| {
                        root::navigate_replace(Route::Browse(chip_target(&target, chip)), cx)
                    }),
                )
            }
            _ => (
                Vec::new(),
                Rc::new(|_: &Chip, _: &mut Window, _: &mut App| {}),
            ),
        }
    }

    /// A row chosen in the list: a track plays, anything else opens. The selection is cleared
    /// again, so choosing the same row twice acts twice.
    fn select(&mut self, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        self.list.set_selected(Vec::new(), cx);
        let Row::Item(section, item) = row else {
            return;
        };
        let Some(content) = self.content.clone() else {
            return;
        };
        let Some(entry) = content
            .sections()
            .get(section)
            .and_then(|s| s.items.get(item))
            .cloned()
        else {
            return;
        };
        let action = match entry {
            Item::Track(_) => self.play_from(&content, section, item),
            other => items::activate(&other),
        };
        action(window, cx);
    }

    /// A click on a list row: an album or playlist plays whole from that row, a list anywhere
    /// else plays its own tracks from it. The loaded rows go along, so the daemon starts at once.
    fn play_from(&self, content: &Content, section: usize, item: usize) -> items::OnTap {
        let playlist = match (&self.route, content.header()) {
            (
                Route::Browse(BrowseTarget::Album(_) | BrowseTarget::Playlist(_)),
                Some(Header::Detail {
                    playlist_id: Some(id),
                    ..
                }),
            ) if section == 0 => Some(id.clone()),
            _ => None,
        };
        let shelf = &content.sections()[section];
        let tracks: Vec<_> = shelf
            .items
            .iter()
            .filter_map(|i| {
                if let Item::Track(t) = i {
                    Some(t.clone())
                } else {
                    None
                }
            })
            .collect();
        let start = shelf.items[..item]
            .iter()
            .filter(|i| matches!(i, Item::Track(_)))
            .count();
        Rc::new(move |_, cx| {
            let source = match &playlist {
                Some(playlist_id) => PlaySource::Playlist {
                    playlist_id: playlist_id.clone(),
                    tracks: tracks.clone(),
                },
                None => PlaySource::Tracks {
                    tracks: tracks.clone(),
                },
            };
            crate::store(cx).play(source, start, false, false);
        })
    }
}

/// The page's side margin, as the old FormalMusic and Music keep it.
const PAGE_INSET: f32 = 32.;
/// How far a list row's own padding reaches in from its hover wash.
const ROW_INSET: f32 = 12.;

fn status_of(has_page: bool, loading: bool, error: Option<String>) -> Status {
    match (has_page, error) {
        (true, _) => Status::Ready,
        (false, Some(error)) if !loading => Status::Failed(error),
        _ => Status::Loading,
    }
}

fn loading_row() -> AnyElement {
    h_flex()
        .h(px(56.))
        .w_full()
        .justify_center()
        .child(spinner())
        .into_any_element()
}

/// What a click on the opening shelves most likely opens: the first two cards of each of the
/// first three carousels.
fn first_targets(content: &Content) -> Vec<BrowseTarget> {
    content
        .sections()
        .iter()
        .filter(|s| s.layout == SectionLayout::Carousel)
        .take(3)
        .flat_map(|s| s.items.iter().filter_map(menus::target_of).take(2))
        .collect()
}

/// The page a chip leads to: Home's mood chips filter Home, a selected chip goes back to the
/// plain page, and the library chips switch tabs.
fn chip_target(current: &BrowseTarget, chip: &Chip) -> BrowseTarget {
    match current {
        BrowseTarget::Library(_) => BrowseTarget::Library(menus::library_tab(&chip.title)),
        _ if chip.selected => BrowseTarget::Home,
        _ => BrowseTarget::HomeChip {
            params: chip.params.clone(),
        },
    }
}

impl Render for PageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.wide = window.viewport_size().width > 1100.;
        match self.sync(cx) {
            Status::Ready => {
                let route = self.route.clone();
                list("page", &self.list, cx.processor(Self::row))
                    .estimated_row_height(px(56.))
                    .on_select(cx.listener(|this, e: &SelectEvent<Row>, window, cx| {
                        this.select(e.key, window, cx)
                    }))
                    .on_near_end(move |_, cx| {
                        let store = crate::store(cx);
                        match &route {
                            Route::Browse(target) => store.load_more(target.clone()),
                            Route::Search(key) => store.search_more(key.clone()),
                        }
                    })
                    .into_any_element()
            }
            Status::Loading => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(spinner())
                .into_any_element(),
            Status::Failed(error) => {
                let signed_out = error.starts_with("Sign in");
                let action = if signed_out {
                    button("page.sign-in", "Sign in")
                        .accent()
                        .on_click(|_, window, cx| crate::sheets::sign_in(window, cx))
                } else {
                    button("page.retry", "Try again")
                        .on_click(cx.listener(|this, _: &ClickEvent, _, _| this.retry()))
                };
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .child(
                        status_page(if signed_out {
                            "This page needs an account"
                        } else {
                            "Could not load this page"
                        })
                        .icon(Icon::TriangleAlert)
                        .description(error),
                    )
                    .child(action)
                    .into_any_element()
            }
        }
    }
}

/// The large title Music puts over its top-level pages; macOS only, where the toolbar carries no
/// page title.
fn large_title(content: &Content) -> Option<&'static str> {
    if !cfg!(target_os = "macos") || content.header().is_some() {
        return None;
    }
    let Content::Page(page) = content else { return None };
    match page.target {
        BrowseTarget::Home | BrowseTarget::HomeChip { .. } => Some("Home"),
        BrowseTarget::Explore => Some("Explore"),
        BrowseTarget::Library(_) => Some("Library"),
        BrowseTarget::History => Some("History"),
        _ => None,
    }
}
