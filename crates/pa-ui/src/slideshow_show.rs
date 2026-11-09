//! Slideshow engine (useShow.ts + ShowStage.tsx): one clock, auto-advance where slides overlap by
//! the transition (like the export), loop / end, and every frame of the stage laid out in px for
//! ui/slideshow/stage.slint. The builder's preview and the full-screen player each own one.
//!
//! Two clocks: the show clock only runs while playing (Ken Burns freezes on pause, as in 1.x);
//! transitions run on wall time from the moment the slide changed.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use pa_core::{MediaItem, MediaKind};
use slint::{Color, Image};

use crate::app::AppInner;
use crate::slideshow::{cut_length, kb_at, PlaySlide, ShowTransition};
use crate::video;
use crate::{SlideVis, StageFrame};

/// What the controls show.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ShowStatus {
    pub index: usize,
    pub count: usize,
    /// 0..1 of the current slide
    pub progress: f64,
    pub playing: bool,
    pub ended: bool,
    /// seconds left in the whole show (music fade-out)
    pub remaining: f64,
}

type FrameSink = Box<dyn Fn(StageFrame)>;
type StatusSink = Box<dyn Fn(&ShowStatus)>;

struct St {
    slides: Vec<PlaySlide>,
    td: f64,
    looping: bool,
    bg: Color,
    captions: bool,
    index: usize,
    dir: f64,
    playing: bool,
    ended: bool,
    clock: f64,
    start_at: f64,
    changed_at: Instant,
    /// when the current slide was entered (the picture wait is capped from here)
    entered_at: Instant,
    /// the outgoing slide while the cut runs: (index, its start on the show clock)
    prev: Option<(usize, f64)>,
    size: (f64, f64),
    scale: f64,
    images: HashMap<String, Image>,
    inflight: HashSet<String>,
    video: Option<(usize, video::Player)>,
    video_frame: Option<Image>,
    last: Instant,
    status: ShowStatus,
}

pub(crate) struct Show {
    app: Weak<AppInner>,
    me: Weak<Show>,
    st: RefCell<St>,
    timer: slint::Timer,
    thumb_size: u32,
    lang_date: bool,
    frame: FrameSink,
    status: StatusSink,
}

const EASE: (f64, f64, f64, f64) = (0.65, 0.0, 0.35, 1.0);
const EASE_OUT: (f64, f64, f64, f64) = (0.16, 1.0, 0.3, 1.0);
const EASE_CIRCLE: (f64, f64, f64, f64) = (0.5, 0.0, 0.2, 1.0);
const EASE_FLIP_IN: (f64, f64, f64, f64) = (0.22, 1.0, 0.36, 1.0);
const EASE_FLIP_OUT: (f64, f64, f64, f64) = (0.64, 0.0, 0.78, 0.0);

/// CSS cubic-bezier(x1, y1, x2, y2) at time `t` (Newton on x, then y).
pub(crate) fn bezier((x1, y1, x2, y2): (f64, f64, f64, f64), t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    let c = |a: f64, b: f64, s: f64| {
        let u = 1.0 - s;
        3.0 * u * u * s * a + 3.0 * u * s * s * b + s * s * s
    };
    let d = |a: f64, b: f64, s: f64| {
        let u = 1.0 - s;
        3.0 * u * u * a + 6.0 * u * s * (b - a) + 3.0 * s * s * (1.0 - b)
    };
    let mut s = t;
    for _ in 0..8 {
        let dx = d(x1, x2, s);
        if dx.abs() < 1e-6 {
            break;
        }
        s = (s - (c(x1, x2, s) - t) / dx).clamp(0.0, 1.0);
    }
    c(y1, y2, s)
}

fn aspect_of(m: &MediaItem) -> f64 {
    match (m.width, m.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => w as f64 / h as f64,
        _ => 1.5,
    }
}

