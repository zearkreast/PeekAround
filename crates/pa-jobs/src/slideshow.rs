//! Pure planning + ffmpeg argument building for slideshow → MP4 export (mirrors `jobs/slideshow.ts`).
//!
//! Rendering model (memory bounded for any number of slides): every image is pre-rendered to a raw
//! RGB canvas; the timeline is cut into one piece per slide (from the end of the previous
//! transition to its own end, cross-fading into the head of the next slide); every piece is
//! rendered by a short ffmpeg (≤ 2 inputs) as raw yuv420p frames streamed into one long-running
//! x264 encoder that also muxes the (looped, trimmed, faded) music. Timing is in whole frames.

use crate::error::HttpError;
use crate::js::{f, num as jnum, round};
use crate::paths::sanitize_name;
use crate::spec::{in_range, is_object, js_string, nullish, num, prop, truthy, StrNum};
use pa_core::SlideTransition;
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::sync::LazyLock;

/// Our transition names → ffmpeg xfade transitions (all present in ffmpeg ≥ 5.0).
pub fn xfade_of(t: SlideTransition) -> Option<&'static str> {
    Some(match t {
        SlideTransition::Fade => "fade",
        SlideTransition::Slide => "slideleft",
        SlideTransition::Zoom => "zoomin",
        SlideTransition::Blur => "hblur",
        SlideTransition::Flip => "squeezev",
        SlideTransition::Circle => "circleopen",
        SlideTransition::Kenburns => "fade",
        SlideTransition::None | SlideTransition::Random => return None,
    })
}

/// Pool for `random` (picked per cut, never the same twice in a row).
pub const RANDOM_TRANSITIONS: [&str; 16] = [
    "fade",
    "slideleft",
    "slideright",
    "slideup",
    "slidedown",
    "circleopen",
    "circleclose",
    "zoomin",
    "hblur",
    "squeezev",
    "smoothleft",
    "smoothright",
    "radial",
    "fadeblack",
    "wipeleft",
    "diagtl",
];

pub const SLIDE_TRANSITIONS: [(&str, SlideTransition); 9] = [
    ("none", SlideTransition::None),
    ("fade", SlideTransition::Fade),
    ("slide", SlideTransition::Slide),
    ("zoom", SlideTransition::Zoom),
    ("kenburns", SlideTransition::Kenburns),
    ("blur", SlideTransition::Blur),
    ("flip", SlideTransition::Flip),
    ("circle", SlideTransition::Circle),
    ("random", SlideTransition::Random),
];

pub const MAX_ITEMS: usize = 1000;
pub const MAX_DURATION: f64 = 3.0 * 3600.0;
/// Ken Burns frames are cropped from a canvas up to 2× the output (≤ 7680 px), in yuv444p.
pub const MAX_KB_CANVAS: f64 = 7680.0;
pub const MAX_KB_SCALE: f64 = 2.0;
pub const SLIDESHOW_CRF: u32 = 20;

