//! Collage (features/collage): photos from a selection ("Make collage") or the picker go into a
//! layout's cells; each photo pans / zooms inside its cell, cells swap by their grip, style,
//! canvas ratio and output size are options, text layers sit on top (the photo editor's shared
//! text panel). The canvas is drawn on the CPU (render.rs) from 1600 px thumbnails for the
//! stage and from the display renditions for the saved file, which goes to `POST /api/edits/save`
//! into the chosen folder.

mod geometry;
mod model;
mod render;
mod templates;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use pa_core::{DirTarget, MediaItem, MediaKind};
use slint::{Color, ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};

use crate::actions_ops::ImageSize;
use crate::app::AppInner;
use crate::editor::doc::{History, Rect, TextLayer};
use crate::editor::text::{self, to_image, LayerOp};
use crate::router::Route;
use crate::{ClAspect, ClCell, ClGradient, ClMini, ClPhoto, ClTemplate, CollageState, EdLayerVis, ToastKind};
use geometry::{cell_at, layout, layout_cells, place_image, rotate_delta, to_cell, Box2};
use model::{design_size, output_size, Background, CollageDoc, Slot, DESIGN_LONG};
use render::Mips;

/// the swap grip: 32 px button 8 px inside the photo window (stage px)
const GRIP: (f64, f64) = (8.0, 40.0);

#[derive(Clone)]
enum Drag {
    Pan { cell: usize, x: f64, y: f64, start: Slot, placed: Box2, win: Box2, rot: f64 },
    Swap { from: usize, over: Option<usize> },
    Layer { op: LayerOp, x: f64, y: f64, start: Box<TextLayer> },
}

/// The canvas on the stage: `s` stage px per design unit, top-left (x, y).
#[derive(Clone, Copy)]
struct View {
    s: f64,
    x: f64,
    y: f64,
}

impl View {
    fn to_stage(self, dx: f64, dy: f64) -> (f64, f64) {
        (self.x + dx * self.s, self.y + dy * self.s)
    }

    fn to_design(self, x: f64, y: f64) -> (f64, f64) {
        ((x - self.x) / self.s, (y - self.y) / self.s)
    }
}

struct Ctl {
    app: Weak<AppInner>,
    /// the photo pool (not undoable)
    photos: RefCell<Vec<MediaItem>>,
    hist: RefCell<History<CollageDoc>>,
    /// ids of the selection this collage was started from
    source_key: RefCell<String>,
    target: RefCell<Option<DirTarget>>,
    tab: RefCell<String>,
    cell: Cell<Option<usize>>,
    selected: Cell<Option<u32>>,
    next_id: Cell<u32>,
    stage: Cell<(f64, f64)>,
    /// the stage's window position
    origin: Cell<(f64, f64)>,
    previews: RefCell<HashMap<String, Arc<Mips>>>,
    loading: RefCell<HashSet<String>>,
    /// 320 px thumbnails (pool tiles, drag ghost)
    tiles: RefCell<HashMap<String, Image>>,
    rendering: Cell<bool>,
    render_again: Cell<bool>,
    /// bumps on every new collage: late results are dropped
    gen: Cell<u64>,
    drag: RefCell<Option<Drag>>,
    /// a pool photo being dragged over the stage, and the cell under it
    pool_drag: Cell<Option<usize>>,
    drop_over: Cell<Option<usize>>,
    rasters: RefCell<text::Rasters>,
    layer_vis: Rc<VecModel<EdLayerVis>>,
    cells: Rc<VecModel<ClCell>>,
    pool: Rc<VecModel<ClPhoto>>,
    templates: Rc<VecModel<ClTemplate>>,
    aspects: Rc<VecModel<ClAspect>>,
    gradients: Rc<VecModel<ClGradient>>,
    /// (count, aspect, seed, lang) the layout cards were drawn for
    tpl_key: RefCell<Option<(usize, &'static str, u32, crate::i18n::Lang)>>,
    exporting: Cell<bool>,
    format: RefCell<String>,
    quality: Cell<u32>,
}

thread_local! {
    static CTL: RefCell<Option<Rc<Ctl>>> = const { RefCell::new(None) };
}

fn ctl() -> Option<Rc<Ctl>> {
    CTL.with(|c| c.borrow().clone())
}

fn rgb_color(c: model::Rgb) -> Color {
    Color::from_rgb_u8(c[0], c[1], c[2])
}

/// A preset as the three stops the Slint card draws (two-stop presets get their midpoint).
fn gradient_row(g: &model::GradientPreset) -> ClGradient {
    let (a, c) = (g.stops[0].1, g.stops[g.stops.len() - 1].1);
    let (mid, b) = if g.stops.len() > 2 {
        (g.stops[1].0, g.stops[1].1)
    } else {
        (0.5, [0, 1, 2].map(|i| ((a[i] as u16 + c[i] as u16) / 2) as u8))
    };
    ClGradient { id: g.id.into(), angle: g.angle as f32, a: rgb_color(a), b: rgb_color(b), c: rgb_color(c), mid: mid as f32, active: false }
}

/// icon box (px) of a canvas ratio
fn aspect_box(r: f64) -> (f32, f32) {
    if r >= 1.0 {
        (22.0, (22.0 / r) as f32)
    } else {
        ((22.0 * r) as f32, 22.0)
    }
}

/// `name.replace(/[\\/:*?"<>|]+/g, '-')`
fn sanitize(name: &str) -> String {
    let mut out = String::new();
    let mut bad = false;
    for ch in name.chars() {
        if matches!(ch, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            if !bad {
                out.push('-');
            }
            bad = true;
        } else {
            out.push(ch);
            bad = false;
        }
    }
    out
}

impl Ctl {
    fn app(&self) -> Option<Rc<AppInner>> {
        self.app.upgrade()
    }

    fn doc(&self) -> CollageDoc {
        self.hist.borrow().doc.clone()
    }

    fn area(&self) -> Rect {
        let (w, h) = design_size(self.hist.borrow().doc.aspect);
        Rect { x: 0.0, y: 0.0, w, h }
    }

