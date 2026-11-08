//! Photo editor document (features/photo-editor: types.ts, adjust.ts, geometry.ts, history.ts):
//! the sliders, the filter presets, the geometry (quarter turns, flips, straighten, crop) and the
//! undo history. Pure, so the preview and the full-size export read the same values.

use std::time::{Duration, Instant};

/// Slider keys, in panel order. Values are "slider units" (mostly −100…100, hue in degrees).
pub const KEYS: [&str; 15] = [
    "exposure",
    "brightness",
    "contrast",
    "highlights",
    "shadows",
    "saturation",
    "vibrance",
    "temperature",
    "tint",
    "hue",
    "fade",
    "vignette",
    "grain",
    "sharpen",
    "blur",
];

/// (min, max) of each slider.
pub const RANGE: [(f32, f32); 15] = [
    (-100.0, 100.0),
    (-100.0, 100.0),
    (-100.0, 100.0),
    (-100.0, 100.0),
    (-100.0, 100.0),
    (-100.0, 100.0),
    (-100.0, 100.0),
    (-100.0, 100.0),
    (-100.0, 100.0),
    (-180.0, 180.0),
    (0.0, 100.0),
    (-100.0, 100.0),
    (0.0, 100.0),
    (0.0, 100.0),
    (0.0, 100.0),
];

/// Panel groups: (title key, first slider index, slider count).
pub const GROUPS: [(&str, usize, usize); 3] = [
    ("photo.group.light", 0, 5),
    ("photo.group.color", 5, 5),
    ("photo.group.effects", 10, 5),
];

pub fn key_index(key: &str) -> Option<usize> {
    KEYS.iter().position(|k| *k == key)
}

const EXPOSURE: usize = 0;
const BRIGHTNESS: usize = 1;
const CONTRAST: usize = 2;
const HIGHLIGHTS: usize = 3;
const SHADOWS: usize = 4;
const SATURATION: usize = 5;
const VIBRANCE: usize = 6;
const TEMPERATURE: usize = 7;
const TINT: usize = 8;
const HUE: usize = 9;
const FADE: usize = 10;
const VIGNETTE: usize = 11;
const GRAIN: usize = 12;
const SHARPEN: usize = 13;
const BLUR: usize = 14;

/// The sliders, indexed like [`KEYS`]. All zero = untouched.
pub type Adjust = [f32; 15];

/// Split toning offsets added in RGB (roughly −0.15…0.15).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SplitTone {
    pub shadows: [f32; 3],
    pub highlights: [f32; 3],
}

pub struct FilterPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub adjust: &'static [(usize, f32)],
    pub split: Option<SplitTone>,
}

