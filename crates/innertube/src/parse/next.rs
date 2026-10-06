//! The `next` endpoint: the player page's up-next queue (`playlistPanelVideoRenderer`
//! rows) and the browse ids of its Lyrics and Related tabs.

use super::items::{
    Byline, blank_track, explicit, like_in_buttons, like_status, rating, video_kind,
};
use super::{continuation, missing, runs, text, thumbnails};
use crate::Result;
use formalmusic_api::{Continuation, Counterpart, Rating, SharedSegment, Track, TrackKind};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct NextResult {
    pub tracks: Vec<Track>,
    /// The queue's playlist, such as the `RDAMVM...` radio id.
    pub playlist_id: Option<String>,
    pub lyrics_browse_id: Option<String>,
    pub related_browse_id: Option<String>,
    /// More queue for radio and long playlists, for [`crate::Client::next_continuation`].
    pub continuation: Option<Continuation>,
    /// The signed-in user's rating of the requested track, when the response
    /// says. Queue rows carry none of their own.
    pub like: Option<Rating>,
}

pub fn parse_next(json: &Value) -> Result<NextResult> {
    let tabs = json["contents"]["singleColumnMusicWatchNextResultsRenderer"]["tabbedRenderer"]
        ["watchNextTabbedResultsRenderer"]["tabs"]
        .as_array()
        .ok_or_else(|| missing("contents.singleColumnMusicWatchNextResultsRenderer...watchNextTabbedResultsRenderer.tabs"))?;

    let mut result = NextResult {
        like: player_like(&json["playerOverlays"]["playerOverlayRenderer"]),
        ..NextResult::default()
    };
    for tab in tabs {
        let tab = &tab["tabRenderer"];
        let browse_id = tab["endpoint"]["browseEndpoint"]["browseId"]
            .as_str()
            .map(str::to_owned);
        match &browse_id {
            Some(id) if id.starts_with("MPLY") => result.lyrics_browse_id = browse_id,
            Some(id) if id.starts_with("MPTR") => result.related_browse_id = browse_id,
            _ => {}
        }
        let panel = &tab["content"]["musicQueueRenderer"]["content"]["playlistPanelRenderer"];
        if panel.is_object() {
            result.tracks = panel_tracks(panel);
            result.playlist_id = panel["playlistId"].as_str().map(str::to_owned);
            result.continuation = continuation(panel);
        }
    }
    Ok(result)
}

pub fn parse_next_continuation(json: &Value) -> Result<NextResult> {
    let panel = &json["continuationContents"]["playlistPanelContinuation"];
    if !panel.is_object() {
        return Err(missing("continuationContents.playlistPanelContinuation"));
    }
    Ok(NextResult {
        tracks: panel_tracks(panel),
        playlist_id: panel["playlistId"].as_str().map(str::to_owned),
        continuation: continuation(panel),
        ..NextResult::default()
    })
}

/// The like button over the player: `actions` in older responses, a view
/// model in the action bar since.
fn player_like(overlay: &Value) -> Option<Rating> {
    like_in_buttons(&overlay["actions"]).or_else(|| {
        overlay["videoActionBar"]["videoActionBarViewModel"]["buttons"]
            .as_array()?
            .iter()
            .find_map(|button| {
                let status = &button["buttonViewModel"]["segmentedLikeDislikeButtonViewModel"]
                    ["likeButtonViewModel"]["likeButtonViewModel"]["likeStatusEntity"]
                    ["likeStatus"];
                rating(status.as_str()?)
            })
    })
}

fn panel_tracks(panel: &Value) -> Vec<Track> {
    panel["contents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(panel_item)
        .collect()
}

/// One queue row. Wrapper rows, which only signed-in sessions get, carry
/// the album version and the music video version of a song: the primary one
/// is the id that was asked for, the other becomes [`Track::counterpart`].
pub fn panel_item(v: &Value) -> Option<Track> {
    let wrapper = &v["playlistPanelVideoWrapperRenderer"];
    let r = if wrapper.is_object() {
        &wrapper["primaryRenderer"]["playlistPanelVideoRenderer"]
    } else {
        &v["playlistPanelVideoRenderer"]
    };
    let video_id = r["videoId"].as_str()?;
    let mut track = blank_track(video_id.to_owned(), text(&r["title"])?);
    let byline = Byline::from_columns([runs(&r["longBylineText"])]);
    let watch = &r["navigationEndpoint"]["watchEndpoint"];
    track.kind = video_kind(watch).unwrap_or(TrackKind::Song);
    track.artists = byline.artists_or_unlinked();
    track.album = byline.album.clone();
    track.duration_ms = text(&r["lengthText"])
        .as_deref()
        .and_then(super::parse_clock);
    track.plays = byline.plays.clone();
    track.thumbnails = thumbnails(&r["thumbnail"]);
    track.explicit = explicit(&r["badges"]);
    track.like = like_status(&r["menu"]);
    track.set_video_id = r["playlistSetVideoId"].as_str().map(str::to_owned);
    track.counterpart = wrapper["counterpart"]
        .as_array()
        .and_then(|list| list.first())
        .and_then(counterpart)
        .map(Box::new);
    Some(track)
}

fn counterpart(v: &Value) -> Option<Counterpart> {
    let r = &v["counterpartRenderer"]["playlistPanelVideoRenderer"];
    let ms = |s: &Value, key: &str| s[key].as_str().and_then(|n| n.parse().ok());
    let segments = v["segmentMap"]["segment"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            Some(SharedSegment {
                start_ms: ms(s, "primaryVideoStartTimeMilliseconds")?,
                counterpart_start_ms: ms(s, "counterpartVideoStartTimeMilliseconds")?,
                duration_ms: ms(s, "durationMilliseconds")?,
            })
        })
        .collect();
    Some(Counterpart {
        video_id: r["videoId"].as_str()?.to_owned(),
        kind: video_kind(&r["navigationEndpoint"]["watchEndpoint"]).unwrap_or(TrackKind::Video),
        thumbnails: thumbnails(&r["thumbnail"]),
        duration_ms: text(&r["lengthText"])
            .as_deref()
            .and_then(super::parse_clock),
        segments,
    })
}
