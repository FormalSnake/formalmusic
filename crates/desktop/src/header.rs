//! Page headers in YouTube Music's three shapes: Detail (album, single,
//! playlist), Artist (a full-bleed banner) and a plain Title.

use std::ops::Range;
use std::rc::Rc;

use formalmusic_api::{BrowseTarget, Header, Link, PlaySource, Rating};
use formalmusic_core::MusicStore;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::actions;
use crate::art;
use crate::icons::{Icon, IconName};
use crate::primitives::{Button, ButtonKind, IconButton};
use crate::theme::{PAGE_INSET, Palette, radius, spacing, type_scale, with_alpha};

const DETAIL_ART: Pixels = px(232.);
const BANNER_HEIGHT: Pixels = px(340.);
/// The description's measure, and how many lines it shows until "More".
const DESCRIPTION_WIDTH: Pixels = px(640.);
const DESCRIPTION_LINES: usize = 3;

/// Whether the album or playlist description is expanded, and how to flip it.
pub struct Description {
    pub open: bool,
    pub toggle: Rc<dyn Fn(&mut App)>,
}

pub fn header(
    header: &Header,
    store: &MusicStore,
    palette: Palette,
    description_state: Description,
    window: &mut Window,
) -> AnyElement {
    let width = window.viewport_size().width;
    match header {
        Header::Detail {
            title,
            subtitle,
            second_subtitle,
            description,
            thumbnails,
            playlist_id,
            editable,
            saved,
            ..
        } => {
            let byline = links(subtitle, palette.secondary, palette.text);
            let actions = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(spacing::X2)
                .pt(spacing::X4)
                .when_some(playlist_id.clone(), |el, playlist_id| {
                    let (play, shuffle) = (store.clone(), store.clone());
                    let shuffle_id = playlist_id.clone();
                    el.child(pill_button(
                        "header-play",
                        IconName::Play,
                        "Play",
                        true,
                        palette,
                        move |_, _, _| {
                            play.play(
                                PlaySource::Playlist {
                                    playlist_id: playlist_id.clone(),
                                },
                                0,
                                false,
                                false,
                            )
                        },
                    ))
                    .child(pill_button(
                        "header-shuffle",
                        IconName::Shuffle,
                        "Shuffle",
                        false,
                        palette,
                        move |_, _, _| {
                            shuffle.play(
                                PlaySource::Playlist {
                                    playlist_id: shuffle_id.clone(),
                                },
                                0,
                                true,
                                false,
                            )
                        },
                    ))
                })
                .when_some(
                    saved.zip(header_playlist(header)),
                    |el, (saved, playlist_id)| {
                        let store = store.clone();
                        el.child(
                            IconButton::new(
                                "header-save",
                                IconName::Saved,
                                if saved {
                                    "Remove from library"
                                } else {
                                    "Save to library"
                                },
                            )
                            .hit(px(36.))
                            .size(px(18.))
                            .filled(saved)
                            .color(if saved {
                                palette.text
                            } else {
                                palette.secondary
                            })
                            .on_click(move |_, _, _| {
                                store.set_in_library(playlist_id.clone(), !saved)
                            }),
                        )
                    },
                )
                .when(*editable, |el| {
                    el.child(
                        div()
                            .text_size(type_scale::CAPTION.font_size)
                            .text_color(palette.tertiary)
                            .child("Your playlist"),
                    )
                });
            div()
                .flex()
                .flex_row()
                .items_end()
                .gap(spacing::X8)
                .px(PAGE_INSET)
                .pt(spacing::X8)
                .pb(spacing::X6)
                .child(art::cover(
                    thumbnails,
                    DETAIL_ART,
                    radius::ART,
                    false,
                    &palette,
                ))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(spacing::X1)
                        .flex_grow(1.)
                        .min_w(px(0.))
                        .child(
                            div()
                                .text_size(type_scale::DISPLAY.font_size)
                                .line_height(type_scale::DISPLAY.line_height)
                                .font_weight(FontWeight::BOLD)
                                .text_color(palette.text)
                                .line_clamp(2)
                                .child(title.clone()),
                        )
                        .child(byline)
                        .when_some(second_subtitle.clone(), |el, text| {
                            el.child(caption(text, palette.secondary))
                        })
                        .when_some(description.as_deref(), |el, text| {
                            el.child(detail_description(text, description_state, palette, window))
                        })
                        .child(actions),
                )
                .into_any_element()
        }
        Header::Artist {
            name,
            description,
            thumbnails,
            channel_id,
            subscribed,
            subscribers,
            shuffle_playlist_id,
            radio_playlist_id,
            monthly_listeners,
        } => {
            let banner = art::source(thumbnails, width.max(px(600.)));
            let fade = linear_gradient(
                180.,
                linear_color_stop(hsla(0., 0., 0., 0.), 0.35),
                linear_color_stop(palette.canvas, 1.),
            );
            let actions = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(spacing::X2)
                .pt(spacing::X3)
                .when_some(shuffle_playlist_id.clone(), |el, playlist_id| {
                    let store = store.clone();
                    el.child(pill_button(
                        "artist-shuffle",
                        IconName::Shuffle,
                        "Shuffle",
                        true,
                        palette,
                        move |_, _, _| {
                            store.play(
                                PlaySource::Playlist {
                                    playlist_id: playlist_id.clone(),
                                },
                                0,
                                true,
                                false,
                            )
                        },
                    ))
                })
                .when_some(radio_playlist_id.clone(), |el, playlist_id| {
                    let store = store.clone();
                    el.child(pill_button(
                        "artist-radio",
                        IconName::Radio,
                        "Radio",
                        false,
                        palette,
                        move |_, _, _| {
                            store.play(
                                PlaySource::Playlist {
                                    playlist_id: playlist_id.clone(),
                                },
                                0,
                                false,
                                true,
                            )
                        },
                    ))
                })
                .when_some(
                    channel_id.clone().zip(*subscribed),
                    |el, (channel_id, subscribed)| {
                        let store = store.clone();
                        let label = match (subscribed, subscribers) {
                            (true, Some(count)) => format!("Subscribed \u{2022} {count}"),
                            (false, Some(count)) => format!("Subscribe \u{2022} {count}"),
                            (true, None) => "Subscribed".into(),
                            (false, None) => "Subscribe".into(),
                        };
                        el.child(
                            Button::new("artist-subscribe", label)
                                .pill()
                                .kind(if subscribed {
                                    ButtonKind::Secondary
                                } else {
                                    ButtonKind::Primary
                                })
                                .on_click(move |_, _, _| {
                                    store.set_subscribed(channel_id.clone(), !subscribed)
                                }),
                        )
                    },
                );
            div()
                .relative()
                .h(BANNER_HEIGHT)
                .w_full()
                .overflow_hidden()
                .bg(palette.raised)
                .when_some(banner, |el, source| {
                    el.child(
                        img(source)
                            .absolute()
                            .inset_0()
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    )
                })
                .child(div().absolute().inset_0().bg(fade))
                .child(div().absolute().inset_0().bg(linear_gradient(
                    90.,
                    linear_color_stop(with_alpha(palette.canvas, 0xb0), 0.),
                    linear_color_stop(hsla(0., 0., 0., 0.), 0.65),
                )))
                .child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0()
                        .px(PAGE_INSET)
                        .pb(spacing::X6)
                        .flex()
                        .flex_col()
                        .gap(spacing::X1)
                        .child(
                            div()
                                .text_size(px(44.))
                                .line_height(px(52.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(palette.text)
                                .child(name.clone()),
                        )
                        .when_some(monthly_listeners.clone(), |el, text| {
                            el.child(caption(text, palette.secondary))
                        })
                        .when_some(description.as_deref().map(one_paragraph), |el, text| {
                            el.child(
                                div()
                                    .max_w(px(640.))
                                    .text_size(type_scale::BODY.font_size)
                                    .line_height(px(20.))
                                    .text_color(palette.secondary)
                                    .line_clamp(2)
                                    .text_ellipsis()
                                    .child(text),
                            )
                        })
                        .child(actions),
                )
                .into_any_element()
        }
        Header::Title { title } => div()
            .px(PAGE_INSET)
            .pt(spacing::X8)
            .pb(spacing::X2)
            .text_size(type_scale::DISPLAY.font_size)
            .line_height(type_scale::DISPLAY.line_height)
            .font_weight(FontWeight::BOLD)
            .text_color(palette.text)
            .child(title.clone())
            .into_any_element(),
    }
}