pub const FILTERS: [FilterPreset; 12] = [
    FilterPreset {
        id: "vivid",
        label: "photo.filter.vivid",
        adjust: &[(CONTRAST, 14.0), (SATURATION, 22.0), (VIBRANCE, 34.0), (SHADOWS, 8.0), (HIGHLIGHTS, -6.0)],
        split: None,
    },
    FilterPreset {
        id: "warm",
        label: "photo.filter.warm",
        adjust: &[(TEMPERATURE, 32.0), (TINT, 6.0), (VIBRANCE, 12.0), (HIGHLIGHTS, -8.0), (EXPOSURE, 3.0)],
        split: None,
    },
    FilterPreset {
        id: "cool",
        label: "photo.filter.cool",
        adjust: &[(TEMPERATURE, -34.0), (TINT, -4.0), (SATURATION, -6.0), (CONTRAST, 6.0)],
        split: None,
    },
    FilterPreset {
        id: "mono",
        label: "photo.filter.mono",
        adjust: &[(SATURATION, -100.0), (CONTRAST, 12.0), (SHADOWS, 6.0)],
        split: None,
    },
    FilterPreset {
        id: "noir",
        label: "photo.filter.noir",
        adjust: &[
            (SATURATION, -100.0),
            (CONTRAST, 34.0),
            (EXPOSURE, -6.0),
            (SHADOWS, -14.0),
            (HIGHLIGHTS, -14.0),
            (VIGNETTE, 36.0),
            (GRAIN, 12.0),
        ],
        split: None,
    },
    FilterPreset {
        id: "vintage",
        label: "photo.filter.vintage",
        adjust: &[
            (TEMPERATURE, 22.0),
            (FADE, 34.0),
            (SATURATION, -22.0),
            (CONTRAST, -8.0),
            (VIGNETTE, 28.0),
            (GRAIN, 12.0),
        ],
        split: Some(SplitTone { shadows: [0.04, 0.0, -0.05], highlights: [0.05, 0.03, -0.04] }),
    },
    FilterPreset {
        id: "fade",
        label: "photo.filter.fade",
        adjust: &[(FADE, 46.0), (CONTRAST, -16.0), (SATURATION, -14.0), (HIGHLIGHTS, -10.0), (EXPOSURE, 4.0)],
        split: None,
    },
    FilterPreset {
        id: "film",
        label: "photo.filter.film",
        adjust: &[(CONTRAST, 10.0), (FADE, 16.0), (GRAIN, 26.0), (TEMPERATURE, 6.0), (SATURATION, -6.0)],
        split: Some(SplitTone { shadows: [-0.03, 0.02, 0.05], highlights: [0.05, 0.02, -0.03] }),
    },
    FilterPreset {
        id: "dramatic",
        label: "photo.filter.dramatic",
        adjust: &[
            (CONTRAST, 36.0),
            (HIGHLIGHTS, -34.0),
            (SHADOWS, 22.0),
            (SATURATION, -12.0),
            (VIGNETTE, 32.0),
            (EXPOSURE, -4.0),
            (SHARPEN, 20.0),
        ],
        split: None,
    },
    FilterPreset {
        id: "matte",
        label: "photo.filter.matte",
        adjust: &[(FADE, 30.0), (CONTRAST, -20.0), (SATURATION, -10.0), (SHADOWS, 16.0), (VIBRANCE, 8.0)],
        split: None,
    },
    FilterPreset {
        id: "golden",
        label: "photo.filter.golden",
        adjust: &[(TEMPERATURE, 38.0), (VIBRANCE, 22.0), (HIGHLIGHTS, 10.0), (EXPOSURE, 5.0), (TINT, 8.0)],
        split: Some(SplitTone { shadows: [0.03, 0.0, -0.04], highlights: [0.06, 0.03, -0.05] }),
    },
    FilterPreset {
        id: "cinema",
        label: "photo.filter.cinema",
        adjust: &[(CONTRAST, 20.0), (SATURATION, -8.0), (FADE, 8.0), (VIGNETTE, 18.0)],
        split: Some(SplitTone { shadows: [-0.05, 0.03, 0.08], highlights: [0.08, 0.03, -0.06] }),
    },
];

pub fn filter(id: &str) -> Option<&'static FilterPreset> {
    FILTERS.iter().find(|f| f.id == id)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Filter {
    pub id: Option<String>,
    /// 0…100
    pub strength: f32,
}

/// Resolved shader parameters (normalized to roughly −1…1; hue in radians).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RenderAdjust {
    pub exposure: f32,
    pub brightness: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub saturation: f32,
    pub vibrance: f32,
    pub temperature: f32,
    pub tint: f32,
    pub hue: f32,
    pub fade: f32,
    pub vignette: f32,
    pub grain: f32,
    pub sharpen: f32,
    pub blur: f32,
    pub split: SplitTone,
}

