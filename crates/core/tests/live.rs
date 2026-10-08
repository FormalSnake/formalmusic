//! Every screen's data path against a running kopuzd. Run with
//! `FORMALMUSIC_SOCKET=<socket> cargo test -p formalmusic-core --test live -- --ignored --test-threads 1`;
//! the signed-in checks skip themselves unless the source is signed in.

use std::time::Duration;

use formalmusic_core::backend::{Backend, ConnectionStatus, Event};
use formalmusic_core::kopuz::{KopuzBackend, socket_path};
use formalmusic_core::model::*;

type Events = tokio::sync::mpsc::UnboundedReceiver<Event>;

async fn connected() -> (KopuzBackend, SessionInfo) {
    let (backend, session, _) = listening().await;
    (backend, session)
}

async fn listening() -> (KopuzBackend, SessionInfo, Events) {
    let backend = KopuzBackend::new(socket_path());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    backend.start(tx);
    let mut session = None;
    let deadline = tokio::time::sleep(Duration::from_secs(20));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            event = rx.recv() => match event {
                Some(Event::Session(s)) => session = Some(s),
                Some(Event::Connection { status: ConnectionStatus::Online, .. }) => break,
                Some(_) => {}
                None => panic!("the backend stopped"),
            },
            _ = &mut deadline => panic!("kopuzd never answered at {}", socket_path().display()),
        }
    }
    (backend, session.expect("a session before online"), rx)
}

/// The first queue that starts with `first` and holds at least `len` tracks;
/// events about the queue before it are passed over.
async fn queue_of(first: &str, len: usize, rx: &mut Events) -> QueueState {
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while std::time::Instant::now() < deadline {
        if let Ok(Some(Event::Queue(queue))) =
            tokio::time::timeout(Duration::from_secs(5), rx.recv()).await
            && queue.tracks.first().is_some_and(|track| track.key == first)
            && queue.tracks.len() >= len
        {
            return queue;
        }
    }
    panic!("the queue never started with {first} and reached {len} tracks");
}

fn tracks(page: &Page) -> Vec<&Track> {
    page.sections
        .iter()
        .flat_map(|section| &section.items)
        .filter_map(|item| match item {
            Item::Track(track) => Some(track),
            _ => None,
        })
        .collect()
}

fn first(page: &Page, pick: impl Fn(&Item) -> bool) -> Option<&Item> {
    page.sections
        .iter()
        .flat_map(|section| &section.items)
        .find(|item| pick(item))
}

#[tokio::test]
#[ignore = "needs a running kopuzd"]
async fn browse_pages_have_their_shapes() {
    let (backend, _) = connected().await;
    let home = backend.browse(&BrowseTarget::Home).await.unwrap();
    assert!(home.sections.len() >= 3, "{} shelves", home.sections.len());
    assert!(!home.chips.is_empty(), "home has its mood chips");
    let chip = home.chips[0].id.clone();
    let filtered = backend.browse(&BrowseTarget::HomeChip(chip)).await.unwrap();
    assert!(filtered.chips.iter().any(|chip| chip.selected));
    let token = home.continuation.clone().expect("home keeps going");
    let more = backend
        .more(&BrowseTarget::Home, None, &token)
        .await
        .unwrap();
    assert!(!more.sections.is_empty());

    let explore = backend.browse(&BrowseTarget::Explore).await.unwrap();
    let shortcuts: Vec<_> = explore
        .sections
        .iter()
        .flat_map(|s| &s.items)
        .filter_map(|item| match item {
            Item::Shortcut { target, .. } => Some(target.clone()),
            _ => None,
        })
        .collect();
    assert!(
        shortcuts.contains(&BrowseTarget::NewReleases),
        "{shortcuts:?}"
    );
    for target in [
        BrowseTarget::NewReleases,
        BrowseTarget::Charts,
        BrowseTarget::MoodsAndGenres,
    ] {
        let page = backend.browse(&target).await.unwrap();
        assert!(!page.sections.is_empty(), "{target:?} is empty");
    }
    let moods = backend.browse(&BrowseTarget::MoodsAndGenres).await.unwrap();
    let Some(Item::Mood { id, color, .. }) = first(&moods, |i| matches!(i, Item::Mood { .. }))
    else {
        panic!("no mood tiles");
    };
    assert!(color.is_some());
    let mood = backend
        .browse(&BrowseTarget::Mood(id.clone()))
        .await
        .unwrap();
    assert!(!mood.sections.is_empty());
}

