//! Apple Music animated covers, which kopuzd has no notion of, so the app
//! looks them up itself.
//!
//! Ported from FormalShell (`AppleMusicArtService.qml`, `Media/applemusic.js`),
//! whose chain in turn comes from DankMaterialShell PR #2918. The matching
//! rules, score thresholds and cache semantics are kept as they are there;
//! the shell's curl processes became `reqwest` calls.
//!
//! The providers are undocumented web endpoints and can change without
//! notice, so every parser treats a surprising body as "nothing found".

mod cache;
mod error;
mod http;

pub mod animated;

pub use animated::AnimatedCovers;
pub use error::{Error, Result};

#[cfg(test)]
mod mock;