/// The user's sliders combined with the chosen preset (scaled by its strength).
pub fn effective_adjust(adjust: &Adjust, filter: Option<&Filter>) -> RenderAdjust {
    let preset = filter.and_then(|f| f.id.as_deref()).and_then(self::filter);
    let k = if preset.is_some() { filter.map_or(0.0, |f| f.strength) / 100.0 } else { 0.0 };
    let pv = |i: usize| -> f32 {
        preset
            .and_then(|p| p.adjust.iter().find(|(j, _)| *j == i))
            .map_or(0.0, |(_, v)| *v)
    };
    let v = |i: usize| -> f32 {
        let (lo, hi) = RANGE[i];
        (adjust[i] + pv(i) * k).clamp(lo, hi)
    };
    let split = preset.and_then(|p| p.split).unwrap_or_default();
    // saturation multiplies, so "Mono" stays mono even after boosting saturation
    let sat = (1.0 + adjust[SATURATION] / 100.0) * (1.0 + pv(SATURATION) * k / 100.0) - 1.0;
    RenderAdjust {
        exposure: v(EXPOSURE) / 100.0,
        brightness: v(BRIGHTNESS) / 100.0,
        contrast: v(CONTRAST) / 100.0,
        highlights: v(HIGHLIGHTS) / 100.0,
        shadows: v(SHADOWS) / 100.0,
        saturation: sat.clamp(-1.0, 1.5),
        vibrance: v(VIBRANCE) / 100.0,
        temperature: v(TEMPERATURE) / 100.0,
        tint: v(TINT) / 100.0,
        hue: v(HUE).to_radians(),
        fade: v(FADE) / 100.0,
        vignette: v(VIGNETTE) / 100.0,
        grain: v(GRAIN) / 100.0,
        sharpen: v(SHARPEN) / 100.0,
        blur: v(BLUR) / 100.0,
        split: SplitTone {
            shadows: split.shadows.map(|x| x * k),
            highlights: split.highlights.map(|x| x * k),
        },
    }
}

pub fn neutral_render() -> RenderAdjust {
    effective_adjust(&[0.0; 15], None)
}

// ───────────── geometry ─────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

pub const FULL_CROP: Rect = Rect { x: 0.0, y: 0.0, w: 1.0, h: 1.0 };

pub const ASPECTS: [&str; 7] = ["free", "original", "1:1", "4:5", "3:4", "16:9", "9:16"];

/// The frame is the source after quarter turns + flips (in source pixels). Straighten rotates
/// the image inside the frame and zooms so the frame stays covered; the crop is normalized.
#[derive(Debug, Clone, PartialEq)]
pub struct Geo {
    /// clockwise quarter turns
    pub rot: u8,
    pub flip_h: bool,
    pub flip_v: bool,
    /// degrees, −45…45
    pub straighten: f64,
    pub crop: Rect,
    pub aspect: &'static str,
}

impl Default for Geo {
    fn default() -> Self {
        Geo { rot: 0, flip_h: false, flip_v: false, straighten: 0.0, crop: FULL_CROP, aspect: "free" }
    }
}

impl Geo {
    pub fn crop_untouched(&self) -> bool {
        self.straighten == 0.0 && self.aspect == "free" && self.crop == FULL_CROP
    }
}

/// Straight (not premultiplied) RGBA.
pub type Rgba = [u8; 4];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextOutline {
    pub color: Rgba,
    pub width: f64,
}

/// Canvas-style shadow: `blur` is twice the Gaussian sigma.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextShadow {
    pub color: Rgba,
    pub blur: f64,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextBackground {
    pub color: Rgba,
    pub padding: f64,
    pub radius: f64,
}

/// A text layer (lib/text.ts). Sizes and the centre (x, y) are in frame pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct TextLayer {
    pub id: u32,
    pub text: String,
    pub family: String,
    pub weight: f32,
    pub italic: bool,
    pub size: f64,
    pub color: Rgba,
    pub align: Align,
    pub line_height: f64,
    pub letter_spacing: f64,
    pub x: f64,
    pub y: f64,
    /// degrees, clockwise
    pub rotation: f64,
    pub opacity: f64,
    pub stroke: Option<TextOutline>,
    pub shadow: Option<TextShadow>,
    pub background: Option<TextBackground>,
}