fn header_playlist(header: &Header) -> Option<String> {
    match header {
        Header::Detail { playlist_id, .. } => playlist_id.clone(),
        _ => None,
    }
}

fn caption(text: String, color: Hsla) -> Div {
    div()
        .text_size(type_scale::BODY.font_size)
        .line_height(type_scale::BODY.line_height)
        .text_color(color)
        .child(text)
}

/// "Album • Nadia Reyes • 2024", where the linked parts open their page.
pub fn links(links: &[Link], color: Hsla, hover: Hsla) -> Div {
    let mut row = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .text_size(type_scale::BODY.font_size)
        .line_height(type_scale::BODY.line_height)
        .text_color(color);
    for (n, link) in links.iter().enumerate() {
        if n > 0 {
            row = row.child(div().px(spacing::X1).child("\u{2022}"));
        }
        row = row.child(link_text(link, ("byline", n), hover));
    }
    row
}

/// A row's second line, "Song \u{2022} Nadia Reyes & Kofi Mensah \u{2022} Night
/// Drive", as one run of text so a line too long for its row ends in a single
/// ellipsis. The linked names open their page and underline under the pointer.
#[derive(IntoElement)]
pub struct LinkLine {
    id: ElementId,
    text: String,
    links: Vec<(Range<usize>, BrowseTarget)>,
    hover: Hsla,
}

