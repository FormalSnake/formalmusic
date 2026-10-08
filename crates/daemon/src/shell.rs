//! The daemon's place in the Windows shell, what MPRIS and the tray are on
//! Linux: the System Media Transport Controls (media keys, the volume flyout
//! and the lock screen), a notification area icon while a track is loaded and
//! `showInTray` is on, and a save of the queue when Windows signs out.
//!
//! All of it hangs off one hidden window on a thread of its own that pumps
//! its messages. Playback changes reach that thread over a channel and a
//! posted wake message; button presses go back to a tokio task, since
//! skipping a track starts async work.

use crate::playback::{Playback, large_art};
use formalmusic_api::{Event, PlayerState, Repeat, Status};
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::mpsc as std_mpsc;
use tokio::sync::{Notify, broadcast, mpsc, watch};
use windows::Foundation::{TimeSpan, TypedEventHandler, Uri};
use windows::Media::{
    MediaPlaybackAutoRepeatMode, MediaPlaybackStatus, MediaPlaybackType,
    SystemMediaTransportControls, SystemMediaTransportControlsButton,
    SystemMediaTransportControlsTimelineProperties,
};
use windows::Storage::Streams::RandomAccessStreamReference;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::WinRT::{
    ISystemMediaTransportControlsInterop, RO_INIT_MULTITHREADED, RoInitialize,
};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    SetCurrentProcessExplicitAppUserModelID, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

const APP_ID: &str = "es.canarycoders.formalmusic";
/// Posted when `Update`s are waiting in the channel.
const WM_WAKE: u32 = WM_APP;
/// The notification area icon's mouse messages.
const WM_TRAY: u32 = WM_APP + 1;
const TRAY_ID: u32 = 1;

enum Action {
    Toggle,
    Play,
    Pause,
    Next,
    Previous,
    Seek(u64),
    Shuffle(bool),
    Repeat(Repeat),
    Open,
    Quit,
}

enum Update {
    Player {
        state: PlayerState,
        position_ms: u64,
        can_next: bool,
        tray: bool,
    },
    Close,
}

