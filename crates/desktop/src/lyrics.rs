//! The expanded player's Lyrics tab, built to `docs/lyrics.md`: FormalShell's
//! lyrics pane (and Kopuz's before it) on GPUI.
//!
//! The pane is a clipped viewport over a column that is painted, not laid
//! out: every row is shaped once per track and width, and a `canvas` paints
//! the glyphs each frame with the row's scale, depth fade, blur and word wipe
//! applied by hand, since GPUI has no transforms, masks or filters on text.
//! Rows never change size with their state, so the column's travel is the
//! only thing that moves it.
//!
//! Frames are asked for every vsync only while a transition settles. While
//! the track plays and only a word wipes, they come at `WIPE_FPS`; between
//! words nothing is painted until the next one starts. Paused, the pane is
//! still.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use formalmusic_core::MusicStore;
use formalmusic_core::model::{Lyrics, Status};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::bridge::{Bridge, Topic};
use crate::icons::{IconName, icon_path};
use crate::motion::{DURATION_BASE, DURATION_FAST, Presence};
use crate::now_playing::placeholder;
use crate::primitives::IconButton;
use crate::theme::{Palette, Theme, radius, spacing, type_scale};

// Timing, in seconds (FS-M:60-96, KZ-L:36-54).
const POSITION_LEAD: f64 = 0.10;
const CARRY_GAP: f64 = 3.0;
const INTERLUDE_MIN: f64 = 5.0;
const LINE_ASSUMED: f64 = 7.0;
const CHUNK_FALLBACK: f64 = 0.35;
const WIPE_MAX: f64 = 1.2;
const GLOW_DECAY: f64 = 0.6;
/// The glow comes up over the opacity fade rather than in one frame.
const GLOW_RISE: f64 = 0.2;
const GLOW_QUANTUM: f32 = 0.05;
/// A word wipes a few pixels per frame at this rate, which reads as smooth.
const WIPE_FPS: f64 = 30.;

// Layout.
const COMFORT_OFFSET: f32 = 0.42;
const DUET_WIDTH: f32 = 0.90;
const ROW_PAD_X: f32 = 12.;
const ROW_PAD_Y: f32 = 6.;
const BACKGROUND_INDENT: f32 = 12.;
const BACKGROUND_SIZE: (f32, f32) = (18., 24.);
const NOTE_SIZE: f32 = 24.;
const EDGE_RAMP_MIN: f32 = 32.;
/// A wheel notch, for devices that report lines rather than pixels.
const WHEEL_LINE: f32 = 32.;

// States.
const SCALE_UNLIT: f32 = 0.85;
const SCALE_LIT: f32 = 1.0;
const SCALE_LIT_SECONDARY: f32 = 0.9;
const DEPTH_OPACITY: [f32; 4] = [1.0, 0.7, 0.45, 0.25];
const BACKGROUND_ALPHA: f32 = 0.7;
const BLUR_MAX_PX: f32 = 6.0;
const BLUR_QUANTUM_PX: f32 = 0.5;
const INTERLUDE_BASE_ALPHA: f32 = 0.35;
/// Blur copies every glyph nine times; past this many rows the paint
/// outgrows a 4 ms frame on e1504g (docs/lyrics.md, paint budget), so rows
/// further out get the five copies of the cheap form instead.
const BLURRED_ROWS: usize = 6;

// Wipe gradient: 2.2 chunk widths, sung to 46%, ramp to 54%, unsung after.
const WIPE_BAND: f32 = 0.176;
const WIPE_TRAVEL: f32 = 1.176;
/// Clip strips the soft band is painted in, each in its own blend of the two inks.
const WIPE_STRIPS: usize = 8;
const GLOW_ALPHA: f32 = 0.3;
const GLOW_RADIUS: (f32, f32) = (4.0, 6.0);

type Curve = (f32, f32, f32, f32);
type Motion = (Duration, Curve);

// FS-T:234-265.
const SCROLL: Motion = (Duration::from_millis(500), (0.38, 1.21, 0.22, 1.0));
const SCALE_ANIM: Motion = (Duration::from_millis(350), (0.42, 1.67, 0.21, 0.9));
const FADE: Motion = (Duration::from_millis(200), (0.34, 0.8, 0.34, 1.0));
const ARRIVAL: Motion = (Duration::from_millis(300), (0.34, 0.88, 0.34, 1.0));
const INK: Motion = ARRIVAL;

