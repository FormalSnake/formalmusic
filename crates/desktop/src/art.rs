//! Artwork decoded at the size it is shown. GPUI samples a texture without
//! mipmaps, so a 544 px cover drawn into a 40 px row aliases and keeps a full
//! size texture alive for a thumbnail. Here each (URL, device size) pair is
//! fetched to disk by core's `ArtCache`, decoded once off the foreground
//! thread, box-filtered down to the box, and kept in a byte-capped cache
//! every view shares. The same path in the messages app is `stills.rs`.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Arc;
use std::time::{Duration, Instant};

use formalmusic_api::Thumbnail;
use gpui_kit::*;
use image::imageops::FilterType;
use image::{DynamicImage, Frame, ImageReader, RgbaImage};

use crate::theme::Palette;

/// Decoded bytes kept across every picture; least recently used go first.
/// Sized for an 8 GB laptop: a screen of cards at 2x is about 20 MB.
const BUDGET_BYTES: usize = 40 * 1024 * 1024;

/// Device pixels the expanded player's backdrop is decoded at. Drawn full
/// size, the GPU's bilinear stretch turns it into a soft wash of the cover's
/// colours, which costs a 3 KB texture instead of a blur pass.
const BACKDROP_PX: u32 = 24;

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    url: Arc<str>,
    px: u32,
}

enum Slot {
    Loading(Vec<EntityId>),
    Ready(Arc<RenderImage>, usize),
    Failed(Instant),
}

struct Entry {
    slot: Slot,
    used: Instant,
}

#[derive(Default)]
struct Art {
    entries: HashMap<Key, Entry>,
    bytes: usize,
    /// Pictures waiting for a fetch slot. Served newest first: what was
    /// asked for last is what is on screen now.
    waiting: Vec<Key>,
    running: usize,
}

/// Fetches in flight at once, and decodes at once. A fast scroll through a
/// long shelf queues dozens; these keep the network and the two runtime
/// threads free for the page itself.
const FETCHES: usize = 6;
const DECODES: usize = 2;

fn decodes() -> std::sync::Arc<tokio::sync::Semaphore> {
    static DECODING: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
        std::sync::OnceLock::new();
    DECODING
        .get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(DECODES)))
        .clone()
}

impl Global for Art {}

fn art(cx: &mut App) -> &mut Art {
    if !cx.has_global::<Art>() {
        cx.set_global(Art::default());
    }
    cx.global_mut::<Art>()
}

/// A failed fetch (offline, a 404) is tried again after this long.
const RETRY_AFTER: Duration = Duration::from_secs(20);

/// `img()` source for square artwork shown `size` wide.
pub fn source(thumbnails: &[Thumbnail], size: Pixels) -> Option<ImageSource> {
    let thumbnails: Arc<[Thumbnail]> = thumbnails.into();
    if thumbnails.is_empty() {
        return None;
    }
    Some(ImageSource::Custom(Arc::new(move |window, cx| {
        let px = (f32::from(size) * window.scale_factor()).round().max(1.) as u32;
        let url = formalmusic_core::art::url_for(&thumbnails, px)?;
        load(
            Key {
                url: url.into(),
                px,
            },
            window,
            cx,
        )
    })))
}

/// The cover at `BACKDROP_PX`, for a backdrop drawn far larger than that.
pub fn backdrop(thumbnails: &[Thumbnail]) -> Option<ImageSource> {
    let url: Arc<str> = formalmusic_core::art::url_for(thumbnails, 60)?.into();
    Some(ImageSource::Custom(Arc::new(move |window, cx| {
        load(
            Key {
                url: url.clone(),
                px: BACKDROP_PX,
            },
            window,
            cx,
        )
    })))
}

/// The hairline around a picture: black on light surfaces, white on dark.
pub fn outline(palette: &Palette) -> Hsla {
    if palette.is_dark() {
        hsla(0., 0., 1., 0.1)
    } else {
        hsla(0., 0., 0., 0.1)
    }
}

/// A square cover with a placeholder fill until it decodes. `round` makes it
/// a circle, as artists are drawn.
pub fn cover(
    thumbnails: &[Thumbnail],
    size: Pixels,
    radius: Pixels,
    round: bool,
    palette: &Palette,
) -> Div {
    let radius = if round { size / 2. } else { radius };
    let mut frame = div()
        .size(size)
        .flex_shrink_0()
        .rounded(radius)
        .overflow_hidden()
        .bg(palette.raised)
        .relative();
    if let Some(source) = source(thumbnails, size) {
        frame = frame.child(
            img(source)
                .size(size)
                .rounded(radius)
                .object_fit(ObjectFit::Cover),
        );
    }
    frame.child(
        div()
            .absolute()
            .inset_0()
            .rounded(radius)
            .border_1()
            .border_color(outline(palette)),
    )
}

fn load(
    key: Key,
    window: &mut Window,
    cx: &mut App,
) -> Option<Result<Arc<RenderImage>, ImageCacheError>> {
    let viewer = window.current_view();
    let (image, start) = {
        let art = art(cx);
        let entry = art.entries.entry(key.clone()).or_insert_with(|| Entry {
            slot: Slot::Loading(Vec::new()),
            used: Instant::now(),
        });
        entry.used = Instant::now();
        if let Slot::Failed(at) = entry.slot
            && at.elapsed() > RETRY_AFTER
        {
            entry.slot = Slot::Loading(Vec::new());
        }
        match &mut entry.slot {
            Slot::Ready(image, _) => (Some(image.clone()), false),
            Slot::Failed(_) => (None, false),
            Slot::Loading(viewers) => {
                let start = viewers.is_empty();
                if !viewers.contains(&viewer) {
                    viewers.push(viewer);
                }
                (None, start)
            }
        }
    };
    if let Some(image) = image {
        return Some(Ok(image));
    }
    if start {
        art(cx).waiting.push(key);
        pump(cx);
    }
    None
}

