//! FormalMusic's window on NativeDesktop. `formalmusic_core::MusicStore` holds every byte of
//! state and talks to the daemon; this crate is views over it. `bridge` turns the store's
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
pub mod video;

use std::sync::Arc;

use formalmusic_core::art::ArtCache;
use formalmusic_core::cache::{CachedState, StateCache};
use formalmusic_core::{MusicStore, StoreOptions, paths};
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
    let art = Arc::new(ArtCache::new(paths::art_dir(), http));
    let transport: Arc<dyn formalmusic_core::Transport> = if demo() {
        Arc::new(formalmusic_core::demo::DemoTransport::new())
    } else {
        Arc::new(formalmusic_core::client::DaemonClient::new(
            formalmusic_api::socket_path(),
        ))
    };
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
        bridge::on_main(rx, |(), cx| cx.refresh_windows());
        if let Ok(guard) = settings.watch() {
            std::mem::forget(guard);
        }
    }
    cx.set_global(music);
    bridge::install(cx, &store);
    root::bind_keys(cx);
    runtime.spawn(async move { store.start().await });
    cx.open_window(
        WindowOptions::main("main")
            .title("FormalMusic")
            .size(size(px(1280.), px(820.)))
            .min_size(size(px(880.), px(560.))),
        |window, cx| cx.new(|cx| Root::new(window, cx)),
    )
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
        let window = init(
            cx,
            Music {
                store,
                runtime: runtime.clone(),
                settings,
            },
        );
        if let Some(primary) = primary {
            raise_on_relaunch(cx, &runtime, primary, window.id());
        }
    })?;
    // Quitting pauses the daemon unless the settings say to keep playing; it has to reach the
    // daemon before the process exits, so this waits for the answer.
    if !keep_playing.is_some_and(|s| s.get().window.keep_playing_when_closed) {
        closing.pause_blocking(std::time::Duration::from_millis(500));
    }
    Ok(())
}

/// A second launch hands its arguments over and exits; the running window comes forward.
fn raise_on_relaunch(
    cx: &mut App,
    runtime: &tokio::runtime::Handle,
    mut primary: nd_ipc::instance::Primary,
    window: nd::core::WindowId,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    runtime.spawn(async move {
        while let Some(launch) = primary.next_launch().await {
            if tx.send(launch).is_err() {
                break;
            }
        }
    });
    bridge::on_main(rx, move |_launch, cx| cx.activate_window(window));
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