// ---------------------------------------------------------------------------
// Display lines and the lit set
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq)]
struct Line {
    start: f64,
    end: Option<f64>,
    text: String,
    words: Vec<Word>,
    background: bool,
    opposite: bool,
    /// An instrumental stretch the view adds; the providers send nothing for one.
    interlude: bool,
    /// The words were spread over the line by character count, not stamped.
    estimated: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct Word {
    start: f64,
    text: String,
    joins_next: bool,
}

/// What the view shows of one track: the display lines plus what the lit
/// set and the wipe read every frame, worked out once.
struct Model {
    lines: Vec<Line>,
    main: Vec<usize>,
    /// When each line stops being lit if it has no end of its own: the next
    /// main line's start, or for a background line the first main start after it.
    limit: Vec<Option<f64>>,
    /// Each chunk's `(start, end)`, per line.
    chunks: Vec<Vec<(f64, f64)>>,
    duet: bool,
    source: Option<String>,
}

fn secs(ms: u64) -> f64 {
    ms as f64 / 1000.
}

/// The wire lines as display lines. A line with no text marks where the
/// line before it stops, which is what LRC files mean by one.
fn raw_lines(lyrics: &Lyrics) -> Vec<Line> {
    let mut out: Vec<Line> = Vec::with_capacity(lyrics.lines.len());
    for line in &lyrics.lines {
        let start = secs(line.start_ms);
        if line.text.trim().is_empty() && line.words.is_empty() {
            if let Some(previous) = out.iter_mut().rev().find(|line| !line.background)
                && previous.end.is_none()
                && previous.start < start
            {
                previous.end = Some(start);
            }
            continue;
        }
        out.push(Line {
            start,
            end: line.end_ms.map(secs).filter(|end| *end > start),
            text: line.text.clone(),
            words: line
                .words
                .iter()
                .map(|word| Word {
                    start: secs(word.start_ms),
                    text: word.text.clone(),
                    joins_next: word.joins_next,
                })
                .collect(),
            background: line.background,
            opposite: line.opposite_turn,
            interlude: false,
            estimated: false,
        });
    }
    out
}

/// Every non-background line, or all of them when every line is background.
fn main_lines(lines: &[Line]) -> Vec<usize> {
    let main: Vec<usize> = (0..lines.len()).filter(|&i| !lines[i].background).collect();
    if main.is_empty() {
        (0..lines.len()).collect()
    } else {
        main
    }
}

fn next_main_start(lines: &[Line], main: &[usize], index: usize) -> Option<f64> {
    main.iter()
        .position(|&i| i == index)
        .and_then(|at| main.get(at + 1))
        .map(|&next| lines[next].start)
}

fn background_bound(lines: &[Line], main: &[usize], line: &Line) -> Option<f64> {
    if line.end.is_some() {
        return None;
    }
    main.iter()
        .map(|&i| lines[i].start)
        .find(|&start| start > line.start)
}

fn line_active_at(line: &Line, t: f64, next_main: Option<f64>) -> bool {
    if t < line.start {
        return false;
    }
    let Some(end) = line.end else {
        return next_main.is_none_or(|next| t < next);
    };
    if t <= end {
        return true;
    }
    next_main.is_some_and(|next| next > end && next - end <= CARRY_GAP && t < next)
}

/// Synthesised words are spread over this very estimate, so they never
/// answer it.
fn line_end_estimate(line: &Line) -> f64 {
    if let Some(end) = line.end {
        return end;
    }
    match line.words.last() {
        Some(word) if !line.estimated => word.start + CHUNK_FALLBACK,
        _ => line.start + LINE_ASSUMED,
    }
}

/// `lines` with an interlude row wherever the song goes quiet: before a
/// first line that starts after the carry gap, and between two runs whose
/// gap is an interlude long, or past the carry gap when the run has end
/// stamps and would otherwise sit dark (FS-M:1020-1094).
fn with_interludes(lines: Vec<Line>) -> Vec<Line> {
    let main = main_lines(&lines);
    let mut gaps: Vec<(usize, f64, f64)> = Vec::new();
    if let Some(&first) = main.first()
        && lines[first].start > CARRY_GAP
    {
        gaps.push((first, 0., lines[first].start));
    }
    for pair in main.windows(2) {
        let (current, next) = (pair[0], pair[1]);
        let next_start = lines[next].start;
        let run = &lines[current..next];
        let run_ends = run.iter().all(|line| line.end.is_some());
        let gap_start = run
            .iter()
            .map(line_end_estimate)
            .fold(f64::NEG_INFINITY, f64::max)
            .min(next_start)
            .max(lines[current].start);
        let gap = next_start - gap_start;
        if gap >= INTERLUDE_MIN || (run_ends && gap > CARRY_GAP) {
            gaps.push((next, gap_start, next_start));
        }
    }
    if gaps.is_empty() {
        return lines;
    }
    let mut out = Vec::with_capacity(lines.len() + gaps.len());
    let mut gaps = gaps.into_iter().peekable();
    for (index, line) in lines.into_iter().enumerate() {
        while let Some(&(_, start, end)) = gaps.peek().filter(|gap| gap.0 == index) {
            gaps.next();
            out.push(Line {
                start,
                end: Some(end),
                interlude: true,
                ..Line::default()
            });
        }
        out.push(line);
    }
    out
}

/// Line-synced lyrics get word timing spread over each line by character
/// count, so they wipe like word-synced ones (FS-M:714-761). Runs over the
/// display lines, so a line's span ends where the next row takes over.
fn synthesise_words(mut lines: Vec<Line>) -> Vec<Line> {
    let main = main_lines(&lines);
    for index in 0..lines.len() {
        let line = &lines[index];
        if line.interlude || !line.words.is_empty() || line.text.contains('\n') {
            continue;
        }
        let words: Vec<&str> = line.text.split_whitespace().collect();
        let total: usize = words.iter().map(|word| word.chars().count()).sum();
        if total == 0 {
            continue;
        }
        let span_end = line
            .end
            .or_else(|| next_main_start(&lines, &main, index))
            .unwrap_or(line.start + LINE_ASSUMED);
        let span = span_end - line.start;
        let mut before = 0;
        let timed = words
            .iter()
            .map(|word| {
                let start = if span > 0. {
                    line.start + span * before as f64 / total as f64
                } else {
                    line.start
                };
                before += word.chars().count();
                Word {
                    start,
                    text: (*word).to_owned(),
                    joins_next: false,
                }
            })
            .collect();
        lines[index].words = timed;
        lines[index].estimated = true;
    }
    lines
}

impl Model {
    fn new(lyrics: &Lyrics) -> Self {
        let lines = synthesise_words(with_interludes(raw_lines(lyrics)));
        let main = main_lines(&lines);
        let limit = lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                if line.background {
                    background_bound(&lines, &main, line)
                } else {
                    next_main_start(&lines, &main, index)
                }
            })
            .collect();
        let chunks = lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                // A row's own end, else the next row's start (FS-P:313-318).
                let line_end = line
                    .end
                    .or_else(|| lines.get(index + 1).map(|next| next.start));
                (0..line.words.len())
                    .map(|n| {
                        let start = line.words[n].start;
                        let end = line
                            .words
                            .get(n + 1)
                            .map(|next| next.start)
                            .or(line_end)
                            .filter(|end| *end > start)
                            .unwrap_or(start + CHUNK_FALLBACK);
                        (start, end)
                    })
                    .collect()
            })
            .collect();
        let duet = lines.iter().any(|line| line.opposite);
        Self {
            lines,
            main,
            limit,
            chunks,
            duet,
            source: lyrics.source.clone(),
        }
    }

    /// The active main line and every other line lit beside it: background
    /// lines on their own timing, duet lines only while a main line is lit.
    fn lit_at(&self, t: f64, lit: &mut Vec<bool>) -> Option<usize> {
        let active = self
            .main
            .iter()
            .copied()
            .take_while(|&i| self.lines[i].start <= t)
            .filter(|&i| line_active_at(&self.lines[i], t, self.limit[i]))
            .last();
        lit.clear();
        lit.extend(self.lines.iter().enumerate().map(|(i, line)| {
            Some(i) == active
                || (line_active_at(line, t, self.limit[i]) && (line.background || active.is_some()))
        }));
        active
    }

    /// Seconds from `t` until the pane next looks different: zero while a
    /// word wipes or glows, a half pixel of an interlude's note, else the
    /// next time a row lights or a word starts. `None` once nothing will.
    fn still_for(&self, t: f64, lit: &[bool], reduce: bool) -> Option<f64> {
        let mut next = f64::INFINITY;
        for (index, line) in self.lines.iter().enumerate() {
            let chunks = &self.chunks[index];
            let events = [Some(line.start), line.end, self.limit[index]];
            for at in events
                .into_iter()
                .flatten()
                .chain(chunks.iter().map(|c| c.0))
            {
                if at > t {
                    next = next.min(at);
                }
            }
            if !lit[index] || reduce {
                continue;
            }
            if line.interlude {
                if let Some(end) = line.end.filter(|end| *end > t) {
                    let step = (end - line.start) / f64::from(NOTE_SIZE * 2.);
                    next = next.min(t + step);
                }
                continue;
            }
            for &(start, end) in chunks {
                let wiping = chunk_fill((start, end), t, line.estimated, false) < 1.;
                let glowing = t < start + GLOW_RISE || (t >= end && t < end + GLOW_DECAY);
                if t >= start && (wiping || glowing) {
                    return Some(0.);
                }
            }
        }
        next.is_finite().then_some(next - t)
    }
}

/// How far chunk `(start, end)` has wiped at `t`. The 1.2 s cap keeps a
/// stamp held over a pause from creeping; synthesised words have no pause in
/// them, so they take their whole span.
fn chunk_fill((start, end): (f64, f64), t: f64, estimated: bool, reduce: bool) -> f32 {
    if reduce {
        return if t >= start { 1. } else { 0. };
    }
    let span = if estimated {
        end - start
    } else {
        (end - start).min(WIPE_MAX)
    };
    if span <= 0. {
        return if t >= start { 1. } else { 0. };
    }
    ((t - start) / span).clamp(0., 1.) as f32
}

fn chunk_glow((start, end): (f64, f64), t: f64) -> f32 {
    if t < start {
        return 0.;
    }
    let rise = (t - start) / GLOW_RISE;
    let glow = if t <= end {
        rise
    } else {
        rise.min(1. - (t - end) / GLOW_DECAY)
    };
    ((glow.clamp(0., 1.) as f32) / GLOW_QUANTUM).round() * GLOW_QUANTUM
}

/// The depth ramp sampled over the rows the pane has room for on that side.
fn depth_opacity(distance: f32, span: f32) -> f32 {
    let last = (DEPTH_OPACITY.len() - 1) as f32;
    let at = (distance.abs() * last / span).min(last);
    let low = at.floor() as usize;
    if low >= DEPTH_OPACITY.len() - 1 {
        return DEPTH_OPACITY[DEPTH_OPACITY.len() - 1];
    }
    DEPTH_OPACITY[low] + (DEPTH_OPACITY[low + 1] - DEPTH_OPACITY[low]) * at.fract()
}

fn blur_for(distance: f32, span: f32) -> f32 {
    let blur = (distance.abs() / span).min(1.) * BLUR_MAX_PX;
    (blur / BLUR_QUANTUM_PX).round() * BLUR_QUANTUM_PX
}

/// Wipe progress on band `index` of a chunk that wrapped: each band finishes
/// before the next starts, weighted by its ink width (FS-M:1204-1219).
fn band_fill(bands: &[Band], index: usize, fill: f32) -> f32 {
    if bands.len() <= 1 {
        return fill;
    }
    let total: f32 = bands.iter().map(Band::width).sum();
    let own = bands[index].width();
    if total <= 0. || own <= 0. {
        return if fill >= 1. { 1. } else { 0. };
    }
    let before: f32 = bands[..index].iter().map(Band::width).sum();
    ((fill * total - before) / own).clamp(0., 1.)
}

// ---------------------------------------------------------------------------
// Layout: shaped once per track and width
// ---------------------------------------------------------------------------

const NO_CHUNK: u32 = u32::MAX;

#[derive(Clone, Copy)]
struct Glyph {
    font_id: FontId,
    id: GlyphId,
    emoji: bool,
    /// Pane coordinates; `baseline` from the row's top.
    x: f32,
    end: f32,
    baseline: f32,
    chunk: u32,
    band: u8,
}

/// One text row's share of a chunk, in pane coordinates.
#[derive(Clone, Copy, Debug)]
struct Band {
    x0: f32,
    x1: f32,
}

impl Band {
    fn width(&self) -> f32 {
        self.x1 - self.x0
    }
}

struct Row {
    y: f32,
    height: f32,
    box_x: f32,
    box_w: f32,
    font_size: f32,
    glyphs: Vec<Glyph>,
    bands: Vec<Vec<Band>>,
}

