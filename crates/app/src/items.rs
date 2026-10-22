//! The pieces pages are made of: cards, track and item rows, mood and shortcut tiles, the
//! quick-picks grid, the top result, page headers and chips. Every row and card plays or opens
//! on a tap and carries its item's context menu.

use std::rc::Rc;

use formalmusic_api::{Chip, Header, Item, Link, PlaySource, Section, Track};
use formalmusic_core::Route;
use formalmusic_core::format;
use nd::core::semantic::Role;
use nd::prelude::*;

use crate::art;
use crate::menus::{self, MenuContext};
use crate::root;

pub const CARD_ART: f32 = 168.;
pub const ROW_ART: f32 = 40.;
pub const MOOD_TILE: f32 = 196.;
pub const QUICK_PICK_WIDTH: f32 = 340.;

pub type OnTap = Rc<dyn Fn(&mut Window, &mut App)>;

/// What a row needs to know about where it is.
#[derive(Clone)]
pub struct RowEnv {
    /// The playing track's id, and whether it is playing.
    pub playing: Option<(String, bool)>,
    pub menu: MenuContext,
    /// Hide artists that are the album's own, on an album page.
    pub hide_artist: Option<String>,
    /// Album and plays columns, when the page is wide enough.
    pub wide: bool,
}

fn tap(f: OnTap) -> impl Fn(&TapEvent, &mut Window, &mut App) + 'static {
    move |_, window, cx| f(window, cx)
}

/// Hover groups: Play and More fade in over the artwork of the card or row under the pointer.
const CARD: &str = "card";
const ROW: &str = "row";

/// Artwork in a box clipped to its corners, for the hover actions to sit over.
fn framed(art: Image, round: bool, corners: impl FnOnce(Div) -> Div) -> Div {
    let frame = div().relative().flex_none().overflow_hidden();
    if round {
        frame.rounded_full()
    } else {
        corners(frame)
    }
    .child(art)
}

/// Play on the hover actions when the item plays (a track, an album, a playlist).
fn actions(actions: ArtworkActions, item: &Item) -> ArtworkActions {
    if !menus::playable(item) {
        return actions;
    }
    let item = item.clone();
    actions.on_play(move |_, cx| menus::play_item(&item, &crate::store(cx), false))
}

fn item_menu(item: &Item) -> impl Fn(ContextMenu, &mut Window, &mut App) -> ContextMenu + 'static {
    let item = item.clone();
    move |menu, _, cx| menus::item_menu(menu, &item, &MenuContext::default(), cx)
}

/// What a tap on an item does: open its page, or for a track start its radio.
pub fn activate(item: &Item) -> OnTap {
    let item = item.clone();
    Rc::new(move |_, cx| match menus::target_of(&item) {
        Some(target) => root::navigate(Route::Browse(target), cx),
        None => menus::play_item(&item, &crate::store(cx), false),
    })
}

fn title_of(item: &Item) -> &str {
    match item {
        Item::Track(t) => &t.title,
        Item::Album { title, .. } | Item::Playlist { title, .. } | Item::Podcast { title, .. } => {
            title
        }
        Item::Mood { title, .. } | Item::Shortcut { title, .. } => title,
        Item::Artist { name, .. } => name,
    }
}

fn subtitle_of(item: &Item) -> String {
    match item {
        Item::Track(t) => format::byline(t),
        Item::Album {
            album_type,
            artists,
            year,
            ..
        } => {
            let mut parts: Vec<String> = Vec::new();
            parts.extend(album_type.clone());
            if !artists.is_empty() {
                parts.push(format::names(artists));
            }
            parts.extend(year.clone());
            parts.join(" \u{2022} ")
        }
        Item::Artist { subtitle, .. }
        | Item::Playlist { subtitle, .. }
        | Item::Podcast { subtitle, .. } => subtitle.clone().unwrap_or_default(),
        _ => String::new(),
    }
}

fn thumbnails_of(item: &Item) -> &[formalmusic_api::Thumbnail] {
    match item {
        Item::Track(t) => &t.thumbnails,
        Item::Album { thumbnails, .. }
        | Item::Artist { thumbnails, .. }
        | Item::Playlist { thumbnails, .. }
        | Item::Podcast { thumbnails, .. } => thumbnails,
        Item::Mood { .. } | Item::Shortcut { .. } => &[],
    }
}

