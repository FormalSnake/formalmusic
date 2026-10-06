//! The root view. Builds the store, lays out sidebar | top bar over page |
//! player bar, keeps the back and forward history, and owns the overlays
//! (expanded player, sign-in, new playlist, menu, toast) and their Escape order.

use std::sync::Arc;

use formalmusic_api::BrowseTarget;
use formalmusic_core::art::ArtCache;
use formalmusic_core::cache::StateCache;
use formalmusic_core::store::VOLUME_STEP;
use formalmusic_core::{ConnectionStatus, MusicStore, Route, StoreOptions, TransportKind, paths};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::bridge::{Bridge, Topic};
use crate::menus::{ContextMenu, MenuRequest};
use crate::motion::{self, DURATION_BASE, DURATION_FAST, DURATION_PANEL, Presence};
use crate::new_playlist::NewPlaylist;
use crate::now_playing::{NowPlaying, Tab};
use crate::page::PageView;
use crate::player_bar::PlayerBar;
use crate::settings::Settings;
use crate::sidebar::Sidebar;
use crate::signin::SignIn;
use crate::theme::{PLAYER_HEIGHT, SIDEBAR_COLLAPSED, SIDEBAR_WIDTH, TITLEBAR_HEIGHT, Theme};
use crate::topbar::TopBar;

const CONTEXT: &str = "App";
/// Pages kept alive for back and forward, with their scroll positions.
const KEPT_PAGES: usize = 8;
const TOAST_FOR: std::time::Duration = std::time::Duration::from_secs(5);
const PAUSE_ON_CLOSE: std::time::Duration = std::time::Duration::from_millis(500);

actions!(
    app,
    [
        GoBack,
        GoForward,
        FocusSearch,
        Dismiss,
        OpenSettings,
        ToggleFrameOverlay
    ]
);

/// Bindings with a modifier. The web app's single-key shortcuts are handled
/// in `on_key_down` instead, where they can stand down while a field has focus.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-[", GoBack, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-]", GoForward, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-f", FocusSearch, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-f", FocusSearch, Some(CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-,", OpenSettings, Some(CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-,", OpenSettings, Some(CONTEXT)),
        KeyBinding::new("alt-left", GoBack, Some(CONTEXT)),
        KeyBinding::new("alt-right", GoForward, Some(CONTEXT)),
        KeyBinding::new("escape", Dismiss, Some(CONTEXT)),
        #[cfg(feature = "frame-overlay")]
        KeyBinding::new("f12", ToggleFrameOverlay, Some(CONTEXT)),
    ]);
}

/// Weak handle to the window's root, for views that navigate or open a menu
/// without being handed the root at construction.
pub struct RootHandle(pub WeakEntity<AppRoot>);

impl Global for RootHandle {}

pub fn root(cx: &App) -> Option<Entity<AppRoot>> {
    cx.try_global::<RootHandle>()
        .and_then(|handle| handle.0.upgrade())
}

pub fn navigate(route: Route, cx: &mut App) {
    if let Some(root) = root(cx) {
        root.update(cx, |this, cx| this.navigate(route, false, cx));
    }
}

/// Swaps the current page without a history entry: chips and filters.
pub fn navigate_replace(route: Route, cx: &mut App) {
    if let Some(root) = root(cx) {
        root.update(cx, |this, cx| this.navigate(route, true, cx));
    }
}

pub fn go_back(_window: &mut Window, cx: &mut App) {
    if let Some(root) = root(cx) {
        root.update(cx, |this, cx| this.step_history(-1, cx));
    }
}

pub fn go_forward(_window: &mut Window, cx: &mut App) {
    if let Some(root) = root(cx) {
        root.update(cx, |this, cx| this.step_history(1, cx));
    }
}

pub fn toggle_expanded(_window: &mut Window, cx: &mut App) {
    if let Some(root) = root(cx) {
        root.update(cx, |this, cx| {
            let open = this.expanded.is_none();
            this.set_expanded(open.then_some(Tab::UpNext), cx);
        });
    }
}

pub fn show_sign_in(cx: &mut App) {
    if let Some(root) = root(cx) {
        root.update(cx, |this, cx| {
            this.sign_in_wanted = true;
            cx.notify();
        });
    }
}

