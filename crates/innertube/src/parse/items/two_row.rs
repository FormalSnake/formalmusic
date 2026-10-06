//! `musicTwoRowItemRenderer`: the square cards in carousels and grids.

use super::{Byline, blank_track, explicit, overlay_playlist_id, video_kind};
use crate::parse::{browse_target, runs, text, thumbnails};
use formalmusic_api::{BrowseTarget, Item, TrackKind};
use serde_json::Value;

pub fn two_row_item(r: &Value) -> Option<Item> {
    let title = text(&r["title"])?;
    let endpoint = &r["navigationEndpoint"];
    let byline = Byline::from_columns([runs(&r["subtitle"])]);
    let subtitle = text(&r["subtitle"]);
    let thumbs = thumbnails(&r["thumbnailRenderer"]);

    if let Some(video_id) = endpoint["watchEndpoint"]["videoId"].as_str() {
        let mut track = blank_track(video_id.to_owned(), title);
        track.kind = video_kind(&endpoint["watchEndpoint"]).unwrap_or(TrackKind::Video);
        track.artists = byline.artists_or_unlinked();
        track.album = byline.album;
        track.plays = byline.plays;
        track.thumbnails = thumbs;
        track.explicit = explicit(&r["subtitleBadges"]);
        return Some(Item::Track(track));
    }
    if let Some(playlist_id) = endpoint["watchPlaylistEndpoint"]["playlistId"].as_str() {
        return Some(Item::Playlist {
            playlist_id: playlist_id.to_owned(),
            title,
            subtitle,
            thumbnails: thumbs,
        });
    }

    match browse_target(endpoint)? {
        BrowseTarget::Album(browse_id) => Some(Item::Album {
            browse_id,
            playlist_id: overlay_playlist_id(&r["thumbnailOverlay"]),
            title,
            album_type: byline.album_type(),
            artists: byline.artists.clone(),
            year: byline.year.clone(),
            thumbnails: thumbs,
            explicit: explicit(&r["subtitleBadges"]),
        }),
        BrowseTarget::Artist(browse_id) => Some(Item::Artist {
            browse_id,
            name: title,
            subtitle,
            thumbnails: thumbs,
        }),
        BrowseTarget::Playlist(playlist_id) => Some(Item::Playlist {
            playlist_id,
            title,
            subtitle,
            thumbnails: thumbs,
        }),
        BrowseTarget::Podcast(browse_id) => Some(Item::Podcast {
            browse_id,
            title,
            subtitle,
            thumbnails: thumbs,
        }),
        BrowseTarget::Episode(browse_id) => {
            let mut track = blank_track(browse_id.strip_prefix("MPED")?.to_owned(), title);
            track.kind = TrackKind::Episode;
            track.thumbnails = thumbs;
            Some(Item::Track(track))
        }
        other => {
            tracing::debug!(?other, "card linking to a page that is not an item");
            None
        }
    }
}