    fn view(&self) -> Option<View> {
        let (sw, sh) = self.stage.get();
        if sw < 1.0 || sh < 1.0 {
            return None;
        }
        let (w, h) = design_size(self.hist.borrow().doc.aspect);
        let narrow = sw < 640.0;
        let pad_x = if narrow { 12.0 } else { 32.0 };
        let pad_top = if narrow { 12.0 } else { 28.0 };
        let pad_bottom = if narrow { 64.0 } else { 76.0 };
        let s = ((sw - pad_x * 2.0) / w).min((sh - pad_top - pad_bottom) / h).max(0.05);
        Some(View { s, x: (sw - w * s) / 2.0, y: pad_top + (sh - pad_top - pad_bottom - h * s) / 2.0 })
    }

    /// natural size of a photo: its preview once loaded, else the item's
    fn dims(&self, id: &str) -> (f64, f64) {
        if let Some(m) = self.previews.borrow().get(id) {
            return (m.width(), m.height());
        }
        self.photos
            .borrow()
            .iter()
            .find(|p| p.id == id)
            .and_then(|p| Some((p.width? as f64, p.height? as f64)))
            .filter(|(w, h)| *w > 0.0 && *h > 0.0)
            .unwrap_or((1000.0, 1000.0))
    }

    // ───────── document ─────────

    fn change(self: &Rc<Self>, f: impl FnOnce(&CollageDoc) -> CollageDoc, key: Option<&str>) {
        let next = f(&self.hist.borrow().doc);
        self.hist.borrow_mut().change(next, key);
        self.after_change();
    }

    fn after_change(self: &Rc<Self>) {
        let count = self.hist.borrow().doc.count;
        if self.cell.get().is_some_and(|c| c >= count) {
            self.cell.set(None);
        }
        self.publish();
        self.request_render();
    }

    fn set_slot(self: &Rc<Self>, i: usize, key: Option<&str>, f: impl FnOnce(&mut Slot)) {
        self.change(
            |d| {
                let mut n = d.clone();
                if let Some(s) = n.slots.get_mut(i) {
                    f(s);
                }
                n
            },
            key,
        );
    }

    fn select_cell(&self, i: Option<usize>) {
        self.cell.set(i);
        if i.is_some() && self.selected.get().is_some() {
            self.selected.set(None);
            self.sync_text();
        }
        self.publish_stage();
    }

    fn select_text(&self, id: Option<u32>) {
        self.selected.set(id);
        if id.is_some() {
            self.cell.set(None);
        }
        self.sync_text();
        self.publish_stage();
    }

    fn sync_text(&self) {
        if let Some(a) = self.app() {
            text::sync_text(&a, &self.doc().layers, self.selected.get(), self.area());
        }
    }

    fn set_tab(&self, t: &str) {
        *self.tab.borrow_mut() = t.to_string();
        if let Some(a) = self.app() {
            a.window.global::<CollageState>().set_tab(t.into());
        }
    }

    // ───────── publishing ─────────

    fn publish(&self) {
        let Some(a) = self.app() else { return };
        let s = a.window.global::<CollageState>();
        let d = self.doc();
        let photos = self.photos.borrow().clone();
        {
            let h = self.hist.borrow();
            s.set_can_undo(h.can_undo());
            s.set_can_redo(h.can_redo());
        }
        s.set_has_photos(!photos.is_empty());
        s.set_count(d.count as i32);
        s.set_photo_count(photos.len() as i32);
        s.set_filled(d.filled());
        s.set_free_template(templates::template(d.template).free);
        self.publish_templates(&a, &d);
        for i in 0..self.aspects.row_count() {
            if let Some(mut r) = self.aspects.row_data(i) {
                let on = r.key == d.aspect;
                if r.active != on {
                    r.active = on;
                    self.aspects.set_row_data(i, r);
                }
            }
        }
        s.set_gap(d.gap as f32);
        s.set_padding(d.padding as f32);
        s.set_radius(d.radius as f32);
        let (kind, gid) = match &d.bg {
            Background::Solid(c) => {
                s.set_bg_color(rgb_color(*c));
                ("solid", "")
            }
            Background::Gradient(id) => ("gradient", *id),
            Background::Blur(v) => {
                s.set_blur(*v as f32);
                ("blur", "")
            }
        };
        s.set_bg_kind(kind.into());
        for i in 0..self.gradients.row_count() {
            if let Some(mut r) = self.gradients.row_data(i) {
                let on = r.id == gid;
                if r.active != on {
                    r.active = on;
                    self.gradients.set_row_data(i, r);
                }
            }
        }
        s.set_long_side(d.long_side.to_string().into());
        let (ow, oh) = output_size(&d);
        s.set_size_text(crate::i18n::tv(a.lang(), "collage.size", &[("w", &ow.to_string()), ("h", &oh.to_string())]).into());
        // the pool
        let tiles = self.tiles.borrow();
        let rows: Vec<ClPhoto> = photos
            .iter()
            .map(|p| {
                let t = tiles.get(&p.id);
                ClPhoto { thumb: t.cloned().unwrap_or_default(), has_thumb: t.is_some(), used: d.used(&p.id) }
            })
            .collect();
        drop(tiles);
        if self.pool.row_count() != rows.len() {
            self.pool.set_vec(rows);
        } else {
            for (i, r) in rows.into_iter().enumerate() {
                let old = self.pool.row_data(i);
                if old.is_none_or(|o| o.used != r.used || o.has_thumb != r.has_thumb) {
                    self.pool.set_row_data(i, r);
                }
            }
        }
        let missing: Vec<String> = photos.iter().filter(|p| !self.tiles.borrow().contains_key(&p.id)).map(|p| p.id.clone()).collect();
        for id in missing {
            let gen = self.gen.get();
            let id2 = id.clone();
            crate::pages::cover(&a, &id, 320, move |img| {
                if let Some(c) = ctl() {
                    if c.gen.get() == gen && c.tiles.borrow_mut().insert(id2, img).is_none() {
                        c.publish();
                    }
                }
            });
        }
        if let Some(t) = self.target.borrow().as_ref() {
            s.set_target_label(target_label(t).into());
        } else {
            s.set_target_label(SharedString::new());
        }
        self.publish_stage();
    }

