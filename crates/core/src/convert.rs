//! kopuz's wire types into the window's model. Nothing here talks to the
//! daemon: the backend fetches, this reshapes, so every rule can be tested on
//! a hand-built answer.

use std::collections::HashSet;

use api::{
    ArtworkRef, ArtworkTarget, CatalogActions, CatalogDetail, CatalogHeader, CatalogItem,
    CatalogItemKind, CatalogShelf, PageEntry, ShelfLayout, TrackInfo,
};

use crate::backend::ClientError;
use crate::model::*;

/// What a conversion needs beyond the answer itself.
#[derive(Default)]
pub struct Context<'a> {
    /// Keys the account likes, for the rows a page sends without a rating.
    pub favorites: Option<&'a HashSet<String>>,
    /// The source's own pages, so a shortcut to one opens the named target.
    pub pages: &'a [PageEntry],
}

/// Which named target a page entry is, by the label kopuzd gives it.
pub fn named_target(label: &str) -> Option<BrowseTarget> {
    Some(match label {
        "home" => BrowseTarget::Home,
        "catalog_page_explore" => BrowseTarget::Explore,
        "new_releases" => BrowseTarget::NewReleases,
        "catalog_page_charts" => BrowseTarget::Charts,
        "catalog_page_moods" => BrowseTarget::MoodsAndGenres,
        "catalog_page_library_songs" => BrowseTarget::Library(LibraryTab::Songs),
        "catalog_page_library_albums" => BrowseTarget::Library(LibraryTab::Albums),
        "catalog_page_library_artists" => BrowseTarget::Library(LibraryTab::Artists),
        "catalog_page_subscriptions" => BrowseTarget::Library(LibraryTab::Subscriptions),
        "catalog_page_library_podcasts" => BrowseTarget::Library(LibraryTab::Podcasts),
        "catalog_page_uploads" => BrowseTarget::Library(LibraryTab::Uploads),
        "catalog_page_history" => BrowseTarget::History,
        _ => return None,
    })
}

fn label_key(entry: &PageEntry) -> &str {
    match &entry.label {
        api::Text::Key(key) | api::Text::Literal(key) => key,
    }
}

/// The page entry a named target opens.
pub fn page_id<'a>(pages: &'a [PageEntry], target: &BrowseTarget) -> Option<&'a str> {
    pages
        .iter()
        .find(|entry| named_target(label_key(entry)).as_ref() == Some(target))
        .map(|entry| entry.id.as_str())
}

/// The request that opens `target`. Library playlists and liked songs are
/// not catalog pages, so they have none.
pub fn request(pages: &[PageEntry], target: &BrowseTarget) -> Option<api::CatalogDetailRequest> {
    use CatalogItemKind as K;
    let (kind, id) = match target {
        BrowseTarget::Album(id) => (K::Album, id.as_str()),
        BrowseTarget::Artist(id) => (K::Artist, id.as_str()),
        BrowseTarget::Playlist(id) => (K::Playlist, id.as_str()),
        BrowseTarget::Podcast(id) => (K::Podcast, id.as_str()),
        BrowseTarget::Episode(id) => (K::Episode, id.as_str()),
        BrowseTarget::Mood(id) => (K::Mood, id.as_str()),
        BrowseTarget::Page(id) | BrowseTarget::HomeChip(id) => (K::Page, id.as_str()),
        BrowseTarget::Library(LibraryTab::Playlists | LibraryTab::LikedSongs) => return None,
        named => (K::Page, page_id(pages, named)?),
    };
    Some(api::CatalogDetailRequest::new(kind, id))
}

/// Where a tile of `kind` with `id` leads.
pub fn target(kind: CatalogItemKind, id: &str, pages: &[PageEntry]) -> Option<BrowseTarget> {
    let id = id.to_owned();
    Some(match kind {
        CatalogItemKind::Album => BrowseTarget::Album(id),
        CatalogItemKind::Artist => BrowseTarget::Artist(id),
        CatalogItemKind::Playlist => BrowseTarget::Playlist(id),
        CatalogItemKind::Podcast => BrowseTarget::Podcast(id),
        CatalogItemKind::Episode => BrowseTarget::Episode(id),
        CatalogItemKind::Mood => BrowseTarget::Mood(id),
        CatalogItemKind::Page => pages
            .iter()
            .find(|entry| entry.id == id)
            .and_then(|entry| named_target(label_key(entry)))
            .unwrap_or(BrowseTarget::Page(id)),
        CatalogItemKind::Track | CatalogItemKind::Video | CatalogItemKind::Unknown => {
            return None;
        }
    })
}