impl TextLayer {
    /// Everything that changes the rasterized look; position, rotation and opacity don't.
    pub fn look_eq(&self, o: &TextLayer) -> bool {
        self.text == o.text
            && self.family == o.family
            && self.weight == o.weight
            && self.italic == o.italic
            && self.size == o.size
            && self.color == o.color
            && self.align == o.align
            && self.line_height == o.line_height
            && self.letter_spacing == o.letter_spacing
            && self.stroke == o.stroke
            && self.shadow == o.shadow
            && self.background == o.background
    }

    /// Every size-like property times `k` (a corner-handle resize).
    pub fn scaled(&self, k: f64) -> TextLayer {
        let r = |n: f64| (n * k * 100.0).round() / 100.0;
        TextLayer {
            size: r(self.size).max(4.0),
            letter_spacing: r(self.letter_spacing),
            stroke: self.stroke.map(|s| TextOutline { width: r(s.width), ..s }),
            shadow: self.shadow.map(|s| TextShadow { blur: r(s.blur), x: r(s.x), y: r(s.y), ..s }),
            background: self.background.map(|b| TextBackground { padding: r(b.padding), radius: r(b.radius), ..b }),
            ..self.clone()
        }
    }
}

/// Freehand brush stroke (strokes.ts); points and size in frame pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Stroke {
    pub color: Rgba,
    pub size: f64,
    pub opacity: f64,
    pub points: Vec<[f64; 2]>,
}

/// Everything that defines an edit — the unit of undo / redo.
#[derive(Debug, Clone, PartialEq)]
pub struct EditDoc {
    pub adjust: Adjust,
    pub filter: Filter,
    pub geo: Geo,
    /// bottom to top
    pub layers: Vec<TextLayer>,
    pub strokes: Vec<Stroke>,
}

impl Default for EditDoc {
    fn default() -> Self {
        EditDoc {
            adjust: [0.0; 15],
            filter: Filter { id: None, strength: 100.0 },
            geo: Geo::default(),
            layers: Vec::new(),
            strokes: Vec::new(),
        }
    }
}

/// Layers and strokes through a frame-pixel point map (quarter turns and flips).
fn map_overlays(doc: &EditDoc, f: impl Fn(f64, f64) -> (f64, f64), mirror: bool) -> (Vec<TextLayer>, Vec<Stroke>) {
    let layers = doc
        .layers
        .iter()
        .map(|l| {
            let (x, y) = f(l.x, l.y);
            TextLayer { x, y, rotation: if mirror { -l.rotation } else { l.rotation }, ..l.clone() }
        })
        .collect();
    let strokes = doc
        .strokes
        .iter()
        .map(|s| Stroke {
            points: s
                .points
                .iter()
                .map(|p| {
                    let (x, y) = f(p[0], p[1]);
                    [x, y]
                })
                .collect(),
            ..s.clone()
        })
        .collect();
    (layers, strokes)
}

/// Frame size (after quarter turns) in source pixels.
pub fn frame_size(src_w: f64, src_h: f64, rot: u8) -> (f64, f64) {
    if rot % 2 == 1 {
        (src_h, src_w)
    } else {
        (src_w, src_h)
    }
}

/// Zoom needed so a straightened image still covers its whole frame.
pub fn straighten_zoom(frame_w: f64, frame_h: f64, deg: f64) -> f64 {
    let a = deg.to_radians().abs();
    a.cos() + (frame_w / frame_h).max(frame_h / frame_w) * a.sin()
}

/// Crop rect in frame pixels, rounded to whole output pixels.
pub fn crop_px(geo: &Geo, frame_w: f64, frame_h: f64) -> Rect {
    let x = (geo.crop.x * frame_w).round();
    let y = (geo.crop.y * frame_h).round();
    let w = (geo.crop.w * frame_w).round().min(frame_w - x).max(1.0);
    let h = (geo.crop.h * frame_h).round().min(frame_h - y).max(1.0);
    Rect { x, y, w, h }
}

