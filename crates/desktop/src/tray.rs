//! The tray icon: the playing track, play and pause, next and previous, a
//! way back to the window and Quit. It shows while a track is loaded and
//! `showInTray` is on, over StatusNotifierItem on Linux and in the
//! notification area on Windows, and while it shows the app keeps running
//! with no window open.

#![cfg_attr(not(any(target_os = "linux", windows)), allow(dead_code))]

use formalmusic_core::MusicStore;
use formalmusic_core::model::Status;
use gpui_kit::{App, Global};
use tokio::sync::{mpsc, watch};

#[derive(Clone, Copy, Debug)]
enum Action {
    Toggle,
    Next,
    Previous,
    Open,
    Quit,
}

#[derive(Clone, Debug, PartialEq, Default)]
struct NowPlaying {
    title: String,
    artist: String,
    playing: bool,
}

impl NowPlaying {
    /// What the tray shows, or `None` while it should not show.
    fn of(store: &MusicStore, shown: bool) -> Option<Self> {
        if !shown {
            return None;
        }
        let state = store.state();
        let track = state.player.track.as_ref()?;
        Some(Self {
            title: track.title.clone(),
            artist: formalmusic_core::format::names(&track.artists),
            playing: matches!(state.player.status, Status::Playing | Status::Loading),
        })
    }
}

struct Tray {
    store: MusicStore,
    shown: watch::Sender<bool>,
}

impl Global for Tray {}

pub fn install(store: MusicStore, cx: &mut App) {
    let shown =
        formalmusic_core::settings::Settings::load(&formalmusic_core::paths::settings_file())
            .show_in_tray;
    let (shown, watching) = watch::channel(shown);
    let (actions, mut requests) = mpsc::unbounded_channel();
    platform::spawn(store.clone(), watching, actions);
    cx.set_global(Tray {
        store: store.clone(),
        shown,
    });
    cx.spawn(async move |cx| {
        while let Some(action) = requests.recv().await {
            cx.update(|cx| run(action, cx));
        }
    })
    .detach();
}

fn run(action: Action, cx: &mut App) {
    let Some(store) = cx.try_global::<Tray>().map(|tray| tray.store.clone()) else {
        return;
    };
    match action {
        Action::Toggle => store.toggle(),
        Action::Next => store.next(),
        Action::Previous => store.previous(),
        Action::Open => crate::raise(cx),
        Action::Quit => {
            store.pause_blocking(std::time::Duration::from_millis(500));
            cx.quit();
        }
    }
}

pub fn set_shown(shown: bool, cx: &mut App) {
    if let Some(tray) = cx.try_global::<Tray>() {
        tray.shown.send_replace(shown);
    }
}

/// Whether the icon is up, so closing the last window leaves the app there.
pub fn showing(cx: &App) -> bool {
    platform::SUPPORTED
        && cx
            .try_global::<Tray>()
            .is_some_and(|tray| NowPlaying::of(&tray.store, *tray.shown.borrow()).is_some())
}

