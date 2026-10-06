//! Single items: tracks, album and playlist cards, artist rows, mood tiles.

mod multi_row;
mod navigation_button;
mod responsive;
mod two_row;

use super::{browse_target, group_text, link, parse_clock, renderer, run_groups};
use formalmusic_api::{BrowseTarget, Item, Link, Rating, Track, TrackKind};
use serde_json::Value;

pub use multi_row::multi_row_item;
pub use navigation_button::navigation_button;
pub use responsive::responsive_list_item;
pub use two_row::two_row_item;

/// Any item renderer, or `None` (logged) for kinds the model has no use for.
pub fn item(v: &Value) -> Option<Item> {
    let (key, r) = renderer(v)?;
    let parsed = match key {
        "musicResponsiveListItemRenderer" => responsive_list_item(r),
        "musicTwoRowItemRenderer" => two_row_item(r),
        "musicMultiRowListItemRenderer" => multi_row_item(r),
        "musicNavigationButtonRenderer" => navigation_button(r),
        "playlistPanelVideoRenderer" | "playlistPanelVideoWrapperRenderer" => {
            crate::parse::next::panel_item(v).map(Item::Track)
        }
        _ => {
            tracing::debug!(renderer = key, "skipping unknown item renderer");
            return None;
        }
    };
    if parsed.is_none() {
        tracing::debug!(renderer = key, "item without the fields it needs");
    }
    parsed
}

pub fn items(list: &Value) -> Vec<Item> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(item)
        .collect()
}

const LABELS: [&str; 13] = [
    "Song",
    "Video",
    "Album",
    "Single",
    "EP",
    "Playlist",
    "Artist",
    "Podcast",
    "Episode",
    "Profile",
    "Station",
    "Audiobook",
    "Chart",
];

/// A subtitle or byline broken into what it names. The parts come in
/// different orders on different pages, so each `•` group is classified by
/// its links and its shape rather than by position.
#[derive(Debug, Default)]
pub(crate) struct Byline {
    pub label: Option<String>,
    pub artists: Vec<Link>,
    pub album: Option<Link>,
    pub duration_ms: Option<u64>,
    pub plays: Option<String>,
    pub year: Option<String>,
    /// Groups that matched nothing above, in order.
    pub rest: Vec<String>,
}

impl Byline {
    /// Each column's runs are classified on their own, since a column break
    /// separates parts the way " • " does.
    pub fn from_columns<'a>(columns: impl IntoIterator<Item = &'a [Value]>) -> Self {
        let mut byline = Self::default();
        for runs in columns {
            for group in run_groups(runs) {
                byline.add(&group);
            }
        }
        byline
    }

    fn add(&mut self, group: &[&Value]) {
        let text = group_text(group);
        if text.is_empty() {
            return;
        }
        let targets: Vec<_> = group
            .iter()
            .filter_map(|r| browse_target(&r["navigationEndpoint"]))
            .collect();
        if self.album.is_none() && targets.iter().any(|t| matches!(t, BrowseTarget::Album(_))) {
            self.album = group
                .iter()
                .find_map(|r| link(r).filter(|l| matches!(l.target, Some(BrowseTarget::Album(_)))));
        } else if self.artists.is_empty() && targets.iter().any(is_channel) {
            self.artists = group.iter().filter_map(|r| link(r)).collect();
        } else if let Some(ms) = parse_clock(&text).filter(|_| self.duration_ms.is_none()) {
            self.duration_ms = Some(ms);
        } else if self.plays.is_none() && is_count(&text) {
            self.plays = Some(text);
        } else if self.year.is_none() && text.len() == 4 && text.chars().all(|c| c.is_ascii_digit())
        {
            self.year = Some(text);
        } else if self.label.is_none() && self.is_first_group() && LABELS.contains(&text.as_str()) {
            self.label = Some(text);
        } else {
            self.rest.push(text);
        }
    }

    fn is_first_group(&self) -> bool {
        self.artists.is_empty()
            && self.album.is_none()
            && self.rest.is_empty()
            && self.plays.is_none()
    }

    pub fn album_type(&self) -> Option<String> {
        self.label
            .clone()
            .filter(|l| matches!(l.as_str(), "Album" | "Single" | "EP"))
    }

    /// Artists, or the first unlinked group when the page left names unlinked
    /// (radio queues, music videos).
    pub fn artists_or_unlinked(&self) -> Vec<Link> {
        if !self.artists.is_empty() {
            return self.artists.clone();
        }
        self.rest
            .first()
            .map(|name| {
                vec![Link {
                    text: name.clone(),
                    target: None,
                }]
            })
            .unwrap_or_default()
    }
}

fn is_channel(target: &BrowseTarget) -> bool {
    matches!(
        target,
        BrowseTarget::Artist(_) | BrowseTarget::ArtistShelf { .. }
    )
}

