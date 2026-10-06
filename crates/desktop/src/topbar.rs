//! The title row over the page: back and forward, the search field with live
//! suggestions, and the account menu.

use formalmusic_api::{Item, Suggestion};
use formalmusic_core::{MusicStore, Route, SearchKey};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions;
use crate::art;
use crate::bridge::{Bridge, Topic};
use crate::icons::{Icon, IconName};
use crate::menus::MenuItem;
use crate::primitives::{IconButton, overlay_shadows};
use crate::theme::{TITLEBAR_HEIGHT, Theme, radius, spacing, type_scale};

const SEARCH_WIDTH: Pixels = px(480.);
const SUGGESTION_ART: Pixels = px(32.);

pub struct TopBar {
    store: MusicStore,
    input: Entity<InputState>,
    focused: bool,
    /// The suggestion the arrow keys are on.
    highlighted: Option<usize>,
    can_back: bool,
    can_forward: bool,
    _subscriptions: Vec<Subscription>,
}

impl TopBar {
    pub fn new(store: MusicStore, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let weak = cx.entity().downgrade();
        for topic in [Topic::Suggestions, Topic::Session, Topic::Accounts] {
            Bridge::watch(cx, topic, weak.clone().into());
        }
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Search songs, albums, artists, podcasts")
        });
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.highlighted = None;
                    this.store.suggest(&input.read(cx).value());
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {
                    let query = match this
                        .highlighted
                        .and_then(|n| this.suggestions().get(n).cloned())
                    {
                        Some(suggestion) => return this.pick(&suggestion, window, cx),
                        None => input.read(cx).value().trim().to_owned(),
                    };
                    this.search(query, window, cx);
                }
                InputEvent::Focus => {
                    this.focused = true;
                    cx.notify();
                }
                InputEvent::Blur => {
                    this.focused = false;
                    this.highlighted = None;
                    cx.notify();
                }
            },
        );
        Self {
            store,
            input,
            focused: false,
            highlighted: None,
            can_back: false,
            can_forward: false,
            _subscriptions: vec![subscription],
        }
    }

    pub fn search_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }

    /// Types `query` into the focused field, as the `suggest` screenshot shows it.
    #[cfg(feature = "screenshot")]
    pub fn type_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |state, cx| {
            state.set_value(query.to_owned(), window, cx)
        });
        window.focus(&self.input.focus_handle(cx), cx);
        self.focused = true;
        self.store.suggest(query);
        cx.notify();
    }

    /// Whether typing goes to the search field, so single-key shortcuts stand down.
    pub fn typing(&self, window: &Window, cx: &App) -> bool {
        self.input.focus_handle(cx).is_focused(window)
    }

    pub fn set_history(&mut self, can_back: bool, can_forward: bool, cx: &mut Context<Self>) {
        if (self.can_back, self.can_forward) != (can_back, can_forward) {
            (self.can_back, self.can_forward) = (can_back, can_forward);
            cx.notify();
        }
    }

    fn suggestions(&self) -> Vec<Suggestion> {
        self.store
            .state()
            .suggestions
            .items
            .iter()
            .cloned()
            .collect()
    }

    fn search(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        if query.is_empty() {
            return;
        }
        self.store.suggest("");
        window.blur(cx);
        crate::app::navigate(
            Route::Search(SearchKey {
                query,
                filter: None,
            }),
            cx,
        );
    }

    fn pick(&mut self, suggestion: &Suggestion, window: &mut Window, cx: &mut Context<Self>) {
        match suggestion {
            Suggestion::Query { text, .. } => {
                let text = text.clone();
                self.input
                    .update(cx, |state, cx| state.set_value(text.clone(), window, cx));
                self.search(text, window, cx);
            }
            Suggestion::Item(item) => {
                window.blur(cx);
                match item {
                    Item::Track(_) => actions::play_item(item, &self.store),
                    other => {
                        if let Some(target) = actions::target_of(other) {
                            actions::open(target, cx);
                        }
                    }
                }
            }
        }
    }

    fn step(&mut self, delta: i32, cx: &mut Context<Self>) {
        let count = self.store.state().suggestions.items.len() as i32;
        if count == 0 {
            return;
        }
        let next = match self.highlighted {
            Some(current) => (current as i32 + delta).rem_euclid(count),
            None if delta > 0 => 0,
            None => count - 1,
        };
        self.highlighted = Some(next as usize);
        cx.notify();
    }

    fn account_menu(&self) -> Vec<MenuItem> {
        let state = self.store.state();
        let mut items = Vec::new();
        if let Some(account) = state
            .session
            .as_ref()
            .and_then(|session| session.account.as_ref())
        {
            items.push(MenuItem::Header(account.name.clone().into()));
            if let Some(handle) = &account.handle {
                items.push(MenuItem::Note(handle.clone().into()));
            }
        }
        if state.accounts.len() > 1 {
            items.push(MenuItem::Separator);
            items.push(MenuItem::Header("Switch account".into()));
            for account in &state.accounts {
                let (store, page_id) = (self.store.clone(), account.page_id.clone());
                let mut item = MenuItem::item(account.name.clone(), move |_, _| {
                    store.switch_account(page_id.clone())
                });
                if account.selected {
                    item = item.icon(IconName::Check);
                }
                items.push(item);
            }
        }
        items.push(MenuItem::Separator);
        if state.signed_in() {
            items.push(
                MenuItem::item("Sign in again", |_, cx| crate::app::show_sign_in(cx))
                    .icon(IconName::Account),
            );
            let store = self.store.clone();
            items.push(
                MenuItem::item("Sign out", move |_, _| store.sign_out()).icon(IconName::SignOut),
            );
        } else {
            items.push(
                MenuItem::item("Sign in", |_, cx| crate::app::show_sign_in(cx))
                    .icon(IconName::Account),
            );
        }
        items
    }

    fn render_suggestions(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let palette = Theme::get(cx);
        let query = self.input.read(cx).value();
        if !self.focused || query.trim().is_empty() {
            return None;
        }
        let suggestions = self.suggestions();
        if suggestions.is_empty() {
            return None;
        }
        let highlighted = self.highlighted;
        let rows = suggestions.into_iter().enumerate().map(|(n, suggestion)| {
            let active = highlighted == Some(n);
            let row = div()
                .id(("suggestion", n))
                .h(px(44.))
                .px(spacing::X3)
                .flex()
                .flex_row()
                .items_center()
                .gap(spacing::X3)
                .rounded(radius::MENU_ITEM)
                .cursor_pointer()
                .when(active, |el| el.bg(palette.hover_wash))
                .hover(move |style| style.bg(palette.hover_wash))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener({
                        let suggestion = suggestion.clone();
                        move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.pick(&suggestion, window, cx);
                        }
                    }),
                );
            match &suggestion {
                // The icon sits in a box as wide as an item's cover, so
                // query and item text start on the same edge.
                Suggestion::Query { text, from_history } => row
                    .child(
                        div()
                            .w(SUGGESTION_ART)
                            .flex_shrink_0()
                            .flex()
                            .justify_center()
                            .child(
                                Icon::new(if *from_history {
                                    IconName::History
                                } else {
                                    IconName::Search
                                })
                                .size(px(16.))
                                .color(palette.secondary),
                            ),
                    )
                    .child(
                        div()
                            .text_size(type_scale::BODY.font_size)
                            .text_color(palette.text)
                            .truncate()
                            .child(text.clone()),
                    ),
                Suggestion::Item(item) => {
                    let (title, subtitle, thumbnails, round) = match item {
                        Item::Track(track) => (
                            track.title.clone(),
                            formalmusic_core::format::byline(track),
                            track.thumbnails.clone(),
                            false,
                        ),
                        Item::Album {
                            title,
                            artists,
                            thumbnails,
                            ..
                        } => (
                            title.clone(),
                            format!(
                                "Album \u{2022} {}",
                                formalmusic_core::format::names(artists)
                            ),
                            thumbnails.clone(),
                            false,
                        ),
                        Item::Artist {
                            name, thumbnails, ..
                        } => (name.clone(), "Artist".into(), thumbnails.clone(), true),
                        Item::Playlist {
                            title,
                            subtitle,
                            thumbnails,
                            ..
                        } => (
                            title.clone(),
                            subtitle.clone().unwrap_or_default(),
                            thumbnails.clone(),
                            false,
                        ),
                        Item::Podcast {
                            title,
                            subtitle,
                            thumbnails,
                            ..
                        } => (
                            title.clone(),
                            subtitle.clone().unwrap_or_default(),
                            thumbnails.clone(),
                            false,
                        ),
                        Item::Mood { title, .. } => {
                            (title.clone(), String::new(), Vec::new(), false)
                        }
                    };
                    row.child(art::cover(
                        &thumbnails,
                        SUGGESTION_ART,
                        radius::ART_SMALL,
                        round,
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
                                    .text_color(palette.text)
                                    .truncate()
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_size(type_scale::CAPTION.font_size)
                                    .text_color(palette.secondary)
                                    .truncate()
                                    .child(subtitle),
                            ),
                    )
                }
            }
        });
        Some(
            deferred(
                anchored().snap_to_window_with_margin(spacing::X2).child(
                    div()
                        .id("suggestions")
                        .occlude()
                        .mt(px(44.))
                        .w(SEARCH_WIDTH)
                        .p(spacing::X1)
                        .flex()
                        .flex_col()
                        .rounded(radius::MENU)
                        .bg(palette.overlay)
                        .border_1()
                        .border_color(palette.overlay_border)
                        .shadow(overlay_shadows(&palette))
                        .children(rows),
                ),
            )
            .with_priority(1),
        )
    }
}