fn explicit(item: &Item) -> bool {
    match item {
        Item::Track(t) => t.explicit,
        Item::Album { explicit, .. } => *explicit,
        _ => false,
    }
}

/// A card in a carousel or grid: artwork (round for artists), title and subtitle.
pub fn card(id: String, item: &Item, cx: &App) -> AnyElement {
    if let Item::Mood { title, color, .. } = item {
        return mood_tile(id, item, title, *color, cx);
    }
    if let Item::Shortcut { title, .. } = item {
        return button(id, title.clone())
            .on_click({
                let f = activate(item);
                move |_, w, cx| f(w, cx)
            })
            .into_any_element();
    }
    let round = matches!(item, Item::Artist { .. });
    let title = title_of(item).to_owned();
    let art = art::cover(
        format!("{id}.art"),
        thumbnails_of(item),
        CARD_ART,
        title.clone(),
    );
    let art = framed(art, round, |d| d.rounded_lg())
        .child(actions(artwork_actions(format!("{id}.actions"), CARD), item).menu(item_menu(item)));
    let menu_item = item.clone();
    v_flex()
        .id(id)
        .group(CARD)
        .role(Role::Button, title.clone())
        .w(px(CARD_ART))
        .flex_none()
        .gap_1()
        .child(art)
        .child(
            h_flex()
                .gap_1()
                .child(label(title).text_style(TextStyle::Headline).truncate())
                .when(explicit(item), |row| {
                    row.child(label("E").text_style(TextStyle::Caption2).secondary())
                }),
        )
        .child(
            label(subtitle_of(item))
                .text_style(TextStyle::Subheadline)
                .secondary()
                .truncate(),
        )
        .on_tap(tap(activate(item)))
        .when_some(menus::target_of(item), |card, target| {
            // A card under the pointer is likely the next click: its page loads ahead.
            card.on_hover(move |hovered, _, cx| {
                if *hovered {
                    crate::store(cx).prefetch(vec![target.clone()]);
                }
            })
        })
        .context_menu(move |menu, _, cx| {
            menus::item_menu(menu, &menu_item, &MenuContext::default(), cx)
        })
        .into_any_element()
}

fn mood_tile(id: String, item: &Item, title: &str, color: Option<u32>, cx: &App) -> AnyElement {
    let accent = color.map(|c| rgb(c).into()).unwrap_or(cx.theme().accent);
    h_flex()
        .id(id)
        .role(Role::Button, title.to_owned())
        .w(px(MOOD_TILE))
        .h(px(48.))
        .flex_none()
        .rounded_md()
        .bg(cx.theme().surface.raised)
        .overflow_hidden()
        .child(div().w(px(6.)).h_full().bg(accent))
        .child(
            label(title.to_owned())
                .text_style(TextStyle::Headline)
                .truncate()
                .px_3(),
        )
        .on_tap(tap(activate(item)))
        .into_any_element()
}

