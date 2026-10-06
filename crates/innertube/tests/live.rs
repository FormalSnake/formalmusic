//! Requests against the real API. Ignored by default; the weekly
//! maintenance run executes them with
//! `cargo test -p formalmusic-innertube -- --ignored` to catch YouTube
//! changing shape before users do.
//!
//! With `FORMALMUSIC_COOKIES` pointing at a Netscape cookie file, the
//! signed-in checks run too, and `FORMALMUSIC_RECORD=1` re-records the
//! signed-in fixtures into `fixtures/private/` (gitignored: they hold the
//! account's library).

use formalmusic_innertube::Client;
use formalmusic_innertube::api::*;
use serde_json::json;

fn client() -> Client {
    Client::anonymous().unwrap()
}

fn signed_in() -> Option<Client> {
    let Ok(path) = std::env::var("FORMALMUSIC_COOKIES") else {
        eprintln!("skipping: FORMALMUSIC_COOKIES is not set");
        return None;
    };
    Some(Client::from_cookie_file(path, None).unwrap())
}

fn all_items(sections: &[Section]) -> impl Iterator<Item = &Item> {
    sections.iter().flat_map(|s| &s.items)
}

#[tokio::test]
#[ignore = "hits music.youtube.com"]
async fn live_home() {
    let client = client();
    let home = client.browse(BrowseTarget::Home).await.unwrap();
    assert!(!home.chips.is_empty(), "home lost its chips");
    assert!(home.sections.len() >= 2);
    assert!(all_items(&home.sections).count() >= 10);
    // Continuation tokens are bound to the visitor id, so the same client
    // has to follow them.
    let more = client
        .continuation(&home.continuation.expect("home continuation"))
        .await
        .unwrap();
    assert!(!more.sections.is_empty());
}

#[tokio::test]
#[ignore = "hits music.youtube.com"]
async fn live_search() {
    let client = client();
    let all = client.search("daft punk", None).await.unwrap();
    assert!(
        all_items(&all.sections)
            .any(|i| matches!(i, Item::Artist { name, .. } if name == "Daft Punk"))
    );

    let songs = client
        .search("daft punk", Some(SearchFilter::Songs))
        .await
        .unwrap();
    let tracks = all_items(&songs.sections)
        .filter(|i| matches!(i, Item::Track(t) if t.video_id.len() == 11))
        .count();
    assert!(tracks >= 10);
    let more = client
        .continuation(&songs.continuation.expect("songs continuation"))
        .await
        .unwrap();
    assert!(!more.items.is_empty());

    let suggestions = client.suggestions("daft p").await.unwrap();
    assert!(
        suggestions
            .iter()
            .any(|s| matches!(s, Suggestion::Query { .. }))
    );
}

#[tokio::test]
#[ignore = "hits music.youtube.com"]
async fn live_album() {
    let album = client()
        .browse(BrowseTarget::Album("MPREb_K8qWMWVqXGi".into()))
        .await
        .unwrap();
    assert!(
        matches!(&album.header, Some(Header::Detail { title, playlist_id: Some(_), .. }) if title == "Random Access Memories")
    );
    let tracks = album.sections[0]
        .items
        .iter()
        .filter(|i| matches!(i, Item::Track(t) if t.duration_ms.is_some()))
        .count();
    assert_eq!(tracks, 13);
}

#[tokio::test]
#[ignore = "hits music.youtube.com"]
async fn live_next() {
    let client = client();
    let radio = client.radio("IluRBvnYMoY").await.unwrap();
    assert!(radio.tracks.len() >= 20);
    assert!(radio.related_browse_id.is_some());
    let more = client
        .next_continuation(
            radio.playlist_id.as_deref().unwrap(),
            &radio.continuation.expect("radio continuation"),
        )
        .await
        .unwrap();
    assert!(!more.tracks.is_empty());

    let related = client
        .related(radio.related_browse_id.as_deref().unwrap())
        .await
        .unwrap();
    assert!(!related.sections.is_empty());
    let lyrics = client.lyrics("IluRBvnYMoY").await.unwrap().expect("lyrics");
    assert!(lyrics.lines.len() > 10);
}

#[tokio::test]
#[ignore = "hits music.youtube.com"]
async fn live_playback_tracking() {
    let tracking = client().playback_tracking("IluRBvnYMoY").await.unwrap();
    assert!(tracking.playback_url.starts_with("https://"));
    assert!(tracking.loudness_db.is_some());
}

#[tokio::test]
#[ignore = "hits music.youtube.com signed in"]
async fn live_signed_in_pages() {
    let Some(client) = signed_in() else { return };
    let session = client.session().await.unwrap();
    assert!(session.signed_in && session.account.is_some());
    assert!(client.accounts().await.unwrap().iter().any(|a| a.selected));
    let home = client.browse(BrowseTarget::Home).await.unwrap();
    assert!(home.sections.len() >= 2);
    let playlists = client
        .browse(BrowseTarget::Library(LibraryTab::Playlists))
        .await
        .unwrap();
    assert!(all_items(&playlists.sections).any(|i| matches!(i, Item::Playlist { .. })));
    let liked = client
        .browse(BrowseTarget::Library(LibraryTab::LikedSongs))
        .await
        .unwrap();
    assert!(
        all_items(&liked.sections)
            .all(|i| matches!(i, Item::Track(t) if t.like == Some(Rating::Like)))
    );
    let tracking = client.playback_tracking("IluRBvnYMoY").await.unwrap();
    assert!(tracking.playback_url.starts_with("https://"));
}

