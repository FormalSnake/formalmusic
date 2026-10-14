//! The window: sidebar (Home, Explore, Library, History and your playlists), toolbar with
//! history and search, the page or the expanded player, the player bar, toasts and the search
//! palette. It owns navigation history and every keyboard shortcut.

use std::time::Duration;

use formalmusic_api::{BrowseTarget, Item, LibraryTab, Suggestion};
use formalmusic_core::{ConnectionStatus, Route, SearchKey, StoreEvent, TransportKind};
use nd::prelude::*;

use crate::bridge;
use crate::menus::{self, ItemCommand};
use crate::now_playing::{NowPlayingView, Tab};
use crate::page::PageView;
use crate::player::Player;
use crate::sheets;

actions!(
    formalmusic,
    [
        GoBack,
        GoForward,
        OpenSearch,
        OpenSettings,
        Dismiss,
        ToggleExpanded,
        ToggleQueue,
        ShowShortcuts,
        TogglePlay,
        NextTrack,
        PreviousTrack,
        SeekForward,
        SeekBack,
        SeekForwardSecond,
        SeekBackSecond,
        VolumeUp,
        VolumeDown,
        ToggleMute,
        ToggleShuffle,
        CycleRepeat,
        LikeTrack,
        DislikeTrack,
        GoHome,
        GoExplore,
        GoLibrary,
        NewPlaylist,
        SignIn,
        SignOut,
        ToggleSidebar,
    ]
);

/// Switch to the account at this index of the account list.
#[derive(Clone, PartialEq, Debug, serde::Deserialize, schemars::JsonSchema, Action)]
#[action(namespace = formalmusic)]
pub struct SwitchAccount(pub usize);

pub const CONTEXT: &str = "FormalMusic";
/// Single keys are shortcuts only while nobody is typing into the search palette.
const KEYS: &str = "FormalMusic && !searching";
/// Pages kept alive for back and forward, with their scroll positions.
const KEEP_PAGES: usize = 8;

pub fn bind_keys(cx: &mut App) {
    let secondary = |key: &str| format!("secondary-{key}");
    let mut bindings = vec![
        KeyBinding::new(&secondary("["), GoBack, Some(CONTEXT)),
        KeyBinding::new(&secondary("]"), GoForward, Some(CONTEXT)),
        KeyBinding::new("alt-left", GoBack, Some(CONTEXT)),
        KeyBinding::new("alt-right", GoForward, Some(CONTEXT)),
        KeyBinding::new(&secondary("f"), OpenSearch, Some(CONTEXT)),
        KeyBinding::new(&secondary("k"), OpenSearch, Some(CONTEXT)),
        KeyBinding::new(&secondary(","), OpenSettings, None),
        KeyBinding::new(&secondary("n"), NewPlaylist, Some(CONTEXT)),
        KeyBinding::new("secondary-alt-s", ToggleSidebar, Some(CONTEXT)),
        KeyBinding::new("escape", Dismiss, Some(KEYS)),
    ];
    bindings.extend([
        KeyBinding::new("space", TogglePlay, Some(KEYS)),
        KeyBinding::new(";", TogglePlay, Some(KEYS)),
        KeyBinding::new("j", NextTrack, Some(KEYS)),
        KeyBinding::new("shift-n", NextTrack, Some(KEYS)),
        KeyBinding::new("k", PreviousTrack, Some(KEYS)),
        KeyBinding::new("shift-p", PreviousTrack, Some(KEYS)),
        KeyBinding::new("l", SeekForward, Some(KEYS)),
        KeyBinding::new("shift-right", SeekForward, Some(KEYS)),
        KeyBinding::new("h", SeekBack, Some(KEYS)),
        KeyBinding::new("shift-left", SeekBack, Some(KEYS)),
        KeyBinding::new("shift-l", SeekForwardSecond, Some(KEYS)),
        KeyBinding::new("shift-h", SeekBackSecond, Some(KEYS)),
        KeyBinding::new("=", VolumeUp, Some(KEYS)),
        KeyBinding::new("shift-up", VolumeUp, Some(KEYS)),
        KeyBinding::new("-", VolumeDown, Some(KEYS)),
        KeyBinding::new("shift-down", VolumeDown, Some(KEYS)),
        KeyBinding::new("m", ToggleMute, Some(KEYS)),
        KeyBinding::new("s", ToggleShuffle, Some(KEYS)),
        KeyBinding::new("r", CycleRepeat, Some(KEYS)),
        KeyBinding::new("+", LikeTrack, Some(KEYS)),
        KeyBinding::new("_", DislikeTrack, Some(KEYS)),
        KeyBinding::new("/", OpenSearch, Some(KEYS)),
        KeyBinding::new("q", ToggleQueue, Some(KEYS)),
        KeyBinding::new("f", ToggleExpanded, Some(KEYS)),
        KeyBinding::new("?", ShowShortcuts, Some(KEYS)),
        KeyBinding::new("shift-/", ShowShortcuts, Some(KEYS)),
        KeyBinding::new("g h", GoHome, Some(KEYS)),
        KeyBinding::new("g e", GoExplore, Some(KEYS)),
        KeyBinding::new("g l", GoLibrary, Some(KEYS)),
        KeyBinding::new("g ,", OpenSettings, Some(KEYS)),
        KeyBinding::new("n", NextTrack, Some(KEYS)),
        KeyBinding::new("p", PreviousTrack, Some(KEYS)),
    ]);
    cx.bind_keys(bindings);
    menus(cx, &[]);
}