#[tokio::test]
#[ignore = "needs a running kopuzd"]
async fn album_artist_playlist_and_podcast_pages() {
    let (backend, _) = connected().await;
    let album = backend
        .browse(&BrowseTarget::Album("MPREb_K8qWMWVqXGi".into()))
        .await
        .unwrap();
    let Some(Header::Detail {
        subtitle,
        second_subtitle,
        play,
        ..
    }) = &album.header
    else {
        panic!("an album has a detail header");
    };
    assert!(subtitle[0].target.is_some(), "the artist links");
    assert!(second_subtitle.as_deref().unwrap().contains("songs"));
    assert!(play.is_some());
    assert_eq!(tracks(&album).len(), 13);

    let artist = backend
        .browse(&BrowseTarget::Artist("UCRr1xG_2WIDs18a6cIiCxeA".into()))
        .await
        .unwrap();
    let Some(Header::Artist { shuffle, radio, .. }) = &artist.header else {
        panic!("an artist has a banner");
    };
    assert!(shuffle.is_some() && radio.is_some());
    assert!(artist.sections.iter().any(|s| s.more.is_some()));

    let charts = backend
        .browse(&BrowseTarget::Playlist(
            "VLPL4fGSI1pDJn6sMPCoD7PdSlEgyUylgxuT".into(),
        ))
        .await
        .unwrap();
    assert!(tracks(&charts).len() >= 50);

    let podcasts = backend
        .browse(&BrowseTarget::Page("FEmusic_home?ggMGSgQIDBAD".into()))
        .await
        .unwrap();
    let Some(Item::Podcast { browse_id, .. }) =
        first(&podcasts, |i| matches!(i, Item::Podcast { .. }))
    else {
        panic!("no podcasts");
    };
    let podcast = backend
        .browse(&BrowseTarget::Podcast(browse_id.clone()))
        .await
        .unwrap();
    let episodes = tracks(&podcast);
    assert!(!episodes.is_empty());
    assert!(episodes.iter().all(|t| t.kind == TrackKind::Episode));
}

#[tokio::test]
#[ignore = "needs a running kopuzd"]
async fn search_suggestions_lyrics_and_related() {
    let (backend, _) = connected().await;
    let all = backend.search("daft punk", None).await.unwrap();
    assert_eq!(all.sections[0].layout, SectionLayout::Hero);
    assert!(all.sections[0].shuffle.is_some(), "the top artist shuffles");
    assert!(
        all.sections
            .iter()
            .any(|s| s.filter == Some(SearchFilter::Songs))
    );
    let songs = backend
        .search("daft punk", Some(SearchFilter::Songs))
        .await
        .unwrap();
    let token = songs.continuation.clone().expect("songs page on");
    let more = backend
        .search_more("daft punk", Some(SearchFilter::Songs), &token)
        .await
        .unwrap();
    assert!(!more.items.is_empty());
    assert!(backend.search_filters().contains(&SearchFilter::Albums));

    let suggestions = backend.suggestions("daft p").await.unwrap();
    assert!(
        suggestions
            .iter()
            .any(|s| matches!(s, Suggestion::Query { .. }))
    );
    assert!(suggestions.iter().any(|s| matches!(s, Suggestion::Item(_))));

    let lyrics = backend
        .lyrics("ZMwJ6R9BE48")
        .await
        .unwrap()
        .expect("lyrics");
    assert!(lyrics.synced && lyrics.lines.len() > 10);
    let related = backend.related("ZMwJ6R9BE48").await.unwrap();
    assert!(!related.sections.is_empty());
    let album = backend
        .share_url(&Item::Album {
            browse_id: "MPREb_K8qWMWVqXGi".into(),
            title: String::new(),
            album_type: None,
            artists: Vec::new(),
            year: None,
            art: None,
            explicit: false,
            actions: Actions::default(),
        })
        .await
        .unwrap();
    assert_eq!(
        album.as_deref(),
        Some("https://music.youtube.com/browse/MPREb_K8qWMWVqXGi")
    );
}

#[tokio::test]
#[ignore = "needs a running kopuzd"]
async fn the_library_of_a_signed_in_account() {
    let (backend, session) = connected().await;
    if !session.signed_in {
        eprintln!("skipped: the source is not signed in");
        return;
    }
    for tab in [
        LibraryTab::Playlists,
        LibraryTab::Songs,
        LibraryTab::Albums,
        LibraryTab::Artists,
        LibraryTab::Subscriptions,
        LibraryTab::LikedSongs,
    ] {
        let page = backend.browse(&BrowseTarget::Library(tab)).await.unwrap();
        assert!(!page.sections.is_empty(), "{tab:?} is empty");
        if tab != LibraryTab::LikedSongs {
            assert!(page.chips.iter().any(|chip| chip.selected), "{tab:?} chips");
        }
    }
    let history = backend.browse(&BrowseTarget::History).await.unwrap();
    assert!(
        tracks(&history)
            .iter()
            .all(|track| track.actions.history_token.is_some())
    );
    let profiles = backend.browser_profiles().await.unwrap();
    println!("{} browsers with a signed-in profile", profiles.len());
    let browsers = backend.browsers().await.unwrap();
    assert!(!browsers.installed.is_empty());
}

