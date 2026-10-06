//! Lyrics with word timing and Apple Music animated covers, for the daemon.
//!
//! Both features are ported from FormalShell (`LyricsService.qml`,
//! `Lyrics/model.js`, `AppleMusicArtService.qml`, `Media/applemusic.js`), whose
//! animated-cover chain in turn comes from DankMaterialShell PR #2918. The
//! matching rules, score thresholds and cache semantics are kept as they are
//! there; the shell's curl processes became `reqwest` calls.
//!
//! The providers here are undocumented web endpoints and can change without
//! notice, so every parser treats a surprising body as "nothing found".

mod cache;
mod error;
mod http;

pub mod animated;
pub mod lyrics;

pub use animated::AnimatedCovers;
pub use error::{Error, Result};
pub use lyrics::{Lyricist, LyricsRequest};

#[cfg(test)]
mod mock;
