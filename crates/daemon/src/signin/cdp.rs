//! The Chrome DevTools Protocol over `--remote-debugging-pipe`: the browser
//! reads commands from fd 3 and writes replies to fd 4, each a JSON message
//! ended by a NUL byte. A pipe, unlike a debugging port, is reachable only by
//! the process that started the browser.

use serde_json::{Value, json};
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::pipe;
use tokio::process::Command;

pub struct Cdp {
    commands: pipe::Sender,
    replies: BufReader<pipe::Receiver>,
    next_id: u64,
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
                if read < 0 || write < 0 || libc::dup2(read, 3) < 0 || libc::dup2(write, 4) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
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
        self.commands.write_all(&message).await?;
        let mut buf = Vec::new();
        loop {
            buf.clear();
            if self.replies.read_until(0, &mut buf).await? == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            if buf.last() == Some(&0) {
                buf.pop();
            }
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