/// A list row for a track: art, title, artists, album and plays when wide, duration. Rows of
/// a virtual list pass `None` and act on the list's selection instead, so a click is one
/// action and assistive technology can press them.
pub fn track_row(
    id: String,
    track: &Track,
    env: &RowEnv,
    on_play: Option<OnTap>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let current = env
        .playing
        .as_ref()
        .is_some_and(|(id, _)| *id == track.video_id);
    let artists: Vec<&Link> = track
        .artists
        .iter()
        .filter(|a| env.hide_artist.as_deref() != Some(a.text.as_str()) || track.artists.len() > 1)
        .collect();
    let byline = artists
        .iter()
        .map(|a| a.text.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let item = Item::Track(track.clone());
    let menu = env.menu.clone();
    let title = text(track.title.clone())
        .truncate()
        .when(current, |t| t.text_color(theme.accent));
    let wash = theme.hover(theme.surface.base);
    let play = artwork_actions(format!("{id}.actions"), ROW).compact();
    let play = match &on_play {
        Some(f) => {
            let f = f.clone();
            play.on_play(move |w, cx| f(w, cx))
        }
        None => play,
    };
    let more = {
        let (item, menu) = (item.clone(), menu.clone());
        hover_menu_button(format!("{id}.more"), ROW, move |m, _, cx| {
            menus::item_menu(m, &item, &menu, cx)
        })
    };
    h_flex()
        .id(id)
        .group(ROW)
        .role(Role::ListItem, track.title.clone())
        .w_full()
        .h(px(56.))
        .px_3()
        .gap_3()
        .rounded(px(8.))
        .hover(move |s| s.bg(wash))
        .child(
            framed(
                art::cover(
                    format!("art.{}", track.video_id),
                    &track.thumbnails,
                    ROW_ART,
                    track.title.clone(),
                ),
                false,
                |d| d.rounded_sm(),
            )
            .child(play),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(h_flex().gap_1().child(title).when(track.explicit, |row| {
                    row.child(label("E").text_style(TextStyle::Caption2).secondary())
                }))
                .when(!byline.is_empty(), |col| {
                    col.child(
                        label(byline)
                            .text_style(TextStyle::Subheadline)
                            .secondary()
                            .truncate(),
                    )
                }),
        )
        .when(env.wide, |row| {
            row.child(
                label(
                    track
                        .album
                        .as_ref()
                        .map(|a| a.text.clone())
                        .unwrap_or_default(),
                )
                .secondary()
                .truncate()
                .w(px(220.)),
            )
            .child(
                label(track.plays.clone().unwrap_or_default())
                    .secondary()
                    .truncate()
                    .w(px(110.)),
            )
        })
        .child(more)
        .child(
            label(track.duration_ms.map(format::duration).unwrap_or_default())
                .secondary()
                .w(px(48.))
                .flex_none(),
        )
        .when_some(on_play, |row, on_play| row.on_tap(tap(on_play)))
        .context_menu(move |m, _, cx| menus::item_menu(m, &item, &menu, cx))
        .into_any_element()
}

/// A list row for anything but a track: art, title and subtitle. `tappable` as for
/// [`track_row`].
pub fn item_row(id: String, item: &Item, tappable: bool, cx: &App) -> AnyElement {
    let round = matches!(item, Item::Artist { .. });
    let title = title_of(item).to_owned();
    let art = art::cover(
        format!("{id}.art"),
        thumbnails_of(item),
        ROW_ART,
        title.clone(),
    );
    let actions = actions(
        artwork_actions(format!("{id}.actions"), ROW).compact(),
        item,
    );
    let art = framed(art, round, |d| d.rounded_sm()).child(actions);
    let menu_item = item.clone();
    let wash = cx.theme().hover(cx.theme().surface.base);
    let more = hover_menu_button(format!("{id}.more"), ROW, item_menu(item));
    h_flex()
        .id(id)
        .group(ROW)
        .role(Role::ListItem, title.clone())
        .w_full()
        .h(px(56.))
        .px_3()
        .gap_3()
        .rounded(px(8.))
        .hover(move |s| s.bg(wash))
        .child(art)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(text(title).truncate())
                .child(
                    label(subtitle_of(item))
                        .text_style(TextStyle::Subheadline)
                        .secondary()
                        .truncate(),
                ),
        )
        .child(more)
        .when(tappable, |row| row.on_tap(tap(activate(item))))
        .context_menu(move |m, _, cx| menus::item_menu(m, &menu_item, &MenuContext::default(), cx))
        .into_any_element()
}

