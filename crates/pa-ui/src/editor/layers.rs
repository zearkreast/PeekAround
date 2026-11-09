//! Drawing and the editor side of text layers in the photo editor (panels/DrawPanel, the Konva
//! layers of EditorStage): freehand brush strokes, and the glue between the editor's document /
//! stage and the shared text-layer panel and gestures (text.rs).

use std::rc::Rc;

use slint::{ComponentHandle, Image, Model};

use super::doc::{self, EditDoc, Rgba, Stroke, TextLayer};
use super::overlay::Canvas;
use super::text::{self, round1, to_color, LayerOp};
use super::{to_image, Ctl, Drag};
use crate::app::AppInner;
use crate::EditorState;

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

impl Ctl {
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
        let vis = if show {
            let shown = sel.filter(|_| self.interactive());
            self.rasters.borrow_mut().vis(&d.layers, shown, v.scale, sf, |x, y| self.to_stage(x, y))
        } else {
            Vec::new()
        };
        if self.layer_vis.row_count() == vis.len() {
            for (i, r) in vis.into_iter().enumerate() {
                self.layer_vis.set_row_data(i, r);
            }
        } else {
            self.layer_vis.set_vec(vis);
        }
        text::publish_panel(&a, &d.layers, sel, self.crop_rect());
        s.set_stroke_count(d.strokes.len() as i32);
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

    // ───────── document changes that only touch the overlays ─────────

    /// Change layers or strokes: no preview render, the GPU image stays as it is.
    pub(super) fn change_overlay(&self, f: impl FnOnce(&EditDoc) -> EditDoc, key: Option<&str>) {
        if self.src.get().is_none() {
            return;
        }
        let next = f(&self.hist.borrow().doc);
        self.hist.borrow_mut().change(next, key);
        self.publish();
    }

    pub(super) fn select(&self, id: Option<u32>) {
        self.selected.set(id);
        self.sync_text();
        self.publish_layers();
    }

    /// The text box shows the selected layer's text again (selection change, undo).
    pub(super) fn sync_text(&self) {
        let Some(a) = self.app() else { return };
        text::sync_text(&a, &self.doc().layers, self.selected.get(), self.crop_rect());
    }

    // ───────── stage gestures ─────────

    pub(super) fn hit_layer(&self, x: f64, y: f64) -> Option<(u32, LayerOp)> {
        if !self.interactive() || self.src.get().is_none() {
            return None;
        }
        let d = self.doc();
        let scale = self.view.get().scale;
        self.rasters.borrow_mut().hit(&d.layers, self.selected.get(), scale, |x, y| self.to_stage(x, y), x, y)
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

    pub(super) fn drag_layer(&self, op: LayerOp, x0: f64, y0: f64, start: &TextLayer, x: f64, y: f64) {
        let c = self.crop_rect();
        let center = self.to_stage(start.x, start.y);
        let snap = (c.x + c.w / 2.0, c.y + c.h / 2.0);
        let (n, guides) = text::drag(op, start, self.view.get().scale, center, snap, x0, y0, x, y);
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

impl text::LayerHost for Ctl {
    fn app(&self) -> Option<Rc<AppInner>> {
        self.app.upgrade()
    }

    fn layers(&self) -> Vec<TextLayer> {
        self.hist.borrow().doc.layers.clone()
    }

    fn commit(&self, layers: Vec<TextLayer>, key: Option<&str>) {
        self.change_overlay(|d| EditDoc { layers, ..d.clone() }, key);
    }

    fn area(&self) -> doc::Rect {
        self.crop_rect()
    }

    fn selected(&self) -> Option<u32> {
        self.selected.get()
    }

    fn select(&self, id: Option<u32>) {
        Ctl::select(self, id);
    }

    fn next_id(&self) -> u32 {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        id
    }
}
