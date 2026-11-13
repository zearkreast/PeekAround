//! Video / audio playback in the browser: a `<video>` (or `<audio>`) element of the page decodes
//! and plays, sound included; each picture is copied from it into the Slint scene, so the player
//! looks and behaves like the native one (`video.rs`, where ffmpeg decodes).
//!
//! - files the browser can play come straight from the server (ranged requests, the element
//!   seeks by itself);
//! - the others (`Playback::Transcode`: mkv, hevc, avi …) come from the transcoding stream of
//!   the server, which cannot seek: a seek asks for a new stream starting there.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{Rgba8Pixel, SharedPixelBuffer};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

pub use crate::video_common::*;

type Canvas = (web_sys::HtmlCanvasElement, web_sys::CanvasRenderingContext2d);

/// `HTMLMediaElement.HAVE_CURRENT_DATA`: the picture of the current position is there
const HAVE_CURRENT_DATA: u16 = 2;
/// `HTMLMediaElement.HAVE_FUTURE_DATA`: enough to keep playing
const HAVE_FUTURE_DATA: u16 = 3;

/// What the ffmpeg filters of the video editor preview (`model::preview_filters`) mean for a
/// canvas: the crop is the source rectangle, rotation and flips a transform, the colours a CSS
/// filter and two overlays. Close to the export, not identical to it.
#[derive(Debug, Clone, PartialEq)]
struct Look {
    /// source rectangle in video pixels (x, y, w, h)
    crop: Option<(f64, f64, f64, f64)>,
    /// unit square of the picture → unit square of the output: p' = [a b; c d] p + (e, f)
    turn: [f64; 6],
    /// `filter` of the canvas: brightness, contrast, saturation, hue
    css: String,
    /// colour temperature in kelvin (6500: neutral)
    kelvin: f64,
    /// angle of the ffmpeg vignette (0: none)
    vignette: f64,
}

impl Default for Look {
    fn default() -> Look {
        Look {
            crop: None,
            turn: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            css: String::new(),
            kelvin: 6500.0,
            vignette: 0.0,
        }
    }
}

/// `key=value` inside a `:` separated filter argument list
fn arg(args: &str, key: &str) -> Option<f64> {
    args.split(':')
        .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('=')?.parse().ok())
}

impl Look {
    fn of(pre: &[String], post: &[String]) -> Look {
        let mut l = Look::default();
        // one more step after the current map: p'' = [a b; c d] p' + (e, f)
        let then = |l: &mut Look, s: [f64; 6]| {
            let t = l.turn;
            l.turn = [
                s[0] * t[0] + s[1] * t[2],
                s[0] * t[1] + s[1] * t[3],
                s[2] * t[0] + s[3] * t[2],
                s[2] * t[1] + s[3] * t[3],
                s[0] * t[4] + s[1] * t[5] + s[4],
                s[2] * t[4] + s[3] * t[5] + s[5],
            ];
        };
        for f in pre {
            let (name, args) = f.split_once('=').unwrap_or((f, ""));
            match (name, args) {
                ("crop", a) => {
                    let n: Vec<f64> = a.split(':').filter_map(|v| v.parse().ok()).collect();
                    if let [w, h, x, y] = n[..] {
                        l.crop = Some((x, y, w, h));
                    }
                }
                ("transpose", "clock") => then(&mut l, [0.0, -1.0, 1.0, 0.0, 1.0, 0.0]),
                ("transpose", "cclock") => then(&mut l, [0.0, 1.0, -1.0, 0.0, 0.0, 1.0]),
                ("hflip", _) => then(&mut l, [-1.0, 0.0, 0.0, 1.0, 1.0, 0.0]),
                ("vflip", _) => then(&mut l, [1.0, 0.0, 0.0, -1.0, 0.0, 1.0]),
                _ => {}
            }
        }
        let mut css = Vec::new();
        for f in post {
            let (name, args) = f.split_once('=').unwrap_or((f, ""));
            match name {
                "eq" => {
                    // ffmpeg adds the brightness, CSS multiplies: about the same around mid grey
                    if let Some(b) = arg(args, "brightness") {
                        css.push(format!("brightness({:.3})", (1.0 + 2.0 * b).max(0.0)));
                    }
                    if let Some(c) = arg(args, "contrast") {
                        css.push(format!("contrast({c:.3})"));
                    }
                    if let Some(s) = arg(args, "saturation") {
                        css.push(format!("saturate({s:.3})"));
                    }
                }
                "hue" => {
                    if let Some(h) = arg(args, "h") {
                        css.push(format!("hue-rotate({h:.2}deg)"));
                    }
                }
                "colortemperature" => l.kelvin = arg(args, "temperature").unwrap_or(6500.0),
                "vignette" => l.vignette = arg(args, "angle").unwrap_or(0.0),
                _ => {}
            }
        }
        l.css = css.join(" ");
        l
    }

