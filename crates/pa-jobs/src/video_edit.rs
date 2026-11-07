//! Pure planning + ffmpeg argument building for the video editor (mirrors `jobs/video-edit.ts`).
//! No I/O here — the runner probes the source, resolves paths and runs what this module produces.
//!
//! Filter order: trim (input seek) → crop → rotate → flip → scale → speed → colour (eq, hue,
//! colortemperature) → vignette → overlays → output format. Only filters of ffmpeg 5.1 are used.

use crate::error::HttpError;
use crate::js::{f, round};
use crate::spec::{in_range, is_object, js_string, nullish, num, prop, truthy, StrNum};
use pa_core::{VideoFormat, VideoQuality};
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::LazyLock;

/// What the runner knows about the source.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoSourceInfo {
    /// displayed size (already swapped for 90/270° rotated sources)
    pub width: f64,
    pub height: f64,
    pub duration: f64,
    pub has_audio: bool,
    pub fps: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Trim {
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Crop {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Normalized colour adjustments (every field set).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Default)]
pub struct Adjust {
    pub brightness: f64,
    pub contrast: f64,
    pub saturation: f64,
    pub hue: f64,
    pub warmth: f64,
    pub vignette: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Overlay {
    pub asset_id: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Output {
    pub format: VideoFormat,
    pub height: Option<f64>,
    pub quality: VideoQuality,
}

/// `NormalizedVideoEdit`
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedVideoEdit {
    pub source_id: String,
    pub trim: Option<Trim>,
    pub crop: Option<Crop>,
    /// 0 | 90 | 180 | 270
    pub rotate: u16,
    pub flip_h: bool,
    pub flip_v: bool,
    pub speed: f64,
    pub volume: f64,
    pub mute: bool,
    pub adjust: Adjust,
    pub overlays: Vec<Overlay>,
    pub output: Output,
}

/// `VideoEditPlan`
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoEditPlan {
    pub source_id: String,
    pub crop: Option<Crop>,
    pub rotate: u16,
    pub flip_h: bool,
    pub flip_v: bool,
    pub speed: f64,
    pub volume: f64,
    pub mute: bool,
    pub adjust: Adjust,
    pub overlays: Vec<Overlay>,
    pub output: Output,
    /// trim in source seconds (always set)
    pub start: f64,
    pub end: f64,
    /// true when the input has to be seeked / limited
    pub trimmed: bool,
    /// size after crop + rotate (before output scaling)
    pub base_width: f64,
    pub base_height: f64,
    /// final output size (even)
    pub width: f64,
    pub height: f64,
    /// expected output duration in seconds
    pub duration: f64,
    pub audio: bool,
    /// output frame rate (gif ≤ 15; never above the source rate after a speed change), None = keep
    pub fps: Option<f64>,
    pub ext: VideoFormat,
}

pub const MAX_OVERLAYS: usize = 48;

pub fn crf_x264(q: VideoQuality) -> u32 {
    match q {
        VideoQuality::High => 18,
        VideoQuality::Medium => 23,
        VideoQuality::Low => 28,
    }
}
pub fn crf_vp9(q: VideoQuality) -> u32 {
    match q {
        VideoQuality::High => 24,
        VideoQuality::Medium => 32,
        VideoQuality::Low => 40,
    }
}
pub fn crf_vp8(q: VideoQuality) -> u32 {
    match q {
        VideoQuality::High => 6,
        VideoQuality::Medium => 10,
        VideoQuality::Low => 20,
    }
}
pub fn gif_fps(q: VideoQuality) -> f64 {
    match q {
        VideoQuality::High => 15.0,
        VideoQuality::Medium => 12.0,
        VideoQuality::Low => 10.0,
    }
}
fn gif_colors(q: VideoQuality) -> u32 {
    match q {
        VideoQuality::Low => 128,
        _ => 256,
    }
}
fn gif_dither(q: VideoQuality) -> &'static str {
    match q {
        VideoQuality::High => "sierra2_4a",
        VideoQuality::Medium => "bayer:bayer_scale=3",
        VideoQuality::Low => "bayer:bayer_scale=4",
    }
}
fn audio_bitrate(q: VideoQuality) -> &'static str {
    match q {
        VideoQuality::High => "192k",
        VideoQuality::Medium => "160k",
        VideoQuality::Low => "128k",
    }
}

