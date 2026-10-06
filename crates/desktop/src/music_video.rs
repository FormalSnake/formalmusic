//! The music video in the expanded player, where the cover sits in Song
//! mode. The daemon plays the audio; this decodes a muted, video-only
//! stream with ffmpeg (`formalmusic_core::video::Synced`) and holds each
//! frame to the store's interpolated playback position. One texture is alive
//! at a time, as for animated covers.
//!
//! Decoding runs only while it is seen: Video mode, the audio playing this
//! very video, and the window visible. The view itself lives only while the
//! expanded player is open.

use std::sync::Arc;
use std::time::{Duration, Instant};

use formalmusic_api::{Status, VideoStream};
use formalmusic_core::MusicStore;
use formalmusic_core::video::{Clock, Lead, Synced, VideoFrame};
use gpui_kit::*;
use image::{Frame, RgbaImage};
use tokio::sync::mpsc;

use crate::bridge::{Bridge, Topic};
use crate::clock::{self, Surface};

const FRAME_QUEUE: usize = 2;
/// How long a hidden window keeps ffmpeg stopped rather than gone, so a
/// quick look elsewhere costs no restart.
const HOLD: Duration = Duration::from_secs(10);
/// A picture coming back takes this long before the owner shows it loading.
const RESUME_GRACE: Duration = Duration::from_millis(150);

pub struct MusicVideo {
    store: MusicStore,
    /// The box the picture fits inside, centred.
    size: Size<Pixels>,
    scale: f32,
    visible: bool,
    /// The owner shows the video rather than the cover.
    active: bool,
    hardware: bool,
    video_id: Option<String>,
    stream: Option<VideoStream>,
    /// A stream request is in flight.
    requesting: bool,
    /// Asked yt-dlp again once already after googlevideo refused the URL.
    refreshed: bool,
    run: Option<Synced>,
    /// The window is hidden and `run` is stopped where it was, until `HOLD`
    /// has passed. Bumped on every hide, so an old timer knows it is stale.
    held: Option<u64>,
    hides: u64,
    lead: Lead,
    /// Since when a frame is owed: decoding started or resumed.
    waiting: Option<Instant>,
    surface: Option<Surface>,
    receiver: Option<Task<()>>,
    frame: Option<Arc<RenderImage>>,
    visibility: Option<Subscription>,
    activation: Option<Subscription>,
}

impl MusicVideo {
    pub fn new(store: MusicStore, cx: &mut Context<Self>) -> Self {
        let weak = cx.entity().downgrade();
        Bridge::watch(cx, Topic::NowPlaying, weak.into());
        cx.on_release(|this: &mut Self, cx| {
            if let Some(frame) = this.frame.take() {
                cx.drop_image(frame, None);
            }
        })
        .detach();
        Self {
            store,
            size: size(px(400.), px(225.)),
            scale: 1.,
            visible: true,
            active: false,
            hardware: hardware_decode(),
            video_id: None,
            stream: None,
            requesting: false,
            refreshed: false,
            run: None,
            held: None,
            hides: 0,
            lead: Lead::default(),
            waiting: None,
            surface: None,
            receiver: None,
            frame: None,
            visibility: None,
            activation: None,
        }
    }

    /// Whether a frame is on screen, so the owner can fade the cover out.
    pub fn showing(&self) -> bool {
        self.active && self.frame.is_some()
    }

    /// Whether the owner should show the picture loading: from the switch
    /// to the first frame, and when a picture coming back is slow to.
    pub fn loading(&self) -> bool {
        self.active
            && (self.requesting || self.run.is_some())
            && (self.frame.is_none()
                || self
                    .waiting
                    .is_some_and(|since| since.elapsed() >= RESUME_GRACE))
    }

    /// Owes a frame from now on, and has the owner look again once the
    /// grace for a picture coming back is over.
    fn wait(&mut self, cx: &mut Context<Self>) {
        self.waiting = Some(Instant::now());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RESUME_GRACE).await;
            let _ = this.update(cx, |this, cx| {
                if this.waiting.is_some() {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Hidden, ffmpeg is stopped where it is and let go after `HOLD`;
    /// shown again, it goes on from there. Called on the window's own
    /// events, so decoding is back underway before the first frame paints.
    fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if visible {
            self.held = None;
            if let Some(run) = &self.run {
                run.resume();
                self.wait(cx);
            }
        } else if let Some(run) = &self.run {
            run.pause();
            self.hides += 1;
            let hide = self.hides;
            self.held = Some(hide);
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(HOLD).await;
                let _ = this.update(cx, |this, cx| {
                    if this.held == Some(hide) {
                        this.held = None;
                        this.sync(cx);
                    }
                });
            })
            .detach();
        }
        self.sync(cx);
    }