impl LinkLine {
    pub fn new(id: impl Into<ElementId>, hover: Hsla) -> Self {
        Self {
            id: id.into(),
            text: String::new(),
            links: Vec::new(),
            hover,
        }
    }

    pub fn text(mut self, text: &str) -> Self {
        self.text.push_str(text);
        self
    }

    /// A bullet between what came before and what follows; nothing at the start.
    pub fn dot(self) -> Self {
        if self.text.is_empty() {
            self
        } else {
            self.text(" \u{2022} ")
        }
    }

    pub fn link(mut self, link: &Link) -> Self {
        let start = self.text.len();
        self.text.push_str(&link.text);
        if let Some(target) = &link.target {
            self.links.push((start..self.text.len(), target.clone()));
        }
        self
    }

    /// "A, B & C", the way `formalmusic_core::format::names` joins them.
    pub fn names(mut self, links: &[Link]) -> Self {
        for (n, link) in links.iter().enumerate() {
            if n > 0 {
                self = self.text(if n + 1 == links.len() { " & " } else { ", " });
            }
            self = self.link(link);
        }
        self
    }
}

impl RenderOnce for LinkLine {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let hovered = window.use_keyed_state(self.id.clone(), cx, |_, _| None::<usize>);
        let ranges: Vec<Range<usize>> = self.links.iter().map(|(range, _)| range.clone()).collect();
        let lit = (*hovered.read(cx))
            .and_then(|n| ranges.get(n))
            .map(|range| {
                (
                    range.clone(),
                    HighlightStyle {
                        color: Some(self.hover),
                        underline: Some(UnderlineStyle {
                            thickness: px(1.),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )
            });
        let targets: Vec<BrowseTarget> = self.links.into_iter().map(|(_, target)| target).collect();
        let (hover_ranges, on_text, on_leave) = (ranges.clone(), hovered.clone(), hovered);
        let set = |state: &Entity<Option<usize>>, value: Option<usize>, cx: &mut App| {
            state.update(cx, |current, cx| {
                if *current != value {
                    *current = value;
                    cx.notify();
                }
            });
        };
        div()
            .id(self.id)
            .min_w(px(0.))
            .truncate()
            // The text only reports the pointer while it is over it, so
            // leaving the line clears the underline here.
            .on_hover(move |inside, _, cx| {
                if !*inside {
                    set(&on_leave, None, cx);
                }
            })
            .child(
                InteractiveText::new("text", StyledText::new(self.text).with_highlights(lit))
                    .on_click(ranges, move |n, _, cx| {
                        cx.stop_propagation();
                        actions::open(targets[n].clone(), cx);
                    })
                    .on_hover(move |index, _, _, cx| {
                        let n = index.and_then(|index| {
                            hover_ranges.iter().position(|range| range.contains(&index))
                        });
                        set(&on_text, n, cx);
                    }),
            )
    }
}

/// One name that opens its page on click, underlined on hover like a link.
pub fn link_text(link: &Link, id: impl Into<ElementId>, hover: Hsla) -> AnyElement {
    let text = link.text.clone();
    match link.target.clone() {
        Some(target) => div()
            .id(id.into())
            .cursor_pointer()
            .hover(move |style| style.text_color(hover).underline())
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                actions::open(target.clone(), cx)
            })
            .child(text)
            .into_any_element(),
        None => div().child(text).into_any_element(),
    }
}

