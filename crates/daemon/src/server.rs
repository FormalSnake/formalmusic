//! The Unix socket: one task per connection, one task per request, so a slow
//! browse never holds up a Toggle behind it.

use crate::daemon::Daemon;
use formalmusic_api::{
    ApiError, Command, Event, PROTOCOL_VERSION, Reply, Request, Response, ResponseResult,
    ServerMessage, wire,
};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::BufReader;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, mpsc};

/// Messages queued for one slow client before its events start being dropped.
const OUTBOX: usize = 1024;

/// Binds the socket, replacing a stale one only when no daemon answers on it.
pub async fn bind(path: &Path) -> anyhow::Result<UnixListener> {
    if let Some(dir) = path.parent() {
        crate::config::create_private_dir(dir)?;
    }
    if path.exists() {
        if answers_hello(path).await {
            anyhow::bail!("formalmusicd is already running on {}", path.display());
        }
        tracing::info!(path = %path.display(), "removing stale socket");
        std::fs::remove_file(path)?;
    }
    Ok(UnixListener::bind(path)?)
}

async fn answers_hello(path: &Path) -> bool {
    let probe = async {
        let stream = UnixStream::connect(path).await.ok()?;
        let (read, mut write) = stream.into_split();
        let hello = Request {
            id: 0,
            command: Command::Hello {
                protocol: PROTOCOL_VERSION,
            },
        };
        wire::write(&mut write, &hello).await.ok()?;
        let mut read = BufReader::new(read);
        let mut buf = String::new();
        let reply: ServerMessage = wire::read(&mut read, &mut buf).await.ok()??;
        Some(matches!(reply, ServerMessage::Response(_)))
    };
    tokio::time::timeout(Duration::from_secs(2), probe)
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
}

pub async fn serve(listener: UnixListener, daemon: Arc<Daemon>) {
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                tokio::spawn(connection(stream, daemon.clone()));
            }
            Err(e) => {
                tracing::warn!("accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn connection(stream: UnixStream, daemon: Arc<Daemon>) {
    let (read, mut write) = stream.into_split();
    let (outbox, mut messages) = mpsc::channel::<ServerMessage>(OUTBOX);
    let writer = tokio::spawn(async move {
        while let Some(message) = messages.recv().await {
            if wire::write(&mut write, &message).await.is_err() {
                break;
            }
        }
    });

    let mut read = BufReader::new(read);
    let mut buf = String::new();
    let mut greeted = false;
    let mut subscription: Option<tokio::task::JoinHandle<()>> = None;
    loop {
        let request: Request = match wire::read(&mut read, &mut buf).await {
            Ok(Some(request)) => request,
            Ok(None) => break,
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                tracing::debug!("unreadable request: {e}");
                continue;
            }
            Err(_) => break,
        };
        let id = request.id;
        match request.command {
            Command::Hello { protocol } => {
                greeted = protocol == PROTOCOL_VERSION;
                let reply = Reply::Hello {
                    protocol: PROTOCOL_VERSION,
                    version: env!("CARGO_PKG_VERSION").into(),
                };
                respond(&outbox, id, Ok(reply)).await;
            }
            _ if !greeted => {
                let err = ApiError::BadRequest(format!(
                    "send hello with protocol {PROTOCOL_VERSION} first"
                ));
                respond(&outbox, id, Err(err)).await;
            }
            Command::Subscribe => {
                if subscription.is_none() {
                    // Subscribe before the snapshot so nothing falls between them.
                    let events = daemon.events.subscribe();
                    let snapshot = [
                        Event::Player(daemon.playback.player_state()),
                        Event::Queue(daemon.playback.queue_state()),
                    ];
                    for event in snapshot {
                        let _ = outbox.send(ServerMessage::Event(event)).await;
                    }
                    subscription = Some(tokio::spawn(forward(events, outbox.clone())));
                }
                respond(&outbox, id, Ok(Reply::Ok)).await;
            }
            command => {
                let daemon = daemon.clone();
                let outbox = outbox.clone();
                tokio::spawn(async move {
                    let result = daemon.dispatch(command).await;
                    respond(&outbox, id, result).await;
                });
            }
        }
    }
    if let Some(subscription) = subscription {
        subscription.abort();
    }
    drop(outbox);
    let _ = writer.await;
}