    /// The layout cards' mini previews (for the current count and ratio).
    fn publish_templates(&self, a: &AppInner, d: &CollageDoc) {
        let key = (d.count, d.aspect, d.seed, a.lang());
        if *self.tpl_key.borrow() != Some(key) {
            *self.tpl_key.borrow_mut() = Some(key);
            let (w, h) = design_size(d.aspect);
            let k = (96.0 / w).min(96.0 / h);
            let rows: Vec<ClTemplate> = templates::TEMPLATES
                .iter()
                .map(|t| {
                    let cells: Vec<ClMini> = layout(t.id, d.count, d.aspect, d.seed, 60.0, 60.0, 40.0)
                        .iter()
                        .map(|c| ClMini {
                            x: (c.x * k) as f32,
                            y: (c.y * k) as f32,
                            w: (c.w * k) as f32,
                            h: (c.h * k) as f32,
                            rot: c.rot as f32,
                            r: (c.radius * k).max(1.5) as f32,
                            ix: (c.inner.x * k) as f32,
                            iy: (c.inner.y * k) as f32,
                            iw: (c.inner.w * k) as f32,
                            ih: (c.inner.h * k) as f32,
                            ir: (c.inner_radius * k).max(1.0) as f32,
                            free: c.free,
                        })
                        .collect();
                    ClTemplate {
                        id: t.id.into(),
                        label: a.t(t.label).into(),
                        w: (w * k) as f32,
                        h: (h * k) as f32,
                        cells: ModelRc::new(VecModel::from(cells)),
                        active: t.id == d.template,
                    }
                })
                .collect();
            self.templates.set_vec(rows);
            return;
        }
        for i in 0..self.templates.row_count() {
            if let Some(mut r) = self.templates.row_data(i) {
                let on = r.id == d.template;
                if r.active != on {
                    r.active = on;
                    self.templates.set_row_data(i, r);
                }
            }
        }
    }

    /// Canvas rect, cell overlays, text layers, the selected cell's tools.
    fn publish_stage(&self) {
        let Some(a) = self.app() else { return };
        let s = a.window.global::<CollageState>();
        let d = self.doc();
        if self.selected.get().is_some_and(|id| !d.layers.iter().any(|l| l.id == id)) {
            self.selected.set(None);
        }
        text::publish_panel(&a, &d.layers, self.selected.get(), self.area());
        let Some(v) = self.view() else { return };
        let (w, h) = design_size(d.aspect);
        s.set_canvas_x(v.x as f32);
        s.set_canvas_y(v.y as f32);
        s.set_canvas_w((w * v.s) as f32);
        s.set_canvas_h((h * v.s) as f32);
        let cells = layout_cells(&d);
        let swap_over = match &*self.drag.borrow() {
            Some(Drag::Swap { over, .. }) => *over,
            _ => self.drop_over.get(),
        };
        let rows: Vec<ClCell> = cells
            .iter()
            .map(|c| {
                let (x, y) = v.to_stage(c.x, c.y);
                ClCell {
                    index: c.index as i32,
                    x: x as f32,
                    y: y as f32,
                    w: (c.w * v.s) as f32,
                    h: (c.h * v.s) as f32,
                    rot: c.rot as f32,
                    ix: (c.inner.x * v.s) as f32,
                    iy: (c.inner.y * v.s) as f32,
                    iw: (c.inner.w * v.s) as f32,
                    ih: (c.inner.h * v.s) as f32,
                    radius: (c.inner_radius * v.s) as f32,
                    filled: d.slots[c.index].id.is_some(),
                    selected: self.cell.get() == Some(c.index),
                    drop: swap_over == Some(c.index),
                }
            })
            .collect();
        if self.cells.row_count() == rows.len() {
            for (i, r) in rows.into_iter().enumerate() {
                if self.cells.row_data(i).as_ref() != Some(&r) {
                    self.cells.set_row_data(i, r);
                }
            }
        } else {
            self.cells.set_vec(rows);
        }
        let sf = a.window.window().scale_factor() as f64;
        let vis = self.rasters.borrow_mut().vis(&d.layers, self.selected.get(), v.s, sf, |x, y| v.to_stage(x, y));
        if self.layer_vis.row_count() == vis.len() {
            for (i, r) in vis.into_iter().enumerate() {
                self.layer_vis.set_row_data(i, r);
            }
        } else {
            self.layer_vis.set_vec(vis);
        }
        let sel = self.cell.get().and_then(|i| cells.iter().find(|c| c.index == i)).filter(|c| d.slots[c.index].id.is_some());
        s.set_tools(sel.is_some());
        if let Some(c) = sel {
            let (sw, sh) = self.stage.get();
            let cx = v.x + (c.x + c.w / 2.0) * v.s;
            s.set_tools_x((cx - 170.0).clamp(8.0, (sw - 348.0).max(8.0)) as f32);
            s.set_tools_y((sh - 56.0).min(v.y + h * v.s + 12.0) as f32);
            s.set_zoom((d.slots[c.index].zoom * 100.0).round() as f32);
        }
        s.set_show_hint(self.cell.get().is_none() && !self.photos.borrow().is_empty());
    }

    // ───────── rendering ─────────

    /// Fetch the 1600 px previews the canvas needs.
    fn ensure_previews(self: &Rc<Self>, ids: &[String]) {
        let Some(a) = self.app() else { return };
        let Some(ops) = a.backend.ops() else { return };
        for id in ids {
            if self.previews.borrow().contains_key(id) || !self.loading.borrow_mut().insert(id.clone()) {
                continue;
            }
            let fut = ops.image(id, ImageSize::Thumb(1600));
            let id = id.clone();
            let me = Rc::downgrade(self);
            a.rt.spawn_then(
                async move {
                    let data = fut.await.map_err(|e| e.message)?;
                    crate::rt::spawn_blocking(move || crate::editor::decode_full(data).map(|img| Arc::new(Mips::new(img))))
                        .await
                        .map_err(|e| e.to_string())?
                },
                move |res| {
                    let Some(c) = me.upgrade() else { return };
                    c.loading.borrow_mut().remove(&id);
                    match res {
                        Ok(m) => {
                            c.previews.borrow_mut().insert(id, m);
                            c.request_render();
                        }
                        Err(e) => log::warn!("collage: preview of {id}: {e}"),
                    }
                },
            );
        }
    }

