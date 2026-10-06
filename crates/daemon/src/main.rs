//! `formalmusicd`: the FormalMusic playback daemon. It owns the YouTube
//! session, the queue and the audio engine, and serves clients over the
//! socket from [`formalmusic_api::socket_path`].

mod config;
mod daemon;
mod extras;
#[cfg(target_os = "linux")]
mod mpris;
mod playback;
mod queue;
mod server;
mod session;
mod signin;
mod streams;
mod tracking;

use anyhow::Context;
use config::{Config, Paths};
use formalmusic_player::Player;
use tokio::signal::unix::{SignalKind, signal};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,formalmusic_innertube=warn".into()),
        )
        .init();

    let paths = Paths::from_env();
    let config = Config::load(&paths.config);
    let socket = formalmusic_api::socket_path();

    // The lock settles two daemons starting at once; the Hello probe in
    // `bind` then tells a live daemon from a stale socket.
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
    let listener = server::bind(&socket).await?;

    let player = Player::new().context("starting the audio engine")?;
    let daemon = daemon::Daemon::new(&paths, config, player)?;
    tracing::info!(socket = %socket.display(), version = env!("CARGO_PKG_VERSION"), "formalmusicd listening");

    tokio::spawn({
        let daemon = daemon.clone();
        async move { daemon.check_session().await }
    });
    #[cfg(target_os = "linux")]
    let _mpris = match mpris::start(daemon.playback.clone(), daemon.events.subscribe()).await {
        Ok(mpris) => Some(mpris),
        Err(e) => {
            tracing::warn!("MPRIS unavailable: {e}");
            None
        }
    };
    let server = tokio::spawn(server::serve(listener, daemon.clone()));

    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    tokio::select! {
        _ = term.recv() => tracing::info!("SIGTERM, shutting down"),
        _ = int.recv() => tracing::info!("SIGINT, shutting down"),
    }
    server.abort();
    daemon.playback.save_now();
    let _ = std::fs::remove_file(&socket);
    drop(lock);
    Ok(())
}
