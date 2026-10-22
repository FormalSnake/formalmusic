//! Tier 1: the store on the demo catalog with nd's test backend, no window server.

use std::time::{Duration, Instant};

use formalmusic::menus::{self, ItemCommand, MenuContext};
use formalmusic_api::{BrowseTarget, Item};
use nd::components::{ContextMenu, MenuItem};
use nd::test::TestApp;

fn demo(cx: &mut TestApp) -> formalmusic::Music {
    // Safety: set before any other thread starts.
    unsafe { std::env::set_var("FORMALMUSIC_DEMO", "1") };
    let runtime = formalmusic::runtime();
    let store = formalmusic::build_store(&runtime, None);
    let music = formalmusic::Music {
        store: store.clone(),
        runtime: runtime.clone(),
        settings: None,
    };
    cx.set_global(music.clone());
    runtime.block_on(store.start());
    music
}

fn wait(what: &str, ok: impl Fn() -> bool) {
    let start = Instant::now();
    while !ok() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn command(menu: &ContextMenu, label: &str) -> ItemCommand {
    let found = menu.items.iter().find_map(|item| match item {
        MenuItem::Action {
            label: l, action, ..
        } if l.as_ref() == label => action.as_any().downcast_ref::<ItemCommand>().cloned(),
        _ => None,
    });
    found.unwrap_or_else(|| panic!("no {label:?} row"))
}

#[test]
fn a_track_menu_queues_and_rates_the_track() {
    let mut cx = TestApp::new();
    let music = demo(&mut cx);
    let store = music.store.clone();
    store.open(BrowseTarget::Home);
    wait("Home", || store.state().page(&BrowseTarget::Home).is_some());
    let songs: Vec<Item> = store
        .state()
        .page(&BrowseTarget::Home)
        .map(|p| {
            p.sections
                .iter()
                .flat_map(|s| &s.items)
                .filter(|i| matches!(i, Item::Track(_)))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    assert!(songs.len() >= 2, "Home has songs");
    let track = songs[1].clone();
    let menu = menus::item_menu(ContextMenu::default(), &track, &MenuContext::default(), &cx);
    let labels: Vec<String> = menu
        .items
        .iter()
        .filter_map(|i| match i {
            MenuItem::Action { label, .. } => Some(label.to_string()),
            _ => None,
        })
        .collect();
    for expected in [
        "Start radio",
        "Play next",
        "Add to queue",
        "Like",
        "Dislike",
        "Share",
    ] {
        assert!(
            labels.iter().any(|l| l == expected),
            "{expected} missing from {labels:?}"
        );
    }
    let (Item::Track(playing), Item::Track(t)) = (&songs[0], &track) else {
        unreachable!()
    };
    store.play(
        formalmusic_api::PlaySource::Tracks {
            tracks: vec![playing.clone()],
        },
        0,
        false,
        false,
    );
    wait("playback", || !store.state().queue.tracks.is_empty());
    assert_eq!(menus::run(&command(&menu, "Add to queue"), &mut cx), None);
    wait("the song at the end of the queue", || {
        store
            .state()
            .queue
            .tracks
            .last()
            .is_some_and(|last| last.video_id == t.video_id)
    });
    assert!(menus::run(&command(&menu, "Like"), &mut cx).is_none());
    wait("the like", || {
        store.state().rating(t) == formalmusic_api::Rating::Like
    });
}

#[test]
fn share_copies_and_go_to_opens() {
    let mut cx = TestApp::new();
    let music = demo(&mut cx);
    let album = Item::Album {
        browse_id: "MPREb_demo".into(),
        playlist_id: Some("OLAK_demo".into()),
        title: "Demo".into(),
        album_type: None,
        artists: Vec::new(),
        year: None,
        thumbnails: Vec::new(),
        explicit: false,
    };
    let menu = menus::item_menu(ContextMenu::default(), &album, &MenuContext::default(), &cx);
    let route = menus::run(&command(&menu, "Go to album"), &mut cx);
    assert_eq!(
        route,
        Some(formalmusic_core::Route::Browse(BrowseTarget::Album(
            "MPREb_demo".into()
        )))
    );
    drop(music);
}

/// A toast's notice clears itself through `bridge::after`, whose timer has to start on the
/// store's runtime: started on nd's task threads it panicked, and the panic ended the app.
#[test]
fn a_notice_clears_itself_after_its_delay() {
    let mut cx = TestApp::new();
    let music = demo(&mut cx);
    let store = music.store.clone();
    store.confirm("Link copied".into());
    let seq = store.state().notice.as_ref().expect("the notice").seq;
    let cleared = store.clone();
    formalmusic::bridge::after(&cx, Duration::from_millis(20), move |_| {
        cleared.clear_notice(seq)
    });
    let start = Instant::now();
    while store.state().notice.is_some() {
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "the notice never cleared"
        );
        std::thread::sleep(Duration::from_millis(10));
        nd::system::run_pending(&mut cx);
    }
    drop(music);
}
