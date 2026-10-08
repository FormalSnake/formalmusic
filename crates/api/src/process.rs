//! Child processes that never open a console window on Windows, and find
//! the install's own python, deno and ffmpeg there first.

use std::ffi::OsStr;

/// Both binaries are GUI-subsystem programs on Windows, so every console
/// program they start (python, ffmpeg) gets a window of its own without this.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn command(program: impl AsRef<OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut command = std::process::Command::new(program);
    #[cfg(windows)]
    {
        std::os::windows::process::CommandExt::creation_flags(&mut command, CREATE_NO_WINDOW);
        // std looks the program up in the child's PATH when one is set.
        if let Some(path) = runtime_path() {
            command.env("PATH", path);
        }
    }
    command
}

#[cfg(feature = "io")]
pub fn async_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    tokio::process::Command::from(command(program))
}

/// `PATH` with `runtime` and `runtime\python` beside the binaries in front,
/// where the Windows installer puts python with yt-dlp, deno and ffmpeg.
/// `None` for a cargo run, which uses whatever is on `PATH`.
#[cfg(windows)]
fn runtime_path() -> Option<&'static std::ffi::OsString> {
    static PATH: std::sync::OnceLock<Option<std::ffi::OsString>> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let runtime = std::env::current_exe().ok()?.parent()?.join("runtime");
        if !runtime.is_dir() {
            return None;
        }
        let mut dirs = vec![runtime.clone(), runtime.join("python")];
        dirs.extend(std::env::var_os("PATH").map_or_else(Vec::new, |path| {
            std::env::split_paths(&path).collect()
        }));
        std::env::join_paths(dirs).ok()
    })
    .as_ref()
}
