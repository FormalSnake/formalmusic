//! The tray item (the menu bar on macOS, a StatusNotifierItem on Linux, the notification area
//! on Windows): the playing track, play or pause, next and previous, the window and Quit. While
//! it shows, the app keeps running behind it with the window closed, as the old daemon's tray
//! did; `window.show_in_tray` turns it off.

use formalmusic_api::Status;
use nd::prelude::*;
use nd::system::{SystemEvent, Tray, TrayIcon, TrayId, TrayMenuItem};

const ID: &str = "formalmusic";

/// The item as last sent, and whether the OS took it.
#[derive(Default)]
struct Shown {
    tray: Option<Tray>,
    live: bool,
}

/// Shows the item and answers its menu. Call once, after the window opens.
pub fn install(cx: &mut App) {
    cx.set_global(Shown::default());
    nd::system::on_event(&nd::system::system(), |event, cx| match event {
        SystemEvent::Tray { tray, item } if tray.0 == ID => chosen(item.as_deref(), cx),
        // The Dock icon on macOS, clicked with the window closed.
        SystemEvent::Activated if cx.window_ids().next().is_none() => crate::show_window(cx),
        _ => {}
    });
    sync(cx);
}

/// Whether the app runs on behind the item once the window closes.
pub fn keeps_running(cx: &App) -> bool {
    cx.try_global::<Shown>().is_some_and(|s| s.live)
}

/// Brings the item in step with the setting and the playing track.
pub fn sync(cx: &mut App) {
    if !cx.has_global::<Shown>() {
        return;
    }
    let tray = crate::settings::current(cx).window.show_in_tray.then(|| tray(cx));
    if cx.global::<Shown>().tray == tray {
        return;
    }
    cx.global_mut::<Shown>().tray = tray.clone();
    let system = nd::system::system();
    match tray {
        Some(tray) => nd::system::spawn(system.tray().set(tray), |result, cx| {
            if let Err(e) = &result {
                eprintln!("formalmusic: tray: {e}");
            }
            // Without the item a closed window has no way back, so the app quits with it.
            let live = result.is_ok() && cx.global::<Shown>().tray.is_some();
            cx.global_mut::<Shown>().live = live;
            cx.set_quit_when_windows_close(!live);
        }),
        None => {
            cx.global_mut::<Shown>().live = false;
            cx.set_quit_when_windows_close(true);
            nd::system::spawn(system.tray().remove(TrayId(ID.into())), |_, _| {});
        }
    }
}

fn tray(cx: &App) -> Tray {
    let store = crate::store(cx);
    let state = store.state();
    let player = &state.player;
    let playing = matches!(player.status, Status::Playing | Status::Loading);
    let mut tray = Tray::new(ID).icon(icon());
    match &player.track {
        Some(track) => {
            let artists = formalmusic_core::format::names(&track.artists);
            tray = tray
                .tooltip(if artists.is_empty() {
                    track.title.clone()
                } else {
                    format!("{} \u{2022} {artists}", track.title)
                })
                .item(TrayMenuItem::item("track", &track.title).disabled());
            if !artists.is_empty() {
                tray = tray.item(TrayMenuItem::item("artists", artists).disabled());
            }
        }
        None => {
            tray = tray
                .tooltip("FormalMusic")
                .item(TrayMenuItem::item("idle", "Not playing").disabled());
        }
    }
    let transport = |id: &str, label: &str| {
        let item = TrayMenuItem::item(id, label);
        if player.track.is_some() { item } else { item.disabled() }
    };
    tray.item(TrayMenuItem::Separator)
        .item(transport("toggle", if playing { "Pause" } else { "Play" }))
        .item(transport("next", "Next"))
        .item(transport("previous", "Previous"))
        .item(TrayMenuItem::Separator)
        .item(TrayMenuItem::item("open", "Open FormalMusic"))
        .item(TrayMenuItem::item("quit", "Quit FormalMusic"))
}

/// A note in the menu bar, as Music's own extras draw; the app's icon elsewhere.
fn icon() -> TrayIcon {
    if cfg!(target_os = "macos") {
        TrayIcon::Named("music.note".into())
    } else {
        TrayIcon::Named(nd::manifest!().id.unwrap_or(ID).into())
    }
}

fn chosen(item: Option<&str>, cx: &mut App) {
    let store = crate::store(cx);
    match item {
        // A click on the icon itself.
        None | Some("open") => crate::show_window(cx),
        Some("toggle") => store.toggle(),
        Some("next") => store.next(),
        Some("previous") => store.previous(),
        Some("quit") => {
            if !crate::settings::current(cx).window.keep_playing_when_closed {
                store.pause();
            }
            cx.quit();
        }
        _ => {}
    }
}