/// Plays an album whole, then the rest of a long playlist behind its first
/// page, leaving the player paused and muted as it was asked.
#[tokio::test]
#[ignore = "needs a running kopuzd; plays audio, muted"]
async fn playing_a_page_queues_all_of_it() {
    let (backend, _, mut rx) = listening().await;
    backend
        .control(formalmusic_core::backend::Control::Muted(true))
        .await
        .unwrap();
    let album = BrowseTarget::Album("MPREb_K8qWMWVqXGi".into());
    let opening = tracks(&backend.browse(&album).await.unwrap())[0]
        .key
        .clone();
    backend
        .play(
            PlaySource::Page {
                target: album,
                tracks: Vec::new(),
            },
            2,
            false,
        )
        .await
        .unwrap();
    let queue = queue_of(&opening, 13, &mut rx).await;
    assert_eq!(queue.tracks.len(), 13);

    let mix = BrowseTarget::Playlist("RDTMAK5uy_nilrsVWxrKskY0ZUpVZ3zpB0u4LwWTVJ4".into());
    let first = backend.browse(&mix).await.unwrap();
    let shown: Vec<Track> = tracks(&first).into_iter().cloned().collect();
    assert!(first.sections[0].continuation.is_some(), "the mix pages on");
    backend
        .play(
            PlaySource::Page {
                target: mix,
                tracks: shown.clone(),
            },
            0,
            false,
        )
        .await
        .unwrap();
    let queue = queue_of(&shown[0].key, shown.len() + 1, &mut rx).await;
    let keys = |tracks: &[Track]| tracks.iter().map(|t| t.key.clone()).collect::<Vec<_>>();
    assert_eq!(keys(&queue.tracks[..shown.len()]), keys(&shown));
    backend
        .control(formalmusic_core::backend::Control::Pause)
        .await
        .unwrap();
}

