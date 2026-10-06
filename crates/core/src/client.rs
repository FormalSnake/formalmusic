//! The daemon over its Unix socket. Two connections: one for requests and
//! their answers, one subscribed to events, so a long page answer never holds
//! up the position ticks. Both come back with backoff when the daemon goes
//! away, and a missing socket starts `formalmusicd` once, since a dev box may
//! have no systemd unit for it.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use formalmusic_api::{
    Command, PROTOCOL_VERSION, Reply, Request, ResponseResult, ServerMessage, wire,
};
use parking_lot::Mutex;
use tokio::io::BufReader;
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::AbortHandle;

use crate::transport::{ClientError, ConnectionStatus, Transport, TransportEvent, TransportKind};

/// How long a request waits for a connection that is still coming up.
const CONNECT_WAIT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Browsing and starting playback go through InnerTube and yt-dlp, which
/// take seconds on a bad network.
const SLOW_REQUEST_TIMEOUT: Duration = Duration::from_secs(45);
/// A browser sign-in lasts as long as the user takes, and the daemon gives
/// up after five minutes.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(320);
const RETRY_MIN: Duration = Duration::from_millis(250);
const RETRY_MAX: Duration = Duration::from_secs(10);
/// A daemon that dies at once would otherwise be restarted on every retry.
const SPAWN_GAP: Duration = Duration::from_secs(30);
/// Failed attempts in a row before the status says offline rather than connecting.
const OFFLINE_AFTER: u32 = 4;

struct Conn {
    writer: tokio::sync::Mutex<OwnedWriteHalf>,
    pending: Mutex<HashMap<u64, oneshot::Sender<ResponseResult>>>,
}

impl Conn {
    /// Drops every waiting sender, so their callers see a disconnect at once.
    fn close(&self) {
        self.pending.lock().clear();
    }
}

struct Shared {
    socket: PathBuf,
    next_id: AtomicU64,
    conn: watch::Sender<Option<Arc<Conn>>>,
    stopped: AtomicBool,
    task: Mutex<Option<AbortHandle>>,
    last_spawn: Mutex<Option<Instant>>,
}

pub struct DaemonClient {
    shared: Arc<Shared>,
}

impl DaemonClient {
    pub fn new(socket: PathBuf) -> Self {
        let (conn, _) = watch::channel(None);
        Self {
            shared: Arc::new(Shared {
                socket,
                next_id: AtomicU64::new(1),
                conn,
                stopped: AtomicBool::new(false),
                task: Mutex::new(None),
                last_spawn: Mutex::new(None),
            }),
        }
    }

    async fn connection(&self) -> Result<Arc<Conn>, ClientError> {
        let mut rx = self.shared.conn.subscribe();
        let ready = tokio::time::timeout(CONNECT_WAIT, rx.wait_for(Option::is_some)).await;
        match ready {
            Ok(Ok(conn)) => Ok(conn.clone().expect("waited for Some")),
            _ => Err(ClientError::Disconnected(
                "The music daemon is not running.".into(),
            )),
        }
    }
}

fn timeout_for(command: &Command) -> Duration {
    match command {
        Command::Browse { .. }
        | Command::Continue { .. }
        | Command::Search { .. }
        | Command::Related { .. }
        | Command::Play { .. }
        | Command::SignIn { .. }
        | Command::ImportCookies { .. }
        | Command::ConnectLastFm { .. }
        | Command::ConnectListenBrainz { .. }
        | Command::Lyrics { .. } => SLOW_REQUEST_TIMEOUT,
        Command::BrowserSignIn { .. } => SIGN_IN_TIMEOUT,
        _ => REQUEST_TIMEOUT,
    }
}

#[async_trait]
impl Transport for DaemonClient {
    fn kind(&self) -> TransportKind {
        TransportKind::Daemon
    }

    fn start(&self, events: mpsc::UnboundedSender<TransportEvent>) {
        let shared = self.shared.clone();
        let task = tokio::spawn(supervise(shared, events));
        *self.shared.task.lock() = Some(task.abort_handle());
    }

    async fn call(&self, command: Command) -> Result<Reply, ClientError> {
        let conn = self.connection().await?;
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        let timeout = timeout_for(&command);
        let (tx, rx) = oneshot::channel();
        conn.pending.lock().insert(id, tx);
        let written = {
            let mut writer = conn.writer.lock().await;
            wire::write(&mut *writer, &Request { id, command }).await
        };
        if let Err(error) = written {
            conn.pending.lock().remove(&id);
            return Err(ClientError::Disconnected(format!(
                "Lost the music daemon: {error}"
            )));
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(ResponseResult::Ok(reply))) => Ok(reply),
            Ok(Ok(ResponseResult::Err(error))) => Err(ClientError::Api(error)),
            Ok(Err(_)) => Err(ClientError::Disconnected("Lost the music daemon.".into())),
            Err(_) => {
                conn.pending.lock().remove(&id);
                Err(ClientError::Timeout)
            }
        }
    }

    fn stop(&self) {
        self.shared.stopped.store(true, Ordering::SeqCst);
        if let Some(task) = self.shared.task.lock().take() {
            task.abort();
        }
        if let Some(conn) = self.shared.conn.send_replace(None) {
            conn.close();
        }
    }
}

