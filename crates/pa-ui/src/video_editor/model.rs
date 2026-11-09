//! Pure model of the video editor (features/video-editor/model.ts). Geometry rules mirror the
//! server planner (pa-jobs video_edit.rs `plan_video_edit`) so overlays rendered here line up
//! with the exported video.

use pa_core::{CropRect, Rotation, TimeRange, VideoAdjust, VideoEditSpec, VideoFormat, VideoOutput, VideoOverlay, VideoQuality};

use crate::editor::doc::TextLayer;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub w: f64,
    pub h: f64,
}

/// Crop ratio presets (`free` has none).
pub const ASPECTS: [&str; 5] = ["free", "16:9", "9:16", "1:1", "4:5"];

pub fn aspect_ratio(key: &str) -> Option<f64> {
    match key {
        "16:9" => Some(16.0 / 9.0),
        "9:16" => Some(9.0 / 16.0),
        "1:1" => Some(1.0),
        "4:5" => Some(4.0 / 5.0),
        _ => None,
    }
}

/// export resolution choices: short side in px (None = keep)
pub const SHORT_SIDES: [Option<u32>; 4] = [None, Some(1080), Some(720), Some(480)];
/// shortest trim range (s)
pub const MIN_TRIM: f64 = 0.2;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Adjust {
    pub brightness: f64,
    pub contrast: f64,
    pub saturation: f64,
    pub hue: f64,
    pub warmth: f64,
    pub vignette: f64,
}

/// Adjust sliders: key, i18n label, range (hue in degrees, the rest −1…1 shown as ±100).
pub const ADJUST_KEYS: [(&str, &str, f64, f64); 6] = [
    ("brightness", "video.brightness", -1.0, 1.0),
    ("contrast", "video.contrast", -1.0, 1.0),
    ("saturation", "video.saturation", -1.0, 1.0),
    ("hue", "video.hue", -180.0, 180.0),
    ("warmth", "video.warmth", -1.0, 1.0),
    ("vignette", "video.vignette", 0.0, 1.0),
];

impl Adjust {
    pub fn get(&self, key: &str) -> f64 {
        match key {
            "brightness" => self.brightness,
            "contrast" => self.contrast,
            "saturation" => self.saturation,
            "hue" => self.hue,
            "warmth" => self.warmth,
            "vignette" => self.vignette,
            _ => 0.0,
        }
    }

    pub fn set(&mut self, key: &str, v: f64) {
        match key {
            "brightness" => self.brightness = v,
            "contrast" => self.contrast = v,
            "saturation" => self.saturation = v,
            "hue" => self.hue = v,
            "warmth" => self.warmth = v,
            "vignette" => self.vignette = v,
            _ => {}
        }
    }

    pub fn is_adjusted(&self) -> bool {
        ADJUST_KEYS.iter().any(|k| self.get(k.0).abs() > 1e-4)
    }
}