fn bad(m: impl Into<String>) -> HttpError {
    HttpError::bad(m)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SlideItem {
    pub id: String,
    pub duration: Option<f64>,
    pub trim: Option<crate::video_edit::Trim>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub folder_id: String,
    pub dir: String,
}

/// `NormalizedSlideshow`
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedSlideshow {
    pub items: Vec<SlideItem>,
    pub duration: f64,
    pub transition: SlideTransition,
    pub transition_duration: f64,
    pub ken_burns: bool,
    pub audio_id: Option<String>,
    pub width: f64,
    pub height: f64,
    pub fps: f64,
    /// '#rrggbb'
    pub background: String,
    pub target: Option<Target>,
    /// file name without extension (sanitized)
    pub name: String,
}

fn n(v: Option<&Value>, name: &str, def: Option<f64>) -> Result<f64, HttpError> {
    num(v, name, def, StrNum::Any)
}

fn media_id(v: Option<&Value>, name: &str) -> Result<String, HttpError> {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+$").unwrap());
    let s = match v {
        Some(x @ (Value::String(_) | Value::Number(_))) => js_string(x).unwrap_or_default(),
        _ => return Err(bad(format!("{name} is required"))),
    };
    if !RE.is_match(&s) {
        return Err(bad(format!("Invalid {name}")));
    }
    Ok(s)
}

/// `'#abc' | 'abc' | '#aabbcc'` → `'#aabbcc'` (absent → black)
pub fn normalize_hex(v: Option<&Value>) -> Result<String, HttpError> {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^#?([0-9a-f]{3}|[0-9a-f]{6})$").unwrap());
    if nullish(v) || matches!(v, Some(Value::String(s)) if s.is_empty()) {
        return Ok("#000000".into());
    }
    let s = match v.unwrap() {
        Value::String(s) => s.clone(),
        other => pa_core::js::to_string_of(other),
    };
    let s = pa_core::js::trim(&s).to_string();
    let Some(m) = RE.captures(&s) else {
        return Err(bad("background must be a hex colour like #000000"));
    };
    let mut h = m[1].to_lowercase();
    if h.len() == 3 {
        h = h.chars().flat_map(|c| [c, c]).collect();
    }
    Ok(format!("#{h}"))
}

/// Shape / range validation of a client spec (any JSON). Errors are 400s with the TS messages.
pub fn normalize_slideshow_spec(spec: &Value) -> Result<NormalizedSlideshow, HttpError> {
    let s = Some(spec);
    if !truthy(s) || !is_object(s) {
        return Err(bad("Invalid slideshow spec"));
    }
    let items_v = match prop(s, "items") {
        Some(Value::Array(a)) if !a.is_empty() => a,
        _ => return Err(bad("items must contain at least one media id")),
    };
    if items_v.len() > MAX_ITEMS {
        return Err(bad(format!("At most {MAX_ITEMS} items are supported")));
    }
    let mut items = Vec::with_capacity(items_v.len());
    for (i, it) in items_v.iter().enumerate() {
        let it = Some(it);
        if !truthy(it) || !is_object(it) {
            return Err(bad(format!("Invalid item {}", i + 1)));
        }
        let id = media_id(prop(it, "id"), &format!("items[{i}].id"))?;
        let d = prop(it, "duration");
        let duration = if nullish(d) {
            None
        } else {
            let name = format!("items[{i}].duration");
            Some(in_range(n(d, &name, None)?, 0.2, 3600.0, &name)?)
        };
        let mut trim = None;
        let t = prop(it, "trim");
        if truthy(t) {
            let start = n(
                prop(t, "start"),
                &format!("items[{i}].trim.start"),
                Some(0.0),
            )?
            .max(0.0);
            let end = n(prop(t, "end"), &format!("items[{i}].trim.end"), None)?;
            if end - start < 0.05 {
                return Err(bad(format!("items[{i}].trim.end must be after trim.start")));
            }
            trim = Some(crate::video_edit::Trim { start, end });
        }
        items.push(SlideItem { id, duration, trim });
    }

    let tv = prop(s, "transition");
    let transition = if nullish(tv) {
        SlideTransition::Fade
    } else {
        match tv {
            Some(Value::String(x)) => SLIDE_TRANSITIONS
                .iter()
                .find(|(k, _)| k == x)
                .map(|(_, t)| *t)
                .ok_or_else(|| bad(format!("Unknown transition \"{x}\"")))?,
            Some(other) => {
                return Err(bad(format!(
                    "Unknown transition \"{}\"",
                    pa_core::js::to_string_of(other)
                )))
            }
            None => unreachable!(),
        }
    };

    let even = |x: f64| round(x / 2.0) * 2.0;
    let width = even(in_range(
        round(n(prop(s, "width"), "width", Some(1920.0))?),
        64.0,
        4096.0,
        "width",
    )?);
    let height = even(in_range(
        round(n(prop(s, "height"), "height", Some(1080.0))?),
        64.0,
        4096.0,
        "height",
    )?);

    let mut target = None;
    let tg = prop(s, "target");
    if truthy(tg) {
        if !is_object(tg) {
            return Err(bad("Invalid target"));
        }
        target = Some(Target {
            folder_id: media_id(prop(tg, "folderId"), "target.folderId")?,
            dir: match prop(tg, "dir") {
                Some(Value::String(d)) => d.clone(),
                _ => String::new(),
            },
        });
    }

    let mut name = match prop(s, "name") {
        Some(Value::String(x)) => {
            static MP4: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\.mp4$").unwrap());
            MP4.replace(pa_core::js::trim(x), "").into_owned()
        }
        _ => String::new(),
    };
    if pa_core::js::len16(&name) > 150 {
        name = pa_core::js::slice16_to(&name, 150).to_string();
    }
    let name = if name.is_empty() {
        "Slayt".to_string()
    } else {
        sanitize_name(&name)
    };

    let duration = in_range(
        n(prop(s, "duration"), "duration", Some(4.0))?,
        0.2,
        600.0,
        "duration",
    )?;
    let transition_duration = in_range(
        n(
            prop(s, "transitionDuration"),
            "transitionDuration",
            Some(0.8),
        )?,
        0.0,
        10.0,
        "transitionDuration",
    )?;
    let ken_burns = truthy(prop(s, "kenBurns"));
    let a = prop(s, "audioId");
    let audio_id = if nullish(a) || matches!(a, Some(Value::String(x)) if x.is_empty()) {
        None
    } else {
        Some(media_id(a, "audioId")?)
    };
    let fps = round(in_range(
        n(prop(s, "fps"), "fps", Some(30.0))?,
        1.0,
        60.0,
        "fps",
    )?);
    let background = normalize_hex(prop(s, "background"))?;

    Ok(NormalizedSlideshow {
        items,
        duration,
        transition,
        transition_duration,
        ken_burns,
        audio_id,
        width,
        height,
        fps,
        background,
        target,
        name,
    })
}

/// What the runner knows about each item before planning.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SlideSource {
    Image,
    Video { duration: f64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KenBurnsMove {
    pub zoom_from: f64,
    pub zoom_to: f64,
    /// focus point as a fraction of the free margin (0 = left/top, 0.5 = centre, 1 = right/bottom)
    pub x_from: f64,
    pub x_to: f64,
    pub y_from: f64,
    pub y_to: f64,
}

const fn kb(
    zoom_from: f64,
    zoom_to: f64,
    x_from: f64,
    x_to: f64,
    y_from: f64,
    y_to: f64,
) -> KenBurnsMove {
    KenBurnsMove {
        zoom_from,
        zoom_to,
        x_from,
        x_to,
        y_from,
        y_to,
    }
}

/// Alternating zoom-in / zoom-out with varying pan directions.
pub const KEN_BURNS_MOVES: [KenBurnsMove; 4] = [
    kb(1.0, 1.15, 0.35, 0.65, 0.5, 0.5),
    kb(1.15, 1.0, 0.65, 0.35, 0.4, 0.6),
    kb(1.0, 1.15, 0.5, 0.5, 0.65, 0.35),
    kb(1.15, 1.0, 0.35, 0.65, 0.35, 0.65),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SlideKind {
    Image,
    Video,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlidePlan {
    pub index: usize,
    pub kind: SlideKind,
    /// length in frames, including the parts shared with transitions
    pub frames: i64,
    /// video only: source second where the clip starts
    pub video_start: f64,
    pub ken_burns: Option<KenBurnsMove>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CutPlan {
    /// overlap in frames (0 = hard cut)
    pub frames: i64,
    pub transition: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SlideshowPlan {
    pub width: f64,
    pub height: f64,
    pub fps: f64,
    pub background: String,
    /// Ken Burns canvas scale factor
    pub kb_scale: f64,
    pub slides: Vec<SlidePlan>,
    /// cuts[i] is between slides i and i+1
    pub cuts: Vec<CutPlan>,
    pub total_frames: i64,
    /// seconds
    pub duration: f64,
}

/// Plan the timeline; `rnd` is `Math.random` (random transitions).
pub fn plan_slideshow(
    n: &NormalizedSlideshow,
    sources: &[SlideSource],
    rnd: &mut dyn FnMut() -> f64,
) -> Result<SlideshowPlan, HttpError> {
    if sources.len() != n.items.len() {
        return Err(HttpError::new(500, "sources do not match items"));
    }
    let fr = n.fps;
    let ken_burns = n.ken_burns || n.transition == SlideTransition::Kenburns;
    let mut kb_count = 0usize;
    let mut slides = Vec::with_capacity(sources.len());
    for (i, (it, src)) in n.items.iter().zip(sources).enumerate() {
        match *src {
            SlideSource::Image => {
                let secs = it.duration.unwrap_or(n.duration);
                let kbm = ken_burns.then(|| {
                    let m = KEN_BURNS_MOVES[kb_count % KEN_BURNS_MOVES.len()];
                    kb_count += 1;
                    m
                });
                slides.push(SlidePlan {
                    index: i,
                    kind: SlideKind::Image,
                    frames: (round(secs * fr) as i64).max(1),
                    video_start: 0.0,
                    ken_burns: kbm,
                });
            }
            SlideSource::Video { duration: dur } => {
                if !(dur > 0.0) {
                    return Err(bad(format!("Item {}: unknown video duration", i + 1)));
                }
                let mut start = 0.0;
                let mut end = dur;
                if let Some(t) = it.trim {
                    start = t.start.min(dur);
                    end = t.end.min(dur);
                } else if let Some(d) = it.duration.filter(|d| *d != 0.0) {
                    end = dur.min(d);
                }
                let frames = round((end - start) * fr) as i64;
                if frames < 1 {
                    return Err(bad(format!("Item {}: the video clip is too short", i + 1)));
                }
                slides.push(SlidePlan {
                    index: i,
                    kind: SlideKind::Video,
                    frames,
                    video_start: start,
                    ken_burns: None,
                });
            }
        }
    }

    let mut cuts = Vec::new();
    let mut prev: Option<String> = None;
    for i in 0..slides.len().saturating_sub(1) {
        let want = if n.transition == SlideTransition::None {
            0
        } else {
            round(n.transition_duration * fr) as i64
        };
        // each slide may give at most half of itself to each neighbouring transition
        let frames = want
            .min(slides[i].frames.div_euclid(2))
            .min(slides[i + 1].frames.div_euclid(2))
            .max(0);
        let mut transition = None;
        if frames > 0 {
            let t = if n.transition == SlideTransition::Random {
                let pool: Vec<&str> = RANDOM_TRANSITIONS
                    .iter()
                    .copied()
                    .filter(|t| Some(*t) != prev.as_deref())
                    .collect();
                let k = ((rnd() * pool.len() as f64).floor() as usize).min(pool.len() - 1);
                pool[k].to_string()
            } else {
                xfade_of(n.transition).unwrap_or("fade").to_string()
            };
            prev = Some(t.clone());
            transition = Some(t);
        }
        cuts.push(CutPlan {
            frames,
            transition: if frames > 0 { transition } else { None },
        });
    }

    let total_frames =
        slides.iter().map(|s| s.frames).sum::<i64>() - cuts.iter().map(|c| c.frames).sum::<i64>();
    let duration = total_frames as f64 / fr;
    if duration > MAX_DURATION {
        return Err(bad("The slideshow is too long (max 3 hours)"));
    }
    let kb_scale = if ken_burns {
        (MAX_KB_CANVAS / n.width.max(n.height))
            .floor()
            .min(MAX_KB_SCALE)
            .max(1.0)
    } else {
        1.0
    };
    Ok(SlideshowPlan {
        width: n.width,
        height: n.height,
        fps: fr,
        background: n.background.clone(),
        kb_scale,
        slides,
        cuts,
        total_frames,
        duration,
    })
}

/// Size of the pre-rendered RGB canvas of an image slide.
pub fn canvas_size(p: &SlideshowPlan, s: &SlidePlan) -> (u32, u32) {
    let k = if s.ken_burns.is_some() {
        p.kb_scale
    } else {
        1.0
    };
    ((p.width * k) as u32, (p.height * k) as u32)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Xfade {
    pub transition: String,
    pub frames: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PieceRange {
    pub slide: usize,
    /// first local frame of slide i rendered by this piece
    pub from: i64,
    /// frames this piece contributes to the output
    pub frames: i64,
    /// the cross-fade into the next slide at the end of this piece, if any
    pub xfade: Option<Xfade>,
}

pub fn piece_range(p: &SlideshowPlan, i: usize) -> PieceRange {
    let s = &p.slides[i];
    let from = if i > 0 { p.cuts[i - 1].frames } else { 0 };
    let cut = p.cuts.get(i);
    PieceRange {
        slide: i,
        from,
        frames: s.frames - from,
        xfade: cut.and_then(|c| {
            (c.frames > 0)
                .then_some(())
                .and(c.transition.clone())
                .map(|t| Xfade {
                    transition: t,
                    frames: c.frames,
                })
        }),
    }
}

/// An input of a piece.
#[derive(Debug, Clone, PartialEq)]
pub enum PieceInput {
    Canvas {
        path: String,
        width: u32,
        height: u32,
    },
    Video {
        path: String,
    },
}

const BT709: &str = "scale=out_color_matrix=bt709:out_range=tv,format=yuv420p,setsar=1";

/// zoompan expressions for a move, rendering local frames from.. of a slide of `total` frames.
pub fn ken_burns_expr(m: &KenBurnsMove, from: i64, total: i64) -> (String, String, String) {
    let den = (total - 1).max(1);
    let pp = format!("((on+{from})/{den})");
    let lerp = |a: f64, b: f64| {
        if (b - a).abs() < 1e-9 {
            f(a, 4)
        } else {
            format!("({}+{}*{pp})", f(a, 4), f(b - a, 4))
        }
    };
    (
        lerp(m.zoom_from, m.zoom_to),
        format!("(iw-iw/zoom)*{}", lerp(m.x_from, m.x_to)),
        format!("(ih-ih/zoom)*{}", lerp(m.y_from, m.y_to)),
    )
}

fn slide_branch(
    p: &SlideshowPlan,
    s: &SlidePlan,
    input: &PieceInput,
    idx: usize,
    from: i64,
    count: i64,
    label: &str,
) -> Result<(Vec<String>, String), HttpError> {
    let fr = jnum(p.fps);
    let w = jnum(p.width);
    let h = jnum(p.height);
    if s.kind == SlideKind::Video {
        let PieceInput::Video { path } = input else {
            return Err(HttpError::new(500, "video slide needs a video input"));
        };
        let ss = s.video_start + from as f64 / p.fps;
        let bg = format!("0x{}", &p.background[1..]);
        let mut args = Vec::new();
        if ss > 0.0005 {
            args.push("-ss".into());
            args.push(f(ss, 3));
        }
        args.push("-i".into());
        args.push(path.clone());
        return Ok((
            args,
            format!(
                "[{idx}:V:0]setpts=PTS-STARTPTS,scale={w}:{h}:force_original_aspect_ratio=decrease:force_divisible_by=2,\
pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color={bg},setsar=1,fps={fr},format=yuv420p,\
tpad=stop_mode=clone:stop=-1,trim=end_frame={count}[{label}]"
            ),
        ));
    }
    let PieceInput::Canvas {
        path,
        width,
        height,
    } = input
    else {
        return Err(HttpError::new(500, "image slide needs a canvas input"));
    };
    let args: Vec<String> = [
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "-video_size",
        &format!("{width}x{height}"),
        "-framerate",
        &fr,
        "-i",
        path,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(m) = &s.ken_burns {
        let (z, x, y) = ken_burns_expr(m, from, s.frames);
        return Ok((
            args,
            format!(
                "[{idx}:v]scale=out_color_matrix=bt709:out_range=tv,format=yuv444p,\
zoompan=z='{z}':x='{x}':y='{y}':d={count}:s={w}x{h}:fps={fr},format=yuv420p,setsar=1[{label}]"
            ),
        ));
    }
    Ok((
        args,
        format!("[{idx}:v]{BT709},tpad=stop_mode=clone:stop=-1,trim=end_frame={count}[{label}]"),
    ))
}

/// ffmpeg args rendering piece `i` as raw yuv420p frames on stdout; `inputs[0]` is slide i,
/// `inputs[1]` slide i+1 (only needed when the piece ends in a transition). Returns (args, frames).
pub fn build_piece_args(
    p: &SlideshowPlan,
    i: usize,
    inputs: &[PieceInput],
) -> Result<(Vec<String>, i64), HttpError> {
    let r = piece_range(p, i);
    let first = inputs
        .first()
        .ok_or_else(|| HttpError::new(500, "piece needs its slide"))?;
    let (a_args, a_filter) = slide_branch(
        p,
        &p.slides[i],
        first,
        0,
        r.from,
        r.frames,
        if r.xfade.is_some() { "a" } else { "v" },
    )?;
    let mut args = vec!["-y".to_string()];
    args.extend(a_args);
    let mut graph = a_filter;
    if let Some(x) = &r.xfade {
        let Some(next) = inputs.get(1) else {
            return Err(HttpError::new(500, "transition piece needs the next slide"));
        };
        let (b_args, b_filter) = slide_branch(p, &p.slides[i + 1], next, 1, 0, x.frames, "b")?;
        args.extend(b_args);
        let dur = x.frames as f64 / p.fps;
        let off = (r.frames - x.frames) as f64 / p.fps;
        graph.push_str(&format!(
            ";{b_filter};[a][b]xfade=transition={}:duration={}:offset={}[v]",
            x.transition,
            f(dur, 6),
            f(off, 6)
        ));
    }
    args.push("-filter_complex".into());
    args.push(graph);
    for s in [
        "-map",
        "[v]",
        "-frames:v",
        &r.frames.to_string(),
        "-an",
        "-sn",
        "-dn",
        "-fps_mode",
        "passthrough",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "yuv420p",
        "pipe:1",
    ] {
        args.push(s.to_string());
    }
    Ok((args, r.frames))
}

/// bytes of one yuv420p frame
pub fn frame_bytes(width: f64, height: f64) -> u64 {
    (width * height * 3.0 / 2.0) as u64
}

/// The final encoder: raw frames on stdin (+ optional looped music) → MP4.
pub fn build_encoder_args(p: &SlideshowPlan, output: &str, music: Option<&str>) -> Vec<String> {
    let d = p.duration;
    let mut args: Vec<String> = [
        "-y",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "yuv420p",
        "-video_size",
        &format!("{}x{}", jnum(p.width), jnum(p.height)),
        "-framerate",
        &jnum(p.fps),
        "-i",
        "pipe:0",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let push = |a: &mut Vec<String>, v: &[&str]| a.extend(v.iter().map(|s| s.to_string()));
    if let Some(m) = music {
        let fade = d.min(2.0);
        push(&mut args, &["-stream_loop", "-1", "-i", m]);
        args.push("-filter_complex".into());
        args.push(format!(
            "[1:a:0]atrim=end={},asetpts=PTS-STARTPTS,afade=t=out:st={}:d={}[a]",
            f(d, 6),
            f((d - fade).max(0.0), 6),
            f(fade, 6)
        ));
        push(&mut args, &["-map", "0:v", "-map", "[a]"]);
    } else {
        push(&mut args, &["-map", "0:v"]);
    }
    push(
        &mut args,
        &[
            "-c:v",
            "libx264",
            "-preset",
            "medium",
            "-crf",
            &SLIDESHOW_CRF.to_string(),
            "-pix_fmt",
            "yuv420p",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
            "-color_range",
            "tv",
        ],
    );
    if music.is_some() {
        push(&mut args, &["-c:a", "aac", "-b:a", "192k"]);
    }
    push(
        &mut args,
        &[
            "-t",
            &f(d, 6),
            "-max_muxing_queue_size",
            "4096",
            "-movflags",
            "+faststart",
            "-f",
            "mp4",
            output,
        ],
    );
    args
}

/// (Y, U, V) of a colour in BT.709 limited range.
pub fn solid_yuv(hex: &str) -> Result<(u8, u8, u8), HttpError> {
    let h = normalize_hex(Some(&Value::String(hex.to_string())))?;
    let c = |i: usize| u8::from_str_radix(&h[1 + i..3 + i], 16).unwrap_or(0) as f64 / 255.0;
    let (r, g, b) = (c(0), c(2), c(4));
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let clamp = |v: f64| round(v).clamp(0.0, 255.0) as u8;
    Ok((
        clamp(16.0 + 219.0 * y),
        clamp(128.0 + (224.0 * (b - y)) / 1.8556),
        clamp(128.0 + (224.0 * (r - y)) / 1.5748),
    ))
}

/// One solid frame (BT.709 limited range yuv420p) used to pad pieces that came up short.
pub fn solid_frame(width: u32, height: u32, hex: &str) -> Result<Vec<u8>, HttpError> {
    let (y, u, v) = solid_yuv(hex)?;
    let luma = (width * height) as usize;
    let chroma = luma / 4;
    let mut buf = vec![y; luma + 2 * chroma];
    buf[luma..luma + chroma].fill(u);
    buf[luma + chroma..].fill(v);
    Ok(buf)
}
