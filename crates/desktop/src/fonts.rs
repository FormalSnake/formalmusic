//! The Linux UI face. Distributions ship Noto Sans as one variable file, and
//! gpui-pre's Linux renderer draws a variable font at its default instance
//! whatever weight is asked for, so every semibold heading came out regular.
//! Static Regular, SemiBold and Bold faces registered under one family give
//! its closest-weight match three real faces to choose from.
//!
//! `packaging/fonts/FormalSans-*.ttf` are the static Noto Sans faces from
//! notofonts.github.io (OFL, `packaging/fonts/OFL.txt`), cut with
//! `pyftsubset --unicodes=U+0000-036F,U+0370-03FF,U+0400-052F,U+1C80-1C8F,U+1E00-1FFF,U+2000-20CF,U+2100-23FF,U+25A0-25FF,U+2C60-2C7F,U+2DE0-2DFF,U+A640-A69F,U+A720-A7FF,U+FB00-FB06,U+FEFF,U+FFFD --layout-features='*' --no-hinting`
//! and renamed to `FAMILY` (name IDs 1-4, 6, 16, 17). Anything outside that
//! range, CJK included, falls back through the system fonts as before.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::App;

pub const FAMILY: &str = "FormalMusic Sans";

static LOADED: AtomicBool = AtomicBool::new(false);

/// True once the bundled faces are registered with the text system.
pub fn loaded() -> bool {
    LOADED.load(Ordering::Relaxed)
}

/// Registers the bundled faces. Runs before the first window and before the
/// component theme reads `theme::font_sans()`, so nothing lays out in the
/// fallback family first.
pub fn install(cx: &mut App) {
    #[cfg(target_os = "linux")]
    {
        use std::borrow::Cow;
        let faces = vec![
            Cow::Borrowed(
                include_bytes!("../../../packaging/fonts/FormalSans-Regular.ttf").as_slice(),
            ),
            Cow::Borrowed(
                include_bytes!("../../../packaging/fonts/FormalSans-SemiBold.ttf").as_slice(),
            ),
            Cow::Borrowed(
                include_bytes!("../../../packaging/fonts/FormalSans-Bold.ttf").as_slice(),
            ),
        ];
        match cx.text_system().add_fonts(faces) {
            Ok(()) => LOADED.store(true, Ordering::Relaxed),
            Err(error) => {
                eprintln!("formalmusic: bundled UI font not registered, using Noto Sans: {error}")
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = cx;
}
