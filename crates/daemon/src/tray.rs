//! The tray icon (StatusNotifierItem): the playing track, transport controls,
//! a way back to the window and a way to stop the daemon. It lives here
//! rather than in the app so it stays while no window is open, and shows
//! whenever a track is loaded and the app's `showInTray` is on.

use crate::playback::Playback;
use formalmusic_api::{Event, Status};
use ksni::menu::StandardItem;
use ksni::{MenuItem, ToolTip, TrayMethods};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Notify, broadcast, watch};

const APP_ID: &str = "es.canarycoders.formalmusic";

#[derive(Debug, Clone, PartialEq, Default)]
struct NowPlaying {
    title: String,
    artist: String,
    playing: bool,
}

impl NowPlaying {
    fn of(playback: &Playback) -> Option<Self> {
        let state = playback.player_state();
        let track = state.track?;
        Some(Self {
            title: track.title,
            artist: track
                .artists
                .iter()
                .map(|a| a.text.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            playing: matches!(state.status, Status::Playing | Status::Loading),
        })
    }

    fn line(&self) -> String {
        if self.artist.is_empty() {
            self.title.clone()
        } else {
            format!("{} \u{2022} {}", self.title, self.artist)
        }
    }
}

struct Tray {
    playback: Arc<Playback>,
    now: NowPlaying,
    quit: Arc<Notify>,
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        APP_ID.into()
    }

    fn title(&self) -> String {
        "FormalMusic".into()
    }

    fn icon_name(&self) -> String {
        APP_ID.into()
    }

    fn icon_theme_path(&self) -> String {
        icon_theme_path()
            .map(|path| path.display().to_string())
            .unwrap_or_default()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: self.now.line(),
            ..ToolTip::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        open_app();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let info = |text: &str| -> MenuItem<Self> {
            StandardItem {
                label: escape(text),
                enabled: false,
                ..StandardItem::default()
            }
            .into()
        };
        let action = |label: &str, run: fn(&mut Self)| -> MenuItem<Self> {
            StandardItem {
                label: label.into(),
                activate: Box::new(run),
                ..StandardItem::default()
            }
            .into()
        };
        let mut items = vec![info(&self.now.title)];
        if !self.now.artist.is_empty() {
            items.push(info(&self.now.artist));
        }
        items.extend([
            MenuItem::Separator,
            action(if self.now.playing { "Pause" } else { "Play" }, |tray| {
                tray.playback.toggle()
            }),
            action("Next", |tray| tray.playback.next()),
            action("Previous", |tray| tray.playback.previous()),
            MenuItem::Separator,
            action("Open FormalMusic", |_| open_app()),
            action("Quit", |tray| tray.quit.notify_one()),
        ]);
        items
    }
}

/// Keeps the icon in step with `shown` and the playing track until the
/// event stream closes.
pub async fn run(
    playback: Arc<Playback>,
    mut events: broadcast::Receiver<Event>,
    mut shown: watch::Receiver<bool>,
    quit: Arc<Notify>,
) {
    let mut handle: Option<ksni::Handle<Tray>> = None;
    let mut last: Option<NowPlaying> = None;
    // A failed spawn (no session bus, no watcher) waits for the setting to
    // change instead of retrying on every event.
    let mut failed = false;
    loop {
        let now = NowPlaying::of(&playback).filter(|_| *shown.borrow());
        match (&handle, now.clone()) {
            (Some(tray), Some(now)) if last.as_ref() != Some(&now) => {
                tray.update(|tray| tray.now = now).await;
            }
            (Some(_), Some(_)) => {}
            (Some(tray), None) => {
                tray.shutdown().await;
                handle = None;
            }
            (None, Some(now)) if !failed => {
                let tray = Tray {
                    playback: playback.clone(),
                    now,
                    quit: quit.clone(),
                };
                match tray.spawn().await {
                    Ok(tray) => handle = Some(tray),
                    Err(e) => {
                        tracing::warn!("tray unavailable: {e}");
                        failed = true;
                    }
                }
            }
            (None, _) => {}
        }
        last = now;
        // Only a player change or the setting can change what the tray shows.
        loop {
            tokio::select! {
                event = events.recv() => match event {
                    Ok(Event::Player(_)) | Err(broadcast::error::RecvError::Lagged(_)) => break,
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                changed = shown.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    failed = false;
                    break;
                }
            }
        }
    }
}

/// Underscores mark mnemonics in menu labels; a doubled one is a literal.
fn escape(text: &str) -> String {
    text.replace('_', "__")
}

/// The package's own `share/icons`, for when it is not on the theme path.
fn icon_theme_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let path = exe.parent()?.parent()?.join("share/icons");
    path.is_dir().then_some(path)
}

/// Starts the app, which raises the open window when there is one. The app
/// sits beside the daemon in the package; PATH is the fallback for a cargo
/// run. Under systemd it goes into a scope of its own: a plain child would
/// join the daemon's cgroup and be killed whenever the daemon stops.
fn open_app() {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("formalmusic")))
        .filter(|path| path.is_file());
    let program = beside.unwrap_or_else(|| PathBuf::from("formalmusic"));
    let scoped = std::env::var_os("INVOCATION_ID").is_some().then(|| {
        let mut command = std::process::Command::new("systemd-run");
        command
            .args(["--user", "--scope", "--quiet", "--"])
            .arg(&program);
        command
    });
    let spawned = scoped
        .and_then(|mut command| command.spawn().ok())
        .map_or_else(|| std::process::Command::new(&program).spawn(), Ok);
    match spawned {
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(e) => tracing::warn!(program = %program.display(), "opening the app: {e}"),
    }
}
