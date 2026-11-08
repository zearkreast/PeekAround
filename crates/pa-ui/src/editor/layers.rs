//! Text layers and drawing in the photo editor (panels/TextPanel, panels/DrawPanel, the Konva
//! layers of EditorStage): the layer list and the selected layer's properties, the layer images
//! on the stage with the move / resize / rotate gestures, and freehand brush strokes.

use std::cell::RefMut;
use std::rc::Rc;

use slint::{Color, ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};

use super::doc::{self, Align, EditDoc, Rgba, Stroke, TextLayer, TextShadow};
use super::overlay::{Canvas, Fonts};
use super::textprops::{self, to_color, DEFAULT_FAMILY};
use super::{to_image, Ctl, Drag};
use crate::{EdFont, EdLayerRow, EdLayerVis, EdText, EditorState};

/// selection frame padding and rotate-handle distance (stage px), as the Konva transformer
const FRAME_PAD: f64 = 6.0;
const ROTATE_OFFSET: f64 = 26.0;

pub(super) struct Brush {
    pub color: Rgba,
    /// 2…100, thousandths of the crop's short side
    pub size: f64,
    pub opacity: f64,
}

impl Default for Brush {
    fn default() -> Self {
        Brush { color: [0xf4, 0x3f, 0x5e, 255], size: 12.0, opacity: 1.0 }
    }
}

/// A layer's raster at one stage scale.
#[derive(Clone)]
pub(super) struct Cached {
    look: TextLayer,
    scale: f64,
    epoch: usize,
    image: Image,
    w: f64,
    h: f64,
    box_w: f64,
    box_h: f64,
    /// raster px per frame px
    rs: f64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum LayerOp {
    Move,
    Scale,
    Rotate,
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

impl Ctl {
    fn fonts(&self) -> RefMut<'_, Fonts> {
        RefMut::map(self.fonts.borrow_mut(), |f| f.get_or_insert_with(|| Fonts::new(slint::fontique_011::shared_collection())))
    }

    pub(super) fn interactive(&self) -> bool {
        let t = self.tab.borrow();
        *t != "crop" && *t != "draw" && !self.compare.get()
    }

    pub(super) fn to_stage(&self, fx: f64, fy: f64) -> (f64, f64) {
        let v = self.view.get();
        let (sw, sh) = self.stage.get();
        ((fx - v.cx) * v.scale + sw / 2.0, (fy - v.cy) * v.scale + sh / 2.0)
    }

    pub(super) fn to_frame(&self, x: f64, y: f64) -> (f64, f64) {
        let v = self.view.get();
        let (sw, sh) = self.stage.get();
        ((x - sw / 2.0) / v.scale + v.cx, (y - sh / 2.0) / v.scale + v.cy)
    }

    fn crop_rect(&self) -> doc::Rect {
        let (fw, fh) = self.frame();
        doc::crop_px(&self.doc().geo, fw, fh)
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
            image: to_image(r.px, r.w, r.h),
        };
        self.rasters.borrow_mut().insert(l.id, c.clone());
        c
    }

    /// box (text + background padding) in frame px
    fn box_of(&self, l: &TextLayer) -> (f64, f64) {
        if let Some(c) = self.rasters.borrow().get(&l.id) {
            if c.look.look_eq(l) {
                return (c.box_w / c.rs, c.box_h / c.rs);
            }
        }
        self.fonts().measure(l)
    }

    pub(super) fn selected_layer(&self) -> Option<TextLayer> {
        let id = self.selected.get()?;
        self.hist.borrow().doc.layers.iter().find(|l| l.id == id).cloned()
    }

    // ───────── publishing ─────────

