//! Everything that is not pixels: the kopuzd connection, the store, the state
//! cache, artwork on disk and the demo catalog. No UI dependency, ever.

pub mod art;
pub mod backend;
pub mod cache;
pub mod convert;
pub mod demo;
pub mod equalizer;
pub mod format;
pub mod kopuz;
pub mod model;
pub mod paths;
pub mod process;
pub mod relay;
pub mod settings;
pub mod store;
pub mod video;

/// The app id the desktop entry, the Windows Start menu shortcut and the media
/// session all carry.
pub const APP_ID: &str = "es.canarycoders.formalmusic";
pub const APP_NAME: &str = "FormalMusic";

pub use backend::{Backend, BackendKind, ClientError, ConnectionStatus};
pub use store::{AppState, MusicStore, Route, SearchKey, StoreEvent, StoreOptions};
