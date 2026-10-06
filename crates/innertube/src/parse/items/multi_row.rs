//! `musicMultiRowListItemRenderer`: podcast episodes, with a description
//! under the title.

use super::{Byline, blank_track};
use crate::parse::{parse_spoken_duration, runs, text, thumbnails};
use formalmusic_api::{Item, TrackKind};
use serde_json::Value;

pub fn multi_row_item(r: &Value) -> Option<Item> {
    let video_id = r["onTap"]["watchEndpoint"]["videoId"].as_str()?;
    let mut track = blank_track(video_id.to_owned(), text(&r["title"])?);
    let byline = Byline::from_columns([runs(&r["subtitle"])]);
    let progress = &r["playbackProgress"]["musicPlaybackProgressRenderer"];
    track.kind = TrackKind::Episode;
    track.duration_ms = text(&progress["durationText"])
        .as_deref()
        .and_then(parse_spoken_duration);
    track.plays = byline.plays;
    track.thumbnails = thumbnails(&r["thumbnail"]);
    Some(Item::Track(track))
}
