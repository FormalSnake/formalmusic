//! Kopuz's wire rows into the app's model. The model keeps YouTube Music's page shapes, and
//! Kopuz's catalog carries the same ones (shelves, chips, header variants), so this is mostly
//! renaming. Ids the daemon hands out go into the model's id fields unchanged and come back
//! the same way: a track key is the video id, a page id is `browseId` or `browseId?params`.

use std::collections::HashSet;

use formalmusic_api::{
    BrowseTarget, Chip, Counterpart, Header, Item, LibraryTab, LibraryToggle, Link, LyricLine,
    LyricWord, Lyrics, Page, PlaybackMode, PlayerState, Privacy, QueueState, Rating, Repeat,
    SearchFilter, Section, SectionLayout, Status, Thumbnail, Thumbnails, Track, TrackKind,
};
use kopuz_api as k;

/// Where the transport keeps what a later command needs and the model has no field for.
pub struct Ctx<'a> {
    /// Keys of the account's liked songs.
    pub liked: &'a HashSet<String>,
    /// Every row's actions, by the id the model carries for it.
    pub actions: &'a mut dyn FnMut(&str, &k::CatalogActions),
}

pub const HOME: &str = "FEmusic_home";
const EXPLORE: &str = "FEmusic_explore";
const NEW_RELEASES: &str = "FEmusic_new_releases";
const CHARTS: &str = "FEmusic_charts";
const MOODS: &str = "FEmusic_moods_and_genres";
const HISTORY: &str = "FEmusic_history";
const LIKED_MUSIC: &str = "LM";

/// The catalog request that opens `target`.
pub fn request(target: &BrowseTarget) -> k::CatalogDetailRequest {
    use k::CatalogItemKind as K;
    let page = |id: &str| k::CatalogDetailRequest::new(K::Page, id);
    match target {
        BrowseTarget::Home => page(HOME),
        BrowseTarget::HomeChip { params } | BrowseTarget::MoodCategory { params } => page(params),
        BrowseTarget::Explore => page(EXPLORE),
        BrowseTarget::NewReleases => page(NEW_RELEASES),
        BrowseTarget::Charts => page(CHARTS),
        BrowseTarget::MoodsAndGenres => page(MOODS),
        BrowseTarget::History => page(HISTORY),
        BrowseTarget::Library(tab) => match tab {
            LibraryTab::LikedSongs => k::CatalogDetailRequest::new(K::Playlist, LIKED_MUSIC),
            LibraryTab::Playlists => page("FEmusic_liked_playlists"),
            LibraryTab::Songs => page("FEmusic_liked_videos"),
            LibraryTab::Albums => page("FEmusic_liked_albums"),
            LibraryTab::Artists => page("FEmusic_library_corpus_track_artists"),
            LibraryTab::Subscriptions => page("FEmusic_library_corpus_artists"),
            LibraryTab::Podcasts => page("FEmusic_library_non_music_audio_list"),
            LibraryTab::Uploads => page("FEmusic_library_privately_owned_tracks"),
        },
        BrowseTarget::Album(id) => k::CatalogDetailRequest::new(K::Album, id),
        BrowseTarget::Artist(id) => k::CatalogDetailRequest::new(K::Artist, id),
        BrowseTarget::Playlist(id) => k::CatalogDetailRequest::new(K::Playlist, id),
        BrowseTarget::Podcast(id) => k::CatalogDetailRequest::new(K::Podcast, id),
        BrowseTarget::Episode(id) => k::CatalogDetailRequest::new(K::Episode, id),
        BrowseTarget::ArtistShelf { browse_id, params } => page(&format!("{browse_id}?{params}")),
        BrowseTarget::Raw { browse_id, params } => match params {
            Some(params) => page(&format!("{browse_id}?{params}")),
            None => page(browse_id),
        },
    }
}

