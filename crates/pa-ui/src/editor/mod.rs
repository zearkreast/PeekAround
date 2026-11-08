//! Photo editor (features/photo-editor): opened by the "edit" action on one photo. The document
//! (doc.rs) is rendered by one shader (render.rs) on the GPU through wgpu (gpu.rs) or on the
//! CPU without an adapter: the preview at stage resolution, the save at full resolution, which
//! goes to `POST /api/edits/save` as a copy next to the original.

mod doc;
#[cfg(feature = "gpu")]
mod gpu;
mod render;

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use image::RgbaImage;
use pa_core::MediaItem;
use slint::{ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};

use crate::actions_ops::ImageSize;
use crate::app::AppInner;
use crate::backend::ThumbData;
use crate::i18n::Lang;
use crate::{EdAspect, EdFilter, EdSlider, EditorState, ToastKind};
use doc::{EditDoc, Handle, History, Rect, View};
use render::{RenderParams, Renderer};

/// filter thumbnail size (device px)
const THUMB: u32 = 168;

type SharedRenderer = Arc<Mutex<Option<Box<dyn Renderer>>>>;

#[derive(Clone, Copy)]
enum Drag {
    Pan { x: f64, y: f64, view: View },
    Crop { mode: Handle, x: f64, y: f64, start: Rect },
}

struct Ctl {
    app: Weak<AppInner>,
    renderer: SharedRenderer,
    item: RefCell<Option<MediaItem>>,
    hist: RefCell<History<EditDoc>>,
    saved: RefCell<EditDoc>,
    /// natural source size once loaded
    src: Cell<Option<(f64, f64)>>,
    /// bumps on every open: late results of an earlier photo are dropped
    gen: Cell<u64>,
    stage: Cell<(f64, f64)>,
    /// None = fit, else stage px per frame px
    zoom: Cell<Option<f64>>,
    view: Cell<View>,
    drag: RefCell<Option<Drag>>,
    compare: Cell<bool>,
    tab: RefCell<String>,
    rendering: Cell<bool>,
    render_again: Cell<bool>,
    thumbs_timer: slint::Timer,
    thumbs_busy: Cell<bool>,
    thumbs_again: Cell<bool>,
    saving: Cell<bool>,
    format: RefCell<String>,
    quality: Cell<u32>,
    sliders: Rc<VecModel<EdSlider>>,
    filters: Rc<VecModel<EdFilter>>,
    aspects: Rc<VecModel<EdAspect>>,
    /// language of the labels in the models
    lang: Cell<Lang>,
}

thread_local! {
    static CTL: RefCell<Option<Rc<Ctl>>> = const { RefCell::new(None) };
}

fn ctl() -> Option<Rc<Ctl>> {
    CTL.with(|c| c.borrow().clone())
}

fn signed(v: f32) -> String {
    let r = (v * 10.0).round() / 10.0;
    if r > 0.0 {
        format!("+{r}")
    } else {
        format!("{r}")
    }
}

/// sliders with a colour swatch (adjust.ts `hint`; the gradients are in editor.slint)
fn has_hint(key: &str) -> bool {
    matches!(key, "saturation" | "vibrance" | "temperature" | "tint" | "hue")
}

/// Decode the original with its EXIF orientation applied.
fn decode_full(data: ThumbData) -> Result<RgbaImage, String> {
    use image::ImageDecoder;
    match data {
        ThumbData::Encoded(bytes) => {
            let mut dec = image::ImageReader::new(std::io::Cursor::new(&bytes[..]))
                .with_guessed_format()
                .map_err(|e| e.to_string())?
                .into_decoder()
                .map_err(|e| e.to_string())?;
            let orientation = dec.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
            let mut img = image::DynamicImage::from_decoder(dec).map_err(|e| e.to_string())?;
            img.apply_orientation(orientation);
            Ok(img.into_rgba8())
        }
        ThumbData::Rgba { width, height, pixels } => RgbaImage::from_raw(width, height, pixels).ok_or_else(|| "bad pixels".into()),
    }
}

fn to_image(px: Vec<u8>, w: u32, h: u32) -> Image {
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
    buf.make_mut_bytes().copy_from_slice(&px);
    Image::from_rgba8_premultiplied(buf)
}

impl Ctl {
    fn app(&self) -> Option<Rc<AppInner>> {
        self.app.upgrade()
    }

    fn doc(&self) -> EditDoc {
        self.hist.borrow().doc.clone()
    }

    fn frame(&self) -> (f64, f64) {
        let (w, h) = self.src.get().unwrap_or((1600.0, 1067.0));
        doc::frame_size(w, h, self.hist.borrow().doc.geo.rot)
    }

