//! Every parser against a recorded response in `fixtures/`. Anonymous
//! responses were recorded with hl=en, gl=US. Pages that need a session have
//! `#[ignore]`d tests naming the fixture they are waiting for.

use formalmusic_innertube::api::*;
use formalmusic_innertube::parse;
use serde_json::Value;

fn load(name: &str) -> Value {
    let path = format!("{}/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&raw).unwrap()
}

fn page(name: &str, target: BrowseTarget) -> Page {
    parse::page::parse_page(target, &load(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn raw(id: &str) -> BrowseTarget {
    BrowseTarget::Raw {
        browse_id: id.into(),
        params: None,
    }
}

fn assert_track(t: &Track) {
    assert_eq!(t.video_id.len(), 11, "video id {:?}", t.video_id);
    assert!(!t.title.trim().is_empty());
    assert!(!t.thumbnails.is_empty(), "{} has no thumbnails", t.title);
    assert!(t.thumbnails.iter().all(|th| th.url.starts_with("https://")));
}

fn assert_item(item: &Item) {
    match item {
        Item::Track(t) => assert_track(t),
        Item::Album {
            browse_id,
            title,
            thumbnails,
            ..
        } => {
            assert!(browse_id.starts_with("MPRE"), "{browse_id}");
            assert!(!title.is_empty() && !thumbnails.is_empty());
        }
        Item::Artist {
            browse_id, name, ..
        } => {
            assert!(
                browse_id.starts_with("UC") || browse_id.starts_with("MPLAUC"),
                "{browse_id}"
            );
            assert!(!name.is_empty());
        }
        Item::Playlist {
            playlist_id,
            title,
            thumbnails,
            ..
        } => {
            assert!(
                !playlist_id.is_empty() && !playlist_id.starts_with("VL"),
                "{playlist_id}"
            );
            assert!(!title.is_empty() && !thumbnails.is_empty());
        }
        Item::Podcast {
            browse_id, title, ..
        } => assert!(browse_id.starts_with("MPSP") && !title.is_empty()),
        Item::Mood { title, params, .. } => assert!(!title.is_empty() && !params.is_empty()),
    }
}

fn assert_sections(sections: &[Section]) {
    assert!(!sections.is_empty());
    for section in sections {
        assert!(
            !section.items.is_empty() || section.continuation.is_some(),
            "empty section {:?}",
            section.title
        );
        section.items.iter().for_each(assert_item);
    }
}

fn tracks(section: &Section) -> Vec<&Track> {
    section
        .items
        .iter()
        .filter_map(|i| {
            if let Item::Track(t) = i {
                Some(t)
            } else {
                None
            }
        })
        .collect()
}

fn titled<'a>(sections: &'a [Section], title: &str) -> &'a Section {
    sections
        .iter()
        .find(|s| s.title.as_deref() == Some(title))
        .unwrap_or_else(|| {
            panic!(
                "no section {title:?} in {:?}",
                sections.iter().map(|s| &s.title).collect::<Vec<_>>()
            )
        })
}

#[test]
fn home() {
    let home = page("home", BrowseTarget::Home);
    assert!(home.chips.len() >= 5);
    assert!(
        home.chips
            .iter()
            .all(|c| !c.params.is_empty() && !c.title.is_empty())
    );
    assert!(home.continuation.is_some());
    assert_sections(&home.sections);
    assert!(
        home.sections
            .iter()
            .all(|s| s.title.is_some() && s.layout == SectionLayout::Carousel)
    );
}

#[test]
fn home_continuation() {
    let more = parse::page::parse_continuation(&load("home_continuation")).unwrap();
    assert_sections(&more.sections);
    let quick_picks = titled(&more.sections, "Quick picks");
    assert_eq!(quick_picks.layout, SectionLayout::TrackGrid);
    let picks = tracks(quick_picks);
    assert!(picks.len() >= 8);
    assert!(
        picks
            .iter()
            .all(|t| !t.artists.is_empty() && t.album.is_some())
    );
}

#[test]
fn explore() {
    let explore = page("explore", BrowseTarget::Explore);
    assert_sections(&explore.sections);
    let moods = titled(&explore.sections, "Moods & genres");
    assert_eq!(moods.layout, SectionLayout::Grid);
    assert_eq!(moods.more, Some(BrowseTarget::MoodsAndGenres));
    assert!(
        moods
            .items
            .iter()
            .all(|i| matches!(i, Item::Mood { color: Some(_), .. }))
    );
    let albums = titled(&explore.sections, "New albums & singles");
    assert!(albums.items.iter().all(|i| matches!(
        i,
        Item::Album {
            playlist_id: Some(_),
            ..
        }
    )));
    assert_eq!(
        titled(&explore.sections, "Trending").layout,
        SectionLayout::TrackGrid
    );
}

#[test]
fn charts() {
    let charts = page("charts", BrowseTarget::Charts);
    assert_eq!(
        charts.header,
        Some(Header::Title {
            title: "Charts".into()
        })
    );
    assert_sections(&charts.sections);
    let artists = titled(&charts.sections, "Top artists");
    assert!(artists.items.len() >= 10);
    assert!(artists.items.iter().all(|i| matches!(
        i,
        Item::Artist {
            subtitle: Some(_),
            ..
        }
    )));
}

#[test]
fn moods_and_genres() {
    let moods = page("moods", BrowseTarget::MoodsAndGenres);
    assert_eq!(moods.sections.len(), 2);
    assert!(
        moods
            .sections
            .iter()
            .all(|s| s.layout == SectionLayout::Grid)
    );
    let total: usize = moods.sections.iter().map(|s| s.items.len()).sum();
    assert!(total >= 20);
    assert_sections(&moods.sections);
}

#[test]
fn mood_category() {
    let category = page(
        "mood_category",
        BrowseTarget::MoodCategory {
            params: "ggMPOg1uX2NXUkgxdW0zUHJp".into(),
        },
    );
    assert_eq!(
        category.header,
        Some(Header::Title {
            title: "Blues".into()
        })
    );
    assert_sections(&category.sections);
    let songs = titled(&category.sections, "Songs");
    assert_eq!(songs.layout, SectionLayout::TrackGrid);
    assert!(tracks(songs).iter().all(|t| !t.artists.is_empty()));
}

#[test]
fn new_releases() {
    let releases = page("new_releases", BrowseTarget::NewReleases);
    assert_sections(&releases.sections);
    let albums = titled(&releases.sections, "Albums & singles");
    assert!(albums.items.len() >= 10);
    assert!(matches!(albums.more, Some(BrowseTarget::Raw { .. })));
}

#[test]
fn album() {
    let album = page("album", BrowseTarget::Album("MPREb_K8qWMWVqXGi".into()));
    let Some(Header::Detail {
        title,
        subtitle,
        second_subtitle,
        description,
        playlist_id,
        editable,
        thumbnails,
        ..
    }) = &album.header
    else {
        panic!("album header: {:?}", album.header);
    };
    assert_eq!(title, "Random Access Memories");
    assert!(
        subtitle
            .iter()
            .any(|l| l.text == "Daft Punk" && matches!(l.target, Some(BrowseTarget::Artist(_))))
    );
    assert!(
        second_subtitle
            .as_deref()
            .is_some_and(|s| s.contains("13 songs"))
    );
    assert!(description.is_some() && !thumbnails.is_empty() && !editable);
    assert!(
        playlist_id
            .as_deref()
            .is_some_and(|id| id.starts_with("OLAK5uy_"))
    );

    let list = &album.sections[0];
    assert_eq!(list.layout, SectionLayout::List);
    let rows = tracks(list);
    assert_eq!(rows.len(), 13);
    for t in rows {
        assert_track(t);
        assert!(t.duration_ms.is_some_and(|ms| ms > 60_000));
        assert_eq!(
            t.album.as_ref().unwrap().target,
            Some(BrowseTarget::Album("MPREb_K8qWMWVqXGi".into()))
        );
        assert!(t.set_video_id.is_some() && !t.artists.is_empty());
        assert_eq!(t.like, Some(Rating::Indifferent));
        // Every row here links a music video; the album still lists songs.
        assert_eq!(t.kind, TrackKind::Song, "{}", t.title);
    }
}

#[test]
fn single() {
    let single = page("single", BrowseTarget::Album("MPREb_X1DQ1j0PPrX".into()));
    let Some(Header::Detail { subtitle, .. }) = &single.header else {
        panic!()
    };
    assert!(subtitle.iter().any(|l| l.text == "Single"));
    assert_eq!(tracks(&single.sections[0]).len(), 3);
    assert_sections(&single.sections);
}

#[test]
fn artist() {
    let artist = page(
        "artist",
        BrowseTarget::Artist("UCRr1xG_2WIDs18a6cIiCxeA".into()),
    );
    let Some(Header::Artist {
        name,
        description,
        thumbnails,
        channel_id,
        subscribers,
        shuffle_playlist_id,
        radio_playlist_id,
        monthly_listeners,
        subscribed,
    }) = &artist.header
    else {
        panic!("artist header: {:?}", artist.header);
    };
    assert_eq!(name, "Daft Punk");
    assert!(description.is_some() && !thumbnails.is_empty() && subscribers.is_some());
    assert!(channel_id.as_deref().is_some_and(|c| c.starts_with("UC")));
    assert_eq!(*subscribed, Some(false));
    assert!(
        shuffle_playlist_id.is_some() && radio_playlist_id.is_some() && monthly_listeners.is_some()
    );

    assert!(artist.sections.len() >= 6);
    assert_sections(&artist.sections);
    let top = titled(&artist.sections, "Top songs");
    assert_eq!(top.layout, SectionLayout::List);
    assert!(matches!(top.more, Some(BrowseTarget::Playlist(_))));
    assert!(
        tracks(top)
            .iter()
            .all(|t| t.album.is_some() && t.plays.is_some())
    );
    let singles = titled(&artist.sections, "Singles & EPs");
    assert!(matches!(
        singles.more,
        Some(BrowseTarget::ArtistShelf { .. })
    ));
    assert!(
        singles
            .items
            .iter()
            .all(|i| matches!(i, Item::Album { year: Some(_), .. }))
    );
    let fans = titled(&artist.sections, "Fans might also like");
    assert!(fans.items.iter().all(|i| matches!(i, Item::Artist { .. })));
}

#[test]
fn artist_shelf() {
    let shelf = page(
        "artist_singles",
        BrowseTarget::ArtistShelf {
            browse_id: "MPADUCRr1xG_2WIDs18a6cIiCxeA".into(),
            params: "ggMIegYIAhoCAQI=".into(),
        },
    );
    assert_eq!(shelf.sections.len(), 1);
    assert_eq!(shelf.sections[0].layout, SectionLayout::Grid);
    assert!(shelf.sections[0].items.len() >= 10);
    assert_sections(&shelf.sections);
}

#[test]
fn chart_playlist() {
    let playlist = page(
        "playlist",
        BrowseTarget::Playlist("PL4fGSI1pDJn6O1LS0XSdF3RyO0Rq_LDeI".into()),
    );
    let Some(Header::Detail {
        title,
        editable,
        second_subtitle,
        ..
    }) = &playlist.header
    else {
        panic!()
    };
    assert_eq!(title, "Top 100 Songs United States");
    assert!(!editable && second_subtitle.is_some());
    let rows = tracks(&playlist.sections[0]);
    assert_eq!(rows.len(), 100);
    for t in rows {
        assert_track(t);
        assert!(t.set_video_id.is_some() && t.duration_ms.is_some() && !t.artists.is_empty());
    }
}

#[test]
fn large_playlist_and_its_continuation() {
    let playlist = page(
        "playlist_large",
        BrowseTarget::Playlist("PL0GvsLQil0MmYC96KEs_7dTNsLm1PS6JX".into()),
    );
    let list = &playlist.sections[0];
    assert_eq!(tracks(list).len(), 100);
    assert!(list.continuation.is_some(), "track continuation");
    assert!(
        playlist.continuation.is_some(),
        "related sections continuation"
    );

    let more = parse::page::parse_continuation(&load("playlist_continuation")).unwrap();
    assert_eq!(more.items.len(), 100);
    more.items.iter().for_each(assert_item);
    assert!(more.continuation.is_some());
}

#[test]
fn podcast() {
    let podcast = page(
        "podcast",
        BrowseTarget::Podcast("MPSPPLC3I8Rb5dhRCNbmAH-2QAMQe_YWH-2_VB".into()),
    );
    let Some(Header::Detail {
        title, description, ..
    }) = &podcast.header
    else {
        panic!()
    };
    assert!(!title.is_empty() && description.is_some());
    let episodes = tracks(&podcast.sections[0]);
    assert!(episodes.len() >= 10);
    for e in episodes {
        assert_track(e);
        assert_eq!(e.kind, TrackKind::Episode);
        assert!(e.duration_ms.is_some());
    }
    assert!(podcast.sections[0].continuation.is_some());
}

#[test]
fn episode() {
    let episode = page("episode", BrowseTarget::Episode("MPED6VpKALzagxo".into()));
    let Some(Header::Detail {
        subtitle,
        description,
        ..
    }) = &episode.header
    else {
        panic!()
    };
    assert!(
        subtitle
            .iter()
            .any(|l| matches!(l.target, Some(BrowseTarget::Podcast(_))))
    );
    assert!(description.is_some());
}

#[test]
fn related() {
    let related = page("related", raw("MPTRt_9x5Za81pHy6"));
    assert_sections(&related.sections);
    assert_eq!(
        titled(&related.sections, "You might also like").layout,
        SectionLayout::TrackGrid
    );
    assert!(
        titled(&related.sections, "Similar artists")
            .items
            .iter()
            .all(|i| matches!(i, Item::Artist { .. }))
    );
}

#[test]
fn search_all() {
    let results = parse::search::parse_search("daft punk", None, &load("search_all")).unwrap();
    assert_sections(&results.sections);
    let top = &results.sections[0];
    assert_eq!(top.layout, SectionLayout::Hero);
    assert!(matches!(&top.items[0], Item::Artist { name, .. } if name == "Daft Punk"));
    assert!(tracks(top).iter().all(|t| !t.artists.is_empty()));
    let all: Vec<&Item> = results.sections.iter().flat_map(|s| &s.items).collect();
    assert!(all.iter().any(|i| matches!(i, Item::Album { .. })));
    assert!(all.iter().any(|i| matches!(i, Item::Playlist { .. })));
    assert!(all.iter().any(|i| matches!(i, Item::Podcast { .. })));
    assert!(
        all.iter()
            .any(|i| matches!(i, Item::Track(t) if t.kind == TrackKind::Video))
    );
    assert!(
        all.iter()
            .any(|i| matches!(i, Item::Track(t) if t.kind == TrackKind::Episode))
    );
}

#[test]
fn search_songs() {
    let results = parse::search::parse_search(
        "daft punk",
        Some(SearchFilter::Songs),
        &load("search_songs"),
    )
    .unwrap();
    assert_sections(&results.sections);
    let songs = titled(&results.sections, "Songs");
    assert!(songs.items.len() >= 10);
    assert!(
        tracks(songs)
            .iter()
            .all(|t| t.kind == TrackKind::Song && t.duration_ms.is_some() && t.album.is_some())
    );
    let token = results.continuation.expect("filtered search continues");
    assert!(token.0.starts_with("search:"));
}

#[test]
fn search_albums_and_artists() {
    let albums = parse::search::parse_search(
        "daft punk",
        Some(SearchFilter::Albums),
        &load("search_albums"),
    )
    .unwrap();
    let items = &titled(&albums.sections, "Albums").items;
    assert!(items.iter().all(|i| matches!(
        i,
        Item::Album {
            album_type: Some(_),
            year: Some(_),
            ..
        }
    )));
    assert_sections(&albums.sections);

    let artists = parse::search::parse_search(
        "daft punk",
        Some(SearchFilter::Artists),
        &load("search_artists"),
    )
    .unwrap();
    assert!(
        titled(&artists.sections, "Artists")
            .items
            .iter()
            .all(|i| matches!(i, Item::Artist { .. }))
    );
    assert_sections(&artists.sections);
}

#[test]
fn search_continuation() {
    let more = parse::page::parse_continuation(&load("search_continuation")).unwrap();
    assert!(more.items.len() >= 10);
    more.items.iter().for_each(assert_item);
    assert!(more.continuation.is_some());
}

#[test]
fn suggestions() {
    let suggestions = parse::suggestions::parse_suggestions(&load("suggestions"));
    let queries = suggestions
        .iter()
        .filter(|s| matches!(s, Suggestion::Query { .. }))
        .count();
    let items = suggestions
        .iter()
        .filter(|s| matches!(s, Suggestion::Item(_)))
        .count();
    assert!(queries >= 3 && items >= 3);
    assert!(
        matches!(&suggestions[0], Suggestion::Query { text, .. } if text.starts_with("daft p"))
    );
}

#[test]
fn next_radio() {
    let next = parse::next::parse_next(&load("next_radio")).unwrap();
    assert_eq!(next.tracks.len(), 50);
    next.tracks.iter().for_each(assert_track);
    assert!(
        next.tracks
            .iter()
            .all(|t| !t.artists.is_empty() && t.duration_ms.is_some())
    );
    assert_eq!(next.playlist_id.as_deref(), Some("RDAMVMIluRBvnYMoY"));
    // The rating of the requested track sits in the player's action bar; the
    // queue rows have none.
    assert_eq!(next.like, Some(Rating::Indifferent));
    assert!(next.tracks.iter().all(|t| t.like.is_none()));
    assert!(
        next.lyrics_browse_id
            .as_deref()
            .is_some_and(|id| id.starts_with("MPLY"))
    );
    assert!(
        next.related_browse_id
            .as_deref()
            .is_some_and(|id| id.starts_with("MPTR"))
    );
    assert!(next.continuation.is_some());
}

#[test]
fn lyrics_from_web_client() {
    let lyrics = parse::lyrics::parse_plain(&load("lyrics")).unwrap();
    assert!(!lyrics.synced);
    assert_eq!(lyrics.source.as_deref(), Some("LyricFind"));
    assert!(lyrics.lines.len() > 10 && lyrics.lines.iter().any(|l| l.text.contains("music")));
}

#[test]
fn lyrics_from_android_client() {
    // This track only has unsynced lyrics; the synced path is covered by a
    // unit test in parse::lyrics.
    let lyrics = parse::lyrics::parse_timed(&load("lyrics_android")).unwrap();
    assert!(!lyrics.synced);
    assert!(lyrics.lines.len() > 10 && lyrics.lines.iter().all(|l| l.start_ms == 0));
}

#[test]
fn player_tracking() {
    let tracking = parse::player::parse_player("IluRBvnYMoY", &load("player")).unwrap();
    assert!(
        tracking
            .playback_url
            .starts_with("https://s.youtube.com/api/stats/playback")
    );
    assert!(
        tracking
            .watchtime_url
            .is_some_and(|u| u.contains("watchtime"))
    );
    assert!(tracking.loudness_db.is_some());
}

#[test]
fn anonymous_player_has_no_premium_audio() {
    assert!(!parse::player::has_premium_audio(&load("player")));
}

#[test]
fn unknown_renderers_are_skipped() {
    let json = serde_json::json!({"contents": {"singleColumnBrowseResultsRenderer": {"tabs": [{"tabRenderer": {"content": {
    "sectionListRenderer": {"contents": [
        {"somethingNewShelfRenderer": {"contents": []}},
        {"musicCarouselShelfRenderer": {"header": {"musicCarouselShelfBasicHeaderRenderer": {"title": {"runs": [{"text": "Shelf"}]}}},
            "contents": [{"futureItemRenderer": {}}, {"musicTwoRowItemRenderer": {
                "title": {"runs": [{"text": "An album"}]},
                "navigationEndpoint": {"browseEndpoint": {"browseId": "MPREb_abc"}},
                "thumbnailRenderer": {"musicThumbnailRenderer": {"thumbnail": {"thumbnails": [{"url": "https://x/y.jpg", "width": 226, "height": 226}]}}}
            }}]}}
    ]}}}}]}}});
    let page = parse::page::parse_page(BrowseTarget::Home, &json).unwrap();
    assert_eq!(page.sections.len(), 1);
    assert_eq!(page.sections[0].items.len(), 1);
}

#[test]
fn missing_structure_is_a_parse_error() {
    let err = parse::page::parse_page(BrowseTarget::Home, &serde_json::json!({"contents": {}}))
        .unwrap_err();
    assert!(matches!(err, ApiError::Parse(path) if path.contains("contents.")));
    let err = parse::next::parse_next(&serde_json::json!({})).unwrap_err();
    assert!(matches!(err, ApiError::Parse(_)));
}

// Pages that need a signed-in session. Their fixtures hold the account's
// library, so they live in the gitignored fixtures/private/ and are recorded
// with `FORMALMUSIC_RECORD=1 FORMALMUSIC_COOKIES=<file> cargo test
// --test live record_signed_in_fixtures -- --ignored`. Without them these
// tests skip, so a fresh clone's `-- --ignored` run only exercises the live
// tests.

fn recorded(name: &str) -> Option<Value> {
    let path = format!(
        "{}/fixtures/private/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let Ok(raw) = std::fs::read_to_string(&path) else {
        eprintln!("skipping: {path} has not been recorded");
        return None;
    };
    Some(serde_json::from_str(&raw).unwrap())
}

fn signed_in_page(name: &str, target: BrowseTarget) -> Option<Page> {
    let json = recorded(name)?;
    Some(parse::page::parse_page(target, &json).unwrap_or_else(|e| panic!("{name}: {e}")))
}

fn library(name: &str, tab: LibraryTab) -> Option<Page> {
    signed_in_page(name, BrowseTarget::Library(tab))
}

fn all_tracks(p: &Page) -> Vec<&Track> {
    p.sections.iter().flat_map(tracks).collect()
}

#[test]
#[ignore = "needs fixtures/private/library_playlists.json (FEmusic_liked_playlists, signed in)"]
fn library_playlists() {
    let Some(p) = library("library_playlists", LibraryTab::Playlists) else {
        return;
    };
    assert_sections(&p.sections);
    assert_eq!(p.sections[0].layout, SectionLayout::Grid);
    assert!(
        p.sections[0]
            .items
            .iter()
            .all(|i| matches!(i, Item::Playlist { .. }))
    );
}

#[test]
#[ignore = "needs fixtures/private/library_songs.json (FEmusic_liked_videos, signed in)"]
fn library_songs() {
    let Some(p) = library("library_songs", LibraryTab::Songs) else {
        return;
    };
    assert_sections(&p.sections);
    let songs = all_tracks(&p);
    assert!(!songs.is_empty());
    assert!(
        songs
            .iter()
            .all(|t| !t.artists.is_empty() && t.duration_ms.is_some())
    );
    assert!(p.sections[0].continuation.is_some() || songs.len() < 25);
}

#[test]
#[ignore = "needs fixtures/private/library_albums.json (FEmusic_liked_albums, signed in)"]
fn library_albums() {
    let Some(p) = library("library_albums", LibraryTab::Albums) else {
        return;
    };
    assert_sections(&p.sections);
    assert!(p.sections[0].items.iter().all(|i| matches!(
        i,
        Item::Album {
            playlist_id: Some(_),
            ..
        }
    )));
}

#[test]
#[ignore = "needs fixtures/private/library_artists.json (FEmusic_library_corpus_track_artists, signed in)"]
fn library_artists() {
    let Some(p) = library("library_artists", LibraryTab::Artists) else {
        return;
    };
    assert_sections(&p.sections);
    assert!(!p.sections[0].items.is_empty());
    assert!(p.sections[0].items.iter().all(|i| matches!(
        i,
        Item::Artist {
            subtitle: Some(_),
            ..
        }
    )));
}

#[test]
#[ignore = "needs fixtures/private/library_subscriptions.json (FEmusic_library_corpus_artists, signed in)"]
fn library_subscriptions() {
    let Some(p) = library("library_subscriptions", LibraryTab::Subscriptions) else {
        return;
    };
    assert_sections(&p.sections);
    assert!(!p.sections[0].items.is_empty());
    assert!(
        p.sections[0]
            .items
            .iter()
            .all(|i| matches!(i, Item::Artist { .. }))
    );
}

#[test]
#[ignore = "needs fixtures/private/library_podcasts.json (FEmusic_library_non_music_audio_list, signed in)"]
fn library_podcasts() {
    let Some(p) = library("library_podcasts", LibraryTab::Podcasts) else {
        return;
    };
    assert_sections(&p.sections);
}

#[test]
#[ignore = "needs fixtures/private/library_uploads.json (FEmusic_library_privately_owned_tracks, signed in)"]
fn library_uploads() {
    // An account without uploads gets only a message, so no sections at all.
    let Some(p) = library("library_uploads", LibraryTab::Uploads) else {
        return;
    };
    assert!(all_tracks(&p).iter().all(|t| t.kind == TrackKind::Upload));
}

#[test]
#[ignore = "needs fixtures/private/liked_songs.json (VLLM, signed in)"]
fn liked_songs() {
    let Some(p) = library("liked_songs", LibraryTab::LikedSongs) else {
        return;
    };
    assert!(matches!(
        &p.header,
        Some(Header::Detail {
            editable: false,
            ..
        })
    ));
    let songs = all_tracks(&p);
    assert!(songs.len() >= 10);
    songs.iter().for_each(|t| assert_track(t));
    assert!(
        songs
            .iter()
            .all(|t| t.like == Some(Rating::Like) && t.set_video_id.is_some())
    );
}

#[test]
#[ignore = "needs fixtures/private/owned_playlist.json (a playlist you own, signed in)"]
fn owned_playlist() {
    let Some(p) = signed_in_page("owned_playlist", BrowseTarget::Playlist("PL".into())) else {
        return;
    };
    let Some(Header::Detail {
        editable,
        privacy,
        playlist_id,
        ..
    }) = &p.header
    else {
        panic!("{:?}", p.header)
    };
    assert!(*editable && privacy.is_some() && playlist_id.is_some());
    let rows = all_tracks(&p);
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|t| t.set_video_id.is_some()));
}