    fn bg_photo(&self, d: &CollageDoc) -> Option<String> {
        if !matches!(d.bg, Background::Blur(_)) {
            return None;
        }
        let pool = self.photos.borrow();
        d.first_photo(pool.first().map(|p| p.id.as_str())).map(str::to_string)
    }

    /// Render the stage preview; while one is in flight the latest state renders right after it.
    fn request_render(self: &Rc<Self>) {
        if self.rendering.get() {
            self.render_again.set(true);
            return;
        }
        let Some(a) = self.app() else { return };
        let Some(v) = self.view() else { return };
        if self.photos.borrow().is_empty() {
            return;
        }
        let d = self.doc();
        let mut want: Vec<String> = d.slots[..d.count].iter().filter_map(|s| s.id.clone()).collect();
        let bg_id = self.bg_photo(&d);
        want.extend(bg_id.clone());
        self.ensure_previews(&want);
        let previews: HashMap<String, Arc<Mips>> =
            want.iter().filter_map(|id| self.previews.borrow().get(id).map(|m| (id.clone(), m.clone()))).collect();
        let bg = bg_id.and_then(|id| previews.get(&id).cloned());
        let sf = a.window.window().scale_factor() as f64;
        let (w, h) = design_size(d.aspect);
        let scale = v.s * sf;
        let (pw, ph) = ((w * scale).round().max(1.0) as u32, (h * scale).round().max(1.0) as u32);
        self.rendering.set(true);
        self.render_again.set(false);
        let gen = self.gen.get();
        let me = Rc::downgrade(self);
        a.rt.spawn_then(
            async move {
                crate::rt::spawn_blocking(move || {
                    let mut get = |id: &str| previews.get(id).cloned();
                    render::render(&d, pw, ph, scale, bg.as_deref(), &mut get, None, true, &|_| {}).px
                })
                .await
                .ok()
            },
            move |px| {
                let Some(c) = me.upgrade() else { return };
                c.rendering.set(false);
                if c.gen.get() == gen {
                    if let (Some(px), Some(a)) = (px, c.app()) {
                        a.window.global::<CollageState>().set_preview(to_image(px, pw, ph));
                    }
                }
                if c.render_again.get() {
                    c.request_render();
                }
            },
        );
    }

    // ───────── photos ─────────

    /// A new collage from these photos (the gallery's "Make collage", "New collage").
    fn start(self: &Rc<Self>, items: Vec<MediaItem>, target: Option<DirTarget>) {
        let imgs: Vec<MediaItem> = items.into_iter().filter(|i| i.kind == MediaKind::Image).collect();
        let ids: Vec<String> = imgs.iter().map(|p| p.id.clone()).collect();
        self.gen.set(self.gen.get() + 1);
        *self.photos.borrow_mut() = imgs;
        *self.hist.borrow_mut() = History::new(CollageDoc::new(&ids));
        *self.target.borrow_mut() = target;
        self.cell.set(None);
        self.selected.set(None);
        self.next_id.set(1);
        *self.drag.borrow_mut() = None;
        self.rasters.borrow_mut().clear();
        self.set_tab("layout");
        if let Some(a) = self.app() {
            let s = a.window.global::<CollageState>();
            s.set_preview(Image::default());
            s.set_export_open(false);
        }
        self.after_change();
    }

    /// Picked photos: into the pool; with `slot`, the first one goes into that cell.
    fn add_photos(self: &Rc<Self>, items: Vec<MediaItem>, slot: Option<usize>) {
        let imgs: Vec<MediaItem> = items.into_iter().filter(|i| i.kind == MediaKind::Image).collect();
        let fresh: Vec<MediaItem> = {
            let pool = self.photos.borrow();
            imgs.iter().filter(|i| !pool.iter().any(|p| p.id == i.id)).cloned().collect()
        };
        if let Some(slot) = slot {
            let Some(first) = imgs.first().map(|i| i.id.clone()) else { return };
            self.photos.borrow_mut().extend(fresh);
            self.change(|d| d.place(slot, &first), None);
            self.select_cell(Some(slot));
            return;
        }
        if fresh.is_empty() {
            return;
        }
        let first = self.photos.borrow().is_empty();
        if self.target.borrow().is_none() {
            *self.target.borrow_mut() = Some(DirTarget { folder_id: fresh[0].folder_id.clone(), dir: fresh[0].dir.clone() });
        }
        let ids: Vec<String> = fresh.iter().map(|p| p.id.clone()).collect();
        self.photos.borrow_mut().extend(fresh);
        self.change(|d| d.add_photos(&ids, first), None);
    }

    fn pick(self: &Rc<Self>, slot: Option<usize>) {
        let Some(a) = self.app() else { return };
        let opts = crate::media_picker::Opts {
            title: a.t(if slot.is_some() { "collage.picker.replace" } else { "collage.picker.title" }),
            images_only: true,
            single: slot.is_some(),
            select_all: false,
            start: self.target.borrow().clone(),
            selected_key: "common.selected",
            add_key: "collage.picker.add",
            empty_key: "collage.picker.empty",
        };
        crate::media_picker::open(&a, opts, move |_, items| {
            if let Some(c) = ctl() {
                c.add_photos(items, slot);
            }
        });
    }

    fn remove_photo(self: &Rc<Self>, i: usize) {
        let Some(p) = self.photos.borrow().get(i).cloned() else { return };
        self.photos.borrow_mut().remove(i);
        self.change(|d| d.remove_photo(&p.id), None);
    }

    fn tile_of(&self, id: &str) -> Image {
        self.tiles.borrow().get(id).cloned().unwrap_or_default()
    }

    fn set_ghost(&self, on: bool, id: Option<&str>, wx: f64, wy: f64) {
        let Some(a) = self.app() else { return };
        let s = a.window.global::<CollageState>();
        if let Some(id) = id {
            s.set_ghost_img(self.tile_of(id));
        }
        s.set_ghost(on);
        s.set_ghost_x(wx as f32);
        s.set_ghost_y(wy as f32);
    }

    /// The cell under a window point (pool drags).
    fn cell_at_window(&self, wx: f64, wy: f64) -> Option<usize> {
        let v = self.view()?;
        let (ox, oy) = self.origin.get();
        let (dx, dy) = v.to_design(wx - ox, wy - oy);
        cell_at(&layout_cells(&self.hist.borrow().doc), dx, dy)
    }