pub fn ext_of(fmt: VideoFormat) -> &'static str {
    match fmt {
        VideoFormat::Mp4 => "mp4",
        VideoFormat::Webm => "webm",
        VideoFormat::Gif => "gif",
    }
}

fn bad(m: impl Into<String>) -> HttpError {
    HttpError::bad(m)
}

fn n(v: Option<&Value>, name: &str, def: Option<f64>) -> Result<f64, HttpError> {
    num(v, name, def, StrNum::NonBlank)
}

/// Shape / range validation of a client spec (any JSON). Errors are 400s with the TS messages.
pub fn normalize_video_edit_spec(spec: &Value) -> Result<NormalizedVideoEdit, HttpError> {
    static ID_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+$").unwrap());
    static OVERLAY_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^[0-9a-f-]{36}\.(png|webp)$").unwrap());
    let s = Some(spec);
    if !truthy(s) || !is_object(s) {
        return Err(bad("Invalid video edit spec"));
    }
    let source_id = match prop(s, "sourceId") {
        Some(v @ (Value::String(_) | Value::Number(_))) => js_string(v).unwrap_or_default(),
        _ => return Err(bad("sourceId is required")),
    };
    if !ID_RE.is_match(&source_id) {
        return Err(bad("Invalid sourceId"));
    }

    let mut trim = None;
    let t = prop(s, "trim");
    if truthy(t) {
        let start = n(prop(t, "start"), "trim.start", Some(0.0))?;
        let end = n(prop(t, "end"), "trim.end", None)?;
        if start < 0.0 {
            return Err(bad("trim.start must be ≥ 0"));
        }
        if end - start < 0.05 {
            return Err(bad("trim.end must be after trim.start"));
        }
        trim = Some(Trim { start, end });
    }

    let mut crop = None;
    let c = prop(s, "crop");
    if truthy(c) {
        let x = round(n(prop(c, "x"), "crop.x", Some(0.0))?);
        let y = round(n(prop(c, "y"), "crop.y", Some(0.0))?);
        let w = round(n(prop(c, "w"), "crop.w", None)?);
        let h = round(n(prop(c, "h"), "crop.h", None)?);
        if x < 0.0 || y < 0.0 {
            return Err(bad("crop.x / crop.y must be ≥ 0"));
        }
        if w < 2.0 || h < 2.0 {
            return Err(bad("crop is too small"));
        }
        crop = Some(Crop { x, y, w, h });
    }

    let rotate = n(prop(s, "rotate"), "rotate", Some(0.0))?;
    if ![0.0, 90.0, 180.0, 270.0].contains(&rotate) {
        return Err(bad("rotate must be 0, 90, 180 or 270"));
    }

    let speed = in_range(n(prop(s, "speed"), "speed", Some(1.0))?, 0.25, 4.0, "speed")?;
    let volume = in_range(
        n(prop(s, "volume"), "volume", Some(1.0))?,
        0.0,
        2.0,
        "volume",
    )?;

    let a_raw = prop(s, "adjust");
    let empty = Value::Object(Default::default());
    let a = if nullish(a_raw) { Some(&empty) } else { a_raw };
    if !is_object(a) {
        return Err(bad("Invalid adjust"));
    }
    let adj = |k: &str, lo: f64, hi: f64| -> Result<f64, HttpError> {
        let name = format!("adjust.{k}");
        in_range(n(prop(a, k), &name, Some(0.0))?, lo, hi, &name)
    };
    let adjust = Adjust {
        brightness: adj("brightness", -1.0, 1.0)?,
        contrast: adj("contrast", -1.0, 1.0)?,
        saturation: adj("saturation", -1.0, 1.0)?,
        hue: adj("hue", -180.0, 180.0)?,
        warmth: adj("warmth", -1.0, 1.0)?,
        vignette: adj("vignette", 0.0, 1.0)?,
    };

    let mut overlays = Vec::new();
    let ov = prop(s, "overlays");
    if !nullish(ov) {
        let Some(Value::Array(list)) = ov else {
            return Err(bad("overlays must be an array"));
        };
        if list.len() > MAX_OVERLAYS {
            return Err(bad(format!(
                "At most {MAX_OVERLAYS} overlays are supported"
            )));
        }
        for o in list {
            let o = Some(o);
            let Some(Value::String(asset_id)) = prop(o, "assetId").filter(|_| is_object(o)) else {
                return Err(bad("Invalid overlay"));
            };
            if !OVERLAY_RE.is_match(asset_id) {
                return Err(bad("Overlay assets must be PNG uploads"));
            }
            let start = n(prop(o, "start"), "overlay.start", Some(0.0))?.max(0.0);
            let end = n(prop(o, "end"), "overlay.end", None)?;
            if end <= start {
                continue; // never visible
            }
            overlays.push(Overlay {
                asset_id: asset_id.clone(),
                start,
                end,
            });
        }
    }

    let out = prop(s, "output");
    let format = match prop(out, "format") {
        None | Some(Value::Null) => VideoFormat::Mp4,
        Some(Value::String(x)) if x == "mp4" => VideoFormat::Mp4,
        Some(Value::String(x)) if x == "webm" => VideoFormat::Webm,
        Some(Value::String(x)) if x == "gif" => VideoFormat::Gif,
        _ => return Err(bad("output.format must be mp4, webm or gif")),
    };
    let quality = match prop(out, "quality") {
        None | Some(Value::Null) => VideoQuality::Medium,
        Some(Value::String(x)) if x == "high" => VideoQuality::High,
        Some(Value::String(x)) if x == "medium" => VideoQuality::Medium,
        Some(Value::String(x)) if x == "low" => VideoQuality::Low,
        _ => return Err(bad("output.quality must be high, medium or low")),
    };
    let h = prop(out, "height");
    let height = if nullish(h) {
        None
    } else {
        Some(round(in_range(
            n(h, "output.height", None)?,
            16.0,
            4320.0,
            "output.height",
        )?))
    };

    Ok(NormalizedVideoEdit {
        source_id,
        trim,
        crop,
        rotate: rotate as u16,
        flip_h: truthy(prop(s, "flipH")),
        flip_v: truthy(prop(s, "flipV")),
        speed,
        volume,
        mute: truthy(prop(s, "mute")),
        adjust,
        overlays,
        output: Output {
            format,
            height,
            quality,
        },
    })
}