pub fn art(art: &ArtworkRef) -> Art {
    let (kind, id) = match &art.target {
        ArtworkTarget::Track(id) => (ArtKind::Track, id),
        ArtworkTarget::Album(id) => (ArtKind::Album, id),
        ArtworkTarget::Artist(id) => (ArtKind::Artist, id),
        ArtworkTarget::Playlist(id) => (ArtKind::Playlist, id),
        ArtworkTarget::Catalog(id) => (ArtKind::Catalog, id),
        ArtworkTarget::Station(id) => (ArtKind::Station, id),
        ArtworkTarget::Account(id) => (ArtKind::Account, id),
    };
    Art {
        kind,
        id: id.clone(),
        version: art.version,
    }
}

/// `None` for the demo's locally painted art, which kopuzd never saw.
pub fn artwork_target(art: &Art) -> Option<ArtworkTarget> {
    let id = art.id.clone();
    Some(match art.kind {
        ArtKind::Track => ArtworkTarget::Track(id),
        ArtKind::Album => ArtworkTarget::Album(id),
        ArtKind::Artist => ArtworkTarget::Artist(id),
        ArtKind::Playlist => ArtworkTarget::Playlist(id),
        ArtKind::Catalog => ArtworkTarget::Catalog(id),
        ArtKind::Station => ArtworkTarget::Station(id),
        ArtKind::Account => ArtworkTarget::Account(id),
        ArtKind::Demo => return None,
    })
}

pub fn rating(rating: api::Rating) -> Rating {
    match rating {
        api::Rating::Like => Rating::Like,
        api::Rating::Dislike => Rating::Dislike,
        api::Rating::None => Rating::Indifferent,
    }
}

pub fn api_rating(rating: Rating) -> api::Rating {
    match rating {
        Rating::Like => api::Rating::Like,
        Rating::Dislike => api::Rating::Dislike,
        Rating::Indifferent => api::Rating::None,
    }
}

pub fn actions(actions: &CatalogActions) -> Actions {
    Actions {
        rate_ref: actions.rate_ref.clone(),
        rating: actions.rating.map(rating),
        save_ref: actions.save_ref.clone(),
        saved: actions.saved,
        follow_ref: actions.follow_ref.clone(),
        followed: actions.followed,
        history_token: actions.history_token.clone(),
    }
}