    /// Width over height of the stream, 16:9 until it is known.
    pub fn aspect(&self) -> f32 {
        match &self.stream {
            Some(stream) if stream.height > 0 => stream.width as f32 / stream.height as f32,
            _ => 16. / 9.,
        }
    }

    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active != active {
            self.active = active;
            self.sync(cx);
            cx.notify();
        }
    }

    pub fn set_size(&mut self, size: Size<Pixels>, cx: &mut Context<Self>) {
        if self.size != size {
            self.size = size;
            if self.run.is_some() {
                self.stop();
                self.sync(cx);
            }
        }
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let (video_id, audible) = {
            let state = self.store.state();
            let video_id = state
                .player
                .track
                .as_ref()
                .and_then(|track| track.video_version())
                .map(str::to_owned);
            let audible = state.player.status == Status::Playing
                && video_id.is_some()
                && state.player.playing_id == video_id;
            (video_id, audible)
        };
        let video_id = video_id.filter(|_| self.active);
        if video_id != self.video_id {
            self.stop();
            if let Some(frame) = self.frame.take() {
                cx.drop_image(frame, None);
            }
            self.video_id = video_id;
            self.lead = Lead::default();
            self.stream = None;
            self.refreshed = false;
            self.requesting = false;
            if self.video_id.is_some() {
                self.request(false, cx);
            }
        }
        let seen = self.visible || (self.held.is_some() && self.run.is_some());
        let wanted = seen && self.active && audible && self.stream.is_some();
        if wanted && self.run.is_none() {
            self.start(cx);
        } else if !wanted && self.run.is_some() {
            self.stop();
        }
    }

    fn request(&mut self, refresh: bool, cx: &mut Context<Self>) {
        let Some(video_id) = self.video_id.clone() else {
            return;
        };
        self.requesting = true;
        let max_height = (f32::from(self.picture().height) * self.scale).round() as u32;
        let task = self.store.runtime().spawn({
            let (store, video_id) = (self.store.clone(), video_id.clone());
            async move { store.video_stream(video_id, max_height, refresh).await }
        });
        cx.spawn(async move |this, cx| {
            let stream = task.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                if this.video_id.as_ref() != Some(&video_id) {
                    return;
                }
                this.requesting = false;
                this.stream = stream;
                this.sync(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The picture's size in device pixels: the stream's shape, fitted in the box.
    fn picture(&self) -> Size<Pixels> {
        let aspect = self.aspect();
        let (w, h) = (self.size.width, self.size.height);
        if f32::from(w) / f32::from(h) > aspect {
            size(h * aspect, h)
        } else {
            size(w, w / aspect)
        }
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        let (Some(stream), Some(video_id)) = (&self.stream, self.video_id.clone()) else {
            return;
        };
        let picture = self.picture();
        // Decoding above the stream's own size only costs more bytes per
        // frame; the GPU scales the rest.
        let width = (f32::from(picture.width) * self.scale).round() as u32;
        let width = width.min(stream.width.max(2));
        let height = (width as f32 / self.aspect()).round() as u32;
        let store = self.store.clone();
        let clock: Clock = Arc::new(move || {
            let state = store.state();
            let audible = state.player.status == Status::Playing
                && state.player.playing_id.as_deref() == Some(video_id.as_str());
            audible.then(|| state.position_now() as f64 / 1000.)
        });
        let (tx, mut rx) = mpsc::channel::<VideoFrame>(FRAME_QUEUE);
        self.surface = Some(clock::Clock::surface(cx));
        self.run = Some(Synced::start(
            self.store.runtime(),
            stream,
            (width, height),
            self.hardware,
            &self.lead,
            clock,
            tx,
        ));
        self.wait(cx);
        cx.notify();
        self.receiver = Some(cx.spawn(async move |this, cx| {
            while let Some(frame) = rx.recv().await {
                if this.update(cx, |this, cx| this.show(frame, cx)).is_err() {
                    return;
                }
            }
            let _ = this.update(cx, |this, cx| this.ended(cx));
        }));
    }

    fn stop(&mut self) {
        self.run = None;
        self.held = None;
        self.waiting = None;
        self.receiver = None;
        self.surface = None;
    }

    /// The decoder gave up while it was still wanted: googlevideo refused
    /// the URL, or it expired. Asks for a fresh one once per video.
    fn ended(&mut self, cx: &mut Context<Self>) {
        self.stop();
        let audible = {
            let state = self.store.state();
            state.player.status == Status::Playing
                && state.player.playing_id.is_some()
                && state.player.playing_id == self.video_id
        };
        if !audible || !self.active || self.requesting {
            self.sync(cx);
            return;
        }
        self.stream = None;
        if !self.refreshed {
            self.refreshed = true;
            self.request(true, cx);
        }
    }

    fn show(&mut self, frame: VideoFrame, cx: &mut Context<Self>) {
        let Some(buffer) = RgbaImage::from_raw(frame.width, frame.height, frame.bgra) else {
            return;
        };
        let image = Arc::new(RenderImage::new(vec![Frame::new(buffer)]));
        if let Some(previous) = self.frame.replace(image) {
            cx.drop_image(previous, None);
        }
        self.waiting = None;
        cx.notify();
        clock::Clock::frame(cx);
    }
}

/// VA-API through ffmpeg where there is a render node, unless
/// `FORMALMUSIC_VAAPI=0`. ffmpeg falls back to software when it fails.
fn hardware_decode() -> bool {
    cfg!(target_os = "linux")
        && std::env::var("FORMALMUSIC_VAAPI").as_deref() != Ok("0")
        && std::path::Path::new("/dev/dri/renderD128").exists()
}

impl Render for MusicVideo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace::render("MusicVideo");
        if self.visibility.is_none() {
            self.visible = window.is_visible();
            self.visibility = Some(
                cx.observe_window_visibility(window, |this, visibility, _, cx| {
                    this.set_visible(visibility.is_visible(), cx)
                }),
            );
            // A window coming back to the front can say so before the
            // compositor reports it visible.
            self.activation = Some(cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.set_visible(true, cx);
                }
            }));
        }
        if self.scale != window.scale_factor() {
            self.scale = window.scale_factor();
            self.stop();
        }
        self.sync(cx);
        let picture = self.picture();
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .children(self.frame.clone().map(|frame| {
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let corners = Corners::all(crate::theme::radius::CARD);
                        let _ = window.paint_image(bounds, bounds, corners, frame, 0, false);
                    },
                )
                .w(picture.width)
                .h(picture.height)
            }))
    }
}
