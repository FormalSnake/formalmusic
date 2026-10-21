//! Tier 3, Linux: the tray item on a private session bus, read and driven as a panel would over
//! D-Bus. The item shows with the app's icon, its menu plays the demo song, the app runs on
//! behind it with the window closed, a click on the item brings the window back, and Quit ends
//! the app. Opens a real window, so it runs only with `ND_REAL=1` (or under nextest).

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--list") {
        println!("tray::the_tray_plays_reopens_and_quits: test");
        return;
    }
    if std::env::var_os("ND_REAL").is_none_or(|v| v != "1") && std::env::var_os("NEXTEST").is_none()
    {
        println!("tray: skipped, it opens a real window; run with ND_REAL=1");
        return;
    }
    #[cfg(target_os = "linux")]
    linux::run();
    #[cfg(not(target_os = "linux"))]
    println!("tray: skipped, the D-Bus tray is Linux's");
}

#[cfg(target_os = "linux")]
mod linux {
    use std::process::Command;
    use std::time::{Duration, Instant};

    use nd_test::{LaunchOptions, launch_app};
    use serde_json::Value;

    struct Bus<'a> {
        address: &'a str,
    }

    impl Bus<'_> {
        fn busctl(&self, args: &[&str]) -> Option<Value> {
            let out = Command::new("busctl")
                .arg(format!("--address={}", self.address))
                .arg("--json=short")
                .args(args)
                .output()
                .ok()?;
            out.status
                .success()
                .then(|| serde_json::from_slice(&out.stdout).ok())
                .flatten()
        }

        /// The app's StatusNotifierItem's bus name, once it is served.
        fn item(&self) -> Option<String> {
            let out = Command::new("busctl")
                .arg(format!("--address={}", self.address))
                .args(["list", "--no-legend"])
                .output()
                .ok()?;
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|l| l.split_whitespace().next())
                .find(|n| n.starts_with("org.kde.StatusNotifierItem-"))
                .map(str::to_owned)
        }

        fn property(&self, item: &str, name: &str) -> Value {
            self.busctl(&[
                "get-property",
                item,
                "/StatusNotifierItem",
                "org.kde.StatusNotifierItem",
                name,
            ])
            .unwrap_or(Value::Null)
        }

        /// The menu's top-level items as (id, label).
        fn menu(&self, item: &str) -> Vec<(i64, String)> {
            let layout = self
                .busctl(&[
                    "call",
                    item,
                    "/MenuBar",
                    "com.canonical.dbusmenu",
                    "GetLayout",
                    "iias",
                    "0",
                    "1",
                    "0",
                ])
                .unwrap_or(Value::Null);
            // (revision, (id, properties, children)); each child is a variant of the same shape.
            let children = &layout["data"][1][2];
            children
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|child| {
                    let node = &child["data"];
                    let id = node[0].as_i64()?;
                    let label = node[1]["label"]["data"].as_str().unwrap_or_default();
                    Some((id, label.to_owned()))
                })
                .collect()
        }

        fn click(&self, item: &str, label: &str) {
            let id = self
                .menu(item)
                .into_iter()
                .find(|(_, l)| l == label)
                .unwrap_or_else(|| panic!("the tray menu has {label}: {:?}", self.menu(item)))
                .0;
            self.busctl(&[
                "call",
                item,
                "/MenuBar",
                "com.canonical.dbusmenu",
                "Event",
                "isvu",
                &id.to_string(),
                "clicked",
                "s",
                "",
                "0",
            ]);
        }
    }

    fn wait(what: &str, ok: impl Fn() -> bool) {
        let start = Instant::now();
        while !ok() {
            assert!(start.elapsed() < Duration::from_secs(15), "timed out: {what}");
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    pub fn run() {
        let hyprland = ndshell::hyprland::Hyprland::start(Default::default()).expect("headless Hyprland");
        let address = hyprland.bus_address().to_owned();
        // The defaults, not this machine's settings: closing the window pauses.
        let config = std::env::temp_dir().join(format!("formalmusic-tray-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&config);
        let options = LaunchOptions::new(env!("CARGO_BIN_EXE_formalmusic"))
            .env("FORMALMUSIC_DEMO", "1")
            .env("XDG_CONFIG_HOME", config.display().to_string())
            .env("DBUS_SESSION_BUS_ADDRESS", address.clone());
        let options = hyprland
            .client_env()
            .into_iter()
            .fold(options.machine_lock(false), |o, (k, v)| o.env(k, v));
        let handle = launch_app(options).expect("launch");
        let app = handle.with_timeout(Duration::from_secs(15));
        let bus = Bus { address: &address };
        // The test is the app's parent and never reaps it, so an exited app stays a zombie.
        let stat = format!("/proc/{}/stat", handle.pid());
        let alive = || {
            std::fs::read_to_string(&stat)
                .is_ok_and(|s| s.rsplit_once(") ").is_some_and(|(_, rest)| !rest.starts_with('Z')))
        };

        wait("the tray item is served", || bus.item().is_some());
        let item = bus.item().expect("item");
        let icon = bus.property(&item, "IconName");
        let pixmap = bus.property(&item, "IconPixmap");
        println!("tray: {item} IconName {} IconPixmap {} bytes", icon["data"], pixmap["data"].to_string().len());
        assert!(
            icon["data"].as_str().is_some_and(|n| !n.is_empty()) || pixmap["data"].as_array().is_some_and(|p| !p.is_empty()),
            "the item shows an icon: {icon} {pixmap}"
        );
        let labels: Vec<String> = bus.menu(&item).into_iter().map(|(_, l)| l).collect();
        println!("tray menu: {labels:?}");
        for label in ["Play", "Next", "Previous", "Open FormalMusic", "Quit FormalMusic"] {
            assert!(labels.iter().any(|l| l == label), "the menu has {label}: {labels:?}");
        }

        bus.click(&item, "Play");
        wait("Play becomes Pause", || bus.menu(&item).iter().any(|(_, l)| l == "Pause"));

        hyprland.request("dispatch hl.dsp.window.close()").expect("Hyprland closes the window");
        wait("the window closes", || {
            app.windows().ok().and_then(|w| w.as_array().map(Vec::len)) == Some(0)
        });
        std::thread::sleep(Duration::from_secs(1));
        assert!(alive(), "the app runs on behind the tray");
        assert!(bus.item().is_some(), "the item stays with the window closed");
        wait("closing the window paused", || bus.menu(&item).iter().any(|(_, l)| l == "Play"));

        bus.busctl(&["call", &item, "/StatusNotifierItem", "org.kde.StatusNotifierItem", "Activate", "ii", "0", "0"]);
        wait("a click on the item opens the window", || {
            app.windows().ok().and_then(|w| w.as_array().map(Vec::len)).is_some_and(|n| n > 0)
        });

        bus.click(&item, "Quit FormalMusic");
        wait("Quit ends the app", || !alive());
        let _ = std::fs::remove_dir_all(&config);
        println!("FORMALMUSIC_TRAY_OK");
    }
}
