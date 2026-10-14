//! Moving pictures: the album's Apple Music motion artwork and, in Video mode, the music video,
//! both decoded by core's ffmpeg pipes into BGRA frames. nd draws pictures through `image()`,
//! so each frame travels as a fast PNG to the pipeline under a fresh key; the frame before
//! stays up until the next one has decoded.
//!
//! Decoding runs only while a view shows the picture and the track plays.

use std::io::Cursor;
use std::path::Path;
use std::sync::Arc;

use formalmusic_api::Status;
use formalmusic_core::video::{self, Clock, Lead, Loop, Synced, VideoFrame, VideoInfo};
use formalmusic_core::{MusicStore, StoreEvent};
use nd::prelude::*;
use tokio::sync::mpsc;

use crate::bridge;

const FRAME_QUEUE: usize = 2;

/// The window's two moving pictures, as an app global.
#[derive(Clone)]
pub struct Pictures {
    pub covers: Entity<Covers>,
    pub music: Entity<MusicVideo>,
}

/// The latest motion artwork frame, re-rendering `viewer` on the next ones.
pub fn cover_frame(viewer: EntityId, cx: &mut App) -> Option<ImageSource> {
    let pictures = cx.try_global::<Pictures>()?.clone();
    let picture = pictures.covers.read(cx).picture.clone();
    add_viewer(&picture, viewer, cx);
    picture.read(cx).frame()
}

/// The latest music video frame, likewise.
pub fn music_frame(viewer: EntityId, cx: &mut App) -> Option<ImageSource> {
    let pictures = cx.try_global::<Pictures>()?.clone();
    let picture = pictures.music.read(cx).picture.clone();
    add_viewer(&picture, viewer, cx);
    picture.read(cx).frame()
}

fn add_viewer(picture: &Entity<Picture>, viewer: EntityId, cx: &mut App) {
    picture.update(cx, |p, _| {
        if !p.viewers.contains(&viewer) {
            p.viewers.push(viewer);
        }
    });
}

/// VA-API through ffmpeg where there is a render node, unless `FORMALMUSIC_VAAPI=0`.
fn hardware_decode() -> bool {
    cfg!(target_os = "linux")
        && std::env::var("FORMALMUSIC_VAAPI").as_deref() != Ok("0")
        && Path::new("/dev/dri/renderD128").exists()
}

/// Encodes a decoded frame for the image pipeline.
fn png(frame: VideoFrame) -> Option<Arc<[u8]>> {
    use image::ImageEncoder;
    let mut rgba = frame.bgra;
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let mut out = Vec::with_capacity(rgba.len() / 3);
    image::codecs::png::PngEncoder::new_with_quality(
        Cursor::new(&mut out),
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::NoFilter,
    )
    .write_image(
        &rgba,
        frame.width,
        frame.height,
        image::ExtendedColorType::Rgba8,
    )
    .ok()?;
    Some(out.into())
}

/// Turns `frames` into encoded pictures on the store's runtime and hands each to `show` on
/// the main thread.
fn pump(
    store: &MusicStore,
    mut frames: mpsc::Receiver<VideoFrame>,
    show: impl FnMut(Arc<[u8]>, &mut App) + 'static,
) {
    let (tx, rx) = mpsc::unbounded_channel();
    store.runtime().spawn(async move {
        while let Some(frame) = frames.recv().await {
            let Ok(Some(bytes)) = tokio::task::spawn_blocking(move || png(frame)).await else {
                continue;
            };
            if tx.send(bytes).is_err() {
                return;
            }
        }
    });
    bridge::on_main(rx, show);
}

enum Run {
    Cover(Loop),
    /// Held only: dropping it stops ffmpeg.
    Music {
        _run: Synced,
    },
}

/// The latest frame of one moving picture, and the decoder behind it.
pub struct Picture {
    run: Option<Run>,
    /// What the decoder is showing: the cover file, or the video id.
    source: Option<String>,
    frame: Option<ImageSource>,
    seq: u64,
    /// The views to re-render on each frame.
    viewers: Vec<EntityId>,
}

impl Picture {
    fn new() -> Self {
        Self {
            run: None,
            source: None,
            frame: None,
            seq: 0,
            viewers: Vec::new(),
        }
    }

    pub fn frame(&self) -> Option<ImageSource> {
        self.frame.clone()
    }

    fn stop(&mut self) {
        self.run = None;
        self.source = None;
        self.frame = None;
    }

