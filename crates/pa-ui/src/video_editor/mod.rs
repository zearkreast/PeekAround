//! Video editor (features/video-editor): opened by the "edit" action on one video. The stage
//! plays the source through the ffmpeg player (src/video.rs) with the edit's crop / rotate /
//! flip / colour as decoder filters, so the preview runs the export's own filters; text layers
//! are drawn by the photo editor's text renderer over it and rendered to PNG overlays on export
//! (`PUT /api/jobs/assets`, then `POST /api/jobs/video-edit`).

mod export;
pub(crate) mod model;

pub(crate) use export::{on_job, owns_job};

use std::cell::{Cell, RefCell, RefMut};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use pa_core::{MediaItem, MediaKind, VideoFormat, VideoQuality};
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};

use crate::actions_ops::ImageSize;
use crate::app::AppInner;
use crate::backend::ThumbData;
use crate::editor::doc::{Align, History, TextLayer, TextShadow};
use crate::editor::overlay::Fonts;
use crate::editor::textprops::{self, DEFAULT_FAMILY};
use crate::i18n::Lang;
use crate::video;
use crate::{EdFont, EdLayerVis, EdSlider, VeBar, VeOption, VeTextRow, VeTick, VideoEditorState};
use model::{EditState, Handle, Rect, Size, VideoText};

/// timeline filmstrip height (px)
const STRIP_H: f64 = 64.0;
/// a scrub seeks at most this often (each seek restarts ffmpeg)
const SCRUB_EVERY: Duration = Duration::from_millis(90);

#[derive(Clone)]
enum Drag {
    /// crop box (oriented source px at the start)
    Crop { handle: Handle, x: f64, y: f64, start: Rect },
    TextMove { id: u32, x: f64, y: f64, start: TextLayer },
    TextScale { id: u32, d0: f64, start: TextLayer },
    Scrub,
    TrimIn,
    TrimOut,
    /// a text bar on the timeline (output seconds at the start)
    Bar { id: u32, part: String, x0: f64, start: f64, end: f64 },
}

/// A layer's raster at one stage scale.
#[derive(Clone)]
struct Cached {
    look: TextLayer,
    scale: f64,
    epoch: usize,
    image: Image,
    w: f64,
    h: f64,
    box_w: f64,
    box_h: f64,
    /// raster px per design px
    rs: f64,
}

/// The export run (JobProgress `useJobRun`).
#[derive(Default)]
struct JobRun {
    /// idle, preparing, queued, running, done, error, canceled
    status: String,
    progress: f64,
    title: String,
    error: String,
    id: Option<String>,
    result: Option<pa_core::FileLocation>,
}

struct Ctl {
    app: Weak<AppInner>,
    /// bumps on every open / close: late results of an earlier video are dropped
    gen: Cell<u64>,
    item: RefCell<Option<MediaItem>>,
    input: RefCell<Option<String>>,
    src: Cell<Size>,
    duration: Cell<f64>,
    fps: Cell<f64>,
    has_audio: Cell<bool>,
    /// details loaded, the player can start
    ready: Cell<bool>,
    hist: RefCell<History<EditState>>,
    tool: RefCell<String>,
    selected: Cell<Option<u32>>,
    next_id: Cell<u32>,
    stage: Cell<(f64, f64)>,
    tl_w: Cell<f64>,
    player: RefCell<Option<video::Player>>,
    tick: slint::Timer,
    filter_timer: slint::Timer,
    scrub_timer: slint::Timer,
    strip_timer: slint::Timer,
    last_status: Cell<Instant>,
    last_seek: Cell<Instant>,
    pending_seek: Cell<Option<f64>>,
    /// playhead (source s)
    time: Cell<f64>,
    playing: Cell<bool>,
    scrubbing: Cell<bool>,
    was_playing: Cell<bool>,
    /// ids of the text layers visible at the playhead
    visible_key: RefCell<String>,
    drag: RefCell<Option<Drag>>,
    fonts: RefCell<Option<Fonts>>,
    rasters: RefCell<HashMap<u32, Cached>>,
    strip_n: Cell<u32>,
    /// why exporting is off ("" = allowed)
    disabled: RefCell<String>,
    suffix: RefCell<String>,
    job: RefCell<JobRun>,
    sliders: Rc<VecModel<EdSlider>>,
    layer_vis: Rc<VecModel<EdLayerVis>>,
    lang: Cell<Option<Lang>>,
}

thread_local! {
    static CTL: RefCell<Option<Rc<Ctl>>> = const { RefCell::new(None) };
}

fn ctl() -> Option<Rc<Ctl>> {
    CTL.with(|c| c.borrow().clone())
}

/// `Number(v.toFixed(2))` of the 1.x number inputs
fn num2(v: f64) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

fn parse_num(s: &str) -> Option<f64> {
    s.trim().replace(',', ".").parse::<f64>().ok().filter(|v| v.is_finite())
}

fn format_str(f: VideoFormat) -> &'static str {
    model::ext_of(f)
}

fn quality_str(q: VideoQuality) -> &'static str {
    match q {
        VideoQuality::High => "high",
        VideoQuality::Medium => "medium",
        VideoQuality::Low => "low",
    }
}

impl Ctl {
    fn app(&self) -> Option<Rc<AppInner>> {
        self.app.upgrade()
    }

    fn doc(&self) -> EditState {
        self.hist.borrow().doc.clone()
    }

    fn with_vs(&self, f: impl FnOnce(&VideoEditorState)) {
        if let Some(a) = self.app() {
            f(&a.window.global::<VideoEditorState>());
        }
    }

    fn crop_mode(&self) -> bool {
        *self.tool.borrow() == "crop"
    }