    fn crop_mode(&self) -> bool {
        *self.tab.borrow() == "crop"
    }

    // ───────── document ─────────

    fn change(self: &Rc<Self>, f: impl FnOnce(&EditDoc) -> EditDoc, key: Option<&str>) {
        if self.src.get().is_none() {
            return;
        }
        let next = f(&self.hist.borrow().doc);
        self.hist.borrow_mut().change(next, key);
        self.after_change();
    }

    fn after_change(self: &Rc<Self>) {
        self.update_view();
        self.publish();
        self.request_render();
        self.request_thumbs();
    }

    fn publish(&self) {
        let Some(a) = self.app() else { return };
        let s = a.window.global::<EditorState>();
        let d = self.doc();
        let h = self.hist.borrow();
        s.set_can_undo(h.can_undo());
        s.set_can_redo(h.can_redo());
        drop(h);
        s.set_dirty(d != *self.saved.borrow());
        s.set_adjust_neutral(d.adjust.iter().all(|v| *v == 0.0));
        // sliders: rows are updated in place so a drag in progress keeps its touch area
        for i in 0..doc::KEYS.len() {
            let v = d.adjust[i];
            if let Some(mut r) = self.sliders.row_data(i) {
                if r.value != v {
                    r.value = v;
                    r.text = if doc::KEYS[i] == "hue" {
                        format!("{}°", signed(v))
                    } else if doc::RANGE[i].0 < 0.0 {
                        signed(v)
                    } else {
                        format!("{v}")
                    }
                    .into();
                    self.sliders.set_row_data(i, r);
                }
            }
        }
        let active = d.filter.id.clone().unwrap_or_default();
        for i in 0..self.filters.row_count() {
            if let Some(mut r) = self.filters.row_data(i) {
                let on = r.id.as_str() == active;
                if r.active != on {
                    r.active = on;
                    self.filters.set_row_data(i, r);
                }
            }
        }
        s.set_has_filter(d.filter.id.is_some());
        s.set_strength(d.filter.strength);
        let (fw, fh) = self.frame();
        for i in 0..self.aspects.row_count() {
            if let Some(mut r) = self.aspects.row_data(i) {
                let on = r.key.as_str() == d.geo.aspect;
                let mut changed = r.active != on;
                r.active = on;
                if r.key == "original" {
                    let (w, h) = aspect_box(fw / fh);
                    if r.w != w || r.h != h {
                        (r.w, r.h) = (w, h);
                        changed = true;
                    }
                }
                if changed {
                    self.aspects.set_row_data(i, r);
                }
            }
        }
        s.set_straighten(d.geo.straighten as f32);
        let c = doc::crop_px(&d.geo, fw, fh);
        s.set_output_size(format!("{} × {}", c.w, c.h).into());
        s.set_crop_untouched(d.geo.crop_untouched());
        self.publish_crop();
    }

    /// the crop box on the stage (logical px)
    fn publish_crop(&self) {
        let Some(a) = self.app() else { return };
        let s = a.window.global::<EditorState>();
        let d = self.doc();
        let (fw, fh) = self.frame();
        let v = self.view.get();
        let (sw, sh) = self.stage.get();
        let r = d.geo.crop;
        let x = (r.x * fw - v.cx) * v.scale + sw / 2.0;
        let y = (r.y * fh - v.cy) * v.scale + sh / 2.0;
        s.set_crop_x(x as f32);
        s.set_crop_y(y as f32);
        s.set_crop_w((r.w * fw * v.scale) as f32);
        s.set_crop_h((r.h * fh * v.scale) as f32);
        let fit = self.fit_view();
        s.set_zoom_fit(self.zoom.get().is_none());
        let sf = a.window.window().scale_factor() as f64;
        s.set_zoom_100(self.zoom.get().is_some_and(|z| (z - 1.0 / sf).abs() < 1e-6));
        s.set_can_pan(!self.crop_mode() && v.scale > fit.scale + 1e-9);
    }

    // ───────── view ─────────

    fn region(&self) -> Rect {
        let (fw, fh) = self.frame();
        if self.crop_mode() {
            Rect { x: 0.0, y: 0.0, w: fw, h: fh }
        } else {
            doc::crop_px(&self.doc().geo, fw, fh)
        }
    }

    fn fit_view(&self) -> View {
        let (sw, sh) = self.stage.get();
        let pad = if sw < 640.0 {
            14.0
        } else if self.crop_mode() {
            44.0
        } else {
            32.0
        };
        doc::fit_view(self.region(), sw, sh, pad)
    }