/// The menu bar; the Account menu lists the accounts to switch to.
fn menus(cx: &mut App, accounts: &[formalmusic_api::Account]) {
    let mut account_items: Vec<MenuItem> = accounts
        .iter()
        .enumerate()
        .map(|(i, a)| MenuItem::action(a.name.clone(), SwitchAccount(i)).checked(a.selected))
        .collect();
    if !account_items.is_empty() {
        account_items.push(MenuItem::separator());
    }
    account_items.push(MenuItem::action("Sign In\u{2026}", SignIn));
    account_items.push(MenuItem::action("Sign Out", SignOut));
    set_menus(
        cx,
        [
            Menu::app([MenuItem::action("Settings\u{2026}", OpenSettings)]),
            Menu::new("File").items([MenuItem::action("New Playlist\u{2026}", NewPlaylist)]),
            Menu::new("View").items([
                MenuItem::action("Search", OpenSearch),
                MenuItem::action("Toggle Sidebar", ToggleSidebar),
                MenuItem::action("Expanded Player", ToggleExpanded),
                MenuItem::action("Up Next", ToggleQueue),
                MenuItem::separator(),
                MenuItem::action("Back", GoBack),
                MenuItem::action("Forward", GoForward),
                MenuItem::action("Home", GoHome),
                MenuItem::action("Explore", GoExplore),
                MenuItem::action("Library", GoLibrary),
            ]),
            Menu::new("Controls").items([
                MenuItem::action("Play or Pause", TogglePlay),
                MenuItem::action("Next", NextTrack),
                MenuItem::action("Previous", PreviousTrack),
                MenuItem::action("Forward 10 Seconds", SeekForward),
                MenuItem::action("Back 10 Seconds", SeekBack),
                MenuItem::separator(),
                MenuItem::action("Volume Up", VolumeUp),
                MenuItem::action("Volume Down", VolumeDown),
                MenuItem::action("Mute", ToggleMute),
                MenuItem::separator(),
                MenuItem::action("Shuffle", ToggleShuffle),
                MenuItem::action("Repeat", CycleRepeat),
                MenuItem::action("Like", LikeTrack),
                MenuItem::action("Dislike", DislikeTrack),
            ]),
            Menu::new("Account").items(account_items),
            Menu::help([MenuItem::action("Keyboard Shortcuts", ShowShortcuts)]),
        ],
    );
}

/// The window's root, reachable from anywhere for navigation.
struct RootHandle(WeakEntity<Root>);

fn root(cx: &App) -> Option<WeakEntity<Root>> {
    cx.try_global::<RootHandle>().map(|h| h.0.clone())
}

pub fn navigate(route: Route, cx: &mut App) {
    if let Some(root) = root(cx) {
        root.update(cx, |this, cx| this.navigate(route, false, cx));
    }
}

/// Navigates without a history entry, for chips and search filters.
pub fn navigate_replace(route: Route, cx: &mut App) {
    if let Some(root) = root(cx) {
        root.update(cx, |this, cx| this.navigate(route, true, cx));
    }
}