fn is_count(text: &str) -> bool {
    [
        "views",
        "plays",
        "view",
        "play",
        "listeners",
        "subscribers",
        "monthly audience",
    ]
    .iter()
    .any(|suffix| text.ends_with(suffix))
}

pub(crate) fn video_kind(watch_endpoint: &Value) -> Option<TrackKind> {
    let kind = watch_endpoint["watchEndpointMusicSupportedConfigs"]["watchEndpointMusicConfig"]["musicVideoType"]
        .as_str()?;
    Some(match kind {
        "MUSIC_VIDEO_TYPE_ATV" => TrackKind::Song,
        "MUSIC_VIDEO_TYPE_PODCAST_EPISODE" => TrackKind::Episode,
        "MUSIC_VIDEO_TYPE_PRIVATELY_OWNED_TRACK" => TrackKind::Upload,
        _ => TrackKind::Video,
    })
}

pub(crate) fn kind_from_label(label: Option<&str>) -> Option<TrackKind> {
    match label? {
        "Song" => Some(TrackKind::Song),
        "Video" => Some(TrackKind::Video),
        "Episode" => Some(TrackKind::Episode),
        _ => None,
    }
}

pub(crate) fn explicit(badges: &Value) -> bool {
    badges.as_array().into_iter().flatten().any(|b| {
        b["musicInlineBadgeRenderer"]["icon"]["iconType"].as_str() == Some("MUSIC_EXPLICIT_BADGE")
    })
}

/// The like state from a row's menu, as the signed-in user set it.
pub(crate) fn like_status(menu: &Value) -> Rating {
    let buttons = menu["menuRenderer"]["topLevelButtons"]
        .as_array()
        .into_iter()
        .flatten();
    for button in buttons {
        match button["likeButtonRenderer"]["likeStatus"].as_str() {
            Some("LIKE") => return Rating::Like,
            Some("DISLIKE") => return Rating::Dislike,
            _ => {}
        }
    }
    Rating::Indifferent
}

/// The "Remove from history" token in a History row's menu.
pub(crate) fn feedback_token(menu: &Value) -> Option<String> {
    menu["menuRenderer"]["items"]
        .as_array()?
        .iter()
        .find_map(|item| {
            item["menuServiceItemRenderer"]["serviceEndpoint"]["feedbackEndpoint"]["feedbackToken"]
                .as_str()
                .map(str::to_owned)
        })
}

/// The playlist a card's play button starts.
pub(crate) fn overlay_playlist_id(overlay: &Value) -> Option<String> {
    let endpoint = &overlay["musicItemThumbnailOverlayRenderer"]["content"]["musicPlayButtonRenderer"]
        ["playNavigationEndpoint"];
    endpoint["watchPlaylistEndpoint"]["playlistId"]
        .as_str()
        .or_else(|| endpoint["watchEndpoint"]["playlistId"].as_str())
        .map(str::to_owned)
}

pub(crate) fn overlay_watch_endpoint(overlay: &Value) -> &Value {
    &overlay["musicItemThumbnailOverlayRenderer"]["content"]["musicPlayButtonRenderer"]["playNavigationEndpoint"]
        ["watchEndpoint"]
}

pub(crate) fn blank_track(video_id: String, title: String) -> Track {
    Track {
        video_id,
        title,
        artists: Vec::new(),
        album: None,
        duration_ms: None,
        thumbnails: Vec::new(),
        explicit: false,
        kind: TrackKind::Song,
        like: Rating::Indifferent,
        set_video_id: None,
        plays: None,
        feedback_token: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn byline_classifies_groups() {
        let runs = json!([
            {"text": "Song"}, {"text": " • "},
            {"text": "Daft Punk", "navigationEndpoint": {"browseEndpoint": {"browseId": "UCabc"}}},
            {"text": " & "},
            {"text": "Julian Casablancas", "navigationEndpoint": {"browseEndpoint": {"browseId": "UCdef"}}},
            {"text": " • "},
            {"text": "Random Access Memories", "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREb_x"}}},
            {"text": " • "}, {"text": "5:38"}
        ]);
        let b = Byline::from_columns([runs.as_array().unwrap().as_slice()]);
        assert_eq!(b.label.as_deref(), Some("Song"));
        assert_eq!(b.artists.len(), 2);
        assert_eq!(b.album.unwrap().text, "Random Access Memories");
        assert_eq!(b.duration_ms, Some(338_000));
    }

    #[test]
    fn unlinked_artist_falls_back_to_first_group() {
        let runs = json!([{"text": "Daft Punk"}, {"text": " • "}, {"text": "900M views"}]);
        let b = Byline::from_columns([runs.as_array().unwrap().as_slice()]);
        assert_eq!(b.artists_or_unlinked()[0].text, "Daft Punk");
        assert_eq!(b.plays.as_deref(), Some("900M views"));
    }
}