/// How a layer is transformed by its transition: offset x (px), scale, horizontal squash
/// (flip), opacity, dim, glow, circle radius (< 0 = none).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Fx {
    pub x: f64,
    pub scale: f64,
    pub sx: f64,
    pub opacity: f64,
    pub dim: f64,
    pub glow: f64,
    pub clip: f64,
}

const STILL: Fx = Fx { x: 0.0, scale: 1.0, sx: 1.0, opacity: 1.0, dim: 0.0, glow: 0.0, clip: -1.0 };

/// The incoming slide `p` (0..1) through its transition into a `w`×`h` stage.
pub(crate) fn fx_in(kind: ShowTransition, dir: f64, p: f64, w: f64, h: f64) -> Fx {
    let e = bezier(EASE, p);
    match kind {
        ShowTransition::None => STILL,
        ShowTransition::Fade => Fx { opacity: e, ..STILL },
        ShowTransition::Slide => Fx { x: dir * w * (1.0 - e), ..STILL },
        ShowTransition::Zoom => {
            let z = bezier(EASE_OUT, p);
            Fx { scale: 1.45 - 0.45 * z, opacity: z, ..STILL }
        }
        // no blur filter in the renderer: a bright bloom that settles stands in for it
        ShowTransition::Blur => Fx { opacity: e, scale: 1.08 - 0.08 * e, glow: 0.5 * (1.0 - e), ..STILL },
        ShowTransition::Flip => {
            if p < 0.5 {
                Fx { opacity: 0.0, ..STILL }
            } else {
                let q = bezier(EASE_FLIP_IN, (p - 0.5) * 2.0);
                Fx { sx: q.max(0.001), scale: 0.86 + 0.14 * q, ..STILL }
            }
        }
        ShowTransition::Circle => {
            // CSS circle(75%) is 75 % of sqrt((w² + h²) / 2)
            let full = 0.75 * ((w * w + h * h) / 2.0).sqrt();
            Fx { clip: bezier(EASE_CIRCLE, p) * full, ..STILL }
        }
    }
}

/// The outgoing slide `p` (0..1) through the transition into the next one.
pub(crate) fn fx_out(kind: ShowTransition, dir: f64, p: f64, w: f64) -> Fx {
    let e = bezier(EASE, p);
    match kind {
        ShowTransition::None => Fx { opacity: 0.0, ..STILL },
        ShowTransition::Slide => Fx { x: -0.35 * dir * w * e, dim: 0.55 * e, ..STILL },
        ShowTransition::Zoom => Fx { scale: 1.0 - 0.1 * e, opacity: 1.0 - 0.7 * e, ..STILL },
        ShowTransition::Blur => Fx { opacity: 1.0 - e, glow: 0.5 * e, ..STILL },
        ShowTransition::Flip => {
            if p >= 0.5 {
                Fx { opacity: 0.0, ..STILL }
            } else {
                let q = bezier(EASE_FLIP_OUT, p * 2.0);
                Fx { sx: (1.0 - q).max(0.001), scale: 1.0 - 0.14 * q, ..STILL }
            }
        }
        // fade / circle: the old slide holds under the new one
        ShowTransition::Fade | ShowTransition::Circle => STILL,
    }
}

/// Lay one slide out: contain fit, Ken Burns (scale, drift in % of the stage), then the layer fx.
pub(crate) fn layout(
    image: Option<Image>,
    aspect: f64,
    kb: Option<(f64, f64, f64)>,
    fx: Fx,
    (w, h): (f64, f64),
) -> SlideVis {
    let (fw, fh) = if w / h.max(1.0) > aspect { (h * aspect, h) } else { (w, w / aspect) };
    let (ks, kx, ky) = kb.unwrap_or((1.0, 0.0, 0.0));
    let (pw, ph) = (fw * ks, fh * ks);
    let (cx, cy) = (w / 2.0 + kx / 100.0 * w, h / 2.0 + ky / 100.0 * h);
    let (px, py) = (cx - pw / 2.0, cy - ph / 2.0);
    let (s, sx) = (fx.scale, fx.sx);
    let (lw, lh) = (w * s * sx, h * s);
    SlideVis {
        has_image: image.is_some(),
        image: image.unwrap_or_default(),
        opacity: fx.opacity as f32,
        lx: ((w - lw) / 2.0 + fx.x) as f32,
        ly: ((h - lh) / 2.0) as f32,
        lw: lw as f32,
        lh: lh as f32,
        ix: (px * s * sx) as f32,
        iy: (py * s) as f32,
        iw: (pw * s * sx) as f32,
        ih: (ph * s) as f32,
        clip_r: fx.clip as f32,
        dim: fx.dim as f32,
        glow: fx.glow as f32,
    }
}