struct Layout {
    width: f32,
    rows: Vec<Row>,
    /// The rows' total height; the footer sits under it.
    height: f32,
    footer: Option<Row>,
}

struct Style {
    font: Font,
    size: f32,
    line_height: f32,
}

/// A word to place: its glyphs relative to its own start, and its width.
struct Placed {
    glyphs: Vec<Glyph>,
    width: f32,
    /// A forced break before it: the `\n` of a merged translation.
    break_before: bool,
}

fn shape(window: &Window, text: &str, style: &Style) -> ShapedLine {
    window.text_system().shape_line(
        SharedString::from(text.to_owned()),
        px(style.size),
        &[TextRun {
            len: text.len(),
            font: style.font.clone(),
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    )
}

/// `text` shaped as one word; `bounds` are the byte offsets where each of
/// its chunks starts, `first` the index of the first chunk.
fn shape_word(
    window: &Window,
    text: &str,
    style: &Style,
    bounds: &[usize],
    first: Option<u32>,
) -> Placed {
    let shaped = shape(window, text, style);
    let mut glyphs: Vec<Glyph> = shaped
        .runs
        .iter()
        .flat_map(|run| run.glyphs.iter().map(move |glyph| (run.font_id, glyph)))
        .map(|(font_id, glyph)| Glyph {
            font_id,
            id: glyph.id,
            emoji: glyph.is_emoji,
            x: f32::from(glyph.position.x),
            end: 0.,
            baseline: 0.,
            chunk: first.map_or(NO_CHUNK, |first| {
                first
                    + bounds
                        .iter()
                        .rposition(|&at| at <= glyph.index)
                        .unwrap_or(0) as u32
            }),
            band: 0,
        })
        .collect();
    let width = f32::from(shaped.width);
    for n in 0..glyphs.len() {
        glyphs[n].end = glyphs.get(n + 1).map_or(width, |next| next.x);
    }
    Placed {
        glyphs,
        width,
        break_before: false,
    }
}

/// The row's words, each chunk group shaped as one word so kerning holds
/// inside it; a row without timing lays out from its text.
fn words_of(window: &Window, line: &Line, style: &Style) -> Vec<Placed> {
    if line.words.is_empty() {
        return line
            .text
            .split('\n')
            .flat_map(|physical| {
                physical
                    .split_whitespace()
                    .enumerate()
                    .map(move |(n, word)| (n == 0, word))
            })
            .enumerate()
            .map(|(n, (first, word))| Placed {
                break_before: first && n > 0,
                ..shape_word(window, word, style, &[0], None)
            })
            .collect();
    }
    let mut out = Vec::new();
    let mut start = 0;
    while start < line.words.len() {
        let mut end = start;
        while end + 1 < line.words.len() && line.words[end].joins_next {
            end += 1;
        }
        let mut text = String::new();
        let mut bounds = Vec::new();
        for word in &line.words[start..=end] {
            bounds.push(text.len());
            text.push_str(word.text.trim());
        }
        if !text.is_empty() {
            out.push(shape_word(
                window,
                &text,
                style,
                &bounds,
                Some(start as u32),
            ));
        }
        start = end + 1;
    }
    out
}

/// Greedy wrap into text rows at `width`, words spaced by the font's own
/// space; a word wider than the row breaks between its glyphs. Returns the
/// glyphs with `x` from the content's left edge and each text row's width.
fn wrap(words: Vec<Placed>, width: f32, space: f32) -> (Vec<(Glyph, usize)>, Vec<f32>) {
    let mut out = Vec::new();
    let mut widths = vec![0f32];
    let mut x = 0f32;
    for word in words {
        if word.break_before || (x > 0. && x + word.width > width) {
            widths.push(0.);
            x = 0.;
        }
        let mut row = widths.len() - 1;
        let mut shift = x;
        for glyph in word.glyphs {
            if glyph.end + shift > width && glyph.x + shift > 0. && word.width > width {
                widths.push(0.);
                row = widths.len() - 1;
                shift = -glyph.x;
            }
            let placed = Glyph {
                x: glyph.x + shift,
                end: glyph.end + shift,
                ..glyph
            };
            widths[row] = widths[row].max(placed.end);
            out.push((placed, row));
        }
        x = widths[widths.len() - 1] + space;
    }
    (out, widths)
}

impl Layout {
    fn new(model: &Model, width: f32, window: &Window) -> Self {
        let family = crate::theme::font_sans();
        let style = |background: bool, opposite: bool| {
            let (size, line_height) = if background {
                BACKGROUND_SIZE
            } else {
                (
                    f32::from(type_scale::LYRIC.font_size),
                    f32::from(type_scale::LYRIC.line_height),
                )
            };
            Style {
                font: Font {
                    family: family.clone(),
                    features: FontFeatures::default(),
                    fallbacks: None,
                    weight: if background {
                        FontWeight::MEDIUM
                    } else {
                        FontWeight::SEMIBOLD
                    },
                    style: if opposite {
                        FontStyle::Italic
                    } else {
                        FontStyle::Normal
                    },
                },
                size,
                line_height,
            }
        };
        let inner = (width - 2. * ROW_PAD_X).max(1.);
        let box_w = inner * if model.duet { DUET_WIDTH } else { 1. };
        let mut rows = Vec::with_capacity(model.lines.len());
        let mut y = 0.;
        for line in &model.lines {
            let box_x = ROW_PAD_X + if line.opposite { inner - box_w } else { 0. };
            if line.interlude {
                let height = NOTE_SIZE + 2. * ROW_PAD_Y;
                rows.push(Row {
                    y,
                    height,
                    box_x,
                    box_w,
                    font_size: NOTE_SIZE,
                    glyphs: Vec::new(),
                    bands: Vec::new(),
                });
                y += height;
                continue;
            }
            let style = style(line.background, line.opposite);
            let indent = if line.background {
                BACKGROUND_INDENT
            } else {
                0.
            };
            let content_x = box_x + if line.opposite { 0. } else { indent };
            let content_w = box_w - indent;
            let space = shape(window, " ", &style);
            let baseline = ROW_PAD_Y
                + (style.line_height - f32::from(space.ascent + space.descent)) / 2.
                + f32::from(space.ascent);
            let (placed, widths) = wrap(
                words_of(window, line, &style),
                content_w,
                f32::from(space.width),
            );
            let mut glyphs = Vec::with_capacity(placed.len());
            let mut bands: Vec<Vec<Band>> = vec![Vec::new(); line.words.len()];
            let mut last_band: Vec<Option<usize>> = vec![None; line.words.len()];
            for (glyph, row) in placed {
                let align = if line.opposite {
                    content_w - widths[row]
                } else {
                    0.
                };
                let mut glyph = Glyph {
                    x: content_x + align + glyph.x,
                    end: content_x + align + glyph.end,
                    baseline: baseline + row as f32 * style.line_height,
                    ..glyph
                };
                if let Some(chunk) = bands.get_mut(glyph.chunk as usize) {
                    let n = glyph.chunk as usize;
                    match last_band[n] {
                        Some(band) if band == row => {
                            let at = chunk.len() - 1;
                            chunk[at].x1 = chunk[at].x1.max(glyph.end);
                        }
                        _ => {
                            chunk.push(Band {
                                x0: glyph.x,
                                x1: glyph.end,
                            });
                            last_band[n] = Some(row);
                        }
                    }
                    glyph.band = (chunk.len() - 1) as u8;
                }
                glyphs.push(glyph);
            }
            let height = 2. * ROW_PAD_Y + widths.len() as f32 * style.line_height;
            rows.push(Row {
                y,
                height,
                box_x,
                box_w,
                font_size: style.size,
                glyphs,
                bands,
            });
            y += height;
        }
        let footer = model.source.as_ref().map(|source| {
            let style = Style {
                font: Font {
                    family: family.clone(),
                    features: FontFeatures::default(),
                    fallbacks: None,
                    weight: FontWeight::NORMAL,
                    style: FontStyle::Normal,
                },
                size: f32::from(type_scale::CAPTION.font_size),
                line_height: f32::from(type_scale::CAPTION.line_height),
            };
            let placed = shape_word(window, &format!("Lyrics from {source}"), &style, &[0], None);
            let space = shape(window, " ", &style);
            let baseline = ROW_PAD_Y
                + (style.line_height - f32::from(space.ascent + space.descent)) / 2.
                + f32::from(space.ascent);
            Row {
                y: y + f32::from(spacing::X6),
                height: 2. * ROW_PAD_Y + style.line_height,
                box_x: ROW_PAD_X,
                box_w: inner,
                font_size: style.size,
                glyphs: placed
                    .glyphs
                    .into_iter()
                    .map(|glyph| Glyph {
                        x: ROW_PAD_X + glyph.x,
                        end: ROW_PAD_X + glyph.end,
                        baseline,
                        ..glyph
                    })
                    .collect(),
                bands: Vec::new(),
            }
        });
        Self {
            width,
            rows,
            height: y,
            footer,
        }
    }
}

// ---------------------------------------------------------------------------
// Motion
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    duration: Duration,
    curve: Curve,
}

impl Tween {
    fn at(value: f32, now: Instant) -> Self {
        Self {
            from: value,
            to: value,
            start: now,
            duration: Duration::ZERO,
            curve: FADE.1,
        }
    }

    fn value(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.start);
        if elapsed >= self.duration {
            return self.to;
        }
        let x = elapsed.as_secs_f32() / self.duration.as_secs_f32();
        let (x1, y1, x2, y2) = self.curve;
        self.from + (self.to - self.from) * crate::motion::cubic_bezier(x1, y1, x2, y2)(x)
    }

    /// Restarts from wherever it is now, so a change mid-flight never jumps.
    fn toward(&mut self, to: f32, (duration, curve): Motion, now: Instant) -> Instant {
        if to != self.to {
            *self = Self {
                from: self.value(now),
                to,
                start: now,
                duration,
                curve,
            };
        }
        self.start + self.duration
    }

    fn restart(&mut self, from: f32, to: f32, (duration, curve): Motion, now: Instant) -> Instant {
        *self = Self {
            from,
            to,
            start: now,
            duration,
            curve,
        };
        now + duration
    }
}

