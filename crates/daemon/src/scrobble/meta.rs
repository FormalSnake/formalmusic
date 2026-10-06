//! What a play is reported as. Album tracks keep YouTube Music's metadata,
//! with the album artist and track number from the album page. Videos get
//! cleaned titles and borrow the album of the matching album track, or the
//! album Last.fm files them under. A MusicBrainz match adds ids and the ISRC
//! when its names agree with ours.

use super::clean::{clean_artist, clean_title, norm, split_upload};
use super::lastfm::LastFm;
use super::listenbrainz::ListenBrainz;
use formalmusic_api::{BrowseTarget, Header, Item, SearchFilter, Track, TrackKind};
use formalmusic_innertube::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Song {
    pub video_id: String,
    pub title: String,
    /// Main artist first.
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub album_artists: Vec<String>,
    pub duration_ms: Option<u64>,
    pub track_number: Option<u32>,
    pub isrc: Option<String>,
    pub recording_mbid: Option<String>,
    pub release_mbid: Option<String>,
}

impl Song {
    pub fn artist(&self) -> &str {
        self.artists.first().map(String::as_str).unwrap_or_default()
    }

    /// YouTube Music's own fields, cleaned when the track is a video.
    pub fn from_track(track: &Track, duration_ms: Option<u64>) -> Self {
        let byline: Vec<String> = track.artists.iter().map(|a| a.text.clone()).collect();
        let mut song = Song {
            video_id: track.video_id.clone(),
            title: track.title.clone(),
            artists: byline.clone(),
            album: track.album.as_ref().map(|a| a.text.clone()),
            duration_ms: track.duration_ms.or(duration_ms),
            ..Song::default()
        };
        if track.kind != TrackKind::Song {
            let title = clean_title(&track.title);
            match split_upload(&title, byline.first().map(String::as_str)) {
                Some((artists, title)) => {
                    song.artists = artists;
                    song.title = title;
                }
                None => {
                    song.title = title;
                    song.artists = byline.iter().map(|a| clean_artist(a)).collect();
                }
            }
        }
        song.artists.retain(|a| !a.is_empty());
        song
    }

    pub fn is_reportable(&self) -> bool {
        !self.title.trim().is_empty() && !self.artist().trim().is_empty()
    }
}

/// The album page's artists and this track's place on it.
pub async fn album_details(client: &Client, browse_id: &str, song: &mut Song) {
    let Ok(page) = client
        .browse(BrowseTarget::Album(browse_id.to_owned()))
        .await
    else {
        return;
    };
    if let Some(Header::Detail {
        title, subtitle, ..
    }) = &page.header
    {
        let artists: Vec<String> = subtitle
            .iter()
            .filter(|l| matches!(l.target, Some(BrowseTarget::Artist(_))))
            .map(|l| l.text.clone())
            .collect();
        if !artists.is_empty() {
            song.album_artists = artists;
        }
        if song.album.is_none() {
            song.album = Some(title.clone());
        }
    }
    let tracks: Vec<&Track> = page
        .sections
        .iter()
        .flat_map(|s| &s.items)
        .filter_map(|i| match i {
            Item::Track(t) => Some(t),
            _ => None,
        })
        .collect();
    let position = tracks
        .iter()
        .position(|t| t.video_id == song.video_id)
        .or_else(|| {
            let title = norm(&song.title);
            tracks.iter().position(|t| norm(&t.title) == title)
        });
    song.track_number = position.map(|i| i as u32 + 1);
}

/// The album track a video stands for, found through YouTube Music's song
/// search: same title, same main artist, about the same length.
pub async fn album_track(client: &Client, song: &Song) -> Option<Track> {
    let query = format!("{} {}", song.artist(), song.title);
    let results = client
        .search(&query, Some(SearchFilter::Songs))
        .await
        .ok()?;
    let (title, artist) = (norm(&song.title), norm(song.artist()));
    results
        .sections
        .iter()
        .flat_map(|s| &s.items)
        .filter_map(|i| match i {
            Item::Track(t) => Some(t),
            _ => None,
        })
        .take(5)
        .find(|t| {
            t.album.is_some()
                && norm(&t.title) == title
                && t.artists.first().is_some_and(|a| norm(&a.text) == artist)
                && match (t.duration_ms, song.duration_ms) {
                    (Some(a), Some(b)) => a.abs_diff(b) <= 15_000,
                    _ => true,
                }
        })
        .cloned()
}

