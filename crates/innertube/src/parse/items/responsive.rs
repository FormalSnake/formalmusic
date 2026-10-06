//! `musicResponsiveListItemRenderer`: every row-shaped item, from search
//! results and album tracks to library artists.

use super::{
    Byline, blank_track, explicit, feedback_token, kind_from_label, like_status,
    overlay_playlist_id, overlay_watch_endpoint, video_kind,
};
use crate::parse::{browse_target, runs, text, thumbnails};
use formalmusic_api::{BrowseTarget, Item, TrackKind};
use serde_json::Value;

pub fn responsive_list_item(r: &Value) -> Option<Item> {
    let columns: Vec<&Value> = r["flexColumns"]
        .as_array()?
        .iter()
        .map(|c| &c["musicResponsiveListItemFlexColumnRenderer"]["text"])
        .collect();
    let title_column = columns.first()?;
    let title = text(title_column)?;
    let title_endpoint = runs(title_column)
        .first()
        .map(|run| &run["navigationEndpoint"])
        .unwrap_or(&Value::Null);

    let fixed = r["fixedColumns"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| runs(&c["musicResponsiveListItemFixedColumnRenderer"]["text"]));
    let byline = Byline::from_columns(columns[1..].iter().map(|c| runs(c)).chain(fixed));
    let subtitle = subtitle(&columns[1..]);
    let thumbs = thumbnails(&r["thumbnail"]);

    let target = browse_target(&r["navigationEndpoint"]).or_else(|| browse_target(title_endpoint));
    let watch = if title_endpoint["watchEndpoint"].is_object() {
        &title_endpoint["watchEndpoint"]
    } else {
        overlay_watch_endpoint(&r["overlay"])
    };
    let video_id = r["playlistItemData"]["videoId"]
        .as_str()
        .or_else(|| watch["videoId"].as_str());

    match (target, video_id) {
        (Some(BrowseTarget::Album(browse_id)), _) => Some(Item::Album {
            browse_id,
            playlist_id: overlay_playlist_id(&r["overlay"]),
            title,
            album_type: byline.album_type(),
            artists: byline.artists_or_unlinked(),
            year: byline.year.clone(),
            thumbnails: thumbs,
            explicit: explicit(&r["badges"]),
        }),
        (Some(BrowseTarget::Artist(browse_id)), _) => Some(Item::Artist {
            browse_id,
            name: title,
            subtitle,
            thumbnails: thumbs,
        }),
        (Some(BrowseTarget::Playlist(playlist_id)), _) => Some(Item::Playlist {
            playlist_id,
            title,
            subtitle,
            thumbnails: thumbs,
        }),
        (Some(BrowseTarget::Podcast(browse_id)), _) => Some(Item::Podcast {
            browse_id,
            title,
            subtitle,
            thumbnails: thumbs,
        }),
        (Some(BrowseTarget::Episode(browse_id)), video_id) => {
            let video_id = video_id
                .map(str::to_owned)
                .or_else(|| browse_id.strip_prefix("MPED").map(str::to_owned))?;
            let mut track = blank_track(video_id, title);
            track.kind = TrackKind::Episode;
            track.thumbnails = thumbs;
            track.plays = byline.plays;
            track.duration_ms = byline.duration_ms;
            Some(Item::Track(track))
        }
        (_, Some(video_id)) => {
            let mut track = blank_track(video_id.to_owned(), title);
            track.kind = video_kind(watch)
                .or_else(|| kind_from_label(byline.label.as_deref()))
                .unwrap_or(TrackKind::Song);
            if track.kind != TrackKind::Episode {
                track.artists = byline.artists_or_unlinked();
            }
            track.album = byline.album;
            track.duration_ms = byline.duration_ms;
            track.plays = byline.plays;
            track.thumbnails = thumbs;
            track.explicit = explicit(&r["badges"]);
            track.like = like_status(&r["menu"]);
            track.set_video_id = r["playlistItemData"]["playlistSetVideoId"]
                .as_str()
                .map(str::to_owned);
            track.feedback_token = feedback_token(&r["menu"]);
            Some(Item::Track(track))
        }
        (other, None) => {
            tracing::debug!(?other, "list item that is neither a track nor a known page");
            None
        }
    }
}

fn subtitle(columns: &[&Value]) -> Option<String> {
    let parts: Vec<String> = columns.iter().filter_map(|c| text(c)).collect();
    (!parts.is_empty()).then(|| parts.join(" • "))
}