    fn fonts(&self) -> RefMut<'_, Fonts> {
        RefMut::map(self.fonts.borrow_mut(), |f| f.get_or_insert_with(|| Fonts::new(slint::fontique_011::shared_collection())))
    }

    // ───────── document ─────────

    fn change(self: &Rc<Self>, f: impl FnOnce(&EditState) -> EditState, key: Option<&str>) {
        if !self.ready.get() {
            return;
        }
        let prev = self.doc();
        let next = model::keep_texts(&prev, f(&prev), self.src.get());
        self.hist.borrow_mut().change(next, key);
        self.after_change();
    }

    fn after_change(self: &Rc<Self>) {
        let s = self.doc();
        if self.selected.get().is_some_and(|id| !s.texts.iter().any(|t| t.layer.id == id)) {
            self.selected.set(None);
        }
        self.publish();
        self.sync_player();
    }

    fn change_text(self: &Rc<Self>, id: u32, key: &str, f: impl FnOnce(&mut VideoText)) {
        let key = format!("text.{id}.{key}");
        self.change(
            |s| {
                let mut n = s.clone();
                if let Some(t) = n.texts.iter_mut().find(|t| t.layer.id == id) {
                    f(t);
                }
                n
            },
            Some(&key),
        );
    }

    // ───────── publishing ─────────

    fn publish(&self) {
        let Some(a) = self.app() else { return };
        let vs = a.window.global::<VideoEditorState>();
        let lang = a.lang();
        let s = self.doc();
        let src = self.src.get();
        let duration = self.duration.get();
        let h = self.hist.borrow();
        vs.set_can_undo(h.can_undo());
        vs.set_can_redo(h.can_redo());
        drop(h);
        vs.set_tool(self.tool.borrow().clone().into());
        let dur = model::out_duration(&s);
        vs.set_dur_text(model::fmt_time(dur).into());
        vs.set_has_audio(self.has_audio.get());
        vs.set_mute(s.mute);
        // trim
        vs.set_trim_start(num2(s.trim.start).into());
        vs.set_trim_end(num2(s.trim.end).into());
        vs.set_clip_len(model::fmt_time(s.trim.end - s.trim.start).into());
        vs.set_full_len(model::fmt_time(duration).into());
        vs.set_trimmed(s.trim.start > 0.0 || s.trim.end < duration);
        // crop
        vs.set_aspect(s.aspect.into());
        vs.set_has_crop(s.crop.is_some());
        vs.set_flip_h(s.flip_h);
        vs.set_flip_v(s.flip_v);
        let out = model::export_size(src, &s);
        vs.set_output_size(format!("{} × {}", out.w, out.h).into());
        // adjust
        for (i, (key, ..)) in model::ADJUST_KEYS.iter().enumerate() {
            let v = s.adjust.get(key);
            let ui = if *key == "hue" { v.round() } else { (v * 100.0).round() } as f32;
            if let Some(mut r) = self.sliders.row_data(i) {
                if r.value != ui {
                    r.value = ui;
                    r.text = if *key == "hue" {
                        format!("{ui}°")
                    } else if ui > 0.0 {
                        format!("+{ui}")
                    } else {
                        format!("{ui}")
                    }
                    .into();
                    self.sliders.set_row_data(i, r);
                }
            }
        }
        vs.set_adjusted(s.adjust.is_adjusted());
        // texts
        let sel = self.selected.get();
        let rows: Vec<VeTextRow> = s
            .texts
            .iter()
            .map(|t| {
                let (a0, b0) = model::text_window(t, dur);
                VeTextRow {
                    id: t.layer.id as i32,
                    label: t.layer.text.lines().next().filter(|l| !l.trim().is_empty()).unwrap_or("…").into(),
                    family: t.layer.family.clone().into(),
                    color: textprops::to_color(t.layer.color),
                    range: format!("{} – {}", model::fmt_time(a0), model::fmt_time(b0)).into(),
                    selected: sel == Some(t.layer.id),
                }
            })
            .collect();
        vs.set_text_rows(ModelRc::new(VecModel::from(rows)));
        let cur = sel.and_then(|id| s.texts.iter().find(|t| t.layer.id == id));
        vs.set_has_sel(cur.is_some());
        if let Some(t) = cur {
            let design = model::base_size(src, s.crop, s.rotate);
            vs.set_sel(textprops::props(&t.layer, (design.h * 0.4).round().max(60.0)));
            let (a0, b0) = model::text_window(t, dur);
            vs.set_sel_from(num2(a0).into());
            vs.set_sel_to(num2(b0).into());
        }
        // speed & sound
        vs.set_speed(s.speed as f32);
        vs.set_speed_hint(crate::i18n::tv(lang, "video.speedHint", &[("d", &model::fmt_time(dur))]).into());
        vs.set_volume((s.volume * 100.0).round() as f32);
        // export
        vs.set_format(format_str(s.output.format).into());
        vs.set_short(s.output.short.map_or("0".to_string(), |v| v.to_string()).into());
        let base = model::base_size(src, s.crop, s.rotate);
        let short = base.w.min(base.h);
        let shorts: Vec<VeOption> = model::SHORT_SIDES
            .iter()
            .filter(|v| v.is_none_or(|v| (v as f64) < short))
            .map(|v| match v {
                None => VeOption { value: "0".into(), label: crate::i18n::t(lang, "video.original").into() },
                Some(v) => VeOption { value: v.to_string().into(), label: format!("{v}p").into() },
            })
            .collect();
        vs.set_shorts(ModelRc::new(VecModel::from(shorts)));
        vs.set_quality(quality_str(s.output.quality).into());
        vs.set_out_dur(model::fmt_time(dur).into());
        vs.set_out_audio(self.has_audio.get() && !s.mute && s.output.format != VideoFormat::Gif);
        vs.set_text_count(s.texts.len() as i32);
        if let Some(item) = self.item.borrow().as_ref() {
            vs.set_file_name(
                format!("{}{}.{}", model::base_name(&item.name), self.suffix.borrow(), format_str(s.output.format)).into(),
            );
        }
        drop(vs);
        self.publish_stage();
        self.publish_timeline();
        self.publish_time();
    }

    /// (x, y, w, h) of the frame on the stage and stage px per design px.
    fn frame_rect(&self) -> (f64, f64, f64, f64, f64) {
        let (sw, sh) = self.stage.get();
        let pad = if self.crop_mode() { 44.0 } else { 20.0 };
        let d = self.design();
        let f = model::contain_size(Size { w: sw - pad * 2.0, h: sh - pad * 2.0 }, d.w / d.h);
        let ds = if d.w > 0.0 { f.w / d.w } else { 0.0 };
        ((sw - f.w) / 2.0, (sh - f.h) / 2.0, f.w, f.h, ds)
    }

    /// The frame on the stage: the whole oriented frame in the crop tool, else crop + rotate.
    fn design(&self) -> Size {
        let s = self.doc();
        if self.crop_mode() {
            model::oriented(self.src.get(), s.rotate)
        } else {
            model::base_size(self.src.get(), s.crop, s.rotate)
        }
    }

    fn publish_stage(&self) {
        let Some(a) = self.app() else { return };
        let vs = a.window.global::<VideoEditorState>();
        let (fx, fy, fw, fh, ds) = self.frame_rect();
        vs.set_frame_x(fx as f32);
        vs.set_frame_y(fy as f32);
        vs.set_frame_w(fw.max(0.0) as f32);
        vs.set_frame_h(fh.max(0.0) as f32);
        if self.crop_mode() {
            let s = self.doc();
            let src = self.src.get();
            let o = model::oriented(src, s.rotate);
            let r = model::normalize_crop(s.crop, src.w, src.h)
                .map(|c| model::orient_rect(c, src, s.rotate, s.flip_h, s.flip_v))
                .unwrap_or(Rect { x: 0.0, y: 0.0, w: o.w, h: o.h });
            vs.set_crop_x((fx + r.x * ds) as f32);
            vs.set_crop_y((fy + r.y * ds) as f32);
            vs.set_crop_w((r.w * ds) as f32);
            vs.set_crop_h((r.h * ds) as f32);
            vs.set_crop_label(format!("{} × {}", r.w.round(), r.h.round()).into());
        }
        drop(vs);
        self.publish_layers();
    }

    fn raster(&self, l: &TextLayer, scale: f64) -> Cached {
        let epoch = crate::pages::fonts::epoch();
        if let Some(c) = self.rasters.borrow().get(&l.id) {
            if c.epoch == epoch && (c.scale - scale).abs() < 1e-9 && c.look.look_eq(l) {
                return c.clone();
            }
        }
        let r = self.fonts().render_text(l, scale);
        let c = Cached {
            look: l.clone(),
            scale,
            epoch,
            w: r.w as f64,
            h: r.h as f64,
            box_w: r.box_w,
            box_h: r.box_h,
            rs: r.scale,
            image: crate::editor::to_image(r.px, r.w, r.h),
        };
        self.rasters.borrow_mut().insert(l.id, c.clone());
        c
    }

    /// (layer, visible at the playhead) for the layers on the stage: the visible ones and,
    /// while editing text, the selected one (dimmed outside its window).
    fn shown(&self) -> Vec<(TextLayer, bool)> {
        if self.crop_mode() {
            return vec![];
        }
        let s = self.doc();
        let dur = model::out_duration(&s);
        let t = model::src_to_out(&s, self.time.get());
        let editing = *self.tool.borrow() == "text";
        s.texts
            .iter()
            .filter_map(|x| {
                let (a0, b0) = model::text_window(x, dur);
                let vis = t >= a0 - 1e-3 && t <= b0 + 1e-3;
                (vis || (editing && self.selected.get() == Some(x.layer.id))).then(|| (x.layer.clone(), vis))
            })
            .collect()
    }

    fn visible_key(&self) -> String {
        let s = self.doc();
        let dur = model::out_duration(&s);
        let t = model::src_to_out(&s, self.time.get());
        s.texts
            .iter()
            .filter(|x| {
                let (a0, b0) = model::text_window(x, dur);
                t >= a0 - 1e-3 && t <= b0 + 1e-3
            })
            .map(|x| x.layer.id.to_string())
            .collect::<Vec<_>>()
            .join("|")
    }

    fn publish_layers(&self) {
        let Some(a) = self.app() else { return };
        *self.visible_key.borrow_mut() = self.visible_key();
        let (fx, fy, _, _, ds) = self.frame_rect();
        let sf = a.window.window().scale_factor() as f64;
        let sel = self.selected.get();
        let mut vis = Vec::new();
        if ds > 0.0 {
            for (l, on) in self.shown() {
                let c = self.raster(&l, ds * sf);
                let k = ds / c.rs;
                vis.push(EdLayerVis {
                    id: l.id as i32,
                    image: c.image,
                    x: (fx + l.x * ds) as f32,
                    y: (fy + l.y * ds) as f32,
                    w: (c.w * k) as f32,
                    h: (c.h * k) as f32,
                    bw: (c.box_w * k) as f32,
                    bh: (c.box_h * k) as f32,
                    rotation: l.rotation as f32,
                    opacity: (l.opacity * if on { 1.0 } else { 0.45 }) as f32,
                    selected: sel == Some(l.id),
                });
            }
        }
        let ids: Vec<u32> = self.doc().texts.iter().map(|t| t.layer.id).collect();
        self.rasters.borrow_mut().retain(|id, _| ids.contains(id));
        if self.layer_vis.row_count() == vis.len() {
            for (i, r) in vis.into_iter().enumerate() {
                self.layer_vis.set_row_data(i, r);
            }
        } else {
            self.layer_vis.set_vec(vis);
        }
    }

    fn publish_timeline(&self) {
        let Some(a) = self.app() else { return };
        let vs = a.window.global::<VideoEditorState>();
        let s = self.doc();
        let duration = self.duration.get().max(0.001);
        vs.set_trim_a((s.trim.start / duration) as f32);
        vs.set_trim_b((s.trim.end / duration) as f32);
        vs.set_trim_a_text(model::fmt_time(s.trim.start).into());
        vs.set_trim_b_text(model::fmt_time(s.trim.end).into());
        let (_, ticks) = model::ruler(duration, self.tl_w.get());
        let ticks: Vec<VeTick> = ticks
            .into_iter()
            .map(|t| {
                let l = model::fmt_time(t);
                VeTick { x: (t / duration) as f32, label: l.strip_suffix(".0").unwrap_or(&l).into() }
            })
            .collect();
        vs.set_ticks(ModelRc::new(VecModel::from(ticks)));
        let dur = model::out_duration(&s);
        let sel = self.selected.get();
        let bars: Vec<VeBar> = s
            .texts
            .iter()
            .map(|t| {
                let (a0, b0) = model::text_window(t, dur);
                VeBar {
                    id: t.layer.id as i32,
                    label: t.layer.text.lines().next().filter(|l| !l.is_empty()).unwrap_or("…").into(),
                    family: t.layer.family.clone().into(),
                    a: (model::out_to_src(&s, a0) / duration) as f32,
                    b: (model::out_to_src(&s, b0) / duration) as f32,
                    selected: sel == Some(t.layer.id),
                }
            })
            .collect();
        vs.set_bars(ModelRc::new(VecModel::from(bars)));
    }

    /// Playhead, clock, and the text layers when the visible set changed.
    fn publish_time(&self) {
        let Some(a) = self.app() else { return };
        let vs = a.window.global::<VideoEditorState>();
        let s = self.doc();
        let t = self.time.get();
        vs.set_head((t / self.duration.get().max(0.001)) as f32);
        let dur = model::out_duration(&s);
        let out = (model::src_to_out(&s, t).max(0.0) * 10.0).round() / 10.0;
        vs.set_time_text(model::fmt_time(out.min(dur)).into());
        vs.set_playing(self.playing.get());
        vs.set_scrubbing(self.scrubbing.get());
        drop(vs);
        if *self.visible_key.borrow() != self.visible_key() {
            self.publish_layers();
        }
    }

    fn sync_text(&self) {
        self.publish();
        self.with_vs(|vs| vs.set_text_sync(vs.get_text_sync() + 1));
    }

    // ───────── playback ─────────

    /// Frame size and decoder filters the stage wants now.
    fn wanted(&self) -> (u32, u32, Vec<String>, Vec<String>) {
        let sf = self.app().map_or(1.0, |a| a.window.window().scale_factor() as f64);
        let (_, _, fw, fh, _) = self.frame_rect();
        let d = self.design();
        let (w, h) = video::output_size(d.w / d.h, (fw * sf).max(2.0), (fh * sf).max(2.0), Some(d.w as i64), Some(d.h as i64));
        let (pre, post) = model::preview_filters(&self.doc(), self.src.get(), self.crop_mode());
        (w, h, pre, post)
    }

    fn ensure_player(self: &Rc<Self>) {
        if self.player.borrow().is_some() || !self.ready.get() || self.stage.get().0 < 2.0 {
            return;
        }
        let Some(input) = self.input.borrow().clone() else { return };
        let (w, h, vf_pre, vf_post) = self.wanted();
        let s = self.doc();
        let src = video::Source {
            input,
            video: true,
            width: w,
            height: h,
            fps: self.fps.get().min(60.0),
            duration: Some(self.duration.get()),
            audio: Some(self.has_audio.get()),
            vf_pre,
            vf_post,
        };
        log::info!("video editor: preview {w}×{h} from {}", src.input);
        let p = video::Player::new(src, self.time.get(), false, s.volume.min(1.0), s.mute || s.volume == 0.0, s.speed);
        *self.player.borrow_mut() = Some(p);
        self.last_status.set(Instant::now() - Duration::from_secs(1));
        let me = Rc::downgrade(self);
        self.tick.start(slint::TimerMode::Repeated, Duration::from_millis(8), move || {
            if let Some(c) = me.upgrade() {
                c.on_tick();
            }
        });
    }

    /// Volume / mute right away; size, filters and speed (they restart ffmpeg) debounced.
    fn sync_player(self: &Rc<Self>) {
        if self.player.borrow().is_none() {
            self.ensure_player();
            return;
        }
        let s = self.doc();
        if let Some(p) = self.player.borrow().as_ref() {
            p.set_volume(s.volume.min(1.0));
            p.set_muted(s.mute || s.volume == 0.0);
        }
        let me = Rc::downgrade(self);
        self.filter_timer.start(slint::TimerMode::SingleShot, Duration::from_millis(140), move || {
            let Some(c) = me.upgrade() else { return };
            let (w, h, pre, post) = c.wanted();
            let speed = c.doc().speed;
            let mut pl = c.player.borrow_mut();
            if let Some(p) = pl.as_mut() {
                p.set_filters(w, h, pre, post);
                p.set_speed(speed);
            }
        });
    }

    fn on_tick(self: &Rc<Self>) {
        let Some(a) = self.app() else { return };
        let mut pl = self.player.borrow_mut();
        let Some(p) = pl.as_mut() else {
            drop(pl);
            self.tick.stop();
            return;
        };
        let vs = a.window.global::<VideoEditorState>();
        if let Some(f) = p.take_frame() {
            vs.set_frame(Image::from_rgba8(f.px));
            vs.set_has_frame(true);
        }
        if self.last_status.get().elapsed() < Duration::from_millis(30) {
            return;
        }
        self.last_status.set(Instant::now());
        let st = p.status();
        if !self.scrubbing.get() {
            let trim = self.hist.borrow().doc.trim;
            if self.playing.get() && (st.position >= trim.end - 0.02 || st.ended) {
                p.seek(trim.start);
                p.set_playing(true);
                self.time.set(trim.start);
            } else {
                self.time.set(st.position);
            }
        }
        drop(pl);
        vs.set_buffering(st.buffering);
        if let Some(e) = st.error.filter(|_| !vs.get_has_frame()) {
            vs.set_error(e.into());
        }
        drop(vs);
        self.publish_time();
    }

    fn seek_to(&self, t: f64, keep_playing: bool) {
        let t = t.clamp(0.0, self.duration.get());
        self.time.set(t);
        if let Some(p) = self.player.borrow_mut().as_mut() {
            p.seek(t);
            p.set_playing(keep_playing);
        }
        self.last_seek.set(Instant::now());
        self.playing.set(keep_playing);
        self.publish_time();
    }

    fn play(&self) {
        if !self.ready.get() {
            return;
        }
        let trim = self.doc().trim;
        let t = self.time.get();
        if t < trim.start - 0.01 || t >= trim.end - 0.05 {
            self.seek_to(trim.start, true);
        } else {
            if let Some(p) = self.player.borrow_mut().as_mut() {
                p.set_playing(true);
            }
            self.playing.set(true);
            self.publish_time();
        }
    }

    fn pause(&self) {
        if let Some(p) = self.player.borrow_mut().as_mut() {
            p.set_playing(false);
        }
        self.playing.set(false);
        self.publish_time();
    }

    fn toggle(&self) {
        if self.playing.get() {
            self.pause();
        } else {
            self.play();
        }
    }

    fn step(&self, frames: i32) {
        self.pause();
        self.seek_to(self.time.get() + frames as f64 / self.fps.get().max(1.0), false);
    }

    /// Live preview while dragging the playhead or a trim handle (seeks throttled).
    fn scrub(self: &Rc<Self>, t: f64) {
        if !self.scrubbing.get() {
            self.was_playing.set(self.playing.get());
            self.pause();
            self.scrubbing.set(true);
        }
        let t = t.clamp(0.0, self.duration.get());
        self.time.set(t);
        self.publish_time();
        if self.last_seek.get().elapsed() >= SCRUB_EVERY {
            self.last_seek.set(Instant::now());
            if let Some(p) = self.player.borrow_mut().as_mut() {
                p.seek(t);
            }
        } else {
            self.pending_seek.set(Some(t));
            if !self.scrub_timer.running() {
                let me = Rc::downgrade(self);
                self.scrub_timer.start(slint::TimerMode::SingleShot, SCRUB_EVERY, move || {
                    let Some(c) = me.upgrade() else { return };
                    if let Some(t) = c.pending_seek.take() {
                        c.last_seek.set(Instant::now());
                        if let Some(p) = c.player.borrow_mut().as_mut() {
                            p.seek(t);
                        }
                    }
                });
            }
        }
    }

    fn end_scrub(&self, t: f64) {
        self.scrub_timer.stop();
        self.pending_seek.set(None);
        self.scrubbing.set(false);
        self.seek_to(t, self.was_playing.get());
        self.was_playing.set(false);
    }

    fn request_strip(self: &Rc<Self>) {
        let me = Rc::downgrade(self);
        self.strip_timer.start(slint::TimerMode::SingleShot, Duration::from_millis(200), move || {
            if let Some(c) = me.upgrade() {
                c.load_strip();
            }
        });
    }

    fn load_strip(self: &Rc<Self>) {
        let Some(a) = self.app() else { return };
        let Some(item) = self.item.borrow().clone() else { return };
        let n = model::filmstrip_frames(self.tl_w.get(), STRIP_H, self.src.get());
        if n == 0 || n == self.strip_n.get() || !self.ready.get() {
            return;
        }
        let Some(ops) = a.backend.ops() else { return };
        self.strip_n.set(n);
        let fut = ops.image(&item.id, ImageSize::Filmstrip(n, 120));
        let gen = self.gen.get();
        let me = Rc::downgrade(self);
        a.rt.spawn_then(
            async move {
                let data = fut.await.map_err(|e| e.message)?;
                tokio::task::spawn_blocking(move || -> Result<(Vec<u8>, u32, u32), String> {
                    match data {
                        ThumbData::Encoded(b) => {
                            let img = image::load_from_memory(&b).map_err(|e| e.to_string())?.into_rgba8();
                            let (w, h) = img.dimensions();
                            Ok((img.into_raw(), w, h))
                        }
                        ThumbData::Rgba { width, height, pixels } => Ok((pixels, width, height)),
                    }
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |r| {
                let Some(c) = me.upgrade() else { return };
                if c.gen.get() != gen {
                    return;
                }
                match r {
                    Ok((px, w, h)) => {
                        let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
                        buf.make_mut_bytes().copy_from_slice(&px);
                        c.with_vs(|vs| {
                            vs.set_strip(Image::from_rgba8(buf));
                            vs.set_has_strip(true);
                        });
                    }
                    Err(e) => log::warn!("video editor filmstrip: {e}"),
                }
            },
        );
    }

    // ───────── open / close ─────────

    fn relabel(&self, lang: Lang) {
        if self.lang.replace(Some(lang)) == Some(lang) {
            return;
        }
        for i in 0..self.sliders.row_count() {
            if let Some(mut r) = self.sliders.row_data(i) {
                r.label = crate::i18n::t(lang, model::ADJUST_KEYS[i].1).into();
                self.sliders.set_row_data(i, r);
            }
        }
    }

    fn publish_fonts(&self) {
        let fonts: Vec<EdFont> = crate::pages::fonts::picker()
            .into_iter()
            .map(|(family, ..)| EdFont { label: family.clone().into(), family: family.into() })
            .collect();
        self.with_vs(|vs| vs.set_fonts(ModelRc::new(VecModel::from(fonts))));
    }

    fn open(self: &Rc<Self>, a: &Rc<AppInner>, item: MediaItem) {
        self.stop_player();
        self.gen.set(self.gen.get() + 1);
        let gen = self.gen.get();
        let duration = item.duration.filter(|d| *d > 0.0).unwrap_or(1.0);
        *self.hist.borrow_mut() = History::new(model::initial_state(duration));
        self.src.set(Size {
            w: item.width.filter(|w| *w > 0).unwrap_or(1280) as f64,
            h: item.height.filter(|h| *h > 0).unwrap_or(720) as f64,
        });
        self.duration.set(duration);
        self.fps.set(30.0);
        self.has_audio.set(false);
        self.ready.set(false);
        *self.input.borrow_mut() = None;
        *self.tool.borrow_mut() = "trim".into();
        self.selected.set(None);
        self.next_id.set(1);
        self.time.set(0.0);
        self.playing.set(false);
        self.scrubbing.set(false);
        *self.drag.borrow_mut() = None;
        self.rasters.borrow_mut().clear();
        self.strip_n.set(0);
        *self.disabled.borrow_mut() = String::new();
        *self.suffix.borrow_mut() = "_edit".into();
        *self.item.borrow_mut() = Some(item.clone());
        self.relabel(a.lang());
        crate::pages::fonts::load(a);
        self.publish_fonts();
        let vs = a.window.global::<VideoEditorState>();
        vs.set_name(item.name.clone().into());
        vs.set_meta(SharedString::new());
        vs.set_loading(true);
        vs.set_error(SharedString::new());
        vs.set_disabled(SharedString::new());
        vs.set_frame(Image::default());
        vs.set_has_frame(false);
        vs.set_strip(Image::default());
        vs.set_has_strip(false);
        vs.set_buffering(false);
        if !self.job_busy() {
            *self.job.borrow_mut() = JobRun::default();
        }
        vs.set_open(true);
        drop(vs);
        self.publish_job();
        self.publish();
        if item.kind != MediaKind::Video {
            self.with_vs(|vs| {
                vs.set_loading(false);
                vs.set_error(a.t("video.notFound").into());
            });
            return;
        }
        let Some(ops) = a.backend.ops() else { return };

        let info = a.backend.info();
        let me = Rc::downgrade(self);
        a.rt.spawn_then(info, move |r| {
            let (Some(c), Ok(info)) = (me.upgrade(), r) else { return };
            let Some(a) = c.app() else { return };
            let why = if info.caps.readonly {
                a.t("settings.readonly")
            } else if !info.caps.video_jobs {
                a.t("video.noFfmpeg")
            } else {
                String::new()
            };
            *c.disabled.borrow_mut() = why.clone();
            c.with_vs(|vs| vs.set_disabled(why.into()));
        });
        crate::api::call::<serde_json::Value>(a, "GET", "/api/settings", None, move |_, r| {
            let (Some(c), Ok(v)) = (ctl(), r) else { return };
            if let Some(sfx) = v.get("editSuffix").and_then(|x| x.as_str()) {
                *c.suffix.borrow_mut() = sfx.to_string();
                c.publish();
            }
        });

        let fut = ops.media(&item.id);
        let me = Rc::downgrade(self);
        a.rt.spawn_then(fut, move |r| {
            let Some(c) = me.upgrade() else { return };
            if c.gen.get() != gen {
                return;
            }
            let Some(a) = c.app() else { return };
            let d = match r {
                Ok(d) => d,
                Err(e) => {
                    log::warn!("video editor: {}", e.message);
                    c.with_vs(|vs| {
                        vs.set_loading(false);
                        vs.set_error(a.t("video.notFound").into());
                    });
                    return;
                }
            };
            let Some(ops) = a.backend.ops() else { return };
            let Some(src) = ops.source(&d.item, Some(&d.path)) else {
                c.with_vs(|vs| {
                    vs.set_loading(false);
                    vs.set_error(a.t("native.videoUnavailable").into());
                });
                return;
            };
            if let (Some(w), Some(h)) = (d.item.width.filter(|w| *w > 0), d.item.height.filter(|h| *h > 0)) {
                c.src.set(Size { w: w as f64, h: h as f64 });
            }
            if let Some(dur) = d.item.duration.filter(|d| *d > 0.0) {
                if (dur - c.duration.get()).abs() > 1e-6 {
                    c.duration.set(dur);
                    *c.hist.borrow_mut() = History::new(model::initial_state(dur));
                }
            }
            let fps = d.video.as_ref().and_then(|v| v.fps).filter(|f| *f > 1.0).unwrap_or(30.0);
            c.fps.set(fps);
            c.has_audio.set(d.video.as_ref().is_some_and(|v| v.audio_codec.is_some()));
            *c.input.borrow_mut() = Some(src.as_input());
            let sz = c.src.get();
            let mut meta = format!("{}×{} · {}", sz.w, sz.h, crate::viewer_fmt::clock(c.duration.get()));
            if d.video.as_ref().and_then(|v| v.fps).is_some() {
                meta.push_str(&format!(" · {} fps", fps.round()));
            }
            c.with_vs(|vs| {
                vs.set_meta(meta.into());
                vs.set_loading(false);
            });
            c.ready.set(true);
            c.publish();
            c.ensure_player();
            c.request_strip();
        });
    }

    fn stop_player(&self) {
        self.tick.stop();
        self.filter_timer.stop();
        self.scrub_timer.stop();
        self.strip_timer.stop();
        if let Some(p) = self.player.borrow_mut().take() {
            // ffmpeg is killed and reaped off the UI thread
            std::thread::spawn(move || drop(p));
        }
    }

    fn close(self: &Rc<Self>) {
        self.stop_player();
        self.gen.set(self.gen.get() + 1);
        self.ready.set(false);
        self.with_vs(|vs| {
            vs.set_open(false);
            vs.set_frame(Image::default());
            vs.set_strip(Image::default());
        });
        self.layer_vis.set_vec(vec![]);
    }

    fn set_tool(self: &Rc<Self>, t: &str) {
        *self.tool.borrow_mut() = t.to_string();
        if t == "crop" {
            self.pause();
        }
        self.publish();
        self.sync_player();
    }

    fn escape(self: &Rc<Self>) {
        if self.selected.get().is_some() {
            self.select(None);
        } else if self.crop_mode() {
            self.set_tool("trim");
        }
    }

    // ───────── stage pointer ─────────

    fn to_design(&self, x: f64, y: f64) -> (f64, f64) {
        let (fx, fy, _, _, ds) = self.frame_rect();
        ((x - fx) / ds.max(1e-9), (y - fy) / ds.max(1e-9))
    }

    fn box_of(&self, l: &TextLayer) -> (f64, f64) {
        if let Some(c) = self.rasters.borrow().get(&l.id) {
            if c.look.look_eq(l) {
                return (c.box_w / c.rs, c.box_h / c.rs);
            }
        }
        self.fonts().measure(l)
    }

    /// Is the stage point on the selected layer's resize handle?
    fn on_handle(&self, l: &TextLayer, x: f64, y: f64) -> bool {
        let (fx, fy, _, _, ds) = self.frame_rect();
        let (bw, bh) = self.box_of(l);
        let (cx, cy) = (fx + l.x * ds, fy + l.y * ds);
        let (lx, ly) = (bw * ds / 2.0 + 6.0, bh * ds / 2.0 + 6.0);
        let (sin, cos) = l.rotation.to_radians().sin_cos();
        let (hx, hy) = (cx + lx * cos - ly * sin, cy + lx * sin + ly * cos);
        (hx - x).hypot(hy - y) <= 12.0
    }

    /// Point (design px) inside a (rotated) layer box, with TextOverlay's 6 px slack.
    fn hit(&self, l: &TextLayer, px: f64, py: f64) -> bool {
        let (bw, bh) = self.box_of(l);
        let (sin, cos) = (-l.rotation.to_radians()).sin_cos();
        let (dx, dy) = (px - l.x, py - l.y);
        let rx = dx * cos - dy * sin;
        let ry = dx * sin + dy * cos;
        rx.abs() <= bw / 2.0 + 6.0 && ry.abs() <= bh / 2.0 + 6.0
    }

    fn press(self: &Rc<Self>, mode: &str, x: f64, y: f64) {
        if !self.ready.get() {
            return;
        }
        if let Some(h) = mode.strip_prefix("crop:").and_then(Handle::parse) {
            let s = self.doc();
            let src = self.src.get();
            let o = model::oriented(src, s.rotate);
            let start = model::normalize_crop(s.crop, src.w, src.h)
                .map(|c| model::orient_rect(c, src, s.rotate, s.flip_h, s.flip_v))
                .unwrap_or(Rect { x: 0.0, y: 0.0, w: o.w, h: o.h });
            *self.drag.borrow_mut() = Some(Drag::Crop { handle: h, x, y, start });
            return;
        }
        if mode != "stage" {
            return;
        }
        let shown = self.shown();
        if let Some((l, _)) = shown.iter().find(|(l, _)| self.selected.get() == Some(l.id)) {
            if self.on_handle(l, x, y) {
                let (px, py) = self.to_design(x, y);
                let d0 = (px - l.x).hypot(py - l.y).max(4.0);
                *self.drag.borrow_mut() = Some(Drag::TextScale { id: l.id, d0, start: l.clone() });
                return;
            }
        }
        let (px, py) = self.to_design(x, y);
        match shown.iter().rev().find(|(l, _)| self.hit(l, px, py)) {
            Some((l, _)) => {
                self.select(Some(l.id));
                *self.drag.borrow_mut() = Some(Drag::TextMove { id: l.id, x, y, start: l.clone() });
            }
            None => {
                self.select(None);
                self.toggle();
            }
        }
    }

    fn drag_to(self: &Rc<Self>, x: f64, y: f64) {
        let Some(drag) = self.drag.borrow().clone() else { return };
        let (_, _, _, _, ds) = self.frame_rect();
        if ds <= 0.0 {
            return;
        }
        match drag {
            Drag::Crop { handle, x: x0, y: y0, start } => {
                let s = self.doc();
                let src = self.src.get();
                let o = model::oriented(src, s.rotate);
                let min = (o.w.min(o.h) * 0.04).max(16.0);
                let n = model::resize_rect(start, handle, (x - x0) / ds, (y - y0) / ds, o, model::aspect_ratio(s.aspect), min);
                let crop = model::unorient_rect(n, src, s.rotate, s.flip_h, s.flip_v);
                self.change(|s| EditState { crop: Some(crop), ..s.clone() }, Some("crop"));
            }
            Drag::TextMove { id, x: x0, y: y0, start } => {
                let d = self.design();
                let nx = (start.x + (x - x0) / ds).clamp(0.0, d.w);
                let ny = (start.y + (y - y0) / ds).clamp(0.0, d.h);
                self.change_text(id, "move", |t| {
                    t.layer.x = nx;
                    t.layer.y = ny;
                });
            }
            Drag::TextScale { id, d0, start } => {
                let d = self.design();
                let (px, py) = self.to_design(x, y);
                let k = ((px - start.x).hypot(py - start.y) / d0).max(0.1);
                let size = (start.size * k).clamp(6.0, d.h * 1.5);
                let f = size / start.size;
                self.change_text(id, "scale", |t| {
                    t.layer.size = size;
                    t.layer.stroke = start.stroke.map(|mut o| {
                        o.width *= f;
                        o
                    });
                    t.layer.background = start.background.map(|mut b| {
                        b.padding *= f;
                        b.radius *= f;
                        b
                    });
                });
            }
            _ => {}
        }
    }

    fn release(&self) {
        if self.drag.borrow_mut().take().is_some() {
            self.hist.borrow_mut().seal();
        }
    }

    fn double_click(self: &Rc<Self>, x: f64, y: f64) {
        let (px, py) = self.to_design(x, y);
        let Some((l, _)) = self.shown().into_iter().rev().find(|(l, _)| self.hit(l, px, py)) else { return };
        *self.drag.borrow_mut() = None;
        self.select(Some(l.id));
        self.set_tool("text");
    }

    // ───────── timeline ─────────

    fn tl_time(&self, x: f64) -> f64 {
        let w = self.tl_w.get().max(1.0);
        (x / w * self.duration.get()).clamp(0.0, self.duration.get())
    }

    fn tl_press(self: &Rc<Self>, kind: &str, id: u32, part: &str, x: f64) {
        if !self.ready.get() {
            return;
        }
        match kind {
            "scrub" => {
                *self.drag.borrow_mut() = Some(Drag::Scrub);
                self.scrub(self.tl_time(x));
            }
            "in" => *self.drag.borrow_mut() = Some(Drag::TrimIn),
            "out" => *self.drag.borrow_mut() = Some(Drag::TrimOut),
            "text" => {
                let s = self.doc();
                let dur = model::out_duration(&s);
                let Some(t) = s.texts.iter().find(|t| t.layer.id == id) else { return };
                let (start, end) = model::text_window(t, dur);
                *self.drag.borrow_mut() = Some(Drag::Bar { id, part: part.to_string(), x0: x, start, end });
                self.selected.set(Some(id));
                self.set_tool("text");
                self.sync_text();
            }
            _ => {}
        }
    }

    fn tl_drag(self: &Rc<Self>, x: f64) {
        let Some(drag) = self.drag.borrow().clone() else { return };
        let t = self.tl_time(x);
        let duration = self.duration.get();
        match drag {
            Drag::Scrub => self.scrub(t),
            Drag::TrimIn => {
                let end = self.doc().trim.end;
                let start = t.clamp(0.0, end - model::MIN_TRIM);
                self.change(|s| EditState { trim: model::Trim { start, end }, ..s.clone() }, Some("trim"));
                self.scrub(start);
            }
            Drag::TrimOut => {
                let start = self.doc().trim.start;
                let end = t.clamp(start + model::MIN_TRIM, duration);
                self.change(|s| EditState { trim: model::Trim { start, end }, ..s.clone() }, Some("trim"));
                self.scrub(end);
            }
            Drag::Bar { id, part, x0, start, end } => {
                let s = self.doc();
                let pps = self.tl_w.get().max(1.0) / duration;
                let delta = (x - x0) / pps / s.speed;
                let (a0, b0) = model::drag_window(start, end, &part, delta, model::out_duration(&s));
                self.change_text(id, "bar", |t| {
                    t.start = a0;
                    t.end = b0;
                });
            }
            _ => {}
        }
    }

    fn tl_release(&self) {
        let drag = self.drag.borrow_mut().take();
        match drag {
            Some(Drag::Scrub) | Some(Drag::TrimIn) => self.end_scrub(self.time.get()),
            Some(Drag::TrimOut) => {
                let trim = self.doc().trim;
                self.end_scrub(trim.start.max(trim.end - 0.05));
            }
            _ => {}
        }
        self.hist.borrow_mut().seal();
    }

    // ───────── text layers ─────────

    fn select(&self, id: Option<u32>) {
        self.selected.set(id);
        self.sync_text();
    }

    fn add_text(self: &Rc<Self>) {
        let Some(a) = self.app() else { return };
        let s = self.doc();
        let d = model::base_size(self.src.get(), s.crop, s.rotate);
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let t = VideoText {
            layer: TextLayer {
                id,
                text: a.t("video.textDefault"),
                family: DEFAULT_FAMILY.into(),
                weight: 700.0,
                italic: false,
                size: (d.w.min(d.h) * 0.09).round().max(12.0),
                color: [255, 255, 255, 255],
                align: Align::Center,
                line_height: 1.15,
                letter_spacing: 0.0,
                x: d.w / 2.0,
                y: d.h * 0.78,
                rotation: 0.0,
                opacity: 1.0,
                stroke: None,
                shadow: Some(TextShadow { color: [0, 0, 0, 115], blur: 18.0, x: 0.0, y: 4.0 }),
                background: None,
            },
            start: 0.0,
            end: model::out_duration(&s),
        };
        self.selected.set(Some(id));
        self.change(
            |s| {
                let mut n = s.clone();
                n.texts.push(t);
                n
            },
            None,
        );
        self.sync_text();
    }

    fn duplicate(self: &Rc<Self>, id: u32) {
        let s = self.doc();
        let Some(src) = s.texts.iter().find(|t| t.layer.id == id).cloned() else { return };
        let d = model::base_size(self.src.get(), s.crop, s.rotate);
        let nid = self.next_id.get();
        self.next_id.set(nid + 1);
        let mut c = src.clone();
        c.layer.id = nid;
        c.layer.x = (src.layer.x + d.w * 0.04).min(d.w);
        c.layer.y = (src.layer.y + d.h * 0.04).min(d.h);
        self.selected.set(Some(nid));
        self.change(
            |s| {
                let mut n = s.clone();
                n.texts.push(c);
                n
            },
            None,
        );
        self.sync_text();
    }

    fn delete_text(self: &Rc<Self>, id: u32) {
        if self.selected.get() == Some(id) {
            self.selected.set(None);
        }
        self.change(
            |s| {
                let mut n = s.clone();
                n.texts.retain(|t| t.layer.id != id);
                n
            },
            None,
        );
    }

    fn with_selected(self: &Rc<Self>, key: &str, f: impl FnOnce(&mut VideoText)) {
        if let Some(id) = self.selected.get() {
            self.change_text(id, key, f);
        }
    }

    /// From / To boxes and the timing buttons (output seconds).
    fn set_window(self: &Rc<Self>, which: &str, v: f64) {
        let s = self.doc();
        let dur = model::out_duration(&s);
        let Some(t) = self.selected.get().and_then(|id| s.texts.iter().find(|t| t.layer.id == id)) else { return };
        let (a0, b0) = model::text_window(t, dur);
        let v = v.clamp(0.0, dur);
        let (start, end) = match which {
            "start" => (v, (v + 0.1).max(b0)),
            "end" => (a0.min(v - 0.1), v),
            "start-here" => (v.min(b0 - 0.1), b0),
            "end-here" => (a0, v.max(a0 + 0.1)),
            _ => (0.0, dur),
        };
        self.with_selected("time", |t| {
            t.start = start;
            t.end = end;
        });
    }

    fn now_out(&self) -> f64 {
        let s = self.doc();
        model::src_to_out(&s, self.time.get()).clamp(0.0, model::out_duration(&s))
    }

    // ───────── export job card ─────────

    fn job_busy(&self) -> bool {
        matches!(self.job.borrow().status.as_str(), "preparing" | "queued" | "running")
    }

    fn publish_job(&self) {
        let busy = self.job_busy();
        let j = self.job.borrow();
        self.with_vs(|vs| {
            vs.set_job_status(if j.status.is_empty() { "idle".into() } else { j.status.clone().into() });
            vs.set_job_progress(j.progress as f32);
            vs.set_job_title(j.title.clone().into());
            vs.set_job_error(j.error.clone().into());
            vs.set_busy(busy);
            vs.set_can_cancel(busy && j.id.is_some());
        });
    }
}

/// Open the editor on one video.
pub(crate) fn open(a: &Rc<AppInner>, item: MediaItem) {
    if let Some(c) = ctl() {
        c.open(a, item);
    }
}

/// Stop playback before thread-local storage is torn down (see explore_player::shutdown).
pub(crate) fn shutdown() {
    if let Some(c) = ctl() {
        c.tick.stop();
        drop(c.player.borrow_mut().take());
    }
}

/// A font was registered: text drawn with a fallback is drawn again, the picker lists it.
pub(crate) fn fonts_changed() {
    let Some(c) = ctl() else { return };
    c.publish_fonts();
    if c.ready.get() {
        c.rasters.borrow_mut().clear();
        c.publish_layers();
    }
}

/// Script step `vestate`: the editor's state on stderr.
pub(crate) fn debug_state() {
    let Some(c) = ctl() else { return };
    let s = c.doc();
    let st = c.player.borrow().as_ref().map(|p| p.status());
    let j = c.job.borrow();
    eprintln!(
        "[veditor] ready={} src={:?} dur={} fps={} audio={} tool={} time={:.2} playing={} status={:?} trim={:?} crop={:?} rot={} flip=({},{}) speed={} vol={} mute={} adjust={:?} texts={:?} output={:?} selected={:?} undo={} job=({} {:.2} {:?} {})",
        c.ready.get(),
        c.src.get(),
        c.duration.get(),
        c.fps.get(),
        c.has_audio.get(),
        c.tool.borrow(),
        c.time.get(),
        c.playing.get(),
        st,
        s.trim,
        s.crop,
        s.rotate,
        s.flip_h,
        s.flip_v,
        s.speed,
        s.volume,
        s.mute,
        s.adjust,
        s.texts.iter().map(|t| (t.layer.id, t.layer.text.as_str(), t.layer.x.round(), t.layer.y.round(), t.layer.size.round(), t.start, t.end)).collect::<Vec<_>>(),
        s.output,
        c.selected.get(),
        c.hist.borrow().can_undo(),
        j.status,
        j.progress,
        j.result,
        j.error,
    );
}

pub(crate) fn install(app: &Rc<AppInner>) {
    let lang = app.lang();
    let sliders = model::ADJUST_KEYS
        .iter()
        .map(|(key, label, lo, hi)| {
            let k = if *key == "hue" { 1.0 } else { 100.0 };
            EdSlider {
                key: (*key).into(),
                label: crate::i18n::t(lang, label).into(),
                heading: SharedString::new(),
                min: (lo * k) as f32,
                max: (hi * k) as f32,
                value: f32::NAN,
                text: SharedString::new(),
                has_hint: false,
            }
        })
        .collect::<Vec<_>>();
    let c = Rc::new(Ctl {
        app: Rc::downgrade(app),
        gen: Cell::new(0),
        item: RefCell::new(None),
        input: RefCell::new(None),
        src: Cell::new(Size { w: 1280.0, h: 720.0 }),
        duration: Cell::new(1.0),
        fps: Cell::new(30.0),
        has_audio: Cell::new(false),
        ready: Cell::new(false),
        hist: RefCell::new(History::new(model::initial_state(1.0))),
        tool: RefCell::new("trim".into()),
        selected: Cell::new(None),
        next_id: Cell::new(1),
        stage: Cell::new((0.0, 0.0)),
        tl_w: Cell::new(0.0),
        player: RefCell::new(None),
        tick: slint::Timer::default(),
        filter_timer: slint::Timer::default(),
        scrub_timer: slint::Timer::default(),
        strip_timer: slint::Timer::default(),
        last_status: Cell::new(Instant::now()),
        last_seek: Cell::new(Instant::now()),
        pending_seek: Cell::new(None),
        time: Cell::new(0.0),
        playing: Cell::new(false),
        scrubbing: Cell::new(false),
        was_playing: Cell::new(false),
        visible_key: RefCell::new(String::new()),
        drag: RefCell::new(None),
        fonts: RefCell::new(None),
        rasters: RefCell::new(HashMap::new()),
        strip_n: Cell::new(0),
        disabled: RefCell::new(String::new()),
        suffix: RefCell::new("_edit".into()),
        job: RefCell::new(JobRun::default()),
        sliders: Rc::new(VecModel::from(sliders)),
        layer_vis: Rc::new(VecModel::default()),
        lang: Cell::new(Some(lang)),
    });
    CTL.with(|x| *x.borrow_mut() = Some(c.clone()));
    let s = app.window.global::<VideoEditorState>();
    s.set_sliders(ModelRc::from(c.sliders.clone()));
    s.set_layers(ModelRc::from(c.layer_vis.clone()));
    macro_rules! on {
        ($cb:ident, |$c:ident $(, $arg:ident)*| $body:expr) => {{
            let w = Rc::downgrade(&c);
            s.$cb(move |$($arg),*| {
                if let Some($c) = w.upgrade() {
                    $body;
                }
            });
        }};
    }
    on!(on_close, |c| c.close());
    on!(on_undo, |c| {
        if c.hist.borrow_mut().undo() {
            c.after_change();
            c.sync_text();
        }
    });
    on!(on_redo, |c| {
        if c.hist.borrow_mut().redo() {
            c.after_change();
            c.sync_text();
        }
    });
    on!(on_reset, |c| {
        let d = c.duration.get();
        c.selected.set(None);
        c.change(|_| model::initial_state(d), None);
        c.sync_text();
    });
    on!(on_set_tool, |c, t| c.set_tool(&t));
    on!(on_resized, |c, w, h| {
        let n = (w as f64, h as f64);
        if c.stage.get() != n {
            c.stage.set(n);
            c.publish_stage();
            c.sync_player();
        }
    });
    on!(on_toggle_play, |c| c.toggle());
    on!(on_play, |c| c.play());
    on!(on_step, |c, n| c.step(n));
    on!(on_seek_by, |c, d| c.seek_to(c.time.get() + d as f64, c.playing.get()));
    on!(on_set_in, |c| {
        if let Some(tr) = model::set_in(&c.doc(), c.time.get()) {
            c.change(|s| EditState { trim: tr, ..s.clone() }, None);
        }
    });
    on!(on_set_out, |c| {
        if let Some(tr) = model::set_out(&c.doc(), c.time.get(), c.duration.get()) {
            c.change(|s| EditState { trim: tr, ..s.clone() }, None);
        }
    });
    on!(on_toggle_mute, |c| c.change(|s| EditState { mute: !s.mute, ..s.clone() }, None));
    on!(on_escape, |c| c.escape());
    on!(on_press, |c, mode, x, y| c.press(&mode, x as f64, y as f64));
    on!(on_drag, |c, x, y| c.drag_to(x as f64, y as f64));
    on!(on_release, |c| c.release());
    on!(on_double_click, |c, x, y| c.double_click(x as f64, y as f64));
    on!(on_tl_resized, |c, w| {
        if (c.tl_w.get() - w as f64).abs() > 0.5 {
            c.tl_w.set(w as f64);
            c.publish_timeline();
            c.request_strip();
        }
    });
    on!(on_tl_press, |c, kind, id, part, x| c.tl_press(&kind, id as u32, &part, x as f64));
    on!(on_tl_drag, |c, x| c.tl_drag(x as f64));
    on!(on_tl_release, |c| c.tl_release());
    on!(on_trim_num, |c, which, text| {
        let Some(v) = parse_num(&text) else { return };
        let (tr, d) = (c.doc().trim, c.duration.get());
        let trim = if which == "start" {
            model::Trim { start: v.clamp(0.0, (tr.end - model::MIN_TRIM).max(0.0)), end: tr.end }
        } else {
            model::Trim { start: tr.start, end: v.clamp(tr.start + model::MIN_TRIM, d) }
        };
        c.change(|s| EditState { trim, ..s.clone() }, None);
    });
    on!(on_trim_reset, |c| {
        let d = c.duration.get();
        c.change(|s| EditState { trim: model::Trim { start: 0.0, end: d }, ..s.clone() }, None);
    });
    on!(on_aspect_pick, |c, key| {
        let src = c.src.get();
        c.change(|s| model::pick_aspect(s, src, &key), None);
    });
    on!(on_crop_reset, |c| c.change(|s| EditState { crop: None, aspect: "free", ..s.clone() }, None));
    on!(on_rotate, |c, dir| c.change(|s| model::rotate_state(s, dir), None));
    on!(on_flip, |c, h| {
        c.change(|s| if h { EditState { flip_h: !s.flip_h, ..s.clone() } } else { EditState { flip_v: !s.flip_v, ..s.clone() } }, None)
    });
    on!(on_look, |c, i| {
        if let Some((_, a)) = model::looks().get(i as usize) {
            let a = *a;
            c.change(|s| EditState { adjust: a, ..s.clone() }, None);
        }
    });
    on!(on_adj, |c, key, v| {
        let v = if key == "hue" { v as f64 } else { v as f64 / 100.0 };
        let k = format!("adj.{key}");
        c.change(
            |s| {
                let mut n = s.clone();
                n.adjust.set(&key, v);
                n
            },
            Some(&k),
        );
    });
    on!(on_adj_reset, |c, key| {
        c.change(
            |s| {
                let mut n = s.clone();
                n.adjust.set(&key, 0.0);
                n
            },
            None,
        )
    });
    on!(on_adj_reset_all, |c| c.change(|s| EditState { adjust: model::Adjust::default(), ..s.clone() }, None));
    on!(on_add_text, |c| c.add_text());
    on!(on_select_text, |c, id| c.select(Some(id as u32)));
    on!(on_dup_text, |c, id| c.duplicate(id as u32));
    on!(on_delete_text, |c, id| c.delete_text(id as u32));
    on!(on_delete_selected, |c| {
        if let Some(id) = c.selected.get() {
            c.delete_text(id);
        }
    });
    on!(on_text_num, |c, key, v| c.with_selected(&key, |t| textprops::set_num(&mut t.layer, &key, v as f64)));
    on!(on_text_str, |c, key, v| c.with_selected(&key, |t| textprops::set_str(&mut t.layer, &key, &v)));
    on!(on_text_color, |c, key, v| c.with_selected(&key, |t| textprops::set_color(&mut t.layer, &key, v)));
    on!(on_text_toggle, |c, key, v| c.with_selected(&key, |t| textprops::toggle(&mut t.layer, &key, v)));
    on!(on_time_num, |c, which, text| {
        if let Some(v) = parse_num(&text) {
            c.set_window(&which, v);
        }
    });
    on!(on_start_here, |c| c.set_window("start-here", c.now_out()));
    on!(on_end_here, |c| c.set_window("end-here", c.now_out()));
    on!(on_whole_clip, |c| c.set_window("whole", 0.0));
    on!(on_set_speed, |c, v| {
        let v = (v as f64).clamp(0.25, 4.0);
        c.change(|s| EditState { speed: v, ..s.clone() }, Some("speed"));
    });
    on!(on_set_mute, |c, m| c.change(|s| EditState { mute: m, ..s.clone() }, None));
    on!(on_set_volume, |c, v| {
        let v = (v as f64 / 100.0).clamp(0.0, 2.0);
        c.change(|s| EditState { volume: v, mute: false, ..s.clone() }, Some("volume"));
    });
    on!(on_set_format, |c, f| {
        let format = match f.as_str() {
            "webm" => VideoFormat::Webm,
            "gif" => VideoFormat::Gif,
            _ => VideoFormat::Mp4,
        };
        c.change(|s| EditState { output: model::Output { format, ..s.output }, ..s.clone() }, None);
    });
    on!(on_set_short, |c, v| {
        let short = v.parse::<u32>().ok().filter(|v| *v > 0);
        c.change(|s| EditState { output: model::Output { short, ..s.output }, ..s.clone() }, None);
    });
    on!(on_set_quality, |c, q| {
        let quality = match q.as_str() {
            "medium" => VideoQuality::Medium,
            "low" => VideoQuality::Low,
            _ => VideoQuality::High,
        };
        c.change(|s| EditState { output: model::Output { quality, ..s.output }, ..s.clone() }, None);
    });
    on!(on_export, |c| c.export());
    on!(on_job_cancel, |c| c.cancel_job());
    on!(on_job_open, |c| c.open_result());
    on!(on_job_close, |c| {
        if !c.job_busy() {
            *c.job.borrow_mut() = JobRun::default();
            c.publish_job();
        }
    });
}