struct RowMotion {
    lit: bool,
    scale: Tween,
    depth: Tween,
    blur: Tween,
    /// 1 while the row's sung ink shows; fades out when it goes dark.
    ink: Tween,
    arrival: Tween,
}

/// Where a row's motion is headed this frame.
#[derive(Clone, Copy)]
struct RowTarget {
    lit: bool,
    scale: f32,
    depth: f32,
    blur: f32,
}

impl RowMotion {
    fn new(now: Instant) -> Self {
        Self {
            lit: false,
            scale: Tween::at(SCALE_UNLIT, now),
            depth: Tween::at(1., now),
            blur: Tween::at(0., now),
            ink: Tween::at(0., now),
            arrival: Tween::at(1., now),
        }
    }

    fn settled(target: RowTarget, now: Instant) -> Self {
        Self {
            lit: target.lit,
            scale: Tween::at(target.scale, now),
            depth: Tween::at(target.depth, now),
            blur: Tween::at(target.blur, now),
            ink: Tween::at(if target.lit { 1. } else { 0. }, now),
            arrival: Tween::at(1., now),
        }
    }

    /// Steps every tween toward `target` and returns when they all land.
    fn follow(
        &mut self,
        target: RowTarget,
        timed: bool,
        motion: &impl Fn(Motion) -> Motion,
        now: Instant,
    ) -> Instant {
        let mut settle = now;
        if target.lit != self.lit {
            self.lit = target.lit;
            if target.lit {
                // The arrival fade takes over the depth ramp from whatever
                // opacity the row shows now. Restarting it at a fixed floor
                // (0.68 in both references) would dim a near row in one
                // frame before fading it back up.
                let shown = self.opacity(now);
                self.depth = Tween::at(1., now);
                settle = self.arrival.restart(shown, 1., motion(ARRIVAL), now);
            }
        }
        // Timed rows light at once and wipe from there, their first chunk
        // still unsung; a row with no timing crossfades its ink both ways.
        let ink = if target.lit { 1. } else { 0. };
        if target.lit && timed {
            self.ink = Tween::at(1., now);
        } else {
            settle = settle.max(self.ink.toward(ink, motion(INK), now));
        }
        settle = settle.max(self.scale.toward(target.scale, motion(SCALE_ANIM), now));
        settle = settle.max(self.depth.toward(target.depth, motion(FADE), now));
        settle.max(self.blur.toward(target.blur, motion(FADE), now))
    }

    fn opacity(&self, now: Instant) -> f32 {
        self.arrival.value(now) * self.depth.value(now)
    }
}

/// How far a row's blur copies share their alpha out. Zero at the 1px step
/// where the copies start, so the stack covers exactly what the single sharp
/// copy below it did and a row easing in or out of blur never steps.
fn spread_for(blur: f32) -> f32 {
    ((blur - 1.) / 0.5).clamp(0., 1.)
}

fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    Hsla::from(Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    })
}

fn fade(color: Hsla, alpha: f32) -> Hsla {
    Hsla {
        a: color.a * alpha,
        ..color
    }
}

/// Offsets that stand in for a gaussian: the four diagonals at `r`, the
/// four axes at `r / 2` between them so the copies fill in rather than read
/// as outlines, then the centre. The axes plus the centre are the cheap form.
fn taps(r: f32) -> [(f32, f32); 9] {
    std::array::from_fn(|k| {
        let (angle, radius) = match k {
            0..8 => (
                k as f32 * std::f32::consts::FRAC_PI_4,
                if k % 2 == 0 { r * 0.5 } else { r },
            ),
            _ => (0., 0.),
        };
        (radius * angle.cos(), radius * angle.sin())
    })
}

const CHEAP_TAPS: [usize; 5] = [0, 2, 4, 6, 8];

/// A text shadow's spread: two rings of eight, the inner one turned half a
/// step so the sixteen copies read as a haze rather than as outlines.
fn glow_taps(r: f32) -> [(f32, f32); 16] {
    std::array::from_fn(|k| {
        let step = std::f32::consts::FRAC_PI_4;
        let (angle, radius) = if k < 8 {
            (k as f32 * step, r)
        } else {
            ((k - 8) as f32 * step + step / 2., r * 0.5)
        };
        (radius * angle.cos(), radius * angle.sin())
    })
}

/// One of `copies` stacked copies of `color`. With `spread` at 0 the stack
/// covers the colour's own alpha where every copy overlaps; towards 1 the
/// copies share the alpha out instead, the way a blur or a shadow spends its
/// ink, so a blurred stroke does not read as a bolder weight.
fn copy(color: Hsla, copies: usize, spread: f32) -> Hsla {
    if copies <= 1 {
        return color;
    }
    let alpha = color.a.clamp(0., 0.999);
    let stacked = 1. - (1. - alpha).powf(1. / copies as f32);
    let shared = alpha / copies as f32;
    Hsla {
        a: stacked + (shared - stacked) * spread,
        ..color
    }
}

// ---------------------------------------------------------------------------
// The pane
// ---------------------------------------------------------------------------

/// What one frame paints from, read in `render`.
#[derive(Clone, Copy)]
struct Frame {
    t: f64,
    playing: bool,
    reduce: bool,
    palette: Palette,
}

/// Everything the canvas needs between frames. Shared with the mouse
/// handlers, which read the row rectangles the last frame painted.
struct Pane {
    lyrics: Option<Arc<Lyrics>>,
    model: Option<Model>,
    layout: Option<Layout>,
    rows: Vec<RowMotion>,
    lit: Vec<bool>,
    anchor: usize,
    column: Tween,
    follow: bool,
    wheel_y: f32,
    hovered: Option<usize>,
    /// The last frame's viewport and each painted row's `(index, top, bottom)`
    /// in window coordinates.
    bounds: Bounds<Pixels>,
    hits: Vec<(usize, f32, f32)>,
    /// When the last running transition settles.
    settle: Instant,
    snap: bool,
    /// The repaint asked for once the pane next changes.
    wake: Option<Task<()>>,
    stats: Stats,
}

#[derive(Default)]
struct Stats {
    frames: u32,
    total: Duration,
    worst: Duration,
    sprites: usize,
}

