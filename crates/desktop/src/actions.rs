//! What clicking an item does, and the context menu every item offers.

use formalmusic_core::model::{
    Actions, BrowseTarget, EnqueuePosition, Item, LibraryTab, Link, PlaySource, Rating, Track,
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
        Item::Mood { id, .. } => Some(BrowseTarget::Mood(id.clone())),
        Item::Shortcut { target, .. } => Some(target.clone()),
    }
}

pub fn open(target: BrowseTarget, cx: &mut App) {
    app::navigate(Route::Browse(target), cx);
}

/// What the play button on a card starts: albums and playlists play whole,
/// a song starts its radio, as on the web app.
pub fn play_source(item: &Item) -> Option<PlaySource> {
    match item {
        Item::Track(track) => Some(PlaySource::Radio {
            key: track.key.clone(),
        }),
        Item::Album { .. } | Item::Playlist { .. } => Some(PlaySource::Page {
            target: target_of(item)?,
        }),
        _ => None,
    }
}

pub fn play_item(item: &Item, store: &MusicStore) {
    if let Some(source) = play_source(item) {
        store.play(source, 0, false);
    }
}

/// Whether a card for this item has a play button.
pub fn playable(item: &Item) -> bool {
    play_source(item).is_some()
}

/// Where a menu came from, which decides the rows that only make sense there.
#[derive(Clone, Default)]
pub struct MenuContext {
    /// The playlist page the row sits on, when it is one you can edit.
    pub editable_playlist: Option<String>,
    /// The row's place in that playlist's list.
    pub row: Option<usize>,
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
            artists, actions, ..
        } => {
            let mut items = play_rows(item, store, false);
            items.extend(save_row(actions, store));
            if let Some(target) = target_of(item) {
                items.push(
                    MenuItem::item("Go to album", move |_, cx| open(target.clone(), cx))
                        .icon(IconName::Album),
                );
            }
            items.extend(artist_rows(artists));
            items.extend(share_row(item, store));
            items
        }
        Item::Playlist { actions, .. } => {
            let mut items = play_rows(item, store, true);
            items.extend(save_row(actions, store));
            items.extend(share_row(item, store));
            items
        }
        Item::Artist { browse_id, .. } => {
            let target = BrowseTarget::Artist(browse_id.clone());
            std::iter::once(
                MenuItem::item("Go to artist", move |_, cx| open(target.clone(), cx))
                    .icon(IconName::Artist),
            )
            .chain(share_row(item, store))
            .collect()
        }
        Item::Podcast { .. } | Item::Mood { .. } | Item::Shortcut { .. } => {
            let Some(target) = target_of(item) else {
                return Vec::new();
            };
            std::iter::once(
                MenuItem::item("Open", move |_, cx| open(target.clone(), cx)).icon(IconName::Open),
            )
            .chain(share_row(item, store))
            .collect()
        }
    }
}

/// "Play", and "Shuffle" where it makes sense, for an album or a playlist.
fn play_rows(item: &Item, store: &MusicStore, shuffle: bool) -> Vec<MenuItem> {
    let Some(source) = play_source(item) else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    {
        let (store, source) = (store.clone(), source.clone());
        rows.push(
            MenuItem::item("Play", move |_, _| store.play(source.clone(), 0, false))
                .icon(IconName::Play),
        );
    }
    if shuffle {
        let store = store.clone();
        rows.push(
            MenuItem::item("Shuffle", move |_, _| store.play(source.clone(), 0, true))
                .icon(IconName::Shuffle),
        );
    }
    rows
}

/// "Save to library" for an album or someone else's playlist not saved yet.
fn save_row(actions: &Actions, store: &MusicStore) -> Option<MenuItem> {
    let save_ref = actions.save_ref.clone()?;
    if actions.saved == Some(true) {
        return None;
    }
    let store = store.clone();
    Some(
        MenuItem::item("Save to library", move |_, _| {
            store.set_in_library(save_ref.clone(), true)
        })
        .icon(IconName::Saved),
    )
}

/// Whether Share has a link to copy for the item.
fn shareable(item: &Item) -> bool {
    matches!(
        item,
        Item::Track(_)
            | Item::Album { .. }
            | Item::Artist { .. }
            | Item::Playlist { .. }
            | Item::Podcast { .. }
    )
}

/// "Share": asks kopuzd for the item's link, copies it and says so.
fn share_row(item: &Item, store: &MusicStore) -> Option<MenuItem> {
    if !shareable(item) {
        return None;
    }
    let (item, store) = (item.clone(), store.clone());
    Some(
        MenuItem::item("Share", move |_, cx| {
            let (item, asking) = (item.clone(), store.clone());
            let task = store
                .runtime()
                .spawn(async move { asking.share_url(&item).await });
            let store = store.clone();
            cx.spawn(async move |cx| {
                let url = task.await.ok().flatten();
                cx.update(|cx| match url {
                    Some(url) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(url));
                        store.confirm("Link copied".into());
                    }
                    None => store.notice("There is no link to share for that.".into()),
                });
            })
            .detach();
        })
        .icon(IconName::Share),
    )
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
        let (store, key) = (store.clone(), track.key.clone());
        items.push(
            MenuItem::item("Start radio", move |_, _| {
                store.play(PlaySource::Radio { key: key.clone() }, 0, false)
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
        let (store, key) = (store.clone(), track.key.clone());
        items.push(
            MenuItem::item("Add to playlist\u{2026}", move |window, cx| {
                let position = window.mouse_position();
                let rows = playlist_picker(&store, vec![key.clone()]);
                // The first menu closes after this returns; the picker opens on the next frame.
                window.defer(cx, move |window, cx| open_menu(position, rows, window, cx));
            })
            .icon(IconName::AddToPlaylist),
        );
    }
    if let Some(saved) = store.state().in_library(track) {
        let (store, track) = (store.clone(), track.clone());
        let (label, icon) = if saved {
            ("Remove from library", IconName::Unsave)
        } else {
            ("Save to library", IconName::Saved)
        };
        items.push(
            MenuItem::item(label, move |_, _| store.set_song_in_library(&track, !saved)).icon(icon),
        );
    }
    if let (Some(playlist_id), Some(row)) = (context.editable_playlist.clone(), context.row) {
        let store = store.clone();
        items.push(
            MenuItem::item("Remove from playlist", move |_, _| {
                store.remove_from_playlist(playlist_id.clone(), row)
            })
            .icon(IconName::Remove)
            .danger(),
        );
    }
    if track.actions.history_token.is_some() {
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
    items.push(MenuItem::Separator);
    items.extend(share_row(&Item::Track(track.clone()), store));
    items
}

/// The second menu "Add to playlist" opens: your playlists, newest first.
fn playlist_picker(store: &MusicStore, keys: Vec<String>) -> Vec<MenuItem> {
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
        let (store, playlist_id, keys) = (store.clone(), playlist_id.clone(), keys.clone());
        rows.push(
            MenuItem::item(title.clone(), move |_, _| {
                store.add_to_playlist(playlist_id.clone(), keys.clone())
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
