//! Maps the icon names used across the app to gpui-kit's bundled Lucide set
//! (`gpui_kit::assets::IconName`).
//!
//! `gpui_kit::component::IconName` is a different, much smaller enum: a
//! curated set gpui-component's own widgets use internally (window controls,
//! chevrons). The full bundled Lucide set lives in `gpui_kit::assets` instead.

use std::borrow::Cow;

use gpui_kit::assets::{AllAssets, IconName as Glyph};
use gpui_kit::{
    App, AssetSource, Hsla, IntoElement, Pixels, RenderOnce, SharedString, Styled, Window, px, svg,
};

/// Names the rest of the UI asks for, independent of which glyph backs them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IconName {
    Home,
    Explore,
    Library,
    Search,
    Plus,
    Back,
    Forward,
    ChevronLeft,
    ChevronRight,
    Play,
    Pause,
    Next,
    Previous,
    Shuffle,
    Repeat,
    RepeatOne,
    Volume,
    VolumeLow,
    Muted,
    Like,
    Dislike,
    PlayNext,
    AddToQueue,
    AddToPlaylist,
    Radio,
    Album,
    Artist,
    Playlist,
    More,
    Close,
    Check,
    Alert,
    Remove,
    Grip,
    Sidebar,
    Account,
    SignOut,
    Saved,
    Music,
    Expand,
    Collapse,
    History,
    Open,
}

/// Resolves to the bundled Lucide glyph.
pub fn glyph(name: IconName) -> Glyph {
    use IconName::*;
    match name {
        Home => Glyph::House,
        Explore => Glyph::Compass,
        Library => Glyph::LibraryBig,
        Search => Glyph::Search,
        Plus => Glyph::Plus,
        Back => Glyph::ChevronLeft,
        Forward => Glyph::ChevronRight,
        ChevronLeft => Glyph::ChevronLeft,
        ChevronRight => Glyph::ChevronRight,
        Play => Glyph::Play,
        Pause => Glyph::Pause,
        Next => Glyph::SkipForward,
        Previous => Glyph::SkipBack,
        Shuffle => Glyph::Shuffle,
        Repeat => Glyph::Repeat,
        RepeatOne => Glyph::Repeat1,
        Volume => Glyph::Volume2,
        VolumeLow => Glyph::Volume1,
        Muted => Glyph::VolumeX,
        Like => Glyph::ThumbsUp,
        Dislike => Glyph::ThumbsDown,
        PlayNext => Glyph::ListStart,
        AddToQueue => Glyph::ListEnd,
        AddToPlaylist => Glyph::ListPlus,
        Radio => Glyph::Radio,
        Album => Glyph::DiscAlbum,
        Artist => Glyph::MicVocal,
        Playlist => Glyph::ListMusic,
        More => Glyph::Ellipsis,
        Close => Glyph::X,
        Check => Glyph::Check,
        Alert => Glyph::CircleAlert,
        Remove => Glyph::CircleMinus,
        Grip => Glyph::GripVertical,
        Sidebar => Glyph::PanelLeft,
        Account => Glyph::CircleUser,
        SignOut => Glyph::LogOut,
        Saved => Glyph::BookmarkPlus,
        Music => Glyph::Music,
        Expand => Glyph::ChevronUp,
        Collapse => Glyph::ChevronDown,
        History => Glyph::Clock,
        Open => Glyph::ExternalLink,
    }
}

/// Prefix of the icon paths `IconAssets` rewrites: it bakes Lucide's stroke
/// down from 2 to 1.5 beside regular copy, keeps 2 for `strong`, and fills
/// the active state (a liked thumb, the play triangle).
const VARIANT_PREFIX: &str = "formalmusic-icon/";

/// The bundled asset set, plus stroke and fill variants of its Lucide glyphs
/// under `formalmusic-icon/{regular,strong,filled}/<path>`.
pub struct IconAssets;

impl AssetSource for IconAssets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        let Some((variant, original)) = path
            .strip_prefix(VARIANT_PREFIX)
            .and_then(|rest| rest.split_once('/'))
        else {
            return AllAssets.load(path);
        };
        let Some(source) = AllAssets.load(original)? else {
            return Ok(None);
        };
        let source = String::from_utf8_lossy(&source);
        let baked = match variant {
            "strong" => source.into_owned(),
            "filled" => source
                .replace("fill=\"none\"", "fill=\"currentColor\"")
                .replace("stroke-width=\"2\"", "stroke-width=\"1.5\""),
            _ => source.replace("stroke-width=\"2\"", "stroke-width=\"1.5\""),
        };
        Ok(Some(Cow::Owned(baked.into_bytes())))
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        AllAssets.list(path)
    }
}

/// The icon element: stroke 1.5, 2 when `strong`, filled for an active state.
#[derive(IntoElement)]
pub struct Icon {
    name: IconName,
    size: Pixels,
    color: Hsla,
    strong: bool,
    filled: bool,
}

impl Icon {
    pub fn new(name: IconName) -> Self {
        Self {
            name,
            size: px(16.),
            color: Hsla::transparent_black(),
            strong: false,
            filled: false,
        }
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = color;
        self
    }

    pub fn strong(mut self, strong: bool) -> Self {
        self.strong = strong;
        self
    }

    pub fn filled(mut self, filled: bool) -> Self {
        self.filled = filled;
        self
    }
}

pub fn icon_path(name: IconName, strong: bool, filled: bool) -> SharedString {
    let variant = if filled {
        "filled"
    } else if strong {
        "strong"
    } else {
        "regular"
    };
    format!("{VARIANT_PREFIX}{variant}/{}", glyph(name).path()).into()
}

impl RenderOnce for Icon {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        svg()
            .path(icon_path(self.name, self.strong, self.filled))
            .flex_shrink_0()
            .size(self.size)
            .text_color(self.color)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn variants_rewrite_the_lucide_source() {
        let load = |path: SharedString| {
            String::from_utf8(IconAssets.load(&path).unwrap().unwrap().into_owned()).unwrap()
        };
        assert!(load(icon_path(IconName::Like, false, false)).contains("stroke-width=\"1.5\""));
        assert!(load(icon_path(IconName::Like, true, false)).contains("stroke-width=\"2\""));
        assert!(load(icon_path(IconName::Like, false, true)).contains("fill=\"currentColor\""));
    }
}
