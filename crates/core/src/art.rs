//! Artwork on disk. Each picture kopuzd hands out is fetched once into
//! `$XDG_CACHE_HOME/formalmusic/art/<hash>`, keyed by its version so a new
//! picture is a new file, and the desktop decodes from there at the size it
//! shows the picture.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::backend::Backend;
use crate::model::{Art, ArtKind};

/// Boxes wider than this many device pixels ask kopuzd for the full-size
/// picture (544 px) rather than its thumbnail (400 px).
pub const HQ_ABOVE: u32 = 400;

pub struct ArtCache {
    dir: PathBuf,
    /// One lock per picture in flight, so two views asking at once fetch once.
    inflight: Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>,
}

impl ArtCache {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            inflight: Mutex::new(HashMap::new()),
        }
    }

    pub fn path_for(&self, art: &Art, hq: bool) -> PathBuf {
        let key = format!("{:?}/{}/{}/{hq}", art.kind, art.id, art.version);
        self.dir.join(format!("{:016x}", fnv1a(key.as_bytes())))
    }

    /// The file for `art`, fetching it first if it is not on disk yet.
    pub async fn fetch(&self, art: &Art, hq: bool, backend: &dyn Backend) -> Option<PathBuf> {
        let path = self.path_for(art, hq);
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Some(path);
        }
        let lock = self
            .inflight
            .lock()
            .entry(path.clone())
            .or_default()
            .clone();
        let _held = lock.lock().await;
        let result = if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            Some(path.clone())
        } else {
            self.download(art, hq, &path, backend).await
        };
        self.inflight.lock().remove(&path);
        result
    }

    async fn download(
        &self,
        art: &Art,
        hq: bool,
        path: &Path,
        backend: &dyn Backend,
    ) -> Option<PathBuf> {
        let bytes = if art.kind == ArtKind::Demo {
            let seed = art.id.clone();
            tokio::task::spawn_blocking(move || demo_art(&seed))
                .await
                .ok()?
        } else {
            match backend.artwork(art, hq).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    tracing::debug!("artwork {}: {error}", art.id);
                    return None;
                }
            }
        };
        tokio::fs::create_dir_all(&self.dir).await.ok()?;
        let temp = path.with_extension(format!("{}.tmp", std::process::id()));
        tokio::fs::write(&temp, &bytes).await.ok()?;
        tokio::fs::rename(&temp, path).await.ok()?;
        Some(path.to_owned())
    }
}

/// 64-bit FNV-1a: stable across builds, unlike `DefaultHasher`, so the file
/// names survive an upgrade.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// A 512 px PNG for a demo seed: a diagonal blend between two hues taken
/// from the seed, with a soft disc, so every demo card is distinct.
fn demo_art(seed: &str) -> Vec<u8> {
    const SIZE: u32 = 512;
    let hash = fnv1a(seed.as_bytes());
    let hue_a = (hash % 360) as f32;
    let hue_b = (hue_a + 40. + ((hash >> 12) % 80) as f32) % 360.;
    let a = hsl(hue_a, 0.55, 0.42);
    let b = hsl(hue_b, 0.6, 0.22);
    let disc = hsl((hue_a + 180.) % 360., 0.5, 0.6);
    let (cx, cy) = (
        0.3 + ((hash >> 20) % 40) as f32 / 100.,
        0.3 + ((hash >> 28) % 40) as f32 / 100.,
    );
    let radius = 0.18 + ((hash >> 36) % 14) as f32 / 100.;
    let image = image::RgbImage::from_fn(SIZE, SIZE, |x, y| {
        let (u, v) = (x as f32 / SIZE as f32, y as f32 / SIZE as f32);
        let t = (u + v) / 2.;
        let mut color = [0f32; 3];
        for (channel, value) in color.iter_mut().enumerate() {
            *value = a[channel] * (1. - t) + b[channel] * t;
        }
        let distance = ((u - cx).powi(2) + (v - cy).powi(2)).sqrt();
        let edge = ((radius - distance) / 0.01).clamp(0., 1.) * 0.85;
        for (channel, value) in color.iter_mut().enumerate() {
            *value = *value * (1. - edge) + disc[channel] * edge;
        }
        image::Rgb(color.map(|value| (value * 255.).round() as u8))
    });
    let mut out = std::io::Cursor::new(Vec::new());
    let _ = image.write_to(&mut out, image::ImageFormat::Png);
    out.into_inner()
}

fn hsl(h: f32, s: f32, l: f32) -> [f32; 3] {
    let c = (1. - (2. * l - 1.).abs()) * s;
    let x = c * (1. - ((h / 60.) % 2. - 1.).abs());
    let m = l - c / 2.;
    let (r, g, b) = match h as u32 / 60 {
        0 => (c, x, 0.),
        1 => (x, c, 0.),
        2 => (0., c, x),
        3 => (0., x, c),
        4 => (x, 0., c),
        _ => (c, 0., x),
    };
    [r + m, g + m, b + m]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn demo_art_lands_on_disk_once() {
        let dir = std::env::temp_dir().join(format!("formalmusic-art-{}", std::process::id()));
        let cache = ArtCache::new(dir.clone());
        let backend = crate::demo::DemoBackend::new();
        let art = Art {
            kind: ArtKind::Demo,
            id: "album-1".into(),
            version: 0,
        };
        let first = cache.fetch(&art, false, &backend).await.unwrap();
        let second = cache.fetch(&art, false, &backend).await.unwrap();
        assert_eq!(first, second);
        assert_ne!(first, cache.path_for(&art, true));
        assert!(
            image::ImageReader::open(&first)
                .unwrap()
                .with_guessed_format()
                .unwrap()
                .decode()
                .is_ok()
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
