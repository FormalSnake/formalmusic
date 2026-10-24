//! Tier 3: the expanded player closes back onto the page. Closes it with the bar's button and
//! Escape, and once more after opening it from the bar's cover, and after each close expects the
//! player and its cover gone and Home shown. With the platform's reduce motion on (Windows with
//! animation effects off) the cover does not travel, and the player has to close all the same.
//! Opens a real window, so it runs only with `ND_REAL=1` (or under nextest).

use std::time::Duration;

use nd_test::{LaunchOptions, Session, expect, launch_app};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--list") {
        println!("collapse::player_closes_onto_the_page: test");
        return;
    }
    if std::env::var_os("ND_REAL").is_none_or(|v| v != "1") && std::env::var_os("NEXTEST").is_none()
    {
        println!("collapse: skipped, it opens a real window; run with ND_REAL=1");
        return;
    }
    let options = LaunchOptions::new(env!("CARGO_BIN_EXE_formalmusic"))
        .env("FORMALMUSIC_DEMO", "1")
        .env("FORMALMUSIC_DEMO_SIGNED_OUT", "0");
    let handle = launch_app(options).expect("launch");
    let app = handle.with_timeout(Duration::from_secs(15));
    if let Err(panic) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&app))) {
        eprintln!("app stderr:\n{}", handle.stderr_tail(40).join("\n"));
        std::panic::resume_unwind(panic);
    }
    println!("FORMALMUSIC_COLLAPSE_OK");
}

fn run(app: &Session) {
    expect(&app.get_by_test_id("chips"))
        .to_be_visible()
        .expect("Home shows");
    let toggle = if app.get_by_test_id("player.expand").count().unwrap_or(0) > 0 {
        "player.expand"
    } else {
        "player.bar.now"
    };
    let closes: [(&str, &dyn Fn()); 3] = [
        ("button", &|| tap(app, "close", &app.get_by_test_id(toggle))),
        ("escape", &|| {
            app.keyboard()
                .press("Escape")
                .expect("Escape closes the player");
        }),
        ("cover", &|| tap(app, "close", &app.get_by_test_id(toggle))),
    ];
    for (how, close) in closes {
        let open = if how == "cover" {
            "player.bar.art"
        } else {
            toggle
        };
        tap(app, "open the player", &app.get_by_test_id(open));
        expect(&app.get_by_test_id("queue"))
            .to_be_visible()
            .expect("the player opens");
        std::thread::sleep(Duration::from_millis(1200));
        close();
        std::thread::sleep(Duration::from_millis(2000));
        let covers = app.get_by_test_id("np.cover").count().unwrap_or(99);
        let queue = app.get_by_test_id("queue").count().unwrap_or(99);
        eprintln!("after closing with {how}: np.cover {covers}, queue {queue}");
        assert_eq!(
            covers, 0,
            "the player's cover is gone after closing with {how}"
        );
        assert_eq!(queue, 0, "the player is gone after closing with {how}");
        expect(&app.get_by_test_id("chips"))
            .to_be_visible()
            .expect("Home shows again");
    }
}

fn tap(app: &Session, what: &str, locator: &nd_test::Locator) {
    expect(locator).to_be_visible().expect(what);
    let b = locator.bounding_box().expect("bounds").expect("laid out");
    app.mouse()
        .click(b.x + b.width / 2., b.y + b.height / 2.)
        .expect(what);
}
