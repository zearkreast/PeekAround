//! Text layers shared by the photo editor and the collage (lib/text.ts, panels/TextPanel
//! `TextLayersPanel`, components/TextLayerControls, konva/TextNode): the panel's layer list and
//! the selected layer's properties on the `TextLayers` global, the commands behind them, the
//! cached layer images on a stage and the move / resize / rotate gestures. Layer coordinates are
//! the host's units (frame px in the editor, design units in the collage).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use slint::{Color, ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};

use super::doc::{Align, Rect, TextLayer, TextShadow};
use super::overlay::Fonts;
use super::textprops::{self, to_color, DEFAULT_FAMILY};
use crate::app::AppInner;
use crate::{EdFont, EdLayerRow, EdLayerVis, TextLayers};

/// selection frame padding and rotate-handle distance (stage px), as the Konva transformer
const FRAME_PAD: f64 = 6.0;
const ROTATE_OFFSET: f64 = 26.0;

/// Emoji the sticker grid offers (panels/TextPanel STICKERS).
pub(crate) const STICKERS: [&str; 16] =
    ["😍", "😂", "🥳", "❤️", "🔥", "✨", "⭐", "🌸", "☀️", "🌈", "🎉", "👍", "😎", "🤩", "🐾", "🎈"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LayerOp {
    Move,
    Scale,
    Rotate,
}

/// Size slider top for a layer area (the crop / the canvas); outline and background follow it.
fn slider_max(area: Rect) -> f64 {
    (area.w.max(area.h) * 0.5).round().max(200.0)
}

pub(crate) fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

pub(crate) fn to_image(px: Vec<u8>, w: u32, h: u32) -> Image {
    let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(w, h);
    buf.make_mut_bytes().copy_from_slice(&px);
    Image::from_rgba8_premultiplied(buf)
}

// ───────── layer images ─────────

/// A layer's raster at one stage scale.
#[derive(Clone)]
pub(crate) struct Cached {
    look: TextLayer,
    scale: f64,
    epoch: usize,
    image: Image,
    w: f64,
    h: f64,
    box_w: f64,
    box_h: f64,
    /// raster px per layer unit
    rs: f64,
}

/// Text shaping and the layer images of one stage (UI thread).
#[derive(Default)]
pub(crate) struct Rasters {
    fonts: Option<Fonts>,
    map: HashMap<u32, Cached>,
}

impl Rasters {
    fn fonts(&mut self) -> &mut Fonts {
        self.fonts.get_or_insert_with(|| Fonts::new(slint::fontique_011::shared_collection()))
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }

    fn raster(&mut self, l: &TextLayer, scale: f64) -> Cached {
        let epoch = crate::pages::fonts::epoch();
        if let Some(c) = self.map.get(&l.id) {
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
        self.map.insert(l.id, c.clone());
        c
    }

    /// box (text + background padding) in layer units
    pub fn box_of(&mut self, l: &TextLayer) -> (f64, f64) {
        if let Some(c) = self.map.get(&l.id) {
            if c.look.look_eq(l) {
                return (c.box_w / c.rs, c.box_h / c.rs);
            }
        }
        self.fonts().measure(l)
    }

    /// The layers on a stage: `scale` stage px per layer unit, `sf` the window scale factor,
    /// `to_stage` a layer point on the stage. Rasters of removed layers are dropped.
    pub fn vis(&mut self, layers: &[TextLayer], sel: Option<u32>, scale: f64, sf: f64, to_stage: impl Fn(f64, f64) -> (f64, f64)) -> Vec<EdLayerVis> {
        let out = layers
            .iter()
            .map(|l| {
                let c = self.raster(l, scale * sf);
                let k = scale / c.rs;
                let (x, y) = to_stage(l.x, l.y);
                EdLayerVis {
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
                    selected: sel == Some(l.id),
                }
            })
            .collect();
        self.map.retain(|id, _| layers.iter().any(|l| l.id == *id));
        out
    }

    /// The layer (and which part of its frame) under a stage point; handles of the selected
    /// layer first, then the layers top-most first.
    pub fn hit(&mut self, layers: &[TextLayer], sel: Option<u32>, scale: f64, to_stage: impl Fn(f64, f64) -> (f64, f64), x: f64, y: f64) -> Option<(u32, LayerOp)> {
        if let Some(l) = sel.and_then(|id| layers.iter().find(|l| l.id == id)) {
            let (bw, bh) = self.box_of(l);
            let (hw, hh) = (bw * scale / 2.0 + FRAME_PAD, bh * scale / 2.0 + FRAME_PAD);
            let (cx, cy) = to_stage(l.x, l.y);
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
        for l in layers.iter().rev() {
            let (bw, bh) = self.box_of(l);
            let (cx, cy) = to_stage(l.x, l.y);
            let (sin, cos) = l.rotation.to_radians().sin_cos();
            let (dx, dy) = (x - cx, y - cy);
            let lx = dx * cos + dy * sin;
            let ly = -dx * sin + dy * cos;
            if lx.abs() <= bw * scale / 2.0 + 4.0 && ly.abs() <= bh * scale / 2.0 + 4.0 {
                return Some((l.id, LayerOp::Move));
            }
        }
        None
    }
}

/// A layer gesture from stage point (x0, y0) to (x, y): `center` is the layer's start centre on
/// the stage, `snap` the area centre (layer units) a move snaps to. Returns the guides shown.
pub(crate) fn drag(op: LayerOp, start: &TextLayer, scale: f64, center: (f64, f64), snap: (f64, f64), x0: f64, y0: f64, x: f64, y: f64) -> (TextLayer, (bool, bool)) {
    let (cx, cy) = center;
    let mut guides = (false, false);
    let n = match op {
        LayerOp::Move => {
            let tol = 8.0 / scale;
            let mut nx = start.x + (x - x0) / scale;
            let mut ny = start.y + (y - y0) / scale;
            if (nx - snap.0).abs() < tol {
                nx = snap.0;
                guides.0 = true;
            }
            if (ny - snap.1).abs() < tol {
                ny = snap.1;
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
    (n, guides)
}

// ───────── layers ─────────

/// A new layer in the middle of `area`: the default text, or an emoji sticker; `n` existing
/// layers step it down-right so new ones don't stack exactly.
pub(crate) fn new_layer(id: u32, text: String, sticker: bool, area: Rect, n: usize) -> TextLayer {
    let short = area.w.min(area.h);
    let off = (n % 5) as f64 * short * 0.04;
    TextLayer {
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
        x: area.x + area.w / 2.0 + off,
        y: area.y + area.h / 2.0 + off,
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
    }
}

// ───────── the panel ─────────

/// What a tool with text layers gives the shared panel.
pub(crate) trait LayerHost {
    fn app(&self) -> Option<Rc<AppInner>>;
    fn layers(&self) -> Vec<TextLayer>;
    /// replace the layers (one undo step unless `key` repeats), then publish
    fn commit(&self, layers: Vec<TextLayer>, key: Option<&str>);
    /// where new layers go; the slider ranges follow its size
    fn area(&self) -> Rect;
    fn selected(&self) -> Option<u32>;
    /// select (or deselect), then publish
    fn select(&self, id: Option<u32>);
    fn next_id(&self) -> u32;
}

/// A command of the panel (the `TextLayers` callbacks).
pub(crate) enum TextCmd {
    Add(String),
    Select(u32),
    Deselect,
    Duplicate(u32),
    Delete(u32),
    DeleteSelected,
    Restack(u32, i32),
    Num(String, f32),
    Str(String, String),
    Color(String, Color),
    Toggle(String, bool),
}

fn edit(h: &dyn LayerHost, key: &str, f: impl FnOnce(&mut TextLayer)) {
    let Some(id) = h.selected() else { return };
    let mut layers = h.layers();
    let Some(l) = layers.iter_mut().find(|l| l.id == id) else { return };
    f(l);
    h.commit(layers, Some(&format!("text.{id}.{key}")));
}

pub(crate) fn run(h: &dyn LayerHost, cmd: TextCmd) {
    match cmd {
        TextCmd::Add(sticker) => {
            let Some(a) = h.app() else { return };
            let mut layers = h.layers();
            let id = h.next_id();
            let text = if sticker.is_empty() { a.t("photo.text.default") } else { sticker.clone() };
            layers.push(new_layer(id, text, !sticker.is_empty(), h.area(), layers.len()));
            h.commit(layers, None);
            h.select(Some(id));
        }
        TextCmd::Select(id) => h.select(Some(id)),
        TextCmd::Deselect => h.select(None),
        TextCmd::Duplicate(id) => {
            let mut layers = h.layers();
            let Some(src) = layers.iter().find(|l| l.id == id).cloned() else { return };
            let a = h.area();
            let d = a.w.min(a.h) * 0.03;
            let nid = h.next_id();
            layers.push(TextLayer { id: nid, x: src.x + d, y: src.y + d, ..src });
            h.commit(layers, None);
            h.select(Some(nid));
        }
        TextCmd::Delete(id) => delete(h, id),
        TextCmd::DeleteSelected => {
            if let Some(id) = h.selected() {
                delete(h, id);
            }
        }
        TextCmd::Restack(id, dir) => {
            let mut layers = h.layers();
            if let Some(i) = layers.iter().position(|l| l.id == id) {
                let j = i as i64 + dir.signum() as i64;
                if j >= 0 && (j as usize) < layers.len() {
                    layers.swap(i, j as usize);
                    h.commit(layers, None);
                }
            }
        }
        TextCmd::Num(key, v) => edit(h, &key, |l| textprops::set_num(l, &key, v as f64)),
        TextCmd::Str(key, v) => edit(h, &key, |l| textprops::set_str(l, &key, &v)),
        TextCmd::Color(key, c) => edit(h, &key, |l| textprops::set_color(l, &key, c)),
        TextCmd::Toggle(key, on) => edit(h, &key, |l| textprops::toggle(l, &key, on)),
    }
}

fn delete(h: &dyn LayerHost, id: u32) {
    if h.selected() == Some(id) {
        h.select(None);
    }
    let mut layers = h.layers();
    layers.retain(|l| l.id != id);
    h.commit(layers, None);
}

thread_local! {
    static ROWS: Rc<VecModel<EdLayerRow>> = Rc::new(VecModel::default());
    /// which tool the panel talks to
    static HOST: RefCell<Option<fn(TextCmd)>> = const { RefCell::new(None) };
}

/// The panel now edits this tool's layers.
pub(crate) fn attach(route: fn(TextCmd)) {
    HOST.with(|h| *h.borrow_mut() = Some(route));
}

/// The layer list and the selected layer's properties.
pub(crate) fn publish_panel(app: &AppInner, layers: &[TextLayer], sel: Option<u32>, area: Rect) {
    let s = app.window.global::<TextLayers>();
    let rows: Vec<EdLayerRow> = layers
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
    ROWS.with(|m| {
        if m.iter().ne(rows.iter().cloned()) {
            m.set_vec(rows);
        }
    });
    let l = sel.and_then(|id| layers.iter().find(|l| l.id == id));
    s.set_has_sel(l.is_some());
    if let Some(l) = l {
        s.set_sel(textprops::props(l, slider_max(area)));
    }
}

/// The text box shows the selected layer's text again (selection change, undo).
pub(crate) fn sync_text(app: &AppInner, layers: &[TextLayer], sel: Option<u32>, area: Rect) {
    let s = app.window.global::<TextLayers>();
    if let Some(l) = sel.and_then(|id| layers.iter().find(|l| l.id == id)) {
        s.set_sel(textprops::props(l, slider_max(area)));
    }
    s.set_text_sync(s.get_text_sync() + 1);
}

/// Put the cursor in the text box (double click on a layer).
pub(crate) fn focus_text(app: &AppInner) {
    let s = app.window.global::<TextLayers>();
    s.set_focus_text(s.get_focus_text() + 1);
}

pub(crate) fn publish_fonts(app: &AppInner) {
    let fonts: Vec<EdFont> = crate::pages::fonts::picker()
        .into_iter()
        .map(|(family, ..)| EdFont { label: family.clone().into(), family: family.into() })
        .collect();
    app.window.global::<TextLayers>().set_fonts(ModelRc::new(VecModel::from(fonts)));
}

pub(crate) fn install(app: &Rc<AppInner>) {
    let s = app.window.global::<TextLayers>();
    s.set_rows(ROWS.with(|m| ModelRc::from(m.clone())));
    s.set_stickers(ModelRc::new(VecModel::from(STICKERS.iter().map(|s| SharedString::from(*s)).collect::<Vec<_>>())));
    fn send(cmd: TextCmd) {
        if let Some(f) = HOST.with(|h| *h.borrow()) {
            f(cmd);
        }
    }
    s.on_add_text(|v| send(TextCmd::Add(v.to_string())));
    s.on_select_layer(|id| send(TextCmd::Select(id as u32)));
    s.on_deselect(|| send(TextCmd::Deselect));
    s.on_duplicate_layer(|id| send(TextCmd::Duplicate(id as u32)));
    s.on_delete_layer(|id| send(TextCmd::Delete(id as u32)));
    s.on_delete_selected(|| send(TextCmd::DeleteSelected));
    s.on_restack(|id, dir| send(TextCmd::Restack(id as u32, dir)));
    s.on_text_num(|k, v| send(TextCmd::Num(k.to_string(), v)));
    s.on_text_str(|k, v| send(TextCmd::Str(k.to_string(), v.to_string())));
    s.on_text_color(|k, v| send(TextCmd::Color(k.to_string(), v)));
    s.on_text_toggle(|k, v| send(TextCmd::Toggle(k.to_string(), v)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_layers_sit_in_the_area_centre() {
        let a = Rect { x: 100.0, y: 0.0, w: 1000.0, h: 500.0 };
        let l = new_layer(3, "Hi".into(), false, a, 0);
        assert_eq!((l.x, l.y, l.size), (600.0, 250.0, 45.0));
        assert!(l.shadow.is_some());
        let s = new_layer(4, "🔥".into(), true, a, 2);
        assert_eq!((s.x, s.y, s.size, s.weight), (640.0, 290.0, 80.0, 400.0));
        assert!(s.shadow.is_none());
    }

    #[test]
    fn commands_change_one_property() {
        let mut l = new_layer(1, "Hi".into(), false, Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 }, 0);
        textprops::set_num(&mut l, "opacity", 50.0);
        assert_eq!(l.opacity, 0.5);
        textprops::toggle(&mut l, "bg", true);
        assert_eq!(l.background.unwrap().padding, (l.size * 0.3).round());
        textprops::set_color(&mut l, "bg.color", slint::Color::from_rgb_u8(255, 0, 0));
        assert_eq!(l.background.unwrap().color, [255, 0, 0, 140]);
        textprops::set_str(&mut l, "align", "right");
        assert_eq!(l.align, Align::Right);
        textprops::set_num(&mut l, "stroke.width", 4.0);
        assert!(l.stroke.is_none());
    }

    #[test]
    fn moves_snap_to_the_centre() {
        let l = new_layer(1, "Hi".into(), false, Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 }, 0);
        let (m, g) = drag(LayerOp::Move, &l, 2.0, (100.0, 100.0), (50.0, 50.0), 100.0, 100.0, 110.0, 160.0);
        assert_eq!((m.x, m.y), (55.0, 80.0));
        assert_eq!(g, (false, false));
        let (m, g) = drag(LayerOp::Move, &l, 2.0, (100.0, 100.0), (50.0, 50.0), 100.0, 100.0, 104.0, 100.0);
        assert_eq!((m.x, g.0), (50.0, true));
        let (r, _) = drag(LayerOp::Rotate, &l, 1.0, (0.0, 0.0), (0.0, 0.0), 10.0, 0.0, 0.0, 10.0);
        assert_eq!(r.rotation, 90.0);
    }
}