    fn update_view(&self) {
        let (sw, sh) = self.stage.get();
        let fit = self.fit_view();
        let v = match self.zoom.get() {
            Some(z) if !self.crop_mode() => {
                let cur = self.view.get();
                doc::clamp_view(View { scale: z, cx: cur.cx, cy: cur.cy }, self.region(), sw, sh)
            }
            _ => fit,
        };
        self.view.set(v);
    }

    // ───────── rendering ─────────

    fn preview_params(&self) -> Option<(RenderParams, u32, u32)> {
        let a = self.app()?;
        let (src_w, src_h) = self.src.get()?;
        let (sw, sh) = self.stage.get();
        if sw < 1.0 || sh < 1.0 {
            return None;
        }
        let sf = a.window.window().scale_factor() as f64;
        let d = self.doc();
        let (fw, fh) = doc::frame_size(src_w, src_h, d.geo.rot);
        let crop = doc::crop_px(&d.geo, fw, fh);
        let v = self.view.get();
        let k = 1.0 / (v.scale * sf);
        let adjust = if self.compare.get() {
            doc::neutral_render()
        } else {
            doc::effective_adjust(&d.adjust, Some(&d.filter))
        };
        let p = RenderParams {
            adjust,
            src_w,
            src_h,
            rot: d.geo.rot,
            flip_h: d.geo.flip_h,
            flip_v: d.geo.flip_v,
            straighten: d.geo.straighten,
            frame_w: fw,
            frame_h: fh,
            ox: v.cx - sw / 2.0 / v.scale,
            oy: v.cy - sh / 2.0 / v.scale,
            k,
            clip: if self.crop_mode() { Rect { x: 0.0, y: 0.0, w: fw, h: fh } } else { crop },
            crop,
            grain_cell: doc::grain_cell_for(crop).max(k),
            sharpen_step: k.max(1.0),
        };
        Some((p, (sw * sf).round().max(1.0) as u32, (sh * sf).round().max(1.0) as u32))
    }

    /// Render the preview; while one is in flight the latest state renders right after it.
    fn request_render(self: &Rc<Self>) {
        if self.rendering.get() {
            self.render_again.set(true);
            return;
        }
        let Some(a) = self.app() else { return };
        let Some((p, w, h)) = self.preview_params() else { return };
        self.rendering.set(true);
        self.render_again.set(false);
        let r = self.renderer.clone();
        let gen = self.gen.get();
        let me = Rc::downgrade(self);
        a.rt.spawn_then(
            async move {
                tokio::task::spawn_blocking(move || {
                    let mut g = r.lock().unwrap();
                    g.as_mut().filter(|r| r.has_source()).map(|r| r.render(&p, w, h))
                })
                .await
                .ok()
                .flatten()
            },
            move |px| {
                let Some(c) = me.upgrade() else { return };
                c.rendering.set(false);
                if c.gen.get() != gen {
                    return;
                }
                if let (Some(px), Some(a)) = (px, c.app()) {
                    a.window.global::<EditorState>().set_preview(to_image(px, w, h));
                }
                if c.render_again.get() {
                    c.request_render();
                }
            },
        );
    }

    /// Filter thumbnails (debounced): each preset on the current photo with the user's sliders + crop.
    fn request_thumbs(self: &Rc<Self>) {
        let me = Rc::downgrade(self);
        self.thumbs_timer.start(slint::TimerMode::SingleShot, Duration::from_millis(120), move || {
            if let Some(c) = me.upgrade() {
                c.render_thumbs();
            }
        });
    }

