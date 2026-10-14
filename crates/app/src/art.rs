//! Artwork for `image()`. nd's pipeline fetches, decodes at display size and keeps the LRU; this
//! only picks the URL a box of a given size should ask for, and paints the demo set's covers.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use formalmusic_api::Thumbnail;
use formalmusic_core::art::{DEMO_SCHEME, demo_art, url_for};
use nd::prelude::*;

thread_local! {
    static DEMO: RefCell<HashMap<String, Arc<[u8]>>> = RefCell::new(HashMap::new());
}

/// The image for `thumbnails` in a box `points` wide, or `None` when there is none.
pub fn source(thumbnails: &[Thumbnail], points: f32) -> Option<ImageSource> {
    // Asked for at twice the box, so it is sharp on a Retina or 2x display.
    let url = url_for(thumbnails, (points * 2.) as u32)?;
    Some(match url.strip_prefix(DEMO_SCHEME) {
        Some(seed) => {
            let bytes = DEMO.with(|demo| {
                demo.borrow_mut()
                    .entry(seed.to_owned())
                    .or_insert_with(|| demo_art(seed).into())
                    .clone()
            });
            ImageSource::bytes(url, bytes)
        }
        None => ImageSource::url(url),
    })
}

/// A cover `size` points square: the artwork, or the pipeline's placeholder while there is none.
pub fn cover(
    id: impl Into<SharedString>,
    thumbnails: &[Thumbnail],
    size: f32,
    alt: impl Into<SharedString>,
) -> Image {
    let el = match source(thumbnails, size) {
        Some(source) => image(source),
        None => image_placeholder(),
    };
    el.id(id).alt(alt).size(px(size)).flex_none()
}
