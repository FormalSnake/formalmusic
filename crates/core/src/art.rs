//! Artwork on disk. Each URL is fetched once into
//! `$XDG_CACHE_HOME/formalmusic/art/<hash>`, and the desktop decodes from
//! there at the size it shows the picture. Googleusercontent URLs carry their
//! size in the path, so they are asked for at a size step just over the box
//! instead of the 544 px the page offered.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use formalmusic_api::{Thumbnail, best};
use parking_lot::Mutex;

/// Sizes asked of the image server, so a 48 px row and a 56 px card share a file.
const STEPS: [u32; 6] = [60, 120, 226, 360, 544, 1080];

/// Scheme of the demo set's artwork, painted locally rather than fetched.
pub const DEMO_SCHEME: &str = "demo:";

pub struct ArtCache {
    dir: PathBuf,
    http: reqwest::Client,
    /// One lock per URL in flight, so two views asking at once fetch once.
    inflight: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl ArtCache {
    pub fn new(dir: PathBuf, http: reqwest::Client) -> Self {
        Self {
            dir,
            http,
            inflight: Mutex::new(HashMap::new()),
        }
    }

    pub fn path_for(&self, url: &str) -> PathBuf {
        self.dir.join(format!("{:016x}", fnv1a(url.as_bytes())))
    }

    /// The file for `url`, fetching it first if it is not on disk yet.
    pub async fn fetch(&self, url: &str) -> Option<PathBuf> {
        let path = self.path_for(url);
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Some(path);
        }
        let lock = self
            .inflight
            .lock()
            .entry(url.to_owned())
            .or_default()
            .clone();
        let _held = lock.lock().await;
        let result = if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            Some(path.clone())
        } else {
            self.download(url, &path).await
        };
        self.inflight.lock().remove(url);
        result
    }

    async fn download(&self, url: &str, path: &Path) -> Option<PathBuf> {
        let bytes = if let Some(seed) = url.strip_prefix(DEMO_SCHEME) {
            let seed = seed.to_owned();
            tokio::task::spawn_blocking(move || demo_art(&seed))
                .await
                .ok()?
        } else {
            let response = self
                .http
                .get(url)
                .send()
                .await
                .ok()?
                .error_for_status()
                .ok()?;
            response.bytes().await.ok()?.to_vec()
        };
        tokio::fs::create_dir_all(&self.dir).await.ok()?;
        let temp = path.with_extension(format!("{}.tmp", std::process::id()));
        tokio::fs::write(&temp, &bytes).await.ok()?;
        tokio::fs::rename(&temp, path).await.ok()?;
        Some(path.to_owned())
    }
}

/// The URL to fetch for a box `px` device pixels wide.
pub fn url_for(thumbnails: &[Thumbnail], px: u32) -> Option<String> {
    let thumb = best(thumbnails, px)?;
    let step = STEPS
        .iter()
        .copied()
        .find(|step| *step >= px)
        .unwrap_or(STEPS[STEPS.len() - 1]);
    Some(resize_url(&thumb.url, step))
}

/// Rewrites the `=w544-h544-...` tail googleusercontent and ggpht URLs carry.
/// Anything else (i.ytimg.com video stills) comes back unchanged.
pub fn resize_url(url: &str, px: u32) -> String {
    let resizable = url.contains("googleusercontent.com") || url.contains("ggpht.com");
    let Some(at) = url.rfind('=').filter(|_| resizable) else {
        return url.to_owned();
    };
    let tail = &url[at + 1..];
    if !tail.starts_with('w') && !tail.starts_with('s') {
        return url.to_owned();
    }
    format!("{}=w{px}-h{px}-l90-rj", &url[..at])
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

    #[test]
    fn googleusercontent_urls_are_asked_for_at_the_next_step() {
        let thumbs = vec![
            Thumbnail {
                url: "https://lh3.googleusercontent.com/abc=w60-h60-l90-rj".into(),
                width: 60,
                height: 60,
            },
            Thumbnail {
                url: "https://lh3.googleusercontent.com/abc=w544-h544-l90-rj".into(),
                width: 544,
                height: 544,
            },
        ];
        assert_eq!(
            url_for(&thumbs, 100).unwrap(),
            "https://lh3.googleusercontent.com/abc=w120-h120-l90-rj"
        );
        assert_eq!(
            url_for(&thumbs, 2000).unwrap(),
            "https://lh3.googleusercontent.com/abc=w1080-h1080-l90-rj"
        );
    }

    #[test]
    fn video_stills_keep_their_url() {
        let url = "https://i.ytimg.com/vi/abc/hqdefault.jpg?sqp=x";
        assert_eq!(resize_url(url, 120), url);
    }

    #[tokio::test]
    async fn demo_art_lands_on_disk_once() {
        let dir = std::env::temp_dir().join(format!("formalmusic-art-{}", std::process::id()));
        let cache = ArtCache::new(dir.clone(), reqwest::Client::new());
        let first = cache.fetch("demo:album-1").await.unwrap();
        let second = cache.fetch("demo:album-1").await.unwrap();
        assert_eq!(first, second);
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
