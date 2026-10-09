//! The current track's cover, with Apple Music's animated cover faded in
//! over it once its first frame is decoded. Frames come from
//! `formalmusic_core::video` already paced and at the drawn size; each one
//! becomes a `RenderImage` painted on a canvas, and the previous texture is
//! released as the next lands, so the atlas holds one frame per cover.
//!
//! Decoding runs only while it is seen: the track plays, the window is
//! visible, and the owner has not paused it. Otherwise ffmpeg is stopped and
//! the last frame stays on screen. A cover given a `source` (the player bar
//! while the expanded player is open) paints that one's frames, scaled down
//! on the GPU, and only decodes itself while the source does not.

use std::path::Path;
use std::sync::Arc;

use formalmusic_core::MusicStore;
use formalmusic_core::model::Status;
use formalmusic_core::video::{self, Loop, VideoFrame, VideoInfo};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use image::{Frame, RgbaImage};
use tokio::sync::mpsc;

use crate::art;
use crate::bridge::{Bridge, Topic};
use crate::clock::{Clock, Surface};
use crate::motion::{self, DURATION_BASE};
use crate::theme::Theme;

/// Frames queued between the pacer and the paint. Two lets one late paint
/// catch up without the pacer stalling.
const FRAME_QUEUE: usize = 2;

pub struct CoverVideo {
    store: MusicStore,
    size: Pixels,
    radius: Pixels,
    max_fps: f64,
    scale: f32,
    visible: bool,
    paused: bool,
    /// `(artist, album)` of the current track, what animated covers are looked up by.
    key: Option<(String, String)>,
    path: Option<Arc<Path>>,
    info: Option<VideoInfo>,
    run: Option<Loop>,
    surface: Option<Surface>,
    receiver: Option<Task<()>>,
    frame: Option<Arc<RenderImage>>,
    /// Seconds into the file where the next run starts.
    position: f64,
    /// Bumped when a cover's first frame lands, so its fade plays once.
    shown: u64,
    visibility: Option<Subscription>,
    source: Option<WeakEntity<CoverVideo>>,
    source_frames: Option<Subscription>,
}

impl CoverVideo {
    pub fn new(
        store: MusicStore,
        size: Pixels,
        radius: Pixels,
        max_fps: f64,
        cx: &mut Context<Self>,
    ) -> Self {
        let weak = cx.entity().downgrade();
        Bridge::watch(cx, Topic::NowPlaying, weak.clone().into());
        Bridge::watch(cx, Topic::AnimatedCover, weak.into());
        cx.on_release(|this: &mut Self, cx| {
            if let Some(frame) = this.frame.take() {
                cx.drop_image(frame, None);
            }
        })
        .detach();
        Self {
            store,
            size,
            radius,
            max_fps,
            scale: 1.,
            visible: true,
            paused: false,
            key: None,
            path: None,
            info: None,
            run: None,
            surface: None,
            receiver: None,
            frame: None,
            position: 0.,
            shown: 0,
            visibility: None,
            source: None,
            source_frames: None,
        }
    }

    /// Shows `source`'s frames while it decodes. Without one, this cover
    /// picks up decoding where the source was.
    pub fn set_source(&mut self, source: Option<Entity<CoverVideo>>, cx: &mut Context<Self>) {
        if let Some(old) = self.source.take().and_then(|old| old.upgrade()) {
            let old = old.read(cx);
            if old.key == self.key {
                self.position = old.position();
            }
        }
        self.source_frames = source.as_ref().map(|source| {
            cx.observe(source, |this: &mut Self, _, cx| {
                this.sync(cx);
                cx.notify();
            })
        });
        self.source = source.map(|source| source.downgrade());
        self.sync(cx);
        cx.notify();
    }

    fn decoding(&self) -> bool {
        self.run.is_some()
    }

    /// Seconds into the file where decoding is or would pick up.
    fn position(&self) -> f64 {
        self.run.as_ref().map_or(self.position, Loop::position)
    }

    /// The source's frame, while the source decodes this same cover.
    fn mirrored(&self, cx: &App) -> Option<Option<Arc<RenderImage>>> {
        let source = self.source.as_ref()?.upgrade()?;
        let source = source.read(cx);
        (source.decoding() && source.key == self.key).then(|| source.frame.clone())
    }

    /// The box changed (the expanded player follows the window size). A
    /// running decode restarts at the new size.
    pub fn set_size(&mut self, size: Pixels, cx: &mut Context<Self>) {
        if self.size == size {
            return;
        }
        self.size = size;
        if self.run.is_some() {
            self.stop();
            self.sync(cx);
        }
    }

    /// Holds the current frame without decoding, for a copy that something
    /// else on screen already shows larger.
    pub fn set_paused(&mut self, paused: bool, cx: &mut Context<Self>) {
        if self.paused != paused {
            self.paused = paused;
            self.sync(cx);
        }
    }