    /// Layer images and the selection on the stage, the layer list, the selected layer's panel.
    pub(super) fn publish_layers(&self) {
        let Some(a) = self.app() else { return };
        let s = a.window.global::<EditorState>();
        let d = self.doc();
        if self.selected.get().is_some_and(|id| !d.layers.iter().any(|l| l.id == id)) {
            self.selected.set(None);
        }
        let sel = self.selected.get();
        let v = self.view.get();
        let sf = a.window.window().scale_factor() as f64;
        let show = !self.compare.get() && self.src.get().is_some();
        let interactive = self.interactive();
        let mut vis = Vec::new();
        if show {
            for l in &d.layers {
                let c = self.raster(l, v.scale * sf);
                let k = v.scale / c.rs;
                let (x, y) = self.to_stage(l.x, l.y);
                vis.push(EdLayerVis {
                    id: l.id as i32,
                    image: c.image,
                    x: x as f32,
                    y: y as f32,
                    w: (c.w * k) as f32,
                    h: (c.h * k) as f32,
                    bw: (c.box_w * k) as f32,
                    bh: (c.box_h * k) as f32,
                    rotation: l.rotation as f32,
                    opacity: l.opacity as f32,
                    selected: interactive && sel == Some(l.id),
                });
            }
        }
        self.rasters.borrow_mut().retain(|id, _| d.layers.iter().any(|l| l.id == *id));
        if self.layer_vis.row_count() == vis.len() {
            for (i, r) in vis.into_iter().enumerate() {
                self.layer_vis.set_row_data(i, r);
            }
        } else {
            self.layer_vis.set_vec(vis);
        }
        let rows: Vec<EdLayerRow> = d
            .layers
            .iter()
            .rev()
            .map(|l| EdLayerRow {
                id: l.id as i32,
                label: l.text.lines().next().filter(|t| !t.trim().is_empty()).unwrap_or("…").into(),
                family: l.family.clone().into(),
                color: to_color(l.color),
                selected: sel == Some(l.id),
            })
            .collect();
        if self.layer_rows.iter().ne(rows.iter().cloned()) {
            self.layer_rows.set_vec(rows);
        }
        s.set_has_sel(sel.is_some());
        if let Some(l) = self.selected_layer() {
            s.set_sel(self.text_props(&l));
        }
        s.set_stroke_count(d.strokes.len() as i32);
    }

    fn text_props(&self, l: &TextLayer) -> EdText {
        let c = self.crop_rect();
        textprops::props(l, (c.w.max(c.h) * 0.5).round().max(200.0))
    }

    /// Brush strokes as one image over the visible part of the region, plus the brush cursor size.
    pub(super) fn publish_strokes(&self) {
        let Some(a) = self.app() else { return };
        let s = a.window.global::<EditorState>();
        let d = self.doc();
        let v = self.view.get();
        let crop = self.crop_rect();
        let b = self.brush.borrow();
        let size = (b.size / 1000.0 * crop.w.min(crop.h)).max(1.0);
        s.set_brush_px((size * v.scale).max(6.0) as f32);
        drop(b);
        let live = self.live.borrow();
        if (d.strokes.is_empty() && live.is_none()) || self.compare.get() || self.src.get().is_none() {
            s.set_strokes_img(Image::default());
            return;
        }
        let sf = a.window.window().scale_factor() as f64;
        let (sw, sh) = self.stage.get();
        let r = self.region();
        let (l, t) = self.to_stage(r.x, r.y);
        let (rr, rb) = self.to_stage(r.x + r.w, r.y + r.h);
        let (l, t, rr, rb) = (l.max(0.0).floor(), t.max(0.0).floor(), rr.min(sw).ceil(), rb.min(sh).ceil());
        if rr <= l || rb <= t {
            s.set_strokes_img(Image::default());
            return;
        }
        let (dw, dh) = (((rr - l) * sf).round().max(1.0) as u32, ((rb - t) * sf).round().max(1.0) as u32);
        let mut c = Canvas::new(dw, dh);
        let (ox, oy) = self.to_frame(l, t);
        let k = v.scale * sf;
        c.draw_strokes(&d.strokes, k, ox, oy);
        if let Some(st) = live.as_ref() {
            c.draw_stroke(st, k, ox, oy);
        }
        s.set_strokes_img(to_image(c.px, dw, dh));
        s.set_strokes_x(l as f32);
        s.set_strokes_y(t as f32);
        s.set_strokes_w((dw as f64 / sf) as f32);
        s.set_strokes_h((dh as f64 / sf) as f32);
    }