/// Waits for what the tray shows to change: a player change or the setting.
async fn changed(
    events: &mut tokio::sync::broadcast::Receiver<formalmusic_core::StoreEvent>,
    shown: &mut watch::Receiver<bool>,
) -> bool {
    use formalmusic_core::StoreEvent;
    use tokio::sync::broadcast::error::RecvError;
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(StoreEvent::Player | StoreEvent::NowPlaying) | Err(RecvError::Lagged(_)) => {
                    return true;
                }
                Ok(_) => {}
                Err(RecvError::Closed) => return false,
            },
            change = shown.changed() => return change.is_ok(),
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::{Action, NowPlaying, changed};
    use formalmusic_core::MusicStore;
    use ksni::menu::StandardItem;
    use ksni::{MenuItem, ToolTip, TrayMethods};
    use tokio::sync::{mpsc, watch};

    pub const SUPPORTED: bool = true;

    struct Icon {
        now: NowPlaying,
        actions: mpsc::UnboundedSender<Action>,
    }

    impl Icon {
        fn send(&self, action: Action) {
            let _ = self.actions.send(action);
        }
    }

    impl ksni::Tray for Icon {
        fn id(&self) -> String {
            crate::APP_ID.into()
        }

        fn title(&self) -> String {
            "FormalMusic".into()
        }

        fn icon_name(&self) -> String {
            crate::APP_ID.into()
        }

        fn icon_theme_path(&self) -> String {
            icon_theme_path()
                .map(|path| path.display().to_string())
                .unwrap_or_default()
        }

        fn tool_tip(&self) -> ToolTip {
            let line = if self.now.artist.is_empty() {
                self.now.title.clone()
            } else {
                format!("{} \u{2022} {}", self.now.title, self.now.artist)
            };
            ToolTip {
                title: line,
                ..ToolTip::default()
            }
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            self.send(Action::Open);
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
            let action = |label: &str, action: Action| -> MenuItem<Self> {
                StandardItem {
                    label: label.into(),
                    activate: Box::new(move |icon: &mut Self| icon.send(action)),
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
                action(
                    if self.now.playing { "Pause" } else { "Play" },
                    Action::Toggle,
                ),
                action("Next", Action::Next),
                action("Previous", Action::Previous),
                MenuItem::Separator,
                action("Open FormalMusic", Action::Open),
                action("Quit", Action::Quit),
            ]);
            items
        }
    }

    pub fn spawn(
        store: MusicStore,
        mut shown: watch::Receiver<bool>,
        actions: mpsc::UnboundedSender<Action>,
    ) {
        let runtime = store.runtime().clone();
        runtime.spawn(async move {
            let mut events = store.events();
            let mut handle: Option<ksni::Handle<Icon>> = None;
            // A failed spawn (no session bus, no watcher) waits for the
            // setting to change instead of retrying on every event.
            let mut failed = false;
            loop {
                let now = NowPlaying::of(&store, *shown.borrow_and_update());
                match (&handle, now) {
                    (Some(icon), Some(now)) => {
                        icon.update(|icon| icon.now = now).await;
                    }
                    (Some(icon), None) => {
                        icon.shutdown().await;
                        handle = None;
                    }
                    (None, Some(now)) if !failed => {
                        let icon = Icon {
                            now,
                            actions: actions.clone(),
                        };
                        match icon.spawn().await {
                            Ok(icon) => handle = Some(icon),
                            Err(error) => {
                                eprintln!("formalmusic: no tray: {error}");
                                failed = true;
                            }
                        }
                    }
                    (None, _) => {}
                }
                let before = *shown.borrow();
                if !changed(&mut events, &mut shown).await {
                    return;
                }
                if *shown.borrow() != before {
                    failed = false;
                }
            }
        });
    }

    /// Underscores mark mnemonics in menu labels; a doubled one is a literal.
    fn escape(text: &str) -> String {
        text.replace('_', "__")
    }

    /// The package's own `share/icons`, for when it is not on the theme path.
    fn icon_theme_path() -> Option<std::path::PathBuf> {
        let exe = std::env::current_exe().ok()?;
        let path = exe.parent()?.parent()?.join("share/icons");
        path.is_dir().then_some(path)
    }
}

#[cfg(windows)]
mod platform {
    //! One hidden window on a thread of its own that pumps its messages. The
    //! icon's state reaches that thread over a channel and a posted wake
    //! message; menu choices go back to the app over `actions`.

    use super::{Action, NowPlaying, changed};
    use formalmusic_core::MusicStore;
    use std::cell::RefCell;
    use std::sync::mpsc as std_mpsc;
    use tokio::sync::{mpsc, watch};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Shell::{
        NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFY_ICON_MESSAGE,
        NOTIFYICONDATAW, Shell_NotifyIconW,
    };
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::{HSTRING, PCWSTR, w};

    pub const SUPPORTED: bool = true;

    /// Posted when updates are waiting in the channel.
    const WM_WAKE: u32 = WM_APP;
    /// The icon's mouse messages.
    const WM_TRAY: u32 = WM_APP + 1;
    const TRAY_ID: u32 = 1;