impl Render for TopBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace::render("TopBar");
        let palette = Theme::get(cx);
        let (signed_in, account) = {
            let state = self.store.state();
            (
                state.signed_in(),
                state
                    .session
                    .as_ref()
                    .and_then(|session| session.account.clone()),
            )
        };
        let suggestions = self.render_suggestions(cx);
        let search = div()
            .relative()
            .w(SEARCH_WIDTH)
            .min_w(px(200.))
            .occlude()
            .capture_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "down" => {
                        this.step(1, cx);
                        cx.stop_propagation();
                    }
                    "up" => {
                        this.step(-1, cx);
                        cx.stop_propagation();
                    }
                    "escape" => {
                        window.blur(cx);
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .child(
                Input::new(&self.input)
                    .prefix(
                        Icon::new(IconName::Search)
                            .size(px(16.))
                            .color(palette.secondary),
                    )
                    .cleanable(true),
            )
            .children(suggestions);
        let store = self.store.clone();
        let account_button = div()
            .id("account")
            .occlude()
            .size(px(32.))
            .rounded(px(16.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(move |style| style.bg(palette.hover_wash))
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new("Account").build(window, cx)
            })
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                store.load_accounts();
                let items = this.account_menu();
                actions::open_menu(event.position(), items, window, cx);
            }))
            .child(match account.filter(|_| signed_in) {
                Some(account) => art::cover(&account.thumbnails, px(28.), px(14.), true, &palette)
                    .into_any_element(),
                None => Icon::new(IconName::Account)
                    .size(px(22.))
                    .color(palette.secondary)
                    .into_any_element(),
            });
        div()
            .h(TITLEBAR_HEIGHT)
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(spacing::X2)
            .px(spacing::X4)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(spacing::X1)
                    .child(
                        IconButton::new("back", IconName::Back, "Back")
                            .disabled(!self.can_back)
                            .on_click(|_, window, cx| crate::app::go_back(window, cx)),
                    )
                    .child(
                        IconButton::new("forward", IconName::Forward, "Forward")
                            .disabled(!self.can_forward)
                            .on_click(|_, window, cx| crate::app::go_forward(window, cx)),
                    ),
            )
            .child(div().pl(spacing::X2).child(search))
            .child(div().flex_grow(1.))
            .child(account_button)
    }
}
