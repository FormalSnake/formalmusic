//! One window per session. The first launch holds a lock and listens on a
//! socket beside it; a later launch finds the lock taken, asks the first one to
//! raise its window, and exits.

use tokio::sync::mpsc::UnboundedReceiver;

pub enum Launch {
    /// Run the app. The receiver yields once per later launch; there is none
    /// when single instance is off (demo data, screenshots).
    First(Option<UnboundedReceiver<()>>),
    /// Another instance has been asked to come forward.
    Handed,
}

#[cfg(unix)]
pub fn claim() -> Launch {
    use std::fs::{File, TryLockError};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    // Demo runs (the screenshot script, the headless checks) sit beside a real
    // session and must not hand over to it.
    let off = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    if std::env::var("FORMALMUSIC_DEMO").as_deref() == Ok("1") || off("FORMALMUSIC_SCREENSHOT") {
        return Launch::First(None);
    }

    // Beside the daemon socket, so a window talking to another daemon
    // (FORMALMUSIC_SOCKET, as test runs set it) never takes over this one.
    let dir = formalmusic_core::kopuz::socket_path()
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(formalmusic_core::paths::cache_dir);
    if std::fs::create_dir_all(&dir).is_err() {
        return Launch::First(None);
    }
    let socket = dir.join("instance.sock");
    let Ok(lock) = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("instance.lock"))
    else {
        return Launch::First(None);
    };

    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            // The first instance may still be starting up and not listening
            // yet, so give it a moment before giving up on raising it.
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if let Ok(mut stream) = UnixStream::connect(&socket) {
                    let _ = stream.write_all(b"activate\n");
                    return Launch::Handed;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            return Launch::Handed;
        }
        Err(TryLockError::Error(_)) => return Launch::First(None),
    }

    // Holding the lock means any socket file left here is from a crashed run.
    let _ = std::fs::remove_file(&socket);
    let Ok(listener) = UnixListener::bind(&socket) else {
        return Launch::First(None);
    };
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("formalmusic-instance".into())
        .spawn(move || {
            // The lock lives as long as this thread, which is the process.
            let _lock = lock;
            for stream in listener.incoming().flatten() {
                let mut line = String::new();
                let _ = BufReader::new(stream).read_line(&mut line);
                if line.trim() == "activate" && sender.send(()).is_err() {
                    break;
                }
            }
        })
        .map_or(Launch::First(None), |_| Launch::First(Some(receiver)))
}

/// The first launch serves a named pipe beside the daemon's; creating the
/// pipe's first instance is the lock.
#[cfg(windows)]
pub fn claim() -> Launch {
    use formalmusic_core::model::local;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let off = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    if std::env::var("FORMALMUSIC_DEMO").as_deref() == Ok("1") || off("FORMALMUSIC_SCREENSHOT") {
        return Launch::First(None);
    }
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return Launch::First(None);
    };
    let pipe = std::path::PathBuf::from(format!(
        "{}-instance",
        formalmusic_core::kopuz::socket_path().display()
    ));
    let listener = runtime.block_on(async { local::Listener::bind(&pipe) });
    let mut listener = match listener {
        Ok(listener) => listener,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            // Windows only lets the process the user is acting on take the
            // foreground, so this one hands its right to the first instance.
            // SAFETY: no pointers involved.
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
                    windows_sys::Win32::UI::WindowsAndMessaging::ASFW_ANY,
                );
            }
            runtime.block_on(async {
                let raise = async {
                    let mut stream = local::connect(&pipe).await?;
                    stream.write_all(b"activate\n").await?;
                    stream.flush().await
                };
                let _ = tokio::time::timeout(std::time::Duration::from_secs(3), raise).await;
            });
            return Launch::Handed;
        }
        Err(_) => return Launch::First(None),
    };
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("formalmusic-instance".into())
        .spawn(move || {
            runtime.block_on(async move {
                while let Ok(stream) = listener.accept().await {
                    let mut line = String::new();
                    let _ = BufReader::new(stream).read_line(&mut line).await;
                    if line.trim() == "activate" && sender.send(()).is_err() {
                        break;
                    }
                }
            })
        })
        .map_or(Launch::First(None), |_| Launch::First(Some(receiver)))
}

#[cfg(not(any(unix, windows)))]
pub fn claim() -> Launch {
    Launch::First(None)
}