    pub(super) fn publish_fonts(&self) {
        let Some(a) = self.app() else { return };
        let fonts: Vec<EdFont> = crate::pages::fonts::picker()
            .into_iter()
            .map(|(family, ..)| EdFont { label: family.clone().into(), family: family.into() })
            .collect();
        a.window.global::<EditorState>().set_fonts(ModelRc::new(VecModel::from(fonts)));
    }

    // ───────── document changes that only touch the overlays ─────────

    /// Change layers or strokes: no preview render, the GPU image stays as it is.
    pub(super) fn change_overlay(self: &Rc<Self>, f: impl FnOnce(&EditDoc) -> EditDoc, key: Option<&str>) {
        if self.src.get().is_none() {
            return;
        }
        let next = f(&self.hist.borrow().doc);
        self.hist.borrow_mut().change(next, key);
        self.publish();
    }

    fn change_layer(self: &Rc<Self>, id: u32, key: &str, f: impl FnOnce(&mut TextLayer)) {
        let key = format!("text.{id}.{key}");
        self.change_overlay(
            |d| {
                let mut n = d.clone();
                if let Some(l) = n.layers.iter_mut().find(|l| l.id == id) {
                    f(l);
                }
                n
            },
            Some(&key),
        );
    }

    pub(super) fn select(self: &Rc<Self>, id: Option<u32>) {
        self.selected.set(id);
        self.sync_text();
        self.publish_layers();
    }

    /// The text box shows the selected layer's text again (selection change, undo).
    pub(super) fn sync_text(&self) {
        let Some(a) = self.app() else { return };
        let s = a.window.global::<EditorState>();
        if let Some(l) = self.selected_layer() {
            s.set_sel(self.text_props(&l));
        }
        s.set_text_sync(s.get_text_sync() + 1);
    }

    /// A new layer in the middle of the crop: the default text, or an emoji sticker.
    pub(super) fn add_text(self: &Rc<Self>, sticker: &str) {
        let Some(a) = self.app() else { return };
        let c = self.crop_rect();
        let short = c.w.min(c.h);
        let n = self.hist.borrow().doc.layers.len();
        let off = (n % 5) as f64 * short * 0.04;
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let text = if sticker.is_empty() { a.t("photo.text.default") } else { sticker.to_string() };
        let sticker = !sticker.is_empty();
        let l = TextLayer {
            id,
            text,
            family: DEFAULT_FAMILY.into(),
            weight: if sticker { 400.0 } else { 700.0 },
            italic: false,
            size: (short * if sticker { 0.16 } else { 0.09 }).round().max(8.0),
            color: [255, 255, 255, 255],
            align: Align::Center,
            line_height: 1.15,
            letter_spacing: 0.0,
            x: c.x + c.w / 2.0 + off,
            y: c.y + c.h / 2.0 + off,
            rotation: 0.0,
            opacity: 1.0,
            stroke: None,
            shadow: (!sticker).then(|| TextShadow {
                color: [0, 0, 0, 115],
                blur: (short * 0.012).round(),
                x: 0.0,
                y: (short * 0.003).round(),
            }),
            background: None,
        };
        self.change_overlay(
            |d| {
                let mut n = d.clone();
                n.layers.push(l);
                n
            },
            None,
        );
        self.select(Some(id));
    }

    pub(super) fn duplicate(self: &Rc<Self>, id: u32) {
        let Some(src) = self.hist.borrow().doc.layers.iter().find(|l| l.id == id).cloned() else { return };
        let c = self.crop_rect();
        let d = c.w.min(c.h) * 0.03;
        let nid = self.next_id.get();
        self.next_id.set(nid + 1);
        let copy = TextLayer { id: nid, x: src.x + d, y: src.y + d, ..src };
        self.change_overlay(
            |doc| {
                let mut n = doc.clone();
                n.layers.push(copy);
                n
            },
            None,
        );
        self.select(Some(nid));
    }

    pub(super) fn delete_layer(self: &Rc<Self>, id: u32) {
        if self.selected.get() == Some(id) {
            self.selected.set(None);
        }
        self.change_overlay(
            |d| {
                let mut n = d.clone();
                n.layers.retain(|l| l.id != id);
                n
            },
            None,
        );
    }