impl Pane {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            lyrics: None,
            model: None,
            layout: None,
            rows: Vec::new(),
            lit: Vec::new(),
            anchor: 0,
            column: Tween::at(0., now),
            follow: true,
            wheel_y: 0.,
            hovered: None,
            bounds: Bounds::default(),
            hits: Vec::new(),
            settle: now,
            snap: true,
            wake: None,
            stats: Stats::default(),
        }
    }

    fn set_lyrics(&mut self, lyrics: Arc<Lyrics>) {
        if self
            .lyrics
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &lyrics))
        {
            return;
        }
        let model = Model::new(&lyrics);
        let now = Instant::now();
        self.rows = model.lines.iter().map(|_| RowMotion::new(now)).collect();
        self.model = Some(model);
        self.lyrics = Some(lyrics);
        self.layout = None;
        self.anchor = 0;
        self.follow = true;
        self.hovered = None;
        self.snap = true;
    }

    fn row_at(&self, position: Point<Pixels>) -> Option<usize> {
        if !self.bounds.contains(&position) {
            return None;
        }
        let y = f32::from(position.y);
        self.hits
            .iter()
            .find(|(_, top, bottom)| y >= *top && y < *bottom)
            .map(|(index, _, _)| *index)
    }

    fn start(&self, row: usize) -> Option<u64> {
        let line = self.model.as_ref()?.lines.get(row)?;
        Some((line.start * 1000.).round().max(0.) as u64)
    }

    /// Where the column rests with `row`'s top at the comfort offset.
    fn resting(&self, row: usize) -> f32 {
        let height = f32::from(self.bounds.size.height);
        let y = self
            .layout
            .as_ref()
            .and_then(|layout| layout.rows.get(row))
            .map_or(0., |row| row.y);
        height * COMFORT_OFFSET - y
    }

    fn scroll(&mut self, delta: f32) {
        let rows = self.layout.as_ref().map_or(0, |layout| layout.rows.len());
        if rows == 0 {
            return;
        }
        let now = Instant::now();
        if self.follow {
            self.follow = false;
            self.wheel_y = self.column.value(now);
        }
        let (top, bottom) = (self.resting(0), self.resting(rows - 1));
        self.wheel_y = (self.wheel_y + delta).clamp(bottom.min(top), top);
        self.column = Tween::at(self.wheel_y, now);
    }

    fn paint(&mut self, bounds: Bounds<Pixels>, frame: Frame, window: &mut Window, cx: &App) {
        let started = Instant::now();
        let Some(model) = self.model.take() else {
            return;
        };
        let width = f32::from(bounds.size.width);
        if self
            .layout
            .as_ref()
            .is_none_or(|layout| layout.width != width)
        {
            self.layout = Some(Layout::new(&model, width, window));
        }
        self.bounds = bounds;
        let sprites = self.paint_model(&model, frame, window, cx);
        let still = frame
            .playing
            .then(|| model.still_for(frame.t, &self.lit, frame.reduce))
            .flatten();
        self.model = Some(model);

        let now = Instant::now();
        self.wake = None;
        if now < self.settle {
            window.request_animation_frame();
        } else if let Some(still) = still {
            let delay = Duration::from_secs_f64(still.max(1. / WIPE_FPS));
            let view = window.current_view();
            self.wake = Some(window.spawn(cx, async move |cx| {
                cx.background_executor().timer(delay).await;
                let _ = cx.update(|_, cx| cx.notify(view));
            }));
        }
        if crate::trace::enabled() {
            let spent = now - started;
            let stats = &mut self.stats;
            stats.frames += 1;
            stats.total += spent;
            stats.worst = stats.worst.max(spent);
            stats.sprites = stats.sprites.max(sprites);
            if stats.frames == 120 {
                crate::trace::log(&format!(
                    "lyrics paint: mean {:.3}ms, worst {:.3}ms, up to {} glyph sprites",
                    stats.total.as_secs_f64() * 1000. / 120.,
                    stats.worst.as_secs_f64() * 1000.,
                    stats.sprites
                ));
                *stats = Stats::default();
            }
        }
    }

    /// Paints the column and returns how many glyph sprites it drew.
    fn paint_model(&mut self, model: &Model, frame: Frame, window: &mut Window, cx: &App) -> usize {
        let now = Instant::now();
        let reduce = frame.reduce;
        let motion = |m: Motion| if reduce { (Duration::ZERO, m.1) } else { m };
        let Some(layout) = self.layout.as_ref() else {
            return 0;
        };
        let viewport = f32::from(self.bounds.size.height);
        let origin = self.bounds.origin;

        // The lit set and the anchor the depth ramp and the scroll hang off.
        let mut lit = std::mem::take(&mut self.lit);
        let active = model.lit_at(frame.t, &mut lit);
        let secondary = lit.iter().rposition(|lit| *lit);
        self.anchor = active.or(secondary).unwrap_or(self.anchor);
        let anchor = self.anchor;

        let pitch = if layout.rows.is_empty() {
            1.
        } else {
            layout.height / layout.rows.len() as f32
        };
        let span_above = (viewport * COMFORT_OFFSET / pitch).max(1.);
        let span_below = (viewport * (1. - COMFORT_OFFSET) / pitch).max(1.);

        let mut settle = self.settle;
        for (index, (line, state)) in model.lines.iter().zip(&mut self.rows).enumerate() {
            let is_lit = lit[index];
            let distance = index as f32 - anchor as f32;
            let span = if distance < 0. {
                span_above
            } else {
                span_below
            };
            let scale = if !is_lit {
                SCALE_UNLIT
            } else if line.background || line.interlude {
                SCALE_LIT_SECONDARY
            } else {
                SCALE_LIT
            };
            let depth = if is_lit {
                1.
            } else {
                depth_opacity(distance, span)
            };
            let blur = if is_lit || self.hovered == Some(index) {
                0.
            } else {
                blur_for(distance, span)
            };
            let target = RowTarget {
                lit: is_lit,
                scale,
                depth,
                blur,
            };
            if self.snap {
                *state = RowMotion::settled(target, now);
                continue;
            }
            settle = settle.max(state.follow(target, !line.words.is_empty(), &motion, now));
        }

        // The column: the anchor's top at the comfort offset while following,
        // wherever the wheel left it otherwise.
        let target = if self.follow {
            viewport * COMFORT_OFFSET - layout.rows.get(anchor).map_or(0., |row| row.y)
        } else {
            self.wheel_y
        };
        if self.snap {
            self.column = Tween::at(target, now);
            self.snap = false;
        } else if self.follow {
            settle = settle.max(self.column.toward(target, motion(SCROLL), now));
        }
        self.settle = settle;
        let column = self.column.value(now);

        let palette = frame.palette;
        let edge_ramp =
            EDGE_RAMP_MIN.max(f32::from(type_scale::LYRIC.line_height) + 2. * ROW_PAD_Y);
        let edge = |top: f32, height: f32| {
            ((top.min(viewport - (top + height))) / edge_ramp).clamp(0., 1.)
        };
        let pane = self.bounds;
        let mut painter = Painter {
            window,
            cx,
            origin,
            sprites: 0,
            spread: 0.,
        };
        self.hits.clear();
        let width = layout.width;
        let mut blurred: Vec<usize> = layout
            .rows
            .iter()
            .enumerate()
            .filter(|(index, row)| {
                let top = column + row.y;
                top + row.height >= 0. && top <= viewport && self.rows[*index].blur.value(now) >= 1.
            })
            .map(|(index, _)| index)
            .collect();
        blurred.sort_by_key(|index| index.abs_diff(anchor));
        blurred.truncate(BLURRED_ROWS);
        for (index, row) in layout.rows.iter().enumerate() {
            let top = column + row.y;
            if top + row.height < 0. || top > viewport {
                continue;
            }
            let state = &self.rows[index];
            let line = &model.lines[index];
            let edge = edge(top, row.height);
            let cell = edge * state.arrival.value(now);
            self.hits.push((
                index,
                f32::from(origin.y) + top,
                f32::from(origin.y) + top + row.height,
            ));
            if cell <= 0.004 {
                continue;
            }
            if self.hovered == Some(index) {
                painter.window.paint_quad(
                    fill(
                        Bounds::new(
                            origin + point(px(0.), px(top)),
                            size(px(width), px(row.height)),
                        ),
                        fade(palette.hover_wash, cell),
                    )
                    .corner_radii(radius::ROW),
                );
            }
            let alpha = cell
                * state.depth.value(now)
                * if line.background {
                    BACKGROUND_ALPHA
                } else {
                    1.
                };
            if alpha <= 0.004 {
                continue;
            }
            let scale = state.scale.value(now);
            let center_y = top + row.height / 2.;
            let pivot_x = if line.interlude && !model.duet {
                row.box_x + row.box_w / 2.
            } else if line.opposite {
                row.box_x + row.box_w
            } else {
                row.box_x
            };
            let transform = Transform {
                pivot: (pivot_x, center_y),
                scale,
                top,
            };
            let blur = state.blur.value(now);
            let ring = taps(blur * 0.6);
            let cheap = CHEAP_TAPS.map(|k| ring[k]);
            // Under a pixel the copies thicken the stroke more than they
            // soften it, so the faintest step of the ramp paints sharp.
            let offsets: &[(f32, f32)] = if blur < 1. {
                &ring[8..]
            } else if blurred.contains(&index) {
                &ring
            } else {
                &cheap
            };
            painter.spread = spread_for(blur);
            if line.interlude {
                let progress = if lit[index] {
                    let end = line.end.unwrap_or(line.start);
                    if end > line.start {
                        ((frame.t - line.start) / (end - line.start)).clamp(0., 1.) as f32
                    } else if frame.t >= line.start {
                        1.
                    } else {
                        0.
                    }
                } else {
                    0.
                };
                painter.note(
                    row,
                    model.duet,
                    &transform,
                    progress,
                    fade(palette.secondary, alpha * INTERLUDE_BASE_ALPHA),
                    fade(palette.text, alpha),
                    offsets,
                );
                continue;
            }
            let ink = state.ink.value(now);
            let sung = mix(palette.secondary, palette.text, ink);
            let chunks = &model.chunks[index];
            let flat = if ink <= 0.001 {
                Some(palette.secondary)
            } else if line.words.is_empty() {
                Some(sung)
            } else {
                None
            };
            if let Some(color) = flat {
                painter.layer(pane, |painter| {
                    painter.glyphs(row, &transform, |_| fade(color, alpha), offsets)
                });
                continue;
            }
            // The glow sits under the chunk being sung, only on lit rows.
            if lit[index] && !reduce {
                for (n, times) in chunks.iter().enumerate() {
                    let glow = chunk_glow(*times, frame.t);
                    if glow > 0. {
                        let radius = (GLOW_RADIUS.0 + GLOW_RADIUS.1 * glow) / 2.;
                        let ring = glow_taps(radius);
                        let color = fade(palette.text, GLOW_ALPHA * glow * alpha);
                        painter.spread = 1.;
                        painter.glyphs_where(
                            row,
                            &transform,
                            |glyph| glyph.chunk == n as u32,
                            |_| color,
                            &ring,
                        );
                    }
                }
            }
            let fills: Vec<f32> = chunks
                .iter()
                .map(|times| chunk_fill(*times, frame.t, line.estimated, reduce))
                .collect();
            painter.wipe(
                row,
                &transform,
                &fills,
                fade(sung, alpha),
                fade(palette.secondary, alpha),
                offsets,
            );
        }
        if let Some(footer) = &layout.footer {
            let top = column + layout.height + (footer.y - layout.height);
            let alpha = edge(top, footer.height);
            if alpha > 0.004 && top < viewport && top + footer.height > 0. {
                let transform = Transform {
                    pivot: (footer.box_x, top + footer.height / 2.),
                    scale: 1.,
                    top,
                };
                painter.glyphs(
                    footer,
                    &transform,
                    |_| fade(palette.tertiary, alpha),
                    &[(0., 0.)],
                );
            }
        }
        self.lit = lit;
        painter.sprites
    }
}

