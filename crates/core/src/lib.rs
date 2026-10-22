//! Everything that is not pixels: the Kopuz daemon connection, the store, the state
//! cache, artwork on disk and the demo catalog. No UI dependency, ever.

pub mod art;
pub mod cache;
pub mod demo;
pub mod format;
pub mod kopuz;
pub mod paths;
pub mod store;
pub mod transport;
pub mod video;

pub use store::{AppState, MusicStore, PlayingFrom, Route, SearchKey, StoreEvent, StoreOptions};
pub use transport::{ClientError, ConnectionStatus, Transport, TransportEvent, TransportKind};
