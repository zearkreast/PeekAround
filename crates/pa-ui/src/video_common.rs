//! What the two video players share (ffmpeg on the native targets, a `<video>` element in the
//! browser): what to play, a decoded frame, the status for the UI, the perf counters.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use slint::{Rgba8Pixel, SharedPixelBuffer};
use web_time::Instant;

static PRESENTED: AtomicU64 = AtomicU64::new(0);

static INTERVALS: Mutex<Vec<f32>> = Mutex::new(Vec::new());
static LAST_PRESENT: Mutex<Option<Instant>> = Mutex::new(None);

/// A frame was put on screen (perf counters: count + intervals between presented frames).
pub fn count_presented() {
    PRESENTED.fetch_add(1, Ordering::Relaxed);
    let now = Instant::now();
    let mut last = LAST_PRESENT.lock().unwrap();
    if let Some(l) = *last {
        let mut v = INTERVALS.lock().unwrap();
        if v.len() < 100_000 {
            v.push(now.duration_since(l).as_secs_f32() * 1000.0);
        }
    }
    *last = Some(now);
}

/// Presented-frame intervals since the last call (ms) — and reset.
pub fn take_intervals() -> Vec<f32> {
    *LAST_PRESENT.lock().unwrap() = None;
    std::mem::take(&mut *INTERVALS.lock().unwrap())
}

/// Frames presented since start.
pub fn presented() -> u64 {
    PRESENTED.load(Ordering::Relaxed)
}

/// What to play.
#[derive(Debug, Clone)]
pub struct Source {
    /// ffmpeg input (path or URL)
    pub input: String,
    /// has a picture (video) — false for audio files
    pub video: bool,
    /// output frame size (display size, even numbers)
    pub width: u32,
    /// output frame height
    pub height: u32,
    /// output frame rate (source fps capped at 60)
    pub fps: f64,
    /// duration (s) when known
    pub duration: Option<f64>,
    /// whether the file has an audio stream (None: unknown → try)
    pub audio: Option<bool>,
    /// ffmpeg filters before the scale to `width`×`height` (video editor: crop, rotate, flip)
    pub vf_pre: Vec<String>,
    /// filters after it (video editor: colour, at display size)
    pub vf_post: Vec<String>,
}

/// A decoded frame.
pub struct Frame {
    /// media time (s)
    pub pts: f64,
    /// pixels
    pub px: SharedPixelBuffer<Rgba8Pixel>,
}

/// Player status for the UI.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    /// media position (s)
    pub position: f64,
    /// playing
    pub playing: bool,
    /// waiting for data
    pub buffering: bool,
    /// reached the end
    pub ended: bool,
    /// an error (ffmpeg missing / unreadable file)
    pub error: Option<String>,
    /// audio plays through the null sink (no output device)
    pub no_device: bool,
    /// furthest decoded media time (buffered bar)
    pub buffered: f64,
}

/// Output size for a picture of `aspect` shown in a `w`×`h` box (physical px), even numbers,
/// never above the source size.
pub fn output_size(
    aspect: f64,
    w: f64,
    h: f64,
    src_w: Option<i64>,
    src_h: Option<i64>,
) -> (u32, u32) {
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        16.0 / 9.0
    };
    let (mut ow, mut oh) = if w / h > aspect {
        (h * aspect, h)
    } else {
        (w, w / aspect)
    };
    if let (Some(sw), Some(sh)) = (src_w, src_h) {
        let (sw, sh) = (sw as f64, sh as f64);
        if sw > 0.0 && sh > 0.0 && (ow > sw || oh > sh) {
            let k = (sw / ow).min(sh / oh);
            ow *= k;
            oh *= k;
        }
    }
    let even = |v: f64| ((v.round() as u32) / 2 * 2).max(2);
    (even(ow), even(oh))
}