/// A row's place this frame: scaled about `pivot`, its top at `top`, both in
/// viewport coordinates.
struct Transform {
    pivot: (f32, f32),
    scale: f32,
    top: f32,
}

impl Transform {
    fn x(&self, x: f32) -> f32 {
        self.pivot.0 + (x - self.pivot.0) * self.scale
    }

    fn y(&self, y: f32) -> f32 {
        self.pivot.1 + (self.top + y - self.pivot.1) * self.scale
    }

    /// The glyph size, on quarter pixels so a scale animation reuses a
    /// handful of atlas entries instead of one per frame.
    fn size(&self, size: f32) -> f32 {
        (size * self.scale * 4.).round() / 4.
    }
}

struct Painter<'a> {
    window: &'a mut Window,
    cx: &'a App,
    origin: Point<Pixels>,
    sprites: usize,
    /// How far the current row's copies share their alpha out (see `copy`).
    spread: f32,
}

impl Painter<'_> {
    fn glyph(&mut self, glyph: &Glyph, size: f32, at: (f32, f32), color: Hsla) {
        let origin = self.origin + point(px(at.0), px(at.1));
        self.sprites += 1;
        let _ = if glyph.emoji {
            self.window
                .paint_emoji(origin, glyph.font_id, glyph.id, px(size))
        } else {
            self.window
                .paint_glyph(origin, glyph.font_id, glyph.id, px(size), color)
        };
    }

    /// Paints into one layer of the scene. A layer takes a single place in
    /// the scene's bounds tree, where each sprite would otherwise search it
    /// for its own; the blur copies stacked over each other made that most
    /// of a frame. Inside a layer sprites are drawn grouped by atlas tile,
    /// which only blends the same as painting order when they share a colour.
    fn layer(&mut self, bounds: Bounds<Pixels>, paint: impl FnOnce(&mut Painter<'_>)) {
        let (cx, origin, spread) = (self.cx, self.origin, self.spread);
        self.sprites += self.window.paint_layer(bounds, |window| {
            let mut painter = Painter {
                window,
                cx,
                origin,
                sprites: 0,
                spread,
            };
            paint(&mut painter);
            painter.sprites
        });
    }

    fn glyphs(
        &mut self,
        row: &Row,
        transform: &Transform,
        color: impl Fn(&Glyph) -> Hsla,
        offsets: &[(f32, f32)],
    ) {
        self.glyphs_where(row, transform, |_| true, color, offsets);
    }

    /// Every matching glyph once per offset.
    fn glyphs_where(
        &mut self,
        row: &Row,
        transform: &Transform,
        only: impl Fn(&Glyph) -> bool,
        color: impl Fn(&Glyph) -> Hsla,
        offsets: &[(f32, f32)],
    ) {
        let size = transform.size(row.font_size);
        for glyph in row.glyphs.iter().filter(|glyph| only(glyph)) {
            let tap = copy(color(glyph), offsets.len(), self.spread);
            let at = (transform.x(glyph.x), transform.y(glyph.baseline));
            for (dx, dy) in offsets {
                self.glyph(glyph, size, (at.0 + dx, at.1 + dy), tap);
            }
        }
    }

    /// The word wipe: left of a chunk's edge in sung ink, right of it in
    /// unsung, and the soft band between painted in strips, each clipped to
    /// its slice and tinted its share of the way between the two.
    fn wipe(
        &mut self,
        row: &Row,
        transform: &Transform,
        fills: &[f32],
        sung: Hsla,
        unsung: Hsla,
        offsets: &[(f32, f32)],
    ) {
        let size = transform.size(row.font_size);
        let spread = self.spread;
        let tap = |color: Hsla| copy(color, offsets.len(), spread);
        let (sung_tap, unsung_tap) = (tap(sung), tap(unsung));
        let row_top = transform.y(0.) - size;
        let row_bottom = transform.y(row.height) + size;
        for glyph in &row.glyphs {
            let at = (transform.x(glyph.x), transform.y(glyph.baseline));
            let Some(bands) = row.bands.get(glyph.chunk as usize) else {
                for (dx, dy) in offsets {
                    self.glyph(glyph, size, (at.0 + dx, at.1 + dy), unsung_tap);
                }
                continue;
            };
            let band = bands[glyph.band as usize];
            let fill = band_fill(bands, glyph.band as usize, fills[glyph.chunk as usize]);
            let w = band.width();
            let hi = band.x0 + WIPE_TRAVEL * w * fill;
            let lo = hi - WIPE_BAND * w;
            if glyph.end <= lo {
                for (dx, dy) in offsets {
                    self.glyph(glyph, size, (at.0 + dx, at.1 + dy), sung_tap);
                }
                continue;
            }
            if glyph.x >= hi {
                for (dx, dy) in offsets {
                    self.glyph(glyph, size, (at.0 + dx, at.1 + dy), unsung_tap);
                }
                continue;
            }
            // Regions in layout x, outermost ones open-ended.
            let reach = row.font_size;
            let mut regions: Vec<(f32, f32, Hsla)> = Vec::with_capacity(WIPE_STRIPS + 2);
            regions.push((glyph.x - reach, lo, sung_tap));
            let step = (hi - lo) / WIPE_STRIPS as f32;
            for strip in 0..WIPE_STRIPS {
                let share = (strip as f32 + 0.5) / WIPE_STRIPS as f32;
                regions.push((
                    lo + step * strip as f32,
                    lo + step * (strip + 1) as f32,
                    mix(sung_tap, unsung_tap, share),
                ));
            }
            regions.push((hi, glyph.end + reach, unsung_tap));
            for (x0, x1, color) in regions {
                if x1 <= glyph.x - reach || x0 >= glyph.end + reach || x1 <= x0 {
                    continue;
                }
                let clip = Bounds::from_corners(
                    self.origin + point(px(transform.x(x0)), px(row_top)),
                    self.origin + point(px(transform.x(x1)), px(row_bottom)),
                );
                self.window
                    .with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                        for (dx, dy) in offsets {
                            let origin = self.origin + point(px(at.0 + dx), px(at.1 + dy));
                            let _ = window.paint_glyph(
                                origin,
                                glyph.font_id,
                                glyph.id,
                                px(size),
                                color,
                            );
                        }
                    });
                self.sprites += offsets.len();
            }
        }
    }

    /// The interlude's note: a faint copy, and the lit copy clipped to how
    /// far the gap has run.
    #[allow(clippy::too_many_arguments)]
    fn note(
        &mut self,
        row: &Row,
        duet: bool,
        transform: &Transform,
        progress: f32,
        base: Hsla,
        lit: Hsla,
        offsets: &[(f32, f32)],
    ) {
        let left = if duet {
            row.box_x
        } else {
            row.box_x + (row.box_w - NOTE_SIZE) / 2.
        };
        let note = (NOTE_SIZE * transform.scale * 2.).round() / 2.;
        let x = transform.x(left);
        let y = transform.y(ROW_PAD_Y);
        let bounds = Bounds::new(self.origin + point(px(x), px(y)), size(px(note), px(note)));
        let path = icon_path(IconName::Music, false, false);
        let spread = self.spread;
        let tap = |color: Hsla| copy(color, offsets.len(), spread);
        for (dx, dy) in offsets {
            let at = bounds.origin + point(px(*dx), px(*dy));
            let _ = self.window.paint_svg(
                Bounds::new(at, bounds.size),
                path.clone(),
                None,
                TransformationMatrix::unit(),
                tap(base),
                self.cx,
            );
        }
        if progress > 0. {
            let clip = Bounds::new(bounds.origin, size(px(note * progress), px(note)));
            self.window
                .with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                    let _ = window.paint_svg(
                        bounds,
                        path.clone(),
                        None,
                        TransformationMatrix::unit(),
                        lit,
                        self.cx,
                    );
                });
        }
    }
}