pub fn pill_button(
    id: &'static str,
    icon: IconName,
    label: &'static str,
    primary: bool,
    palette: Palette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let (fill, fg) = if primary {
        (palette.text, palette.canvas)
    } else {
        (palette.press_wash, palette.text)
    };
    div()
        .id(id)
        .h(px(36.))
        .pl(spacing::X3)
        .pr(spacing::X4)
        .rounded(radius::PILL)
        .bg(fill)
        .flex()
        .flex_row()
        .items_center()
        .gap(spacing::X2)
        .cursor_pointer()
        .tab_index(0)
        .border_2()
        .border_color(palette.transparent)
        .focus_visible(move |style| style.border_color(palette.focus_ring))
        .hover(|style| style.opacity(0.9))
        .active(|style| style.opacity(0.75))
        .on_click(on_click)
        .child(
            Icon::new(icon)
                .size(px(16.))
                .color(fg)
                .filled(icon == IconName::Play),
        )
        .child(
            div()
                .text_size(type_scale::BODY.font_size)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg)
                .child(label),
        )
}

/// The like and dislike pair beside a track.
pub fn rating_buttons(
    id: &str,
    rating: Rating,
    palette: Palette,
    on_rate: impl Fn(Rating, &mut App) + Clone + 'static,
) -> impl IntoElement {
    let like = on_rate.clone();
    div()
        .flex()
        .flex_row()
        .gap(spacing::X1)
        .child(
            IconButton::new(
                SharedString::from(format!("{id}-dislike")),
                IconName::Dislike,
                if rating == Rating::Dislike {
                    "Remove dislike"
                } else {
                    "Dislike"
                },
            )
            .filled(rating == Rating::Dislike)
            .color(if rating == Rating::Dislike {
                palette.text
            } else {
                palette.secondary
            })
            .on_click(move |_, _, cx| {
                on_rate(
                    if rating == Rating::Dislike {
                        Rating::Indifferent
                    } else {
                        Rating::Dislike
                    },
                    cx,
                )
            }),
        )
        .child(
            IconButton::new(
                SharedString::from(format!("{id}-like")),
                IconName::Like,
                if rating == Rating::Like {
                    "Remove from liked songs"
                } else {
                    "Like"
                },
            )
            .filled(rating == Rating::Like)
            .color(if rating == Rating::Like {
                palette.text
            } else {
                palette.secondary
            })
            .on_click(move |_, _, cx| {
                like(
                    if rating == Rating::Like {
                        Rating::Indifferent
                    } else {
                        Rating::Like
                    },
                    cx,
                )
            }),
        )
}

/// An album or playlist description: the first paragraph clamped to a few
/// lines with an ellipsis, and "More" to read all of it, source note
/// included, when there is more to read.
fn detail_description(
    text: &str,
    state: Description,
    palette: Palette,
    window: &mut Window,
) -> AnyElement {
    let first = one_paragraph(text);
    let more = first.len() < text.trim_end().len()
        || wrapped_lines(&first, type_scale::BODY.font_size, window) > DESCRIPTION_LINES;
    let body = div()
        .max_w(DESCRIPTION_WIDTH)
        .text_size(type_scale::BODY.font_size)
        .line_height(px(20.))
        .text_color(palette.secondary);
    let body = if state.open {
        body.flex().flex_col().gap(spacing::X2).children(
            text.split("\n\n")
                .map(|paragraph| paragraph.replace('\n', " ")),
        )
    } else {
        body.line_clamp(DESCRIPTION_LINES)
            .text_ellipsis()
            .child(first)
    };
    let toggle = state.toggle;
    div()
        .pt(spacing::X2)
        .flex()
        .flex_col()
        .items_start()
        .gap(spacing::X1)
        .child(body)
        .when(more, |el| {
            el.child(
                div()
                    .id("description-more")
                    .cursor_pointer()
                    .text_size(type_scale::BODY.font_size)
                    .line_height(px(20.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(palette.text)
                    .hover(|style| style.underline())
                    .on_click(move |_, _, cx| toggle(cx))
                    .child(if state.open { "Less" } else { "More" }),
            )
        })
        .into_any_element()
}

/// Lines `text` wraps to at the description's measure.
fn wrapped_lines(text: &str, font_size: Pixels, window: &mut Window) -> usize {
    let font = window.text_style().font();
    let run = TextRun {
        len: text.len(),
        font,
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_text(
            text.to_owned().into(),
            font_size,
            &[run],
            Some(DESCRIPTION_WIDTH),
            None,
        )
        .map(|lines| {
            lines
                .iter()
                .map(|line| line.wrap_boundaries().len() + 1)
                .sum()
        })
        .unwrap_or(0)
}

/// Descriptions come with blank lines and a source note; the clamped header
/// shows the first paragraph only.
fn one_paragraph(text: &str) -> String {
    text.split("\n\n").next().unwrap_or(text).replace('\n', " ")
}