pub fn new_playlist(window: &mut Window, cx: &mut App) {
    let Some(root) = root(cx) else { return };
    let weak = root.downgrade();
    let store = root.read(cx).store.clone();
    let close = move |window: &mut Window, cx: &mut App| {
        let _ = weak.update(cx, |this, cx| {
            this.new_playlist = None;
            this.root_focus.focus(window, cx);
            cx.notify();
        });
    };
    let dialog = cx.new(|cx| NewPlaylist::new(store, close, window, cx));
    root.update(cx, |this, cx| {
        this.menu = None;
        this.new_playlist = Some(dialog);
        cx.notify();
    });
}

pub fn show_settings(window: &mut Window, cx: &mut App) {
    if let Some(root) = root(cx) {
        root.update(cx, |this, cx| this.open_settings(window, cx));
    }
}

pub struct AppRoot {
    store: MusicStore,
    history: Vec<Route>,
    cursor: usize,
    /// Most recently used last.
    pages: Vec<Entity<PageView>>,
    sidebar: Entity<Sidebar>,
    topbar: Entity<TopBar>,
    player_bar: Entity<PlayerBar>,
    expanded: Option<Entity<NowPlaying>>,
    expanded_shown: Presence<Entity<NowPlaying>>,
    /// The expanded player has finished opening over the sidebar, top bar
    /// and page, so they are left out of the frame: each of its frames would
    /// otherwise replay every one of their primitives underneath it.
    covered: bool,
    sign_in: Option<Entity<SignIn>>,
    /// Opened from a menu or a page, as opposed to shown because nobody is signed in.
    sign_in_wanted: bool,
    /// "Browse without signing in" was chosen this session.
    sign_in_dismissed: bool,
    new_playlist: Option<Entity<NewPlaylist>>,
    settings: Option<Entity<Settings>>,
    menu: Option<Entity<ContextMenu>>,
    /// The `?` overlay is open.
    shortcuts: bool,
    /// When a `g` was pressed, while it waits for the rest of its chord.
    chord_at: Option<std::time::Instant>,
    toast: Option<(u64, SharedString)>,
    toast_shown: Presence<SharedString>,
    root_focus: FocusHandle,
}

pub fn demo() -> bool {
    std::env::var("FORMALMUSIC_DEMO").as_deref() == Ok("1")
}

/// Where `state.json` lives. The demo keeps its own beside the real one, so
/// a demo run paints from cache like a real one without touching it.
pub fn state_dir() -> std::path::PathBuf {
    if demo() {
        paths::cache_dir().join("demo")
    } else {
        paths::cache_dir()
    }
}