#[test]
#[ignore = "needs fixtures/private/history.json (FEmusic_history, signed in)"]
fn history() {
    let Some(p) = signed_in_page("history", BrowseTarget::History) else {
        return;
    };
    assert_eq!(
        p.header,
        Some(Header::Title {
            title: "History".into()
        })
    );
    assert_sections(&p.sections);
    assert!(p.sections.iter().all(|s| s.title.is_some()));
    assert!(all_tracks(&p).iter().all(|t| t.feedback_token.is_some()));
}

#[test]
#[ignore = "needs fixtures/private/accounts_list.json (account/accounts_list, signed in)"]
fn accounts_list() {
    let Some(json) = recorded("accounts_list") else {
        return;
    };
    let accounts = parse::account::parse_accounts(&json).unwrap();
    assert_eq!(accounts.iter().filter(|a| a.selected).count(), 1);
    assert!(
        accounts
            .iter()
            .all(|a| !a.name.is_empty() && !a.thumbnails.is_empty())
    );
}

#[test]
#[ignore = "needs fixtures/private/account_menu.json (account/account_menu, signed in)"]
fn account_menu() {
    let Some(json) = recorded("account_menu") else {
        return;
    };
    let session = parse::account::parse_session(&json).unwrap();
    let account = session.account.unwrap();
    assert!(!account.name.is_empty() && !account.thumbnails.is_empty());
}

#[test]
#[ignore = "needs fixtures/private/home.json (FEmusic_home, signed in)"]
fn signed_in_home() {
    let Some(home) = signed_in_page("home", BrowseTarget::Home) else {
        return;
    };
    assert!(!home.chips.is_empty() && home.continuation.is_some());
    assert_sections(&home.sections);
    assert!(home.sections.iter().all(|s| s.title.is_some()));
}

#[test]
#[ignore = "needs fixtures/private/player.json (player, signed in)"]
fn signed_in_player() {
    let Some(json) = recorded("player") else {
        return;
    };
    let tracking = parse::player::parse_player("IluRBvnYMoY", &json).unwrap();
    assert!(tracking.playback_url.starts_with("https://") && tracking.loudness_db.is_some());
}
