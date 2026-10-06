//! Home, Explore and Library, then "New playlist" and your playlists. It
//! collapses to a column of icons, the way the web app's guide does.

use formalmusic_api::{BrowseTarget, Item, LibraryTab};
use formalmusic_core::{MusicStore, Route};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::bridge::{Bridge, Topic};
use crate::icons::{Icon, IconName};
use crate::primitives::IconButton;
use crate::theme::{
    Palette, TITLEBAR_HEIGHT, Theme, radius, spacing, traffic_light_clearance, type_scale,
};

const NAV: [(&str, IconName, BrowseTarget); 3] = [
    ("Home", IconName::Home, BrowseTarget::Home),
    ("Explore", IconName::Explore, BrowseTarget::Explore),
    (
        "Library",
        IconName::Library,
        BrowseTarget::Library(LibraryTab::Playlists),
    ),
];

pub struct Sidebar {
    store: MusicStore,
    collapsed: bool,
    route: Option<Route>,
    scroll: UniformListScrollHandle,
}

impl Sidebar {
    pub fn new(store: MusicStore, cx: &mut Context<Self>) -> Self {
        let weak = cx.entity().downgrade();
        Bridge::watch(
            cx,
            Topic::Page(BrowseTarget::Library(LibraryTab::Playlists)),
            weak.clone().into(),
        );
        Bridge::watch(cx, Topic::Session, weak.into());
        Self {
            store,
            collapsed: false,
            route: None,
            scroll: UniformListScrollHandle::new(),
        }
    }

    pub fn collapsed(&self) -> bool {
        self.collapsed
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        self.collapsed = !self.collapsed;
        cx.notify();
    }

    pub fn set_route(&mut self, route: Route, cx: &mut Context<Self>) {
        if self.route.as_ref() != Some(&route) {
            self.route = Some(route);
            cx.notify();
        }
    }

    fn is_current(&self, target: &BrowseTarget) -> bool {
        match (&self.route, target) {
            (Some(Route::Browse(BrowseTarget::Library(_))), BrowseTarget::Library(_)) => true,
            (Some(Route::Browse(BrowseTarget::HomeChip { .. })), BrowseTarget::Home) => true,
            (Some(Route::Browse(current)), target) => current == target,
            _ => false,
        }
    }
}