/// Fills in what the YouTube Music pages could not: the album from Last.fm
/// for videos without an album track, then MusicBrainz ids and the ISRC
/// when the match names the same recording.
pub async fn enrich(
    song: &mut Song,
    lastfm: Option<&LastFm>,
    listenbrainz: &ListenBrainz,
    http: &reqwest::Client,
) {
    if song.album.is_none()
        && let Some(lastfm) = lastfm
        && let Some((album, album_artist)) = lastfm.album(song.artist(), &song.title).await
    {
        song.album = Some(album);
        song.album_artists = album_artist.into_iter().collect();
    }
    let Some(mapping) = listenbrainz
        .lookup(song.artist(), &song.title, song.album.as_deref())
        .await
    else {
        return;
    };
    if !confident(song, &mapping) {
        tracing::debug!(
            title = song.title,
            matched = mapping.recording_name,
            "musicbrainz match not confident"
        );
        return;
    }
    song.recording_mbid = mapping.recording_mbid;
    let same_release = match (&song.album, &mapping.release_name) {
        (Some(ours), Some(theirs)) => norm(ours) == norm(theirs),
        (None, Some(_)) => true,
        _ => false,
    };
    if same_release {
        song.release_mbid = mapping.release_mbid;
        if song.album.is_none() {
            song.album = mapping.release_name;
        }
    }
    if song.isrc.is_none()
        && let Some(mbid) = &song.recording_mbid
    {
        song.isrc = isrc(http, mbid).await;
    }
}

/// Same title and the main artist named in the credit.
pub fn confident(song: &Song, mapping: &super::listenbrainz::Mapping) -> bool {
    norm(&mapping.recording_name) == norm(&song.title)
        && norm(&mapping.artist_credit_name).contains(&norm(song.artist()))
}

/// The recording's ISRC when MusicBrainz lists exactly one.
async fn isrc(http: &reqwest::Client, recording_mbid: &str) -> Option<String> {
    let json: Value = http
        .get(format!(
            "https://musicbrainz.org/ws/2/recording/{recording_mbid}"
        ))
        .query(&[("inc", "isrcs"), ("fmt", "json")])
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    match json["isrcs"].as_array()?.as_slice() {
        [one] => one.as_str().map(str::to_owned),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::listenbrainz::Mapping;
    use super::*;
    use formalmusic_api::{Link, Rating};

    fn track(title: &str, artist: &str, kind: TrackKind, album: Option<&str>) -> Track {
        Track {
            video_id: "v".into(),
            title: title.into(),
            artists: vec![Link {
                text: artist.into(),
                target: None,
            }],
            album: album.map(|a| Link {
                text: a.into(),
                target: None,
            }),
            duration_ms: Some(213_000),
            thumbnails: vec![],
            explicit: false,
            kind,
            like: Rating::Indifferent,
            set_video_id: None,
            plays: None,
            feedback_token: None,
        }
    }

    #[test]
    fn album_tracks_keep_youtube_music_metadata() {
        let t = track(
            "Never Gonna Give You Up (2022 Remaster)",
            "Rick Astley",
            TrackKind::Song,
            Some("Whenever You Need Somebody"),
        );
        let song = Song::from_track(&t, None);
        assert_eq!(song.title, "Never Gonna Give You Up (2022 Remaster)");
        assert_eq!(song.artists, ["Rick Astley"]);
        assert_eq!(song.album.as_deref(), Some("Whenever You Need Somebody"));
        assert_eq!(song.duration_ms, Some(213_000));
    }

    #[test]
    fn videos_get_clean_titles_and_artists() {
        let t = track(
            "The Weeknd - Blinding Lights (Official Video)",
            "TheWeekndVEVO",
            TrackKind::Upload,
            None,
        );
        let song = Song::from_track(&t, None);
        assert_eq!(song.title, "Blinding Lights");
        assert_eq!(song.artists, ["The Weeknd"]);
        let t = track("bad guy", "Billie Eilish - Topic", TrackKind::Video, None);
        let song = Song::from_track(&t, None);
        assert_eq!(
            (song.title.as_str(), song.artist()),
            ("bad guy", "Billie Eilish")
        );
    }

    #[test]
    fn musicbrainz_matches_need_the_same_names() {
        let song = Song {
            title: "Blinding Lights".into(),
            artists: vec!["The Weeknd".into()],
            ..Song::default()
        };
        let good = Mapping {
            recording_name: "Blinding Lights".into(),
            artist_credit_name: "The Weeknd".into(),
            ..Mapping::default()
        };
        assert!(confident(&song, &good));
        let other = Mapping {
            recording_name: "Blinding Lights (Chromatics remix)".into(),
            ..good
        };
        assert!(!confident(&song, &other));
    }
}