/// Opens or closes the expanded player on the tab it last showed.
pub fn toggle_expanded(cx: &mut App) {
    if let Some(root) = root(cx) {
        root.update(cx, |this, cx| {
            let tab = if this.expanded {
                None
            } else {
                Some(this.now_playing.read(cx).tab)
            };
            this.set_expanded(tab, cx);
        });
    }
}

pub fn dismiss_sign_in(cx: &mut App) {
    if let Some(root) = root(cx) {
        root.update(cx, |this, cx| {
            this.sign_in_dismissed = true;
            cx.notify();
        });
    }
}

/// Closes every sheet open over the main window.
pub fn close_sheets(cx: &mut App) {
    let sheets: Vec<_> = cx
        .window_ids()
        .filter(|id| {
            cx.window(*id)
                .is_some_and(|w| matches!(w.options().kind, WindowKind::Sheet { .. }))
        })
        .collect();
    for id in sheets {
        cx.close_window(id);
    }
}

pub struct Root {
    history: Vec<Route>,
    cursor: usize,
    pages: Vec<Entity<PageView>>,
    player: Entity<Player>,
    now_playing: Entity<NowPlayingView>,
    expanded: bool,
    palette_open: bool,
    query: SharedString,
    toasts: Vec<Toast>,
    shown_notice: u64,
    sign_in_dismissed: bool,
    sign_in: Option<Entity<sheets::SignIn>>,
    sidebar_collapsed: bool,
    accounts: Vec<formalmusic_api::Account>,
    pictures: crate::video::Pictures,
}