impl Show {
    /// `thumb_size`: 640 (preview) or 1600 (full screen); `lang_date`: captions carry the date line.
    pub(crate) fn new(
        app: &Rc<AppInner>,
        thumb_size: u32,
        lang_date: bool,
        frame: impl Fn(StageFrame) + 'static,
        status: impl Fn(&ShowStatus) + 'static,
    ) -> Rc<Show> {
        let now = Instant::now();
        Rc::new_cyclic(|me| Show {
            app: Rc::downgrade(app),
            me: me.clone(),
            st: RefCell::new(St {
                slides: vec![],
                td: 1.0,
                looping: false,
                bg: Color::from_rgb_u8(0, 0, 0),
                captions: false,
                index: 0,
                dir: 1.0,
                playing: false,
                ended: false,
                clock: 0.0,
                start_at: 0.0,
                changed_at: now,
                entered_at: now,
                prev: None,
                size: (640.0, 360.0),
                scale: 1.0,
                images: HashMap::new(),
                inflight: HashSet::new(),
                video: None,
                video_frame: None,
                last: now,
                status: ShowStatus::default(),
            }),
            timer: slint::Timer::default(),
            thumb_size,
            lang_date,
            frame: Box::new(frame),
            status: Box::new(status),
        })
    }

    /// New playlist (restarts at the first slide).
    pub(crate) fn load(&self, slides: Vec<PlaySlide>, td: f64, looping: bool, bg: Color, captions: bool, autoplay: bool) {
        {
            let mut s = self.st.borrow_mut();
            s.slides = slides;
            s.td = td;
            s.looping = looping;
            s.bg = bg;
            s.captions = captions;
            s.index = 0;
            s.dir = 1.0;
            s.playing = autoplay;
            s.ended = false;
            s.clock = 0.0;
            s.start_at = 0.0;
            s.changed_at = Instant::now();
            s.entered_at = s.changed_at;
            s.prev = None;
            s.video = None;
            s.video_frame = None;
        }
        self.after_change();
        self.start();
    }

    pub(crate) fn set_captions(&self, on: bool) {
        self.st.borrow_mut().captions = on;
    }

    pub(crate) fn resize(&self, w: f64, h: f64, scale: f64) {
        let mut s = self.st.borrow_mut();
        s.size = (w.max(1.0), h.max(1.0));
        s.scale = scale.max(0.5);
    }

    pub(crate) fn status(&self) -> ShowStatus {
        self.st.borrow().status.clone()
    }

    fn start(&self) {
        if self.timer.running() {
            return;
        }
        self.st.borrow_mut().last = Instant::now();
        let me = self.me.clone();
        self.timer.start(slint::TimerMode::Repeated, Duration::from_millis(16), move || {
            if let Some(m) = me.upgrade() {
                m.tick();
            }
        });
    }

    /// Stop the clock and any video (closing the player / leaving the builder).
    pub(crate) fn stop(&self) {
        self.timer.stop();
        let mut s = self.st.borrow_mut();
        s.video = None;
        s.playing = false;
    }

    pub(crate) fn set_playing(&self, p: bool) {
        let mut s = self.st.borrow_mut();
        s.playing = p;
        if let Some((_, v)) = s.video.as_mut() {
            v.set_playing(p);
        }
        drop(s);
        self.start();
    }

