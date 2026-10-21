//! FormalMusic's window on NativeDesktop. `formalmusic_core::MusicStore` holds every byte of
//! state and talks to the Kopuz daemon; this crate is views over it. `bridge` turns the store's
//! events into view notifications, `root` is the window, `page` renders any page or search,
//! `now_playing` the expanded player with the queue, lyrics and related tabs.

pub mod art;
pub mod bridge;
pub mod items;
pub mod menus;
pub mod now_playing;
pub mod page;
pub mod player;
pub mod root;
pub mod settings;
pub mod sheets;
pub mod tray;
pub mod video;

use std::sync::Arc;

use formalmusic_core::art::ArtCache;
use formalmusic_core::cache::{CachedState, StateCache};
use formalmusic_core::{MusicStore, StoreOptions, kopuz, paths};
use kopuz_client::GrpcApi;
use nd::prelude::*;

pub use root::Root;

/// `FORMALMUSIC_DEMO=1`: recorded pages and simulated playback, no daemon and no network.
pub fn demo() -> bool {
    std::env::var("FORMALMUSIC_DEMO").as_deref() == Ok("1")
}

/// Where `state.json` lives. The demo keeps its own beside the real one.
pub fn state_dir() -> std::path::PathBuf {
    if demo() {
        paths::cache_dir().join("demo")
    } else {
        paths::cache_dir()
    }
}

/// The store and the runtime its I/O runs on, as an app global.
#[derive(Clone)]
pub struct Music {
    pub store: MusicStore,
    pub runtime: tokio::runtime::Handle,
    pub settings: Option<nd_settings::SettingsHandle<formalmusic_api::settings::Settings>>,
}

pub fn music(cx: &App) -> &Music {
    cx.global::<Music>()
}

pub fn store(cx: &App) -> MusicStore {
    cx.global::<Music>().store.clone()
}

/// A two-thread runtime for the store's socket, timers and artwork, leaked for the process.
pub fn runtime() -> tokio::runtime::Handle {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("formalmusic-rt")
        .enable_all()
        .build()
        .expect("tokio runtime");
    Box::leak(Box::new(runtime)).handle().clone()
}