/// A compact track for the quick picks grid and the top result's songs.
fn compact_track(id: String, track: &Track, on_play: OnTap, env: &RowEnv, cx: &App) -> AnyElement {
    let current = env
        .playing
        .as_ref()
        .is_some_and(|(id, _)| *id == track.video_id);
    let item = Item::Track(track.clone());
    let menu = env.menu.clone();
    let play = {
        let f = on_play.clone();
        artwork_actions(format!("{id}.actions"), ROW)
            .compact()
            .on_play(move |w, cx| f(w, cx))
    };
    let more = {
        let (item, menu) = (item.clone(), menu.clone());
        hover_menu_button(format!("{id}.more"), ROW, move |m, _, cx| {
            menus::item_menu(m, &item, &menu, cx)
        })
    };
    let wash = cx.theme().hover(cx.theme().surface.base);
    h_flex()
        .id(id)
        .group(ROW)
        .role(Role::ListItem, track.title.clone())
        .w(px(QUICK_PICK_WIDTH))
        .flex_none()
        .h(px(52.))
        .px_2()
        .gap_3()
        .rounded(px(8.))
        .hover(move |s| s.bg(wash))
        .child(
            framed(
                art::cover(
                    format!("qp.{}", track.video_id),
                    &track.thumbnails,
                    ROW_ART,
                    track.title.clone(),
                ),
                false,
                |d| d.rounded_sm(),
            )
            .child(play),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    text(track.title.clone())
                        .truncate()
                        .when(current, |t| t.text_color(cx.theme().accent)),
                )
                .child(
                    label(format::byline(track))
                        .text_style(TextStyle::Subheadline)
                        .secondary()
                        .truncate(),
                ),
        )
        .child(more)
        .on_tap(tap(on_play))
        .context_menu(move |m, _, cx| menus::item_menu(m, &item, &menu, cx))
        .into_any_element()
}

/// Plays `section`'s tracks from the `index`th.
pub fn play_tracks(tracks: Vec<Track>, index: usize) -> OnTap {
    Rc::new(move |_, cx| {
        crate::store(cx).play(
            PlaySource::Tracks {
                tracks: tracks.clone(),
            },
            index,
            false,
            false,
        )
    })
}

fn tracks_of(section: &Section) -> Vec<Track> {
    section
        .items
        .iter()
        .filter_map(|i| {
            if let Item::Track(t) = i {
                Some(t.clone())
            } else {
                None
            }
        })
        .collect()
}

/// Quick picks: tracks in columns of four rows, scrolled sideways.
pub fn track_grid(
    id: String,
    section: &Section,
    handle: &ShelfHandle,
    env: &RowEnv,
    cx: &App,
) -> AnyElement {
    let tracks = tracks_of(section);
    let columns: Vec<_> = tracks
        .chunks(4)
        .enumerate()
        .map(|(c, chunk)| {
            v_flex()
                .gap_1()
                .flex_none()
                .children(chunk.iter().enumerate().map(|(r, t)| {
                    compact_track(
                        format!("{id}.{}", c * 4 + r),
                        t,
                        play_tracks(tracks.clone(), c * 4 + r),
                        env,
                        cx,
                    )
                }))
        })
        .collect();
    shelf(id, handle)
        .bleed(crate::page::PAGE_INSET)
        .label(section.title.clone().unwrap_or_default())
        .gap_4()
        .h(px(4. * 52. + 3. * 4. + 8.))
        .flex_none()
        .children(columns)
        .into_any_element()
}

/// A carousel of cards scrolled sideways.
pub fn carousel(id: String, section: &Section, handle: &ShelfHandle, cx: &App) -> AnyElement {
    shelf(id.clone(), handle)
        .bleed(crate::page::PAGE_INSET)
        .label(section.title.clone().unwrap_or_default())
        .gap_4()
        .h(px(CARD_ART + 60.))
        .flex_none()
        .children(
            section
                .items
                .iter()
                .enumerate()
                .map(|(i, item)| card(format!("{id}.{i}"), item, cx)),
        )
        .into_any_element()
}