    /// dir > 0 moves the layer up (towards the top of the stack)
    pub(super) fn restack(self: &Rc<Self>, id: u32, dir: i32) {
        self.change_overlay(
            |d| {
                let mut n = d.clone();
                if let Some(i) = n.layers.iter().position(|l| l.id == id) {
                    let j = i as i64 + dir.signum() as i64;
                    if j >= 0 && (j as usize) < n.layers.len() {
                        n.layers.swap(i, j as usize);
                    }
                }
                n
            },
            None,
        );
    }

    pub(super) fn set_num(self: &Rc<Self>, key: &str, v: f32) {
        let Some(l) = self.selected_layer() else { return };
        self.change_layer(l.id, key, |l| textprops::set_num(l, key, v as f64));
    }

    pub(super) fn set_str(self: &Rc<Self>, key: &str, v: &str) {
        let Some(l) = self.selected_layer() else { return };
        if matches!(key, "text" | "family" | "align") {
            self.change_layer(l.id, key, |l| textprops::set_str(l, key, v));
        }
    }

    pub(super) fn set_color(self: &Rc<Self>, key: &str, c: Color) {
        let Some(l) = self.selected_layer() else { return };
        self.change_layer(l.id, key, |l| textprops::set_color(l, key, c));
    }

    pub(super) fn toggle(self: &Rc<Self>, key: &str, on: bool) {
        let Some(l) = self.selected_layer() else { return };
        self.change_layer(l.id, key, |l| textprops::toggle(l, key, on));
    }

    // ───────── stage gestures ─────────

