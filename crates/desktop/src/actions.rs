//! What clicking an item does, and the context menu every item offers.

use formalmusic_api::{
    BrowseTarget, EnqueuePosition, Item, LibraryTab, Link, PlaySource, Rating, Track,
};
use formalmusic_core::{MusicStore, Route};
use gpui_kit::*;

use crate::app::{self, AppRoot};
use crate::icons::IconName;
use crate::menus::{MenuItem, MenuRequest};

/// Where a click on the item goes. Tracks have none: a click plays them.
pub fn target_of(item: &Item) -> Option<BrowseTarget> {
    match item {
        Item::Track(_) => None,
        Item::Album { browse_id, .. } => Some(BrowseTarget::Album(browse_id.clone())),
        Item::Artist { browse_id, .. } => Some(BrowseTarget::Artist(browse_id.clone())),
        Item::Playlist { playlist_id, .. } => Some(BrowseTarget::Playlist(playlist_id.clone())),
        Item::Podcast { browse_id, .. } => Some(BrowseTarget::Podcast(browse_id.clone())),
        Item::Mood { params, .. } => Some(BrowseTarget::MoodCategory {
            params: params.clone(),
        }),
        Item::Shortcut { target, .. } => Some(target.clone()),
    }
}

pub fn open(target: BrowseTarget, cx: &mut App) {
    app::navigate(Route::Browse(target), cx);
}

/// The play button on a card: albums and playlists play whole, a song starts
/// its radio, as on the web app.
pub fn play_item(item: &Item, store: &MusicStore) {
    match item {
        Item::Track(track) => store.play(
            PlaySource::Radio {
                video_id: track.video_id.clone(),
            },
            0,
            false,
            true,
        ),
        Item::Album {
            playlist_id: Some(playlist_id),
            ..
        }
        | Item::Playlist { playlist_id, .. } => store.play(
            PlaySource::Playlist {
                playlist_id: playlist_id.clone(),
                tracks: Vec::new(),
            },
            0,
            false,
            false,
        ),
        _ => {}
    }
}

/// Whether a card for this item has a play button.
pub fn playable(item: &Item) -> bool {
    matches!(
        item,
        Item::Track(_)
            | Item::Album {
                playlist_id: Some(_),
                ..
            }
            | Item::Playlist { .. }
    )
}

/// Where a menu came from, which decides the rows that only make sense there.
#[derive(Clone, Default)]
pub struct MenuContext {
    /// The playlist page the row sits on, when it is one you can edit.
    pub editable_playlist: Option<String>,
}

pub fn open_menu(position: Point<Pixels>, items: Vec<MenuItem>, window: &mut Window, cx: &mut App) {
    if let Some(root) = app::root(cx) {
        AppRoot::open_menu(&root, MenuRequest::at(position, items), window, cx);
    }
}

/// `open_menu` growing upwards, for the player bar along the bottom.
pub fn open_menu_above(
    position: Point<Pixels>,
    items: Vec<MenuItem>,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(root) = app::root(cx) {
        AppRoot::open_menu(&root, MenuRequest::at(position, items).above(), window, cx);
    }
}