async fn supervise(shared: Arc<Shared>, events: mpsc::UnboundedSender<TransportEvent>) {
    let mut delay = RETRY_MIN;
    let mut failures = 0u32;
    while !shared.stopped.load(Ordering::SeqCst) {
        match connect(&shared).await {
            Ok((conn, responses, subscribed)) => {
                failures = 0;
                delay = RETRY_MIN;
                shared.conn.send_replace(Some(conn.clone()));
                let _ = events.send(TransportEvent::Connection {
                    status: ConnectionStatus::Online,
                    error: None,
                });
                tokio::select! {
                    _ = read_responses(conn.clone(), responses) => {}
                    _ = read_events(subscribed, events.clone()) => {}
                }
                shared.conn.send_replace(None);
                conn.close();
                if shared.stopped.load(Ordering::SeqCst) {
                    return;
                }
                let _ = events.send(TransportEvent::Connection {
                    status: ConnectionStatus::Connecting,
                    error: Some("The music daemon closed the connection.".into()),
                });
            }
            Err(error) => {
                failures += 1;
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) {
                    maybe_spawn(&shared);
                }
                let status = if failures >= OFFLINE_AFTER {
                    ConnectionStatus::Offline
                } else {
                    ConnectionStatus::Connecting
                };
                let _ = events.send(TransportEvent::Connection {
                    status,
                    error: Some(describe(&error)),
                });
            }
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(RETRY_MAX);
    }
}

fn describe(error: &io::Error) -> String {
    match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused => {
            "The music daemon is not running.".into()
        }
        io::ErrorKind::InvalidData => error.to_string(),
        _ => format!("Could not reach the music daemon: {error}"),
    }
}

type Reader = BufReader<OwnedReadHalf>;

/// The subscribed connection only reads once `Subscribe` is out, but its
/// write half stays open beside the reader: dropping it would be a half
/// close the daemon reads as the client leaving.
struct Subscribed {
    reader: Reader,
    _writer: OwnedWriteHalf,
}

async fn connect(shared: &Shared) -> io::Result<(Arc<Conn>, Reader, Subscribed)> {
    let (responses, writer) = open(shared).await?;
    let (reader, mut subscriber) = open(shared).await?;
    let id = shared.next_id.fetch_add(1, Ordering::Relaxed);
    wire::write(
        &mut subscriber,
        &Request {
            id,
            command: Command::Subscribe,
        },
    )
    .await?;
    let conn = Arc::new(Conn {
        writer: tokio::sync::Mutex::new(writer),
        pending: Mutex::new(HashMap::new()),
    });
    Ok((
        conn,
        responses,
        Subscribed {
            reader,
            _writer: subscriber,
        },
    ))
}

/// Connects and checks the protocol number.
async fn open(shared: &Shared) -> io::Result<(Reader, OwnedWriteHalf)> {
    let stream = UnixStream::connect(&shared.socket).await?;
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    let id = shared.next_id.fetch_add(1, Ordering::Relaxed);
    wire::write(
        &mut write,
        &Request {
            id,
            command: Command::Hello {
                protocol: PROTOCOL_VERSION,
            },
        },
    )
    .await?;
    let mut buf = String::new();
    loop {
        let message: Option<ServerMessage> = wire::read(&mut read, &mut buf).await?;
        match message {
            None => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the daemon hung up during the handshake",
                ));
            }
            Some(ServerMessage::Response(response)) if response.id == id => {
                return match response.result {
                    ResponseResult::Ok(Reply::Hello { protocol, .. })
                        if protocol == PROTOCOL_VERSION =>
                    {
                        Ok((read, write))
                    }
                    ResponseResult::Ok(Reply::Hello { protocol, version }) => Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "A different version of the music daemon is running ({version}, protocol {protocol}). Restart formalmusicd."
                        ),
                    )),
                    other => Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("Unexpected handshake answer: {other:?}"),
                    )),
                };
            }
            Some(_) => {}
        }
    }
}

async fn read_responses(conn: Arc<Conn>, mut reader: Reader) {
    let mut buf = String::new();
    loop {
        match wire::read::<_, ServerMessage>(&mut reader, &mut buf).await {
            Ok(Some(ServerMessage::Response(response))) => {
                if let Some(waiting) = conn.pending.lock().remove(&response.id) {
                    let _ = waiting.send(response.result);
                }
            }
            Ok(Some(ServerMessage::Event(_))) => {}
            Ok(None) => return,
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                tracing::warn!("daemon: unreadable line: {error}")
            }
            Err(_) => return,
        }
    }
}