    fn pool_drag(&self, i: usize, wx: f64, wy: f64) {
        let Some(id) = self.photos.borrow().get(i).map(|p| p.id.clone()) else { return };
        if self.pool_drag.get() != Some(i) {
            self.pool_drag.set(Some(i));
        }
        self.set_ghost(true, Some(&id), wx, wy);
        let over = self.cell_at_window(wx, wy);
        if over != self.drop_over.get() {
            self.drop_over.set(over);
            self.publish_stage();
        }
    }

    fn pool_drop(self: &Rc<Self>, i: usize, wx: f64, wy: f64) {
        self.pool_drag.set(None);
        self.drop_over.set(None);
        self.set_ghost(false, None, 0.0, 0.0);
        let id = self.photos.borrow().get(i).map(|p| p.id.clone());
        match (id, self.cell_at_window(wx, wy)) {
            (Some(id), Some(cell)) => {
                self.change(|d| d.place(cell, &id), None);
                self.select_cell(Some(cell));
            }
            _ => self.publish_stage(),
        }
    }

    // ───────── stage pointer ─────────

    fn press(self: &Rc<Self>, x: f64, y: f64) {
        attach();
        let Some(v) = self.view() else { return };
        let d = self.doc();
        let hit = self.rasters.borrow_mut().hit(&d.layers, self.selected.get(), v.s, |x, y| v.to_stage(x, y), x, y);
        if let Some((id, op)) = hit {
            if self.selected.get() != Some(id) {
                self.select_text(Some(id));
            }
            if *self.tab.borrow() != "text" {
                self.set_tab("text");
            }
            if let Some(start) = d.layers.iter().find(|l| l.id == id).cloned() {
                *self.drag.borrow_mut() = Some(Drag::Layer { op, x, y, start: Box::new(start) });
            }
            return;
        }
        let (dx, dy) = v.to_design(x, y);
        let cells = layout_cells(&d);
        let Some(i) = cell_at(&cells, dx, dy) else {
            self.selected.set(None);
            self.sync_text();
            self.select_cell(None);
            return;
        };
        if self.selected.get().is_some() {
            self.selected.set(None);
            self.sync_text();
        }
        self.select_cell(Some(i));
        let c = cells[i];
        let slot = d.slots[i].clone();
        let Some(id) = slot.id.clone() else {
            self.pick(Some(i));
            return;
        };
        let (u, w) = to_cell(&c, dx, dy);
        let (gu, gv) = ((u - c.inner.x) * v.s, (w - c.inner.y) * v.s);
        if gu >= GRIP.0 && gu <= GRIP.1 && gv >= GRIP.0 && gv <= GRIP.1 {
            *self.drag.borrow_mut() = Some(Drag::Swap { from: i, over: None });
            let (ox, oy) = self.origin.get();
            self.set_ghost(true, Some(&id), ox + x, oy + y);
            return;
        }
        let (iw, ih) = self.dims(&id);
        let placed = place_image(c.inner.w, c.inner.h, iw, ih, &slot);
        *self.drag.borrow_mut() = Some(Drag::Pan { cell: i, x, y, start: slot, placed, win: c.inner, rot: c.rot });
    }

    fn drag_to(self: &Rc<Self>, x: f64, y: f64) {
        let Some(v) = self.view() else { return };
        let drag = self.drag.borrow().clone();
        match drag {
            Some(Drag::Pan { cell, x: x0, y: y0, start, placed, win, rot }) => {
                let (dx, dy) = rotate_delta(rot, (x - x0) / v.s, (y - y0) / v.s);
                let (px, py) = geometry::pan(&start, placed, win, dx, dy);
                let key = format!("pan.{cell}");
                self.set_slot(cell, Some(&key), |s| {
                    s.px = px;
                    s.py = py;
                });
            }
            Some(Drag::Swap { from, over }) => {
                let (dx, dy) = v.to_design(x, y);
                let n = cell_at(&layout_cells(&self.hist.borrow().doc), dx, dy).filter(|c| *c != from);
                let (ox, oy) = self.origin.get();
                self.set_ghost(true, None, ox + x, oy + y);
                if n != over {
                    *self.drag.borrow_mut() = Some(Drag::Swap { from, over: n });
                    self.publish_stage();
                }
            }
            Some(Drag::Layer { op, x: x0, y: y0, start }) => {
                let (w, h) = design_size(self.hist.borrow().doc.aspect);
                let center = v.to_stage(start.x, start.y);
                let (n, guides) = text::drag(op, &start, v.s, center, (w / 2.0, h / 2.0), x0, y0, x, y);
                self.set_guides(guides);
                let key = format!("layer.{op:?}.{}", start.id);
                let mut layers = self.doc().layers;
                if let Some(l) = layers.iter_mut().find(|l| l.id == start.id) {
                    *l = n;
                }
                text::LayerHost::commit(&**self, layers, Some(&key));
            }
            None => {}
        }
    }

    fn release(self: &Rc<Self>) {
        let drag = self.drag.borrow_mut().take();
        match drag {
            Some(Drag::Swap { from, over }) => {
                self.set_ghost(false, None, 0.0, 0.0);
                match over {
                    Some(to) => {
                        self.change(|d| d.swap(from, to), None);
                        self.select_cell(Some(to));
                    }
                    None => self.publish_stage(),
                }
            }
            Some(Drag::Layer { .. }) => self.set_guides((false, false)),
            _ => {}
        }
        self.hist.borrow_mut().seal();
    }

    fn double_click(self: &Rc<Self>, x: f64, y: f64) {
        let Some(v) = self.view() else { return };
        let d = self.doc();
        if let Some((id, _)) = self.rasters.borrow_mut().hit(&d.layers, self.selected.get(), v.s, |x, y| v.to_stage(x, y), x, y) {
            self.select_text(Some(id));
            self.set_tab("text");
            if let Some(a) = self.app() {
                text::focus_text(&a);
            }
            return;
        }
        let (dx, dy) = v.to_design(x, y);
        if let Some(i) = cell_at(&layout_cells(&d), dx, dy) {
            if d.slots[i].id.is_some() {
                self.pick(Some(i));
            }
        }
    }

