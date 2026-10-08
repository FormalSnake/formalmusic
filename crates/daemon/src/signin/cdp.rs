//! The Chrome DevTools Protocol over `--remote-debugging-pipe`: the browser
//! reads commands from fd 3 and writes replies to fd 4 (the two handles
//! named by `--remote-debugging-io-pipes` on Windows), each a JSON message
//! ended by a NUL byte. A pipe, unlike a debugging port, is reachable only by
//! the process that started the browser.

use serde_json::{Value, json};
use std::io;

pub use imp::{Cdp, pipes};

#[cfg(unix)]
mod imp {
    use std::io;
    use std::os::fd::{AsRawFd, OwnedFd};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::unix::pipe;
    use tokio::process::Command;

    pub struct Cdp {
        commands: pipe::Sender,
        replies: BufReader<pipe::Receiver>,
        pub(super) next_id: u64,
    }

    /// The browser's ends of the two pipes, handed to it as fds 3 and 4.
    pub struct BrowserEnds {
        read: OwnedFd,
        write: OwnedFd,
    }

    pub fn pipes() -> io::Result<(Cdp, BrowserEnds)> {
        let (browser_read, commands) = std::io::pipe()?;
        let (replies, browser_write) = std::io::pipe()?;
        let cdp = Cdp {
            commands: pipe::Sender::from_owned_fd(OwnedFd::from(commands))?,
            replies: BufReader::new(pipe::Receiver::from_owned_fd(OwnedFd::from(replies))?),
            next_id: 0,
        };
        let ends = BrowserEnds {
            read: browser_read.into(),
            write: browser_write.into(),
        };
        Ok((cdp, ends))
    }

    impl BrowserEnds {
        /// Puts the ends on fds 3 and 4 in the child. Drop `self` once the child
        /// has spawned, or the browser never sees end of file from this side.
        pub fn attach(&self, command: &mut Command) {
            let (read, write) = (self.read.as_raw_fd(), self.write.as_raw_fd());
            // SAFETY: only async-signal-safe calls between fork and exec.
            unsafe {
                command.pre_exec(move || {
                    // Either end may already sit on 3 or 4, so both move above
                    // them before the dup2s. dup2 clears close-on-exec.
                    let read = libc::fcntl(read, libc::F_DUPFD_CLOEXEC, 10);
                    let write = libc::fcntl(write, libc::F_DUPFD_CLOEXEC, 10);
                    if read < 0 || write < 0 || libc::dup2(read, 3) < 0 || libc::dup2(write, 4) < 0
                    {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
    }

    impl Cdp {
        pub(super) async fn send(&mut self, message: &[u8]) -> io::Result<()> {
            self.commands.write_all(message).await
        }

        /// The next message without its NUL; `None` at end of file.
        pub(super) async fn next(&mut self) -> io::Result<Option<Vec<u8>>> {
            let mut buf = Vec::new();
            if self.replies.read_until(0, &mut buf).await? == 0 {
                return Ok(None);
            }
            if buf.last() == Some(&0) {
                buf.pop();
            }
            Ok(Some(buf))
        }
    }
}

/// Anonymous pipes have no async reads on Windows, so a thread reads the
/// replies and hands them over whole.
#[cfg(windows)]
mod imp {
    use std::io::{self, BufRead, Write};
    use std::os::windows::io::{AsRawHandle, OwnedHandle};
    use tokio::process::Command;
    use tokio::sync::mpsc;
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};

    pub struct Cdp {
        commands: std::io::PipeWriter,
        replies: mpsc::UnboundedReceiver<Vec<u8>>,
        pub(super) next_id: u64,
    }

    pub struct BrowserEnds {
        read: OwnedHandle,
        write: OwnedHandle,
    }

    pub fn pipes() -> io::Result<(Cdp, BrowserEnds)> {
        let (browser_read, commands) = std::io::pipe()?;
        let (replies, browser_write) = std::io::pipe()?;
        let (sender, receiver) = mpsc::unbounded_channel();
        std::thread::Builder::new()
            .name("formalmusicd-cdp".into())
            .spawn(move || {
                let mut replies = io::BufReader::new(replies);
                loop {
                    let mut buf = Vec::new();
                    match replies.read_until(0, &mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {
                            if buf.last() == Some(&0) {
                                buf.pop();
                            }
                            if sender.send(buf).is_err() {
                                return;
                            }
                        }
                    }
                }
            })?;
        let cdp = Cdp {
            commands,
            replies: receiver,
            next_id: 0,
        };
        let ends = BrowserEnds {
            read: browser_read.into(),
            write: browser_write.into(),
        };
        Ok((cdp, ends))
    }

    impl BrowserEnds {
        /// Makes both ends inheritable and names them on the command line.
        /// Drop `self` once the child has spawned, or the browser never sees
        /// end of file from this side.
        pub fn attach(&self, command: &mut Command) {
            for end in [&self.read, &self.write] {
                // SAFETY: a valid handle owned by `self`.
                unsafe {
                    SetHandleInformation(
                        end.as_raw_handle(),
                        HANDLE_FLAG_INHERIT,
                        HANDLE_FLAG_INHERIT,
                    );
                }
            }
            command.arg(format!(
                "--remote-debugging-io-pipes={},{}",
                self.read.as_raw_handle() as usize,
                self.write.as_raw_handle() as usize
            ));
        }
    }

    impl Cdp {
        /// Commands are a few hundred bytes, far below the pipe's buffer.
        pub(super) async fn send(&mut self, message: &[u8]) -> io::Result<()> {
            self.commands.write_all(message)
        }

        pub(super) async fn next(&mut self) -> io::Result<Option<Vec<u8>>> {
            Ok(self.replies.recv().await)
        }
    }
}

impl Cdp {
    /// Sends `method` to the browser target and waits for its result. End of
    /// file means the browser is gone.
    pub async fn call(&mut self, method: &str, params: Value) -> io::Result<Value> {
        self.call_in(None, method, params).await
    }

    /// [`Cdp::call`] on the page attached as `session`.
    pub async fn call_in(
        &mut self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> io::Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let mut message = json!({ "id": id, "method": method, "params": params });
        if let Some(session) = session {
            message["sessionId"] = session.into();
        }
        let mut message = serde_json::to_vec(&message)?;
        message.push(0);
        self.send(&message).await?;
        loop {
            let Some(buf) = self.next().await? else {
                return Err(io::ErrorKind::UnexpectedEof.into());
            };
            let Ok(mut reply) = serde_json::from_slice::<Value>(&buf) else {
                continue;
            };
            // Events carry no id.
            if reply.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = reply.get("error") {
                return Err(io::Error::other(format!("{method}: {error}")));
            }
            return Ok(reply.get_mut("result").map(Value::take).unwrap_or_default());
        }
    }
}