    /// Draw the current picture of `v` over the whole `w` × `h` canvas.
    fn draw(&self, ctx: &web_sys::CanvasRenderingContext2d, v: &web_sys::HtmlVideoElement, w: f64, h: f64) -> Option<()> {
        let t = self.turn;
        let (sx, sy, sw, sh) = self
            .crop
            .unwrap_or((0.0, 0.0, v.video_width() as f64, v.video_height() as f64));
        ctx.set_filter(if self.css.is_empty() { "none" } else { &self.css });
        ctx.set_transform(w * t[0], h * t[2], w * t[1], h * t[3], w * t[4], h * t[5]).ok()?;
        let drawn = ctx.draw_image_with_html_video_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
            v, sx, sy, sw, sh, 0.0, 0.0, 1.0, 1.0,
        );
        ctx.set_filter("none");
        ctx.set_transform(1.0, 0.0, 0.0, 1.0, 0.0, 0.0).ok()?;
        drawn.ok()?;
        let warm = (6500.0 - self.kelvin) / 2500.0;
        if warm.abs() > 1e-3 {
            ctx.set_global_composite_operation("soft-light").ok()?;
            ctx.set_global_alpha(warm.abs().min(1.0) * 0.35);
            ctx.set_fill_style_str(if warm > 0.0 { "#ff9a2e" } else { "#3d8bff" });
            ctx.fill_rect(0.0, 0.0, w, h);
            ctx.set_global_alpha(1.0);
            ctx.set_global_composite_operation("source-over").ok()?;
        }
        if self.vignette > 1e-3 {
            // ffmpeg darkens by cos(angle · distance / half diagonal)⁴
            let r = (w * w + h * h).sqrt() / 2.0;
            let g = ctx.create_radial_gradient(w / 2.0, h / 2.0, 0.0, w / 2.0, h / 2.0, r).ok()?;
            for i in 0..=8 {
                let d = i as f64 / 8.0;
                let keep = (self.vignette * d).cos().max(0.0).powi(4);
                g.add_color_stop(d as f32, &format!("rgba(0,0,0,{:.3})", 1.0 - keep)).ok()?;
            }
            ctx.set_fill_style_canvas_gradient(&g);
            ctx.fill_rect(0.0, 0.0, w, h);
        }
        Some(())
    }
}

/// One playing item. Dropping it stops the element and its download.
pub struct Player {
    src: Source,
    el: web_sys::HtmlMediaElement,
    video: Option<web_sys::HtmlVideoElement>,
    canvas: Option<Canvas>,
    /// a transcoding stream of the server: no seeking inside it
    stream: bool,
    /// media time at which the clock of the element starts (a stream starts where it was asked to)
    base: f64,
    /// media time of the last picture handed out (NaN: none yet)
    last: f64,
    look: Look,
    error: Rc<RefCell<Option<String>>>,
    _on_error: Closure<dyn FnMut()>,
}

/// Is `url` the transcoding stream (`/api/media/:id/stream`)?
fn is_stream(url: &str) -> bool {
    url.split('?').next().is_some_and(|p| p.ends_with("/stream"))
}

/// The stream URL starting at `t` seconds.
fn stream_at(url: &str, t: f64) -> String {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let mut q: Vec<String> = query
        .split('&')
        .filter(|p| !p.is_empty() && !p.starts_with("t="))
        .map(String::from)
        .collect();
    if t > 0.0 {
        q.push(format!("t={t:.2}"));
    }
    if q.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{}", q.join("&"))
    }
}

/// `play()` answers with a promise that fails when the browser refuses (autoplay with sound
/// before any click): the element then stays paused, which the status reports.
fn play(el: &web_sys::HtmlMediaElement) {
    if let Ok(p) = el.play() {
        wasm_bindgen_futures::spawn_local(async move {
            let _ = wasm_bindgen_futures::JsFuture::from(p).await;
        });
    }
}