/// The top result: its card with Play, Shuffle and Radio, and up to four songs beside it.
pub fn hero(id: String, section: &Section, env: &RowEnv, cx: &App) -> AnyElement {
    let Some(top) = section.items.first() else {
        return div().into_any_element();
    };
    let tracks = tracks_of(section);
    let songs: Vec<Track> = section
        .items
        .iter()
        .skip(1)
        .filter_map(|i| {
            if let Item::Track(t) = i {
                Some(t.clone())
            } else {
                None
            }
        })
        .take(4)
        .collect();
    let (shuffle_id, radio_id) = (
        section.shuffle_playlist_id.clone(),
        section.radio_playlist_id.clone(),
    );
    let play_top = top.clone();
    let buttons = h_flex()
        .gap_2()
        .child(
            button(format!("{id}.play"), "Play")
                .accent()
                .on_click(move |_, _, cx| menus::play_item(&play_top, &crate::store(cx), false)),
        )
        .when_some(shuffle_id, |row, playlist_id| {
            row.child(
                button(format!("{id}.shuffle"), "Shuffle").on_click(move |_, _, cx| {
                    crate::store(cx).play(
                        PlaySource::Playlist {
                            playlist_id: playlist_id.clone(),
                            tracks: Vec::new(),
                        },
                        0,
                        true,
                        false,
                    )
                }),
            )
        })
        .when_some(radio_id, |row, playlist_id| {
            row.child(
                button(format!("{id}.radio"), "Radio").on_click(move |_, _, cx| {
                    crate::store(cx).play(
                        PlaySource::Playlist {
                            playlist_id: playlist_id.clone(),
                            tracks: Vec::new(),
                        },
                        0,
                        false,
                        true,
                    )
                }),
            )
        });
    let first_song = tracks.len().saturating_sub(songs.len());
    h_flex()
        .id(id.clone())
        .gap_6()
        .flex_wrap()
        .child(
            v_flex()
                .gap_2()
                .child(card(format!("{id}.top"), top, cx))
                .child(buttons),
        )
        .child(
            v_flex()
                .gap_1()
                .children(songs.iter().enumerate().map(|(i, t)| {
                    compact_track(
                        format!("{id}.song{i}"),
                        t,
                        play_tracks(tracks.clone(), first_song + i),
                        env,
                        cx,
                    )
                })),
        )
        .into_any_element()
}

/// What picking a chip does.
pub type OnChip = Rc<dyn Fn(&Chip, &mut Window, &mut App)>;

/// Chips under the header: Home's moods, Library's tabs, a search's filters.
pub fn chips(id: &str, chips: &[Chip], handle: &ShelfHandle, on_select: OnChip) -> AnyElement {
    shelf(id.to_owned(), handle)
        .bleed(crate::page::PAGE_INSET)
        .label("Filters")
        .gap_2()
        .h(px(36.))
        .flex_none()
        .children(chips.iter().enumerate().map(|(i, chip)| {
            let on_select = on_select.clone();
            let chip_value = chip.clone();
            button(format!("{id}.{i}"), chip.title.clone())
                .when(chip.selected, |b| b.accent())
                .on_click(move |_, w, cx| on_select(&chip_value, w, cx))
        }))
        .into_any_element()
}

/// The page header: album or playlist details, an artist, or a plain title.
/// A description clamped to three lines that opens in place when clicked, as Music's album
/// notes do; one short enough to fit shows whole and does nothing.
fn description_text(text: String, open: bool, on_toggle: OnTap) -> AnyElement {
    let long = text.chars().count() > DESCRIPTION_FITS || text.lines().count() > 3;
    let body = label(text).secondary().wraps().max_w(px(640.));
    if !long {
        return body.into_any_element();
    }
    div()
        .id("header.description")
        .max_w(px(640.))
        .role(
            Role::Button,
            if open { "Collapse description" } else { "Expand description" },
        )
        .on_tap(move |_, w, cx| on_toggle(w, cx))
        .child(body.when(!open, |l| l.line_clamp(3)))
        .into_any_element()
}

/// Characters three lines of the description hold at its widest.
const DESCRIPTION_FITS: usize = 280;