fn build_store(
    runtime: &tokio::runtime::Handle,
    preloaded: Option<formalmusic_core::cache::CachedState>,
) -> MusicStore {
    let demo = demo();
    let http = reqwest::Client::builder()
        .user_agent(concat!("formalmusic/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default();
    let art = Arc::new(ArtCache::new(paths::art_dir(), http));
    let transport: Arc<dyn formalmusic_core::Transport> = if demo {
        Arc::new(formalmusic_core::demo::DemoTransport::new())
    } else {
        Arc::new(formalmusic_core::client::DaemonClient::new(
            formalmusic_api::socket_path(),
        ))
    };
    let cache = Some(Arc::new(StateCache::new(&state_dir())));
    MusicStore::new(
        transport,
        StoreOptions {
            cache,
            art,
            preloaded,
        },
        runtime.clone(),
    )
}

impl AppRoot {
    pub fn new(
        runtime: tokio::runtime::Handle,
        preload: Option<std::thread::JoinHandle<Option<formalmusic_core::cache::CachedState>>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let weak = cx.entity().downgrade();
        cx.set_global(RootHandle(weak.clone()));
        motion::install(cx);
        #[cfg(feature = "frame-overlay")]
        match std::env::var("FORMALMUSIC_FRAME_OVERLAY").ok().as_deref() {
            Some("full") => window.set_debug_frame_overlay_mode(DebugFrameOverlayMode::Full),
            Some("minimal") => window.set_debug_frame_overlay_mode(DebugFrameOverlayMode::Minimal),
            _ => {}
        }
        let store = {
            let _guard = runtime.enter();
            let preloaded = preload.and_then(|thread| thread.join().ok()).flatten();
            crate::trace::log_if_enabled(&format!(
                "cache read, {} pages",
                preloaded.as_ref().map_or(0, |cached| cached.pages.len())
            ));
            build_store(&runtime, preloaded)
        };
        Bridge::drain(cx, store.clone());
        // Closing the last window pauses, unless the settings say to keep
        // playing. It has to reach the daemon before the process exits, so it
        // waits for the answer instead of going through the store's queue.
        let closing = store.clone();
        cx.on_window_closed(move |cx, _| {
            if cx.windows().is_empty()
                && !formalmusic_core::settings::Settings::load(&paths::settings_file())
                    .keep_playing_when_closed
            {
                closing.pause_blocking(PAUSE_ON_CLOSE);
            }
        })
        .detach();
        let starting = store.clone();
        store.spawn(async move { starting.start().await });
        for topic in [Topic::Session, Topic::Connection, Topic::Notice] {
            Bridge::watch(cx, topic, weak.clone().into());
        }

        let sidebar = cx.new(|cx| Sidebar::new(store.clone(), cx));
        let topbar = cx.new(|cx| TopBar::new(store.clone(), window, cx));
        let player_bar = cx.new(|cx| PlayerBar::new(store.clone(), cx));
        let root_focus = cx.focus_handle();
        root_focus.focus(window, cx);

        let mut this = Self {
            store,
            history: Vec::new(),
            cursor: 0,
            pages: Vec::new(),
            sidebar,
            topbar,
            player_bar,
            expanded: None,
            expanded_shown: Presence::new(DURATION_BASE),
            covered: false,
            sign_in: None,
            sign_in_wanted: false,
            sign_in_dismissed: false,
            new_playlist: None,
            settings: None,
            menu: None,
            shortcuts: false,
            chord_at: None,
            toast: None,
            toast_shown: Presence::new(DURATION_FAST),
            root_focus,
        };
        this.navigate(Route::Browse(BrowseTarget::Home), false, cx);
        match std::env::var("FORMALMUSIC_TOUR").as_deref() {
            Ok("1") => tour(window, cx),
            Ok("lyrics") => player_tour(Some(Tab::Lyrics), true, window, cx),
            Ok("cover") => player_tour(Some(Tab::UpNext), true, window, cx),
            Ok("bar") => player_tour(None, true, window, cx),
            Ok("expanded") => player_tour(Some(Tab::UpNext), false, window, cx),
            _ => {}
        }
        #[cfg(feature = "screenshot")]
        if let Ok(out) = std::env::var("FORMALMUSIC_SCREENSHOT") {
            screenshot(out.into(), window, cx);
        }
        this
    }

    pub(crate) fn current_route(&self) -> Option<&Route> {
        self.history.get(self.cursor)
    }

    fn page_for(&mut self, route: &Route, cx: &mut Context<Self>) -> Entity<PageView> {
        if let Some(index) = self
            .pages
            .iter()
            .position(|page| page.read(cx).route() == route)
        {
            let page = self.pages.remove(index);
            page.read(cx).fetch();
            self.pages.push(page.clone());
            return page;
        }
        let store = self.store.clone();
        let route = route.clone();
        let page = cx.new(|cx| PageView::new(route, store, cx));
        self.pages.push(page.clone());
        if self.pages.len() > KEPT_PAGES {
            self.pages.remove(0);
        }
        page
    }

    fn navigate(&mut self, route: Route, replace: bool, cx: &mut Context<Self>) {
        self.menu = None;
        if self.expanded.is_some() {
            self.set_expanded(None, cx);
        }
        if self.current_route() == Some(&route) {
            if let Some(page) = self
                .pages
                .iter()
                .find(|page| page.read(cx).route() == &route)
            {
                page.read(cx).scroll_to_top();
                page.update(cx, |_, cx| cx.notify());
            }
            return;
        }
        if replace && !self.history.is_empty() {
            self.history[self.cursor] = route.clone();
        } else {
            self.history.truncate(self.cursor + 1);
            self.history.push(route.clone());
            self.cursor = self.history.len() - 1;
        }
        self.show_route(route, cx);
    }

    fn step_history(&mut self, delta: isize, cx: &mut Context<Self>) {
        let next = self.cursor as isize + delta;
        if next < 0 || next as usize >= self.history.len() {
            return;
        }
        self.cursor = next as usize;
        if self.expanded.is_some() {
            self.set_expanded(None, cx);
        }
        let route = self.history[self.cursor].clone();
        self.show_route(route, cx);
    }

    fn show_route(&mut self, route: Route, cx: &mut Context<Self>) {
        crate::trace::stamp("navigate");
        self.page_for(&route, cx);
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_route(route.clone(), cx));
        let (can_back, can_forward) = (self.cursor > 0, self.cursor + 1 < self.history.len());
        self.topbar.update(cx, |topbar, cx| {
            topbar.set_history(can_back, can_forward, cx)
        });
        cx.notify();
    }

    fn set_expanded(&mut self, tab: Option<Tab>, cx: &mut Context<Self>) {
        let was_open = self.expanded.is_some();
        match tab {
            Some(tab) => {
                if self.store.state().player.track.is_none() {
                    return;
                }
                match &self.expanded {
                    Some(view) => view.update(cx, |view, cx| view.set_tab(tab, cx)),
                    None => {
                        let store = self.store.clone();
                        self.expanded = Some(cx.new(|cx| NowPlaying::new(store, tab, cx)));
                    }
                }
            }
            None => self.expanded = None,
        }
        let cover = self
            .expanded
            .as_ref()
            .map(|view| view.read(cx).cover().clone());
        self.player_bar
            .update(cx, |bar, cx| bar.set_expanded(cover, cx));
        if was_open != self.expanded.is_some() {
            self.covered = false;
            if self.expanded.is_some() {
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(DURATION_PANEL).await;
                    let _ = this.update(cx, |this, cx| {
                        if this.expanded.is_some() && !this.covered {
                            this.covered = true;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    pub fn open_menu(this: &Entity<Self>, request: MenuRequest, window: &mut Window, cx: &mut App) {
        let weak = this.downgrade();
        let close = move |window: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |this, cx| {
                this.menu = None;
                this.root_focus.focus(window, cx);
                cx.notify();
            });
        };
        let menu = ContextMenu::open(request, close, window, cx);
        this.update(cx, |this, cx| {
            this.menu = Some(menu);
            cx.notify();
        });
    }

    /// Escape closes the topmost thing, in this order, then leaves the
    /// search field.
    fn on_dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some()
            || std::mem::take(&mut self.shortcuts)
            || self.new_playlist.take().is_some()
            || self.settings.take().is_some()
        {
        } else if self.sign_in.is_some() {
            self.close_sign_in();
        } else if self.expanded.is_some() {
            self.set_expanded(None, cx);
        } else {
            cx.propagate();
            return;
        }
        self.root_focus.focus(window, cx);
        cx.notify();
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu = None;
        if self.settings.is_none() {
            let weak = cx.entity().downgrade();
            let close = move |window: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |this, cx| {
                    this.settings = None;
                    this.root_focus.focus(window, cx);
                    cx.notify();
                });
            };
            let store = self.store.clone();
            self.settings = Some(cx.new(|cx| Settings::new(store, close, window, cx)));
        }
        cx.notify();
    }

    fn on_open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings(window, cx);
    }

    fn close_sign_in(&mut self) {
        self.sign_in = None;
        self.sign_in_wanted = false;
        self.sign_in_dismissed = true;
    }

    fn typing(&self, window: &Window, cx: &App) -> bool {
        self.topbar.read(cx).typing(window, cx)
            || self.sign_in.is_some()
            || self.new_playlist.is_some()
            || self.settings.is_some()
    }

    /// music.youtube.com's single-key shortcuts. They stand down while a
    /// text field has focus, so typing a "k" in search stays a "k".
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.typing(window, cx) || self.menu.is_some() {
            return;
        }
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        if modifiers.control || modifiers.platform || modifiers.alt {
            return;
        }
        let typed = keystroke
            .key_char
            .as_deref()
            .unwrap_or(keystroke.key.as_str());
        let after_g = self
            .chord_at
            .take()
            .is_some_and(|at| at.elapsed() < crate::shortcuts::CHORD_TIMEOUT);
        let Some(shortcut) =
            crate::shortcuts::resolve(after_g, keystroke.key.as_str(), typed, modifiers.shift)
        else {
            return;
        };
        cx.stop_propagation();
        use crate::shortcuts::Shortcut;
        let store = self.store.clone();
        let rate = |liked: formalmusic_api::Rating| {
            let state = store.state();
            let Some(track) = state.player.track.clone() else {
                return;
            };
            let rating = if state.rating(&track) == liked {
                formalmusic_api::Rating::Indifferent
            } else {
                liked
            };
            drop(state);
            store.rate(&track, rating);
        };
        match shortcut {
            Shortcut::Toggle => store.toggle(),
            Shortcut::Next => store.next(),
            Shortcut::Previous => store.previous(),
            Shortcut::Seek(delta_ms) => store.seek_by(delta_ms),
            Shortcut::Volume(steps) => {
                let volume = store.state().player.volume;
                store.set_volume(volume + steps * VOLUME_STEP);
            }
            Shortcut::Mute => {
                let muted = store.state().player.muted;
                store.set_muted(!muted);
            }
            Shortcut::Shuffle => store.toggle_shuffle(),
            Shortcut::Repeat => store.cycle_repeat(),
            Shortcut::Like => rate(formalmusic_api::Rating::Like),
            Shortcut::Dislike => rate(formalmusic_api::Rating::Dislike),
            Shortcut::Search => {
                let handle = self.topbar.read(cx).search_handle(cx);
                window.focus(&handle, cx);
            }
            Shortcut::Queue => {
                let open = self
                    .expanded
                    .as_ref()
                    .is_none_or(|view| view.read(cx).tab() != Tab::UpNext);
                self.set_expanded(open.then_some(Tab::UpNext), cx);
            }
            Shortcut::Expand => {
                let open = self.expanded.is_none();
                self.set_expanded(open.then_some(Tab::UpNext), cx);
            }
            Shortcut::Help => {
                self.shortcuts = !self.shortcuts;
                cx.notify();
            }
            Shortcut::Chord => self.chord_at = Some(std::time::Instant::now()),
            Shortcut::Home => self.navigate(Route::Browse(BrowseTarget::Home), false, cx),
            Shortcut::Explore => self.navigate(Route::Browse(BrowseTarget::Explore), false, cx),
            Shortcut::Library => self.navigate(
                Route::Browse(BrowseTarget::Library(
                    formalmusic_api::LibraryTab::Playlists,
                )),
                false,
                cx,
            ),
            Shortcut::Settings => self.open_settings(window, cx),
        }
    }

    fn on_focus_search(&mut self, _: &FocusSearch, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.topbar.read(cx).search_handle(cx);
        window.focus(&handle, cx);
    }

    fn on_back(&mut self, _: &GoBack, _: &mut Window, cx: &mut Context<Self>) {
        self.step_history(-1, cx);
    }

    fn on_forward(&mut self, _: &GoForward, _: &mut Window, cx: &mut Context<Self>) {
        self.step_history(1, cx);
    }

    #[cfg(feature = "frame-overlay")]
    fn on_toggle_frame_overlay(
        &mut self,
        _: &ToggleFrameOverlay,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        window.cycle_debug_frame_overlay_mode();
        window.refresh();
    }

    /// The toast follows the store's notice; each one clears itself.
    fn sync_toast(&mut self, cx: &mut Context<Self>) {
        let notice = self.store.state().notice.clone();
        if notice.as_ref().map(|(seq, _)| *seq) != self.toast.as_ref().map(|(seq, _)| *seq) {
            if let Some((seq, _)) = &notice {
                let (seq, store) = (*seq, self.store.clone());
                cx.spawn(async move |_, cx| {
                    cx.background_executor().timer(TOAST_FOR).await;
                    store.clear_notice(seq);
                })
                .detach();
            }
            self.toast = notice.map(|(seq, text)| (seq, text.into()));
        }
        let shown = self.toast.as_ref().map(|(_, text)| text.clone());
        self.toast_shown
            .set(shown, |this: &mut Self| &mut this.toast_shown, cx);
    }

    /// Whether the sign-in screen covers the window: asked for, or the
    /// daemon says nobody is signed in and browsing signed out was not chosen.
    fn wants_sign_in(&self) -> bool {
        if self.sign_in_wanted {
            return true;
        }
        let state = self.store.state();
        !self.sign_in_dismissed
            && state.connection == ConnectionStatus::Online
            && state
                .session
                .as_ref()
                .is_some_and(|session| !session.signed_in)
    }
}

