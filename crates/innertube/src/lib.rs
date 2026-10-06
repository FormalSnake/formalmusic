//! A client for YouTube Music's InnerTube API, the JSON API behind
//! music.youtube.com, speaking as the `WEB_REMIX` web client.
//!
//! [`Client`] sends the requests; the `parse` modules turn responses into the
//! page shapes of [`formalmusic_api`]. Streams are out of scope: the daemon
//! resolves those with yt-dlp.

mod auth;
mod client;
mod endpoints;
pub mod parse;

pub use client::{Client, NextResult, PlaybackTracking};
pub use formalmusic_api as api;

pub type Result<T> = std::result::Result<T, api::ApiError>;