#[tokio::test]
#[ignore = "writes fixtures/private/ when FORMALMUSIC_RECORD=1"]
async fn record_signed_in_fixtures() {
    if std::env::var("FORMALMUSIC_RECORD").as_deref() != Ok("1") {
        eprintln!("skipping: FORMALMUSIC_RECORD is not 1");
        return;
    }
    let Some(client) = signed_in() else { return };
    let dir = format!("{}/fixtures/private", env!("CARGO_MANIFEST_DIR"));
    std::fs::create_dir_all(&dir).unwrap();
    let save = |name: &str, value: &serde_json::Value| {
        std::fs::write(
            format!("{dir}/{name}.json"),
            serde_json::to_string(value).unwrap(),
        )
        .unwrap();
    };
    let browse = [
        ("home", "FEmusic_home"),
        ("library_playlists", "FEmusic_liked_playlists"),
        ("library_songs", "FEmusic_liked_videos"),
        ("library_albums", "FEmusic_liked_albums"),
        ("library_artists", "FEmusic_library_corpus_track_artists"),
        ("library_subscriptions", "FEmusic_library_corpus_artists"),
        ("library_podcasts", "FEmusic_library_non_music_audio_list"),
        ("library_uploads", "FEmusic_library_privately_owned_tracks"),
        ("liked_songs", "VLLM"),
        ("history", "FEmusic_history"),
    ];
    for (name, browse_id) in browse {
        save(
            name,
            &client
                .raw("browse", json!({ "browseId": browse_id }))
                .await
                .unwrap(),
        );
    }
    save(
        "accounts_list",
        &client
            .raw("account/accounts_list", json!({}))
            .await
            .unwrap(),
    );
    save(
        "account_menu",
        &client.raw("account/account_menu", json!({})).await.unwrap(),
    );
    let player = client.player_body("IluRBvnYMoY").await;
    save("player", &client.raw("player", player).await.unwrap());

    let playlists = client
        .browse(BrowseTarget::Library(LibraryTab::Playlists))
        .await
        .unwrap();
    for item in all_items(&playlists.sections) {
        let Item::Playlist { playlist_id, .. } = item else {
            continue;
        };
        if !playlist_id.starts_with("PL") {
            continue;
        }
        let json = client
            .raw("browse", json!({ "browseId": format!("VL{playlist_id}") }))
            .await
            .unwrap();
        if json
            .to_string()
            .contains("musicEditablePlaylistDetailHeaderRenderer")
        {
            save("owned_playlist", &json);
            break;
        }
    }
}

/// Like `assert!`, but returns an error so the test can still clean up.
macro_rules! check {
    ($cond:expr) => {
        if !$cond {
            return Err(ApiError::Parse(format!(
                "check failed: {}",
                stringify!($cond)
            )));
        }
    };
}

fn playlist_rows(page: &Page) -> Vec<(String, String)> {
    all_items(&page.sections)
        .filter_map(|i| match i {
            Item::Track(t) => Some((t.video_id.clone(), t.set_video_id.clone()?)),
            _ => None,
        })
        .collect()
}