impl Render for AppRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace::render("AppRoot");
        let palette = Theme::get(cx);
        self.sync_toast(cx);

        if self.wants_sign_in() {
            if self.sign_in.is_none() {
                let weak = cx.entity().downgrade();
                let close = move |window: &mut Window, cx: &mut App| {
                    let _ = weak.update(cx, |this, cx| {
                        this.close_sign_in();
                        this.root_focus.focus(window, cx);
                        cx.notify();
                    });
                };
                let store = self.store.clone();
                self.sign_in = Some(cx.new(|cx| SignIn::new(store, close, window, cx)));
            }
            // Asked for while signed in, it is a switch to another profile
            // and closes itself once that worked.
            if self.store.state().signed_in() && !self.sign_in_wanted {
                self.sign_in = None;
            }
        } else {
            self.sign_in = None;
        }

        self.expanded_shown.set(
            self.expanded.clone(),
            |this: &mut Self| &mut this.expanded_shown,
            cx,
        );
        let expanded = self.expanded_shown.current().cloned().map(|view| {
            let open = self.expanded_shown.is_open();
            let height: f32 = (window.viewport_size().height - PLAYER_HEIGHT).into();
            let layer = div()
                .absolute()
                .left_0()
                .right_0()
                .bottom(PLAYER_HEIGHT)
                .h(px(height))
                .child(view);
            motion::toward(
                layer,
                self.expanded_shown.id("expanded"),
                open,
                DURATION_PANEL,
                DURATION_BASE,
                move |el, t| el.mb(px(-height * 0.06 * (1. - t))).opacity(t),
            )
        });

        let notice = self.toast_shown.current().cloned().map(|message| {
            let open = self.toast_shown.is_open();
            let pill = crate::toast::toast(&message, cx);
            motion::toward(
                pill,
                self.toast_shown.id("toast"),
                open,
                DURATION_BASE,
                DURATION_FAST,
                |el, t| {
                    el.pb(crate::toast::BOTTOM - crate::toast::RISE * (1. - t))
                        .opacity(t)
                },
            )
        });

        let page = self.pages.last().cloned();
        let collapsed = self.sidebar.read(cx).collapsed();
        let caption = crate::chrome::has_caption_buttons(window);
        let offline = {
            let state = self.store.state();
            (state.connection == ConnectionStatus::Offline
                && self.store.kind() == TransportKind::Daemon)
                .then(|| {
                    state
                        .connection_error
                        .clone()
                        .unwrap_or_else(|| "The music daemon is not running.".into())
                })
        };

        div()
            .key_context(CONTEXT)
            .id("app-root")
            .track_focus(&self.root_focus)
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(palette.canvas)
            .text_color(palette.text)
            .font_family(crate::theme::font_sans())
            .on_action(cx.listener(Self::on_dismiss))
            .on_action(cx.listener(Self::on_open_settings))
            .on_action(cx.listener(Self::on_focus_search))
            .on_action(cx.listener(Self::on_back))
            .on_action(cx.listener(Self::on_forward))
            .when(cfg!(feature = "frame-overlay"), |el| {
                #[cfg(feature = "frame-overlay")]
                let el = el.on_action(cx.listener(Self::on_toggle_frame_overlay));
                el
            })
            .on_key_down(cx.listener(Self::on_key_down))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    .h(TITLEBAR_HEIGHT)
                    .child(crate::chrome::drag_layer("title-drag", window, cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_grow(1.)
                    .min_h(px(0.))
                    .when(!self.covered, |el| {
                        el.child(
                            AnyView::from(self.sidebar.clone()).cached(
                                StyleRefinement::default()
                                    .w(if collapsed {
                                        SIDEBAR_COLLAPSED
                                    } else {
                                        SIDEBAR_WIDTH
                                    })
                                    .h_full()
                                    .flex_shrink_0(),
                            ),
                        )
                        .child(
                            div()
                                .w(px(1.))
                                .h_full()
                                .flex_shrink_0()
                                .bg(palette.sidebar_border),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_grow(1.)
                                .min_w(px(0.))
                                .h_full()
                                .child(
                                    div()
                                        .h(TITLEBAR_HEIGHT)
                                        .flex_shrink_0()
                                        .w_full()
                                        .when(caption, |el| {
                                            el.pr(crate::chrome::caption_reserve(window))
                                        })
                                        .child(AnyView::from(self.topbar.clone()).cached(
                                            StyleRefinement::default().w_full().h(TITLEBAR_HEIGHT),
                                        )),
                                )
                                .when_some(offline, |el, reason| {
                                    el.child(
                                        div()
                                            .flex_shrink_0()
                                            .px(crate::theme::PAGE_INSET)
                                            .py(crate::theme::spacing::X2)
                                            .bg(palette.danger_soft)
                                            .text_size(crate::theme::type_scale::CAPTION.font_size)
                                            .text_color(palette.text)
                                            .child(format!(
                                                "{reason} Showing what was cached; retrying."
                                            )),
                                    )
                                })
                                .child(
                                    div()
                                        .relative()
                                        .flex_grow(1.)
                                        .flex_basis(px(0.))
                                        .min_h(px(0.))
                                        .w_full()
                                        .children(page.map(|page| {
                                            AnyView::from(page)
                                                .cached(StyleRefinement::default().size_full())
                                        })),
                                ),
                        )
                    }),
            )
            .child(
                AnyView::from(self.player_bar.clone()).cached(
                    StyleRefinement::default()
                        .w_full()
                        .h(PLAYER_HEIGHT)
                        .flex_shrink_0(),
                ),
            )
            .children(expanded)
            .when_some(self.new_playlist.clone(), |el, dialog| el.child(dialog))
            .when_some(self.settings.clone(), |el, dialog| el.child(dialog))
            .when_some(self.sign_in.clone(), |el, screen| {
                el.child(div().absolute().inset_0().child(screen))
            })
            .when(self.shortcuts, |el| {
                let weak = cx.entity().downgrade();
                el.child(crate::shortcuts::overlay(palette, move |window, cx| {
                    let _ = weak.update(cx, |this, cx| {
                        this.shortcuts = false;
                        this.root_focus.focus(window, cx);
                        cx.notify();
                    });
                }))
            })
            .when_some(self.menu.clone(), |el, menu| el.child(menu))
            .when(caption, |el| {
                el.child(
                    div()
                        .absolute()
                        .top_0()
                        .right_0()
                        .occlude()
                        .child(crate::chrome::caption_buttons()),
                )
            })
            .children(notice)
            .children(crate::trace::probe())
    }
}