/// The target a link of `kind` to `id` opens.
fn target(kind: k::CatalogItemKind, id: &str) -> BrowseTarget {
    use k::CatalogItemKind as K;
    match kind {
        K::Album => BrowseTarget::Album(id.into()),
        K::Artist => BrowseTarget::Artist(id.into()),
        K::Playlist => BrowseTarget::Playlist(id.into()),
        K::Podcast => BrowseTarget::Podcast(id.into()),
        K::Episode => BrowseTarget::Episode(id.into()),
        K::Mood => BrowseTarget::MoodCategory { params: id.into() },
        _ => BrowseTarget::Raw {
            browse_id: id.into(),
            params: None,
        },
    }
}

/// Artwork as two thumbnails, the grid size and the full one. The URL is the [`ArtCache`]
/// key, `kopuz:<kind>/<id>@<version>`, with `#hq` on the large one.
///
/// [`ArtCache`]: crate::art::ArtCache
pub fn thumbnails(artwork: Option<&k::ArtworkRef>) -> Thumbnails {
    let Some(art) = artwork else {
        return Vec::new();
    };
    let url = format!(
        "{}{}/{}@{}",
        crate::art::KOPUZ_SCHEME,
        art.target.kind(),
        art.target.id(),
        art.version
    );
    vec![
        Thumbnail {
            url: url.clone(),
            width: 226,
            height: 226,
        },
        Thumbnail {
            url: format!("{url}#hq"),
            width: 1200,
            height: 1200,
        },
    ]
}

/// What a [`thumbnails`] URL asks the daemon for.
pub fn artwork_request(url: &str) -> Option<k::ArtworkRequest> {
    let rest = url.strip_prefix(crate::art::KOPUZ_SCHEME)?;
    let (rest, hq) = match rest.strip_suffix("#hq") {
        Some(rest) => (rest, true),
        None => (rest, false),
    };
    let (kind, rest) = rest.split_once('/')?;
    let (id, _version) = rest.rsplit_once('@')?;
    let id = id.to_owned();
    let target = match kind {
        "track" => k::ArtworkTarget::Track(id),
        "album" => k::ArtworkTarget::Album(id),
        "artist" => k::ArtworkTarget::Artist(id),
        "playlist" => k::ArtworkTarget::Playlist(id),
        "catalog" => k::ArtworkTarget::Catalog(id),
        "station" => k::ArtworkTarget::Station(id),
        "account" => k::ArtworkTarget::Account(id),
        _ => return None,
    };
    Some(k::ArtworkRequest { target, hq })
}

pub fn track(row: &k::TrackInfo, liked: &HashSet<String>) -> Track {
    let mut artists: Vec<Link> = row
        .credits
        .iter()
        .map(|credit| Link {
            text: credit.name.clone(),
            target: credit.key.as_ref().map(|key| BrowseTarget::Artist(key.clone())),
        })
        .collect();
    if artists.is_empty() && !row.artist.is_empty() {
        artists.push(Link {
            text: row.artist.clone(),
            target: None,
        });
    }
    let album = (!row.album.is_empty()).then(|| Link {
        text: row.album.clone(),
        target: (!row.album_id.is_empty()).then(|| BrowseTarget::Album(row.album_id.clone())),
    });
    let kind = match row.version() {
        Some(k::TrackVersion::Video) => TrackKind::Video,
        _ => TrackKind::Song,
    };
    let thumbnails = thumbnails(row.artwork.as_ref());
    let counterpart = row.counterpart.as_ref().map(|other| {
        Box::new(Counterpart {
            video_id: other.key.clone(),
            kind: match other.version {
                k::TrackVersion::Song => TrackKind::Song,
                k::TrackVersion::Video => TrackKind::Video,
            },
            thumbnails: thumbnails.clone(),
            duration_ms: other.duration_ms,
            segments: Vec::new(),
        })
    });
    Track {
        video_id: row.key.clone(),
        title: row.title.clone(),
        artists,
        album,
        duration_ms: row.duration_ms,
        thumbnails,
        explicit: row.explicit,
        kind,
        like: liked.contains(&row.key).then_some(Rating::Like),
        set_video_id: None,
        plays: row.plays.clone(),
        feedback_token: None,
        library: None,
        counterpart,
    }
}

