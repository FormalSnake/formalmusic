//! The current track's cover, with Apple Music's animated cover faded in
//! over it once its first frame is decoded. Frames come from
//! `formalmusic_core::video` already paced and at the drawn size; each one
//! becomes a `RenderImage` painted on a canvas, and the previous texture is
//! released as the next lands, so the atlas holds one frame per cover.
//!
//! Decoding runs only while it is seen: the track plays, the window is
//! visible, and the owner has not paused it. Otherwise ffmpeg is stopped and
//! the last frame stays on screen.

use std::path::Path;
use std::sync::Arc;

use formalmusic_api::Status;
use formalmusic_core::MusicStore;
use formalmusic_core::video::{self, Loop, VideoFrame, VideoInfo};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use image::{Frame, RgbaImage};
use tokio::sync::mpsc;

use crate::art;
use crate::bridge::{Bridge, Topic};
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
    /// `(artist, album)` of the current track, what the daemon looks covers up by.
    key: Option<(String, String)>,
    path: Option<Arc<Path>>,
    info: Option<VideoInfo>,
    run: Option<Loop>,
    receiver: Option<Task<()>>,
    frame: Option<Arc<RenderImage>>,
    /// Seconds into the file where the next run starts.
    position: f64,
    /// Bumped when a cover's first frame lands, so its fade plays once.
    shown: u64,
    visibility: Option<Subscription>,
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
            receiver: None,
            frame: None,
            position: 0.,
            shown: 0,
            visibility: None,
        }
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
        let wanted = self.visible && !self.paused && playing && self.info.is_some();
        if wanted && self.run.is_none() {
            self.start(cx);
        } else if !wanted && self.run.is_some() {
            self.stop();
        }
    }

    fn probe(&mut self, path: Arc<Path>, cx: &mut Context<Self>) {
        let task = self.store.runtime().spawn({
            let path = path.clone();
            async move { video::probe(&path).await }
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
        self.run = Some(Loop::start(
            self.store.runtime(),
            path,
            info,
            side,
            self.max_fps,
            self.position,
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
    }

    fn release(&mut self, cx: &mut Context<Self>) {
        if let Some(frame) = self.frame.take() {
            cx.drop_image(frame, None);
        }
    }

    fn show(&mut self, frame: VideoFrame, cx: &mut Context<Self>) {
        let Some(buffer) = RgbaImage::from_raw(frame.side, frame.side, frame.bgra) else {
            return;
        };
        let image = Arc::new(RenderImage::new(vec![Frame::new(buffer)]));
        match self.frame.replace(image) {
            Some(previous) => cx.drop_image(previous, None),
            None => self.shown += 1,
        }
        cx.notify();
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
        let thumbnails = self
            .store
            .state()
            .player
            .track
            .as_ref()
            .map(|track| track.thumbnails.clone())
            .unwrap_or_default();
        let (size, radius) = (self.size, self.radius);
        let picture = self.frame.clone().map(|frame| {
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
            .child(art::cover(&thumbnails, size, radius, false, &palette))
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
