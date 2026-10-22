//! Tier 3: the window's look on the demo catalog, for review against the old FormalMusic and
//! the platform. Walks Home with a card under the pointer, a playlist with a song row under the
//! pointer, the player bar's toggles, the expanded player opening and closing, its queue and
//! lyrics, and the signed-out sidebar, keeping a screenshot of each in `ND_SHOT=<dir>`. Opens a real window, so it runs only with
//! `ND_REAL=1` (or under nextest).

use std::time::Duration;

use nd_test::{LaunchOptions, Session, expect, launch_app};

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
    #[cfg(target_os = "linux")]
    app_pid: u32,
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
    /// Window widths to lay the expanded player out at: from the minimum up. Hyprland holds a
    /// window to any size; macOS to the app's minimum.
    fn widths(&self) -> Vec<u32> {
        if cfg!(target_os = "macos") {
            vec![880, 1000, 1400]
        } else {
            vec![480, 700, 1000, 1400]
        }
    }

    /// Records the window for `secs` seconds as `<name>.mp4`/`.mov`, at the display's rate: for
    /// checking motion artwork frame by frame (`ND_RECORD=<secs>`; on Linux `ND_RECORDER` names
    /// the wf-recorder binary when it is not on the PATH).
    fn record(&self, name: &str, secs: u64) {
        let Some(dir) = &self.dir else { return };
        #[cfg(target_os = "macos")]
        {
            let Some(w) = ndshell::window_list::of_pid(self.pid).into_iter().next() else {
                return;
            };
            let (x, y, ww, hh) = w.bounds;
            let rect = format!("-R{},{},{},{}", x as i64, y as i64, ww as i64, hh as i64);
            let _ = std::process::Command::new("screencapture")
                .args(["-v", "-x", &format!("-V{secs}"), &rect])
                .arg(dir.join(format!("{name}.mov")))
                .status();
        }
        #[cfg(target_os = "linux")]
        {
            let Ok(mut child) = self
                .hyprland
                .command(std::env::var("ND_RECORDER").unwrap_or_else(|_| "wf-recorder".into()))
                .arg("-y")
                .arg("-f")
                .arg(dir.join(format!("{name}.mp4")))
                .spawn()
            else {
                eprintln!("record {name}: no wf-recorder");
                return;
            };
            std::thread::sleep(Duration::from_secs(secs));
            // SIGINT lets it finish the file.
            let _ = std::process::Command::new("kill")
                .args(["-INT", &child.id().to_string()])
                .status();
            let _ = child.wait();
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let _ = (dir, name, secs);
    }

    /// Resizes the window to `width` points wide, 820 tall.
    fn resize(&self, width: u32) {
        #[cfg(target_os = "macos")]
        {
            let Some(w) = ndshell::window_list::of_pid(self.pid).into_iter().next() else {
                return;
            };
            let (x, y, ww, hh) = w.bounds;
            // Room to grow: the window first moves to the screen's left edge.
            if x > 0. {
                ndshell::input::drag((x + ww / 2., y + 12.), -x, 0., 20);
            }
            let Some(w) = ndshell::window_list::of_pid(self.pid).into_iter().next() else {
                return;
            };
            let (x, y, ww, hh2) = w.bounds;
            let _ = hh;
            ndshell::input::drag(
                (x + ww - 3., y + hh2 - 3.),
                f64::from(width) - ww,
                820. - hh2,
                20,
            );
        }
        #[cfg(target_os = "linux")]
        {
            // The headless session floats every window; its config is Lua, so dispatchers are too.
            let window = format!("pid:{}", self.app_pid);
            for call in [
                format!("hl.dsp.window.resize({{ x = {width}, y = 820, relative = false, window = \"{window}\" }})"),
                format!("hl.dsp.window.move({{ x = 0, y = 0, relative = false, window = \"{window}\" }})"),
            ] {
                match self.hyprland.request(&format!("dispatch {call}")) {
                    Ok(reply) if reply.trim() == "ok" => {}
                    reply => eprintln!("resize: {call}: {reply:?}"),
                }
            }
            let pid = i64::from(self.app_pid);
            let size = self
                .hyprland
                .clients()
                .ok()
                .and_then(|c| c.into_iter().find(|c| c.pid == pid))
                .map(|c| c.size);
            eprintln!("resize: {width} gave {size:?}");
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let _ = width;
    }

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

/// Puts the pointer at window point `(x, y)`. Hover comes from tracking areas on macOS, which a
/// posted move never reaches, so there the real pointer moves.
fn point_at(app: &Session, shots: &Shots, x: f32, y: f32) {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        ndshell::input::activate(shots.pid);
        let window = ndshell::window_list::of_pid(shots.pid)
            .into_iter()
            .next()
            .expect("window on screen");
        let (wx, wy, _, _) = window.bounds;
        ndshell::input::move_to((wx + f64::from(x), wy + f64::from(y)));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = shots;
        app.mouse().move_to(x, y).expect("move the pointer");
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
    let hyprland =
        ndshell::hyprland::Hyprland::start(Default::default()).expect("headless Hyprland");
    #[cfg(target_os = "linux")]
    let options = hyprland
        .client_env()
        .into_iter()
        .fold(options.machine_lock(false), |o, (k, v)| o.env(k, v));
    let handle = launch_app(options).expect("launch");
    #[cfg(target_os = "linux")]
    let shots = Shots {
        dir,
        hyprland,
        app_pid: handle.pid(),
    };
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
        // A card's own frame comes back relative to its shelf's scroll view, so aim from the
        // shelf: the first card's 168 pt artwork sits above its two 30 pt text lines.
        let b = (0..8)
            .find_map(|s| {
                app.get_by_test_id(&format!("shelf-{s}"))
                    .bounding_box()
                    .ok()
                    .flatten()
            })
            .expect("Home shows a shelf of cards");
        for s in 0..8 {
            for id in [format!("shelf-{s}"), format!("shelf-{s}.0")] {
                eprintln!("BOXDBG {id} {:?}", app.get_by_test_id(&id).bounding_box());
            }
        }
        point_at(app, shots, b.x + 84., b.y + b.height - 60. - 84.);
        std::thread::sleep(Duration::from_millis(400));
        shots.take(app, "1-home-card-hover");

        app.keyboard().press("g l").expect("open Library");
        tap(
            app,
            &app.get_by_text("Morning run").first(),
            "open a playlist from the Library",
        );
        let row = app.get_by_test_id("row-0-1");
        expect(&row).to_be_visible().expect("the playlist lists songs");
        let b = row.bounding_box().expect("bounds").expect("laid out");
        point_at(app, shots, b.x + b.width / 2., b.y + b.height / 2.);
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
        // Frames while the cover travels out of the bar (ND_ANIMATION_SLOWDOWN=8 spreads them
        // over the whole move), then the settled player.
        app.get_by_test_id("player.queue")
            .click()
            .expect("open the player");
        for i in 0..6 {
            shots.take(app, &format!("4-opening-{i}"));
        }
        expect(&app.get_by_test_id("queue"))
            .to_be_visible()
            .expect("the expanded player opens on Up next");
        std::thread::sleep(Duration::from_millis(800));
        shots.take(app, "4-expanded");
        if let Some(secs) = std::env::var("ND_RECORD").ok().and_then(|s| s.parse().ok()) {
            shots.record("4-expanded", secs);
        }
        app.get_by_test_id("np.tabs")
            .fill("Lyrics")
            .expect("pick the Lyrics tab");
        std::thread::sleep(Duration::from_millis(1500));
        shots.take(app, "4-lyrics");
        // The player's layout from the narrowest window up: the artwork keeps to its column.
        for width in shots.widths() {
            shots.resize(width);
            std::thread::sleep(Duration::from_millis(800));
            shots.take(app, &format!("6-lyrics-{width}"));
        }
        shots.resize(1280);
        app.get_by_test_id("player.queue")
            .click()
            .expect("back to the queue");
        expect(&app.get_by_test_id("queue"))
            .to_be_visible()
            .expect("the queue button shows Up next");
        app.get_by_test_id("player.queue")
            .click()
            .expect("close the player");
        for i in 0..6 {
            shots.take(app, &format!("4-closing-{i}"));
        }
        expect(&app.get_by_test_id("page"))
            .to_be_visible()
            .expect("the page shows again");
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