impl Root {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let weak = cx.weak_entity();
        cx.set_global(RootHandle(weak));
        let id = cx.entity_id();
        for topic in [
            StoreEvent::Session,
            StoreEvent::Connection,
            StoreEvent::Notice,
            StoreEvent::Accounts,
            StoreEvent::Suggestions,
            StoreEvent::NowPlaying,
            StoreEvent::Page(BrowseTarget::Library(LibraryTab::Playlists)),
        ] {
            bridge::watch(cx, topic, id);
        }
        crate::store(cx).open(BrowseTarget::Library(LibraryTab::Playlists));
        crate::store(cx).load_accounts();
        let pictures = crate::video::Pictures {
            covers: cx.new(crate::video::Covers::new),
            music: cx.new(crate::video::MusicVideo::new),
        };
        cx.set_global(pictures.clone());
        let home = Route::Browse(BrowseTarget::Home);
        let page = cx.new(|cx| PageView::new(home.clone(), cx));
        Self {
            history: vec![home],
            cursor: 0,
            pages: vec![page],
            player: cx.new(Player::new),
            now_playing: cx.new(|cx| NowPlayingView::new(Tab::UpNext, cx)),
            expanded: false,
            palette_open: false,
            query: SharedString::default(),
            toasts: Vec::new(),
            shown_notice: 0,
            sign_in_dismissed: false,
            sign_in: None,
            sidebar_collapsed: false,
            accounts: Vec::new(),
            pictures,
        }
    }

    pub fn route(&self) -> &Route {
        &self.history[self.cursor]
    }

    pub fn expanded(&self) -> bool {
        self.expanded
    }

    fn page(&mut self, route: &Route, cx: &mut Context<Self>) -> Entity<PageView> {
        if let Some(at) = self.pages.iter().position(|p| p.read(cx).route() == route) {
            let page = self.pages.remove(at);
            self.pages.push(page.clone());
            page.read(cx).fetch();
            return page;
        }
        let page = cx.new(|cx| PageView::new(route.clone(), cx));
        self.pages.push(page.clone());
        if self.pages.len() > KEEP_PAGES {
            self.pages.remove(0);
        }
        page
    }

    pub fn navigate(&mut self, route: Route, replace: bool, cx: &mut Context<Self>) {
        self.expanded = false;
        if *self.route() == route {
            let page = self.page(&route, cx);
            page.update(cx, |page, cx| page.scroll_to_top(cx));
            cx.notify();
            return;
        }
        if replace {
            self.history[self.cursor] = route.clone();
        } else {
            self.history.truncate(self.cursor + 1);
            self.history.push(route.clone());
            self.cursor = self.history.len() - 1;
        }
        self.page(&route, cx);
        cx.notify();
    }

    fn go(&mut self, delta: isize, cx: &mut Context<Self>) {
        let next = self.cursor as isize + delta;
        if next < 0 || next as usize >= self.history.len() {
            return;
        }
        self.cursor = next as usize;
        self.expanded = false;
        let route = self.route().clone();
        self.page(&route, cx);
        cx.notify();
    }

    /// Opens the expanded player on `tab`, or closes it with `None`.
    pub fn set_expanded(&mut self, tab: Option<Tab>, cx: &mut Context<Self>) {
        match tab {
            Some(tab) => {
                self.expanded = true;
                self.now_playing.update(cx, |np, cx| np.set_tab(tab, cx));
            }
            None => self.expanded = false,
        }
        cx.notify();
    }

    fn sidebar_nodes(&self, cx: &App) -> Vec<SourceNode> {
        let store = crate::store(cx);
        let state = store.state();
        let mut nodes = vec![
            source_node("nav.home", "Home").icon(Icon::Music),
            source_node("nav.explore", "Explore").icon(Icon::Globe),
            source_node("nav.library", "Library").icon(Icon::Archive),
            source_node("nav.history", "History").icon(Icon::Clock),
            source_section("playlists", "Playlists"),
            source_node("nav.new-playlist", "New playlist")
                .parent("playlists")
                .icon(Icon::Plus),
        ];
        if state.signed_in() {
            for item in state.library_playlists() {
                if let Item::Playlist {
                    playlist_id, title, ..
                } = item
                {
                    nodes.push(
                        source_node(format!("playlist:{playlist_id}"), title.clone())
                            .parent("playlists")
                            .icon(Icon::FileText),
                    );
                }
            }
        } else {
            nodes.push(
                source_node("nav.signed-out", "Sign in to see your playlists here.")
                    .parent("playlists")
                    .selectable(false),
            );
        }
        nodes
    }

    fn selected_node(&self) -> Option<SharedString> {
        Some(
            match self.route() {
                Route::Browse(BrowseTarget::Home | BrowseTarget::HomeChip { .. }) => {
                    "nav.home".to_owned()
                }
                Route::Browse(
                    BrowseTarget::Explore
                    | BrowseTarget::NewReleases
                    | BrowseTarget::Charts
                    | BrowseTarget::MoodsAndGenres,
                ) => "nav.explore".to_owned(),
                Route::Browse(BrowseTarget::Library(_)) => "nav.library".to_owned(),
                Route::Browse(BrowseTarget::History) => "nav.history".to_owned(),
                Route::Browse(BrowseTarget::Playlist(id)) => format!("playlist:{id}"),
                _ => return None,
            }
            .into(),
        )
    }

    fn sidebar_select(&mut self, e: &SourceSelected, window: &mut Window, cx: &mut Context<Self>) {
        let route = match e.id.as_ref() {
            "nav.home" => Route::Browse(BrowseTarget::Home),
            "nav.explore" => Route::Browse(BrowseTarget::Explore),
            "nav.library" => Route::Browse(BrowseTarget::Library(LibraryTab::Playlists)),
            "nav.history" => Route::Browse(BrowseTarget::History),
            "nav.new-playlist" => {
                sheets::new_playlist(window, cx);
                cx.notify();
                return;
            }
            other => match other.strip_prefix("playlist:") {
                Some(id) => Route::Browse(BrowseTarget::Playlist(id.to_owned())),
                None => return,
            },
        };
        self.navigate(route, false, cx);
    }

    /// The palette's rows: what the daemon suggests for the query, history first.
    fn palette_items(&self, cx: &App) -> Vec<PaletteItem> {
        let store = crate::store(cx);
        let state = store.state();
        if state.suggestions.query != self.query.as_ref() {
            return Vec::new();
        }
        let q = self.query.to_lowercase();
        state
            .suggestions
            .items
            .iter()
            .enumerate()
            .map(|(i, s)| match s {
                Suggestion::Query { text, from_history } => {
                    palette_item(format!("q:{i}"), text.clone())
                        .icon(if *from_history {
                            Icon::Clock
                        } else {
                            Icon::Search
                        })
                        .when(!q.is_empty() && text.to_lowercase().starts_with(&q), |p| {
                            p.completion(text.clone())
                        })
                }
                Suggestion::Item(item) => {
                    let (title, kind) = match item {
                        Item::Track(t) => (t.title.clone(), "Song"),
                        Item::Album { title, .. } => (title.clone(), "Album"),
                        Item::Artist { name, .. } => (name.clone(), "Artist"),
                        Item::Playlist { title, .. } => (title.clone(), "Playlist"),
                        Item::Podcast { title, .. } => (title.clone(), "Podcast"),
                        Item::Mood { title, .. } | Item::Shortcut { title, .. } => {
                            (title.clone(), "")
                        }
                    };
                    palette_item(format!("i:{i}"), title)
                        .subtitle(kind)
                        .icon(Icon::Music)
                }
            })
            .collect()
    }

    fn search(&mut self, query: &str, cx: &mut Context<Self>) {
        let query = query.trim();
        if query.is_empty() {
            return;
        }
        self.palette_open = false;
        self.navigate(
            Route::Search(SearchKey {
                query: query.to_owned(),
                filter: None,
            }),
            false,
            cx,
        );
    }

    fn palette_activate(
        &mut self,
        e: &PaletteActivated,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = crate::store(cx);
        let index: Option<usize> = e.id.get(2..).and_then(|i| i.parse().ok());
        let suggestion = index.and_then(|i| store.state().suggestions.items.get(i).cloned());
        self.palette_open = false;
        match suggestion {
            Some(Suggestion::Query { text, .. }) => self.search(&text, cx),
            Some(Suggestion::Item(item)) => match menus::target_of(&item) {
                Some(target) => self.navigate(Route::Browse(target), false, cx),
                None => menus::play_item(&item, &store, false),
            },
            None => {}
        }
        cx.notify();
    }

    /// Notices from the store become toasts; each clears itself after five seconds.
    fn sync_notice(&mut self, cx: &mut Context<Self>) {
        let notice = crate::store(cx).state().notice.clone();
        let Some(notice) = notice else { return };
        if notice.seq == self.shown_notice {
            return;
        }
        self.shown_notice = notice.seq;
        let seq = notice.seq;
        self.toasts.push(
            toast(format!("notice-{seq}"), notice.text.clone())
                .timeout(Duration::from_secs(5))
                .when(notice.failure, |t| t.high_priority()),
        );
        bridge::after(cx, Duration::from_secs(5), move |cx| {
            crate::store(cx).clear_notice(seq)
        });
    }

    fn signing_in(&mut self, cx: &mut Context<Self>) -> Option<Entity<sheets::SignIn>> {
        let store = crate::store(cx);
        let prompt = {
            let state = store.state();
            store.kind() == TransportKind::Daemon
                && state.connection == ConnectionStatus::Online
                && state.session.as_ref().is_some_and(|s| !s.signed_in)
        };
        if !prompt || self.sign_in_dismissed {
            self.sign_in = None;
            return None;
        }
        Some(
            self.sign_in
                .get_or_insert_with(|| cx.new(sheets::SignIn::new))
                .clone(),
        )
    }

    /// Asks for the moving pictures the window shows now: big in the expanded player, small
    /// in the bar when the settings want it there.
    fn want_pictures(&mut self, cx: &mut Context<Self>) {
        let video = crate::store(cx).state().player.mode == formalmusic_api::PlaybackMode::Video;
        let in_bar = crate::settings::current(cx).window.animated_cover_in_bar;
        let cover = if self.expanded {
            Some(480.)
        } else if in_bar {
            Some(44.)
        } else {
            None
        };
        let music = (self.expanded && video).then_some(480.);
        self.pictures
            .covers
            .update(cx, |c, cx| c.set_desired(cover, cx));
        self.pictures
            .music
            .update(cx, |m, cx| m.set_desired(music, cx));
    }

    fn content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(sign_in) = self.signing_in(cx) {
            return div().size_full().child(sign_in).into_any_element();
        }
        if self.expanded {
            return div()
                .size_full()
                .child(self.now_playing.clone())
                .into_any_element();
        }
        let route = self.route().clone();
        let page = self.page(&route, cx);
        div().size_full().child(page).into_any_element()
    }

    fn handlers(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &GoBack, _, cx| this.go(-1, cx)))
            .on_action(cx.listener(|this, _: &GoForward, _, cx| this.go(1, cx)))
            .on_action(cx.listener(|this, _: &OpenSearch, _, cx| {
                this.palette_open = true;
                cx.notify();
            }))
            .on_action(|_: &OpenSettings, _, cx| crate::settings::open(cx))
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| {
                if this.expanded {
                    this.set_expanded(None, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleExpanded, _, cx| {
                let tab = if this.expanded {
                    None
                } else {
                    Some(this.now_playing.read(cx).tab)
                };
                this.set_expanded(tab, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleQueue, _, cx| {
                let open_on_queue = this.expanded && this.now_playing.read(cx).tab == Tab::UpNext;
                this.set_expanded(
                    if open_on_queue {
                        None
                    } else {
                        Some(Tab::UpNext)
                    },
                    cx,
                );
            }))
            .on_action(|_: &ShowShortcuts, window, cx| sheets::shortcuts(window, cx))
            .on_action(|_: &TogglePlay, _, cx| crate::store(cx).toggle())
            .on_action(|_: &NextTrack, _, cx| crate::store(cx).next())
            .on_action(|_: &PreviousTrack, _, cx| crate::store(cx).previous())
            .on_action(|_: &SeekForward, _, cx| {
                crate::store(cx).seek_by(formalmusic_core::store::SEEK_STEP_MS as i64)
            })
            .on_action(|_: &SeekBack, _, cx| {
                crate::store(cx).seek_by(-(formalmusic_core::store::SEEK_STEP_MS as i64))
            })
            .on_action(|_: &SeekForwardSecond, _, cx| crate::store(cx).seek_by(1000))
            .on_action(|_: &SeekBackSecond, _, cx| crate::store(cx).seek_by(-1000))
            .on_action(|_: &VolumeUp, _, cx| step_volume(1., cx))
            .on_action(|_: &VolumeDown, _, cx| step_volume(-1., cx))
            .on_action(|_: &ToggleMute, _, cx| {
                let store = crate::store(cx);
                let muted = store.state().player.muted;
                store.set_muted(!muted);
            })
            .on_action(|_: &ToggleShuffle, _, cx| crate::store(cx).toggle_shuffle())
            .on_action(|_: &CycleRepeat, _, cx| crate::store(cx).cycle_repeat())
            .on_action(|_: &LikeTrack, _, cx| rate_current(formalmusic_api::Rating::Like, cx))
            .on_action(|_: &DislikeTrack, _, cx| rate_current(formalmusic_api::Rating::Dislike, cx))
            .on_action(cx.listener(|this, _: &GoHome, _, cx| {
                this.navigate(Route::Browse(BrowseTarget::Home), false, cx)
            }))
            .on_action(cx.listener(|this, _: &GoExplore, _, cx| {
                this.navigate(Route::Browse(BrowseTarget::Explore), false, cx)
            }))
            .on_action(cx.listener(|this, _: &GoLibrary, _, cx| {
                this.navigate(
                    Route::Browse(BrowseTarget::Library(LibraryTab::Playlists)),
                    false,
                    cx,
                )
            }))
            .on_action(|_: &NewPlaylist, window, cx| sheets::new_playlist(window, cx))
            .on_action(|_: &SignIn, window, cx| sheets::sign_in(window, cx))
            .on_action(|_: &SignOut, _, cx| crate::store(cx).sign_out())
            .on_action(
                cx.listener(|this, SwitchAccount(i): &SwitchAccount, _, cx| {
                    if let Some(account) = this.accounts.get(*i) {
                        crate::store(cx).switch_account(account.page_id.clone());
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.sidebar_collapsed = !this.sidebar_collapsed;
                cx.notify();
            }))
            .on_action(cx.listener(|this, command: &ItemCommand, _, cx| {
                if let Some(route) = menus::run(command, cx) {
                    this.navigate(route, false, cx);
                }
            }))
    }
}

fn step_volume(steps: f32, cx: &mut App) {
    let store = crate::store(cx);
    let volume = store.state().player.volume;
    store.set_volume((volume + steps * formalmusic_core::store::VOLUME_STEP).clamp(0., 1.));
}

/// Like or dislike the playing track; the same key again takes it back.
fn rate_current(rating: formalmusic_api::Rating, cx: &mut App) {
    let store = crate::store(cx);
    let Some(track) = store.state().player.track.clone() else {
        return;
    };
    let current = store.state().rating(&track);
    store.rate(
        &track,
        if current == rating {
            formalmusic_api::Rating::Indifferent
        } else {
            rating
        },
    );
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_notice(cx);
        let accounts = crate::store(cx).state().accounts.clone();
        if accounts != self.accounts {
            menus(cx, &accounts);
            self.accounts = accounts;
        }
        let store = crate::store(cx);
        let offline = store.kind() == TransportKind::Daemon
            && store.state().connection == ConnectionStatus::Offline;
        let reason = store
            .state()
            .connection_error
            .clone()
            .unwrap_or_else(|| "The music daemon is not answering.".into());
        let palette_items = self.palette_items(cx);
        let content = self.content(cx);
        self.want_pictures(cx);
        let decoders = div()
            .size_0()
            .child(self.pictures.covers.clone())
            .child(self.pictures.music.clone());
        let (back, forward) = (self.cursor > 0, self.cursor + 1 < self.history.len());
        // Actions are handled on a box around the scaffold: with nothing focused nd dispatches
        // from the view's root node, which the scaffold's own handlers sit below.
        let root = div().size_full().key_context(if self.palette_open {
            "FormalMusic searching"
        } else {
            CONTEXT
        });
        self.handlers(root, cx).child(
            scaffold()
                .toolbar(
                    toolbar()
                        .title("FormalMusic")
                        .sidebar_toggle()
                        .item(
                            icon_button("toolbar.back", Icon::ChevronLeft)
                                .tooltip("Back")
                                .disabled(!back)
                                .on_click(
                                    cx.listener(|this, _: &ClickEvent, _, cx| this.go(-1, cx)),
                                ),
                        )
                        .item(
                            icon_button("toolbar.forward", Icon::ChevronRight)
                                .tooltip("Forward")
                                .disabled(!forward)
                                .on_click(
                                    cx.listener(|this, _: &ClickEvent, _, cx| this.go(1, cx)),
                                ),
                        )
                        .flexible_space()
                        .item(
                            icon_button("toolbar.search", Icon::Search)
                                .tooltip("Search")
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.palette_open = true;
                                    cx.notify();
                                })),
                        )
                        .item(
                            icon_button("toolbar.settings", Icon::Settings)
                                .tooltip("Settings")
                                .on_click(|_, _, cx| crate::settings::open(cx)),
                        ),
                )
                .sidebar(
                    sidebar()
                        .collapsed(self.sidebar_collapsed)
                        .min_w(px(200.))
                        .child(
                            source_tree("sidebar")
                                .nodes(self.sidebar_nodes(cx))
                                .selected_opt(self.selected_node())
                                .on_select(cx.listener(Self::sidebar_select)),
                        ),
                )
                .content(
                    v_flex()
                        .size_full()
                        .child(
                            banner(
                                "offline",
                                format!("{reason} Showing what was cached; retrying."),
                            )
                            .revealed(offline),
                        )
                        .child(div().flex_1().min_h_0().w_full().child(content))
                        .child(separator())
                        .child(self.player.clone())
                        .child(decoders)
                        .child(
                            toast_overlay("toasts")
                                .toasts(self.toasts.clone())
                                .on_dismiss(cx.listener(|this, e: &ToastDismissed, _, cx| {
                                    this.toasts.retain(|t| t.id != e.id);
                                    cx.notify();
                                })),
                        ),
                )
                .overlay(
                    command_palette("search")
                        .open(self.palette_open)
                        .placeholder("Search songs, albums, artists, podcasts")
                        .query(self.query.clone())
                        .items(palette_items)
                        .on_query_change(cx.listener(|this, e: &PaletteQueryChanged, _, cx| {
                            this.query = e.text.clone();
                            crate::store(cx).suggest(&e.text);
                            cx.notify();
                        }))
                        .on_activate(cx.listener(Self::palette_activate))
                        .on_submit(cx.listener(|this, e: &PaletteSubmitted, _, cx| {
                            let text = e.text.clone();
                            this.search(&text, cx);
                            cx.notify();
                        }))
                        .on_cancel(cx.listener(|this, _: &PaletteCancelled, _, cx| {
                            this.palette_open = false;
                            cx.notify();
                        })),
                ),
        )
    }
}