    fn render_thumbs(self: &Rc<Self>) {
        if *self.tab.borrow() != "filters" {
            return;
        }
        if self.thumbs_busy.get() {
            self.thumbs_again.set(true);
            return;
        }
        let Some(a) = self.app() else { return };
        let Some((src_w, src_h)) = self.src.get() else { return };
        let d = self.doc();
        let (fw, fh) = doc::frame_size(src_w, src_h, d.geo.rot);
        let crop = doc::crop_px(&d.geo, fw, fh);
        let side = crop.w.min(crop.h);
        let k = side / THUMB as f64;
        let mut jobs = Vec::new();
        for i in 0..self.filters.row_count() {
            let Some(row) = self.filters.row_data(i) else { continue };
            let id = (!row.id.is_empty()).then(|| row.id.to_string());
            jobs.push(RenderParams {
                adjust: doc::effective_adjust(&d.adjust, Some(&doc::Filter { id, strength: 100.0 })),
                src_w,
                src_h,
                rot: d.geo.rot,
                flip_h: d.geo.flip_h,
                flip_v: d.geo.flip_v,
                straighten: d.geo.straighten,
                frame_w: fw,
                frame_h: fh,
                ox: crop.x + (crop.w - side) / 2.0,
                oy: crop.y + (crop.h - side) / 2.0,
                k,
                clip: crop,
                crop,
                grain_cell: k.max(1.0),
                sharpen_step: k.max(1.0),
            });
        }
        self.thumbs_busy.set(true);
        self.thumbs_again.set(false);
        let r = self.renderer.clone();
        let gen = self.gen.get();
        let me = Rc::downgrade(self);
        a.rt.spawn_then(
            async move {
                tokio::task::spawn_blocking(move || {
                    let mut g = r.lock().unwrap();
                    let r = g.as_mut().filter(|r| r.has_source())?;
                    Some(jobs.iter().map(|p| r.render(p, THUMB, THUMB)).collect::<Vec<_>>())
                })
                .await
                .ok()
                .flatten()
            },
            move |out| {
                let Some(c) = me.upgrade() else { return };
                c.thumbs_busy.set(false);
                if c.gen.get() != gen {
                    return;
                }
                for (i, px) in out.into_iter().flatten().enumerate() {
                    if let Some(mut row) = c.filters.row_data(i) {
                        row.thumb = to_image(px, THUMB, THUMB);
                        c.filters.set_row_data(i, row);
                    }
                }
                if c.thumbs_again.get() {
                    c.render_thumbs();
                }
            },
        );
    }

    // ───────── open / close ─────────

    fn relabel(&self, lang: Lang) {
        if self.lang.replace(lang) == lang {
            return;
        }
        let t = |k: &str| crate::i18n::t(lang, k);
        for i in 0..self.sliders.row_count() {
            if let Some(mut r) = self.sliders.row_data(i) {
                r.label = t(&format!("photo.adj.{}", r.key)).into();
                if let Some((title, ..)) = doc::GROUPS.iter().find(|g| g.1 == i) {
                    r.heading = t(title).to_uppercase().into();
                }
                self.sliders.set_row_data(i, r);
            }
        }
        for i in 0..self.filters.row_count() {
            if let Some(mut r) = self.filters.row_data(i) {
                r.label = match doc::filter(&r.id) {
                    Some(f) => t(f.label),
                    None => t("photo.filter.none"),
                }
                .into();
                self.filters.set_row_data(i, r);
            }
        }
        for i in 0..self.aspects.row_count() {
            if let Some(mut r) = self.aspects.row_data(i) {
                match r.key.as_str() {
                    "free" => r.label = t("photo.crop.free").into(),
                    "original" => r.label = t("photo.crop.original").into(),
                    _ => continue,
                }
                self.aspects.set_row_data(i, r);
            }
        }
    }

    fn open(self: &Rc<Self>, a: &Rc<AppInner>, item: MediaItem) {
        let Some(ops) = a.backend.ops() else { return };
        self.gen.set(self.gen.get() + 1);
        let gen = self.gen.get();
        *self.hist.borrow_mut() = History::new(EditDoc::default());
        *self.saved.borrow_mut() = EditDoc::default();
        self.src.set(None);
        self.zoom.set(None);
        self.compare.set(false);
        *self.drag.borrow_mut() = None;
        *self.tab.borrow_mut() = "adjust".into();
        self.relabel(a.lang());
        let base = match item.name.rfind('.') {
            Some(i) if i > 0 => item.name[..i].to_string(),
            _ => item.name.clone(),
        };
        let s = a.window.global::<EditorState>();
        s.set_name(item.name.clone().into());
        s.set_file_base(base.into());
        s.set_loading(true);
        s.set_error(SharedString::new());
        s.set_preview(Image::default());
        s.set_compare(false);
        s.set_tab("adjust".into());
        s.set_save_open(false);
        s.set_saving(false);
        s.set_confirm_leave(false);
        s.set_format(self.format.borrow().clone().into());
        s.set_quality(self.quality.get() as f32);
        for i in 0..self.filters.row_count() {
            if let Some(mut r) = self.filters.row_data(i) {
                r.thumb = Image::default();
                self.filters.set_row_data(i, r);
            }
        }
        s.set_open(true);
        *self.item.borrow_mut() = Some(item.clone());
        self.publish();

        // the save popover previews the server's edit suffix
        crate::api::call::<serde_json::Value>(a, "GET", "/api/settings", None, |a, r| {
            if let Ok(v) = r {
                if let Some(sfx) = v.get("editSuffix").and_then(|x| x.as_str()) {
                    a.window.global::<EditorState>().set_file_suffix(sfx.into());
                }
            }
        });

        let fut = ops.image(&item.id, ImageSize::Full);
        let r = self.renderer.clone();
        let me = Rc::downgrade(self);
        a.rt.spawn_then(
            async move {
                let data = fut.await.map_err(|e| e.message)?;
                tokio::task::spawn_blocking(move || -> Result<(f64, f64, String), String> {
                    let img = Arc::new(decode_full(data)?);
                    let (w, h) = img.dimensions();
                    let mut g = r.lock().unwrap();
                    let rr = g.get_or_insert_with(render::create);
                    rr.set_source(img);
                    Ok((w as f64, h as f64, rr.name()))
                })
                .await
                .map_err(|e| e.to_string())?
            },
            move |res| {
                let Some(c) = me.upgrade() else { return };
                if c.gen.get() != gen {
                    return;
                }
                let Some(a) = c.app() else { return };
                let s = a.window.global::<EditorState>();
                s.set_loading(false);
                match res {
                    Ok((w, h, name)) => {
                        log::info!("photo editor: {w}×{h} on {name}");
                        c.src.set(Some((w, h)));
                        c.after_change();
                    }
                    Err(e) => {
                        log::warn!("photo editor: {e}");
                        s.set_error(a.t("photo.loadError").into());
                    }
                }
            },
        );
    }