/// `FORMALMUSIC_TOUR=1` (with the demo): Home, an artist, the 1000 track
/// playlist scrolled to its end, then the expanded player, a few seconds
/// each, then Home paused for eight seconds and playing after that, so the
/// memory and CPU after a browse can be read from outside on a machine
/// nobody is clicking on. Ten seconds into playback the expanded player
/// opens again, for the animated cover's figure. Each step goes to the
/// trace log.
fn tour(window: &mut Window, cx: &mut Context<AppRoot>) {
    cx.spawn_in(window, async move |this, cx| {
        let executor = cx.background_executor().clone();
        let catalog = formalmusic_core::demo::catalog();
        let steps: [(&str, Option<Route>); 4] = [
            (
                "artist",
                Some(Route::Browse(BrowseTarget::Artist(
                    catalog.artists[2].browse_id.clone(),
                ))),
            ),
            (
                "playlist",
                Some(Route::Browse(BrowseTarget::Playlist(
                    catalog.playlists[1].playlist_id.clone(),
                ))),
            ),
            ("expanded", None),
            ("home", Some(Route::Browse(BrowseTarget::Home))),
        ];
        for (name, route) in steps {
            executor.timer(std::time::Duration::from_secs(3)).await;
            let _ = this.update(cx, |this, cx| {
                crate::trace::log(&format!("tour: {name}"));
                match route {
                    Some(route) => this.navigate(route, false, cx),
                    None => this.set_expanded(Some(Tab::UpNext), cx),
                }
            });
            if name == "playlist" {
                // Down to the end and back, a page at a time, so every
                // continuation loads and every row is laid out once.
                for _ in 0..12 {
                    executor.timer(std::time::Duration::from_millis(400)).await;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(page) = this.pages.last() {
                            page.read(cx).scroll_to_end();
                            page.update(cx, |_, cx| cx.notify());
                        }
                    });
                }
            }
        }
        // Eight paused seconds to read idle CPU from, then playback for the
        // playing figure.
        executor.timer(std::time::Duration::from_secs(8)).await;
        let _ = this.update(cx, |this, _| {
            crate::trace::log("tour: play");
            this.store.toggle();
        });
        executor.timer(std::time::Duration::from_secs(10)).await;
        let _ = this.update(cx, |this, cx| {
            crate::trace::log("tour: expanded playing");
            this.set_expanded(Some(Tab::UpNext), cx);
        });
    })
    .detach();
}

