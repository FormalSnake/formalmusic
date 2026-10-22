//! Context menus for tracks, albums, playlists, artists and queue rows. nd menus dispatch
//! actions, so the item a menu was built for is parked in a small registry when the menu opens
//! and the chosen row arrives as one [`ItemCommand`] naming it and the operation.

use std::cell::RefCell;
use std::collections::VecDeque;

use formalmusic_api::{
    BrowseTarget, EnqueuePosition, Item, LibraryTab, Link, PlaySource, Rating, Track,
};
use formalmusic_core::{MusicStore, Route};
use nd::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;

/// A menu row chosen: `op` on the item registered as `target`.
#[derive(Clone, PartialEq, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = formalmusic)]
pub struct ItemCommand {
    pub target: u64,
    pub op: Op,
}

#[derive(Clone, Copy, PartialEq, Debug, Deserialize, JsonSchema)]
pub enum Op {
    Open,
    Play,
    Shuffle,
    StartRadio,
    PlayNext,
    AddToQueue,
    /// The playlist at this index of the menu's playlist rows.
    AddToPlaylist(usize),
    SaveToLibrary(bool),
    RemoveFromPlaylist,
    RemoveFromHistory,
    Rate(RatingOp),
    /// The album, or the artist at this index of the track's artists.
    GoToAlbum,
    GoToArtist(usize),
    Share,
    /// Queue rows: jump, remove, move by `delta` rows.
    QueueRemove,
    QueueMove(i32),
    /// Editable playlist rows: move by `delta` rows.
    PlaylistMove(i32),
}

#[derive(Clone, Copy, PartialEq, Debug, Deserialize, JsonSchema)]
pub enum RatingOp {
    Like,
    Dislike,
    Indifferent,
}

impl From<RatingOp> for Rating {
    fn from(op: RatingOp) -> Rating {
        match op {
            RatingOp::Like => Rating::Like,
            RatingOp::Dislike => Rating::Dislike,
            RatingOp::Indifferent => Rating::Indifferent,
        }
    }
}

/// Where a menu came from, which decides the rows that only make sense there.
#[derive(Clone, Default, Debug)]
pub struct MenuContext {
    /// The playlist page the row sits on, when it is one you can edit, and the row's index.
    pub editable_playlist: Option<(String, usize)>,
    /// The row's index in the queue, for the queue's own rows.
    pub queue_index: Option<usize>,
}

struct Entry {
    id: u64,
    item: Item,
    context: MenuContext,
    playlists: Vec<String>,
}

thread_local! {
    static OPEN: RefCell<(u64, VecDeque<Entry>)> = const { RefCell::new((0, VecDeque::new())) };
}

/// Menus stay answerable for this many later menus, far more than can be open at once.
const KEEP: usize = 16;

fn register(item: &Item, context: &MenuContext, playlists: Vec<String>) -> u64 {
    OPEN.with(|open| {
        let (next, entries) = &mut *open.borrow_mut();
        *next += 1;
        if entries.len() == KEEP {
            entries.pop_front();
        }
        entries.push_back(Entry {
            id: *next,
            item: item.clone(),
            context: context.clone(),
            playlists,
        });
        *next
    })
}

/// The web app's item targets: what a click on a card opens. A track plays instead.
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

/// The play button on a card: albums and playlists play whole, a song starts its radio.
/// Whether [`play_item`] plays `item`.
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

pub fn play_item(item: &Item, store: &MusicStore, shuffle: bool) {
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
            shuffle,
            false,
        ),
        _ => {}
    }
}

/// The web app's link to an item, as Share copies it.
pub fn share_url(item: &Item) -> Option<String> {
    const BASE: &str = "https://music.youtube.com";
    Some(match item {
        Item::Track(track) => format!("{BASE}/watch?v={}", track.video_id),
        Item::Album {
            playlist_id: Some(playlist_id),
            ..
        }
        | Item::Playlist { playlist_id, .. } => {
            // A playlist's browse id, as Kopuz hands it out, is its list id behind `VL`.
            let list = playlist_id.strip_prefix("VL").unwrap_or(playlist_id);
            format!("{BASE}/playlist?list={list}")
        }
        Item::Album { browse_id, .. } => format!("{BASE}/browse/{browse_id}"),
        Item::Artist { browse_id, .. } => format!("{BASE}/channel/{browse_id}"),
        // A podcast's page is its playlist with the MPSP prefix dropped.
        Item::Podcast { browse_id, .. } => match browse_id.strip_prefix("MPSP") {
            Some(playlist_id) => format!("{BASE}/playlist?list={playlist_id}"),
            None => format!("{BASE}/browse/{browse_id}"),
        },
        Item::Mood { .. } | Item::Shortcut { .. } => return None,
    })
}

fn artists_of(item: &Item) -> &[Link] {
    match item {
        Item::Track(track) => &track.artists,
        Item::Album { artists, .. } => artists,
        _ => &[],
    }
}