/// A [`LibraryToggle`] token names its direction, since Kopuz's save takes one ref for both.
pub fn library_token(save_ref: &str, saved: bool) -> String {
    format!("{}{save_ref}", if saved { '1' } else { '0' })
}

pub fn read_library_token(token: &str) -> Option<(&str, bool)> {
    match token.split_at_checked(1)? {
        ("1", save_ref) => Some((save_ref, true)),
        ("0", save_ref) => Some((save_ref, false)),
        _ => None,
    }
}

fn item(tile: &k::CatalogItem, ctx: &mut Ctx) -> Option<Item> {
    use k::CatalogItemKind as K;
    (ctx.actions)(&tile.id, &tile.actions);
    let thumbnails = thumbnails(tile.artwork.as_ref());
    Some(match tile.kind {
        K::Track | K::Video | K::Episode => {
            let row = tile.track.as_ref()?;
            (ctx.actions)(&row.key, &tile.actions);
            let mut track = track(row, ctx.liked);
            track.kind = match tile.kind {
                K::Video => TrackKind::Video,
                K::Episode => TrackKind::Episode,
                _ => track.kind,
            };
            if let Some(rating) = tile.actions.rating {
                track.like = Some(match rating {
                    k::Rating::Like => Rating::Like,
                    k::Rating::Dislike => Rating::Dislike,
                    k::Rating::None => Rating::Indifferent,
                });
            }
            if let (Some(save_ref), Some(saved)) = (&tile.actions.save_ref, tile.actions.saved) {
                track.library = Some(LibraryToggle {
                    saved,
                    add_token: library_token(save_ref, true),
                    remove_token: library_token(save_ref, false),
                });
            }
            track.feedback_token = tile.actions.history_token.clone();
            if track.thumbnails.is_empty() {
                track.thumbnails = thumbnails;
            }
            Item::Track(track)
        }
        K::Album => {
            let (album_type, artists, year) = album_byline(tile.subtitle.as_deref());
            Item::Album {
                browse_id: tile.id.clone(),
                playlist_id: None,
                title: tile.title.clone(),
                album_type,
                artists,
                year,
                thumbnails,
                explicit: tile.explicit,
            }
        }
        K::Artist => Item::Artist {
            browse_id: tile.id.clone(),
            name: tile.title.clone(),
            subtitle: tile.subtitle.clone(),
            thumbnails,
        },
        K::Playlist => Item::Playlist {
            playlist_id: tile.id.clone(),
            title: tile.title.clone(),
            subtitle: tile.subtitle.clone(),
            thumbnails,
        },
        K::Podcast => Item::Podcast {
            browse_id: tile.id.clone(),
            title: tile.title.clone(),
            subtitle: tile.subtitle.clone(),
            thumbnails,
        },
        K::Mood => Item::Mood {
            title: tile.title.clone(),
            params: tile.id.clone(),
            color: tile.accent.as_deref().and_then(argb),
        },
        K::Page => Item::Shortcut {
            title: tile.title.clone(),
            target: target(K::Page, &tile.id),
            icon: None,
        },
        K::Unknown => return None,
    })
}

/// "Album • Artist • 2023" split into its kind, its artists and its year.
fn album_byline(subtitle: Option<&str>) -> (Option<String>, Vec<Link>, Option<String>) {
    let Some(subtitle) = subtitle else {
        return (None, Vec::new(), None);
    };
    let mut parts: Vec<&str> = subtitle.split(" • ").map(str::trim).collect();
    let album_type = match parts.first() {
        Some(&kind) if matches!(kind, "Album" | "Single" | "EP") => {
            parts.remove(0);
            Some(kind.to_owned())
        }
        _ => None,
    };
    let year = match parts.last() {
        Some(year) if year.len() == 4 && year.bytes().all(|b| b.is_ascii_digit()) => {
            let year = year.to_string();
            parts.pop();
            Some(year)
        }
        _ => None,
    };
    let artists = parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .map(|part| Link {
            text: part.to_owned(),
            target: None,
        })
        .collect();
    (album_type, artists, year)
}