    pub(crate) fn toggle(&self) {
        let ended = self.st.borrow().ended;
        if ended {
            self.go(0, 1.0);
            self.set_playing(true);
        } else {
            let p = self.st.borrow().playing;
            self.set_playing(!p);
        }
    }

    pub(crate) fn next(&self) {
        let (i, n, looping) = {
            let s = self.st.borrow();
            (s.index, s.slides.len(), s.looping)
        };
        if n == 0 {
            return;
        }
        if i + 1 < n {
            self.go(i + 1, 1.0);
        } else if looping && n > 1 {
            self.go(0, 1.0);
        } else {
            let mut s = self.st.borrow_mut();
            s.ended = true;
            s.playing = false;
            if let Some((_, v)) = s.video.as_mut() {
                v.set_playing(false);
            }
        }
    }

    pub(crate) fn prev(&self) {
        let (i, elapsed) = {
            let s = self.st.borrow();
            (s.index, s.clock - s.start_at)
        };
        // like a music player: back to the start of this slide first
        if elapsed > 1.5 || i == 0 {
            self.go(i, -1.0);
        } else {
            self.go(i - 1, -1.0);
        }
    }

    fn go(&self, i: usize, dir: f64) {
        {
            let mut s = self.st.borrow_mut();
            if s.slides.is_empty() {
                return;
            }
            let i = i.min(s.slides.len() - 1);
            let old = (s.index, s.start_at);
            s.prev = if old.0 != i { Some(old) } else { None };
            s.index = i;
            s.dir = dir;
            s.start_at = s.clock;
            s.changed_at = Instant::now();
            s.entered_at = s.changed_at;
            s.ended = false;
            s.video = None;
            s.video_frame = None;
        }
        self.after_change();
        self.start();
    }

    /// Load pictures around the current slide, start a video slide.
    fn after_change(&self) {
        let Some(a) = self.app.upgrade() else { return };
        let (ids, video) = {
            let s = self.st.borrow();
            let n = s.slides.len();
            let ids: Vec<String> = (0..3)
                .filter_map(|k| s.slides.get((s.index + k) % n.max(1)))
                .map(|p| p.media.id.clone())
                .filter(|id| !s.images.contains_key(id) && !s.inflight.contains(id))
                .collect();
            let video = s
                .slides
                .get(s.index)
                .filter(|p| p.media.kind == MediaKind::Video)
                .cloned();
            (ids, video)
        };
        for id in ids {
            self.st.borrow_mut().inflight.insert(id.clone());
            let me = self.me.clone();
            let key = id.clone();
            crate::pages::cover(&a, &id, self.thumb_size, move |img| {
                if let Some(m) = me.upgrade() {
                    let mut s = m.st.borrow_mut();
                    s.inflight.remove(&key);
                    s.images.insert(key, img);
                }
            });
        }
        if let Some(p) = video {
            self.start_video(&a, &p);
        }
    }

    fn start_video(&self, a: &Rc<AppInner>, p: &PlaySlide) {
        let Some(src) = a.backend.ops().and_then(|o| o.source(&p.media, None)) else {
            return;
        };
        let (index, (w, h), scale, playing) = {
            let s = self.st.borrow();
            (s.index, s.size, s.scale, s.playing)
        };
        let (ow, oh) = video::output_size(
            aspect_of(&p.media),
            w * scale,
            h * scale,
            p.media.width,
            p.media.height,
        );
        let source = video::Source {
            input: src.as_input(),
            video: true,
            width: ow,
            height: oh,
            fps: 30.0,
            duration: p.media.duration,
            audio: None,
            vf_pre: Vec::new(),
            vf_post: Vec::new(),
        };
        let start = p.trim.as_ref().map_or(0.0, |t| t.start);
        // slides play muted (the music is the show's sound), as in 1.x
        let player = video::Player::new(source, start, playing, 1.0, true, 1.0);
        self.st.borrow_mut().video = Some((index, player));
    }

