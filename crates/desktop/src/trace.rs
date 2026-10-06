//! `FORMALMUSIC_TRACE=1` timing on stderr: time to first paint, latency from a
//! keystroke or a chat switch to the frame that shows it, and once a second
//! how many frames were painted and which views rendered. Every entry point
//! is a no-op when the variable is unset.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui_kit::*;

static START: OnceLock<Instant> = OnceLock::new();
static ENABLED: LazyLock<bool> = LazyLock::new(|| {
    std::env::var("FORMALMUSIC_TRACE").is_ok_and(|value| !value.is_empty() && value != "0")
});
static FRAMES: AtomicU64 = AtomicU64::new(0);
static RENDERS: Mutex<BTreeMap<&'static str, u64>> = Mutex::new(BTreeMap::new());

thread_local! {
    static PENDING: RefCell<Vec<(&'static str, Instant)>> = const { RefCell::new(Vec::new()) };
    static PAINTED_ONCE: RefCell<bool> = const { RefCell::new(false) };
}

pub fn enabled() -> bool {
    *ENABLED
}

/// Call first thing in `main`, so "+ms" counts from process start.
pub fn init() {
    START.get_or_init(Instant::now);
    if !enabled() {
        return;
    }
    std::thread::Builder::new()
        .name("formalmusic-trace".into())
        .spawn(|| {
            let mut last_frames = 0;
            let mut last: BTreeMap<&'static str, u64> = BTreeMap::new();
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let frames = FRAMES.load(Ordering::Relaxed);
                let renders = RENDERS
                    .lock()
                    .map(|renders| renders.clone())
                    .unwrap_or_default();
                let delta: Vec<String> = renders
                    .iter()
                    .filter_map(|(view, count)| {
                        let diff = count - last.get(view).copied().unwrap_or(0);
                        (diff > 0).then(|| format!("{view}={diff}"))
                    })
                    .collect();
                if frames != last_frames || !delta.is_empty() {
                    log(&format!(
                        "1s: frames={} renders {}",
                        frames - last_frames,
                        delta.join(" ")
                    ));
                }
                last_frames = frames;
                last = renders;
            }
        })
        .ok();
}

fn elapsed_ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.
}

pub fn log(message: &str) {
    let start = START.get().copied().unwrap_or_else(Instant::now);
    eprintln!("[trace +{:.1}ms] {message}", elapsed_ms(start));
}

pub fn log_if_enabled(message: &str) {
    if enabled() {
        log(message);
    }
}

/// Counts one `render` of `view`.
pub fn render(view: &'static str) {
    if enabled() {
        if let Ok(mut renders) = RENDERS.lock() {
            *renders.entry(view).or_default() += 1;
        }
    }
}

/// Marks the start of something whose latency ends at the next painted frame.
pub fn stamp(label: &'static str) {
    if enabled() {
        PENDING.with(|pending| pending.borrow_mut().push((label, Instant::now())));
    }
}

/// Logs a keystroke-to-paint line for every key the window handles.
pub fn watch_keys(cx: &mut App) {
    if enabled() {
        cx.observe_keystrokes(|_, _, _| stamp("key")).detach();
    }
}

fn painted(window: &Window) {
    FRAMES.fetch_add(1, Ordering::Relaxed);
    let first = PAINTED_ONCE.with(|once| !std::mem::replace(&mut *once.borrow_mut(), true));
    if first {
        let gpu = window
            .gpu_specs()
            .map(|specs| {
                format!(
                    "{} ({}, software: {})",
                    specs.device_name, specs.driver_name, specs.is_software_emulated
                )
            })
            .unwrap_or_else(|| "unknown".into());
        log(&format!(
            "first paint, gpu {gpu}, window active {}",
            window.is_window_active()
        ));
    }
    let pending = PENDING.with(|pending| std::mem::take(&mut *pending.borrow_mut()));
    for (label, at) in pending {
        log(&format!("{label} -> paint {:.2}ms", elapsed_ms(at)));
    }
}

/// A zero-size element that reports each frame it is painted in. Belongs in
/// the root view, which paints on every frame.
pub fn probe() -> Option<AnyElement> {
    enabled().then(|| {
        canvas(|_, _, _| {}, |_, _, window, _| painted(window))
            .absolute()
            .size_0()
            .into_any_element()
    })
}