fn some_text(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// A row as the window draws it. A row with no rating of its own is a
/// like when the account's favorites hold it.
pub fn track(info: &TrackInfo, kind: TrackKind, actions: Actions, ctx: &Context) -> Track {
    let artists = if info.credits.is_empty() {
        some_text(&info.artist)
            .map(|text| vec![Link { text, target: None }])
            .unwrap_or_default()
    } else {
        info.credits
            .iter()
            .map(|credit| Link {
                text: credit.name.clone(),
                target: credit.key.clone().map(BrowseTarget::Artist),
            })
            .collect()
    };
    let album = some_text(&info.album).map(|text| Link {
        text,
        target: some_text(&info.album_id).map(BrowseTarget::Album),
    });
    let mut actions = actions;
    if actions.rating.is_none()
        && ctx
            .favorites
            .is_some_and(|favorites| favorites.contains(&info.key))
    {
        actions.rating = Some(Rating::Like);
    }
    if actions.rate_ref.is_none() {
        actions.rate_ref = Some(info.key.clone());
    }
    Track {
        key: info.key.clone(),
        title: info.title.clone(),
        artists,
        album,
        duration_ms: info.duration_ms.filter(|ms| *ms > 0),
        art: info.artwork.as_ref().map(art),
        explicit: info.explicit,
        kind,
        plays: info.plays.as_deref().and_then(some_text),
        actions,
        counterpart: info.counterpart.as_ref().map(|other| {
            Box::new(Counterpart {
                key: other.key.clone(),
                version: mode(other.version),
                duration_ms: other.duration_ms.filter(|ms| *ms > 0),
            })
        }),
    }
}

pub fn mode(version: api::TrackVersion) -> PlaybackMode {
    match version {
        api::TrackVersion::Song => PlaybackMode::Song,
        api::TrackVersion::Video => PlaybackMode::Video,
    }
}

pub fn version(mode: PlaybackMode) -> api::TrackVersion {
    match mode {
        PlaybackMode::Song => api::TrackVersion::Song,
        PlaybackMode::Video => api::TrackVersion::Video,
    }
}

/// `#rrggbb` to `0xRRGGBB`.
fn color(accent: &str) -> Option<u32> {
    u32::from_str_radix(accent.strip_prefix('#')?, 16).ok()
}

pub fn item(item: &CatalogItem, ctx: &Context) -> Option<Item> {
    let acts = actions(&item.actions);
    let art = item.artwork.as_ref().map(art);
    let subtitle = item.subtitle.as_deref().and_then(some_text);
    Some(match item.kind {
        CatalogItemKind::Track | CatalogItemKind::Video | CatalogItemKind::Episode => {
            let kind = match item.kind {
                CatalogItemKind::Video => TrackKind::Video,
                CatalogItemKind::Episode => TrackKind::Episode,
                _ => TrackKind::Song,
            };
            let mut row = track(item.track.as_ref()?, kind, acts, ctx);
            if row.title.is_empty() {
                row.title.clone_from(&item.title);
            }
            if row.art.is_none() {
                row.art = art;
            }
            Item::Track(row)
        }
        CatalogItemKind::Album => Item::Album {
            browse_id: item.id.clone(),
            title: item.title.clone(),
            album_type: None,
            artists: subtitle
                .map(|text| vec![Link { text, target: None }])
                .unwrap_or_default(),
            year: None,
            art,
            explicit: item.explicit,
            actions: acts,
        },
        CatalogItemKind::Artist => Item::Artist {
            browse_id: item.id.clone(),
            name: item.title.clone(),
            subtitle,
            art,
            actions: acts,
        },
        CatalogItemKind::Playlist => Item::Playlist {
            playlist_id: item.id.clone(),
            title: item.title.clone(),
            subtitle,
            art,
            actions: acts,
            web_url: item.web_url.clone(),
        },
        CatalogItemKind::Podcast => Item::Podcast {
            browse_id: item.id.clone(),
            title: item.title.clone(),
            subtitle,
            art,
            actions: acts,
        },
        CatalogItemKind::Mood => Item::Mood {
            title: item.title.clone(),
            id: item.id.clone(),
            color: item.accent.as_deref().and_then(color),
        },
        CatalogItemKind::Page => {
            let target = target(item.kind, &item.id, ctx.pages)?;
            // The web app's own icons for Explore's three buttons, which the
            // tile draws by name.
            let icon = match &target {
                BrowseTarget::NewReleases => Some("MUSIC_NEW_RELEASE"),
                BrowseTarget::Charts => Some("TRENDING_UP"),
                BrowseTarget::MoodsAndGenres => Some("STICKER_EMOTICON"),
                BrowseTarget::Page(id) if id.starts_with(charts_prefix(ctx.pages)) => {
                    Some("TRENDING_UP")
                }
                _ => None,
            };
            Item::Shortcut {
                title: item.title.clone(),
                target,
                icon: icon.map(str::to_owned),
            }
        }
        CatalogItemKind::Unknown => return None,
    })
}

/// Charts open with parameters after the charts page's own id.
fn charts_prefix(pages: &[PageEntry]) -> &str {
    page_id(pages, &BrowseTarget::Charts).unwrap_or("\u{0}")
}

pub fn layout(layout: ShelfLayout) -> SectionLayout {
    match layout {
        ShelfLayout::Carousel => SectionLayout::Carousel,
        ShelfLayout::Grid => SectionLayout::Grid,
        ShelfLayout::List => SectionLayout::List,
        ShelfLayout::TrackGrid => SectionLayout::TrackGrid,
        ShelfLayout::Hero => SectionLayout::Hero,
    }
}

pub fn items(items: &[CatalogItem], ctx: &Context) -> Vec<Item> {
    items.iter().filter_map(|i| item(i, ctx)).collect()
}

pub fn section(shelf: &CatalogShelf, ctx: &Context) -> Section {
    let items = items(&shelf.items, ctx);
    // The top result plays as its kind does: a song starts its radio, an
    // artist shuffles their songs and starts a radio from the first of them.
    let (shuffle, radio) = match (shelf.layout, shelf.items.first()) {
        (ShelfLayout::Hero, Some(top)) if top.kind == CatalogItemKind::Artist => (
            Some(PlaySource::Artist {
                key: top.id.clone(),
            }),
            first_track(&items).map(|key| PlaySource::Radio { key }),
        ),
        _ => (None, None),
    };
    Section {
        title: some_text(&shelf.title),
        strapline: shelf.strapline.as_deref().and_then(some_text),
        layout: layout(shelf.layout),
        more: shelf
            .more_ref
            .as_deref()
            .and_then(|id| target(shelf.more_kind, id, ctx.pages)),
        continuation: shelf.continuation.clone().map(Continuation),
        filter: shelf
            .search_filter
            .as_deref()
            .and_then(SearchFilter::from_id),
        shuffle,
        radio,
        items,
    }
}

fn first_track(items: &[Item]) -> Option<String> {
    items.iter().find_map(|item| match item {
        Item::Track(track) => Some(track.key.clone()),
        _ => None,
    })
}

fn privacy(privacy: api::PlaylistPrivacy) -> Privacy {
    match privacy {
        api::PlaylistPrivacy::Private => Privacy::Private,
        api::PlaylistPrivacy::Unlisted => Privacy::Unlisted,
        api::PlaylistPrivacy::Public => Privacy::Public,
    }
}

pub fn api_privacy(privacy: Privacy) -> api::PlaylistPrivacy {
    match privacy {
        Privacy::Private => api::PlaylistPrivacy::Private,
        Privacy::Unlisted => api::PlaylistPrivacy::Unlisted,
        Privacy::Public => api::PlaylistPrivacy::Public,
    }
}

/// "13 songs • 1 hour, 14 minutes", the line under an album's byline.
pub fn length(count: usize, total_ms: u64) -> String {
    let songs = match count {
        1 => "1 song".to_owned(),
        n => format!("{n} songs"),
    };
    let minutes = total_ms / 60_000;
    let time = match (minutes / 60, minutes % 60) {
        (0, 0) => return songs,
        (0, m) => format!("{m} minute{}", if m == 1 { "" } else { "s" }),
        (h, 0) => format!("{h} hour{}", if h == 1 { "" } else { "s" }),
        (h, m) => format!(
            "{h} hour{}, {m} minute{}",
            if h == 1 { "" } else { "s" },
            if m == 1 { "" } else { "s" }
        ),
    };
    format!("{songs} \u{2022} {time}")
}

fn tracks(detail: &CatalogDetail, ctx: &Context) -> Vec<Item> {
    let kind = match detail.kind {
        CatalogItemKind::Podcast | CatalogItemKind::Episode => TrackKind::Episode,
        _ => TrackKind::Song,
    };
    detail
        .tracks
        .iter()
        .map(|info| Item::Track(track(info, kind, Actions::default(), ctx)))
        .collect()
}

fn header(target: &BrowseTarget, detail: &CatalogDetail, sections: &[Section]) -> Option<Header> {
    let art = detail.artwork.as_ref().map(art);
    let description = detail.description.as_deref().and_then(some_text);
    Some(match detail.header {
        CatalogHeader::None => return None,
        CatalogHeader::Title => Header::Title {
            title: detail.title.clone(),
        },
        CatalogHeader::Detail => {
            // "Album • Daft Punk • 2013", or "Playlist • YouTube Charts", as
            // the web app's header reads.
            let plain = |text: Option<&str>| {
                text.and_then(some_text)
                    .map(|text| Link { text, target: None })
            };
            let byline = detail.subtitle.as_deref().and_then(some_text);
            let subtitle = plain(detail.album_type.as_deref())
                .into_iter()
                .chain(byline.map(|text| Link {
                    text,
                    target: detail.artist_key.clone().map(BrowseTarget::Artist),
                }))
                .chain(plain(detail.owner.as_deref()))
                .chain(plain(detail.year.as_deref()))
                .collect();
            let rows: Vec<&Track> = sections
                .iter()
                .filter(|section| section.layout == SectionLayout::List)
                .flat_map(|section| &section.items)
                .filter_map(|item| match item {
                    Item::Track(track) => Some(track),
                    _ => None,
                })
                .collect();
            // A list still paging in has no total to give.
            let complete = sections
                .iter()
                .all(|section| section.continuation.is_none());
            let counted = (!rows.is_empty() && complete).then(|| {
                length(
                    rows.len(),
                    rows.iter().filter_map(|track| track.duration_ms).sum(),
                )
            });
            let plays = detail.plays.as_deref().and_then(some_text);
            let second_subtitle = match (plays, counted) {
                (Some(plays), Some(counted)) => Some(format!("{plays} \u{2022} {counted}")),
                (plays, counted) => plays.or(counted),
            };
            // An album or a playlist plays by its id; any other list plays
            // the rows it shows.
            let play = (!rows.is_empty()).then(|| match target {
                BrowseTarget::Album(_) | BrowseTarget::Playlist(_) => PlaySource::Page {
                    target: target.clone(),
                },
                _ => PlaySource::Tracks {
                    tracks: rows.iter().map(|track| (*track).clone()).collect(),
                },
            });
            Header::Detail {
                title: detail.title.clone(),
                subtitle,
                second_subtitle,
                description,
                art,
                play,
                editable: detail.privacy.is_some(),
                privacy: detail.privacy.map(privacy),
                actions: actions(&detail.actions),
            }
        }
        CatalogHeader::Artist => Header::Artist {
            name: detail.title.clone(),
            description,
            art,
            subscribers: detail.subtitle.as_deref().and_then(some_text),
            monthly_listeners: detail.monthly_listeners.as_deref().and_then(some_text),
            shuffle: Some(PlaySource::Artist {
                key: detail.id.clone(),
            }),
            radio: sections
                .iter()
                .find(|section| section.layout == SectionLayout::List)
                .and_then(|section| first_track(&section.items))
                .map(|key| PlaySource::Radio { key }),
            actions: actions(&detail.actions),
        },
    })
}

pub fn page(target: BrowseTarget, detail: &CatalogDetail, ctx: &Context) -> Page {
    let rows = tracks(detail, ctx);
    let mut continuation = detail.continuation.clone().map(Continuation);
    let mut sections = Vec::new();
    if !rows.is_empty() {
        // A track list's continuation pages the list, not the page.
        sections.push(Section {
            layout: SectionLayout::List,
            items: rows,
            continuation: continuation.take(),
            ..Section::default()
        });
    }
    sections.extend(detail.shelves.iter().map(|shelf| section(shelf, ctx)));
    let header = header(&target, detail, &sections);
    Page {
        header,
        chips: detail
            .chips
            .iter()
            .map(|chip| Chip {
                title: chip.label.clone(),
                id: chip.id.clone(),
                selected: chip.selected,
            })
            .collect(),
        sections,
        continuation,
        target,
    }
}

/// More of a page, or of one of its sections: a track list continues with
/// tracks, a shelf with its one answering shelf, a page with shelves.
pub fn more(detail: &CatalogDetail, part: Option<usize>, ctx: &Context) -> ContinuationPage {
    let continuation = detail.continuation.clone().map(Continuation);
    match part {
        Some(_) if !detail.tracks.is_empty() => ContinuationPage {
            sections: Vec::new(),
            items: tracks(detail, ctx),
            continuation,
        },
        Some(_) => {
            let shelf = detail.shelves.first();
            ContinuationPage {
                sections: Vec::new(),
                items: shelf.map(|s| items(&s.items, ctx)).unwrap_or_default(),
                continuation: shelf
                    .and_then(|s| s.continuation.clone())
                    .map(Continuation)
                    .or(continuation),
            }
        }
        None => ContinuationPage {
            sections: detail.shelves.iter().map(|s| section(s, ctx)).collect(),
            items: Vec::new(),
            continuation,
        },
    }
}

pub fn search(
    query: &str,
    filter: Option<SearchFilter>,
    results: &api::SearchResults,
    ctx: &Context,
) -> SearchResults {
    let mut sections: Vec<Section> = results.shelves.iter().map(|s| section(s, ctx)).collect();
    // A source with no filtered search answers with plain rows.
    if sections.is_empty() && !results.tracks.is_empty() {
        sections.push(Section {
            title: Some("Songs".into()),
            items: results
                .tracks
                .iter()
                .map(|info| Item::Track(track(info, TrackKind::Song, Actions::default(), ctx)))
                .collect(),
            ..Section::default()
        });
    }
    SearchResults {
        query: query.to_owned(),
        filter,
        correction: results.correction.clone(),
        sections,
        continuation: results.continuation.clone().map(Continuation),
    }
}

pub fn search_more(results: &api::SearchResults, ctx: &Context) -> ContinuationPage {
    let shelf = results.shelves.first();
    ContinuationPage {
        sections: Vec::new(),
        items: shelf.map(|s| items(&s.items, ctx)).unwrap_or_default(),
        continuation: results.continuation.clone().map(Continuation),
    }
}

pub fn suggestion(suggestion: &api::SearchSuggestion, ctx: &Context) -> Option<Suggestion> {
    Some(match &suggestion.item {
        Some(hit) => Suggestion::Item(item(hit, ctx)?),
        None => Suggestion::Query {
            text: suggestion.text.clone(),
            from_history: suggestion.from_history,
        },
    })
}

/// Lyrics as the lyrics view lays them out. A chunk carries only its start,
/// so it ends where the next one starts, or where its line does.
pub fn lyrics(view: &api::LyricsView) -> Option<Lyrics> {
    if !view.synced.is_empty() {
        let lines: Vec<LyricLine> = view
            .synced
            .iter()
            .enumerate()
            .map(|(index, line)| {
                let line_end = line
                    .end_ms
                    .or_else(|| view.synced.get(index + 1).map(|next| next.start_ms));
                let words = line
                    .chunks
                    .iter()
                    .enumerate()
                    .filter(|(_, chunk)| !chunk.text.trim().is_empty())
                    .map(|(at, chunk)| {
                        let next = line.chunks.get(at + 1);
                        let end_ms = next
                            .map(|next| next.start_ms)
                            .or(line_end)
                            .unwrap_or(chunk.start_ms + 400)
                            .max(chunk.start_ms);
                        LyricWord {
                            start_ms: chunk.start_ms,
                            end_ms,
                            text: chunk.text.trim().to_owned(),
                            joins_next: next.is_some()
                                && !chunk.text.ends_with(char::is_whitespace)
                                && !next.is_some_and(|n| n.text.starts_with(char::is_whitespace)),
                        }
                    })
                    .collect::<Vec<_>>();
                LyricLine {
                    start_ms: line.start_ms,
                    end_ms: line.end_ms,
                    text: line.text.clone(),
                    words,
                    background: line.background,
                    agent: None,
                    opposite_turn: line.opposite_turn,
                }
            })
            .collect();
        let word_synced = lines.iter().any(|line| !line.words.is_empty());
        return Some(Lyrics {
            source: None,
            lines,
            synced: true,
            word_synced,
        });
    }
    let plain = view.plain.as_deref().and_then(some_text)?;
    Some(Lyrics {
        source: None,
        lines: plain
            .lines()
            .map(|text| LyricLine {
                text: text.to_owned(),
                ..LyricLine::default()
            })
            .collect(),
        synced: false,
        word_synced: false,
    })
}

pub fn status(state: &api::PlayerState) -> Status {
    match (state.intent, state.phase) {
        (api::Intent::Loading { .. }, api::Phase::Paused) => Status::Paused,
        (api::Intent::Loading { .. }, _) => Status::Loading,
        (_, api::Phase::Playing) => Status::Playing,
        (_, api::Phase::Paused | api::Phase::Ended) => Status::Paused,
        (api::Intent::Committed { .. }, api::Phase::Idle) if state.track.is_some() => {
            Status::Loading
        }
        _ => Status::Stopped,
    }
}

pub fn repeat(mode: api::LoopMode) -> Repeat {
    match mode {
        api::LoopMode::None => Repeat::Off,
        api::LoopMode::Queue => Repeat::All,
        api::LoopMode::Track => Repeat::One,
    }
}

pub fn loop_mode(repeat: Repeat) -> api::LoopMode {
    match repeat {
        Repeat::Off => api::LoopMode::None,
        Repeat::All => api::LoopMode::Queue,
        Repeat::One => api::LoopMode::Track,
    }
}

/// Where playback is by the daemon's clock at `now_ms`.
pub fn position(anchor: Option<&api::PositionAnchor>, now_ms: u64) -> u64 {
    anchor.map_or(0, |anchor| match anchor.playing {
        true => anchor.ms + now_ms.saturating_sub(anchor.at_ms),
        false => anchor.ms,
    })
}

pub fn player(state: &api::PlayerState, ctx: &Context) -> PlayerState {
    // During a crossfade the outgoing track is the one still on screen.
    let shown = state
        .fading
        .as_ref()
        .map(|fading| &fading.track)
        .or(state.track.as_ref());
    let track = shown.map(|info| track(info, TrackKind::Song, Actions::default(), ctx));
    let position_ms = match &state.fading {
        Some(fading) => fading.position_ms,
        None => position(state.position.as_ref(), state.now_ms),
    };
    PlayerState {
        status: status(state),
        duration_ms: track.as_ref().and_then(|track| track.duration_ms),
        stream: shown
            .filter(|info| info.bitrate > 0)
            .map(|info| match &info.format {
                Some(format) => format!("{} {} kbps", format.to_lowercase(), info.bitrate),
                None => format!("{} kbps", info.bitrate),
            }),
        track,
        position_ms,
        volume: state.volume,
        muted: state.muted,
        repeat: repeat(state.queue.loop_mode),
        shuffle: state.queue.shuffle,
        mode: PlaybackMode::default(),
        output_latency_ms: state.output_latency_ms.unwrap_or(0),
    }
}

pub fn queue(snapshot: &api::QueueSnapshot, ctx: &Context) -> QueueState {
    QueueState {
        tracks: snapshot
            .items
            .iter()
            .map(|info| track(info, TrackKind::Song, Actions::default(), ctx))
            .collect(),
        current: snapshot.position.map(|index| index as usize),
    }
}

/// How much of the track is downloaded, as a play time: the range that
/// starts at the beginning, as a share of the file.
pub fn buffered_ms(ranges: &[api::BufferedRange], duration_ms: Option<u64>) -> u64 {
    let (Some(duration), Some(head)) = (duration_ms, ranges.iter().find(|r| r.start == 0)) else {
        return 0;
    };
    match head.total.filter(|total| *total > 0) {
        Some(total) => (duration as u128 * head.end.min(total) as u128 / total as u128) as u64,
        None => 0,
    }
}

pub fn error(error: api::ApiError) -> ClientError {
    use api::ErrorCode;
    match error.code {
        ErrorCode::SourceAuthExpired => ClientError::SignedOut,
        ErrorCode::NotFound => ClientError::NotFound(error.message),
        ErrorCode::SourceUnreachable => ClientError::Network(error.message),
        ErrorCode::InvalidInput => ClientError::BadRequest(error.message),
        ErrorCode::Unsupported => ClientError::Unsupported(error.message),
        ErrorCode::DaemonGone => {
            ClientError::Disconnected("The music daemon is not running.".into())
        }
        ErrorCode::Conflict | ErrorCode::Internal => ClientError::Failed(error.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(key: &str) -> TrackInfo {
        TrackInfo {
            key: key.into(),
            title: format!("song {key}"),
            artist: "Ada & Boris".into(),
            album: "First".into(),
            album_id: "album-1".into(),
            duration_ms: Some(0),
            credits: vec![
                api::ArtistCredit {
                    name: "Ada".into(),
                    key: Some("UCada".into()),
                },
                api::ArtistCredit {
                    name: "Boris".into(),
                    key: None,
                },
            ],
            ..TrackInfo::default()
        }
    }

    fn entry(id: &str, label: &str) -> PageEntry {
        PageEntry {
            id: id.into(),
            label: api::Text::Key(label.into()),
            icon: api::Icon::default(),
        }
    }

    #[test]
    fn a_row_links_its_credits_and_album_and_drops_a_zero_length() {
        let row = track(
            &info("v1"),
            TrackKind::Song,
            Actions::default(),
            &Context::default(),
        );
        assert_eq!(row.artists.len(), 2);
        assert_eq!(
            row.artists[0].target,
            Some(BrowseTarget::Artist("UCada".into()))
        );
        assert_eq!(row.artists[1].target, None);
        assert_eq!(
            row.album.unwrap().target,
            Some(BrowseTarget::Album("album-1".into()))
        );
        assert_eq!(row.duration_ms, None);
        assert_eq!(row.actions.rate_ref.as_deref(), Some("v1"));
    }

    #[test]
    fn a_favorite_without_a_rating_reads_as_liked() {
        let favorites = HashSet::from(["v1".to_owned()]);
        let ctx = Context {
            favorites: Some(&favorites),
            pages: &[],
        };
        let liked = track(&info("v1"), TrackKind::Song, Actions::default(), &ctx);
        let other = track(&info("v2"), TrackKind::Song, Actions::default(), &ctx);
        assert_eq!(liked.actions.rating, Some(Rating::Like));
        assert_eq!(other.actions.rating, None);
    }

    #[test]
    fn named_targets_open_the_page_entry_with_their_label() {
        let pages = [
            entry("FEmusic_home", "home"),
            entry("FEmusic_explore", "catalog_page_explore"),
            entry("FEmusic_history", "catalog_page_history"),
        ];
        let open = |target| request(&pages, &target).map(|r| r.id);
        assert_eq!(
            open(BrowseTarget::Explore).as_deref(),
            Some("FEmusic_explore")
        );
        assert_eq!(
            open(BrowseTarget::History).as_deref(),
            Some("FEmusic_history")
        );
        assert_eq!(open(BrowseTarget::Charts), None);
        assert_eq!(
            target(CatalogItemKind::Page, "FEmusic_explore", &pages),
            Some(BrowseTarget::Explore)
        );
        assert_eq!(
            target(CatalogItemKind::Page, "FEmusic_charts?x", &pages),
            Some(BrowseTarget::Page("FEmusic_charts?x".into()))
        );
    }

    #[test]
    fn an_album_page_lists_its_tracks_under_a_counted_header() {
        let detail = CatalogDetail {
            kind: CatalogItemKind::Album,
            id: "MPREb".into(),
            title: "First".into(),
            subtitle: Some("Ada".into()),
            artist_key: Some("UCada".into()),
            year: Some("2013".into()),
            header: CatalogHeader::Detail,
            tracks: vec![
                TrackInfo {
                    duration_ms: Some(1_800_000),
                    ..info("a")
                },
                TrackInfo {
                    duration_ms: Some(2_700_000),
                    ..info("b")
                },
            ],
            ..CatalogDetail::default()
        };
        let page = page(
            BrowseTarget::Album("MPREb".into()),
            &detail,
            &Context::default(),
        );
        assert_eq!(page.sections[0].items.len(), 2);
        let Some(Header::Detail {
            subtitle,
            second_subtitle,
            play,
            ..
        }) = page.header
        else {
            panic!("no detail header");
        };
        assert_eq!(
            subtitle[0].target,
            Some(BrowseTarget::Artist("UCada".into()))
        );
        assert_eq!(subtitle[1].text, "2013");
        assert_eq!(
            second_subtitle.as_deref(),
            Some("2 songs \u{2022} 1 hour, 15 minutes")
        );
        assert!(matches!(play, Some(PlaySource::Page { .. })));
    }

    #[test]
    fn a_header_reads_like_the_web_app_and_plays_by_id() {
        let detail = CatalogDetail {
            kind: CatalogItemKind::Playlist,
            id: "VLPL1".into(),
            title: "Top 100".into(),
            header: CatalogHeader::Detail,
            owner: Some("YouTube Charts".into()),
            plays: Some("2.1M views".into()),
            tracks: vec![TrackInfo {
                duration_ms: Some(180_000),
                explicit: true,
                plays: Some("9M plays".into()),
                counterpart: Some(api::TrackCounterpart {
                    key: "video".into(),
                    version: api::TrackVersion::Video,
                    duration_ms: Some(200_000),
                }),
                ..info("a")
            }],
            ..CatalogDetail::default()
        };
        let target = BrowseTarget::Playlist("VLPL1".into());
        let page = page(target.clone(), &detail, &Context::default());
        let Some(Header::Detail {
            subtitle,
            second_subtitle,
            play,
            ..
        }) = &page.header
        else {
            panic!("no detail header");
        };
        assert_eq!(subtitle[0].text, "YouTube Charts");
        assert_eq!(
            second_subtitle.as_deref(),
            Some("2.1M views \u{2022} 1 song \u{2022} 3 minutes")
        );
        assert_eq!(play, &Some(PlaySource::Page { target }));
        let Item::Track(row) = &page.sections[0].items[0] else {
            panic!("no row");
        };
        assert!(row.explicit);
        assert_eq!(row.plays.as_deref(), Some("9M plays"));
        assert_eq!(row.version(), Some(PlaybackMode::Song));
        assert_eq!(row.counterpart.as_ref().unwrap().key, "video");
    }

    #[test]
    fn a_playlist_continuation_pages_its_tracks_not_the_page() {
        let detail = CatalogDetail {
            kind: CatalogItemKind::Playlist,
            header: CatalogHeader::Detail,
            tracks: vec![info("a")],
            continuation: Some("next".into()),
            ..CatalogDetail::default()
        };
        let page = page(
            BrowseTarget::Playlist("PL".into()),
            &detail,
            &Context::default(),
        );
        assert_eq!(page.continuation, None);
        assert_eq!(
            page.sections[0].continuation,
            Some(Continuation("next".into()))
        );
        let more = more(&detail, Some(0), &Context::default());
        assert_eq!(more.items.len(), 1);
    }

    #[test]
    fn chunks_end_where_the_next_starts_and_join_inside_a_word() {
        let view = api::LyricsView {
            plain: None,
            synced: vec![api::LyricLineView {
                start_ms: 1000,
                end_ms: Some(3000),
                text: "Hello there".into(),
                chunks: vec![
                    api::LyricChunkView {
                        start_ms: 1000,
                        text: "Hel".into(),
                    },
                    api::LyricChunkView {
                        start_ms: 1300,
                        text: "lo ".into(),
                    },
                    api::LyricChunkView {
                        start_ms: 2000,
                        text: "there".into(),
                    },
                ],
                ..api::LyricLineView::default()
            }],
        };
        let lyrics = lyrics(&view).unwrap();
        assert!(lyrics.synced && lyrics.word_synced);
        let words = &lyrics.lines[0].words;
        assert_eq!((words[0].end_ms, words[0].joins_next), (1300, true));
        assert_eq!((words[1].text.as_str(), words[1].joins_next), ("lo", false));
        assert_eq!(words[2].end_ms, 3000);
    }

    #[test]
    fn the_buffered_head_is_a_share_of_the_track() {
        let ranges = [api::BufferedRange {
            start: 0,
            end: 250,
            total: Some(1000),
        }];
        assert_eq!(buffered_ms(&ranges, Some(200_000)), 50_000);
        assert_eq!(buffered_ms(&ranges, None), 0);
    }

    #[test]
    fn a_loading_track_reads_as_loading_until_it_plays() {
        let mut state = api::PlayerState {
            intent: api::Intent::Loading {
                token: 1,
                from_token: None,
            },
            track: Some(info("a")),
            ..api::PlayerState::default()
        };
        assert_eq!(status(&state), Status::Loading);
        state.intent = api::Intent::Committed { token: 1 };
        state.phase = api::Phase::Playing;
        assert_eq!(status(&state), Status::Playing);
        state.phase = api::Phase::Paused;
        assert_eq!(status(&state), Status::Paused);
        state.track = None;
        state.intent = api::Intent::Stopped;
        state.phase = api::Phase::Idle;
        assert_eq!(status(&state), Status::Stopped);
    }
}