    fn show(this: &WeakEntity<Self>, generation: u64, bytes: Arc<[u8]>, cx: &mut App) {
        this.update(cx, |p, cx| {
            if p.seq >> 32 != generation || p.run.is_none() {
                return;
            }
            p.seq += 1;
            p.frame = Some(ImageSource::bytes(format!("video-{}", p.seq), bytes));
            let app: &mut App = cx;
            for id in p.viewers.clone() {
                app.notify(id);
            }
        });
    }

    /// A new decoder: its frames carry a generation, so a stopped one's late frames are dropped.
    fn next_generation(&mut self) -> u64 {
        self.seq = ((self.seq >> 32) + 1) << 32;
        self.seq >> 32
    }
}

/// Motion artwork for the playing album, at `side` points and `fps` frames a second.
pub struct Covers {
    pub picture: Entity<Picture>,
    info: Option<(String, VideoInfo)>,
    probing: Option<String>,
    asked: Option<(String, String)>,
    fps: f64,
    position: f64,
    desired: Option<f32>,
}

impl Covers {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let id = cx.entity_id();
        for topic in [
            StoreEvent::NowPlaying,
            StoreEvent::Player,
            StoreEvent::AnimatedCover,
        ] {
            bridge::watch(cx, topic, id);
        }
        Self {
            picture: cx.new(|_| Picture::new()),
            info: None,
            probing: None,
            asked: None,
            fps: 12.,
            position: 0.,
            desired: None,
        }
    }

    /// Decodes at `side` points, for the largest view that shows it, or stops with `None`.
    pub fn set_desired(&mut self, side: Option<f32>, cx: &mut Context<Self>) {
        if self.desired != side {
            self.desired = side;
            cx.notify();
        }
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let side = self.desired;
        let store = crate::store(cx);
        let (key, playing) = {
            let state = store.state();
            let key = state.player.track.as_ref().and_then(|t| {
                Some((
                    t.artists.first()?.text.clone(),
                    t.album.as_ref()?.text.clone(),
                ))
            });
            (key, state.player.status == Status::Playing)
        };
        if key != self.asked {
            self.asked = key.clone();
            self.position = 0.;
            self.info = None;
            if let Some(key) = key.clone() {
                store.load_animated_cover(key);
            }
        }
        let path = key.as_ref().and_then(|k| {
            store
                .state()
                .animated_covers
                .get(k)
                .and_then(|e| e.path.clone())
        });
        let path_key = path.as_ref().map(|p| p.to_string_lossy().into_owned());
        if path_key.is_some()
            && self.info.as_ref().map(|(p, _)| p) != path_key.as_ref()
            && self.probing != path_key
        {
            self.probing = path_key.clone();
            let (p, page) = (
                path.clone().unwrap_or_else(|| Arc::from(Path::new(""))),
                cx.weak_entity(),
            );
            let wanted = path_key.clone().unwrap_or_default();
            self.info = None;
            bridge::task(
                cx,
                async move { video::probe(&p).await },
                move |info, cx| {
                    page.update(cx, |this, cx| {
                        this.probing = None;
                        if let Some(info) = info {
                            this.info = Some((wanted, info));
                        }
                        cx.notify();
                    });
                },
            );
        }
        let side = side.map(|s| (s * 2.).round() as u32);
        let ready = self
            .info
            .as_ref()
            .filter(|(p, _)| Some(p) == path_key.as_ref())
            .map(|(_, i)| i.clone());
        let picture = self.picture.clone();
        let running = picture.read(cx).source.clone();
        match (side, ready, playing) {
            (Some(side), Some(info), true) => {
                let source = format!("{}@{side}", path_key.clone().unwrap_or_default());
                if running.as_ref() != Some(&source) {
                    let Some(path) = path.clone() else { return };
                    let fps = if side > 300 { 24. } else { self.fps };
                    let (tx, rx) = mpsc::channel(FRAME_QUEUE);
                    let run = Loop::start(
                        store.runtime(),
                        &path,
                        &info,
                        side,
                        (fps, self.position),
                        hardware_decode(),
                        tx,
                    );
                    let weak = picture.downgrade();
                    let generation = picture.update(cx, |p, _| {
                        let g = p.next_generation();
                        p.run = Some(Run::Cover(run));
                        p.source = Some(source);
                        g
                    });
                    pump(&store, rx, move |bytes, cx| {
                        Picture::show(&weak, generation, bytes, cx)
                    });
                }
            }
            _ => {
                if running.is_some() {
                    picture.update(cx, |p, _| {
                        if let Some(Run::Cover(run)) = &p.run {
                            self.position = run.position();
                        }
                        p.stop();
                    });
                }
            }
        }
    }
}

