//! Tier 3 against a running Kopuz daemon: the built app on `FORMALMUSIC_KOPUZ_SOCKET`, signed
//! out. It browses Home, searches for an album, plays it, skips to the next track and checks
//! that the queue follows.
//!
//! Runs only with `FORMALMUSIC_KOPUZ_SOCKET` set: point it at a scratch kopuzd
//! (`kopuzd --socket <tmp>/kopuzd.sock --db-path <tmp>/library.db`), never the owner's.
//! `ND_SHOT=<dir>` keeps screenshots of each step there.

use std::time::Duration;

use nd_test::{LaunchOptions, Session, expect, launch_app};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--list") {
        println!("kopuz::home_search_play_and_queue: test");
        return;
    }
    let Some(socket) = std::env::var_os("FORMALMUSIC_KOPUZ_SOCKET") else {
        println!("kopuz: skipped, set FORMALMUSIC_KOPUZ_SOCKET to a scratch kopuzd");
        return;
    };
    home_search_play_and_queue(&socket.to_string_lossy());
    println!("FORMALMUSIC_KOPUZ_OK");
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

fn click_center(app: &Session, test_id: &str) {
    let target = app.get_by_test_id(test_id);
    expect(&target).to_be_visible().expect(test_id);
    let b = target.bounding_box().expect("bounds").expect("laid out");
    app.mouse()
        .click(b.x + b.width / 2., b.y + b.height / 2.)
        .expect(test_id);
}

fn wait(what: &str, ok: impl Fn() -> bool) {
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        if ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    panic!("timed out waiting for {what}");
}

fn home_search_play_and_queue(socket: &str) {
    let options = LaunchOptions::new(env!("CARGO_BIN_EXE_formalmusic"))
        .env("FORMALMUSIC_KOPUZ_SOCKET", socket);
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
    let app = handle.with_timeout(Duration::from_secs(30));

    // First run, signed out: the onboarding offers to browse without an account.
    app.get_by_test_id("signin.skip")
        .click()
        .expect("browse without signing in");

    // Home: Kopuz's FEmusic_home with its mood chips.
    expect(&app.get_by_test_id("chips"))
        .to_be_visible()
        .expect("Home shows");
    shot(&app, "1-home");

    app.get_by_test_id("toolbar.search")
        .click()
        .expect("open search");
    app.keyboard()
        .type_text("Random Access Memories")
        .expect("type a query");
    app.keyboard().press("enter").expect("search");
    // The Albums filter.
    app.get_by_test_id("chips.3")
        .click()
        .expect("filter to albums");
    shot(&app, "2-search");
    click_center(&app, "grid-0-0");

    // The album page: choosing its first row plays the album.
    click_center(&app, "row-0-0");
    let seek = app.get_by_test_id("player.bar.seek");
    let started = seek.value().ok().flatten().unwrap_or_default();
    wait("playback to move the bar", || {
        seek.value().ok().flatten().unwrap_or_default() != started
    });
    shot(&app, "3-playing");

    // Queue control: next moves to the album's second track.
    // The cover image is named after the playing track.
    let bar = app.get_by_test_id("player.bar.art");
    let first = bar.text().unwrap_or_default();
    app.get_by_test_id("player.bar.next")
        .click()
        .expect("next");
    wait("next to change the track", || {
        bar.text().unwrap_or_default() != first
    });

    // The queue holds the rest of the album.
    app.keyboard().press("q").expect("press q");
    expect(&app.get_by_test_id("queue"))
        .to_be_visible()
        .expect("q shows the queue");
    expect(&app.get_by_test_id("queue-2"))
        .to_be_attached()
        .expect("the album is queued");
    shot(&app, "4-queue");

    // Pause from the bar, once the new track is playing.
    let loaded = seek.value().ok().flatten().unwrap_or_default();
    wait("the next track to play", || {
        seek.value().ok().flatten().unwrap_or_default() != loaded
    });
    app.get_by_test_id("player.bar.play")
        .click()
        .expect("pause");
    // The bar runs on its own clock until the daemon's paused state lands.
    std::thread::sleep(Duration::from_secs(1));
    let paused = seek.value().ok().flatten().unwrap_or_default();
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        seek.value().ok().flatten().unwrap_or_default(),
        paused,
        "paused, the bar holds still"
    );
}