/// Pixel aspect (w / h) of a preset, None for free.
pub fn aspect_ratio(key: &str, frame_w: f64, frame_h: f64) -> Option<f64> {
    match key {
        "original" => Some(frame_w / frame_h),
        "1:1" => Some(1.0),
        "4:5" => Some(4.0 / 5.0),
        "3:4" => Some(3.0 / 4.0),
        "16:9" => Some(16.0 / 9.0),
        "9:16" => Some(9.0 / 16.0),
        _ => None,
    }
}

/// Largest crop of the given pixel aspect centred on the current crop, inside the frame.
pub fn fit_aspect(crop: Rect, ratio: f64, frame_w: f64, frame_h: f64) -> Rect {
    let cx = (crop.x + crop.w / 2.0) * frame_w;
    let cy = (crop.y + crop.h / 2.0) * frame_h;
    let mut w = frame_w;
    let mut h = w / ratio;
    if h > frame_h {
        h = frame_h;
        w = h * ratio;
    }
    // shrunk a little when the current crop is small, so presets feel "local"
    let area = crop.w * frame_w * crop.h * frame_h;
    if area < w * h * 0.9 {
        let k = (area.max(w * h * 0.25) / (w * h)).sqrt();
        w *= k;
        h *= k;
    }
    let x = (cx - w / 2.0).max(0.0).min(frame_w - w);
    let y = (cy - h / 2.0).max(0.0).min(frame_h - h);
    Rect { x: x / frame_w, y: y / frame_h, w: w / frame_w, h: h / frame_h }
}

/// Rotate the edit 90° clockwise (dir = 1) or counter-clockwise (dir = −1); `frame_w` ×
/// `frame_h` is the frame before the turn.
pub fn rotate_doc(doc: &EditDoc, dir: i32, frame_w: f64, frame_h: f64) -> EditDoc {
    let g = &doc.geo;
    let one_flip = g.flip_h != g.flip_v;
    let rot = (g.rot as i32 + if one_flip { -dir } else { dir }).rem_euclid(4) as u8;
    let c = g.crop;
    let crop = if dir == 1 {
        Rect { x: 1.0 - c.y - c.h, y: c.x, w: c.h, h: c.w }
    } else {
        Rect { x: c.y, y: 1.0 - c.x - c.w, w: c.h, h: c.w }
    };
    let aspect = if g.aspect == "1:1" || g.aspect == "free" { g.aspect } else { "free" };
    let (layers, strokes) = if dir == 1 {
        map_overlays(doc, |x, y| (frame_h - y, x), false)
    } else {
        map_overlays(doc, |x, y| (y, frame_w - x), false)
    };
    EditDoc { geo: Geo { rot, crop, aspect, ..g.clone() }, layers, strokes, ..doc.clone() }
}

pub fn flip_doc(doc: &EditDoc, horizontal: bool, frame_w: f64, frame_h: f64) -> EditDoc {
    let g = &doc.geo;
    let c = g.crop;
    let crop = if horizontal {
        Rect { x: 1.0 - c.x - c.w, ..c }
    } else {
        Rect { y: 1.0 - c.y - c.h, ..c }
    };
    let (layers, strokes) = if horizontal {
        map_overlays(doc, |x, y| (frame_w - x, y), true)
    } else {
        map_overlays(doc, |x, y| (x, frame_h - y), true)
    };
    EditDoc {
        geo: Geo {
            flip_h: g.flip_h ^ horizontal,
            flip_v: g.flip_v ^ !horizontal,
            straighten: -g.straighten,
            crop,
            ..g.clone()
        },
        layers,
        strokes,
        ..doc.clone()
    }
}