    fn close(self: &Rc<Self>) {
        let Some(a) = self.app() else { return };
        let s = a.window.global::<EditorState>();
        if s.get_dirty() && !self.saving.get() {
            s.set_confirm_leave(true);
            return;
        }
        self.leave();
    }

    fn leave(self: &Rc<Self>) {
        let Some(a) = self.app() else { return };
        self.gen.set(self.gen.get() + 1);
        let s = a.window.global::<EditorState>();
        s.set_confirm_leave(false);
        s.set_open(false);
        s.set_preview(Image::default());
        self.src.set(None);
        *self.item.borrow_mut() = None;
        let r = self.renderer.clone();
        a.rt.handle().spawn_blocking(move || {
            if let Some(r) = r.lock().unwrap().as_mut() {
                r.clear_source();
            }
        });
    }

    // ───────── pointer ─────────

    fn press(self: &Rc<Self>, mode: &str, x: f64, y: f64) {
        if self.src.get().is_none() {
            return;
        }
        if mode == "pan" {
            let fit = self.fit_view();
            if self.view.get().scale > fit.scale + 1e-9 {
                *self.drag.borrow_mut() = Some(Drag::Pan { x, y, view: self.view.get() });
            }
            return;
        }
        let Some(h) = Handle::parse(mode) else { return };
        let (fw, fh) = self.frame();
        let c = self.doc().geo.crop;
        let start = Rect { x: c.x * fw, y: c.y * fh, w: c.w * fw, h: c.h * fh };
        *self.drag.borrow_mut() = Some(Drag::Crop { mode: h, x, y, start });
    }

    fn drag_to(self: &Rc<Self>, x: f64, y: f64) {
        let (sw, sh) = self.stage.get();
        let v = self.view.get();
        let drag = *self.drag.borrow();
        match drag {
            Some(Drag::Pan { x: x0, y: y0, view }) => {
                let n = View { cx: view.cx - (x - x0) / view.scale, cy: view.cy - (y - y0) / view.scale, ..view };
                self.view.set(doc::clamp_view(n, self.region(), sw, sh));
                self.publish_crop();
                self.request_render();
            }
            Some(Drag::Crop { mode, x: x0, y: y0, start }) => {
                let (fw, fh) = self.frame();
                let ratio = doc::aspect_ratio(self.doc().geo.aspect, fw, fh);
                let min = (48.0 / v.scale).max(16.0);
                let n = doc::resize_crop(start, mode, (x - x0) / v.scale, (y - y0) / v.scale, ratio, fw, fh, min);
                let crop = Rect { x: n.x / fw, y: n.y / fh, w: n.w / fw, h: n.h / fh };
                self.change(|d| EditDoc { geo: doc::Geo { crop, ..d.geo.clone() }, ..d.clone() }, Some("geo.crop"));
            }
            None => {}
        }
    }

    fn release(&self) {
        if self.drag.borrow_mut().take().is_some() {
            self.hist.borrow_mut().seal();
        }
    }