/// `#rrggbb` as `0xAARRGGBB`, opaque.
fn argb(hex: &str) -> Option<u32> {
    let rgb = u32::from_str_radix(hex.strip_prefix('#')?, 16).ok()?;
    Some(0xff00_0000 | rgb)
}

fn layout(layout: k::ShelfLayout) -> SectionLayout {
    match layout {
        k::ShelfLayout::Carousel => SectionLayout::Carousel,
        k::ShelfLayout::Grid => SectionLayout::Grid,
        k::ShelfLayout::List => SectionLayout::List,
        k::ShelfLayout::TrackGrid => SectionLayout::TrackGrid,
        k::ShelfLayout::Hero => SectionLayout::Hero,
    }
}

pub fn search_filter(id: &str) -> Option<SearchFilter> {
    serde_json::from_value(serde_json::Value::String(id.to_owned())).ok()
}

pub fn search_filter_id(filter: SearchFilter) -> String {
    match serde_json::to_value(filter) {
        Ok(serde_json::Value::String(id)) => id,
        _ => unreachable!("search filters serialise as strings"),
    }
}

/// A shelf, with `more` encoded for [`super::Cursor`] by the caller.
pub fn section(
    shelf: &k::CatalogShelf,
    continuation: Option<formalmusic_api::Continuation>,
    ctx: &mut Ctx,
) -> Section {
    Section {
        title: (!shelf.title.is_empty()).then(|| shelf.title.clone()),
        strapline: shelf.strapline.clone(),
        layout: layout(shelf.layout),
        items: shelf.items.iter().filter_map(|tile| item(tile, ctx)).collect(),
        more: shelf
            .more_ref
            .as_deref()
            .map(|id| target(shelf.more_kind, id)),
        continuation,
        filter: shelf.search_filter.as_deref().and_then(search_filter),
        shuffle_playlist_id: None,
        radio_playlist_id: None,
    }
}

pub fn items(shelves: &[k::CatalogShelf], ctx: &mut Ctx) -> Vec<Item> {
    shelves
        .iter()
        .flat_map(|shelf| shelf.items.iter())
        .filter_map(|tile| item(tile, ctx))
        .collect()
}

fn privacy(privacy: k::PlaylistPrivacy) -> Privacy {
    match privacy {
        k::PlaylistPrivacy::Private => Privacy::Private,
        k::PlaylistPrivacy::Unlisted => Privacy::Unlisted,
        k::PlaylistPrivacy::Public => Privacy::Public,
    }
}

pub fn wire_privacy(privacy: Privacy) -> k::PlaylistPrivacy {
    match privacy {
        Privacy::Private => k::PlaylistPrivacy::Private,
        Privacy::Unlisted => k::PlaylistPrivacy::Unlisted,
        Privacy::Public => k::PlaylistPrivacy::Public,
    }
}

/// The track rows of a detail page. In a playlist the account owns, `set_video_id` is the
/// row's position, which Kopuz's playlist edits take.
pub fn track_rows(detail: &k::CatalogDetail, offset: usize, ctx: &mut Ctx) -> Vec<Item> {
    let owned = detail.kind == k::CatalogItemKind::Playlist && detail.privacy.is_some();
    detail
        .tracks
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let mut track = track(row, ctx.liked);
            if owned {
                track.set_video_id = Some((offset + index).to_string());
            }
            Item::Track(track)
        })
        .collect()
}