async fn forward(mut events: broadcast::Receiver<Event>, outbox: mpsc::Sender<ServerMessage>) {
    loop {
        match events.recv().await {
            Ok(event) => match outbox.try_send(ServerMessage::Event(event)) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::debug!("subscriber is behind, dropping an event")
                }
                Err(mpsc::error::TrySendError::Closed(_)) => return,
            },
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::debug!(n, "subscriber missed events")
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

async fn respond(outbox: &mpsc::Sender<ServerMessage>, id: u64, result: Result<Reply, ApiError>) {
    let result = match result {
        Ok(reply) => ResponseResult::Ok(reply),
        Err(err) => ResponseResult::Err(err),
    };
    let _ = outbox
        .send(ServerMessage::Response(Response { id, result }))
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Paths};
    use formalmusic_api::{PlaySource, Repeat, Status};
    use formalmusic_player::{OutputKind, Player};
    use tokio::io::BufReader;
    use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

    struct Conn {
        read: BufReader<OwnedReadHalf>,
        write: OwnedWriteHalf,
        buf: String,
        next_id: u64,
    }

    impl Conn {
        async fn open(path: &Path) -> Self {
            let (read, write) = UnixStream::connect(path).await.unwrap().into_split();
            Self {
                read: BufReader::new(read),
                write,
                buf: String::new(),
                next_id: 0,
            }
        }

        async fn send(&mut self, command: Command) -> u64 {
            self.next_id += 1;
            let request = Request {
                id: self.next_id,
                command,
            };
            wire::write(&mut self.write, &request).await.unwrap();
            self.next_id
        }

        async fn recv(&mut self) -> ServerMessage {
            self.recv_within(Duration::from_secs(5)).await
        }

        async fn recv_within(&mut self, wait: Duration) -> ServerMessage {
            let read = wire::read(&mut self.read, &mut self.buf);
            tokio::time::timeout(wait, read)
                .await
                .unwrap_or_else(|_| panic!("no message within {wait:?}"))
                .unwrap()
                .expect("daemon hung up")
        }

        /// Sends `command` and returns its response, skipping events.
        async fn call(&mut self, command: Command) -> ResponseResult {
            let id = self.send(command).await;
            loop {
                if let ServerMessage::Response(response) = self.recv().await
                    && response.id == id
                {
                    return response.result;
                }
            }
        }
    }

    async fn start() -> (tempfile::TempDir, std::path::PathBuf) {
        serve_in(tempfile::tempdir().unwrap()).await
    }

    async fn serve_in(dir: tempfile::TempDir) -> (tempfile::TempDir, std::path::PathBuf) {
        let paths = Paths {
            state: dir.path().join("state"),
            config: dir.path().join("daemon.json"),
            app_settings: dir.path().join("config.json"),
        };
        let player = Player::with_output(OutputKind::Null {
            sample_rate: 48_000,
            channels: 2,
        })
        .unwrap();
        let daemon = Daemon::new(&paths, Config::default(), player).unwrap();
        let socket = dir.path().join("run/formalmusicd.sock");
        let listener = bind(&socket).await.unwrap();
        tokio::spawn(serve(listener, daemon));
        (dir, socket)
    }

    #[tokio::test]
    async fn handshake_subscribe_and_commands() {
        let (_dir, socket) = start().await;
        let mut conn = Conn::open(&socket).await;

        let before = conn.call(Command::PlayerState).await;
        assert!(matches!(
            before,
            ResponseResult::Err(ApiError::BadRequest(_))
        ));

        let hello = conn
            .call(Command::Hello {
                protocol: PROTOCOL_VERSION,
            })
            .await;
        let ResponseResult::Ok(Reply::Hello { protocol, version }) = hello else {
            panic!("{hello:?}")
        };
        assert_eq!(protocol, PROTOCOL_VERSION);
        assert_eq!(version, env!("CARGO_PKG_VERSION"));

        let mut events = Conn::open(&socket).await;
        events
            .call(Command::Hello {
                protocol: PROTOCOL_VERSION,
            })
            .await;
        events.send(Command::Subscribe).await;
        assert!(
            matches!(events.recv().await, ServerMessage::Event(Event::Player(p)) if p.status == Status::Stopped)
        );
        assert!(
            matches!(events.recv().await, ServerMessage::Event(Event::Queue(q)) if q.tracks.is_empty())
        );
        assert!(matches!(
            events.recv().await,
            ServerMessage::Response(Response {
                result: ResponseResult::Ok(Reply::Ok),
                ..
            })
        ));

        conn.call(Command::SetRepeat {
            repeat: Repeat::All,
        })
        .await;
        assert!(
            matches!(events.recv().await, ServerMessage::Event(Event::Player(p)) if p.repeat == Repeat::All)
        );
        assert!(matches!(
            events.recv().await,
            ServerMessage::Event(Event::Queue(_))
        ));

        conn.call(Command::SetVolume { volume: 1.7 }).await;
        assert!(
            matches!(events.recv().await, ServerMessage::Event(Event::Player(p)) if p.volume == 1.0)
        );

        let moved = conn.call(Command::MoveInQueue { from: 0, to: 1 }).await;
        assert!(matches!(
            moved,
            ResponseResult::Err(ApiError::BadRequest(_))
        ));
        let state = conn.call(Command::PlayerState).await;
        assert!(
            matches!(state, ResponseResult::Ok(Reply::Player(p)) if p.repeat == Repeat::All && p.volume == 1.0)
        );
    }

    /// Against the real YouTube, signed in from the Netscape cookie file at
    /// `FORMALMUSIC_COOKIES`, with yt-dlp on `PATH`: an endless mix answers
    /// Play with its first track loading as quickly as a single track does.
    /// Audible playback follows once yt-dlp has the stream, as for any track.
    /// `cargo test -p formalmusicd -- --ignored --nocapture live_`
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "signs in to music.youtube.com and streams from googlevideo"]
    async fn live_supermix_starts_loading_within_three_seconds() {
        let Ok(path) = std::env::var("FORMALMUSIC_COOKIES") else {
            eprintln!("skipping: FORMALMUSIC_COOKIES is not set");
            return;
        };
        let stored = crate::session::Stored {
            cookies: crate::session::cookie_header(&std::fs::read_to_string(path).unwrap()),
            page_id: None,
            profile: None,
        };
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(
            state.join("session.json"),
            serde_json::to_vec(&stored).unwrap(),
        )
        .unwrap();
        let (_dir, socket) = serve_in(dir).await;
        let hello = || Command::Hello {
            protocol: PROTOCOL_VERSION,
        };
        let mut conn = Conn::open(&socket).await;
        conn.call(hello()).await;
        let mut events = Conn::open(&socket).await;
        events.call(hello()).await;
        events.call(Command::Subscribe).await;

        let asked = std::time::Instant::now();
        let play = conn
            .call(Command::Play {
                source: PlaySource::Playlist {
                    playlist_id: "RDTMAK5uy_kset8DisdE7LSD4TNjEVvrKRTmG7a56sY".into(),
                    tracks: Vec::new(),
                },
                start_index: 0,
                shuffle: false,
                radio: false,
            })
            .await;
        assert!(matches!(play, ResponseResult::Ok(_)), "{play:?}");
        let started = asked.elapsed();
        assert!(started < Duration::from_secs(3), "Play took {started:?}");
        loop {
            match events.recv_within(Duration::from_secs(30)).await {
                ServerMessage::Event(Event::Player(p)) if p.status == Status::Playing => break,
                ServerMessage::Event(Event::Notice { message }) => panic!("{message}"),
                _ => {}
            }
        }
        eprintln!(
            "Play answered after {started:?}, audible after {:?}",
            asked.elapsed()
        );
    }

    #[tokio::test]
    async fn refuses_to_start_twice() {
        let (_dir, socket) = start().await;
        let err = bind(&socket).await.unwrap_err();
        assert!(err.to_string().contains("already running"), "{err}");

        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join("stale.sock");
        drop(std::os::unix::net::UnixListener::bind(&stale).unwrap());
        assert!(stale.exists());
        bind(&stale).await.expect("a stale socket is replaced");
    }
}