/// The context menu for `item`, built when it opens.
pub fn item_menu(menu: ContextMenu, item: &Item, context: &MenuContext, cx: &App) -> ContextMenu {
    let store = crate::store(cx);
    let state = store.state();
    let playlists: Vec<(String, String)> = state
        .library_playlists()
        .into_iter()
        .filter_map(|item| match item {
            Item::Playlist {
                playlist_id, title, ..
            } if playlist_id != "LM" => Some((playlist_id.clone(), title.clone())),
            _ => None,
        })
        .collect();
    let id = register(
        item,
        context,
        playlists.iter().map(|(id, _)| id.clone()).collect(),
    );
    let cmd = |op: Op| ItemCommand { target: id, op };
    let mut menu = menu;
    if let Some(index) = context.queue_index {
        menu = menu
            .item("Play", cmd(Op::Play))
            .item("Move up", cmd(Op::QueueMove(-1)))
            .item("Move down", cmd(Op::QueueMove(1)))
            .item_destructive("Remove from queue", cmd(Op::QueueRemove))
            .separator();
        let _ = index;
    }
    match item {
        Item::Track(track) => {
            let rating = state.rating(track);
            menu = menu
                .item("Start radio", cmd(Op::StartRadio))
                .item("Play next", cmd(Op::PlayNext))
                .item("Add to queue", cmd(Op::AddToQueue));
            let mut picker = Menu::new("Add to playlist");
            for (i, (_, title)) in playlists.iter().enumerate() {
                picker = picker.item(MenuItem::action(title.clone(), cmd(Op::AddToPlaylist(i))));
            }
            if playlists.is_empty() {
                picker =
                    picker.item(MenuItem::action("No playlists yet", cmd(Op::Open)).enabled(false));
            }
            menu = menu.submenu(picker);
            if let Some(saved) = state.in_library(track) {
                menu = menu.item(
                    if saved {
                        "Remove from library"
                    } else {
                        "Save to library"
                    },
                    cmd(Op::SaveToLibrary(!saved)),
                );
            }
            if let Some((_, row)) = &context.editable_playlist
                && track.set_video_id.is_some()
            {
                if *row > 0 {
                    menu = menu.item("Move up", cmd(Op::PlaylistMove(-1)));
                }
                menu = menu
                    .item("Move down", cmd(Op::PlaylistMove(1)))
                    .item_destructive("Remove from playlist", cmd(Op::RemoveFromPlaylist));
            }
            if track.feedback_token.is_some() {
                menu = menu.item_destructive("Remove from history", cmd(Op::RemoveFromHistory));
            }
            menu = menu
                .separator()
                .item(
                    if rating == Rating::Like {
                        "Remove from liked songs"
                    } else {
                        "Like"
                    },
                    cmd(Op::Rate(if rating == Rating::Like {
                        RatingOp::Indifferent
                    } else {
                        RatingOp::Like
                    })),
                )
                .item(
                    if rating == Rating::Dislike {
                        "Remove dislike"
                    } else {
                        "Dislike"
                    },
                    cmd(Op::Rate(if rating == Rating::Dislike {
                        RatingOp::Indifferent
                    } else {
                        RatingOp::Dislike
                    })),
                );
            if track.album.as_ref().is_some_and(|a| a.target.is_some()) || !track.artists.is_empty()
            {
                menu = menu.separator();
            }
            if track.album.as_ref().is_some_and(|a| a.target.is_some()) {
                menu = menu.item("Go to album", cmd(Op::GoToAlbum));
            }
        }
        Item::Album { playlist_id, .. } => {
            if playlist_id.is_some() {
                menu = menu
                    .item("Play", cmd(Op::Play))
                    .item("Save to library", cmd(Op::SaveToLibrary(true)));
            }
            menu = menu.item("Go to album", cmd(Op::Open));
        }
        Item::Playlist { .. } => {
            menu = menu
                .item("Play", cmd(Op::Play))
                .item("Shuffle", cmd(Op::Shuffle))
                .item("Save to library", cmd(Op::SaveToLibrary(true)));
        }
        Item::Artist { .. } => menu = menu.item("Go to artist", cmd(Op::Open)),
        Item::Podcast { .. } | Item::Mood { .. } | Item::Shortcut { .. } => {
            menu = menu.item("Open", cmd(Op::Open))
        }
    }
    let artists = artists_of(item);
    for (i, artist) in artists.iter().enumerate() {
        if artist.target.is_none() {
            continue;
        }
        let label = if artists.len() > 1 {
            format!("Go to {}", artist.text)
        } else {
            "Go to artist".to_owned()
        };
        menu = menu.item(label, cmd(Op::GoToArtist(i)));
    }
    if share_url(item).is_some() {
        menu = menu.separator().item("Share", cmd(Op::Share));
    }
    menu
}

