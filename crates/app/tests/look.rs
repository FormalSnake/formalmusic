//! Tier 3: the window's look on the demo catalog, for review against the old FormalMusic and
//! the platform. Walks Home, a playlist with a song row under the pointer, the player bar's
//! toggles, the expanded player and its queue, and the signed-out sidebar, keeping a
//! screenshot of each in `ND_SHOT=<dir>`. Opens a real window, so it runs only with
//! `ND_REAL=1` (or under nextest).

use std::time::Duration;

use nd_test::{LaunchOptions, Role, Session, expect, launch_app};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--list") {
        println!("look::demo_pages_player_and_sign_in: test");
        return;
    }
    if std::env::var_os("ND_REAL").is_none_or(|v| v != "1") && std::env::var_os("NEXTEST").is_none()
    {
        println!("look: skipped, it opens a real window; run with ND_REAL=1");
        return;
    }
    signed_in();
    signed_out();
    println!("FORMALMUSIC_LOOK_OK");
}

/// Where the screenshots go, and what takes them. The GPUI backend renders to no image on Linux,
/// so grim reads the headless output; AppKit's layer render leaves out the sidebar's vibrancy and
/// the toolbar, so on macOS the window server composites the app's window.
struct Shots {
    dir: Option<std::path::PathBuf>,
    #[cfg(target_os = "linux")]
    hyprland: ndshell::hyprland::Hyprland,
    #[cfg(target_os = "macos")]
    pid: u32,
}

