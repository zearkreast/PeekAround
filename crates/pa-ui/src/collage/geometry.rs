//! Cell rectangles in design units and photo placement inside a cell (features/collage/geometry.ts).

use super::model::{design_size, CollageDoc, Slot};
use super::templates::{self, CellStyle};

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Box2 {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// A laid-out cell in design units; `inner` is the photo window relative to the cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellGeom {
    pub index: usize,
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// degrees, clockwise around the cell centre
    pub rot: f64,
    pub style: CellStyle,
    pub radius: f64,
    pub inner: Box2,
    pub inner_radius: f64,
    pub free: bool,
}

const EPS: f64 = 1e-6;

/// The template of the doc resolved into concrete cells (design units).
pub fn layout_cells(d: &CollageDoc) -> Vec<CellGeom> {
    layout(d.template, d.count, d.aspect, d.seed, d.gap, d.padding, d.radius)
}

pub fn layout(template: &str, count: usize, aspect: &str, seed: u32, gap: f64, padding: f64, radius: f64) -> Vec<CellGeom> {
    let (w0, h0) = design_size(aspect);
    let tpl = templates::template(template);
    let p = padding;
    let content = Box2 { x: p, y: p, w: w0 - p * 2.0, h: h0 - p * 2.0 };
    let ratio = if content.h > 0.0 && content.w / content.h > 0.0 { content.w / content.h } else { w0 / h0 };
    let g = gap / 2.0;
    templates::make(tpl.id, count, ratio, seed)
        .into_iter()
        .enumerate()
        .map(|(index, c)| {
            if tpl.free {
                let w = c.w * content.w;
                let h = c.h * content.h;
                let polaroid = c.style == CellStyle::Polaroid;
                let side = if polaroid { w * 0.055 } else { (w * 0.03).max(6.0) };
                let bottom = if polaroid { w * 0.2 } else { side };
                return CellGeom {
                    index,
                    x: content.x + c.x * content.w - w / 2.0,
                    y: content.y + c.y * content.h - h / 2.0,
                    w,
                    h,
                    rot: c.rot,
                    style: c.style,
                    radius: (radius * 0.35).min(w * 0.05),
                    inner: Box2 { x: side, y: side, w: w - side * 2.0, h: h - side - bottom },
                    inner_radius: (radius * 0.2).min(w * 0.03),
                    free: true,
                };
            }
            let mut x = content.x + c.x * content.w;
            let mut y = content.y + c.y * content.h;
            let mut w = c.w * content.w;
            let mut h = c.h * content.h;
            if c.x > EPS {
                x += g;
                w -= g;
            }
            if c.x + c.w < 1.0 - EPS {
                w -= g;
            }
            if c.y > EPS {
                y += g;
                h -= g;
            }
            if c.y + c.h < 1.0 - EPS {
                h -= g;
            }
            let w = w.max(1.0);
            let h = h.max(1.0);
            let r = radius.min(w / 2.0).min(h / 2.0);
            CellGeom {
                index,
                x,
                y,
                w,
                h,
                rot: 0.0,
                style: c.style,
                radius: r,
                inner: Box2 { x: 0.0, y: 0.0, w, h },
                inner_radius: r,
                free: false,
            }
        })
        .collect()
}

/// Where the photo is drawn inside its window (relative to the window): cover fit, zoom, pan.
pub fn place_image(win_w: f64, win_h: f64, img_w: f64, img_h: f64, slot: &Slot) -> Box2 {
    let s = (win_w / img_w).max(win_h / img_h) * slot.zoom.max(1.0);
    let w = img_w * s;
    let h = img_h * s;
    Box2 { x: (win_w - w) / 2.0 + slot.px * (w - win_w) / 2.0, y: (win_h - h) / 2.0 + slot.py * (h - win_h) / 2.0, w, h }
}

/// Cover a w×h box with an image, scaled by `over` (hides blurred edges).
pub fn cover_rect(w: f64, h: f64, iw: f64, ih: f64, over: f64) -> Box2 {
    let s = (w / iw).max(h / ih) * over;
    Box2 { x: (w - iw * s) / 2.0, y: (h - ih * s) / 2.0, w: iw * s, h: ih * s }
}

/// Pan after a pointer move of (dx, dy) design units in the cell's own (rotated) frame
/// (CollageCanvas onCellDown): the overflow of the photo past its window maps to −1…1.
pub fn pan(start: &Slot, placed: Box2, win: Box2, dx: f64, dy: f64) -> (f64, f64) {
    let ovx = placed.w - win.w;
    let ovy = placed.h - win.h;
    let px = if ovx > 0.5 { (start.px + 2.0 * dx / ovx).clamp(-1.0, 1.0) } else { 0.0 };
    let py = if ovy > 0.5 { (start.py + 2.0 * dy / ovy).clamp(-1.0, 1.0) } else { 0.0 };
    (px, py)
}