fn even(n: f64) -> f64 {
    (round(n / 2.0) * 2.0).max(2.0)
}
fn even_floor(n: f64) -> f64 {
    ((n / 2.0).floor() * 2.0).max(2.0)
}

/// Resolve the spec against the probed source (400 for out-of-bounds values).
pub fn plan_video_edit(
    n: &NormalizedVideoEdit,
    src: &VideoSourceInfo,
) -> Result<VideoEditPlan, HttpError> {
    if !(src.width > 0.0 && src.height > 0.0) {
        return Err(bad("The video has no readable video stream"));
    }
    if !(src.duration > 0.0) {
        return Err(bad("Unknown video duration"));
    }
    let mut start = 0.0;
    let mut end = src.duration;
    if let Some(t) = n.trim {
        if t.start >= src.duration - 0.05 {
            return Err(bad("trim.start is beyond the end of the video"));
        }
        start = t.start;
        end = t.end.min(src.duration);
        if end - start < 0.05 {
            return Err(bad("Trimmed clip is too short"));
        }
    }
    let trimmed = start > 0.0005 || end < src.duration - 0.0005;

    let mut crop = n.crop;
    if let Some(c) = crop {
        if c.x >= src.width - 1.0 || c.y >= src.height - 1.0 {
            return Err(bad("crop is outside the video frame"));
        }
        let w = c.w.min(src.width - c.x);
        let h = c.h.min(src.height - c.y);
        if w < 2.0 || h < 2.0 {
            return Err(bad("crop is too small"));
        }
        crop = Some(Crop { w, h, ..c });
        if c.x == 0.0 && c.y == 0.0 && w == src.width && h == src.height {
            crop = None;
        }
    }

    let (mut base_width, mut base_height) = match crop {
        Some(c) => (c.w, c.h),
        None => (src.width, src.height),
    };
    if n.rotate == 90 || n.rotate == 270 {
        std::mem::swap(&mut base_width, &mut base_height);
    }

    let (width, height) = match n.output.height {
        Some(h) if h != 0.0 => {
            let height = even(h);
            (even(base_width * height / base_height), height)
        }
        _ => (even_floor(base_width), even_floor(base_height)),
    };
    if width > 8192.0 || height > 8192.0 {
        return Err(bad("Output resolution is too large"));
    }

    let format = n.output.format;
    let duration = (end - start) / n.speed;
    let overlays: Vec<Overlay> = n
        .overlays
        .iter()
        .filter(|o| o.start < duration)
        .cloned()
        .collect();

    let mut fps = None;
    let src_fps = src.fps.filter(|&x| x > 0.0 && x < 1000.0);
    if let Some(sf) = src_fps {
        if n.speed != 1.0 {
            fps = Some((round(sf.min(sf * n.speed) * 1000.0) / 1000.0).max(1.0));
        }
    }
    if format == VideoFormat::Gif {
        fps = Some(gif_fps(n.output.quality).min(fps.or(src_fps).unwrap_or(f64::INFINITY)));
    }

    Ok(VideoEditPlan {
        source_id: n.source_id.clone(),
        crop,
        rotate: n.rotate,
        flip_h: n.flip_h,
        flip_v: n.flip_v,
        speed: n.speed,
        volume: n.volume,
        mute: n.mute,
        adjust: n.adjust,
        overlays,
        output: n.output,
        start,
        end,
        trimmed,
        base_width,
        base_height,
        width,
        height,
        duration,
        audio: src.has_audio && !n.mute && format != VideoFormat::Gif,
        fps,
        ext: format,
    })
}