async fn read_events(subscribed: Subscribed, events: mpsc::UnboundedSender<TransportEvent>) {
    let Subscribed {
        mut reader,
        _writer,
    } = subscribed;
    let mut buf = String::new();
    loop {
        match wire::read::<_, ServerMessage>(&mut reader, &mut buf).await {
            Ok(Some(ServerMessage::Event(event))) => {
                if events.send(TransportEvent::Event(event)).is_err() {
                    return;
                }
            }
            Ok(Some(ServerMessage::Response(_))) => {}
            Ok(None) => return,
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                tracing::warn!("daemon: unreadable event: {error}")
            }
            Err(_) => return,
        }
    }
}

/// Starts `formalmusicd`: through its systemd user unit when there is one,
/// else from beside this binary or `PATH`, in its own process group so it
/// outlives the window and ignores the terminal's signals.
fn maybe_spawn(shared: &Shared) {
    {
        let mut last = shared.last_spawn.lock();
        if last.is_some_and(|at| at.elapsed() < SPAWN_GAP) {
            return;
        }
        *last = Some(Instant::now());
    }
    if start_unit() {
        return;
    }
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("formalmusicd")))
        .filter(|path| path.is_file());
    let program = beside.unwrap_or_else(|| PathBuf::from("formalmusicd"));
    let mut command = std::process::Command::new(&program);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    match command.spawn() {
        Ok(mut child) => {
            tracing::info!("started {}", program.display());
            // Reaped here so an early exit does not leave a zombie behind.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(error) => tracing::warn!("could not start {}: {error}", program.display()),
    }
}

/// A daemon started by the window lives in the window's scope and takes the
/// lock, so the unit then fails to start (a home-manager switch restarting it
/// while the window was open left it in `start-limit-hit`). Asking systemd
/// first keeps the unit the owner; `reset-failed` clears an earlier failure.
#[cfg(target_os = "linux")]
fn start_unit() -> bool {
    let systemctl = |args: &[&str]| {
        std::process::Command::new("systemctl")
            .arg("--user")
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    if !systemctl(&["cat", "formalmusicd.service"]) {
        return false;
    }
    let _ = systemctl(&["reset-failed", "formalmusicd.service"]);
    let started = systemctl(&["start", "formalmusicd.service"]);
    if started {
        tracing::info!("started formalmusicd.service");
    }
    started
}

#[cfg(not(target_os = "linux"))]
fn start_unit() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use formalmusic_api::{Event, Response};
    use tokio::net::UnixListener;

    /// A daemon that answers Hello and Toggle, and sends one event to subscribers.
    async fn fake_daemon(listener: UnixListener) {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut read = BufReader::new(read);
                let mut buf = String::new();
                while let Ok(Some(request)) = wire::read::<_, Request>(&mut read, &mut buf).await {
                    let reply = match request.command {
                        Command::Hello { .. } => Reply::Hello {
                            protocol: PROTOCOL_VERSION,
                            version: "test".into(),
                        },
                        Command::Subscribe => {
                            let event = ServerMessage::Event(Event::Position {
                                position_ms: 1500,
                                buffered_ms: 3000,
                            });
                            wire::write(&mut write, &event).await.unwrap();
                            continue;
                        }
                        _ => Reply::Ok,
                    };
                    let response = ServerMessage::Response(Response {
                        id: request.id,
                        result: ResponseResult::Ok(reply),
                    });
                    wire::write(&mut write, &response).await.unwrap();
                }
            });
        }
    }

    #[tokio::test]
    async fn requests_get_answers_and_events_arrive_on_their_own_connection() {
        let dir = std::env::temp_dir().join(format!("fm-client-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("d.sock");
        let _ = std::fs::remove_file(&socket);
        tokio::spawn(fake_daemon(UnixListener::bind(&socket).unwrap()));

        let client = DaemonClient::new(socket);
        let (tx, mut rx) = mpsc::unbounded_channel();
        client.start(tx);
        assert!(matches!(client.call(Command::Toggle).await, Ok(Reply::Ok)));
        let mut saw_online = false;
        let mut saw_position = false;
        while !(saw_online && saw_position) {
            match tokio::time::timeout(Duration::from_secs(2), rx.recv())
                .await
                .unwrap()
                .unwrap()
            {
                TransportEvent::Connection {
                    status: ConnectionStatus::Online,
                    ..
                } => saw_online = true,
                TransportEvent::Event(Event::Position {
                    position_ms: 1500, ..
                }) => saw_position = true,
                _ => {}
            }
        }
        client.stop();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_missing_daemon_fails_the_request_instead_of_hanging() {
        let client = DaemonClient::new(std::env::temp_dir().join("fm-no-such-daemon.sock"));
        *client.shared.last_spawn.lock() = Some(Instant::now());
        let (tx, _rx) = mpsc::unbounded_channel();
        client.start(tx);
        assert!(matches!(
            client.call(Command::Toggle).await,
            Err(ClientError::Disconnected(_))
        ));
        client.stop();
    }
}