pub fn header(header: &Header, description_open: bool, on_toggle: OnTap, cx: &App) -> AnyElement {
    let theme = cx.theme();
    match header {
        Header::Title { title } => label(title.clone())
            .text_style(TextStyle::LargeTitle)
            .into_any_element(),
        Header::Detail {
            title,
            subtitle,
            second_subtitle,
            description,
            thumbnails,
            playlist_id,
            editable,
            saved,
            privacy,
        } => {
            let links = h_flex().gap_1().flex_wrap().children(
                subtitle
                    .iter()
                    .enumerate()
                    .map(|(i, part)| link_or_text(format!("header.sub{i}"), part)),
            );
            let actions =
                h_flex()
                    .gap_2()
                    .when_some(playlist_id.clone(), |row, playlist_id| {
                        let (a, b) = (playlist_id.clone(), playlist_id.clone());
                        row.child(button("header.play", "Play").icon(Icon::Play).accent().on_click(
                            move |_, _, cx| {
                                crate::store(cx).play(
                                    PlaySource::Playlist {
                                        playlist_id: a.clone(),
                                        tracks: Vec::new(),
                                    },
                                    0,
                                    false,
                                    false,
                                )
                            },
                        ))
                        .child(
                            button("header.shuffle", "Shuffle").icon(Icon::Shuffle).on_click(move |_, _, cx| {
                                crate::store(cx).play(
                                    PlaySource::Playlist {
                                        playlist_id: b.clone(),
                                        tracks: Vec::new(),
                                    },
                                    0,
                                    true,
                                    false,
                                )
                            }),
                        )
                    })
                    .when_some(
                        saved.zip(playlist_id.clone()),
                        |row, (saved, playlist_id)| {
                            row.child(
                                button(
                                    "header.save",
                                    if saved {
                                        "Remove from library"
                                    } else {
                                        "Save to library"
                                    },
                                )
                                .on_click(move |_, _, cx| {
                                    crate::store(cx).set_in_library(playlist_id.clone(), !saved)
                                }),
                            )
                        },
                    )
                    .when(*editable && privacy.is_some(), |row| {
                        let current = crate::sheets::EditPlaylist {
                            playlist_id: playlist_id.clone().unwrap_or_default(),
                            title: title.clone(),
                            description: description.clone().unwrap_or_default(),
                            privacy: privacy.unwrap_or(formalmusic_api::Privacy::Private),
                        };
                        row.child(button("header.edit", "Edit playlist").on_click(
                            move |_, window, cx| {
                                crate::sheets::edit_playlist(current.clone(), window, cx)
                            },
                        ))
                    });
            h_flex()
                .gap_6()
                .items_end()
                .child(art::cover("header.art", thumbnails, 232., title.clone()).rounded_lg())
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_2()
                        .child(label(title.clone()).text_style(TextStyle::LargeTitle))
                        .child(links)
                        .when_some(second_subtitle.clone(), |col, s| {
                            col.child(label(s).secondary())
                        })
                        .when_some(description.clone(), |col, d| {
                            col.child(description_text(d, description_open, on_toggle.clone()))
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
            let subscribe = subscribed.map(|on| {
                let channel = channel_id.clone().unwrap_or_default();
                let count = subscribers.clone().unwrap_or_default();
                let label = if on {
                    format!("Subscribed \u{2022} {count}")
                } else {
                    format!("Subscribe \u{2022} {count}")
                };
                button("header.subscribe", label)
                    .on_click(move |_, _, cx| crate::store(cx).set_subscribed(channel.clone(), !on))
            });
            v_flex()
                .gap_3()
                .child(art::cover("header.art", thumbnails, 232., name.clone()).rounded_full())
                .child(label(name.clone()).text_style(TextStyle::LargeTitle))
                .when_some(monthly_listeners.clone(), |col, l| {
                    col.child(label(l).secondary())
                })
                .when_some(description.clone(), |col, d| {
                    col.child(description_text(d, description_open, on_toggle.clone()))
                })
                .child(
                    h_flex()
                        .gap_2()
                        .when_some(shuffle_playlist_id.clone(), |row, id| {
                            row.child(button("header.shuffle", "Shuffle").icon(Icon::Shuffle).accent().on_click(
                                move |_, _, cx| {
                                    crate::store(cx).play(
                                        PlaySource::Playlist {
                                            playlist_id: id.clone(),
                                            tracks: Vec::new(),
                                        },
                                        0,
                                        true,
                                        false,
                                    )
                                },
                            ))
                        })
                        .when_some(radio_playlist_id.clone(), |row, id| {
                            row.child(button("header.radio", "Radio").on_click(move |_, _, cx| {
                                crate::store(cx).play(
                                    PlaySource::Playlist {
                                        playlist_id: id.clone(),
                                        tracks: Vec::new(),
                                    },
                                    0,
                                    false,
                                    true,
                                )
                            }))
                        })
                        .children(subscribe),
                )
                .text_color(theme.text.primary)
                .into_any_element()
        }
    }
}

/// A subtitle part: a link to its page when it has one.
pub fn link_or_text(id: String, part: &Link) -> AnyElement {
    match part.target.clone() {
        Some(target) => link(id, part.text.clone())
            .keep_internal()
            .on_click(move |_, _, cx| root::navigate(Route::Browse(target.clone()), cx))
            .into_any_element(),
        None => label(part.text.clone()).secondary().into_any_element(),
    }
}