/// The store over the daemon, or over the demo catalog with `FORMALMUSIC_DEMO=1`.
pub fn build_store(runtime: &tokio::runtime::Handle, preloaded: Option<CachedState>) -> MusicStore {
    let http = reqwest::Client::builder()
        .user_agent(concat!("formalmusic/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_default();
    let art = ArtCache::new(paths::art_dir(), http);
    let (transport, art): (Arc<dyn formalmusic_core::Transport>, _) = if demo() {
        (Arc::new(formalmusic_core::demo::DemoTransport::new()), art)
    } else {
        let socket = kopuz::socket_path();
        // tonic's lazy channel spawns its connector on the current runtime.
        let _entered = runtime.enter();
        let daemon = Arc::new(
            GrpcApi::new(socket.clone()).expect("a socket path is a valid daemon address"),
        );
        (
            Arc::new(kopuz::KopuzTransport::new(daemon.clone(), socket)),
            art.with_daemon(daemon),
        )
    };
    let art = Arc::new(art);
    let cache = Some(Arc::new(StateCache::new(&state_dir())));
    MusicStore::new(
        transport,
        StoreOptions {
            cache,
            art,
            preloaded,
        },
        runtime.clone(),
    )
}

pub fn app() -> Application {
    Application::new(nd::manifest!())
}

/// Installs the store, keys and menus, and opens the window.
pub fn init(cx: &mut App, music: Music) -> WindowHandle<Root> {
    let store = music.store.clone();
    let runtime = music.runtime.clone();
    if let Some(settings) = &music.settings {
        nd::components::forms::settings::provide(cx, settings.clone());
        // Edits from Home Manager or another window show at once.
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        settings.on_event(move |_| {
            let _ = tx.send(());
        });
        let reload = store.clone();
        bridge::on_main(rx, move |(), cx| {
            reload.reload_settings();
            cx.refresh_windows();
            tray::sync(cx);
        });
        if let Ok(guard) = settings.watch() {
            std::mem::forget(guard);
        }
    }
    art::install(cx, store.art().clone(), runtime.clone());
    cx.set_global(music);
    bridge::install(cx, &store);
    root::bind_keys(cx);
    runtime.spawn(async move { store.start().await });
    open_window(cx)
}

/// The main window's id, while it is open.
struct MainWindow(nd::core::WindowId);

fn open_window(cx: &mut App) -> WindowHandle<Root> {
    let handle = cx.open_window(
        WindowOptions::main("main")
            .title("FormalMusic")
            .size(size(px(1280.), px(820.)))
            .min_size(size(px(880.), px(560.))),
        |window, cx| {
            // Behind the tray the app outlives the window, so closing it pauses here rather than
            // at exit, unless the settings say to keep playing.
            window.on_should_close(|_, cx| {
                if tray::keeps_running(cx)
                    && !settings::current(cx).window.keep_playing_when_closed
                {
                    store(cx).pause();
                }
                true
            });
            cx.new(|cx| Root::new(window, cx))
        },
    );
    cx.set_global(MainWindow(handle.id()));
    handle
}

/// Brings the main window forward, opening it again when it was closed behind the tray.
pub fn show_window(cx: &mut App) {
    let open = cx.try_global::<MainWindow>().map(|w| w.0).filter(|id| cx.window_ids().any(|w| w == *id));
    match open {
        Some(id) => cx.activate_window(id),
        None => {
            let id = open_window(cx).id();
            cx.activate_window(id);
        }
    }
}

pub fn run() -> nd::Result<()> {
    let preload = std::thread::Builder::new()
        .name("formalmusic-cache".into())
        .spawn(|| StateCache::new(&state_dir()).load_blocking())
        .ok();
    let runtime = runtime();
    let primary = if demo() {
        None
    } else {
        let endpoint = nd_ipc::Endpoint::for_app(formalmusic_api::settings::APP, "instance");
        match runtime.block_on(nd_ipc::instance::acquire(
            &endpoint,
            &nd_ipc::instance::Launch::from_env(),
        )) {
            Ok(nd_ipc::instance::Instance::Forwarded) => return Ok(()),
            Ok(nd_ipc::instance::Instance::Primary(primary)) => Some(primary),
            Err(e) => {
                eprintln!("formalmusic: single instance: {e}");
                None
            }
        }
    };
    let settings = match formalmusic_api::settings::load() {
        Ok(handle) => Some(handle),
        Err(e) => {
            eprintln!("formalmusic: settings: {e:?}");
            None
        }
    };
    let preloaded = preload.and_then(|thread| thread.join().ok()).flatten();
    let store = build_store(&runtime, preloaded);
    let (closing, keep_playing) = (store.clone(), settings.clone());
    app().run(move |cx| {
        init(
            cx,
            Music {
                store,
                runtime: runtime.clone(),
                settings,
            },
        );
        if let Some(primary) = primary {
            raise_on_relaunch(cx, &runtime, primary);
        }
        tray::install(cx);
    })?;
    // Quitting pauses the daemon unless the settings say to keep playing; it has to reach the
    // daemon before the process exits, so this waits for the answer.
    if !keep_playing.is_some_and(|s| s.get().window.keep_playing_when_closed) {
        closing.pause_blocking(std::time::Duration::from_millis(500));
    }
    Ok(())
}

/// A second launch hands its arguments over and exits; the window comes forward, opened again
/// when it was closed behind the tray.
fn raise_on_relaunch(
    cx: &mut App,
    runtime: &tokio::runtime::Handle,
    mut primary: nd_ipc::instance::Primary,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    runtime.spawn(async move {
        while let Some(launch) = primary.next_launch().await {
            if tx.send(launch).is_err() {
                break;
            }
        }
    });
    bridge::on_main(rx, move |_launch, cx| show_window(cx));
    let _ = cx;
}

/// The window over the demo catalog with no daemon, for tests: no settings file, no single
/// instance.
pub fn open_demo(cx: &mut App) -> WindowHandle<Root> {
    // Safety: tests call this before any thread reads the environment.
    unsafe { std::env::set_var("FORMALMUSIC_DEMO", "1") };
    let runtime = runtime();
    let store = build_store(&runtime, None);
    init(
        cx,
        Music {
            store,
            runtime,
            settings: None,
        },
    )
}