    fn wheel(self: &Rc<Self>, x: f64, y: f64, dy: f64) {
        if self.src.get().is_none() || self.crop_mode() {
            return;
        }
        let Some(a) = self.app() else { return };
        let sf = a.window.window().scale_factor() as f64;
        let (sw, sh) = self.stage.get();
        let v = self.view.get();
        let fit = self.fit_view();
        let scale = (v.scale * (dy * 0.0025).exp()).clamp(fit.scale, (8.0 / sf).max(fit.scale));
        // keep the frame point under the cursor in place
        let px = (x - sw / 2.0) / v.scale + v.cx;
        let py = (y - sh / 2.0) / v.scale + v.cy;
        let n = View { scale, cx: px - (x - sw / 2.0) / scale, cy: py - (y - sh / 2.0) / scale };
        self.zoom.set(if (scale - fit.scale).abs() < 1e-9 { None } else { Some(scale) });
        self.view.set(doc::clamp_view(n, self.region(), sw, sh));
        self.publish_crop();
        self.request_render();
    }

    fn zoom_to(self: &Rc<Self>, z: &str) {
        let Some(a) = self.app() else { return };
        let sf = a.window.window().scale_factor() as f64;
        self.zoom.set(if z == "100" { Some(1.0 / sf) } else { None });
        self.update_view();
        self.publish_crop();
        self.request_render();
    }

    // ───────── save ─────────

    fn save(self: &Rc<Self>) {
        if self.saving.get() {
            return;
        }
        let Some(a) = self.app() else { return };
        let Some(item) = self.item.borrow().clone() else { return };
        let Some((src_w, src_h)) = self.src.get() else { return };
        let d = self.doc();
        self.saving.set(true);
        let s = a.window.global::<EditorState>();
        s.set_saving(true);
        s.set_save_progress(0.02);
        s.set_save_label(a.t("photo.rendering").into());
        let gen = self.gen.get();
        let r = self.renderer.clone();
        let doc2 = d.clone();
        let rendering = a.t("photo.rendering");
        let encoding = a.t("photo.encoding");
        let render = async move {
            tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
                use image::ImageEncoder;
                let mut g = r.lock().unwrap();
                let rr = g.as_mut().filter(|r| r.has_source()).ok_or("no source")?;
                let img = render::render_full(rr.as_mut(), &doc2, src_w, src_h, &|p| progress(gen, p * 0.8, &rendering));
                drop(g);
                progress(gen, 0.82, &encoding);
                let mut out = Vec::new();
                image::codecs::png::PngEncoder::new_with_quality(
                    &mut out,
                    image::codecs::png::CompressionType::Fast,
                    image::codecs::png::FilterType::Adaptive,
                )
                .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgba8)
                .map_err(|e| e.to_string())?;
                Ok(out)
            })
            .await
            .map_err(|e| e.to_string())?
        };
        let me = Rc::downgrade(self);
        a.rt.spawn_then(render, move |png| {
            let Some(c) = me.upgrade() else { return };
            let Some(a) = c.app() else { return };
            let png = match png {
                Ok(p) => p,
                Err(e) => return c.saved_done(gen, &d, Err(e)),
            };
            let Some(ops) = a.backend.ops() else { return c.saved_done(gen, &d, Err("offline".into())) };
            let s = a.window.global::<EditorState>();
            s.set_save_progress(0.9);
            s.set_save_label(a.t("common.saving").into());
            let fmt = c.format.borrow().clone();
            let up = ops.save_edit(png, &item.id, &fmt, c.quality.get());
            let me = Rc::downgrade(&c);
            a.rt.spawn_then(async move { up.await.map_err(|e| e.message) }, move |res| {
                if let Some(c) = me.upgrade() {
                    c.saved_done(gen, &d, res);
                }
            });
        });
    }

    fn saved_done(self: &Rc<Self>, gen: u64, d: &EditDoc, res: Result<serde_json::Value, String>) {
        let Some(a) = self.app() else { return };
        self.saving.set(false);
        let s = a.window.global::<EditorState>();
        s.set_saving(false);
        match res {
            Ok(v) => {
                let name = v.pointer("/item/name").and_then(|n| n.as_str()).unwrap_or_default().to_string();
                if self.gen.get() == gen {
                    if self.hist.borrow().doc == *d {
                        *self.saved.borrow_mut() = d.clone();
                    }
                    s.set_save_progress(1.0);
                    s.set_save_open(false);
                    self.publish();
                }
                a.toast(ToastKind::Success, &a.t("photo.saved"), &name);
            }
            Err(e) => a.toast(ToastKind::Error, &a.t("common.error"), &e),
        }
    }
}

/// Save progress from the render thread.
fn progress(gen: u64, p: f32, label: &str) {
    let label = label.to_string();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(c) = ctl() else { return };
        if c.gen.get() != gen || !c.saving.get() {
            return;
        }
        if let Some(a) = c.app() {
            let s = a.window.global::<EditorState>();
            s.set_save_progress(p);
            s.set_save_label(label.into());
        }
    });
}

