//! What the store talks to: the daemon over its socket, or the demo set.

use async_trait::async_trait;
use formalmusic_api::{ApiError, Command, Event, Reply};
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ConnectionStatus {
    #[default]
    Connecting,
    Online,
    Offline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransportKind {
    Daemon,
    Demo,
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum ClientError {
    /// The daemon answered and said no.
    #[error(transparent)]
    Api(#[from] ApiError),
    /// No connection, or it dropped before the answer came.
    #[error("{0}")]
    Disconnected(String),
    #[error("the daemon did not answer in time")]
    Timeout,
    #[error("the daemon answered with the wrong kind of reply")]
    UnexpectedReply,
}

#[derive(Clone, Debug)]
pub enum TransportEvent {
    Connection {
        status: ConnectionStatus,
        error: Option<String>,
    },
    Event(Event),
}

#[async_trait]
pub trait Transport: Send + Sync {
    fn kind(&self) -> TransportKind;

    /// Starts connecting. Connection changes and daemon events go to
    /// `events` for as long as the transport lives; the subscription's
    /// opening `Player` and `Queue` snapshots arrive there too.
    fn start(&self, events: mpsc::UnboundedSender<TransportEvent>);

    /// One request and its answer.
    async fn call(&self, command: Command) -> Result<Reply, ClientError>;

    fn stop(&self);
}