/// Carries out a chosen row. Returns the page to open, when the row opens one.
pub fn run(command: &ItemCommand, cx: &mut App) -> Option<Route> {
    let (item, context, playlists) = OPEN.with(|open| {
        let open = open.borrow();
        let entry = open.1.iter().find(|e| e.id == command.target)?;
        Some((
            entry.item.clone(),
            entry.context.clone(),
            entry.playlists.clone(),
        ))
    })?;
    let store = crate::store(cx);
    let track = match &item {
        Item::Track(track) => Some(track.clone()),
        _ => None,
    };
    let enqueue = |track: &Option<Track>, position| {
        if let Some(track) = track {
            store.enqueue(vec![track.clone()], position);
        }
    };
    match command.op {
        Op::Open => return target_of(&item).map(Route::Browse),
        Op::Play => match context.queue_index {
            Some(index) => store.jump_to(index),
            None => play_item(&item, &store, false),
        },
        Op::Shuffle => play_item(&item, &store, true),
        Op::StartRadio => {
            if let Some(track) = &track {
                store.play(
                    PlaySource::Radio {
                        video_id: track.video_id.clone(),
                    },
                    0,
                    false,
                    true,
                );
            }
        }
        Op::PlayNext => enqueue(&track, EnqueuePosition::Next),
        Op::AddToQueue => enqueue(&track, EnqueuePosition::End),
        Op::AddToPlaylist(i) => {
            if let (Some(track), Some(playlist)) = (&track, playlists.get(i)) {
                store.add_to_playlist(playlist.clone(), vec![track.video_id.clone()]);
            }
        }
        Op::SaveToLibrary(saved) => match &item {
            Item::Track(track) => store.set_song_in_library(track, saved),
            Item::Album {
                playlist_id: Some(id),
                ..
            }
            | Item::Playlist {
                playlist_id: id, ..
            } => store.set_in_library(id.clone(), saved),
            _ => {}
        },
        Op::RemoveFromPlaylist => {
            if let (Some(track), Some((playlist, _))) = (&track, &context.editable_playlist) {
                store.remove_from_playlist(playlist.clone(), track);
            }
        }
        Op::RemoveFromHistory => {
            if let Some(track) = &track {
                store.remove_from_history(track);
            }
        }
        Op::Rate(rating) => {
            if let Some(track) = &track {
                store.rate(track, rating.into());
            }
        }
        Op::GoToAlbum => {
            return track
                .and_then(|t| t.album.and_then(|a| a.target))
                .map(Route::Browse);
        }
        Op::GoToArtist(i) => {
            return artists_of(&item)
                .get(i)
                .and_then(|a| a.target.clone())
                .map(Route::Browse);
        }
        Op::Share => {
            if let Some(url) = share_url(&item) {
                nd::clipboard::write_text(cx, url);
                store.confirm("Link copied".into());
            }
        }
        Op::QueueRemove => {
            if let Some(index) = context.queue_index {
                store.remove_from_queue(index);
            }
        }
        Op::QueueMove(delta) => {
            if let Some(index) = context.queue_index {
                let len = store.state().queue.tracks.len();
                let to = (index as i64 + i64::from(delta)).clamp(0, len.saturating_sub(1) as i64)
                    as usize;
                if to != index {
                    store.move_in_queue(index, to);
                }
            }
        }
        Op::PlaylistMove(delta) => {
            if let Some((playlist, row)) = &context.editable_playlist {
                let to = (*row as i64 + i64::from(delta)).max(0) as usize;
                if to != *row {
                    store.move_in_playlist(playlist.clone(), *row, to);
                }
            }
        }
    }
    None
}

/// Library tabs by the chip titles the page carries.
pub fn library_tab(title: &str) -> LibraryTab {
    match title {
        "Songs" => LibraryTab::Songs,
        "Albums" => LibraryTab::Albums,
        "Artists" => LibraryTab::Artists,
        "Subscriptions" => LibraryTab::Subscriptions,
        "Podcasts" => LibraryTab::Podcasts,
        "Uploads" => LibraryTab::Uploads,
        _ => LibraryTab::Playlists,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_copies_the_web_apps_links() {
        let album = |playlist_id: Option<&str>| Item::Album {
            browse_id: "MPREb_x".into(),
            playlist_id: playlist_id.map(Into::into),
            title: "A".into(),
            album_type: None,
            artists: Vec::new(),
            year: None,
            thumbnails: Vec::new(),
            explicit: false,
        };
        let urls: Vec<String> = [
            album(Some("OLAK5uy_x")),
            album(None),
            Item::Artist {
                browse_id: "UCx".into(),
                name: "B".into(),
                subtitle: None,
                thumbnails: Vec::new(),
            },
            Item::Podcast {
                browse_id: "MPSPPLx".into(),
                title: "C".into(),
                subtitle: None,
                thumbnails: Vec::new(),
            },
        ]
        .iter()
        .map(|item| share_url(item).unwrap())
        .collect();
        assert_eq!(
            urls,
            [
                "https://music.youtube.com/playlist?list=OLAK5uy_x",
                "https://music.youtube.com/browse/MPREb_x",
                "https://music.youtube.com/channel/UCx",
                "https://music.youtube.com/playlist?list=PLx",
            ]
        );
    }
}