/// Likes, follows, saves and a playlist of its own, each undone again, so
/// the account ends as it started.
#[tokio::test]
#[ignore = "needs a running kopuzd; changes the account and puts it back"]
async fn library_changes_land_and_are_undone() {
    let (backend, session) = connected().await;
    let backend = &backend;
    if !session.signed_in {
        eprintln!("skipped: the source is not signed in");
        return;
    }
    let album_target = BrowseTarget::Album("MPREb_K8qWMWVqXGi".into());
    let album = backend.browse(&album_target).await.unwrap();
    let Some(Header::Detail { actions, .. }) = &album.header else {
        panic!("an album has a detail header");
    };
    let save_ref = actions.save_ref.clone().expect("an album can be saved");
    let was_saved = actions.saved.unwrap_or(false);
    backend.save(&save_ref, !was_saved).await.unwrap();
    let saved = |page: &Page| match &page.header {
        Some(Header::Detail { actions, .. }) => actions.saved,
        _ => None,
    };
    let after = backend.browse(&album_target).await.unwrap();
    backend.save(&save_ref, was_saved).await.unwrap();
    assert_eq!(saved(&after), Some(!was_saved));
    let back = backend.browse(&album_target).await.unwrap();
    assert_eq!(saved(&back), Some(was_saved));

    // A row nobody likes yet: liked, then back to no rating.
    let row = tracks(&album)
        .into_iter()
        .find(|track| track.actions.rating.is_none())
        .expect("an album track that is not liked")
        .clone();
    let rate_ref = row.actions.rate_ref.clone().unwrap();
    backend.rate(&rate_ref, Rating::Like).await.unwrap();
    backend.rate(&rate_ref, Rating::Indifferent).await.unwrap();

    let artist_target = BrowseTarget::Artist("UCvA7ExlBsBUb5XQEYzJC1ww".into());
    let followed = |page: &Page| match &page.header {
        Some(Header::Artist { actions, .. }) => (actions.follow_ref.clone(), actions.followed),
        _ => (None, None),
    };
    let (follow_ref, was) = followed(&backend.browse(&artist_target).await.unwrap());
    let (follow_ref, was) = (follow_ref.expect("an artist can be followed"), was.unwrap());
    // YouTube's artist page takes a few seconds to show a new subscription.
    let (artist, followed) = (&artist_target, &followed);
    let settles = |want: bool| async move {
        for _ in 0..10 {
            if followed(&backend.browse(artist).await.unwrap()).1 == Some(want) {
                return true;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        false
    };
    backend.follow(&follow_ref, !was).await.unwrap();
    let flipped = settles(!was).await;
    backend.follow(&follow_ref, was).await.unwrap();
    assert!(flipped, "the follow never showed");
    assert!(settles(was).await, "the unfollow never showed");

    let keys: Vec<String> = tracks(&album)
        .iter()
        .take(2)
        .map(|t| t.key.clone())
        .collect();
    let id = backend
        .create_playlist("FormalMusic check".into(), vec![keys[0].clone()])
        .await
        .unwrap();
    let result = async {
        backend.add_to_playlist(&id, vec![keys[1].clone()]).await?;
        backend
            .edit_playlist(
                &id,
                PlaylistDetails {
                    description: Some("checked".into()),
                    ..PlaylistDetails::default()
                },
            )
            .await?;
        // YouTube takes a moment to list a track just added.
        let target = BrowseTarget::Playlist(id.clone());
        let mut page = backend.browse(&target).await?;
        for _ in 0..10 {
            if tracks(&page).len() == keys.len() {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            page = backend.browse(&target).await?;
        }
        let listed: Vec<String> = tracks(&page).iter().map(|t| t.key.clone()).collect();
        backend.remove_from_playlist(&id, 0).await?;
        let after = backend.browse(&BrowseTarget::Playlist(id.clone())).await?;
        let remaining: Vec<String> = tracks(&after).iter().map(|t| t.key.clone()).collect();
        Ok::<_, formalmusic_core::ClientError>((page, listed, remaining))
    }
    .await;
    backend.delete_playlist(&id).await.unwrap();
    let (page, listed, remaining) = result.unwrap();
    assert_eq!(listed, keys);
    assert_eq!(remaining, keys[1..]);
    assert!(matches!(
        page.header,
        Some(Header::Detail { editable: true, .. })
    ));
}

/// A fresh daemon: FormalMusic sets its source up anonymously, a pasted
/// `Cookie` header signs it in, and signing out leaves it anonymous again.
/// `FORMALMUSIC_LIVE_SESSION` names a JSON file whose `cookies` is that header.
#[tokio::test]
#[ignore = "needs a fresh kopuzd"]
async fn a_fresh_daemon_signs_in_and_out() {
    let Ok(path) = std::env::var("FORMALMUSIC_LIVE_SESSION") else {
        eprintln!("skipped: FORMALMUSIC_LIVE_SESSION is not set");
        return;
    };
    let session: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let cookies = session["cookies"].as_str().unwrap().to_owned();
    let (backend, first) = connected().await;
    assert!(!first.signed_in, "a fresh source starts anonymous");
    let home = backend.browse(&BrowseTarget::Home).await.unwrap();
    assert!(!home.sections.is_empty(), "anonymous home");
    assert!(
        backend.sign_in("not a cookie".into()).await.is_err(),
        "nonsense is refused"
    );
    let signed = backend.sign_in(cookies).await.unwrap();
    assert!(signed.signed_in);
    let library = backend
        .browse(&BrowseTarget::Library(LibraryTab::Albums))
        .await
        .unwrap();
    assert!(!library.sections.is_empty());
    backend.sign_out().await.unwrap();
    assert!(!backend.session().await.unwrap().signed_in);
}

/// The accounts under the sign-in, and a switch to another and back where
/// there is one.
#[tokio::test]
#[ignore = "needs a running kopuzd; switches account and back"]
async fn brand_accounts_list_and_switch_back() {
    let (backend, session) = connected().await;
    if !session.signed_in {
        eprintln!("skipped: the source is not signed in");
        return;
    }
    let accounts = backend.accounts().await.unwrap();
    println!("{} accounts", accounts.len());
    let Some(active) = accounts.iter().find(|a| a.selected) else {
        eprintln!("skipped: the source has no accounts to switch between");
        return;
    };
    assert_eq!(
        session.account.as_ref().map(|a| &a.name),
        Some(&active.name)
    );
    let Some(other) = accounts.iter().find(|a| !a.selected) else {
        return;
    };
    let switched = backend.switch_account(other.page_id.clone()).await.unwrap();
    let back = backend
        .switch_account(active.page_id.clone())
        .await
        .unwrap();
    assert_eq!(switched.account.map(|a| a.name), Some(other.name.clone()));
    assert_eq!(back.account.map(|a| a.name), Some(active.name.clone()));
}
