//! Tier 3: the built app as its own process on the demo catalog (recorded pages, simulated
//! playback, no daemon and no network), driven over its automation socket. It searches,
//! plays the first song it finds, seeks, and checks that the lyrics follow the clock.
//!
//! Opens a real window, so it runs only with `ND_REAL=1` (or under nextest). `ND_SHOT=<dir>`
//! keeps screenshots of each step there.

use std::time::Duration;

use nd_test::{LaunchOptions, Session, expect, launch_app};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--list") {
        println!("real::demo_search_plays_seeks_and_lyrics_follow: test");
        return;
    }
    if std::env::var_os("ND_REAL").is_none_or(|v| v != "1") && std::env::var_os("NEXTEST").is_none()
    {
        println!("real: skipped, it opens a real window; run with ND_REAL=1");
        return;
    }
    demo_search_plays_seeks_and_lyrics_follow();
    println!("FORMALMUSIC_REAL_OK");
}

fn shot(app: &Session, name: &str) {
    if let Some(dir) = std::env::var_os("ND_SHOT") {
        let _ = std::fs::create_dir_all(&dir);
        let path = std::path::Path::new(&dir).join(format!("{name}.png"));
        if let Err(e) = app.screenshot(&path, None) {
            eprintln!("screenshot {name}: {e}");
        }
    }
}

/// The lyrics view's value: the line it centres.
fn lyric(app: &Session) -> String {
    app.get_by_test_id("lyrics")
        .value()
        .ok()
        .flatten()
        .unwrap_or_default()
}

fn demo_search_plays_seeks_and_lyrics_follow() {
    let options =
        LaunchOptions::new(env!("CARGO_BIN_EXE_formalmusic")).env("FORMALMUSIC_DEMO", "1");
    // On Linux the window opens in a private headless Hyprland, never on a display.
    #[cfg(target_os = "linux")]
    let hyprland =
        ndshell::hyprland::Hyprland::start(Default::default()).expect("headless Hyprland");
    #[cfg(target_os = "linux")]
    let options = hyprland
        .client_env()
        .into_iter()
        .fold(options.machine_lock(false), |o, (k, v)| o.env(k, v));
    let handle = launch_app(options).expect("launch");
    let app = handle.with_timeout(Duration::from_secs(15));

    // Home paints from the demo catalog.
    expect(&app.get_by_test_id("chips"))
        .to_be_visible()
        .expect("Home shows");
    shot(&app, "1-home");

    // Open a page: search from the palette, as the toolbar's search does.
    app.get_by_test_id("toolbar.search")
        .click()
        .expect("open search");
    expect(&app.get_by_test_id("search.search"))
        .to_be_visible()
        .expect("the search palette opens");
    app.keyboard()
        .type_text("Lumen Atlas")
        .expect("type a query");
    app.keyboard().press("enter").expect("search");
    expect(&app.get_by_test_id("top-0"))
        .to_be_visible()
        .expect("the search page shows its top result");
    // The Songs filter: one list of songs.
    app.get_by_test_id("chips.1")
        .click()
        .expect("filter to songs");
    let first_row = app.get_by_test_id("row-0-0");
    expect(&first_row)
        .to_be_visible()
        .expect("the search page lists songs");
    shot(&app, "2-search");

    // Start playback: choosing a row plays it.
    // A pointer click: nd's automation presses buttons only, and a list row is a table row.
    let b = first_row.bounding_box().expect("bounds").expect("laid out");
    app.mouse()
        .click(b.x + b.width / 2., b.y + b.height / 2.)
        .expect("play the first track");
    let seek = app.get_by_test_id("player.bar.seek");
    // Playing: the bar's position moves on its own.
    let before = seek.value().ok().flatten().unwrap_or_default();
    let moved = (0..40).any(|_| {
        std::thread::sleep(Duration::from_millis(250));
        seek.value().ok().flatten().unwrap_or_default() != before
    });
    assert!(moved, "the track plays: the bar stayed at {before}");

    // The expanded player on its Lyrics tab.
    app.get_by_test_id("player.expand")
        .click()
        .expect("open the player");

    app.get_by_test_id("np.tabs")
        .fill("Lyrics")
        .expect("pick the Lyrics tab");
    let lyrics = app.get_by_test_id("lyrics");
    expect(&lyrics).to_be_visible().expect("lyrics show");

    // Seek into the song: the bar moves and the lyrics follow to a line.
    seek.fill("0:40").expect("seek");
    let at_40 = wait_for_line(&app, |l| !l.is_empty()).expect("a line is lit at 0:40");
    let value = seek.value().ok().flatten().unwrap_or_default();
    assert!(
        value.starts_with("0:4"),
        "the bar sits at the seek: {value}"
    );
    shot(&app, "3-lyrics-40s");

    // Seek again: a different line is centred.
    seek.fill("1:30").expect("seek again");
    let at_90 =
        wait_for_line(&app, |l| !l.is_empty() && l != at_40).expect("the lyrics follow the seek");
    // And while it plays they keep following without another seek.
    let later = wait_for_line(&app, |l| !l.is_empty() && l != at_90);
    assert!(
        later.is_some(),
        "the lyrics move on while the track plays (stuck on {at_90:?})"
    );
    shot(&app, "4-lyrics-follow");

    // Single-key shortcuts reach the window: Q shows Up next, Escape closes the player.
    app.keyboard().press("q").expect("press q");
    expect(&app.get_by_test_id("queue"))
        .to_be_visible()
        .expect("q shows the queue");
    app.keyboard().press("escape").expect("press escape");
    expect(&app.get_by_test_id("chips"))
        .to_be_visible()
        .expect("escape closes the player");

    // Chords open the other pages, and back returns through them.
    app.keyboard().press("g e").expect("g e");
    expect(&app.get_by_text("New releases").first())
        .to_be_visible()
        .expect("Explore opens");
    app.keyboard().press("g l").expect("g l");
    expect(&app.get_by_test_id("chips.0"))
        .to_have_text("Playlists")
        .expect("Library opens");
    shot(&app, "5-library");
    app.get_by_test_id("toolbar.back").click().expect("back");
    expect(&app.get_by_text("New releases").first())
        .to_be_visible()
        .expect("back to Explore");

    // The settings window, with its rows from config.toml.
    app.get_by_test_id("toolbar.settings")
        .click()
        .expect("open settings");
    expect(&app.get_by_test_id("playback.quality"))
        .to_be_visible()
        .expect("settings show the quality picker");
    // Further down the scrolling form (nd cannot scroll a form row into view on AppKit yet).
    expect(&app.get_by_test_id("eq.preset"))
        .to_be_attached()
        .expect("and the equalizer");
    shot(&app, "6-settings");
}

/// Polls the centred lyric until `ok` holds, for up to 15 seconds.
fn wait_for_line(app: &Session, ok: impl Fn(&str) -> bool) -> Option<String> {
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(15) {
        let line = lyric(app);
        if ok(&line) {
            return Some(line);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}