impl Player {
    /// Start playing `src` at `start` seconds (`playing`: start right away or paused on the
    /// first frame).
    pub fn new(
        src: Source,
        start: f64,
        playing: bool,
        volume: f64,
        muted: bool,
        speed: f64,
    ) -> Player {
        let error = Rc::new(RefCell::new(None));
        let look = Look::of(&src.vf_pre, &src.vf_post);
        let doc = web_sys::window().and_then(|w| w.document());
        let made = doc
            .as_ref()
            .and_then(|d| d.create_element(if src.video { "video" } else { "audio" }).ok());
        let Some(el) = made.and_then(|e| e.dyn_into::<web_sys::HtmlMediaElement>().ok()) else {
            // no document: a player that reports the error and never has a picture
            *error.borrow_mut() = Some("no document".to_string());
            let el: web_sys::HtmlMediaElement = JsValue::NULL.unchecked_into();
            return Player {
                src,
                el,
                video: None,
                canvas: None,
                stream: false,
                base: 0.0,
                last: f64::NAN,
                look,
                error,
                _on_error: Closure::<dyn FnMut()>::new(|| {}),
            };
        };
        let video = el.clone().dyn_into::<web_sys::HtmlVideoElement>().ok();
        let _ = el.set_attribute("playsinline", "");
        // in the document (some browsers only decode attached elements) but never seen: the
        // pictures are drawn by the UI
        let _ = el.set_attribute(
            "style",
            "position:fixed;left:0;top:0;width:1px;height:1px;opacity:0;pointer-events:none",
        );
        if let Some(body) = doc.and_then(|d| d.body()) {
            let _ = body.append_child(&el);
        }
        let (e2, el2) = (error.clone(), el.clone());
        let on_error = Closure::<dyn FnMut()>::new(move || {
            let msg = el2
                .error()
                .map(|e| e.message())
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "this browser cannot play the file".to_string());
            *e2.borrow_mut() = Some(msg);
        });
        let _ = el.add_event_listener_with_callback("error", on_error.as_ref().unchecked_ref());
        el.set_preload("auto");
        el.set_volume(volume.clamp(0.0, 1.0));
        el.set_muted(muted);
        let stream = is_stream(&src.input);
        let start = start.max(0.0);
        if stream {
            el.set_src(&stream_at(&src.input, start));
        } else {
            el.set_src(&src.input);
            if start > 0.0 {
                el.set_current_time(start);
            }
        }
        // after the source: loading it resets the rate
        el.set_default_playback_rate(speed.clamp(0.25, 4.0));
        el.set_playback_rate(speed.clamp(0.25, 4.0));
        if playing {
            play(&el);
        }
        Player {
            src,
            el,
            video,
            canvas: None,
            stream,
            base: if stream { start } else { 0.0 },
            last: f64::NAN,
            look,
            error,
            _on_error: on_error,
        }
    }

    fn alive(&self) -> bool {
        !self.el.is_null()
    }

    /// Current media time.
    pub fn clock(&self) -> f64 {
        if !self.alive() {
            return 0.0;
        }
        self.base + self.el.current_time()
    }

    /// The picture to show now, when the element has a newer one than the last handed out.
    pub fn take_frame(&mut self) -> Option<Frame> {
        let v = self.video.as_ref()?;
        if v.ready_state() < HAVE_CURRENT_DATA || v.seeking() || v.video_width() == 0 {
            return None;
        }
        let t = self.base + v.current_time();
        // while playing, one copy per source frame is enough
        let step = if v.paused() {
            1e-4
        } else {
            0.5 / self.src.fps.max(1.0)
        };
        if self.last.is_finite() && (t - self.last).abs() < step {
            return None;
        }
        let (w, h) = (self.src.width.max(2), self.src.height.max(2));
        if self.canvas.is_none() {
            let doc = web_sys::window()?.document()?;
            let canvas = doc
                .create_element("canvas")
                .ok()?
                .dyn_into::<web_sys::HtmlCanvasElement>()
                .ok()?;
            let opts = js_sys::Object::new();
            let _ = js_sys::Reflect::set(&opts, &"willReadFrequently".into(), &JsValue::TRUE);
            let ctx = canvas
                .get_context_with_context_options("2d", &opts)
                .ok()??
                .dyn_into::<web_sys::CanvasRenderingContext2d>()
                .ok()?;
            self.canvas = Some((canvas, ctx));
        }
        let (canvas, ctx) = self.canvas.as_ref()?;
        if canvas.width() != w || canvas.height() != h {
            canvas.set_width(w);
            canvas.set_height(h);
        }
        self.look.draw(ctx, v, w as f64, h as f64)?;
        let data = ctx.get_image_data(0.0, 0.0, w as f64, h as f64).ok()?.data().0;
        self.last = t;
        Some(Frame {
            pts: t,
            px: SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&data, w, h),
        })
    }

    /// Status for the UI.
    pub fn status(&self) -> Status {
        if !self.alive() {
            return Status {
                error: self.error.borrow().clone(),
                ..Default::default()
            };
        }
        let el = &self.el;
        let ended = el.ended();
        let playing = !el.paused() && !ended;
        let now = el.current_time();
        let ranges = el.buffered();
        let mut buffered = now;
        for i in 0..ranges.length() {
            if let (Ok(a), Ok(b)) = (ranges.start(i), ranges.end(i)) {
                if a <= now + 0.1 && b >= now {
                    buffered = b;
                }
            }
        }
        let pos = (self.base + now).min(self.src.duration.unwrap_or(f64::MAX));
        Status {
            position: if ended {
                self.src.duration.unwrap_or(pos)
            } else {
                pos
            },
            playing,
            buffering: playing && (el.ready_state() < HAVE_FUTURE_DATA || el.seeking()),
            ended,
            error: self.error.borrow().clone(),
            no_device: false,
            buffered: self.base + buffered,
        }
    }

    /// play / pause
    pub fn set_playing(&mut self, on: bool) {
        if !self.alive() {
            return;
        }
        if on {
            play(&self.el);
        } else {
            let _ = self.el.pause();
        }
    }

    /// Seek (a transcoding stream starts again at the new position).
    pub fn seek(&mut self, t: f64) {
        if !self.alive() {
            return;
        }
        let t = match self.src.duration {
            Some(d) => t.clamp(0.0, (d - 0.05).max(0.0)),
            None => t.max(0.0),
        };
        self.last = f64::NAN;
        if self.stream {
            let playing = !self.el.paused();
            let rate = self.el.playback_rate();
            self.base = t;
            self.el.set_src(&stream_at(&self.src.input, t));
            self.el.set_playback_rate(rate);
            if playing {
                play(&self.el);
            }
        } else {
            self.el.set_current_time(t);
        }
    }

    /// Playback speed.
    pub fn set_speed(&mut self, s: f64) {
        if self.alive() {
            self.el.set_playback_rate(s.clamp(0.25, 4.0));
        }
    }

    /// volume 0..1
    pub fn set_volume(&self, v: f64) {
        if self.alive() {
            self.el.set_volume(v.clamp(0.0, 1.0));
        }
    }

    /// mute
    pub fn set_muted(&self, m: bool) {
        if self.alive() {
            self.el.set_muted(m);
        }
    }

    /// New frame size / filters (the next picture is drawn with them).
    pub fn set_filters(&mut self, width: u32, height: u32, vf_pre: Vec<String>, vf_post: Vec<String>) {
        self.look = Look::of(&vf_pre, &vf_post);
        self.src.width = width;
        self.src.height = height;
        self.src.vf_pre = vf_pre;
        self.src.vf_post = vf_post;
        self.last = f64::NAN;
    }

    /// what is playing
    pub fn source(&self) -> &Source {
        &self.src
    }

    /// The picture at the current position in the full size of the video, as JPEG.
    pub fn snapshot_jpeg(&self) -> Option<Vec<u8>> {
        use base64::Engine;
        let v = self.video.as_ref()?;
        let (w, h) = (v.video_width(), v.video_height());
        if w == 0 || h == 0 || v.ready_state() < HAVE_CURRENT_DATA {
            return None;
        }
        let canvas = web_sys::window()?
            .document()?
            .create_element("canvas")
            .ok()?
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .ok()?;
        canvas.set_width(w);
        canvas.set_height(h);
        let ctx = canvas
            .get_context("2d")
            .ok()??
            .dyn_into::<web_sys::CanvasRenderingContext2d>()
            .ok()?;
        ctx.draw_image_with_html_video_element(v, 0.0, 0.0).ok()?;
        let url = canvas
            .to_data_url_with_type_and_encoder_options("image/jpeg", &JsValue::from_f64(0.92))
            .ok()?;
        let b64 = url.split_once(";base64,")?.1;
        base64::engine::general_purpose::STANDARD.decode(b64).ok()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        if !self.alive() {
            return;
        }
        let _ = self.el.pause();
        let _ = self
            .el
            .remove_event_listener_with_callback("error", self._on_error.as_ref().unchecked_ref());
        // without a source the browser drops the download and the decoder
        let _ = self.el.remove_attribute("src");
        self.el.load();
        self.el.remove();
    }
}

/// Extracting a frame from a file is the job of ffmpeg on the native targets; the browser takes
/// it from the playing element ([`Player::snapshot_jpeg`]).
pub fn grab_frame(_input: &str, _t: f64, _out: &std::path::Path) -> Result<(), String> {
    Err("not available in the browser".into())
}