/// Crop handle being dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    Move,
    N,
    S,
    E,
    W,
    NW,
    NE,
    SW,
    SE,
}

impl Handle {
    pub fn parse(s: &str) -> Option<Handle> {
        Some(match s {
            "move" => Handle::Move,
            "n" => Handle::N,
            "s" => Handle::S,
            "e" => Handle::E,
            "w" => Handle::W,
            "nw" => Handle::NW,
            "ne" => Handle::NE,
            "sw" => Handle::SW,
            "se" => Handle::SE,
            _ => return None,
        })
    }
    fn has_w(self) -> bool {
        matches!(self, Handle::W | Handle::NW | Handle::SW)
    }
    fn has_e(self) -> bool {
        matches!(self, Handle::E | Handle::NE | Handle::SE)
    }
    fn has_n(self) -> bool {
        matches!(self, Handle::N | Handle::NW | Handle::NE)
    }
    fn has_s(self) -> bool {
        matches!(self, Handle::S | Handle::SW | Handle::SE)
    }
    fn corner(self) -> bool {
        matches!(self, Handle::NW | Handle::NE | Handle::SW | Handle::SE)
    }
}

/// New crop (frame px) after dragging a handle by (dx, dy) frame px (CropOverlay.tsx).
pub fn resize_crop(s: Rect, mode: Handle, dx: f64, dy: f64, ratio: Option<f64>, fw: f64, fh: f64, min: f64) -> Rect {
    let clamp = |v: f64, lo: f64, hi: f64| v.max(lo).min(hi);
    if mode == Handle::Move {
        return Rect { x: clamp(s.x + dx, 0.0, fw - s.w), y: clamp(s.y + dy, 0.0, fh - s.h), ..s };
    }
    let (mut x0, mut y0, mut x1, mut y1) = (s.x, s.y, s.x + s.w, s.y + s.h);
    if mode.has_w() {
        x0 = clamp(x0 + dx, 0.0, x1 - min);
    }
    if mode.has_e() {
        x1 = clamp(x1 + dx, x0 + min, fw);
    }
    if mode.has_n() {
        y0 = clamp(y0 + dy, 0.0, y1 - min);
    }
    if mode.has_s() {
        y1 = clamp(y1 + dy, y0 + min, fh);
    }
    let Some(ratio) = ratio else {
        return Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
    };
    if mode.corner() {
        // keep the opposite corner fixed
        let ax = if mode.has_w() { s.x + s.w } else { s.x };
        let ay = if mode.has_n() { s.y + s.h } else { s.y };
        let mut w = x1 - x0;
        let mut h = y1 - y0;
        if w / h > ratio {
            w = h * ratio;
        } else {
            h = w / ratio;
        }
        let max_w = if mode.has_w() { ax } else { fw - ax };
        let max_h = if mode.has_n() { ay } else { fh - ay };
        if w > max_w {
            w = max_w;
            h = w / ratio;
        }
        if h > max_h {
            h = max_h;
            w = h * ratio;
        }
        return Rect {
            x: if mode.has_w() { ax - w } else { ax },
            y: if mode.has_n() { ay - h } else { ay },
            w,
            h,
        };
    }
    // edge: the other dimension follows, centred
    if mode == Handle::N || mode == Handle::S {
        let mut h = y1 - y0;
        let mut w = h * ratio;
        if w > fw {
            w = fw;
            h = w / ratio;
        }
        let cx = s.x + s.w / 2.0;
        return Rect {
            x: clamp(cx - w / 2.0, 0.0, fw - w),
            y: if mode == Handle::N { s.y + s.h - h } else { s.y },
            w,
            h,
        };
    }
    let mut w = x1 - x0;
    let mut h = w / ratio;
    if h > fh {
        h = fh;
        w = h * ratio;
    }
    let cy = s.y + s.h / 2.0;
    Rect { x: if mode == Handle::W { s.x + s.w - w } else { s.x }, y: clamp(cy - h / 2.0, 0.0, fh - h), w, h }
}