// ---------------------------------------------------------------------------
// The view
// ---------------------------------------------------------------------------

pub struct LyricsView {
    store: MusicStore,
    key: Option<String>,
    pane: Rc<RefCell<Pane>>,
    resync: Presence<()>,
}

impl LyricsView {
    pub fn new(store: MusicStore, cx: &mut Context<Self>) -> Self {
        let weak = cx.entity().downgrade();
        Bridge::watch(cx, Topic::NowPlaying, weak.clone().into());
        Bridge::watch(cx, Topic::Position, weak.into());
        Self {
            store,
            key: None,
            pane: Rc::new(RefCell::new(Pane::new())),
            resync: Presence::new(DURATION_FAST),
        }
    }

    /// Hands the column back to the song.
    pub fn resume(&mut self, cx: &mut Context<Self>) {
        self.pane.borrow_mut().follow = true;
        cx.notify();
    }

    fn plain(&self, lyrics: &Lyrics, palette: Palette) -> AnyElement {
        div()
            .id("plain-lyrics")
            .size_full()
            .overflow_y_scroll()
            .px(px(ROW_PAD_X))
            .pb(spacing::X8)
            .flex()
            .flex_col()
            .text_size(type_scale::LYRIC.font_size)
            .line_height(type_scale::LYRIC.line_height)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(palette.text)
            .children(lyrics.lines.iter().map(|line| {
                div()
                    .py(px(ROW_PAD_Y))
                    .min_h(type_scale::LYRIC.line_height)
                    .child(line.text.clone())
            }))
            .children(lyrics.source.as_ref().map(|source| {
                div()
                    .pt(spacing::X6)
                    .text_size(type_scale::CAPTION.font_size)
                    .line_height(type_scale::CAPTION.line_height)
                    .font_weight(FontWeight::NORMAL)
                    .text_color(palette.tertiary)
                    .child(format!("Lyrics from {source}"))
            }))
            .into_any_element()
    }
}