    fn tick(&self) {
        let now = Instant::now();
        let advance = {
            let mut s = self.st.borrow_mut();
            let dt = now.duration_since(s.last).as_secs_f64().min(0.1);
            s.last = now;
            let n = s.slides.len();
            if n == 0 {
                return;
            }
            if let Some((_, v)) = s.video.as_mut() {
                if let Some(f) = v.take_frame() {
                    s.video_frame = Some(Image::from_rgba8(f.px));
                }
            }
            let cur = s.slides[s.index.min(n - 1)].clone();
            // hold the clock and the transition until the slide's picture is there (at most 3 s),
            // so a slide never fades in black and then pops
            let ready = s.images.contains_key(&cur.media.id)
                || (cur.media.kind == MediaKind::Video && s.video_frame.is_some());
            let waiting = !ready && now.duration_since(s.entered_at) < Duration::from_secs(3);
            if waiting {
                s.changed_at = now;
            } else if s.playing && !s.ended {
                s.clock += dt;
            }
            let elapsed = s.clock - s.start_at;
            let next = if s.index + 1 < n {
                Some(s.index + 1)
            } else if s.looping && n > 1 {
                Some(0)
            } else {
                None
            };
            let cut = match next {
                Some(0) => s.td.min(cur.dur / 2.0).min(s.slides[0].dur / 2.0),
                Some(j) => cut_length(&s.slides, j, s.td),
                None => 0.0,
            };
            let advance_at = (cur.dur - cut).max(0.2);
            let mut rest = (advance_at - elapsed).max(0.0);
            for j in s.index + 1..n {
                rest += s.slides[j].dur - if j + 1 < n { cut_length(&s.slides, j + 1, s.td) } else { 0.0 };
            }
            s.status = ShowStatus {
                index: s.index,
                count: n,
                progress: (elapsed / advance_at).min(1.0),
                playing: s.playing,
                ended: s.ended,
                remaining: rest,
            };
            s.playing && !s.ended && elapsed >= advance_at
        };
        if advance {
            self.next();
        }
        let status = self.status();
        (self.status)(&status);
        (self.frame)(self.compose());
    }

    fn compose(&self) -> StageFrame {
        let s = self.st.borrow();
        let n = s.slides.len();
        let i = s.index.min(n.saturating_sub(1));
        let cur = &s.slides[i];
        let size = s.size;
        let t = s.changed_at.elapsed().as_secs_f64();
        let d = {
            let c = cut_length(&s.slides, i, s.td);
            if c > 0.0 {
                c
            } else if cur.transition == ShowTransition::None {
                0.0
            } else {
                s.td.min(cur.dur / 2.0)
            }
        }
        .max(0.05);
        let p = (t / d).min(1.0);
        let kind = cur.transition;
        let span_of = |k: usize| {
            let sl = &s.slides[k];
            let next_cut = if k + 1 < n { cut_length(&s.slides, k + 1, s.td) } else { 0.0 };
            (sl.dur + next_cut).max(0.5)
        };
        let kb_of = |k: usize, start: f64| {
            s.slides[k]
                .kb
                .map(|m| kb_at(m, (s.clock - start) / span_of(k)))
        };
        let img_of = |k: usize, active: bool| -> Option<Image> {
            let sl = &s.slides[k];
            if active && sl.media.kind == MediaKind::Video {
                if let Some(f) = &s.video_frame {
                    return Some(f.clone());
                }
            }
            s.images.get(&sl.media.id).cloned()
        };
        let cur_vis = layout(
            img_of(i, true),
            aspect_of(&cur.media),
            kb_of(i, s.start_at),
            fx_in(kind, s.dir, p, size.0, size.1),
            size,
        );
        let prev = s.prev.filter(|_| p < 1.0 && kind != ShowTransition::None);
        let prev_vis = prev.map(|(k, start)| {
            layout(
                img_of(k, false),
                aspect_of(&s.slides[k].media),
                kb_of(k, start),
                fx_out(kind, s.dir, p, size.0),
                size,
            )
        });
        // caption: fades in after 60 % of the cut, rising 14 px over 0.6 s
        let (cap_o, cap_y) = if s.captions {
            let c = ((t - d * 0.6) / 0.6).clamp(0.0, 1.0);
            let e = bezier(EASE_FLIP_IN, c);
            (e, 14.0 * (1.0 - e))
        } else {
            (0.0, 0.0)
        };
        let title = cur
            .media
            .name
            .rsplit_once('.')
            .map_or(cur.media.name.as_str(), |(a, _)| a)
            .to_string();
        let date = if self.lang_date && (cur.media.local_at != 0 || cur.media.taken_at != 0) {
            let local = if cur.media.local_at != 0 {
                cur.media.local_at
            } else {
                cur.media.taken_at + crate::viewer_fmt::local_offset_ms()
            };
            self.app
                .upgrade()
                .map(|a| crate::viewer_fmt::date_long(local, a.lang()))
                .unwrap_or_default()
        } else {
            String::new()
        };
        StageFrame {
            cur: cur_vis,
            has_prev: prev_vis.is_some(),
            prev: prev_vis.unwrap_or_default(),
            bg: s.bg,
            caption_title: title.into(),
            caption_date: date.into(),
            caption_opacity: cap_o as f32,
            caption_y: cap_y as f32,
        }
    }
}