impl Shots {
    fn take(&self, app: &Session, name: &str) {
        let Some(dir) = &self.dir else { return };
        let _ = std::fs::create_dir_all(dir);
        let path = dir.join(format!("{name}.png"));
        #[cfg(target_os = "linux")]
        {
            let _ = app;
            // A dev shell's PATH leaves the system's grim out.
            let system = std::path::Path::new("/run/current-system/sw/bin/grim");
            let grim = if system.exists() { system } else { std::path::Path::new("grim") };
            match self.hyprland.command(grim).arg(&path).status() {
                Ok(status) if status.success() => {}
                other => eprintln!("screenshot {name}: grim {other:?}"),
            }
        }
        #[cfg(target_os = "macos")]
        {
            let _ = app;
            let Some(window) = ndshell::window_list::of_pid(self.pid).into_iter().next() else {
                eprintln!("screenshot {name}: no window on screen");
                return;
            };
            if let Err(e) = ndshell::window_list::capture(window.number, &path) {
                eprintln!("screenshot {name}: {e}");
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        if let Err(e) = app.screenshot(&path, None) {
            eprintln!("screenshot {name}: {e}");
        }
    }
}

impl Shots {
    /// Waits up to five seconds for the bar's cover to paint: covers decode off the main thread,
    /// and a capture taken before shows the flat placeholder.
    #[cfg(target_os = "macos")]
    fn wait_for_art(&self, app: &Session) {
        let Some(dir) = &self.dir else { return };
        let probe = dir.join("art-probe.png");
        let art = app.get_by_test_id("player.bar.art");
        let Ok(Some(b)) = art.bounding_box() else { return };
        for _ in 0..25 {
            let Some(window) = ndshell::window_list::of_pid(self.pid).into_iter().next() else { return };
            if ndshell::window_list::capture(window.number, &probe).is_ok()
                && let Ok(img) = image::open(&probe)
            {
                let img = img.to_rgb8();
                let scale = f64::from(img.width()) / window.bounds.2;
                let at = |x: f32, y: f32| {
                    let px = f64::from(b.x + x) * scale;
                    let py = f64::from(b.y + y) * scale;
                    *img.get_pixel(px as u32, py as u32)
                };
                let corner = at(b.width * 0.15, b.height * 0.15);
                let centre = at(b.width / 2., b.height / 2.);
                if corner != centre {
                    let _ = std::fs::remove_file(&probe);
                    return;
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let _ = std::fs::remove_file(&probe);
        eprintln!("screenshot: the bar's cover had not painted after five seconds");
    }

    #[cfg(not(target_os = "macos"))]
    fn wait_for_art(&self, _app: &Session) {
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Clicks the middle of `locator` with the pointer: nd's automation presses buttons only.
fn tap(app: &Session, locator: &nd_test::Locator, what: &str) {
    expect(locator).to_be_visible().expect(what);
    let b = locator.bounding_box().expect("bounds").expect("laid out");
    app.mouse()
        .click(b.x + b.width / 2., b.y + b.height / 2.)
        .expect(what);
}

fn launch(signed_out: bool, run: impl FnOnce(&Session, &Shots)) {
    let options = LaunchOptions::new(env!("CARGO_BIN_EXE_formalmusic"))
        .env("FORMALMUSIC_DEMO", "1")
        .env(
            "FORMALMUSIC_DEMO_SIGNED_OUT",
            if signed_out { "1" } else { "0" },
        );
    let dir = std::env::var_os("ND_SHOT").map(std::path::PathBuf::from);
    #[cfg(target_os = "linux")]
    let shots = Shots {
        dir,
        hyprland: ndshell::hyprland::Hyprland::start(Default::default())
            .expect("headless Hyprland"),
    };
    #[cfg(target_os = "linux")]
    let options = shots
        .hyprland
        .client_env()
        .into_iter()
        .fold(options.machine_lock(false), |o, (k, v)| o.env(k, v));
    let handle = launch_app(options).expect("launch");
    #[cfg(target_os = "macos")]
    let shots = Shots { dir, pid: handle.pid() };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let shots = Shots { dir };
    run(&handle.with_timeout(Duration::from_secs(15)), &shots);
}

fn signed_in() {
    launch(false, |app, shots| {
        expect(&app.get_by_test_id("chips"))
            .to_be_visible()
            .expect("Home shows");
        shots.take(app, "1-home");

        app.keyboard().press("g l").expect("open Library");
        tap(
            app,
            &app.get_by_text("Morning run").first(),
            "open a playlist from the Library",
        );
        let row = app.get_by_test_id("row-0-1");
        expect(&row).to_be_visible().expect("the playlist lists songs");
        let b = row.bounding_box().expect("bounds").expect("laid out");
        app.mouse()
            .move_to(b.x + b.width / 2., b.y + b.height / 2.)
            .expect("point at a song");
        std::thread::sleep(Duration::from_millis(300));
        shots.take(app, "2-playlist-hover");

        app.mouse()
            .click(b.x + b.width / 2., b.y + b.height / 2.)
            .expect("play the song");
        expect(&app.get_by_test_id("player.more"))
            .to_be_visible()
            .expect("the bar shows the song's controls");
        app.get_by_test_id("player.shuffle")
            .click()
            .expect("shuffle on");
        app.get_by_test_id("player.repeat")
            .click()
            .expect("repeat all");
        std::thread::sleep(Duration::from_millis(300));
        shots.wait_for_art(app);
        shots.take(app, "3-player-bar");

        // The queue button is on both the docked bar and macOS's pill.
        app.get_by_test_id("player.queue")
            .click()
            .expect("open the player");
        expect(&app.get_by_test_id("queue"))
            .to_be_visible()
            .expect("the expanded player opens on Up next");
        shots.take(app, "4-expanded");
        app.get_by_test_id("np.tabs")
            .fill("Lyrics")
            .expect("pick the Lyrics tab");
        app.get_by_test_id("player.queue")
            .click()
            .expect("back to the queue");
        expect(&app.get_by_test_id("queue"))
            .to_be_visible()
            .expect("the queue button shows Up next");
        app.get_by_test_id("player.queue")
            .click()
            .expect("close the player");
        expect(&app.get_by_test_id("page"))
            .to_be_visible()
            .expect("the page shows again");

        // Home scrolled: content under the toolbar, a shelf scrolled out to the content's edge,
        // then the end of the page resting clear of the player bar.
        app.keyboard().press("g h").expect("open Home");
        expect(&app.get_by_test_id("chips"))
            .to_be_visible()
            .expect("Home shows");
        let page = app.get_by_test_id("page");
        page.scroll(0., 240.).expect("scroll Home down");
        app.get_by_role(Role::ScrollArea)
            .name("Listen again")
            .scroll(400., 0.)
            .expect("scroll a shelf sideways");
        std::thread::sleep(Duration::from_millis(500));
        shots.take(app, "6-scrolled");
        page.scroll(0., 100_000.).expect("scroll to the end");
        std::thread::sleep(Duration::from_millis(500));
        shots.take(app, "7-end");
    });
}

fn signed_out() {
    launch(true, |app, shots| {
        std::thread::sleep(Duration::from_secs(2));
        shots.take(app, "5-signed-out");
        expect(&app.get_by_test_id("toolbar.search"))
            .to_be_visible()
            .expect("the window shows signed out");
    });
}