/// Split a speed factor into atempo steps that each stay within 0.5..2.
pub fn atempo_chain(speed: f64) -> Vec<f64> {
    let mut steps = Vec::new();
    let mut s = speed;
    while s > 2.0 + 1e-9 {
        steps.push(2.0);
        s /= 2.0;
    }
    while s < 0.5 - 1e-9 {
        steps.push(0.5);
        s /= 0.5;
    }
    if (s - 1.0).abs() > 1e-6 {
        steps.push(s);
    }
    steps
}

/// Colour adjust mapping (client values → ffmpeg): eq brightness 0.3·b, contrast / saturation
/// 1 + x, hue degrees, colortemperature 6500 − 2500·w (lightness preserved), vignette v·π/2.5.
pub fn color_filters(a: &Adjust) -> Vec<String> {
    let mut out = Vec::new();
    let mut eq = Vec::new();
    if a.brightness.abs() > 1e-4 {
        eq.push(format!("brightness={}", f(a.brightness * 0.3, 4)));
    }
    if a.contrast.abs() > 1e-4 {
        eq.push(format!("contrast={}", f(1.0 + a.contrast, 4)));
    }
    if a.saturation.abs() > 1e-4 {
        eq.push(format!("saturation={}", f(1.0 + a.saturation, 4)));
    }
    if !eq.is_empty() {
        out.push(format!("eq={}", eq.join(":")));
    }
    if a.hue.abs() > 0.05 {
        out.push(format!("hue=h={}", f(a.hue, 2)));
    }
    if a.warmth.abs() > 1e-3 {
        out.push(format!(
            "colortemperature=temperature={}:pl=1",
            crate::js::num(round(6500.0 - 2500.0 * a.warmth))
        ));
    }
    if a.vignette > 1e-3 {
        out.push(format!(
            "vignette=angle={}",
            f(a.vignette * std::f64::consts::PI / 2.5, 4)
        ));
    }
    out
}

/// The geometric + timing part of the video chain (shared by every output format).
pub fn geometry_filters(p: &VideoEditPlan) -> Vec<String> {
    let num = crate::js::num;
    let mut out = Vec::new();
    if let Some(c) = p.crop {
        out.push(format!(
            "crop={}:{}:{}:{}:exact=1",
            num(c.w),
            num(c.h),
            num(c.x),
            num(c.y)
        ));
    }
    match p.rotate {
        90 => out.push("transpose=clock".into()),
        270 => out.push("transpose=cclock".into()),
        180 => {
            out.push("hflip".into());
            out.push("vflip".into());
        }
        _ => {}
    }
    if p.flip_h {
        out.push("hflip".into());
    }
    if p.flip_v {
        out.push("vflip".into());
    }
    if p.width != p.base_width || p.height != p.base_height {
        out.push(format!(
            "scale={}:{}:flags=bicubic",
            num(p.width),
            num(p.height)
        ));
    }
    out.push("setsar=1".into());
    out.push(if p.speed == 1.0 {
        "setpts=PTS-STARTPTS".into()
    } else {
        format!("setpts=(PTS-STARTPTS)/{}", f(p.speed, 6))
    });
    if let Some(fps) = p.fps {
        out.push(format!("fps={}", f(fps, 3)));
    }
    out
}