/// Owns the shell thread; dropping it removes the icon and ends the thread.
pub struct Shell {
    hwnd: isize,
    updates: std_mpsc::Sender<Update>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Shell {
    fn drop(&mut self) {
        let _ = self.updates.send(Update::Close);
        wake(self.hwnd);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn wake(hwnd: isize) {
    // SAFETY: posting to a window that may already be gone only fails.
    unsafe {
        let _ = PostMessageW(
            Some(HWND(hwnd as *mut _)),
            WM_WAKE,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

pub fn start(
    playback: Arc<Playback>,
    events: broadcast::Receiver<Event>,
    shown: watch::Receiver<bool>,
    quit: Arc<Notify>,
) -> anyhow::Result<Shell> {
    // The media flyout names the app through the Start menu shortcut with
    // this id, which the installer writes.
    // SAFETY: a static string.
    unsafe {
        let _ = SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(APP_ID));
    }
    let (actions, mut requests) = mpsc::unbounded_channel();
    let (updates, inbox) = std_mpsc::channel();
    let (ready, window) = std_mpsc::channel();
    let saver = playback.clone();
    let thread = std::thread::Builder::new()
        .name("formalmusicd-shell".into())
        .spawn(move || {
            let on_end = Box::new(move || saver.save_now());
            match create(actions, inbox, on_end) {
                Ok(hwnd) => {
                    let _ = ready.send(Ok(hwnd.0 as isize));
                    pump();
                }
                Err(e) => {
                    let _ = ready.send(Err(e));
                }
            }
        })?;
    let hwnd = window.recv()??;

    tokio::spawn({
        let playback = playback.clone();
        async move {
            while let Some(action) = requests.recv().await {
                match action {
                    Action::Toggle => playback.toggle("media keys"),
                    Action::Play => playback.resume(),
                    Action::Pause => playback.pause("media keys"),
                    Action::Next => playback.next(),
                    Action::Previous => playback.previous(),
                    Action::Seek(position_ms) => playback.seek(position_ms),
                    Action::Shuffle(on) => playback.set_shuffle(on),
                    Action::Repeat(repeat) => playback.set_repeat(repeat),
                    Action::Open => open_app(),
                    Action::Quit => quit.notify_one(),
                }
            }
        }
    });
    tokio::spawn(follow(playback, events, shown, updates.clone(), hwnd));
    Ok(Shell {
        hwnd,
        updates,
        thread: Some(thread),
    })
}

/// Sends the player's state on every player, queue or seek change and when
/// the tray setting flips.
async fn follow(
    playback: Arc<Playback>,
    mut events: broadcast::Receiver<Event>,
    mut shown: watch::Receiver<bool>,
    updates: std_mpsc::Sender<Update>,
    hwnd: isize,
) {
    let mut seeks = playback.subscribe_seeks();
    loop {
        let update = Update::Player {
            state: playback.player_state(),
            position_ms: playback.live_position(),
            can_next: playback.can_go_next(),
            tray: *shown.borrow_and_update(),
        };
        if updates.send(update).is_err() {
            return;
        }
        wake(hwnd);
        loop {
            tokio::select! {
                event = events.recv() => match event {
                    Ok(Event::Player(_) | Event::Queue(_))
                    | Err(broadcast::error::RecvError::Lagged(_)) => break,
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                seek = seeks.recv() => match seek {
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => break,
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                changed = shown.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    break;
                }
            }
        }
    }
}

/// What the tray shows, kept to compare against the next update.
#[derive(Clone, PartialEq)]
struct NowPlaying {
    title: String,
    artist: String,
    playing: bool,
}

struct State {
    hwnd: HWND,
    actions: mpsc::UnboundedSender<Action>,
    inbox: std_mpsc::Receiver<Update>,
    on_end: Box<dyn Fn()>,
    smtc: Option<SystemMediaTransportControls>,
    art: Option<String>,
    tray: Option<NowPlaying>,
    taskbar_created: u32,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn create(
    actions: mpsc::UnboundedSender<Action>,
    inbox: std_mpsc::Receiver<Update>,
    on_end: Box<dyn Fn()>,
) -> anyhow::Result<HWND> {
    // SAFETY: plain Win32 calls on this thread; the class and window live
    // until the thread ends.
    unsafe {
        RoInitialize(RO_INIT_MULTITHREADED)?;
        let instance = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance.into(),
            lpszClassName: w!("FormalMusicShell"),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 {
            anyhow::bail!("registering the shell window class failed");
        }
        // A real top-level window, never shown: the media controls and
        // sign-out messages need one, and a message-only window gets neither.
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            w!("FormalMusicShell"),
            w!("FormalMusic"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let smtc = media_controls(hwnd, &actions)
            .inspect_err(|e| tracing::warn!("media controls unavailable: {e}"))
            .ok();
        let taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));
        STATE.with_borrow_mut(|state| {
            *state = Some(State {
                hwnd,
                actions,
                inbox,
                on_end,
                smtc,
                art: None,
                tray: None,
                taskbar_created,
            })
        });
        Ok(hwnd)
    }
}

fn pump() {
    let mut message = MSG::default();
    // SAFETY: the standard message loop.
    unsafe {
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

fn media_controls(
    hwnd: HWND,
    actions: &mpsc::UnboundedSender<Action>,
) -> windows::core::Result<SystemMediaTransportControls> {
    let interop = windows::core::factory::<
        SystemMediaTransportControls,
        ISystemMediaTransportControlsInterop,
    >()?;
    // SAFETY: `hwnd` is this thread's live window.
    let smtc: SystemMediaTransportControls = unsafe { interop.GetForWindow(hwnd)? };
    smtc.SetIsEnabled(true)?;
    smtc.SetIsPlayEnabled(true)?;
    smtc.SetIsPauseEnabled(true)?;
    smtc.SetIsStopEnabled(true)?;
    smtc.SetIsNextEnabled(true)?;
    smtc.SetIsPreviousEnabled(true)?;
    smtc.DisplayUpdater()?.SetType(MediaPlaybackType::Music)?;

    let send = actions.clone();
    smtc.ButtonPressed(&TypedEventHandler::new(move |_, args| {
        let args: &windows::Media::SystemMediaTransportControlsButtonPressedEventArgs =
            windows::core::Ref::ok(&args)?;
        let action = match args.Button()? {
            SystemMediaTransportControlsButton::Play => Action::Play,
            SystemMediaTransportControlsButton::Pause
            | SystemMediaTransportControlsButton::Stop => Action::Pause,
            SystemMediaTransportControlsButton::Next => Action::Next,
            SystemMediaTransportControlsButton::Previous => Action::Previous,
            _ => return Ok(()),
        };
        let _ = send.send(action);
        Ok(())
    }))?;
    let send = actions.clone();
    smtc.PlaybackPositionChangeRequested(&TypedEventHandler::new(move |_, args| {
        let args: &windows::Media::PlaybackPositionChangeRequestedEventArgs =
            windows::core::Ref::ok(&args)?;
        let ticks = args.RequestedPlaybackPosition()?.Duration.max(0);
        let _ = send.send(Action::Seek(ticks as u64 / 10_000));
        Ok(())
    }))?;
    let send = actions.clone();
    smtc.ShuffleEnabledChangeRequested(&TypedEventHandler::new(move |_, args| {
        let args: &windows::Media::ShuffleEnabledChangeRequestedEventArgs =
            windows::core::Ref::ok(&args)?;
        let _ = send.send(Action::Shuffle(args.RequestedShuffleEnabled()?));
        Ok(())
    }))?;
    let send = actions.clone();
    smtc.AutoRepeatModeChangeRequested(&TypedEventHandler::new(move |_, args| {
        let args: &windows::Media::AutoRepeatModeChangeRequestedEventArgs =
            windows::core::Ref::ok(&args)?;
        let repeat = match args.RequestedAutoRepeatMode()? {
            MediaPlaybackAutoRepeatMode::Track => Repeat::One,
            MediaPlaybackAutoRepeatMode::List => Repeat::All,
            _ => Repeat::Off,
        };
        let _ = send.send(Action::Repeat(repeat));
        Ok(())
    }))?;
    Ok(smtc)
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let handled = STATE.with(|cell| {
        // A message sent while a menu is up or during creation finds the
        // state borrowed or empty and takes the default path.
        let Ok(mut borrowed) = cell.try_borrow_mut() else {
            return None;
        };
        let state = borrowed.as_mut()?;
        match message {
            WM_WAKE => {
                drain(state);
                Some(LRESULT(0))
            }
            WM_TRAY => {
                match (lparam.0 & 0xffff) as u32 {
                    WM_LBUTTONUP => {
                        let _ = state.actions.send(Action::Open);
                    }
                    WM_RBUTTONUP | WM_CONTEXTMENU => {
                        let now = state.tray.clone();
                        let actions = state.actions.clone();
                        drop(borrowed);
                        if let Some(now) = now {
                            menu(hwnd, &now, &actions);
                        }
                    }
                    _ => {}
                }
                Some(LRESULT(0))
            }
            WM_QUERYENDSESSION => Some(LRESULT(1)),
            WM_ENDSESSION => {
                if wparam.0 != 0 {
                    tracing::info!("windows is signing out, saving the queue");
                    (state.on_end)();
                }
                Some(LRESULT(0))
            }
            m if m == state.taskbar_created => {
                // Explorer restarted and forgot the icon.
                if let Some(now) = state.tray.clone() {
                    tray_icon(state.hwnd, NIM_ADD, &now);
                }
                Some(LRESULT(0))
            }
            _ => None,
        }
    });
    // SAFETY: forwarding the message this procedure was called with.
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, message, wparam, lparam) })
}

fn drain(state: &mut State) {
    let mut latest = None;
    while let Ok(update) = state.inbox.try_recv() {
        match update {
            Update::Close => {
                if state.tray.take().is_some() {
                    remove_tray_icon(state.hwnd);
                }
                // SAFETY: ends this thread's message loop.
                unsafe { PostQuitMessage(0) };
                return;
            }
            player => latest = Some(player),
        }
    }
    let Some(Update::Player {
        state: player,
        position_ms,
        can_next,
        tray,
    }) = latest
    else {
        return;
    };
    if let Some(smtc) = &state.smtc
        && let Err(e) = update_media_controls(smtc, &mut state.art, &player, position_ms, can_next)
    {
        tracing::debug!("media controls: {e}");
    }
    let now = player.track.as_ref().filter(|_| tray).map(|track| NowPlaying {
        title: track.title.clone(),
        artist: track
            .artists
            .iter()
            .map(|a| a.text.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        playing: matches!(player.status, Status::Playing | Status::Loading),
    });
    match (&state.tray, now) {
        (Some(shown), Some(now)) if *shown != now => {
            tray_icon(state.hwnd, NIM_MODIFY, &now);
            state.tray = Some(now);
        }
        (Some(_), Some(_)) => {}
        (None, Some(now)) => {
            tray_icon(state.hwnd, NIM_ADD, &now);
            state.tray = Some(now);
        }
        (Some(_), None) => {
            remove_tray_icon(state.hwnd);
            state.tray = None;
        }
        (None, None) => {}
    }
}

fn update_media_controls(
    smtc: &SystemMediaTransportControls,
    art: &mut Option<String>,
    player: &PlayerState,
    position_ms: u64,
    can_next: bool,
) -> windows::core::Result<()> {
    smtc.SetPlaybackStatus(match player.status {
        Status::Playing => MediaPlaybackStatus::Playing,
        Status::Loading => MediaPlaybackStatus::Changing,
        Status::Paused => MediaPlaybackStatus::Paused,
        Status::Stopped => MediaPlaybackStatus::Stopped,
    })?;
    smtc.SetIsNextEnabled(can_next)?;
    smtc.SetIsPreviousEnabled(player.track.is_some())?;
    smtc.SetShuffleEnabled(player.shuffle)?;
    smtc.SetAutoRepeatMode(match player.repeat {
        Repeat::Off => MediaPlaybackAutoRepeatMode::None,
        Repeat::All => MediaPlaybackAutoRepeatMode::List,
        Repeat::One => MediaPlaybackAutoRepeatMode::Track,
    })?;

    let updater = smtc.DisplayUpdater()?;
    let Some(track) = &player.track else {
        updater.ClearAll()?;
        updater.SetType(MediaPlaybackType::Music)?;
        *art = None;
        return updater.Update();
    };
    updater.SetType(MediaPlaybackType::Music)?;
    let music = updater.MusicProperties()?;
    music.SetTitle(&HSTRING::from(track.title.as_str()))?;
    let artists = track
        .artists
        .iter()
        .map(|a| a.text.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    music.SetArtist(&HSTRING::from(artists))?;
    music.SetAlbumTitle(&HSTRING::from(
        track.album.as_ref().map_or("", |a| a.text.as_str()),
    ))?;
    let url = track.thumbnails.last().map(|t| large_art(&t.url));
    // Setting the same thumbnail again refetches it and flickers the flyout.
    if url != *art {
        match &url {
            Some(url) => updater.SetThumbnail(&RandomAccessStreamReference::CreateFromUri(
                &Uri::CreateUri(&HSTRING::from(url.as_str()))?,
            )?)?,
            None => updater.SetThumbnail(None)?,
        }
        *art = url;
    }
    updater.Update()?;

    let ticks = |ms: u64| TimeSpan {
        Duration: ms as i64 * 10_000,
    };
    let timeline = SystemMediaTransportControlsTimelineProperties::new()?;
    let duration = player.duration_ms.unwrap_or(0);
    timeline.SetStartTime(ticks(0))?;
    timeline.SetMinSeekTime(ticks(0))?;
    timeline.SetEndTime(ticks(duration))?;
    timeline.SetMaxSeekTime(ticks(duration))?;
    timeline.SetPosition(ticks(position_ms.min(duration.max(position_ms))))?;
    smtc.UpdateTimelineProperties(&timeline)
}

fn tray_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        ..Default::default()
    }
}

fn tray_icon(hwnd: HWND, verb: windows::Win32::UI::Shell::NOTIFY_ICON_MESSAGE, now: &NowPlaying) {
    let mut data = tray_data(hwnd);
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    data.uCallbackMessage = WM_TRAY;
    // SAFETY: the icon embedded by build.rs, resource 1, at the small size.
    data.hIcon = unsafe {
        GetModuleHandleW(None)
            .ok()
            .and_then(|module| {
                LoadImageW(
                    Some(module.into()),
                    PCWSTR(1 as _),
                    IMAGE_ICON,
                    GetSystemMetrics(SM_CXSMICON),
                    GetSystemMetrics(SM_CYSMICON),
                    LR_DEFAULTCOLOR,
                )
                .ok()
            })
            .map(|handle| HICON(handle.0))
            .unwrap_or_default()
    };
    let tip = if now.artist.is_empty() {
        now.title.clone()
    } else {
        format!("{}\n{}", now.title, now.artist)
    };
    let tip: Vec<u16> = tip.encode_utf16().take(data.szTip.len() - 1).collect();
    data.szTip[..tip.len()].copy_from_slice(&tip);
    // SAFETY: `data` is fully initialised for this window.
    unsafe {
        let _ = Shell_NotifyIconW(verb, &data);
    }
}

fn remove_tray_icon(hwnd: HWND) {
    // SAFETY: as above.
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &tray_data(hwnd));
    }
}

const MENU_TOGGLE: usize = 1;
const MENU_NEXT: usize = 2;
const MENU_PREVIOUS: usize = 3;
const MENU_OPEN: usize = 4;
const MENU_QUIT: usize = 5;

fn menu(hwnd: HWND, now: &NowPlaying, actions: &mpsc::UnboundedSender<Action>) {
    let text = |s: &str| HSTRING::from(s.replace('&', "&&"));
    // SAFETY: a popup menu owned and destroyed here.
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, &text(&now.title));
        if !now.artist.is_empty() {
            let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, 0, &text(&now.artist));
        }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let toggle = if now.playing { w!("Pause") } else { w!("Play") };
        let _ = AppendMenuW(menu, MF_STRING, MENU_TOGGLE, toggle);
        let _ = AppendMenuW(menu, MF_STRING, MENU_NEXT, w!("Next"));
        let _ = AppendMenuW(menu, MF_STRING, MENU_PREVIOUS, w!("Previous"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
        let _ = AppendMenuW(menu, MF_STRING, MENU_OPEN, w!("Open FormalMusic"));
        let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit"));
        let _ = SetMenuDefaultItem(menu, MENU_OPEN as u32, 0);

        let mut cursor = Default::default();
        let _ = GetCursorPos(&mut cursor);
        // Without the foreground the menu stays open after a click elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
            cursor.x,
            cursor.y,
            None,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        let action = match chosen.0 as usize {
            MENU_TOGGLE => Action::Toggle,
            MENU_NEXT => Action::Next,
            MENU_PREVIOUS => Action::Previous,
            MENU_OPEN => Action::Open,
            MENU_QUIT => Action::Quit,
            _ => return,
        };
        let _ = actions.send(action);
    }
}

/// Starts the app beside the daemon, which raises the open window when
/// there is one.
fn open_app() {
    let Some(program) = std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("formalmusic.exe")))
    else {
        return;
    };
    // The click on the icon made this process the one the user is acting on;
    // passing that on lets the window come to the front.
    // SAFETY: no pointers involved.
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
    }
    match formalmusic_api::process::command(&program).spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(e) => tracing::warn!(program = %program.display(), "opening the app: {e}"),
    }
}