/// `detail`'s header. The playlist id it hands Play is the id the queue context takes.
pub fn header(detail: &k::CatalogDetail, ctx: &mut Ctx) -> Option<Header> {
    use k::CatalogItemKind as K;
    let thumbnails = thumbnails(detail.artwork.as_ref());
    (ctx.actions)(&detail.id, &detail.actions);
    if let Some(playback) = &detail.playback_id {
        (ctx.actions)(playback, &detail.actions);
    }
    Some(match detail.header {
        k::CatalogHeader::None => return None,
        k::CatalogHeader::Title => Header::Title {
            title: detail.title.clone(),
        },
        k::CatalogHeader::Artist => Header::Artist {
            name: detail.title.clone(),
            description: detail.description.clone(),
            thumbnails,
            channel_id: detail
                .actions
                .follow_ref
                .clone()
                .or_else(|| Some(detail.id.clone())),
            subscribed: detail.actions.followed,
            subscribers: detail.subtitle.clone(),
            shuffle_playlist_id: detail.playback_id.clone(),
            radio_playlist_id: None,
            monthly_listeners: detail.monthly_listeners.clone(),
        },
        k::CatalogHeader::Detail => {
            let mut subtitle = Vec::new();
            if let Some(owner) = &detail.owner {
                subtitle.push(Link {
                    text: owner.clone(),
                    target: None,
                });
            }
            if let Some(line) = &detail.subtitle {
                subtitle.push(Link {
                    text: line.clone(),
                    target: detail.artist_key.clone().map(BrowseTarget::Artist),
                });
            }
            let second_subtitle = [detail.year.as_deref(), detail.plays.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" • ");
            Header::Detail {
                title: detail.title.clone(),
                subtitle,
                second_subtitle: (!second_subtitle.is_empty()).then_some(second_subtitle),
                description: detail.description.clone(),
                thumbnails,
                playlist_id: match detail.kind {
                    K::Album | K::Playlist => Some(detail.id.clone()),
                    _ => detail.playback_id.clone(),
                },
                editable: detail.privacy.is_some(),
                saved: detail.actions.saved,
                privacy: detail.privacy.map(privacy),
            }
        }
    })
}

pub fn chips(detail: &k::CatalogDetail) -> Vec<Chip> {
    detail
        .chips
        .iter()
        .map(|chip| Chip {
            title: chip.label.clone(),
            params: chip.id.clone(),
            selected: chip.selected,
        })
        .collect()
}

pub fn page(target: BrowseTarget, header: Option<Header>, chips: Vec<Chip>) -> Page {
    Page {
        target,
        header,
        chips,
        sections: Vec::new(),
        continuation: None,
    }
}

pub fn lyrics(view: k::LyricsView) -> Option<Lyrics> {
    if !view.synced.is_empty() {
        let word_synced = view.synced.iter().any(|line| !line.chunks.is_empty());
        let lines = view
            .synced
            .into_iter()
            .map(|line| {
                let end = line.end_ms;
                let starts: Vec<u64> = line.chunks.iter().map(|c| c.start_ms).collect();
                let words = line
                    .chunks
                    .iter()
                    .enumerate()
                    .map(|(i, chunk)| LyricWord {
                        start_ms: chunk.start_ms,
                        end_ms: starts
                            .get(i + 1)
                            .copied()
                            .or(end)
                            .unwrap_or(chunk.start_ms),
                        text: chunk.text.trim_end().to_owned(),
                        joins_next: !chunk.text.ends_with(char::is_whitespace)
                            && i + 1 < starts.len(),
                    })
                    .collect();
                LyricLine {
                    start_ms: line.start_ms,
                    end_ms: line.end_ms,
                    text: line.text,
                    words,
                    background: line.background,
                    agent: None,
                    opposite_turn: line.opposite_turn,
                }
            })
            .collect();
        return Some(Lyrics {
            source: None,
            lines,
            synced: true,
            word_synced,
        });
    }
    let plain = view.plain.filter(|text| !text.trim().is_empty())?;
    Some(Lyrics {
        source: None,
        lines: plain
            .lines()
            .map(|text| LyricLine {
                text: text.to_owned(),
                ..Default::default()
            })
            .collect(),
        synced: false,
        word_synced: false,
    })
}