fn nav_item(
    id: &'static str,
    label: &'static str,
    icon: IconName,
    current: bool,
    collapsed: bool,
    palette: Palette,
    target: BrowseTarget,
) -> impl IntoElement {
    div()
        .id(id)
        .h(px(40.))
        .mx(spacing::X2)
        .px(spacing::X3)
        .flex()
        .flex_row()
        .items_center()
        .when(collapsed, |el| el.justify_center().px_0())
        .gap(spacing::X3)
        .rounded(radius::ROW)
        .cursor_pointer()
        .on_hover({
            let target = target.clone();
            move |hovered, _, cx| {
                crate::actions::prefetch_on_hover(Some(target.clone()), *hovered, cx)
            }
        })
        .when(current, |el| el.bg(palette.press_wash))
        .when(!current, |el| {
            el.hover(move |style| style.bg(palette.hover_wash))
        })
        .on_click(move |_, _, cx| crate::app::navigate(Route::Browse(target.clone()), cx))
        .when(collapsed, |el| {
            el.tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(label).build(window, cx)
            })
        })
        .child(
            Icon::new(icon)
                .size(px(20.))
                .color(if current {
                    palette.text
                } else {
                    palette.secondary
                })
                .strong(current),
        )
        .when(!collapsed, |el| {
            el.child(
                div()
                    .text_size(type_scale::BODY.font_size)
                    .font_weight(if current {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(palette.text)
                    .child(label),
            )
        })
}

impl Render for Sidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace::render("Sidebar");
        let palette = Theme::get(cx);
        let collapsed = self.collapsed;
        let playlists: Vec<(String, String, Option<String>)> = {
            let state = self.store.state();
            state
                .library_playlists()
                .into_iter()
                .filter_map(|item| match item {
                    Item::Playlist {
                        playlist_id,
                        title,
                        subtitle,
                        ..
                    } => Some((playlist_id.clone(), title.clone(), subtitle.clone())),
                    _ => None,
                })
                .collect()
        };
        let signed_in = self.store.state().signed_in();
        let current_playlist = match &self.route {
            Some(Route::Browse(BrowseTarget::Playlist(id))) => Some(id.clone()),
            _ => None,
        };
        let toggle = IconButton::new(
            "sidebar-toggle",
            IconName::Sidebar,
            if collapsed {
                "Show sidebar"
            } else {
                "Hide sidebar"
            },
        )
        .on_click(cx.listener(|this, _, _, cx| this.toggle(cx)));

        let mut column = div()
            .id("sidebar")
            .size_full()
            .flex()
            .flex_col()
            .bg(palette.sidebar)
            .child(
                div()
                    .h(TITLEBAR_HEIGHT)
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .when(collapsed, |el| el.justify_center())
                    .when(!collapsed, |el| {
                        el.pl(traffic_light_clearance().max(spacing::X4))
                            .pr(spacing::X2)
                            .justify_end()
                    })
                    // Above the drag strip, so the button takes its own clicks.
                    .child(div().occlude().child(toggle)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(spacing::X1)
                    .pt(spacing::X2)
                    .children(NAV.iter().enumerate().map(|(n, (label, icon, target))| {
                        nav_item(
                            ["nav-home", "nav-explore", "nav-library"][n],
                            label,
                            *icon,
                            self.is_current(target),
                            collapsed,
                            palette,
                            target.clone(),
                        )
                    })),
            );

        if collapsed {
            return column.child(div().flex_grow(1.));
        }

        column = column
            .child(
                div()
                    .h(px(1.))
                    .mx(spacing::X4)
                    .my(spacing::X4)
                    .bg(palette.sidebar_border),
            )
            .child(
                div().px(spacing::X3).pb(spacing::X2).child(
                    div()
                        .id("new-playlist")
                        .h(px(36.))
                        .rounded(radius::PILL)
                        .border_1()
                        .border_color(palette.separator)
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_center()
                        .gap(spacing::X2)
                        .cursor_pointer()
                        .hover(move |style| style.bg(palette.hover_wash))
                        .on_click(|_, window, cx| crate::app::new_playlist(window, cx))
                        .child(Icon::new(IconName::Plus).size(px(16.)).color(palette.text))
                        .child(
                            div()
                                .text_size(type_scale::BODY.font_size)
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(palette.text)
                                .child("New playlist"),
                        ),
                ),
            );

        if !signed_in {
            return column.child(
                div()
                    .px(spacing::X4)
                    .pt(spacing::X2)
                    .text_size(type_scale::CAPTION.font_size)
                    .line_height(type_scale::CAPTION.line_height)
                    .text_color(palette.tertiary)
                    .child("Sign in to see your playlists here."),
            );
        }

        let count = playlists.len();
        column.child(
            uniform_list("sidebar-playlists", count, move |range, _window, _cx| {
                range
                    .map(|index| {
                        let (playlist_id, title, subtitle) = playlists[index].clone();
                        let current = current_playlist.as_deref() == Some(playlist_id.as_str());
                        let subtitle = subtitle.map(|text| {
                            text.split(" \u{2022} ")
                                .skip(1)
                                .collect::<Vec<_>>()
                                .join(" \u{2022} ")
                        });
                        div()
                            .id(("playlist", index))
                            .h(px(52.))
                            .mx(spacing::X2)
                            .px(spacing::X3)
                            .flex()
                            .flex_col()
                            .justify_center()
                            .rounded(radius::ROW)
                            .cursor_pointer()
                            .when(current, |el| el.bg(palette.press_wash))
                            .when(!current, |el| {
                                el.hover(move |style| style.bg(palette.hover_wash))
                            })
                            .on_hover({
                                let target = BrowseTarget::Playlist(playlist_id.clone());
                                move |hovered, _, cx| {
                                    crate::actions::prefetch_on_hover(
                                        Some(target.clone()),
                                        *hovered,
                                        cx,
                                    )
                                }
                            })
                            .on_click(move |_, _, cx| {
                                crate::app::navigate(
                                    Route::Browse(BrowseTarget::Playlist(playlist_id.clone())),
                                    cx,
                                )
                            })
                            .child(
                                div()
                                    .text_size(type_scale::BODY.font_size)
                                    .line_height(type_scale::BODY.line_height)
                                    .text_color(palette.text)
                                    .truncate()
                                    .child(title),
                            )
                            .when_some(subtitle.filter(|text| !text.is_empty()), |el, text| {
                                el.child(
                                    div()
                                        .text_size(type_scale::CAPTION.font_size)
                                        .line_height(type_scale::CAPTION.line_height)
                                        .text_color(palette.secondary)
                                        .truncate()
                                        .child(text),
                                )
                            })
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(&self.scroll)
            .flex_grow(1.)
            .min_h(px(0.))
            .pb(spacing::X2),
        )
    }
}