    fn wheel(self: &Rc<Self>, x: f64, y: f64, dy: f64) {
        let Some(v) = self.view() else { return };
        let d = self.doc();
        let (px, py) = v.to_design(x, y);
        let Some(i) = cell_at(&layout_cells(&d), px, py) else { return };
        if d.slots[i].id.is_none() {
            return;
        }
        let zoom = (d.slots[i].zoom * (dy * 0.0022).exp()).clamp(1.0, 4.0);
        let key = format!("zoom.{i}");
        self.set_slot(i, Some(&key), |s| s.zoom = zoom);
        if self.cell.get() != Some(i) {
            self.select_cell(Some(i));
        }
    }

    fn set_guides(&self, (gx, gy): (bool, bool)) {
        if let Some(a) = self.app() {
            let s = a.window.global::<CollageState>();
            s.set_guide_x(gx);
            s.set_guide_y(gy);
        }
    }

    // ───────── save ─────────

    fn toggle_export(&self) {
        let Some(a) = self.app() else { return };
        let s = a.window.global::<CollageState>();
        if s.get_export_open() {
            if !self.exporting.get() {
                s.set_export_open(false);
            }
            return;
        }
        if !self.hist.borrow().doc.filled() {
            return;
        }
        let now = crate::fmt::now_ms() + crate::viewer_fmt::local_offset_ms();
        let date = crate::viewer_fmt::date_medium(now, a.lang());
        s.set_default_name(crate::i18n::tv(a.lang(), "collage.defaultName", &[("date", &date)]).into());
        s.set_format(self.format.borrow().clone().into());
        s.set_quality(self.quality.get() as f32);
        s.set_export_open(true);
    }

    fn change_target(&self) {
        let Some(a) = self.app() else { return };
        let title = a.t("collage.saveTo");
        crate::pages::dialogs::pick_folder(&a, &title, "", |_, t| {
            if let Some(c) = ctl() {
                *c.target.borrow_mut() = Some(t);
                c.publish();
            }
        });
    }

    fn save(self: &Rc<Self>, name: &str) {
        if self.exporting.get() {
            return;
        }
        let Some(a) = self.app() else { return };
        let Some(target) = self.target.borrow().clone() else {
            self.change_target();
            return;
        };
        let Some(ops) = a.backend.ops() else { return };
        let s = a.window.global::<CollageState>();
        let typed = name.trim();
        let name = sanitize(if typed.is_empty() { s.get_default_name().to_string() } else { typed.to_string() }.trim());
        let d = self.doc();
        let (ow, oh) = output_size(&d);
        let scale = d.long_side as f64 / DESIGN_LONG;
        let mut futs = HashMap::new();
        for id in d.slots[..d.count].iter().filter_map(|s| s.id.as_ref()) {
            futs.entry(id.clone()).or_insert_with(|| ops.image(id, ImageSize::Zoom));
        }
        let bg_fut = self.bg_photo(&d).map(|id| ops.image(&id, ImageSize::Thumb(1600)));
        let collection = (!d.layers.is_empty()).then(slint::fontique_011::shared_collection);
        self.exporting.set(true);
        s.set_exporting(true);
        s.set_progress(0.02);
        let rendering = a.t("collage.rendering");
        s.set_progress_label(rendering.clone().into());
        let gen = self.gen.get();
        #[cfg(not(target_arch = "wasm32"))]
        let handle = a.rt.handle().clone();
        let render = async move {
            // the browser has no worker thread that could wait for a download: they all finish
            // before the drawing starts
            #[cfg(target_arch = "wasm32")]
            let (bg, mut ready) = {
                let mips = |data| crate::editor::decode_full(data).ok().map(|img| Arc::new(Mips::new(img)));
                let mut ready = HashMap::new();
                for (id, f) in futs.drain() {
                    ready.insert(id, f.await.ok().and_then(mips));
                }
                let bg = match bg_fut {
                    Some(f) => f.await.ok().and_then(mips),
                    None => None,
                };
                (bg, ready)
            };
            crate::rt::spawn_blocking(move || -> Result<Vec<u8>, String> {
                use image::ImageEncoder;
                #[cfg(not(target_arch = "wasm32"))]
                let load = |f: crate::backend::BoxFut<crate::backend::ThumbData>| {
                    handle.block_on(f).ok().and_then(|data| crate::editor::decode_full(data).ok()).map(|img| Arc::new(Mips::new(img)))
                };
                #[cfg(not(target_arch = "wasm32"))]
                let bg = bg_fut.and_then(load);
                // the display renditions load one at a time, as the cells are drawn
                #[cfg(not(target_arch = "wasm32"))]
                let mut get = |id: &str| futs.remove(id).and_then(load);
                #[cfg(target_arch = "wasm32")]
                let mut get = |id: &str| ready.remove(id).flatten();
                let mut fonts = collection.map(crate::editor::overlay::Fonts::new);
                let canvas = render::render(&d, ow, oh, scale, bg.as_deref(), &mut get, fonts.as_mut(), false, &|p| progress(gen, p * 0.9, &rendering));
                let img = render::unpremultiply(canvas);
                let mut out = Vec::new();
                image::codecs::png::PngEncoder::new_with_quality(&mut out, image::codecs::png::CompressionType::Fast, image::codecs::png::FilterType::Adaptive)
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
                Err(e) => return c.saved(Err(e)),
            };
            let Some(ops) = a.backend.ops() else { return c.saved(Err("offline".into())) };
            let s = a.window.global::<CollageState>();
            s.set_progress(0.92);
            s.set_progress_label(a.t("common.saving").into());
            let fmt = c.format.borrow().clone();
            let up = ops.save_image(png, target, &name, &fmt, c.quality.get());
            let me = Rc::downgrade(&c);
            a.rt.spawn_then(async move { up.await.map_err(|e| e.message) }, move |res| {
                if let Some(c) = me.upgrade() {
                    c.saved(res);
                }
            });
        });
    }

