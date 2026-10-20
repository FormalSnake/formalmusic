//! Child processes that never open a console window on Windows.

use std::ffi::OsStr;

/// The window is a GUI-subsystem program on Windows, so every console program
/// it starts (ffmpeg) gets a window of its own without this.
#[cfg(windows)]
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn command(program: impl AsRef<OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut command = std::process::Command::new(program);
    #[cfg(windows)]
    {
        std::os::windows::process::CommandExt::creation_flags(&mut command, CREATE_NO_WINDOW);
    }
    command
}

#[cfg(feature = "io")]
pub fn async_command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    tokio::process::Command::from(command(program))
}