pub fn player(state: &k::PlayerState, now_ms: u64, liked: &HashSet<String>) -> PlayerState {
    let row = state
        .fading
        .as_ref()
        .map(|fading| &fading.track)
        .or(state.track.as_ref());
    let track = row.map(|row| track(row, liked));
    let status = match (state.phase, state.intent) {
        (_, k::Intent::Loading { .. }) => Status::Loading,
        (k::Phase::Playing, _) => Status::Playing,
        (k::Phase::Paused, _) => Status::Paused,
        (k::Phase::Idle | k::Phase::Ended, k::Intent::Stopped) => Status::Stopped,
        (k::Phase::Idle | k::Phase::Ended, k::Intent::Committed { .. }) => Status::Paused,
    };
    let mode = match row.and_then(k::TrackInfo::version) {
        Some(k::TrackVersion::Video) => PlaybackMode::Video,
        _ => PlaybackMode::Song,
    };
    PlayerState {
        status,
        position_ms: position(state.position.as_ref(), now_ms),
        duration_ms: row.and_then(|row| row.duration_ms),
        volume: state.volume,
        muted: state.muted,
        repeat: match state.queue.loop_mode {
            k::LoopMode::None => Repeat::Off,
            k::LoopMode::Queue => Repeat::All,
            k::LoopMode::Track => Repeat::One,
        },
        shuffle: state.queue.shuffle,
        stream: row
            .filter(|row| row.bitrate > 0)
            .map(|row| match &row.format {
                Some(format) => format!("{} {} kbps", format.to_lowercase(), row.bitrate),
                None => format!("{} kbps", row.bitrate),
            }),
        related_browse_id: row.map(|row| row.key.clone()),
        mode,
        playing_id: row.map(|row| row.key.clone()),
        track,
    }
}

/// Where an anchor puts playback at daemon time `now_ms`.
pub fn position(anchor: Option<&k::PositionAnchor>, now_ms: u64) -> u64 {
    match anchor {
        Some(anchor) if anchor.playing => anchor.ms + now_ms.saturating_sub(anchor.at_ms),
        Some(anchor) => anchor.ms,
        None => 0,
    }
}

pub fn queue(snapshot: &k::QueueSnapshot, liked: &HashSet<String>) -> QueueState {
    QueueState {
        tracks: snapshot.items.iter().map(|row| track(row, liked)).collect(),
        current: snapshot.position.map(|index| index as usize),
        radio: matches!(
            snapshot.context,
            Some(k::QueueContext::TrackRadio { .. } | k::QueueContext::PlaylistRadio { .. })
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artwork_urls_round_trip() {
        let art = k::ArtworkRef::new(k::ArtworkTarget::Catalog("MPREb_x@y".into()), 42);
        let thumbs = thumbnails(Some(&art));
        let small = artwork_request(&thumbs[0].url).unwrap();
        let large = artwork_request(&thumbs[1].url).unwrap();
        assert_eq!(small.target, art.target);
        assert!(!small.hq && large.hq);
    }

    #[test]
    fn album_bylines_split() {
        let (kind, artists, year) = album_byline(Some("Single • Daft Punk • 2013"));
        assert_eq!(kind.as_deref(), Some("Single"));
        assert_eq!(artists[0].text, "Daft Punk");
        assert_eq!(year.as_deref(), Some("2013"));
    }

    #[test]
    fn search_filters_use_kopuz_ids() {
        assert_eq!(search_filter_id(SearchFilter::CommunityPlaylists), "community_playlists");
        assert_eq!(search_filter("songs"), Some(SearchFilter::Songs));
    }

    #[test]
    fn library_tokens_carry_their_direction() {
        assert_eq!(read_library_token(&library_token("LIB:a|b", false)), Some(("LIB:a|b", false)));
    }
}