impl Render for Covers {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(cx);
        div()
    }
}

/// The music video of the playing track, decoded muted and held to the daemon's clock.
pub struct MusicVideo {
    pub picture: Entity<Picture>,
    video_id: Option<String>,
    stream: Option<formalmusic_api::VideoStream>,
    requesting: bool,
    refreshed: bool,
    lead: Lead,
    desired: Option<f32>,
}

impl MusicVideo {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let id = cx.entity_id();
        for topic in [StoreEvent::NowPlaying, StoreEvent::Player] {
            bridge::watch(cx, topic, id);
        }
        Self {
            picture: cx.new(|_| Picture::new()),
            video_id: None,
            stream: None,
            requesting: false,
            refreshed: false,
            lead: Lead::default(),
            desired: None,
        }
    }

    /// Plays the video at `width` points, or stops with `None`.
    pub fn set_desired(&mut self, width: Option<f32>, cx: &mut Context<Self>) {
        if self.desired != width {
            self.desired = width;
            cx.notify();
        }
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let width = self.desired;
        let store = crate::store(cx);
        let (video_id, audible) = {
            let state = store.state();
            let video_id = state
                .player
                .track
                .as_ref()
                .and_then(|t| t.video_version())
                .map(str::to_owned);
            let audible = state.player.status == Status::Playing
                && video_id.is_some()
                && state.player.playing_id == video_id;
            (video_id, audible)
        };
        let video_id = video_id.filter(|_| width.is_some());
        if video_id != self.video_id {
            self.picture.update(cx, |p, _| p.stop());
            self.video_id = video_id.clone();
            self.stream = None;
            self.refreshed = false;
            self.lead = Lead::default();
            if video_id.is_some() {
                self.request(false, width.unwrap_or(640.), cx);
            }
        }
        let running = self.picture.read(cx).run.is_some();
        match (width, &self.stream, audible) {
            (Some(width), Some(stream), true) if !running => {
                let w = ((width * 2.).round() as u32).min(stream.width.max(2));
                let aspect = if stream.height > 0 {
                    stream.width as f32 / stream.height as f32
                } else {
                    16. / 9.
                };
                let h = (w as f32 / aspect).round() as u32;
                let id = self.video_id.clone().unwrap_or_default();
                let clock_store = store.clone();
                let clock: Clock = Arc::new(move || {
                    let state = clock_store.state();
                    let audible = state.player.status == Status::Playing
                        && state.player.playing_id.as_deref() == Some(id.as_str());
                    audible.then(|| state.position_now() as f64 / 1000.)
                });
                let (tx, rx) = mpsc::channel(FRAME_QUEUE);
                let run = Synced::start(
                    store.runtime(),
                    stream,
                    (w, h),
                    hardware_decode(),
                    &self.lead,
                    clock,
                    tx,
                );
                let weak = self.picture.downgrade();
                let generation = self.picture.update(cx, |p, _| {
                    let g = p.next_generation();
                    p.run = Some(Run::Music { _run: run });
                    p.source = self.video_id.clone();
                    g
                });
                pump(&store, rx, move |bytes, cx| {
                    Picture::show(&weak, generation, bytes, cx)
                });
            }
            (Some(_), _, true) => {}
            _ if running => self.picture.update(cx, |p, _| p.stop()),
            _ => {}
        }
    }

    fn request(&mut self, refresh: bool, width: f32, cx: &mut Context<Self>) {
        let Some(video_id) = self.video_id.clone() else {
            return;
        };
        self.requesting = true;
        let store = crate::store(cx);
        let page = cx.weak_entity();
        let max_height = (width * 2. * 9. / 16.).round() as u32;
        let id = video_id.clone();
        bridge::task(
            cx,
            async move { store.video_stream(id, max_height, refresh).await },
            move |stream, cx| {
                page.update(cx, |this, cx| {
                    if this.video_id.as_ref() != Some(&video_id) {
                        return;
                    }
                    this.requesting = false;
                    if stream.is_none() && !this.refreshed {
                        this.refreshed = true;
                        this.request(true, width, cx);
                        return;
                    }
                    this.stream = stream;
                    cx.notify();
                });
            },
        );
    }
}

impl Render for MusicVideo {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync(cx);
        div()
    }
}