    /// Brings the decoder in line with the track, the cover and whether
    /// anyone can see it.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let (key, playing) = {
            let state = self.store.state();
            let key = state.player.track.as_ref().and_then(|track| {
                let artist = track.artists.first()?;
                let album = track.album.as_ref()?;
                Some((artist.text.clone(), album.text.clone()))
            });
            (key, state.player.status == Status::Playing)
        };
        if key != self.key {
            self.stop();
            self.release(cx);
            self.position = 0.;
            self.path = None;
            self.info = None;
            self.key = key.clone();
            if let Some(key) = key {
                self.store.load_animated_cover(key);
            }
        }
        let path = self.key.as_ref().and_then(|key| {
            self.store
                .state()
                .animated_covers
                .get(key)
                .and_then(|entry| entry.path.clone())
        });
        if path != self.path {
            self.stop();
            self.info = None;
            self.path = path.clone();
            if let Some(path) = path {
                self.probe(path, cx);
            }
        }
        let mirrored = self.mirrored(cx).is_some();
        if mirrored {
            self.release(cx);
        }
        let wanted = self.visible && !self.paused && !mirrored && playing && self.info.is_some();
        let was = self.run.is_some();
        if wanted && self.run.is_none() {
            self.start(cx);
        } else if !wanted && self.run.is_some() {
            self.stop();
        }
        // A cover mirroring this one takes over or hands back decoding.
        if was != self.run.is_some() {
            cx.notify();
        }
    }

    fn probe(&mut self, path: Arc<Path>, cx: &mut Context<Self>) {
        let task = self.store.runtime().spawn({
            let path = path.clone();
            async move { video::probe(&*path).await }
        });
        cx.spawn(async move |this, cx| {
            let info = task.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                if this.path.as_ref() == Some(&path) {
                    this.info = info;
                    this.sync(cx);
                }
            });
        })
        .detach();
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        let (Some(path), Some(info)) = (&self.path, &self.info) else {
            return;
        };
        let side = (f32::from(self.size) * self.scale).round() as u32;
        let (tx, mut rx) = mpsc::channel::<VideoFrame>(FRAME_QUEUE);
        self.surface = Some(Clock::surface(cx));
        self.run = Some(Loop::start(
            self.store.runtime(),
            path,
            info,
            side,
            (self.max_fps, self.position),
            hardware_decode(),
            tx,
        ));
        self.receiver = Some(cx.spawn(async move |this, cx| {
            while let Some(frame) = rx.recv().await {
                if this.update(cx, |this, cx| this.show(frame, cx)).is_err() {
                    return;
                }
            }
        }));
    }

    fn stop(&mut self) {
        if let Some(run) = self.run.take() {
            self.position = run.position();
        }
        self.receiver = None;
        self.surface = None;
    }

    fn release(&mut self, cx: &mut Context<Self>) {
        if let Some(frame) = self.frame.take() {
            cx.drop_image(frame, None);
        }
    }

    fn show(&mut self, frame: VideoFrame, cx: &mut Context<Self>) {
        let Some(buffer) = RgbaImage::from_raw(frame.width, frame.height, frame.bgra) else {
            return;
        };
        let image = Arc::new(RenderImage::new(vec![Frame::new(buffer)]));
        match self.frame.replace(image) {
            Some(previous) => cx.drop_image(previous, None),
            None => self.shown += 1,
        }
        cx.notify();
        Clock::frame(cx);
    }
}

impl Render for CoverVideo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace::render("CoverVideo");
        if self.visibility.is_none() {
            self.visible = window.is_visible();
            self.visibility = Some(cx.observe_window_visibility(
                window,
                |this, visibility, _, cx| {
                    this.visible = visibility.is_visible();
                    this.sync(cx);
                },
            ));
        }
        if self.scale != window.scale_factor() {
            self.scale = window.scale_factor();
            self.stop();
        }
        self.sync(cx);
        let palette = Theme::get(cx);
        let art = self
            .store
            .state()
            .player
            .track
            .as_ref()
            .and_then(|track| track.art.clone());
        let (size, radius) = (self.size, self.radius);
        let frame = self.mirrored(cx).unwrap_or_else(|| self.frame.clone());
        let picture = frame.map(|frame| {
            let corners = Corners::all(radius);
            motion::toward(
                div().absolute().inset_0().child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            let _ = window.paint_image(bounds, bounds, corners, frame, 0, false);
                        },
                    )
                    .size_full(),
                ),
                ElementId::NamedInteger("cover-video".into(), self.shown),
                true,
                DURATION_BASE,
                DURATION_BASE,
                |layer, t| layer.opacity(t),
            )
        });
        div()
            .relative()
            .size(size)
            .flex_shrink_0()
            .child(art::cover(art.as_ref(), size, radius, false, &palette))
            .when_some(picture, |el, picture| {
                el.child(picture).child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(radius)
                        .border_1()
                        .border_color(art::outline(&palette)),
                )
            })
    }
}

/// VA-API through ffmpeg for animated covers and music videos where there is
/// a render node, unless `FORMALMUSIC_VAAPI=0`. ffmpeg falls back to
/// software when it fails.
pub(crate) fn hardware_decode() -> bool {
    cfg!(target_os = "linux")
        && std::env::var("FORMALMUSIC_VAAPI").as_deref() != Ok("0")
        && std::path::Path::new("/dev/dri/renderD128").exists()
}