/// Looks (panels.tsx LOOKS): i18n label and the adjust it sets (the rest 0).
pub fn looks() -> [(&'static str, Adjust); 5] {
    let z = Adjust::default();
    [
        ("video.look.vivid", Adjust { contrast: 0.15, saturation: 0.35, brightness: 0.03, ..z }),
        ("video.look.warm", Adjust { warmth: 0.55, saturation: 0.1, vignette: 0.25, ..z }),
        ("video.look.cool", Adjust { warmth: -0.5, contrast: 0.08, ..z }),
        ("video.look.cinema", Adjust { contrast: 0.22, saturation: -0.18, warmth: 0.15, vignette: 0.55, ..z }),
        ("video.look.mono", Adjust { saturation: -1.0, contrast: 0.2, ..z }),
    ]
}

/// A text layer; geometry in design px (crop + rotate, before output scaling), start / end
/// on the output timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoText {
    pub layer: TextLayer,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Output {
    pub format: VideoFormat,
    /// target short side (None = original); converted to the server's output height on export
    pub short: Option<u32>,
    pub quality: VideoQuality,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trim {
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EditState {
    /// source seconds
    pub trim: Trim,
    /// source pixels in displayed orientation (before the user rotation)
    pub crop: Option<Rect>,
    pub aspect: &'static str,
    /// 0, 90, 180, 270
    pub rotate: u16,
    pub flip_h: bool,
    pub flip_v: bool,
    pub speed: f64,
    pub volume: f64,
    pub mute: bool,
    pub adjust: Adjust,
    pub texts: Vec<VideoText>,
    pub output: Output,
}

pub fn initial_state(duration: f64) -> EditState {
    EditState {
        trim: Trim { start: 0.0, end: duration.max(0.1) },
        crop: None,
        aspect: "free",
        rotate: 0,
        flip_h: false,
        flip_v: false,
        speed: 1.0,
        volume: 1.0,
        mute: false,
        adjust: Adjust::default(),
        texts: Vec::new(),
        output: Output { format: VideoFormat::Mp4, short: None, quality: VideoQuality::High },
    }
}

pub fn clamp(v: f64, lo: f64, hi: f64) -> f64 {
    hi.min(lo.max(v))
}

fn even(n: f64) -> f64 {
    ((n / 2.0).round() * 2.0).max(2.0)
}

fn even_floor(n: f64) -> f64 {
    ((n / 2.0).floor() * 2.0).max(2.0)
}

// ───────────── geometry ─────────────

/// Integer crop clamped to the frame (None when it covers the whole frame) — the server's rules.
pub fn normalize_crop(crop: Option<Rect>, src_w: f64, src_h: f64) -> Option<Rect> {
    let c = crop?;
    let x = clamp(c.x.round(), 0.0, src_w - 2.0);
    let y = clamp(c.y.round(), 0.0, src_h - 2.0);
    let w = clamp(c.w.round(), 2.0, src_w - x);
    let h = clamp(c.h.round(), 2.0, src_h - y);
    if x == 0.0 && y == 0.0 && w == src_w && h == src_h {
        return None;
    }
    Some(Rect { x, y, w, h })
}

pub fn rotated(r: u16) -> bool {
    r == 90 || r == 270
}

/// The frame after the user rotation (no crop).
pub fn oriented(src: Size, r: u16) -> Size {
    if rotated(r) {
        Size { w: src.h, h: src.w }
    } else {
        src
    }
}

/// Size after crop + rotate (the text design space).
pub fn base_size(src: Size, crop: Option<Rect>, rotate: u16) -> Size {
    let c = normalize_crop(crop, src.w, src.h);
    let (w, h) = c.map_or((src.w, src.h), |c| (c.w, c.h));
    oriented(Size { w, h }, rotate)
}

/// Final output size — mirrors plan_video_edit on the server.
pub fn output_size(src: Size, crop: Option<Rect>, rotate: u16, height: Option<f64>) -> Size {
    let b = base_size(src, crop, rotate);
    match height {
        Some(h) if h > 0.0 => {
            let h = even(h);
            Size { w: even(b.w * h / b.h), h }
        }
        _ => Size { w: even_floor(b.w), h: even_floor(b.h) },
    }
}

/// Output height for a short-side choice (720p of a portrait clip = 720 px wide).
pub fn target_height(base: Size, short: Option<u32>) -> Option<f64> {
    let s = short? as f64;
    Some(if base.h <= base.w { s } else { (s * base.h / base.w).round() })
}

/// What the exported file will measure.
pub fn export_size(src: Size, s: &EditState) -> Size {
    output_size(src, s.crop, s.rotate, target_height(base_size(src, s.crop, s.rotate), s.output.short))
}

/// Source point → oriented (rotated + flipped) point; `src` is the unrotated frame size.
pub fn orient_point(x: f64, y: f64, src: Size, rot: u16, flip_h: bool, flip_v: bool) -> (f64, f64) {
    let (mut u, mut v) = match rot {
        90 => (src.h - y, x),
        180 => (src.w - x, src.h - y),
        270 => (y, src.w - x),
        _ => (x, y),
    };
    let o = oriented(src, rot);
    if flip_h {
        u = o.w - u;
    }
    if flip_v {
        v = o.h - v;
    }
    (u, v)
}

/// Oriented point → source point (inverse of [`orient_point`]).
pub fn unorient_point(mut u: f64, mut v: f64, src: Size, rot: u16, flip_h: bool, flip_v: bool) -> (f64, f64) {
    let o = oriented(src, rot);
    if flip_h {
        u = o.w - u;
    }
    if flip_v {
        v = o.h - v;
    }
    match rot {
        90 => (v, src.h - u),
        180 => (src.w - u, src.h - v),
        270 => (src.w - v, u),
        _ => (u, v),
    }
}

fn rect_from_points(a: (f64, f64), b: (f64, f64)) -> Rect {
    Rect { x: a.0.min(b.0), y: a.1.min(b.1), w: (a.0 - b.0).abs(), h: (a.1 - b.1).abs() }
}

pub fn orient_rect(r: Rect, src: Size, rot: u16, fh: bool, fv: bool) -> Rect {
    rect_from_points(orient_point(r.x, r.y, src, rot, fh, fv), orient_point(r.x + r.w, r.y + r.h, src, rot, fh, fv))
}

pub fn unorient_rect(r: Rect, src: Size, rot: u16, fh: bool, fv: bool) -> Rect {
    rect_from_points(unorient_point(r.x, r.y, src, rot, fh, fv), unorient_point(r.x + r.w, r.y + r.h, src, rot, fh, fv))
}

/// Largest centred rect of `ratio` (w/h) inside `b`.
pub fn fit_aspect(b: Size, ratio: f64) -> Rect {
    let mut w = b.w;
    let mut h = w / ratio;
    if h > b.h {
        h = b.h;
        w = h * ratio;
    }
    Rect { x: (b.w - w) / 2.0, y: (b.h - h) / 2.0, w, h }
}

/// Fit a box of `aspect` into `avail` (contain).
pub fn contain_size(avail: Size, aspect: f64) -> Size {
    if avail.w <= 0.0 || avail.h <= 0.0 {
        return Size { w: 0.0, h: 0.0 };
    }
    if avail.w / avail.h > aspect {
        Size { w: avail.h * aspect, h: avail.h }
    } else {
        Size { w: avail.w, h: avail.w / aspect }
    }
}

/// Keep text layers in place (relative to the frame) when the design space changes size.
pub fn remap_texts(texts: &[VideoText], from: Size, to: Size) -> Vec<VideoText> {
    if from == to {
        return texts.to_vec();
    }
    let rx = to.w / from.w;
    let ry = to.h / from.h;
    // geometric mean: reversible (crop on → off restores sizes) and neutral for 90° rotations
    let k = (rx * ry).sqrt();
    texts
        .iter()
        .map(|t| {
            let l = &t.layer;
            let mut n = l.clone();
            n.x = l.x * rx;
            n.y = l.y * ry;
            n.size = (l.size * k).max(4.0);
            n.letter_spacing = l.letter_spacing * k;
            if let Some(s) = n.stroke.as_mut() {
                s.width *= k;
            }
            if let Some(s) = n.shadow.as_mut() {
                s.blur *= k;
                s.x *= k;
                s.y *= k;
            }
            if let Some(b) = n.background.as_mut() {
                b.padding *= k;
                b.radius *= k;
            }
            VideoText { layer: n, ..t.clone() }
        })
        .collect()
}

/// `store.set`: text layers stay attached to the frame when crop / rotation change the design
/// space (unless the change set the texts itself).
pub fn keep_texts(prev: &EditState, mut next: EditState, src: Size) -> EditState {
    let geo = prev.crop != next.crop || prev.rotate != next.rotate;
    if geo && prev.texts == next.texts && !next.texts.is_empty() {
        let from = base_size(src, prev.crop, prev.rotate);
        let to = base_size(src, next.crop, next.rotate);
        next.texts = remap_texts(&next.texts, from, to);
    }
    next
}

/// Rotate by ±90°: 16:9 ↔ 9:16 follow the turn, 4:5 drops to free.
pub fn rotate_state(s: &EditState, dir: i32) -> EditState {
    let r = ((s.rotate as i32 + if dir < 0 { -90 } else { 90 }).rem_euclid(360)) as u16;
    let aspect = match s.aspect {
        "free" | "1:1" => s.aspect,
        "16:9" => "9:16",
        "9:16" => "16:9",
        _ => "free",
    };
    EditState { rotate: r, aspect, ..s.clone() }
}

/// Pick a crop ratio: the largest centred crop of that ratio over the oriented frame.
pub fn pick_aspect(s: &EditState, src: Size, key: &str) -> EditState {
    let Some(&k) = ASPECTS.iter().find(|a| **a == key) else { return s.clone() };
    match aspect_ratio(k) {
        None => EditState { aspect: "free", ..s.clone() },
        Some(ratio) => {
            let r = fit_aspect(oriented(src, s.rotate), ratio);
            EditState { aspect: k, crop: Some(unorient_rect(r, src, s.rotate, s.flip_h, s.flip_v)), ..s.clone() }
        }
    }
}

// ───────────── time ─────────────

pub fn out_duration(s: &EditState) -> f64 {
    ((s.trim.end - s.trim.start) / s.speed).max(0.0)
}

pub fn src_to_out(s: &EditState, t: f64) -> f64 {
    (t - s.trim.start) / s.speed
}

pub fn out_to_src(s: &EditState, t: f64) -> f64 {
    s.trim.start + t * s.speed
}

/// Visible window of a text layer on the output timeline.
pub fn text_window(t: &VideoText, dur: f64) -> (f64, f64) {
    let start = clamp(t.start, 0.0, dur);
    let end = clamp(t.end, start, dur);
    (start, end)
}

/// Move a trim edge to `t` (source s), keeping [`MIN_TRIM`]; None when it would not fit.
pub fn set_in(s: &EditState, t: f64) -> Option<Trim> {
    (t < s.trim.end - MIN_TRIM).then_some(Trim { start: t.max(0.0), end: s.trim.end })
}

pub fn set_out(s: &EditState, t: f64, duration: f64) -> Option<Trim> {
    (t > s.trim.start + MIN_TRIM).then_some(Trim { start: s.trim.start, end: t.min(duration) })
}

/// Text bar drag on the timeline: `part` move / start / end by `delta` output seconds.
pub fn drag_window(start: f64, end: f64, part: &str, delta: f64, total: f64) -> (f64, f64) {
    match part {
        "move" => {
            let len = end - start;
            let s = clamp(start + delta, 0.0, (total - len).max(0.0));
            (s, s + len)
        }
        "start" => (clamp(start + delta, 0.0, end - 0.1), end),
        _ => (start, clamp(end + delta, start + 0.1, total)),
    }
}

/// `m:ss.s` (Timeline fmtTime).
pub fn fmt_time(s: f64) -> String {
    let s = if s.is_finite() && s >= 0.0 { s } else { 0.0 };
    // round to tenths first so 59.96 reads 1:00.0, not 0:60.0
    let tenths = (s * 10.0).round() as i64;
    let m = tenths / 600;
    let rest = tenths - m * 600;
    format!("{m}:{:02}.{}", rest / 10, rest % 10)
}

/// Ruler ticks: the step (s) whose labels are at least 64 px apart, and the tick times.
pub fn ruler(duration: f64, width: f64) -> (f64, Vec<f64>) {
    if width <= 0.0 || duration <= 0.0 {
        return (0.0, vec![]);
    }
    let pps = width / duration;
    let step = [0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0]
        .into_iter()
        .find(|s| s * pps >= 64.0)
        .unwrap_or(600.0);
    let mut ticks = vec![];
    let mut k = 0.0;
    while k * step <= duration + 1e-6 {
        ticks.push(k * step);
        k += 1.0;
    }
    (step, ticks)
}

/// Frames of the timeline filmstrip for a strip `w` × `h` px (even, 4…60).
pub fn filmstrip_frames(w: f64, h: f64, src: Size) -> u32 {
    if w <= 0.0 {
        return 0;
    }
    let n = ((w / (h * (src.w / src.h)) / 2.0).round() * 2.0).clamp(4.0, 60.0);
    n as u32
}

// ───────────── preview (ffmpeg filters of the decoder) ─────────────

fn fnum(n: f64, digits: usize) -> String {
    let s = format!("{n:.digits$}");
    let s = if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s };
    if s == "-0" {
        "0".into()
    } else {
        s
    }
}

/// The export's colour chain (pa-jobs `color_filters`): eq, hue, colortemperature, vignette.
pub fn color_filters(a: &Adjust, vignette: bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut eq = Vec::new();
    if a.brightness.abs() > 1e-4 {
        eq.push(format!("brightness={}", fnum(a.brightness * 0.3, 4)));
    }
    if a.contrast.abs() > 1e-4 {
        eq.push(format!("contrast={}", fnum(1.0 + a.contrast, 4)));
    }
    if a.saturation.abs() > 1e-4 {
        eq.push(format!("saturation={}", fnum(1.0 + a.saturation, 4)));
    }
    if !eq.is_empty() {
        out.push(format!("eq={}", eq.join(":")));
    }
    if a.hue.abs() > 0.05 {
        out.push(format!("hue=h={}", fnum(a.hue, 2)));
    }
    if a.warmth.abs() > 1e-3 {
        out.push(format!("colortemperature=temperature={}:pl=1", (6500.0 - 2500.0 * a.warmth).round()));
    }
    if vignette && a.vignette > 1e-3 {
        out.push(format!("vignette=angle={}", fnum(a.vignette * std::f64::consts::PI / 2.5, 4)));
    }
    out
}

/// Decoder filters for the stage preview: (before the scale to display size, after it). The
/// crop tool shows the whole oriented frame without the vignette (Stage.tsx).
pub fn preview_filters(s: &EditState, src: Size, crop_mode: bool) -> (Vec<String>, Vec<String>) {
    let mut pre = Vec::new();
    if !crop_mode {
        if let Some(c) = normalize_crop(s.crop, src.w, src.h) {
            pre.push(format!("crop={}:{}:{}:{}:exact=1", c.w, c.h, c.x, c.y));
        }
    }
    match s.rotate {
        90 => pre.push("transpose=clock".into()),
        270 => pre.push("transpose=cclock".into()),
        180 => {
            pre.push("hflip".into());
            pre.push("vflip".into());
        }
        _ => {}
    }
    if s.flip_h {
        pre.push("hflip".into());
    }
    if s.flip_v {
        pre.push("vflip".into());
    }
    (pre, color_filters(&s.adjust, !crop_mode))
}

// ───────────── crop box ─────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    Move,
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

impl Handle {
    pub fn parse(s: &str) -> Option<Handle> {
        Some(match s {
            "move" => Handle::Move,
            "n" => Handle::N,
            "s" => Handle::S,
            "e" => Handle::E,
            "w" => Handle::W,
            "ne" => Handle::NE,
            "nw" => Handle::NW,
            "se" => Handle::SE,
            "sw" => Handle::SW,
            _ => return None,
        })
    }

    fn dirs(self) -> (i32, i32) {
        match self {
            Handle::Move => (0, 0),
            Handle::N => (0, -1),
            Handle::S => (0, 1),
            Handle::E => (1, 0),
            Handle::W => (-1, 0),
            Handle::NE => (1, -1),
            Handle::NW => (-1, -1),
            Handle::SE => (1, 1),
            Handle::SW => (-1, 1),
        }
    }
}

/// Resize / move a crop rect (oriented source px) by a pointer delta, inside `b`, at least
/// `min` px, and at `ratio` when set.
pub fn resize_rect(r: Rect, handle: Handle, dx: f64, dy: f64, b: Size, ratio: Option<f64>, min: f64) -> Rect {
    if handle == Handle::Move {
        return Rect { x: clamp(r.x + dx, 0.0, b.w - r.w), y: clamp(r.y + dy, 0.0, b.h - r.h), ..r };
    }
    let (dir_x, dir_y) = handle.dirs();
    let Some(ratio) = ratio else {
        let (mut x0, mut y0, mut x1, mut y1) = (r.x, r.y, r.x + r.w, r.y + r.h);
        if dir_x < 0 {
            x0 = clamp(x0 + dx, 0.0, x1 - min);
        }
        if dir_x > 0 {
            x1 = clamp(x1 + dx, x0 + min, b.w);
        }
        if dir_y < 0 {
            y0 = clamp(y0 + dy, 0.0, y1 - min);
        }
        if dir_y > 0 {
            y1 = clamp(y1 + dy, y0 + min, b.h);
        }
        return Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
    };
    let cx = r.x + r.w / 2.0;
    let cy = r.y + r.h / 2.0;
    let ax = match dir_x {
        1 => r.x,
        -1 => r.x + r.w,
        _ => cx,
    };
    let ay = match dir_y {
        1 => r.y,
        -1 => r.y + r.h,
        _ => cy,
    };
    let mut w = match dir_x {
        1 => (r.x + r.w + dx - ax).abs(),
        -1 => (r.x + dx - ax).abs(),
        _ => 0.0,
    };
    let h0 = match dir_y {
        1 => (r.y + r.h + dy - ay).abs(),
        -1 => (r.y + dy - ay).abs(),
        _ => 0.0,
    };
    if dir_x != 0 && dir_y != 0 {
        w = w.max(h0 * ratio);
    } else if dir_x == 0 {
        w = h0 * ratio;
    }
    let max_w = match dir_x {
        1 => b.w - ax,
        -1 => ax,
        _ => 2.0 * cx.min(b.w - cx),
    };
    let max_h = match dir_y {
        1 => b.h - ay,
        -1 => ay,
        _ => 2.0 * cy.min(b.h - cy),
    };
    let hi = max_w.min(max_h * ratio);
    let w = hi.min(w.max(min).max(min * ratio));
    let h = w / ratio;
    let x = match dir_x {
        1 => ax,
        -1 => ax - w,
        _ => cx - w / 2.0,
    };
    let y = match dir_y {
        1 => ay,
        -1 => ay - h,
        _ => cy - h / 2.0,
    };
    Rect { x, y, w, h }
}

// ───────────── export ─────────────

/// Text layers with the same visible window → one overlay PNG each: (start, end, layers).
pub fn overlay_groups(texts: &[VideoText], dur: f64) -> Vec<(f64, f64, Vec<TextLayer>)> {
    let mut groups: Vec<(String, f64, f64, Vec<TextLayer>)> = Vec::new();
    for t in texts {
        if t.layer.text.trim().is_empty() || t.layer.opacity <= 0.0 {
            continue;
        }
        let (s, e) = text_window(t, dur);
        if e - s < 0.02 {
            continue;
        }
        let key = format!("{s:.3}-{e:.3}");
        match groups.iter_mut().find(|g| g.0 == key) {
            Some(g) => g.3.push(t.layer.clone()),
            None => groups.push((key, s, e, vec![t.layer.clone()])),
        }
    }
    groups.into_iter().map(|g| (g.1, g.2, g.3)).collect()
}

fn round3(n: f64) -> f64 {
    (n * 1000.0).round() / 1000.0
}

pub fn build_spec(source_id: &str, s: &EditState, src: Size, duration: f64, overlays: Vec<VideoOverlay>) -> VideoEditSpec {
    let crop = normalize_crop(s.crop, src.w, src.h);
    let trimmed = s.trim.start > 0.01 || s.trim.end < duration - 0.01;
    let a = s.adjust;
    VideoEditSpec {
        source_id: source_id.to_string(),
        trim: trimmed.then(|| TimeRange { start: round3(s.trim.start), end: round3(s.trim.end) }),
        crop: crop.map(|c| CropRect { x: c.x, y: c.y, w: c.w, h: c.h }),
        rotate: Some(match s.rotate {
            90 => Rotation::R90,
            180 => Rotation::R180,
            270 => Rotation::R270,
            _ => Rotation::R0,
        }),
        flip_h: Some(s.flip_h),
        flip_v: Some(s.flip_v),
        speed: Some(s.speed),
        volume: Some(s.volume),
        mute: Some(s.mute),
        adjust: Some(VideoAdjust {
            brightness: Some(a.brightness),
            contrast: Some(a.contrast),
            saturation: Some(a.saturation),
            hue: Some(a.hue),
            warmth: Some(a.warmth),
            vignette: Some(a.vignette),
        }),
        overlays: Some(overlays),
        output: VideoOutput {
            format: s.output.format,
            height: target_height(base_size(src, s.crop, s.rotate), s.output.short).map(|h| h as i64),
            quality: s.output.quality,
        },
    }
}

pub fn base_name(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 => &name[..i],
        _ => name,
    }
}

pub fn ext_of(f: VideoFormat) -> &'static str {
    match f {
        VideoFormat::Mp4 => "mp4",
        VideoFormat::Webm => "webm",
        VideoFormat::Gif => "gif",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::doc::{Align, TextOutline, TextShadow};

    const HD: Size = Size { w: 1920.0, h: 1080.0 };

    fn text(x: f64, y: f64, start: f64, end: f64) -> VideoText {
        VideoText {
            layer: TextLayer {
                id: 1,
                text: "Hi".into(),
                family: "Arial".into(),
                weight: 700.0,
                italic: false,
                size: 100.0,
                color: [255, 255, 255, 255],
                align: Align::Center,
                line_height: 1.15,
                letter_spacing: 2.0,
                x,
                y,
                rotation: 0.0,
                opacity: 1.0,
                stroke: Some(TextOutline { color: [0, 0, 0, 255], width: 4.0 }),
                shadow: Some(TextShadow { color: [0, 0, 0, 115], blur: 18.0, x: 0.0, y: 4.0 }),
                background: None,
            },
            start,
            end,
        }
    }

    #[test]
    fn crop_is_normalized_like_the_server() {
        assert_eq!(normalize_crop(None, 1920.0, 1080.0), None);
        assert_eq!(normalize_crop(Some(Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 }), 1920.0, 1080.0), None);
        let c = normalize_crop(Some(Rect { x: -5.4, y: 10.6, w: 3000.0, h: 0.5 }), 1920.0, 1080.0).unwrap();
        assert_eq!(c, Rect { x: 0.0, y: 11.0, w: 1920.0, h: 2.0 });
    }

    #[test]
    fn sizes_follow_crop_rotate_and_height() {
        let crop = Some(Rect { x: 100.0, y: 50.0, w: 801.0, h: 601.0 });
        assert_eq!(base_size(HD, crop, 90), Size { w: 601.0, h: 801.0 });
        assert_eq!(output_size(HD, crop, 0, None), Size { w: 800.0, h: 600.0 });
        assert_eq!(output_size(HD, None, 0, Some(720.0)), Size { w: 1280.0, h: 720.0 });
        // 720p of a portrait result is 720 px wide
        let b = base_size(HD, None, 90);
        assert_eq!(target_height(b, Some(720)), Some(1280.0));
        assert_eq!(target_height(HD, None), None);
        let mut s = initial_state(10.0);
        s.rotate = 270;
        s.output.short = Some(480);
        assert_eq!(export_size(HD, &s), Size { w: 480.0, h: 854.0 });
    }

    #[test]
    fn orient_and_unorient_are_inverse() {
        for rot in [0, 90, 180, 270] {
            for (fh, fv) in [(false, false), (true, false), (false, true), (true, true)] {
                let (u, v) = orient_point(300.0, 200.0, HD, rot, fh, fv);
                let (x, y) = unorient_point(u, v, HD, rot, fh, fv);
                assert!((x - 300.0).abs() < 1e-9 && (y - 200.0).abs() < 1e-9, "{rot} {fh} {fv}");
                let r = Rect { x: 10.0, y: 20.0, w: 300.0, h: 100.0 };
                let back = unorient_rect(orient_rect(r, HD, rot, fh, fv), HD, rot, fh, fv);
                assert!((back.x - r.x).abs() < 1e-9 && (back.w - r.w).abs() < 1e-9 && (back.h - r.h).abs() < 1e-9);
            }
        }
        // 90° clockwise: the top-left corner goes to the top-right
        assert_eq!(orient_point(0.0, 0.0, HD, 90, false, false), (1080.0, 0.0));
    }

    #[test]
    fn aspect_presets_and_rotation() {
        let s = initial_state(10.0);
        let sq = pick_aspect(&s, HD, "1:1");
        assert_eq!(sq.aspect, "1:1");
        assert_eq!(sq.crop, Some(Rect { x: 420.0, y: 0.0, w: 1080.0, h: 1080.0 }));
        let free = pick_aspect(&sq, HD, "free");
        assert_eq!((free.aspect, free.crop), ("free", sq.crop));
        let wide = pick_aspect(&s, HD, "16:9");
        let r = rotate_state(&wide, 1);
        assert_eq!((r.rotate, r.aspect), (90, "9:16"));
        assert_eq!(rotate_state(&pick_aspect(&s, HD, "4:5"), -1).aspect, "free");
        assert_eq!(rotate_state(&s, -1).rotate, 270);
    }

    #[test]
    fn texts_stay_on_the_frame_when_cropping() {
        let mut s = initial_state(10.0);
        s.texts = vec![text(960.0, 540.0, 0.0, 10.0)];
        let n = keep_texts(&s, EditState { crop: Some(Rect { x: 0.0, y: 0.0, w: 960.0, h: 540.0 }), ..s.clone() }, HD);
        let l = &n.texts[0].layer;
        assert_eq!((l.x, l.y), (480.0, 270.0));
        assert!((l.size - 50.0).abs() < 1e-9);
        assert!((l.stroke.unwrap().width - 2.0).abs() < 1e-9);
        // crop off again restores the layer
        let back = keep_texts(&n, EditState { crop: None, ..n.clone() }, HD);
        assert!((back.texts[0].layer.size - 100.0).abs() < 1e-9);
        // a change that sets texts itself is left alone
        let own = EditState { rotate: 90, texts: vec![text(1.0, 2.0, 0.0, 1.0)], ..s.clone() };
        assert_eq!(keep_texts(&s, own.clone(), HD).texts, own.texts);
    }

    #[test]
    fn time_mapping_and_windows() {
        let mut s = initial_state(20.0);
        s.trim = Trim { start: 4.0, end: 14.0 };
        s.speed = 2.0;
        assert_eq!(out_duration(&s), 5.0);
        assert_eq!(src_to_out(&s, 8.0), 2.0);
        assert_eq!(out_to_src(&s, 2.0), 8.0);
        assert_eq!(text_window(&text(0.0, 0.0, 1.0, 9.0), 5.0), (1.0, 5.0));
        assert_eq!(text_window(&text(0.0, 0.0, 7.0, 9.0), 5.0), (5.0, 5.0));
        assert_eq!(set_in(&s, 13.9), None);
        assert_eq!(set_in(&s, 2.0), Some(Trim { start: 2.0, end: 14.0 }));
        assert_eq!(set_out(&s, 30.0, 20.0), Some(Trim { start: 4.0, end: 20.0 }));
        assert_eq!(set_out(&s, 4.1, 20.0), None);
        assert_eq!(drag_window(1.0, 2.0, "move", 10.0, 5.0), (4.0, 5.0));
        assert_eq!(drag_window(1.0, 2.0, "start", 5.0, 5.0), (1.9, 2.0));
        assert_eq!(drag_window(1.0, 2.0, "end", -5.0, 5.0), (1.0, 1.1));
    }

    #[test]
    fn clock_text_and_ruler() {
        assert_eq!(fmt_time(0.0), "0:00.0");
        assert_eq!(fmt_time(65.44), "1:05.4");
        assert_eq!(fmt_time(59.96), "1:00.0");
        assert_eq!(fmt_time(f64::NAN), "0:00.0");
        let (step, ticks) = ruler(30.0, 640.0);
        assert_eq!(step, 5.0);
        assert_eq!(ticks, vec![0.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0]);
        assert_eq!(filmstrip_frames(960.0, 64.0, HD), 8);
        assert_eq!(filmstrip_frames(100.0, 64.0, HD), 4);
    }

    #[test]
    fn crop_box_resizes_inside_bounds() {
        let b = Size { w: 1000.0, h: 500.0 };
        let r = Rect { x: 100.0, y: 100.0, w: 400.0, h: 200.0 };
        assert_eq!(resize_rect(r, Handle::Move, 900.0, -50.0, b, None, 16.0), Rect { x: 600.0, y: 50.0, w: 400.0, h: 200.0 });
        assert_eq!(resize_rect(r, Handle::SE, 50.0, 1000.0, b, None, 16.0), Rect { x: 100.0, y: 100.0, w: 450.0, h: 400.0 });
        assert_eq!(resize_rect(r, Handle::W, 1000.0, 0.0, b, None, 16.0).w, 16.0);
        // ratio kept, clamped to the bounds
        let q = resize_rect(r, Handle::SE, 2000.0, 0.0, b, Some(2.0), 16.0);
        assert_eq!(q, Rect { x: 100.0, y: 100.0, w: 800.0, h: 400.0 });
        let n = resize_rect(r, Handle::N, 0.0, -50.0, b, Some(2.0), 16.0);
        assert!((n.w / n.h - 2.0).abs() < 1e-9 && n.y + n.h == 300.0);
    }

    #[test]
    fn preview_filters_match_the_export_chain() {
        let mut s = initial_state(10.0);
        s.crop = Some(Rect { x: 10.0, y: 20.0, w: 640.0, h: 360.0 });
        s.rotate = 90;
        s.flip_h = true;
        s.adjust = Adjust { brightness: 0.1, contrast: -0.2, hue: 30.0, warmth: 0.5, vignette: 0.5, ..Default::default() };
        let (pre, post) = preview_filters(&s, HD, false);
        assert_eq!(pre, vec!["crop=640:360:10:20:exact=1", "transpose=clock", "hflip"]);
        assert_eq!(
            post,
            vec!["eq=brightness=0.03:contrast=0.8", "hue=h=30", "colortemperature=temperature=5250:pl=1", "vignette=angle=0.6283"]
        );
        // the crop tool: whole frame, no vignette
        let (pre, post) = preview_filters(&s, HD, true);
        assert_eq!(pre, vec!["transpose=clock", "hflip"]);
        assert_eq!(post.len(), 3);
    }

    #[test]
    fn overlays_group_by_window() {
        let a = text(0.0, 0.0, 0.0, 5.0);
        let mut b = text(0.0, 0.0, 0.0, 5.0);
        b.layer.id = 2;
        let c = text(0.0, 0.0, 1.0, 2.0);
        let mut empty = text(0.0, 0.0, 0.0, 5.0);
        empty.layer.text = "  ".into();
        let short = text(0.0, 0.0, 4.99, 5.0);
        let g = overlay_groups(&[a, b, c, empty, short], 5.0);
        assert_eq!(g.len(), 2);
        assert_eq!((g[0].0, g[0].1, g[0].2.len()), (0.0, 5.0, 2));
        assert_eq!((g[1].0, g[1].1), (1.0, 2.0));
    }

    #[test]
    fn spec_has_the_1x_shape() {
        let mut s = initial_state(12.0);
        let v = serde_json::to_value(build_spec("7", &s, HD, 12.0, vec![])).unwrap();
        assert_eq!(v["sourceId"], "7");
        assert!(v.get("trim").is_none() && v.get("crop").is_none());
        assert_eq!(v["rotate"], 0);
        assert_eq!(v["output"], serde_json::json!({"format": "mp4", "height": null, "quality": "high"}));
        assert_eq!(v["adjust"]["vignette"], 0);
        s.trim = Trim { start: 1.23456, end: 9.0 };
        s.crop = Some(Rect { x: 0.0, y: 0.0, w: 1000.4, h: 1080.0 });
        s.rotate = 90;
        s.speed = 1.5;
        s.output = Output { format: VideoFormat::Gif, short: Some(480), quality: VideoQuality::Low };
        let ov = vec![VideoOverlay { asset_id: "a.png".into(), start: 0.0, end: 2.0 }];
        let v = serde_json::to_value(build_spec("7", &s, HD, 12.0, ov)).unwrap();
        assert_eq!(v["trim"], serde_json::json!({"start": 1.235, "end": 9}));
        assert_eq!(v["crop"], serde_json::json!({"x": 0, "y": 0, "w": 1000, "h": 1080}));
        assert_eq!(v["rotate"], 90);
        assert_eq!(v["speed"], 1.5);
        // the 1000×1080 crop turned is landscape: 480 is its height
        assert_eq!(v["output"]["height"], 480);
        assert_eq!(v["overlays"][0]["assetId"], "a.png");
        s.rotate = 0;
        let v = serde_json::to_value(build_spec("7", &s, HD, 12.0, vec![])).unwrap();
        assert_eq!(v["output"]["height"], 518);
        assert_eq!(base_name("clip.final.mp4"), "clip.final");
        assert_eq!(base_name(".hidden"), ".hidden");
    }
}