/// What part of the frame is on screen: `scale` stage px per frame px, centred on (cx, cy).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub scale: f64,
    pub cx: f64,
    pub cy: f64,
}

pub fn fit_view(region: Rect, stage_w: f64, stage_h: f64, pad: f64) -> View {
    let s = ((stage_w - pad * 2.0) / region.w).min((stage_h - pad * 2.0) / region.h);
    View { scale: s.max(0.01), cx: region.x + region.w / 2.0, cy: region.y + region.h / 2.0 }
}

/// Keep the region on screen when zoomed / panned.
pub fn clamp_view(v: View, region: Rect, stage_w: f64, stage_h: f64) -> View {
    let half_w = stage_w / 2.0 / v.scale;
    let half_h = stage_h / 2.0 / v.scale;
    let axis = |c: f64, lo: f64, len: f64, half: f64| {
        if len <= half * 2.0 {
            lo + len / 2.0
        } else {
            c.max(lo + half).min(lo + len - half)
        }
    };
    View {
        scale: v.scale,
        cx: axis(v.cx, region.x, region.w, half_w),
        cy: axis(v.cy, region.y, region.h, half_h),
    }
}

/// Grain cell size (frame px) at full resolution; the preview uses max(this, 1 screen px).
pub fn grain_cell_for(crop: Rect) -> f64 {
    (crop.w.min(crop.h) / 1600.0).max(1.0)
}

// ───────────── history ─────────────

const LIMIT: usize = 120;
const COALESCE: Duration = Duration::from_millis(900);

/// Undo / redo; consecutive changes with the same key within 900 ms merge into one step
/// (slider drags), so history stays meaningful without commit callbacks.
pub struct History<T> {
    pub doc: T,
    past: Vec<T>,
    future: Vec<T>,
    last_key: Option<String>,
    last_at: Option<Instant>,
}

impl<T: Clone + PartialEq> History<T> {
    pub fn new(doc: T) -> Self {
        History { doc, past: vec![], future: vec![], last_key: None, last_at: None }
    }