pub fn item_menu(item: &Item, context: &MenuContext, store: &MusicStore) -> Vec<MenuItem> {
    match item {
        Item::Track(track) => track_menu(track, context, store),
        Item::Album {
            playlist_id,
            artists,
            browse_id,
            ..
        } => {
            let mut items = Vec::new();
            if let Some(playlist_id) = playlist_id.clone() {
                let (play, save) = (store.clone(), store.clone());
                let save_id = playlist_id.clone();
                items.push(
                    MenuItem::item("Play", move |_, _| {
                        play.play(
                            PlaySource::Playlist {
                                playlist_id: playlist_id.clone(),
                                tracks: Vec::new(),
                            },
                            0,
                            false,
                            false,
                        )
                    })
                    .icon(IconName::Play),
                );
                items.push(
                    MenuItem::item("Save to library", move |_, _| {
                        save.set_in_library(save_id.clone(), true)
                    })
                    .icon(IconName::Saved),
                );
            }
            let target = BrowseTarget::Album(browse_id.clone());
            items.push(
                MenuItem::item("Go to album", move |_, cx| open(target.clone(), cx))
                    .icon(IconName::Album),
            );
            items.extend(artist_rows(artists));
            items
        }
        Item::Playlist { playlist_id, .. } => {
            let (play, shuffle, save) = (store.clone(), store.clone(), store.clone());
            let (a, b, c) = (
                playlist_id.clone(),
                playlist_id.clone(),
                playlist_id.clone(),
            );
            vec![
                MenuItem::item("Play", move |_, _| {
                    play.play(
                        PlaySource::Playlist {
                            playlist_id: a.clone(),
                            tracks: Vec::new(),
                        },
                        0,
                        false,
                        false,
                    )
                })
                .icon(IconName::Play),
                MenuItem::item("Shuffle", move |_, _| {
                    shuffle.play(
                        PlaySource::Playlist {
                            playlist_id: b.clone(),
                            tracks: Vec::new(),
                        },
                        0,
                        true,
                        false,
                    )
                })
                .icon(IconName::Shuffle),
                MenuItem::item("Save to library", move |_, _| {
                    save.set_in_library(c.clone(), true)
                })
                .icon(IconName::Saved),
            ]
        }
        Item::Artist { browse_id, .. } => {
            let target = BrowseTarget::Artist(browse_id.clone());
            vec![
                MenuItem::item("Go to artist", move |_, cx| open(target.clone(), cx))
                    .icon(IconName::Artist),
            ]
        }
        Item::Podcast { .. } | Item::Mood { .. } | Item::Shortcut { .. } => {
            let Some(target) = target_of(item) else {
                return Vec::new();
            };
            vec![MenuItem::item("Open", move |_, cx| open(target.clone(), cx)).icon(IconName::Open)]
        }
    }
}

fn artist_rows(artists: &[Link]) -> Vec<MenuItem> {
    artists
        .iter()
        .filter_map(|artist| {
            let target = artist.target.clone()?;
            let label = if artists.len() > 1 {
                format!("Go to {}", artist.text)
            } else {
                "Go to artist".to_owned()
            };
            Some(
                MenuItem::item(label, move |_, cx| open(target.clone(), cx)).icon(IconName::Artist),
            )
        })
        .collect()
}