    struct State {
        hwnd: HWND,
        actions: mpsc::UnboundedSender<Action>,
        inbox: std_mpsc::Receiver<Option<NowPlaying>>,
        now: Option<NowPlaying>,
        taskbar_created: u32,
    }

    thread_local! {
        static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    }

    pub fn spawn(
        store: MusicStore,
        mut shown: watch::Receiver<bool>,
        actions: mpsc::UnboundedSender<Action>,
    ) {
        let (updates, inbox) = std_mpsc::channel();
        let (ready, window) = std_mpsc::channel();
        let started = std::thread::Builder::new()
            .name("formalmusic-tray".into())
            .spawn(move || match create(actions, inbox) {
                Ok(hwnd) => {
                    let _ = ready.send(Some(hwnd.0 as isize));
                    pump();
                }
                Err(error) => {
                    eprintln!("formalmusic: no tray: {error}");
                    let _ = ready.send(None);
                }
            });
        if started.is_err() {
            return;
        }
        let Ok(Some(hwnd)) = window.recv() else {
            return;
        };
        store.runtime().clone().spawn(async move {
            let mut events = store.events();
            loop {
                let now = NowPlaying::of(&store, *shown.borrow_and_update());
                if updates.send(now).is_err() {
                    return;
                }
                wake(hwnd);
                if !changed(&mut events, &mut shown).await {
                    return;
                }
            }
        });
    }

    fn wake(hwnd: isize) {
        // SAFETY: posting to a window that may already be gone only fails.
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_WAKE, WPARAM(0), LPARAM(0));
        }
    }

    fn create(
        actions: mpsc::UnboundedSender<Action>,
        inbox: std_mpsc::Receiver<Option<NowPlaying>>,
    ) -> windows::core::Result<HWND> {
        // SAFETY: plain Win32 calls on this thread; the class and window live
        // until the thread ends.
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance.into(),
                lpszClassName: w!("FormalMusicTray"),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 {
                return Err(windows::core::Error::from_win32());
            }
            // A real top-level window, never shown: a message-only window
            // never hears Explorer's TaskbarCreated broadcast.
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                w!("FormalMusicTray"),
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
            let taskbar_created = RegisterWindowMessageW(w!("TaskbarCreated"));
            STATE.with_borrow_mut(|state| {
                *state = Some(State {
                    hwnd,
                    actions,
                    inbox,
                    now: None,
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
                            let now = state.now.clone();
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
                m if m == state.taskbar_created => {
                    // Explorer restarted and forgot the icon.
                    if let Some(now) = state.now.clone() {
                        icon(state.hwnd, NIM_ADD, &now);
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
        let Some(now) = state.inbox.try_iter().last() else {
            return;
        };
        match (&state.now, now) {
            (Some(shown), Some(now)) if *shown != now => {
                icon(state.hwnd, NIM_MODIFY, &now);
                state.now = Some(now);
            }
            (Some(_), Some(_)) => {}
            (None, Some(now)) => {
                icon(state.hwnd, NIM_ADD, &now);
                state.now = Some(now);
            }
            (Some(_), None) => {
                remove(state.hwnd);
                state.now = None;
            }
            (None, None) => {}
        }
    }

    fn data(hwnd: HWND) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_ID,
            ..Default::default()
        }
    }

    fn icon(hwnd: HWND, verb: NOTIFY_ICON_MESSAGE, now: &NowPlaying) {
        let mut data = data(hwnd);
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

    fn remove(hwnd: HWND) {
        // SAFETY: as above.
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &data(hwnd));
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
}

#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    use super::Action;
    use formalmusic_core::MusicStore;
    use tokio::sync::{mpsc, watch};

    pub const SUPPORTED: bool = false;

    pub fn spawn(
        _store: MusicStore,
        _shown: watch::Receiver<bool>,
        _actions: mpsc::UnboundedSender<Action>,
    ) {
    }
}