/// `FORMALMUSIC_TOUR=lyrics`, `cover` or `bar` (with the demo): the expanded
/// player on its Lyrics or Up next tab, or the bar alone, eight seconds
/// paused, then playing, so the frame cost and CPU of each can be read the
/// same way as the main tour's. `expanded` opens Up next and leaves playback
/// alone, for a daemon that already plays a music video.
fn player_tour(tab: Option<Tab>, play: bool, window: &mut Window, cx: &mut Context<AppRoot>) {
    cx.spawn_in(window, async move |this, cx| {
        let executor = cx.background_executor().clone();
        executor.timer(std::time::Duration::from_secs(2)).await;
        let _ = this.update(cx, |this, cx| {
            crate::trace::log("tour: player");
            this.set_expanded(tab, cx);
        });
        if !play {
            return;
        }
        executor.timer(std::time::Duration::from_secs(8)).await;
        let _ = this.update(cx, |this, _| {
            crate::trace::log("tour: play");
            this.store.toggle();
        });
    })
    .detach();
}

/// `scripts/screenshot.sh`: opens the scene `FORMALMUSIC_SCREENSHOT_SCENE`
/// names, gives the artwork a moment to decode, renders the frame offscreen
/// (animations jumped to their end), writes a PNG and quits.
#[cfg(feature = "screenshot")]
fn screenshot(out: std::path::PathBuf, window: &mut Window, cx: &mut Context<AppRoot>) {
    use formalmusic_api::LibraryTab;
    use formalmusic_core::SearchKey;
    cx.set_reduce_motion(true);
    let scene = std::env::var("FORMALMUSIC_SCREENSHOT_SCENE").unwrap_or_default();
    cx.spawn_in(window, async move |this, cx| {
        let executor = cx.background_executor().clone();
        let wait = |ms| executor.timer(std::time::Duration::from_millis(ms));
        wait(600).await;
        let catalog = formalmusic_core::demo::catalog();
        let _ = this.update_in(cx, |this, window, cx| match scene.as_str() {
            "album" => this.navigate(
                Route::Browse(BrowseTarget::Album("MPREb_K8qWMWVqXGi".into())),
                false,
                cx,
            ),
            "artist" => this.navigate(
                Route::Browse(BrowseTarget::Artist("UCRr1xG_2WIDs18a6cIiCxeA".into())),
                false,
                cx,
            ),
            "playlist" => this.navigate(
                Route::Browse(BrowseTarget::Playlist(
                    "PL0GvsLQil0MmYC96KEs_7dTNsLm1PS6JX".into(),
                )),
                false,
                cx,
            ),
            "own-playlist" => this.navigate(
                Route::Browse(BrowseTarget::Playlist(
                    catalog.playlists[1].playlist_id.clone(),
                )),
                false,
                cx,
            ),
            "explore" => this.navigate(Route::Browse(BrowseTarget::Explore), false, cx),
            "history" => this.navigate(Route::Browse(BrowseTarget::History), false, cx),
            "library" => this.navigate(
                Route::Browse(BrowseTarget::Library(LibraryTab::Playlists)),
                false,
                cx,
            ),
            "search" => this.navigate(
                Route::Search(SearchKey {
                    query: "light".into(),
                    filter: None,
                }),
                false,
                cx,
            ),
            "queue" => this.set_expanded(Some(Tab::UpNext), cx),
            // Paused mid-word with a backing vocal lit, the wipe and the
            // depth ramp drawn as they play rather than jumped to an end.
            "lyrics" | "lyrics-duet" => {
                cx.set_reduce_motion(false);
                let duet = scene == "lyrics-duet";
                let queue = this.store.state().queue.clone();
                if let Some(index) = queue
                    .tracks
                    .iter()
                    .position(|track| formalmusic_core::demo::duet(&track.video_id) == duet)
                    && queue.current != Some(index)
                {
                    this.store.jump_to(index);
                    this.store.toggle();
                }
                this.store.seek(24_100);
                this.set_expanded(Some(Tab::Lyrics), cx)
            }
            "related" => this.set_expanded(Some(Tab::Related), cx),
            "signin" => {}
            "shortcuts" => this.shortcuts = true,
            "settings" => this.open_settings(window, cx),
            "collapsed" => this.sidebar.update(cx, |sidebar, cx| sidebar.toggle(cx)),
            "menu" => {
                let track = catalog.albums[2].tracks[0].clone();
                let items = crate::actions::track_menu(
                    &track,
                    &crate::actions::MenuContext::default(),
                    &this.store,
                );
                let weak = cx.entity().downgrade();
                let close = move |_: &mut Window, cx: &mut App| {
                    let _ = weak.update(cx, |this, cx| {
                        this.menu = None;
                        cx.notify();
                    });
                };
                this.menu = Some(ContextMenu::open(
                    MenuRequest::at(point(px(700.), px(300.)), items),
                    close,
                    window,
                    cx,
                ));
            }
            "suggest" => this
                .topbar
                .update(cx, |topbar, cx| topbar.type_query("ha", window, cx)),
            _ => {}
        });
        wait(2500).await;
        let _ = cx.update(|window, _| window.refresh());
        wait(400).await;
        let _ = cx.update(|window, _| window.refresh());
        wait(200).await;
        let _ = cx.update(|window, cx| {
            match window.render_to_image() {
                Ok(image) => {
                    if let Some(parent) = out.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    match image.save(&out) {
                        Ok(()) => println!("[screenshot] wrote {}", out.display()),
                        Err(error) => eprintln!("[screenshot] {error}"),
                    }
                }
                Err(error) => eprintln!("[screenshot] {error}"),
            }
            cx.quit();
        });
    })
    .detach();
}