/// Every mutation once, each undone before the test ends, on the account
/// behind `FORMALMUSIC_COOKIES`. Runs only with `FORMALMUSIC_MUTATE=1` so the
/// weekly run does not touch the account unasked.
#[tokio::test]
#[ignore = "changes the signed-in account, then restores it; needs FORMALMUSIC_MUTATE=1"]
async fn live_mutations_round_trip() {
    if std::env::var("FORMALMUSIC_MUTATE").as_deref() != Ok("1") {
        eprintln!("skipping: FORMALMUSIC_MUTATE is not 1");
        return;
    }
    let Some(client) = signed_in() else { return };

    // Like, on a track that is not liked yet.
    let mut video = None;
    for candidate in ["IluRBvnYMoY", "zhl-Cs1-sG4", "ajGKWk0auOc"] {
        if client.next(Some(candidate), None).await.unwrap().like == Some(Rating::Indifferent) {
            video = Some(candidate);
            break;
        }
    }
    let video = video.expect("every candidate track is already rated");
    let track = RateTarget::Track {
        video_id: video.into(),
    };
    let like_settles_at = |want: Rating| {
        let client = client.clone();
        async move {
            for _ in 0..10 {
                if client.next(Some(video), None).await.unwrap().like == Some(want) {
                    return true;
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            false
        }
    };
    client.rate(&track, Rating::Like).await.unwrap();
    let liked = like_settles_at(Rating::Like).await;
    client.rate(&track, Rating::Indifferent).await.unwrap();
    assert!(liked, "like never showed up");
    // Ratings take a moment to read back.
    assert!(
        like_settles_at(Rating::Indifferent).await,
        "like was not removed"
    );

    // Subscribe, on an artist not subscribed to yet.
    let mut channel = None;
    for artist in [
        "UCcrR-Or3AH2RKJqOntiD_Tw",
        "UCjct1Am9Wi9tB076rIBrlMg",
        "UC3V5kzHK2r4rbbXlf4OpFmw",
    ] {
        let page = client
            .browse(BrowseTarget::Artist(artist.into()))
            .await
            .unwrap();
        if let Some(Header::Artist {
            channel_id: Some(id),
            subscribed: Some(false),
            ..
        }) = page.header
        {
            channel = Some((artist, id));
            break;
        }
    }
    let (artist, channel_id) = channel.expect("already subscribed to every candidate artist");
    let subscribed_state = |client: Client| async move {
        match client
            .browse(BrowseTarget::Artist(artist.into()))
            .await
            .unwrap()
            .header
        {
            Some(Header::Artist { subscribed, .. }) => subscribed,
            _ => None,
        }
    };
    let subscription_settles_at = |want: bool| {
        let client = client.clone();
        async move {
            for _ in 0..10 {
                if subscribed_state(client.clone()).await == Some(want) {
                    return true;
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            false
        }
    };
    client.set_subscribed(&channel_id, true).await.unwrap();
    let subscribed = subscription_settles_at(true).await;
    client.set_subscribed(&channel_id, false).await.unwrap();
    assert!(subscribed, "subscription never showed up");
    assert!(
        subscription_settles_at(false).await,
        "subscription was not removed"
    );

    // A private playlist, edited every way, then deleted.
    let id = client
        .create_playlist(
            "FormalMusic test",
            "",
            Privacy::Private,
            &["IluRBvnYMoY".into()],
        )
        .await
        .unwrap();
    let edits = async {
        let browse = || client.browse(BrowseTarget::Playlist(id.clone()));
        client
            .edit_playlist(&id, &[PlaylistEdit::Add { video_id: "zhl-Cs1-sG4".into() }, PlaylistEdit::Add { video_id: "ajGKWk0auOc".into() }])
            .await?;
        let page = browse().await?;
        check!(matches!(page.header, Some(Header::Detail { editable: true, privacy: Some(Privacy::Private), .. })));
        let rows = playlist_rows(&page);
        check!(rows.len() == 3);

        let (last_video, last_set) = rows[2].clone();
        client
            .edit_playlist(&id, &[PlaylistEdit::Move { set_video_id: last_set, before_set_video_id: Some(rows[0].1.clone()) }])
            .await?;
        let moved = playlist_rows(&browse().await?);
        check!(moved.len() == 3 && moved[0].0 == last_video);

        let (removed_video, removed_set) = moved[1].clone();
        client
            .edit_playlist(&id, &[PlaylistEdit::Remove { video_id: removed_video.clone(), set_video_id: removed_set }])
            .await?;
        let remaining = playlist_rows(&browse().await?);
        check!(remaining.len() == 2);
        check!(remaining.iter().all(|(v, _)| *v != removed_video));

        client.edit_playlist(&id, &[PlaylistEdit::Rename { title: "FormalMusic test renamed".into() }]).await?;
        let mut renamed = false;
        for _ in 0..10 {
            let page = browse().await?;
            if matches!(page.header, Some(Header::Detail { title, .. }) if title == "FormalMusic test renamed") {
                renamed = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        check!(renamed);
        Ok::<_, ApiError>(())
    }
    .await;
    client.delete_playlist(&id).await.unwrap();
    edits.unwrap();
    let library = client
        .browse(BrowseTarget::Library(LibraryTab::Playlists))
        .await
        .unwrap();
    assert!(
        !all_items(&library.sections)
            .any(|i| matches!(i, Item::Playlist { playlist_id, .. } if *playlist_id == id))
    );
}

/// `fixtures/next_counterpart.json`: the `next` row of an album track with
/// its music video as the counterpart. Only signed-in sessions get these
/// wrapper rows, so this needs `FORMALMUSIC_COOKIES` too.
#[tokio::test]
#[ignore = "writes fixtures/next_counterpart.json when FORMALMUSIC_RECORD=1"]
async fn record_counterpart_fixture() {
    if std::env::var("FORMALMUSIC_RECORD").as_deref() != Ok("1") {
        eprintln!("skipping: FORMALMUSIC_RECORD is not 1");
        return;
    }
    let Some(client) = signed_in() else { return };
    let body = json!({
        "videoId": "J7p4bzqLvCw",
        "isAudioOnly": true,
        "enablePersistentPlaylistPanel": true,
    });
    let mut json = client.raw("next", body).await.unwrap();
    // The visitor data and token jar identify the session.
    json.as_object_mut().unwrap().remove("responseContext");
    std::fs::write(
        format!(
            "{}/fixtures/next_counterpart.json",
            env!("CARGO_MANIFEST_DIR")
        ),
        serde_json::to_string(&json).unwrap(),
    )
    .unwrap();
}