    fn saved(&self, res: Result<serde_json::Value, String>) {
        let Some(a) = self.app() else { return };
        self.exporting.set(false);
        let s = a.window.global::<CollageState>();
        s.set_exporting(false);
        match res {
            Ok(v) => {
                s.set_progress(1.0);
                s.set_export_open(false);
                let str_at = |p: &str| v.pointer(p).and_then(|n| n.as_str()).unwrap_or_default().to_string();
                let route = Route::Folder(str_at("/item/folderId"), str_at("/item/dir"));
                crate::pages::dialogs::toast_open(&a, &a.t("collage.saved"), &str_at("/item/name"), route);
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
        if c.gen.get() != gen || !c.exporting.get() {
            return;
        }
        if let Some(a) = c.app() {
            let s = a.window.global::<CollageState>();
            s.set_progress(p);
            s.set_progress_label(label.into());
        }
    });
}

fn target_label(t: &DirTarget) -> String {
    let base = crate::actions_ui::folder_name(&t.folder_id).unwrap_or_default();
    match (base.is_empty(), t.dir.is_empty()) {
        (_, true) => base,
        (true, false) => t.dir.clone(),
        (false, false) => format!("{base} / {}", t.dir),
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
        let next = CollageDoc { layers, ..self.doc() };
        self.hist.borrow_mut().change(next, key);
        if let Some(a) = self.app() {
            let s = a.window.global::<CollageState>();
            let h = self.hist.borrow();
            s.set_can_undo(h.can_undo());
            s.set_can_redo(h.can_redo());
        }
        self.publish_stage();
    }

    fn area(&self) -> Rect {
        Ctl::area(self)
    }

    fn selected(&self) -> Option<u32> {
        self.selected.get()
    }

    fn select(&self, id: Option<u32>) {
        self.select_text(id);
    }

    fn next_id(&self) -> u32 {
        let used = self.hist.borrow().doc.layers.iter().map(|l| l.id).max().unwrap_or(0);
        let id = self.next_id.get().max(used + 1);
        self.next_id.set(id + 1);
        id
    }
}

/// The text panel edits the collage's layers.
fn attach() {
    text::attach(|cmd| {
        if let Some(c) = ctl() {
            text::run(&*c, cmd);
        }
    });
}

/// "Make collage" on a selection: a new collage of its photos (unless it is the one already open).
pub(crate) fn open_with(app: &Rc<AppInner>, items: Vec<MediaItem>) {
    let Some(c) = ctl() else { return };
    let imgs: Vec<MediaItem> = items.into_iter().filter(|i| i.kind == MediaKind::Image).collect();
    let key = imgs.iter().map(|i| i.id.as_str()).collect::<Vec<_>>().join(",");
    if !key.is_empty() && key != *c.source_key.borrow() {
        *c.source_key.borrow_mut() = key;
        let target = imgs.first().map(|i| DirTarget { folder_id: i.folder_id.clone(), dir: i.dir.clone() });
        c.start(imgs, target);
    }
    app.go(Route::Collage, true);
}

/// a route was entered: the collage page takes the text panel
pub(crate) fn enter(app: &Rc<AppInner>, route: &Route) {
    let Some(c) = ctl() else { return };
    if matches!(route, Route::Collage) {
        attach();
        crate::pages::fonts::load(app);
        text::publish_fonts(app);
        c.sync_text();
        c.publish();
        c.request_render();
    }
}

/// A font was registered: text drawn with a fallback is drawn again.
pub(crate) fn fonts_changed() {
    if let Some(c) = ctl() {
        c.rasters.borrow_mut().clear();
        c.publish_stage();
    }
}

/// Script step `cstate`: the collage's state on stderr.
pub(crate) fn debug_state() {
    let Some(c) = ctl() else { return };
    let d = c.doc();
    eprintln!(
        "[collage] photos={} count={} template={} aspect={} gap={} padding={} radius={} bg={:?} slots={:?} layers={:?} cell={:?} text={:?} undo={} target={:?}",
        c.photos.borrow().len(),
        d.count,
        d.template,
        d.aspect,
        d.gap,
        d.padding,
        d.radius,
        d.bg,
        d.slots[..d.count].iter().map(|s| (s.id.clone(), s.zoom, s.px, s.py)).collect::<Vec<_>>(),
        d.layers.iter().map(|l| (l.id, l.text.as_str(), l.x, l.y)).collect::<Vec<_>>(),
        c.cell.get(),
        c.selected.get(),
        c.hist.borrow().can_undo(),
        c.target.borrow()
    );
}

pub(crate) fn install(app: &Rc<AppInner>) {
    let lang = app.lang();
    let aspects = model::ASPECTS
        .iter()
        .map(|(k, r)| {
            let (w, h) = aspect_box(*r);
            let label = match *k {
                "a4" => "A4".to_string(),
                "9:16" => format!("9:16 · {}", crate::i18n::t(lang, "collage.story")),
                k => k.to_string(),
            };
            ClAspect { key: (*k).into(), label: label.into(), w, h, active: false }
        })
        .collect::<Vec<_>>();
    let gradients = model::GRADIENTS.iter().map(gradient_row).collect::<Vec<_>>();
    let c = Rc::new(Ctl {
        app: Rc::downgrade(app),
        photos: RefCell::new(Vec::new()),
        hist: RefCell::new(History::new(CollageDoc::new(&[]))),
        source_key: RefCell::new(String::new()),
        target: RefCell::new(None),
        tab: RefCell::new("layout".into()),
        cell: Cell::new(None),
        selected: Cell::new(None),
        next_id: Cell::new(1),
        stage: Cell::new((0.0, 0.0)),
        origin: Cell::new((0.0, 0.0)),
        previews: RefCell::new(HashMap::new()),
        loading: RefCell::new(HashSet::new()),
        tiles: RefCell::new(HashMap::new()),
        rendering: Cell::new(false),
        render_again: Cell::new(false),
        gen: Cell::new(0),
        drag: RefCell::new(None),
        pool_drag: Cell::new(None),
        drop_over: Cell::new(None),
        rasters: RefCell::new(text::Rasters::default()),
        layer_vis: Rc::new(VecModel::default()),
        cells: Rc::new(VecModel::default()),
        pool: Rc::new(VecModel::default()),
        templates: Rc::new(VecModel::default()),
        aspects: Rc::new(VecModel::from(aspects)),
        gradients: Rc::new(VecModel::from(gradients)),
        tpl_key: RefCell::new(None),
        exporting: Cell::new(false),
        format: RefCell::new("jpg".into()),
        quality: Cell::new(92),
    });
    CTL.with(|x| *x.borrow_mut() = Some(c.clone()));
    let s = app.window.global::<CollageState>();
    s.set_layers(ModelRc::from(c.layer_vis.clone()));
    s.set_cells(ModelRc::from(c.cells.clone()));
    s.set_photos(ModelRc::from(c.pool.clone()));
    s.set_templates(ModelRc::from(c.templates.clone()));
    s.set_aspects(ModelRc::from(c.aspects.clone()));
    s.set_gradients(ModelRc::from(c.gradients.clone()));
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
    on!(on_new_collage, |c| {
        let t = c.target.borrow().clone();
        c.start(Vec::new(), t);
    });
    on!(on_pick, |c| c.pick(None));
    on!(on_pick_for, |c, i| {
        let cell = if i < 0 { c.cell.get() } else { Some(i as usize) };
        if cell.is_some() {
            c.pick(cell);
        }
    });
    on!(on_set_tab, |c, t| c.set_tab(&t));
    on!(on_set_count, |c, n| c.change(|d| CollageDoc { count: (n.max(0) as usize).clamp(2, model::MAX_CELLS), ..d.clone() }, Some("count")));
    on!(on_set_template, |c, id| {
        let t = templates::template(&id).id;
        c.change(|d| CollageDoc { template: t, ..d.clone() }, None);
    });
    on!(on_shuffle, |c| c.change(|d| CollageDoc { seed: d.seed.wrapping_add(1), ..d.clone() }, None));
    on!(on_set_aspect, |c, k| {
        if let Some(k) = model::aspect_key(&k) {
            c.change(|d| d.with_aspect(k), None);
        }
    });
    on!(on_set_gap, |c, v| c.change(|d| CollageDoc { gap: v as f64, ..d.clone() }, Some("gap")));
    on!(on_set_padding, |c, v| c.change(|d| CollageDoc { padding: v as f64, ..d.clone() }, Some("padding")));
    on!(on_set_radius, |c, v| c.change(|d| CollageDoc { radius: v as f64, ..d.clone() }, Some("radius")));
    on!(on_set_bg_kind, |c, k| {
        let bg = match k.as_str() {
            "gradient" => Background::Gradient(model::GRADIENTS[0].id),
            "blur" => Background::Blur(40.0),
            _ => Background::Solid([0xff, 0xff, 0xff]),
        };
        c.change(|d| CollageDoc { bg, ..d.clone() }, None);
    });
    on!(on_set_bg_color, |c, col| {
        let bg = Background::Solid([col.red(), col.green(), col.blue()]);
        c.change(|d| CollageDoc { bg, ..d.clone() }, Some("bg.color"));
    });
    on!(on_set_gradient, |c, id| {
        let g = model::gradient(&id).id;
        c.change(|d| CollageDoc { bg: Background::Gradient(g), ..d.clone() }, None);
    });
    on!(on_set_blur, |c, v| c.change(|d| CollageDoc { bg: Background::Blur(v as f64), ..d.clone() }, Some("bg.blur")));
    on!(on_set_long_side, |c, v| {
        if let Ok(n) = v.parse::<u32>() {
            if model::RESOLUTIONS.contains(&n) {
                c.change(|d| CollageDoc { long_side: n, ..d.clone() }, None);
            }
        }
    });
    on!(on_place, |c, i| {
        let id = c.photos.borrow().get(i as usize).map(|p| p.id.clone());
        if let Some(id) = id {
            let cell = c.cell.get();
            c.change(|d| d.place_from_pool(cell, &id), None);
        }
    });
    on!(on_remove_photo, |c, i| c.remove_photo(i as usize));
    on!(on_pool_drag, |c, i, x, y| c.pool_drag(i as usize, x as f64, y as f64));
    on!(on_pool_drop, |c, i, x, y| c.pool_drop(i as usize, x as f64, y as f64));
    on!(on_set_zoom, |c, v| {
        if let Some(i) = c.cell.get() {
            let key = format!("zoom.{i}");
            c.set_slot(i, Some(&key), |s| s.zoom = (v as f64 / 100.0).clamp(1.0, 4.0));
        }
    });
    on!(on_recenter, |c| {
        if let Some(i) = c.cell.get() {
            c.set_slot(i, None, |s| {
                s.zoom = 1.0;
                s.px = 0.0;
                s.py = 0.0;
            });
        }
    });
    on!(on_clear_cell, |c| {
        if let Some(i) = c.cell.get() {
            c.set_slot(i, None, |s| *s = Slot::new(None));
        }
    });
    on!(on_delete_selected, |c| {
        if let Some(id) = c.selected.get() {
            text::run(&*c, text::TextCmd::Delete(id));
        } else if let Some(i) = c.cell.get() {
            c.set_slot(i, None, |s| *s = Slot::new(None));
        }
    });
    on!(on_escape, |c| {
        let open = c.app().is_some_and(|a| a.window.global::<CollageState>().get_export_open());
        if open {
            c.toggle_export();
        } else {
            c.selected.set(None);
            c.sync_text();
            c.select_cell(None);
        }
    });
    on!(on_resized, |c, w, h, x, y| {
        c.origin.set((x as f64, y as f64));
        let n = (w as f64, h as f64);
        if c.stage.get() != n {
            c.stage.set(n);
            c.publish_stage();
            c.request_render();
        }
    });
    on!(on_press, |c, x, y| c.press(x as f64, y as f64));
    on!(on_drag, |c, x, y| c.drag_to(x as f64, y as f64));
    on!(on_release, |c| c.release());
    on!(on_wheel, |c, x, y, dy| c.wheel(x as f64, y as f64, dy as f64));
    on!(on_double_click, |c, x, y| c.double_click(x as f64, y as f64));
    on!(on_toggle_export, |c| c.toggle_export());
    on!(on_change_target, |c| c.change_target());
    on!(on_set_format, |c, f| {
        *c.format.borrow_mut() = f.to_string();
        if let Some(a) = c.app() {
            a.window.global::<CollageState>().set_format(f);
        }
    });
    on!(on_set_quality, |c, q| {
        c.quality.set(q.round().clamp(40.0, 100.0) as u32);
        if let Some(a) = c.app() {
            a.window.global::<CollageState>().set_quality(q);
        }
    });
    on!(on_save, |c, name| c.save(&name));
}