/// A point (design units) in the cell's unrotated frame, relative to its top-left corner.
pub fn to_cell(c: &CellGeom, x: f64, y: f64) -> (f64, f64) {
    let (cx, cy) = (c.x + c.w / 2.0, c.y + c.h / 2.0);
    let (sin, cos) = c.rot.to_radians().sin_cos();
    let (dx, dy) = (x - cx, y - cy);
    (dx * cos + dy * sin + c.w / 2.0, -dx * sin + dy * cos + c.h / 2.0)
}

/// The top-most cell under a design point (free cells overlap: later ones are on top).
pub fn cell_at(cells: &[CellGeom], x: f64, y: f64) -> Option<usize> {
    cells.iter().rev().find_map(|c| {
        let (u, v) = to_cell(c, x, y);
        (u >= 0.0 && v >= 0.0 && u <= c.w && v <= c.h).then_some(c.index)
    })
}

/// A pointer delta (design units) turned into the cell's rotated frame.
pub fn rotate_delta(rot: f64, dx: f64, dy: f64) -> (f64, f64) {
    let (sin, cos) = (-rot).to_radians().sin_cos();
    (dx * cos - dy * sin, dx * sin + dy * cos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collage::model::CollageDoc;

    #[test]
    fn grid_cells_leave_gaps_and_padding() {
        let mut d = CollageDoc::new(&[]);
        d.aspect = "1:1";
        d.count = 4;
        d.gap = 20.0;
        d.padding = 40.0;
        let cells = layout_cells(&d);
        assert_eq!(cells.len(), 4);
        let a = cells[0];
        assert_eq!((a.x, a.y), (40.0, 40.0));
        assert_eq!(a.w, 960.0 - 10.0);
        let b = cells[1];
        assert_eq!(b.x, 40.0 + 960.0 + 10.0);
        assert_eq!(b.x + b.w, 2000.0 - 40.0);
        assert_eq!(b.x - (a.x + a.w), 20.0);
        assert_eq!(a.radius, 12.0);
        assert_eq!(a.inner, Box2 { x: 0.0, y: 0.0, w: a.w, h: a.h });
    }

    #[test]
    fn radius_is_capped_by_the_cell() {
        let mut d = CollageDoc::new(&[]);
        d.radius = 5000.0;
        let c = layout_cells(&d)[0];
        assert_eq!(c.radius, c.w.min(c.h) / 2.0);
    }

    #[test]
    fn free_cells_have_a_paper_frame() {
        let mut d = CollageDoc::new(&[]);
        d.template = "polaroid";
        let cells = layout_cells(&d);
        for c in &cells {
            assert!(c.free);
            assert!((c.inner.x - c.w * 0.055).abs() < 1e-9);
            assert!((c.h - c.inner.y - c.inner.h - c.w * 0.2).abs() < 1e-9);
            assert!(c.rot != 0.0);
        }
    }

    #[test]
    fn photos_cover_their_window() {
        let s = Slot::new(Some("a".into()));
        let r = place_image(100.0, 100.0, 400.0, 200.0, &s);
        assert_eq!(r, Box2 { x: -50.0, y: 0.0, w: 200.0, h: 100.0 });
        let left = place_image(100.0, 100.0, 400.0, 200.0, &Slot { px: 1.0, ..s.clone() });
        assert_eq!(left.x, 0.0);
        let z = place_image(100.0, 100.0, 400.0, 200.0, &Slot { zoom: 2.0, py: -1.0, ..s.clone() });
        assert_eq!((z.w, z.h, z.y), (400.0, 200.0, -100.0));
        let c = cover_rect(100.0, 50.0, 10.0, 10.0, 1.0);
        assert_eq!(c, Box2 { x: 0.0, y: -25.0, w: 100.0, h: 100.0 });
    }

    #[test]
    fn pan_clamps_and_ignores_axes_without_overflow() {
        let s = Slot::new(Some("a".into()));
        let win = Box2 { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
        let placed = place_image(100.0, 100.0, 400.0, 200.0, &s);
        assert_eq!(pan(&s, placed, win, 25.0, 30.0), (0.5, 0.0));
        assert_eq!(pan(&s, placed, win, 500.0, 0.0), (1.0, 0.0));
    }

    #[test]
    fn hit_testing_follows_rotation() {
        let mut d = CollageDoc::new(&[]);
        d.template = "polaroid";
        d.count = 2;
        let cells = layout_cells(&d);
        let c = cells[1];
        let (cx, cy) = (c.x + c.w / 2.0, c.y + c.h / 2.0);
        assert_eq!(cell_at(&cells, cx, cy), Some(1));
        let (u, v) = to_cell(&c, cx, cy);
        assert!((u - c.w / 2.0).abs() < 1e-9 && (v - c.h / 2.0).abs() < 1e-9);
        assert_eq!(cell_at(&cells, -10.0, -10.0), None);
        let (dx, dy) = rotate_delta(90.0, 10.0, 0.0);
        assert!(dx.abs() < 1e-9 && (dy + 10.0).abs() < 1e-9);
    }
}