impl Render for LyricsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::trace::render("LyricsView");
        let palette = Theme::get(cx);
        let (key, entry, position_ms, playing) = {
            let state = self.store.state();
            let key = state.current_key().map(str::to_owned);
            let entry = key.as_ref().and_then(|id| state.lyrics.get(id).cloned());
            (
                key,
                entry,
                state.position_now(),
                state.player.status == Status::Playing,
            )
        };
        if key != self.key {
            let weak = cx.entity().downgrade();
            if let Some(old) = self.key.take() {
                Bridge::unwatch(cx, &Topic::Lyrics(old), &weak.clone().into());
            }
            if let Some(id) = &key {
                Bridge::watch(cx, Topic::Lyrics(id.clone()), weak.into());
                self.store.load_lyrics(id.clone());
            }
            self.key = key;
        }
        let Some(lyrics) = entry.as_ref().and_then(|entry| entry.lyrics.clone()) else {
            let missing = entry.as_ref().is_some_and(|entry| entry.missing);
            return placeholder(
                if missing {
                    "No lyrics for this song."
                } else {
                    "Loading lyrics\u{2026}"
                },
                palette,
            );
        };
        if !lyrics.synced {
            return self.plain(&lyrics, palette);
        }
        self.pane.borrow_mut().set_lyrics(lyrics);
        let (follow, hovered) = {
            let pane = self.pane.borrow();
            (pane.follow, pane.hovered)
        };
        self.resync.set(
            (!follow).then_some(()),
            |this: &mut Self| &mut this.resync,
            cx,
        );
        let frame = Frame {
            t: position_ms as f64 / 1000. + POSITION_LEAD,
            playing,
            reduce: cx.reduce_motion(),
            palette,
        };
        let pane = self.pane.clone();
        let resync = self.resync.current().map(|_| {
            let open = self.resync.is_open();
            let view = cx.entity().downgrade();
            crate::motion::toward(
                div()
                    .absolute()
                    .right(spacing::X4)
                    .bottom(spacing::X4)
                    .rounded(radius::PILL)
                    .bg(palette.overlay)
                    .shadow(crate::primitives::overlay_shadows(&palette))
                    .child(
                        IconButton::new("lyrics-resync", IconName::Resync, "Follow the song")
                            .hit(px(36.))
                            .size(px(16.))
                            .color(palette.text)
                            .on_click(move |_, _, cx| {
                                let _ = view.update(cx, |this, cx| this.resume(cx));
                            }),
                    ),
                self.resync.id("lyrics-resync"),
                open,
                DURATION_BASE,
                DURATION_FAST,
                |el, t| el.opacity(t),
            )
        });
        let (hover_pane, click_pane, wheel_pane) = (pane.clone(), pane.clone(), pane.clone());
        let store = self.store.clone();
        div()
            .id("lyrics")
            .relative()
            .size_full()
            .overflow_hidden()
            .when(hovered.is_some(), |el| el.cursor_pointer())
            .on_mouse_move(cx.listener(move |_, event: &MouseMoveEvent, _, cx| {
                let mut pane = hover_pane.borrow_mut();
                let row = pane.row_at(event.position);
                if row != pane.hovered {
                    pane.hovered = row;
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !hovered {
                    this.pane.borrow_mut().hovered = None;
                    cx.notify();
                }
            }))
            .on_click(move |event, _, _| {
                let pane = click_pane.borrow();
                if let Some(start) = pane
                    .row_at(event.position())
                    .and_then(|row| pane.start(row))
                {
                    store.seek(start);
                }
            })
            .on_scroll_wheel(cx.listener(move |_, event: &ScrollWheelEvent, _, cx| {
                let delta = match event.delta {
                    ScrollDelta::Pixels(delta) => f32::from(delta.y),
                    ScrollDelta::Lines(delta) => delta.y * WHEEL_LINE,
                };
                if delta != 0. {
                    wheel_pane.borrow_mut().scroll(delta);
                    cx.notify();
                }
            }))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| pane.borrow_mut().paint(bounds, frame, window, cx),
                )
                .size_full(),
            )
            .children(resync)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use formalmusic_core::model::{LyricLine, LyricWord};

    fn line(start_ms: u64, end_ms: Option<u64>, text: &str) -> LyricLine {
        LyricLine {
            start_ms,
            end_ms,
            text: text.into(),
            ..LyricLine::default()
        }
    }

    fn lyrics(lines: Vec<LyricLine>) -> Lyrics {
        Lyrics {
            source: None,
            lines,
            synced: true,
            word_synced: false,
        }
    }

    fn lit(model: &Model, t: f64) -> Vec<usize> {
        let mut lit = Vec::new();
        model.lit_at(t, &mut lit);
        (0..lit.len()).filter(|&i| lit[i]).collect()
    }

    #[::core::prelude::v1::test]
    fn a_short_gap_carries_the_line_and_a_long_one_gets_a_note() {
        let model = Model::new(&lyrics(vec![
            line(1_000, Some(3_000), "one"),
            line(5_000, Some(7_000), "two"),
            line(14_000, None, "three"),
        ]));
        assert!(!model.lines[0].interlude, "a 1 s run-in gets no note");
        assert_eq!(lit(&model, 4.0), vec![0], "2 s gap: the line stays lit");
        let note = model.lines.iter().position(|line| line.interlude).unwrap();
        assert_eq!(note, 2);
        assert_eq!(model.lines[note].start, 7.0);
        assert_eq!(model.lines[note].end, Some(14.0));
        assert_eq!(lit(&model, 9.0), vec![note]);
    }

    #[::core::prelude::v1::test]
    fn the_three_to_five_second_hole_is_closed_when_the_run_has_end_stamps() {
        let model = Model::new(&lyrics(vec![
            line(1_000, Some(3_000), "one"),
            line(7_000, Some(9_000), "two"),
        ]));
        assert!(model.lines[1].interlude);
        assert_eq!(lit(&model, 5.0), vec![1]);
    }

    #[::core::prelude::v1::test]
    fn background_lines_light_on_their_own_timing_and_duets_only_beside_a_main_line() {
        let mut backing = line(2_000, Some(6_000), "(oh)");
        backing.background = true;
        let mut other = line(2_500, Some(4_000), "you");
        other.opposite_turn = true;
        let model = Model::new(&lyrics(vec![
            line(1_000, Some(3_000), "me"),
            backing,
            other,
            line(4_500, Some(8_000), "us"),
        ]));
        assert!(model.duet);
        assert_eq!(lit(&model, 2.7), vec![0, 1, 2]);
        assert_eq!(lit(&model, 5.0), vec![1, 3]);
    }

    #[::core::prelude::v1::test]
    fn line_synced_lyrics_get_words_spread_by_characters() {
        let model = Model::new(&lyrics(vec![
            line(1_000, None, "ab cdef"),
            line(4_000, None, "next"),
        ]));
        let words = &model.lines[0].words;
        assert!(model.lines[0].estimated);
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].start, 1.0);
        assert!((words[1].start - 2.0).abs() < 1e-9, "a third of 3 s in");
        assert_eq!(model.chunks[0][1], (2.0, 4.0));
        assert_eq!(chunk_fill(model.chunks[0][1], 3.0, true, false), 0.5);
        assert_eq!(
            chunk_fill((1.0, 4.0), 1.6, false, false),
            0.5,
            "stamped words cap at 1.2 s"
        );
    }

    #[::core::prelude::v1::test]
    fn an_empty_line_ends_the_one_before_it() {
        let model = Model::new(&lyrics(vec![
            line(1_000, None, "sung"),
            line(4_000, None, ""),
            line(12_000, None, "again"),
        ]));
        assert_eq!(model.lines[0].end, Some(4.0));
        assert!(model.lines[1].interlude);
        assert_eq!(model.lines.len(), 3);
    }

    #[::core::prelude::v1::test]
    fn glow_holds_through_the_chunk_then_decays() {
        assert_eq!(chunk_glow((1.0, 2.0), 0.5), 0.);
        assert_eq!(chunk_glow((1.0, 2.0), 1.1), 0.5);
        assert_eq!(chunk_glow((1.0, 2.0), 1.5), 1.);
        assert!((chunk_glow((1.0, 2.0), 2.3) - 0.5).abs() < 1e-6);
        assert_eq!(chunk_glow((1.0, 2.0), 3.0), 0.);
    }

    /// What the first glyph of a row paints at its centre, as brightness over
    /// a black pane: the row's opacity, the coverage of its blur copies where
    /// they all overlap, and its ink (unsung at the first chunk's start).
    fn first_glyph(state: &RowMotion, timed: bool, now: Instant) -> f32 {
        let (unsung, sung) = (hsla(0., 0., 0.6, 1.), hsla(0., 0., 1., 1.));
        let ink = state.ink.value(now);
        let color = if timed {
            unsung
        } else {
            mix(unsung, sung, ink)
        };
        let blur = state.blur.value(now);
        let copies = if blur < 1. { 1 } else { taps(0.).len() };
        let tap = copy(fade(color, state.opacity(now)), copies, spread_for(blur));
        let coverage = 1. - (1. - tap.a).powi(copies as i32);
        coverage * color.to_rgb().r
    }

    #[::core::prelude::v1::test]
    fn a_row_lighting_up_never_steps_its_ink() {
        let motion = |m: Motion| m;
        let unlit = RowTarget {
            lit: false,
            scale: SCALE_UNLIT,
            depth: depth_opacity(1., 6.),
            blur: blur_for(1., 6.),
        };
        let lit = RowTarget {
            lit: true,
            scale: SCALE_LIT,
            depth: 1.,
            blur: 0.,
        };
        assert!(unlit.blur >= 1., "the case under test starts blurred");
        for timed in [true, false] {
            let start = Instant::now();
            let mut state = RowMotion::settled(unlit, start);
            let mut last = first_glyph(&state, timed, start);
            for step in 1..=125 {
                let now = start + Duration::from_millis(step * 4);
                let target = if step > 25 { lit } else { unlit };
                state.follow(target, timed, &motion, now);
                let painted = first_glyph(&state, timed, now);
                assert!(
                    painted >= last - 1e-4 && painted - last < 0.03,
                    "timed {timed}: {last} to {painted} at {} ms",
                    step * 4
                );
                last = painted;
            }
            assert!((last - if timed { 0.6 } else { 1. }).abs() < 1e-3);
        }
    }

    #[::core::prelude::v1::test]
    fn glow_comes_up_without_a_step() {
        let mut last = 0.;
        for ms in 0..=400 {
            let glow = chunk_glow((1.0, 2.0), 1.0 + ms as f64 / 1000.);
            assert!(glow >= last && glow - last <= GLOW_QUANTUM + 1e-6);
            last = glow;
        }
        assert_eq!(last, 1.);
    }

    #[::core::prelude::v1::test]
    fn the_depth_ramp_reaches_its_floor_at_the_span() {
        assert_eq!(depth_opacity(0., 6.), 1.);
        assert_eq!(depth_opacity(2., 6.), 0.7);
        assert_eq!(depth_opacity(-9., 6.), 0.25);
        assert_eq!(blur_for(3., 6.), 3.);
        assert_eq!(blur_for(30., 6.), BLUR_MAX_PX);
    }

    #[::core::prelude::v1::test]
    fn a_wrapped_chunk_wipes_its_rows_in_reading_order() {
        let bands = [Band { x0: 0., x1: 300. }, Band { x0: 0., x1: 100. }];
        assert_eq!(band_fill(&bands, 0, 0.5), 2. / 3.);
        assert_eq!(band_fill(&bands, 1, 0.5), 0.);
        assert_eq!(band_fill(&bands, 1, 0.875), 0.5);
    }

    #[::core::prelude::v1::test]
    fn word_chunks_keep_their_own_timing() {
        let mut timed = line(1_000, Some(3_000), "hello");
        timed.words = vec![
            LyricWord {
                start_ms: 1_000,
                end_ms: 1_400,
                text: "hel".into(),
                joins_next: true,
            },
            LyricWord {
                start_ms: 1_500,
                end_ms: 2_000,
                text: "lo".into(),
                joins_next: false,
            },
        ];
        let model = Model::new(&lyrics(vec![timed]));
        assert!(!model.lines[0].estimated);
        assert_eq!(model.chunks[0], vec![(1.0, 1.5), (1.5, 3.0)]);
    }

    #[::core::prelude::v1::test]
    fn the_pane_is_still_between_words_and_lines() {
        let mut first = line(1_000, Some(6_000), "one two");
        first.words = vec![
            LyricWord {
                start_ms: 1_000,
                end_ms: 1_500,
                text: "one".into(),
                joins_next: false,
            },
            LyricWord {
                start_ms: 1_500,
                end_ms: 2_000,
                text: "two".into(),
                joins_next: false,
            },
        ];
        let model = Model::new(&lyrics(vec![first, line(20_000, None, "three")]));
        let still = |t: f64| {
            let mut lit = Vec::new();
            model.lit_at(t, &mut lit);
            model.still_for(t, &lit, false)
        };
        assert_eq!(still(0.5), Some(0.5));
        assert_eq!(still(1.2), Some(0.));
        // "two" has wiped and glows steadily until the line ends.
        assert_eq!(still(4.0), Some(2.0));
        // The interlude's note fills over 14 s, a half pixel at a time.
        let step = still(8.0).unwrap();
        assert!(
            (step - 14. / f64::from(NOTE_SIZE * 2.)).abs() < 1e-9,
            "{step}"
        );
        assert_eq!(still(25.0), None);
    }
}
