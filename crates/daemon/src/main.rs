//! `formalmusicd`: the FormalMusic playback daemon. It owns the YouTube
//! session, the queue and the audio engine, and serves clients over the
//! socket from [`formalmusic_api::socket_path`].
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod daemon;
mod extras;
#[cfg(target_os = "linux")]
mod mpris;
mod playback;
mod playlist;
mod queue;
mod scrobble;
mod server;
mod session;
#[cfg(windows)]
mod shell;
mod signin;
mod streams;
mod tracking;
#[cfg(target_os = "linux")]
mod tray;

use anyhow::Context;
use config::Paths;
use formalmusic_player::{OutputKind, Player};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let paths = Paths::from_env();
    #[cfg(not(windows))]
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_env_filter(log_filter())
        .init();
    // config.toml, imported once from the old config.json and daemon.json.
    let settings = formalmusic_api::settings::load()
        .inspect_err(|e| tracing::warn!("settings: {e:?}; playing with the defaults"))
        .ok();
    let socket = formalmusic_api::socket_path();

    // The lock settles two daemons starting at once; the Hello probe in
    // `bind` then tells a live daemon from a stale socket. A named pipe is
    // its own lock.
    #[cfg(unix)]
    let lock = {
        if let Some(dir) = socket.parent() {
            config::create_private_dir(dir)?;
        }
        let lock_path = socket.with_extension("lock");
        let lock = std::fs::File::create(&lock_path)
            .with_context(|| format!("creating {}", lock_path.display()))?;
        if lock.try_lock().is_err() {
            anyhow::bail!(
                "formalmusicd is already running (lock held on {})",
                lock_path.display()
            );
        }
        lock
    };
    let listener = server::bind(&socket).await?;
    // A GUI-subsystem program on Windows has no stderr, so the log goes to a
    // file there, started afresh by the daemon that won the pipe.
    #[cfg(windows)]
    {
        config::create_private_dir(&paths.state)?;
        let log = std::fs::File::create(paths.state.join("formalmusicd.log"))?;
        tracing_subscriber::fmt()
            .with_writer(std::sync::Mutex::new(log))
            .with_ansi(false)
            .with_env_filter(log_filter())
            .init();
    }

    // `FORMALMUSIC_AUDIO=null` plays into a sink that keeps real time and
    // discards the sound, for headless runs that measure the app.
    let player = match std::env::var("FORMALMUSIC_AUDIO").as_deref() {
        Ok("null") => Player::with_output(OutputKind::Null {
            sample_rate: 48_000,
            channels: 2,
        }),
        _ => Player::new(),
    }
    .context("starting the audio engine")?;
    let daemon = daemon::Daemon::new(&paths, settings.clone(), player)?;
    // A save from the Settings window or a Home Manager switch applies at once.
    let _watch = settings.as_ref().and_then(|settings| {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        settings.on_event(move |event| {
            if let nd_settings::Event::Changed(change) = event {
                let _ = tx.send(change.new.clone());
            }
        });
        let daemon = daemon.clone();
        tokio::spawn(async move {
            while let Some(new) = rx.recv().await {
                daemon.apply_settings(&new);
            }
        });
        settings
            .watch()
            .inspect_err(|e| tracing::warn!("watching config.toml: {e}"))
            .ok()
    });
    tracing::info!(socket = %socket.display(), version = env!("CARGO_PKG_VERSION"), "formalmusicd listening");

    tokio::spawn({
        let daemon = daemon.clone();
        async move { daemon.session_upkeep().await }
    });
    #[cfg(target_os = "linux")]
    let _mpris = match mpris::start(daemon.playback.clone(), daemon.events.subscribe()).await {
        Ok(mpris) => Some(mpris),
        Err(e) => {
            tracing::warn!("MPRIS unavailable: {e}");
            None
        }
    };
    // The tray's Quit: pause and exit 0, which `Restart=on-failure` leaves
    // stopped.
    let quit = std::sync::Arc::new(tokio::sync::Notify::new());
    #[cfg(target_os = "linux")]
    tokio::spawn(tray::run(
        daemon.playback.clone(),
        daemon.events.subscribe(),
        daemon.tray.subscribe(),
        quit.clone(),
    ));
    #[cfg(windows)]
    let _shell = shell::start(
        daemon.playback.clone(),
        daemon.events.subscribe(),
        daemon.tray.subscribe(),
        quit.clone(),
    )
    .inspect_err(|e| tracing::warn!("shell integration unavailable: {e}"))
    .ok();
    let server = tokio::spawn(server::serve(listener, daemon.clone()));

    tokio::select! {
        _ = terminated() => {}
        _ = quit.notified() => {
            tracing::info!("quit from the tray, shutting down");
            daemon.playback.pause("tray quit");
            let _ = daemon.events.send(formalmusic_api::Event::Quit);
            // Long enough for subscribed windows to read it before the
            // socket goes away.
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
    }
    server.abort();
    daemon.playback.save_now();
    #[cfg(unix)]
    {
        let _ = std::fs::remove_file(&socket);
        drop(lock);
    }
    Ok(())
}

fn log_filter() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,formalmusic_innertube=warn".into())
}

#[cfg(unix)]
async fn terminated() {
    use tokio::signal::unix::{SignalKind, signal};
    let (Ok(mut term), Ok(mut int)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        return std::future::pending().await;
    };
    tokio::select! {
        _ = term.recv() => tracing::info!("SIGTERM, shutting down"),
        _ = int.recv() => tracing::info!("SIGINT, shutting down"),
    }
}

/// Sign-out and shutdown reach the daemon through its shell window
/// (`WM_ENDSESSION`), since a GUI process gets no console events.
#[cfg(windows)]
async fn terminated() {
    std::future::pending().await
}