/// The video filter graph up to (and including) the overlays; input 0 is the source, inputs
/// 1..n the overlay PNGs. Ends in `[vo]`.
pub fn video_graph(p: &VideoEditPlan) -> String {
    let num = crate::js::num;
    let mut chain = geometry_filters(p);
    chain.extend(color_filters(&p.adjust));
    let mut parts = vec![format!("[0:V:0]{}[vb]", chain.join(","))];
    let mut cur = "vb".to_string();
    for (i, o) in p.overlays.iter().enumerate() {
        let next = format!("v{i}");
        parts.push(format!(
            "[{}:v]scale={}:{},format=rgba[ov{i}]",
            i + 1,
            num(p.width),
            num(p.height)
        ));
        parts.push(format!(
            "[{cur}][ov{i}]overlay=0:0:enable='between(t,{},{})'[{next}]",
            f(o.start, 3),
            f(o.end, 3)
        ));
        cur = next;
    }
    parts.push(format!("[{cur}]null[vo]"));
    parts.join(";")
}

pub fn audio_graph(p: &VideoEditPlan) -> Option<String> {
    if !p.audio {
        return None;
    }
    let mut chain = vec!["asetpts=PTS-STARTPTS".to_string()];
    chain.extend(
        atempo_chain(p.speed)
            .into_iter()
            .map(|s| format!("atempo={}", f(s, 6))),
    );
    if (p.volume - 1.0).abs() > 1e-4 {
        chain.push(format!("volume={}", f(p.volume, 4)));
    }
    Some(format!("[0:a:0]{}[ao]", chain.join(",")))
}

/// Paths of one export.
#[derive(Debug, Clone)]
pub struct VideoEditIo {
    pub input: String,
    pub overlays: Vec<String>,
    pub output: String,
    /// gif only: temp palette file
    pub palette: Option<String>,
    /// encoder names available in this ffmpeg build
    pub encoders: HashSet<String>,
}

/// One ffmpeg invocation of a job.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FfmpegPass {
    pub args: Vec<String>,
    /// share of the job's total progress
    pub weight: f64,
}

fn input_args(p: &VideoEditPlan, io: &VideoEditIo) -> Vec<String> {
    let mut a = Vec::new();
    if p.trimmed {
        a.extend([
            "-ss".into(),
            f(p.start, 3),
            "-t".into(),
            f(p.end - p.start, 3),
        ]);
    }
    a.push("-i".into());
    a.push(io.input.clone());
    for o in &io.overlays {
        a.push("-i".into());
        a.push(o.clone());
    }
    a
}

/// Encoders of a format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Codecs {
    pub video: &'static str,
    pub audio: Option<&'static str>,
}

/// Choose the encoders for a format, or 400 when this ffmpeg can't produce it.
pub fn pick_codecs(format: VideoFormat, encoders: &HashSet<String>) -> Result<Codecs, HttpError> {
    match format {
        VideoFormat::Gif => Ok(Codecs {
            video: "gif",
            audio: None,
        }),
        VideoFormat::Mp4 => {
            if !encoders.contains("libx264") {
                return Err(bad("This ffmpeg build has no H.264 encoder (libx264)"));
            }
            Ok(Codecs {
                video: "libx264",
                audio: Some("aac"),
            })
        }
        VideoFormat::Webm => {
            let video = if encoders.contains("libvpx-vp9") {
                "libvpx-vp9"
            } else if encoders.contains("libvpx") {
                "libvpx"
            } else {
                return Err(bad("This ffmpeg build cannot encode WebM (libvpx missing)"));
            };
            let audio = if encoders.contains("libopus") {
                Some("libopus")
            } else if encoders.contains("libvorbis") {
                Some("libvorbis")
            } else {
                None
            };
            Ok(Codecs { video, audio })
        }
    }
}

fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// All ffmpeg invocations for a plan (gif = palette pass + encode pass).
pub fn build_video_edit_passes(
    p: &VideoEditPlan,
    io: &VideoEditIo,
) -> Result<Vec<FfmpegPass>, HttpError> {
    if io.overlays.len() != p.overlays.len() {
        return Err(HttpError::new(500, "overlay paths do not match the plan"));
    }
    let codecs = pick_codecs(p.ext, &io.encoders)?;
    let q = p.output.quality;
    let vgraph = video_graph(p);

    if p.ext == VideoFormat::Gif {
        let Some(palette) = io.palette.clone() else {
            return Err(HttpError::new(500, "palette path required for gif output"));
        };
        let pal_idx = 1 + io.overlays.len();
        let mut pal = vec!["-y".to_string()];
        pal.extend(input_args(p, io));
        pal.push("-filter_complex".into());
        pal.push(format!(
            "{vgraph};[vo]palettegen=stats_mode=diff:max_colors={}[pal]",
            gif_colors(q)
        ));
        pal.extend(strs(&[
            "-map",
            "[pal]",
            "-frames:v",
            "1",
            "-update",
            "1",
            "-f",
            "image2",
        ]));
        pal.push(palette.clone());
        let mut enc = vec!["-y".to_string()];
        enc.extend(input_args(p, io));
        enc.push("-i".into());
        enc.push(palette);
        enc.push("-filter_complex".into());
        enc.push(format!(
            "{vgraph};[vo][{pal_idx}:v]paletteuse=dither={}:diff_mode=rectangle[g]",
            gif_dither(q)
        ));
        enc.extend(strs(&[
            "-map",
            "[g]",
            "-an",
            "-map_chapters",
            "-1",
            "-c:v",
            "gif",
            "-loop",
            "0",
            "-f",
            "gif",
        ]));
        enc.push(io.output.clone());
        return Ok(vec![
            FfmpegPass {
                args: pal,
                weight: 0.35,
            },
            FfmpegPass {
                args: enc,
                weight: 0.65,
            },
        ]);
    }

    let agraph = if codecs.audio.is_some() {
        audio_graph(p)
    } else {
        None
    };
    let mut args = vec!["-y".to_string()];
    args.extend(input_args(p, io));
    args.push("-filter_complex".into());
    args.push(match &agraph {
        Some(a) => format!("{vgraph};{a}"),
        None => vgraph.clone(),
    });
    args.extend(strs(&["-map", "[vo]"]));
    if agraph.is_some() {
        args.extend(strs(&["-map", "[ao]"]));
    } else {
        args.push("-an".into());
    }
    args.extend(strs(&["-map_chapters", "-1", "-sn", "-dn"]));
    match codecs.video {
        "libx264" => {
            args.extend(strs(&["-c:v", "libx264", "-preset", "medium", "-crf"]));
            args.push(crf_x264(q).to_string());
            args.extend(strs(&["-pix_fmt", "yuv420p"]));
        }
        "libvpx-vp9" => {
            args.extend(strs(&["-c:v", "libvpx-vp9", "-crf"]));
            args.push(crf_vp9(q).to_string());
            args.extend(strs(&[
                "-b:v",
                "0",
                "-deadline",
                "good",
                "-cpu-used",
                "4",
                "-row-mt",
                "1",
                "-pix_fmt",
                "yuv420p",
            ]));
        }
        _ => {
            args.extend(strs(&["-c:v", "libvpx", "-crf"]));
            args.push(crf_vp8(q).to_string());
            args.extend(strs(&[
                "-b:v",
                "8M",
                "-deadline",
                "good",
                "-cpu-used",
                "4",
                "-pix_fmt",
                "yuv420p",
            ]));
        }
    }
    if let (Some(_), Some(a)) = (&agraph, codecs.audio) {
        args.extend(strs(&["-c:a", a, "-b:a", audio_bitrate(q)]));
    }
    args.extend(strs(&["-max_muxing_queue_size", "4096"]));
    if p.ext == VideoFormat::Mp4 {
        args.extend(strs(&["-movflags", "+faststart", "-f", "mp4"]));
    } else {
        args.extend(strs(&["-f", "webm"]));
    }
    args.push(io.output.clone());
    Ok(vec![FfmpegPass { args, weight: 1.0 }])
}
