//! The local socket under [`crate::socket_path`]: a Unix socket, or a named
//! pipe on Windows, where tokio has no Unix sockets.

use std::io;
use std::path::Path;

pub use imp::{Listener, Stream};

pub type ReadHalf = tokio::io::ReadHalf<Stream>;
pub type WriteHalf = tokio::io::WriteHalf<Stream>;

pub fn split(stream: Stream) -> (ReadHalf, WriteHalf) {
    tokio::io::split(stream)
}

pub async fn connect(path: &Path) -> io::Result<Stream> {
    imp::connect(path).await
}

#[cfg(unix)]
mod imp {
    use std::io;
    use std::path::Path;

    pub type Stream = tokio::net::UnixStream;

    pub async fn connect(path: &Path) -> io::Result<Stream> {
        Stream::connect(path).await
    }

    #[derive(Debug)]
    pub struct Listener(tokio::net::UnixListener);

    impl Listener {
        /// Fails when the path is taken; callers remove a stale socket first.
        pub fn bind(path: &Path) -> io::Result<Self> {
            tokio::net::UnixListener::bind(path).map(Self)
        }

        pub async fn accept(&mut self) -> io::Result<Stream> {
            self.0.accept().await.map(|(stream, _)| stream)
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::path::Path;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use std::time::Duration;
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
    use tokio::net::windows::named_pipe::{
        ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
    };

    /// All instances of the pipe are busy serving other clients.
    const ERROR_PIPE_BUSY: i32 = 231;

    pub enum Stream {
        Client(NamedPipeClient),
        Server(NamedPipeServer),
    }

    pub async fn connect(path: &Path) -> io::Result<Stream> {
        loop {
            match ClientOptions::new().open(path) {
                Ok(client) => return Ok(Stream::Client(client)),
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Always holds one instance waiting for the next client, created before
    /// the connected one is handed out so a client never finds no pipe.
    #[derive(Debug)]
    pub struct Listener {
        path: std::path::PathBuf,
        next: NamedPipeServer,
    }

    impl Listener {
        /// Fails with `PermissionDenied` when another process already serves
        /// the pipe, which is the single-instance check on Windows.
        pub fn bind(path: &Path) -> io::Result<Self> {
            let next = ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create(path)?;
            Ok(Self {
                path: path.to_path_buf(),
                next,
            })
        }

        pub async fn accept(&mut self) -> io::Result<Stream> {
            self.next.connect().await?;
            let fresh = ServerOptions::new()
                .reject_remote_clients(true)
                .create(&self.path)?;
            Ok(Stream::Server(std::mem::replace(&mut self.next, fresh)))
        }
    }

    macro_rules! each {
        ($self:ident, $pipe:ident => $body:expr) => {
            match $self.get_mut() {
                Stream::Client($pipe) => $body,
                Stream::Server($pipe) => $body,
            }
        };
    }

    impl AsyncRead for Stream {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            each!(self, pipe => Pin::new(pipe).poll_read(cx, buf))
        }
    }

    impl AsyncWrite for Stream {
        fn poll_write(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            each!(self, pipe => Pin::new(pipe).poll_write(cx, buf))
        }

        fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            each!(self, pipe => Pin::new(pipe).poll_flush(cx))
        }

        fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            each!(self, pipe => Pin::new(pipe).poll_shutdown(cx))
        }
    }
}