impl Drop for Show {
    fn drop(&mut self) {
        self.timer.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bezier_ends_and_midpoint() {
        assert!((bezier(EASE, 0.0)).abs() < 1e-9);
        assert!((bezier(EASE, 1.0) - 1.0).abs() < 1e-9);
        // symmetric curve: half way in time is half way in value
        assert!((bezier(EASE, 0.5) - 0.5).abs() < 1e-3);
        // ease-out is ahead of linear early on
        assert!(bezier(EASE_OUT, 0.2) > 0.5);
    }

    #[test]
    fn transitions_start_and_end() {
        let (w, h) = (1600.0, 900.0);
        for k in crate::slideshow::RANDOM_POOL {
            let end = fx_in(k, 1.0, 1.0, w, h);
            assert!((end.opacity - 1.0).abs() < 1e-6 && end.x.abs() < 1e-6, "{k:?} lands on the stage");
            assert!((end.scale - 1.0).abs() < 1e-6 && (end.sx - 1.0).abs() < 1e-6, "{k:?} at full size");
        }
        assert_eq!(fx_in(ShowTransition::Slide, -1.0, 0.0, w, h).x, -w);
        assert_eq!(fx_in(ShowTransition::Circle, 1.0, 0.0, w, h).clip, 0.0);
        assert_eq!(fx_out(ShowTransition::Fade, 1.0, 0.7, w), STILL);
        assert_eq!(fx_out(ShowTransition::Flip, 1.0, 0.6, w).opacity, 0.0);
    }

    #[test]
    fn contain_and_ken_burns_layout() {
        // a 3:2 picture in a 16:9 stage: full height, centred
        let v = layout(None, 1.5, None, STILL, (1600.0, 900.0));
        assert_eq!((v.iw, v.ih, v.ix, v.iy), (1350.0, 900.0, 125.0, 0.0));
        // Ken Burns 1.15 with a 2.2 % drift to the right
        let v = layout(None, 1.5, Some((1.15, 2.2, 0.0)), STILL, (1600.0, 900.0));
        assert!((v.iw - 1552.5).abs() < 1e-3);
        assert!((v.ix + v.iw / 2.0 - (800.0 + 35.2)).abs() < 1e-3);
        // a half-flipped layer is half as wide and stays centred
        let v = layout(None, 1.5, None, Fx { sx: 0.5, ..STILL }, (1600.0, 900.0));
        assert_eq!((v.lw, v.lx), (800.0, 400.0));
    }
}