    /// The layer (and which part of its frame) under a stage point; handles of the selected
    /// layer first, then the layers top-most first.
    pub(super) fn hit_layer(&self, x: f64, y: f64) -> Option<(u32, LayerOp)> {
        if !self.interactive() || self.src.get().is_none() {
            return None;
        }
        let v = self.view.get();
        let d = self.doc();
        if let Some(l) = self.selected_layer() {
            let (bw, bh) = self.box_of(&l);
            let (hw, hh) = (bw * v.scale / 2.0 + FRAME_PAD, bh * v.scale / 2.0 + FRAME_PAD);
            let (cx, cy) = self.to_stage(l.x, l.y);
            let (sin, cos) = l.rotation.to_radians().sin_cos();
            let at = |lx: f64, ly: f64| (cx + lx * cos - ly * sin, cy + lx * sin + ly * cos);
            let near = |p: (f64, f64), r: f64| (p.0 - x).hypot(p.1 - y) <= r;
            if near(at(0.0, -hh - ROTATE_OFFSET), 14.0) {
                return Some((l.id, LayerOp::Rotate));
            }
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                if near(at(sx * hw, sy * hh), 12.0) {
                    return Some((l.id, LayerOp::Scale));
                }
            }
        }
        for l in d.layers.iter().rev() {
            let (bw, bh) = self.box_of(l);
            let (cx, cy) = self.to_stage(l.x, l.y);
            let (sin, cos) = l.rotation.to_radians().sin_cos();
            let (dx, dy) = (x - cx, y - cy);
            let lx = dx * cos + dy * sin;
            let ly = -dx * sin + dy * cos;
            if lx.abs() <= bw * v.scale / 2.0 + 4.0 && ly.abs() <= bh * v.scale / 2.0 + 4.0 {
                return Some((l.id, LayerOp::Move));
            }
        }
        None
    }

    pub(super) fn start_layer_drag(self: &Rc<Self>, id: u32, op: LayerOp, x: f64, y: f64) {
        if self.selected.get() != Some(id) {
            self.select(Some(id));
        }
        if *self.tab.borrow() != "text" {
            self.set_tab("text");
        }
        let Some(start) = self.selected_layer() else { return };
        *self.drag.borrow_mut() = Some(Drag::Layer { op, x, y, start: Box::new(start) });
    }

    pub(super) fn drag_layer(self: &Rc<Self>, op: LayerOp, x0: f64, y0: f64, start: &TextLayer, x: f64, y: f64) {
        let v = self.view.get();
        let (cx, cy) = self.to_stage(start.x, start.y);
        let mut guides = (false, false);
        let n = match op {
            LayerOp::Move => {
                let c = self.crop_rect();
                let (ccx, ccy) = (c.x + c.w / 2.0, c.y + c.h / 2.0);
                let tol = 8.0 / v.scale;
                let mut nx = start.x + (x - x0) / v.scale;
                let mut ny = start.y + (y - y0) / v.scale;
                if (nx - ccx).abs() < tol {
                    nx = ccx;
                    guides.0 = true;
                }
                if (ny - ccy).abs() < tol {
                    ny = ccy;
                    guides.1 = true;
                }
                TextLayer { x: round1(nx), y: round1(ny), ..start.clone() }
            }
            LayerOp::Scale => {
                let d0 = (x0 - cx).hypot(y0 - cy).max(1.0);
                let k = ((x - cx).hypot(y - cy) / d0).max(0.05);
                start.scaled(k)
            }
            LayerOp::Rotate => {
                let a0 = (y0 - cy).atan2(x0 - cx);
                let a = (y - cy).atan2(x - cx);
                let mut r = start.rotation + (a - a0).to_degrees();
                r = (r + 180.0).rem_euclid(360.0) - 180.0;
                let snap = (r / 45.0).round() * 45.0;
                if (r - snap).abs() < 4.0 {
                    r = snap;
                }
                TextLayer { rotation: round1(r), ..start.clone() }
            }
        };
        self.set_guides(guides);
        let key = format!("layer.{op:?}.{}", start.id);
        self.change_overlay(
            |d| {
                let mut doc = d.clone();
                if let Some(l) = doc.layers.iter_mut().find(|l| l.id == start.id) {
                    *l = n;
                }
                doc
            },
            Some(&key),
        );
    }

    pub(super) fn set_guides(&self, (gx, gy): (bool, bool)) {
        let Some(a) = self.app() else { return };
        let s = a.window.global::<EditorState>();
        s.set_guide_x(gx);
        s.set_guide_y(gy);
    }

    pub(super) fn start_draw(self: &Rc<Self>, x: f64, y: f64) {
        let (fx, fy) = self.to_frame(x, y);
        let c = self.crop_rect();
        let b = self.brush.borrow();
        let st = Stroke {
            color: b.color,
            size: (b.size / 1000.0 * c.w.min(c.h)).max(1.0),
            opacity: b.opacity,
            points: vec![[round1(fx), round1(fy)]],
        };
        drop(b);
        *self.live.borrow_mut() = Some(st);
        *self.drag.borrow_mut() = Some(Drag::Draw { x, y });
        self.publish_strokes();
    }

    pub(super) fn draw_to(&self, lx: f64, ly: f64, x: f64, y: f64) -> bool {
        if (x - lx).hypot(y - ly) < 1.5 {
            return false;
        }
        let (fx, fy) = self.to_frame(x, y);
        if let Some(st) = self.live.borrow_mut().as_mut() {
            st.points.push([round1(fx), round1(fy)]);
        }
        self.publish_strokes();
        true
    }

    pub(super) fn end_draw(self: &Rc<Self>) {
        let Some(st) = self.live.borrow_mut().take() else { return };
        self.change_overlay(
            |d| {
                let mut n = d.clone();
                n.strokes.push(st);
                n
            },
            None,
        );
    }

    pub(super) fn set_brush(&self, f: impl FnOnce(&mut Brush)) {
        f(&mut self.brush.borrow_mut());
        let Some(a) = self.app() else { return };
        let s = a.window.global::<EditorState>();
        let b = self.brush.borrow();
        s.set_brush_color(to_color(b.color));
        s.set_brush_size(b.size as f32);
        s.set_brush_opacity((b.opacity * 100.0).round() as f32);
        drop(b);
        self.publish_strokes();
    }
}

/// Emoji the sticker grid offers (panels/TextPanel STICKERS).
pub(super) const STICKERS: [&str; 16] =
    ["😍", "😂", "🥳", "❤️", "🔥", "✨", "⭐", "🌸", "☀️", "🌈", "🎉", "👍", "😎", "🤩", "🐾", "🎈"];

pub(super) fn stickers() -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(STICKERS.iter().map(|s| SharedString::from(*s)).collect::<Vec<_>>()))
}