/// icon box (px) of an aspect preset
fn aspect_box(r: f64) -> (f32, f32) {
    if r >= 1.0 {
        (20.0, (20.0 / r) as f32)
    } else {
        ((20.0 * r) as f32, 20.0)
    }
}

/// Open the editor on one photo.
pub(crate) fn open(a: &Rc<AppInner>, item: MediaItem) {
    if let Some(c) = ctl() {
        c.open(a, item);
    }
}

/// Script step `estate`: the editor's state on stderr.
pub(crate) fn debug_state() {
    let Some(c) = ctl() else { return };
    let name = c.renderer.lock().map(|r| r.as_ref().map(|r| r.name())).ok().flatten();
    let d = c.doc();
    eprintln!(
        "[editor] renderer={:?} src={:?} tab={} view={:?} undo={} adjust={:?} filter={:?} geo={:?}",
        name,
        c.src.get(),
        c.tab.borrow(),
        c.view.get(),
        c.hist.borrow().can_undo(),
        d.adjust,
        d.filter,
        d.geo
    );
}

pub(crate) fn install(app: &Rc<AppInner>) {
    let lang = app.lang();
    let t = |k: &str| crate::i18n::t(lang, k);
    let mut sliders = Vec::new();
    for (title, first, n) in doc::GROUPS {
        for i in first..first + n {
            let key = doc::KEYS[i];
            sliders.push(EdSlider {
                key: key.into(),
                label: t(&format!("photo.adj.{key}")).into(),
                heading: if i == first { t(title).to_uppercase().into() } else { SharedString::new() },
                min: doc::RANGE[i].0,
                max: doc::RANGE[i].1,
                value: f32::NAN,
                text: SharedString::new(),
                has_hint: has_hint(key),
            });
        }
    }
    let mut filters = vec![EdFilter { id: SharedString::new(), label: t("photo.filter.none").into(), thumb: Image::default(), active: true }];
    for f in &doc::FILTERS {
        filters.push(EdFilter { id: f.id.into(), label: t(f.label).into(), thumb: Image::default(), active: false });
    }
    let aspects = doc::ASPECTS
        .iter()
        .map(|k| {
            let (w, h) = aspect_box(doc::aspect_ratio(k, 1.0, 1.0).unwrap_or(1.0));
            EdAspect {
                key: (*k).into(),
                label: match *k {
                    "free" => t("photo.crop.free").into(),
                    "original" => t("photo.crop.original").into(),
                    k => k.into(),
                },
                w,
                h,
                free: *k == "free",
                active: *k == "free",
            }
        })
        .collect::<Vec<_>>();
    let c = Rc::new(Ctl {
        app: Rc::downgrade(app),
        renderer: Arc::new(Mutex::new(None)),
        item: RefCell::new(None),
        hist: RefCell::new(History::new(EditDoc::default())),
        saved: RefCell::new(EditDoc::default()),
        src: Cell::new(None),
        gen: Cell::new(0),
        stage: Cell::new((0.0, 0.0)),
        zoom: Cell::new(None),
        view: Cell::new(View { scale: 1.0, cx: 0.0, cy: 0.0 }),
        drag: RefCell::new(None),
        compare: Cell::new(false),
        tab: RefCell::new("adjust".into()),
        rendering: Cell::new(false),
        render_again: Cell::new(false),
        thumbs_timer: slint::Timer::default(),
        thumbs_busy: Cell::new(false),
        thumbs_again: Cell::new(false),
        saving: Cell::new(false),
        format: RefCell::new("jpg".into()),
        quality: Cell::new(92),
        sliders: Rc::new(VecModel::from(sliders)),
        filters: Rc::new(VecModel::from(filters)),
        aspects: Rc::new(VecModel::from(aspects)),
        lang: Cell::new(lang),
    });
    CTL.with(|x| *x.borrow_mut() = Some(c.clone()));
    let s = app.window.global::<EditorState>();
    s.set_sliders(ModelRc::from(c.sliders.clone()));
    s.set_filters(ModelRc::from(c.filters.clone()));
    s.set_aspects(ModelRc::from(c.aspects.clone()));
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
    on!(on_leave, |c| c.leave());
    on!(on_stay, |c| {
        if let Some(a) = c.app() {
            a.window.global::<EditorState>().set_confirm_leave(false);
        }
    });
    on!(on_undo, |c| {
        if c.hist.borrow_mut().undo() {
            c.after_change();
        }
    });
    on!(on_redo, |c| {
        if c.hist.borrow_mut().redo() {
            c.after_change();
        }
    });
    on!(on_set_compare, |c, v| {
        if c.compare.get() != v && c.src.get().is_some() {
            c.compare.set(v);
            if let Some(a) = c.app() {
                a.window.global::<EditorState>().set_compare(v);
            }
            c.request_render();
        }
    });
    on!(on_set_tab, |c, t| {
        *c.tab.borrow_mut() = t.to_string();
        if let Some(a) = c.app() {
            a.window.global::<EditorState>().set_tab(t.clone());
        }
        if t == "crop" {
            c.zoom.set(None);
        }
        c.update_view();
        c.publish();
        c.request_render();
        if t == "filters" {
            c.request_thumbs();
        }
    });
    on!(on_slider, |c, key, v| {
        if let Some(i) = doc::key_index(&key) {
            let k = format!("adj.{key}");
            c.change(
                |d| {
                    let mut n = d.clone();
                    n.adjust[i] = v;
                    n
                },
                Some(&k),
            );
        }
    });
    on!(on_slider_reset, |c, key| {
        if let Some(i) = doc::key_index(&key) {
            c.change(
                |d| {
                    let mut n = d.clone();
                    n.adjust[i] = 0.0;
                    n
                },
                None,
            );
        }
    });
    on!(on_reset_all, |c| c.change(|d| EditDoc { adjust: [0.0; 15], ..d.clone() }, None));
    on!(on_pick_filter, |c, id| {
        c.change(
            |d| {
                let id = (!id.is_empty()).then(|| id.to_string());
                let strength = if d.filter.id == id { d.filter.strength } else { 100.0 };
                EditDoc { filter: doc::Filter { id, strength }, ..d.clone() }
            },
            None,
        );
    });
    on!(on_set_strength, |c, v| {
        c.change(|d| EditDoc { filter: doc::Filter { strength: v, ..d.filter.clone() }, ..d.clone() }, Some("filter.strength"));
    });
    on!(on_aspect, |c, key| {
        let Some(&k) = doc::ASPECTS.iter().find(|a| **a == key.as_str()) else { return };
        let (fw, fh) = c.frame();
        c.change(
            |d| {
                let crop = match doc::aspect_ratio(k, fw, fh) {
                    Some(r) => doc::fit_aspect(d.geo.crop, r, fw, fh),
                    None => d.geo.crop,
                };
                EditDoc { geo: doc::Geo { aspect: k, crop, ..d.geo.clone() }, ..d.clone() }
            },
            None,
        );
    });
    on!(on_rotate, |c, dir| c.change(|d| doc::rotate_doc(d, if dir < 0 { -1 } else { 1 }), None));
    on!(on_flip, |c, h| c.change(|d| doc::flip_doc(d, h), None));
    on!(on_set_straighten, |c, v| {
        c.change(|d| EditDoc { geo: doc::Geo { straighten: v as f64, ..d.geo.clone() }, ..d.clone() }, Some("geo.straighten"));
    });
    on!(on_reset_crop, |c| {
        c.change(
            |d| EditDoc { geo: doc::Geo { crop: doc::FULL_CROP, straighten: 0.0, aspect: "free", ..d.geo.clone() }, ..d.clone() },
            None,
        );
    });
    on!(on_press, |c, mode, x, y| c.press(&mode, x as f64, y as f64));
    on!(on_drag, |c, x, y| c.drag_to(x as f64, y as f64));
    on!(on_release, |c| c.release());
    on!(on_wheel, |c, x, y, dy| c.wheel(x as f64, y as f64, dy as f64));
    on!(on_zoom, |c, z| c.zoom_to(&z));
    on!(on_resized, |c, w, h| {
        let n = (w as f64, h as f64);
        if c.stage.get() != n {
            c.stage.set(n);
            c.update_view();
            c.publish_crop();
            c.request_render();
        }
    });
    on!(on_toggle_save, |c| {
        if let Some(a) = c.app() {
            let s = a.window.global::<EditorState>();
            if c.src.get().is_some() && !(s.get_save_open() && c.saving.get()) {
                s.set_save_open(!s.get_save_open());
            }
        }
    });
    on!(on_set_format, |c, f| {
        *c.format.borrow_mut() = f.to_string();
        if let Some(a) = c.app() {
            a.window.global::<EditorState>().set_format(f);
        }
    });
    on!(on_set_quality, |c, q| {
        c.quality.set(q.round().clamp(40.0, 100.0) as u32);
        if let Some(a) = c.app() {
            a.window.global::<EditorState>().set_quality(q);
        }
    });
    on!(on_save, |c| c.save());
}