pub fn track_menu(track: &Track, context: &MenuContext, store: &MusicStore) -> Vec<MenuItem> {
    let rating = store.state().rating(track);
    let mut items = Vec::new();
    {
        let (store, video_id) = (store.clone(), track.video_id.clone());
        items.push(
            MenuItem::item("Start radio", move |_, _| {
                store.play(
                    PlaySource::Radio {
                        video_id: video_id.clone(),
                    },
                    0,
                    false,
                    true,
                )
            })
            .icon(IconName::Radio),
        );
    }
    for (label, icon, position) in [
        ("Play next", IconName::PlayNext, EnqueuePosition::Next),
        ("Add to queue", IconName::AddToQueue, EnqueuePosition::End),
    ] {
        let (store, track) = (store.clone(), track.clone());
        items.push(
            MenuItem::item(label, move |_, _| {
                store.enqueue(vec![track.clone()], position)
            })
            .icon(icon),
        );
    }
    {
        let (store, video_id) = (store.clone(), track.video_id.clone());
        items.push(
            MenuItem::item("Add to playlist\u{2026}", move |window, cx| {
                let position = window.mouse_position();
                let rows = playlist_picker(&store, vec![video_id.clone()]);
                // The first menu closes after this returns; the picker opens on the next frame.
                window.defer(cx, move |window, cx| open_menu(position, rows, window, cx));
            })
            .icon(IconName::AddToPlaylist),
        );
    }
    if let Some(playlist_id) = context
        .editable_playlist
        .clone()
        .filter(|_| track.set_video_id.is_some())
    {
        let (store, track) = (store.clone(), track.clone());
        items.push(
            MenuItem::item("Remove from playlist", move |_, _| {
                store.remove_from_playlist(playlist_id.clone(), &track)
            })
            .icon(IconName::Remove)
            .danger(),
        );
    }
    if track.feedback_token.is_some() {
        let (store, track) = (store.clone(), track.clone());
        items.push(
            MenuItem::item("Remove from history", move |_, _| {
                store.remove_from_history(&track)
            })
            .icon(IconName::Remove)
            .danger(),
        );
    }
    items.push(MenuItem::Separator);
    {
        let (store, track) = (store.clone(), track.clone());
        let (label, next) = if rating == Rating::Like {
            ("Remove from liked songs", Rating::Indifferent)
        } else {
            ("Like", Rating::Like)
        };
        items
            .push(MenuItem::item(label, move |_, _| store.rate(&track, next)).icon(IconName::Like));
    }
    {
        let (store, track) = (store.clone(), track.clone());
        let (label, next) = if rating == Rating::Dislike {
            ("Remove dislike", Rating::Indifferent)
        } else {
            ("Dislike", Rating::Dislike)
        };
        items.push(
            MenuItem::item(label, move |_, _| store.rate(&track, next)).icon(IconName::Dislike),
        );
    }
    let album = track.album.as_ref().and_then(|album| album.target.clone());
    let artists = artist_rows(&track.artists);
    if album.is_some() || !artists.is_empty() {
        items.push(MenuItem::Separator);
    }
    if let Some(target) = album {
        items.push(
            MenuItem::item("Go to album", move |_, cx| open(target.clone(), cx))
                .icon(IconName::Album),
        );
    }
    items.extend(artists);
    items
}

/// The second menu "Add to playlist" opens: your playlists, newest first.
fn playlist_picker(store: &MusicStore, video_ids: Vec<String>) -> Vec<MenuItem> {
    let state = store.state();
    let mut rows = vec![MenuItem::Header("Add to playlist".into())];
    for item in state.library_playlists() {
        let Item::Playlist {
            playlist_id, title, ..
        } = item
        else {
            continue;
        };
        if playlist_id == "LM" {
            continue;
        }
        let (store, playlist_id, video_ids) =
            (store.clone(), playlist_id.clone(), video_ids.clone());
        rows.push(
            MenuItem::item(title.clone(), move |_, _| {
                store.add_to_playlist(playlist_id.clone(), video_ids.clone())
            })
            .icon(IconName::Playlist),
        );
    }
    if rows.len() == 1 {
        rows.push(MenuItem::Note("No playlists yet".into()));
        let target = BrowseTarget::Library(LibraryTab::Playlists);
        rows.push(MenuItem::item("Open library", move |_, cx| {
            open(target.clone(), cx)
        }));
    }
    rows
}

/// Which hover is current, so leaving a card before the delay cancels its fetch.
#[derive(Default)]
struct HoverPrefetch {
    generation: u64,
}

impl Global for HoverPrefetch {}

/// Long enough that sweeping the pointer across a shelf fetches nothing.
const HOVER_DELAY: std::time::Duration = std::time::Duration::from_millis(80);

/// Fetches the page a card leads to once the pointer has rested on it, so
/// the click finds it cached.
pub fn prefetch_on_hover(target: Option<BrowseTarget>, hovered: bool, cx: &mut App) {
    if !cx.has_global::<HoverPrefetch>() {
        cx.set_global(HoverPrefetch::default());
    }
    let generation = {
        let state = cx.global_mut::<HoverPrefetch>();
        state.generation += 1;
        state.generation
    };
    let (Some(target), true) = (target, hovered) else {
        return;
    };
    let Some(store) = crate::bridge::store(cx) else {
        return;
    };
    cx.spawn(async move |cx| {
        cx.background_executor().timer(HOVER_DELAY).await;
        if cx.update(|cx| cx.global::<HoverPrefetch>().generation == generation) {
            store.open(target);
        }
    })
    .detach();
}