/// Starts waiting fetches while there are free slots. One not drawn for a
/// second was scrolled away: it is dropped, and asked for again if it comes back.
fn pump(cx: &mut App) {
    loop {
        let next = {
            let art = art(cx);
            if art.running >= FETCHES {
                return;
            }
            let Some(key) = art.waiting.pop() else { return };
            let fresh = art
                .entries
                .get(&key)
                .is_some_and(|entry| entry.used.elapsed() < IN_USE);
            if !fresh {
                // Its views repaint: one still on screen asks again and goes
                // to the front, one scrolled away does not.
                if let Some(Entry {
                    slot: Slot::Loading(viewers),
                    ..
                }) = art.entries.remove(&key)
                {
                    for viewer in viewers {
                        cx.notify(viewer);
                    }
                }
                continue;
            }
            art.running += 1;
            key
        };
        start_decode(next, cx);
    }
}

fn start_decode(key: Key, cx: &mut App) {
    let Some(store) = crate::bridge::store(cx) else {
        art(cx).running -= 1;
        return;
    };
    let cache = store.art().clone();
    let url = key.url.clone();
    let px = key.px;
    let task = store.runtime().spawn(async move {
        let path = cache.fetch(&url).await?;
        let _permit = decodes().acquire_owned().await.ok()?;
        tokio::task::spawn_blocking(move || decode(&path, px))
            .await
            .ok()
            .flatten()
    });
    cx.spawn(async move |cx| {
        let image = task.await.ok().flatten();
        cx.update(|cx| {
            art(cx).running -= 1;
            finish(key, image, cx);
            pump(cx);
        });
    })
    .detach();
}

fn finish(key: Key, image: Option<RenderImage>, cx: &mut App) {
    let viewers = {
        let art = art(cx);
        let Some(entry) = art.entries.get_mut(&key) else {
            return;
        };
        let slot = match image {
            Some(image) => {
                let bytes = image.as_bytes(0).map_or(0, <[u8]>::len);
                art.bytes += bytes;
                Slot::Ready(Arc::new(image), bytes)
            }
            None => Slot::Failed(Instant::now()),
        };
        match std::mem::replace(&mut entry.slot, slot) {
            Slot::Loading(viewers) => viewers,
            _ => Vec::new(),
        }
    };
    evict(&key, cx);
    for viewer in viewers {
        cx.notify(viewer);
    }
}

/// Anything drawn this recently is on screen or about to be again. Evicting it
/// only makes the next frame decode it anew and paint a blank box meanwhile.
const IN_USE: Duration = Duration::from_secs(1);

fn evict(keep: &Key, cx: &mut App) {
    let mut dropped = Vec::new();
    {
        let art = art(cx);
        let now = Instant::now();
        while art.bytes > BUDGET_BYTES {
            let oldest = art
                .entries
                .iter()
                .filter(|(key, entry)| {
                    *key != keep
                        && matches!(entry.slot, Slot::Ready(..))
                        && now.saturating_duration_since(entry.used) >= IN_USE
                })
                .min_by_key(|(_, entry)| entry.used)
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else { break };
            if let Some(Entry {
                slot: Slot::Ready(image, bytes),
                ..
            }) = art.entries.remove(&oldest)
            {
                art.bytes = art.bytes.saturating_sub(bytes);
                dropped.push(image);
            }
        }
    }
    for image in dropped {
        cx.drop_image(image, None);
    }
}

fn decode(path: &std::path::Path, px: u32) -> Option<RenderImage> {
    let bytes = std::fs::read(path).ok()?;
    let image = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let mut scaled = fit(image, px);
    // `image` decodes RGBA; GPUI wants BGRA.
    for pixel in scaled.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Some(RenderImage::new(vec![Frame::new(scaled)]))
}

/// Scales `image` so it covers a `px` square, never up. Video stills come in
/// 16:9, so the centre square is cut out the way the web app crops them.
fn fit(image: DynamicImage, px: u32) -> RgbaImage {
    let (w, h) = (image.width(), image.height());
    if w == 0 || h == 0 {
        return image.into_rgba8();
    }
    let side = w.min(h);
    let square = if w != h {
        image.crop_imm((w - side) / 2, (h - side) / 2, side, side)
    } else {
        image
    };
    if side <= px {
        return square.into_rgba8();
    }
    let filter = if px <= BACKDROP_PX {
        FilterType::Triangle
    } else {
        FilterType::CatmullRom
    };
    square.resize_exact(px, px, filter).into_rgba8()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn a_wide_still_is_cut_to_its_centre_square_and_scaled_down() {
        let wide = DynamicImage::ImageRgba8(RgbaImage::new(480, 360));
        assert_eq!(fit(wide, 120).dimensions(), (120, 120));
    }

    #[::core::prelude::v1::test]
    fn a_small_cover_is_never_scaled_up() {
        let small = DynamicImage::ImageRgba8(RgbaImage::new(60, 60));
        assert_eq!(fit(small, 240).dimensions(), (60, 60));
    }
}