    pub fn change(&mut self, next: T, key: Option<&str>) {
        if next == self.doc {
            return;
        }
        let now = Instant::now();
        let merge = key.is_some()
            && key == self.last_key.as_deref()
            && self.last_at.is_some_and(|t| now.duration_since(t) < COALESCE);
        if !merge {
            if self.past.len() >= LIMIT {
                self.past.remove(0);
            }
            self.past.push(std::mem::replace(&mut self.doc, next));
        } else {
            self.doc = next;
        }
        self.future.clear();
        self.last_key = key.map(str::to_string);
        self.last_at = Some(now);
    }

    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.past.pop() else { return false };
        self.future.insert(0, std::mem::replace(&mut self.doc, prev));
        self.last_key = None;
        true
    }

    pub fn redo(&mut self) -> bool {
        if self.future.is_empty() {
            return false;
        }
        let next = self.future.remove(0);
        self.past.push(std::mem::replace(&mut self.doc, next));
        self.last_key = None;
        true
    }

    /// the next change starts a new step (end of a drag)
    pub fn seal(&mut self) {
        self.last_key = None;
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_adjust_mixes_preset_by_strength() {
        let mut a = [0.0; 15];
        a[CONTRAST] = 10.0;
        let f = Filter { id: Some("vivid".into()), strength: 50.0 };
        let r = effective_adjust(&a, Some(&f));
        assert!((r.contrast - 0.17).abs() < 1e-6);
        assert!((r.vibrance - 0.17).abs() < 1e-6);
        // mono stays mono whatever the user's saturation
        a[SATURATION] = 80.0;
        let m = effective_adjust(&a, Some(&Filter { id: Some("mono".into()), strength: 100.0 }));
        assert_eq!(m.saturation, -1.0);
        let s = effective_adjust(&a, Some(&Filter { id: Some("vintage".into()), strength: 50.0 }));
        assert!((s.split.shadows[0] - 0.02).abs() < 1e-6);
        assert_eq!(neutral_render(), RenderAdjust::default());
    }

    #[test]
    fn crop_px_rounds_inside_the_frame() {
        let g = Geo { crop: Rect { x: 0.1, y: 0.25, w: 0.95, h: 0.5 }, ..Geo::default() };
        let r = crop_px(&g, 1000.0, 800.0);
        assert_eq!(r, Rect { x: 100.0, y: 200.0, w: 900.0, h: 400.0 });
    }

    #[test]
    fn rotate_and_flip_remap_the_crop() {
        let mut d = EditDoc::default();
        d.geo.crop = Rect { x: 0.1, y: 0.2, w: 0.3, h: 0.4 };
        d.strokes.push(Stroke { color: [0, 0, 0, 255], size: 4.0, opacity: 1.0, points: vec![[10.0, 20.0]] });
        let r = rotate_doc(&d, 1, 400.0, 300.0);
        assert_eq!(r.geo.rot, 1);
        assert!((r.geo.crop.x - 0.4).abs() < 1e-9 && (r.geo.crop.y - 0.1).abs() < 1e-9);
        assert_eq!(r.strokes[0].points[0], [280.0, 10.0]);
        let back = rotate_doc(&r, -1, 300.0, 400.0);
        assert_eq!(back.geo.rot, 0);
        assert!((back.geo.crop.x - 0.1).abs() < 1e-9 && (back.geo.crop.y - 0.2).abs() < 1e-9);
        assert_eq!(back.strokes, d.strokes);
        let f = flip_doc(&d, true, 400.0, 300.0);
        assert!(f.geo.flip_h && !f.geo.flip_v);
        assert!((f.geo.crop.x - 0.6).abs() < 1e-9);
        assert_eq!(f.strokes[0].points[0], [390.0, 20.0]);
        // with one flip a clockwise turn counts backwards
        assert_eq!(rotate_doc(&f, 1, 400.0, 300.0).geo.rot, 3);
    }

    #[test]
    fn aspect_presets_fit_the_frame() {
        let r = fit_aspect(FULL_CROP, 1.0, 1600.0, 900.0);
        assert!((r.w * 1600.0 - 900.0).abs() < 1e-6 && (r.h - 1.0).abs() < 1e-9);
        assert!((r.x - 350.0 / 1600.0).abs() < 1e-9);
        assert!((straighten_zoom(100.0, 100.0, 0.0) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn resize_keeps_ratio_and_bounds() {
        let s = Rect { x: 100.0, y: 100.0, w: 200.0, h: 200.0 };
        let r = resize_crop(s, Handle::SE, 50.0, 10.0, Some(1.0), 1000.0, 1000.0, 16.0);
        assert_eq!((r.x, r.y), (100.0, 100.0));
        assert!((r.w - r.h).abs() < 1e-9);
        let m = resize_crop(s, Handle::Move, -500.0, 0.0, None, 1000.0, 1000.0, 16.0);
        assert_eq!(m.x, 0.0);
        let w = resize_crop(s, Handle::W, -50.0, 0.0, None, 1000.0, 1000.0, 16.0);
        assert_eq!((w.x, w.w), (50.0, 250.0));
    }

    #[test]
    fn history_coalesces_same_key() {
        let mut h = History::new(0);
        h.change(1, Some("a"));
        h.change(2, Some("a"));
        h.change(3, Some("b"));
        assert!(h.undo());
        assert_eq!(h.doc, 2);
        assert!(h.undo());
        assert_eq!(h.doc, 0);
        assert!(!h.undo());
        assert!(h.redo());
        assert_eq!(h.doc, 2);
        h.change(9, None);
        assert!(!h.can_redo());
    }
}
