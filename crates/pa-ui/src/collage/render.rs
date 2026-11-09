//! The collage on the CPU (features/collage/render.ts): background, cells with their photos,
//! paper frames and shadows of the free layouts, and (for the saved file) the text layers. The
//! preview on the stage and the export go through the same code at different scales. Output is
//! premultiplied RGBA ([`Canvas`]).

use image::RgbaImage;

use super::geometry::{cover_rect, layout_cells, place_image, CellGeom};
use super::model::{gradient, Background, CollageDoc, GradientPreset, Rgb};
use crate::editor::overlay::{self, Canvas, Fonts};

/// Box-filtered halvings of a photo; sampling picks the level closest above the output size, so
/// a 6000 px original drawn 800 px wide doesn't alias.
pub struct Mips(Vec<RgbaImage>);

impl Mips {
    pub fn new(img: RgbaImage) -> Mips {
        let mut v = vec![img];
        loop {
            let last = v.last().expect("one level");
            let (w, h) = last.dimensions();
            if w.min(h) < 64 {
                break;
            }
            let next = halve(last);
            v.push(next);
        }
        Mips(v)
    }

    pub fn width(&self) -> f64 {
        self.0[0].width() as f64
    }

    pub fn height(&self) -> f64 {
        self.0[0].height() as f64
    }

    /// The level to sample when `k` source px land on one output px, and its scale.
    fn level(&self, k: f64) -> (&RgbaImage, f64) {
        let mut l = 0;
        let mut f = 1.0;
        while l + 1 < self.0.len() && k / (f * 2.0) >= 1.0 {
            l += 1;
            f *= 2.0;
        }
        let img = &self.0[l];
        (img, img.width() as f64 / self.width())
    }
}

fn halve(src: &RgbaImage) -> RgbaImage {
    let (w, h) = src.dimensions();
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let s = src.as_raw();
    let mut out = vec![0u8; nw as usize * nh as usize * 4];
    for y in 0..nh as usize {
        let y0 = (2 * y).min(h as usize - 1);
        let y1 = (2 * y + 1).min(h as usize - 1);
        for x in 0..nw as usize {
            let x0 = (2 * x).min(w as usize - 1);
            let x1 = (2 * x + 1).min(w as usize - 1);
            let i = |xx: usize, yy: usize| (yy * w as usize + xx) * 4;
            let (a, b, c, d) = (i(x0, y0), i(x1, y0), i(x0, y1), i(x1, y1));
            let o = (y * nw as usize + x) * 4;
            for ch in 0..4 {
                out[o + ch] = ((s[a + ch] as u32 + s[b + ch] as u32 + s[c + ch] as u32 + s[d + ch] as u32 + 2) / 4) as u8;
            }
        }
    }
    RgbaImage::from_raw(nw, nh, out).expect("size")
}

/// Bilinear sample at (u, v) source px (pixel centres at .5), clamped to the edge; premultiplied.
fn sample(img: &RgbaImage, u: f64, v: f64) -> [f32; 4] {
    let (w, h) = (img.width() as i64, img.height() as i64);
    let (u, v) = (u - 0.5, v - 0.5);
    let (x0, y0) = (u.floor(), v.floor());
    let (fx, fy) = ((u - x0) as f32, (v - y0) as f32);
    let (x0, y0) = (x0 as i64, y0 as i64);
    let px = img.as_raw();
    let mut acc = [0.0f32; 4];
    for (dy, wy) in [(0, 1.0 - fy), (1, fy)] {
        let y = (y0 + dy).clamp(0, h - 1);
        for (dx, wx) in [(0, 1.0 - fx), (1, fx)] {
            let wgt = wx * wy;
            if wgt == 0.0 {
                continue;
            }
            let x = (x0 + dx).clamp(0, w - 1);
            let i = ((y * w + x) * 4) as usize;
            let a = px[i + 3] as f32 / 255.0;
            acc[0] += px[i] as f32 / 255.0 * a * wgt;
            acc[1] += px[i + 1] as f32 / 255.0 * a * wgt;
            acc[2] += px[i + 2] as f32 / 255.0 * a * wgt;
            acc[3] += a * wgt;
        }
    }
    acc
}

/// Premultiplied source-over into one u8 pixel.
#[inline]
fn over(d: &mut [u8], s: [f32; 4]) {
    let inv = 1.0 - s[3];
    for c in 0..4 {
        d[c] = ((s[c] + d[c] as f32 / 255.0 * inv) * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
    }
}

fn rgba(c: Rgb, a: f32) -> [f32; 4] {
    [c[0] as f32 / 255.0 * a, c[1] as f32 / 255.0 * a, c[2] as f32 / 255.0 * a, a]
}

/// Signed distance (px) from a point to a rounded rect given by its half size and radius,
/// relative to the rect's centre.
#[inline]
fn sd_round_rect(px: f64, py: f64, hw: f64, hh: f64, r: f64) -> f64 {
    let r = r.min(hw).min(hh).max(0.0);
    let qx = px.abs() - (hw - r);
    let qy = py.abs() - (hh - r);
    let outside = qx.max(0.0).hypot(qy.max(0.0));
    outside + qx.max(qy).min(0.0) - r
}

#[inline]
fn coverage(sd: f64) -> f32 {
    (0.5 - sd).clamp(0.0, 1.0) as f32
}

fn fill(c: &mut Canvas, color: [f32; 4]) {
    for p in c.px.as_chunks_mut::<4>().0.iter_mut() {
        over(p, color);
    }
}

/// Canvas `createLinearGradient` along a CSS angle over the whole canvas (geometry.ts).
fn fill_gradient(c: &mut Canvas, g: &GradientPreset) {
    let (w, h) = (c.w as f64, c.h as f64);
    let a = g.angle.to_radians();
    let (dx, dy) = (a.sin(), -a.cos());
    let len = (w * dx).abs() + (h * dy).abs();
    let (sx, sy) = (w / 2.0 - dx * len / 2.0, h / 2.0 - dy * len / 2.0);
    let stops = g.stops;
    let color_at = |t: f64| -> [f32; 4] {
        let t = t.clamp(0.0, 1.0);
        let mut prev = stops[0];
        for &s in stops {
            if t <= s.0 {
                let span = (s.0 - prev.0).max(1e-9);
                let k = if s.0 == prev.0 { 1.0 } else { ((t - prev.0) / span) as f32 };
                let mix = |i: usize| (prev.1[i] as f32 + (s.1[i] as f32 - prev.1[i] as f32) * k) / 255.0;
                return [mix(0), mix(1), mix(2), 1.0];
            }
            prev = s;
        }
        let l = stops[stops.len() - 1].1;
        [l[0] as f32 / 255.0, l[1] as f32 / 255.0, l[2] as f32 / 255.0, 1.0]
    };
    let cw = c.w as usize;
    for (y, row) in c.px.chunks_exact_mut(cw * 4).enumerate() {
        for (x, p) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let t = ((x as f64 + 0.5 - sx) * dx + (y as f64 + 0.5 - sy) * dy) / len;
            over(p, color_at(t));
        }
    }
}

/// The blurred, slightly darkened cover photo behind everything (render.ts `drawBlurred`):
/// blurred at a reduced size with a margin (no dark edges), then scaled up.
fn fill_blurred(c: &mut Canvas, img: &Mips, sigma: f64) {
    let (w, h) = (c.w as f64, c.h as f64);
    let r = cover_rect(w, h, img.width(), img.height(), 1.12);
    let k = (4.0 / sigma.max(1e-3)).min(1.0);
    let margin = (4.0 * sigma * k).ceil() + 2.0;
    let (sw, sh) = (((w * k).ceil() + 2.0 * margin) as usize, ((h * k).ceil() + 2.0 * margin) as usize);
    let (lvl, ls) = img.level(img.width() / r.w / k);
    let mut planes = [vec![0f32; sw * sh], vec![0f32; sw * sh], vec![0f32; sw * sh]];
    for y in 0..sh {
        for x in 0..sw {
            // small-buffer px → canvas px → image px
            let cx = (x as f64 + 0.5 - margin) / k;
            let cy = (y as f64 + 0.5 - margin) / k;
            let u = (cx - r.x) / r.w * img.width() * ls;
            let v = (cy - r.y) / r.h * img.height() * ls;
            let s = sample(lvl, u, v);
            let i = y * sw + x;
            planes[0][i] = s[0];
            planes[1][i] = s[1];
            planes[2][i] = s[2];
        }
    }
    for p in planes.iter_mut() {
        overlay::blur(p, sw, sh, sigma * k);
    }
    let cw = c.w as usize;
    for (y, row) in c.px.chunks_exact_mut(cw * 4).enumerate() {
        let v = (y as f64 + 0.5) * k + margin - 0.5;
        let y0 = v.floor().clamp(0.0, (sh - 1) as f64) as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let fy = (v - v.floor()).clamp(0.0, 1.0) as f32;
        for (x, p) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let u = (x as f64 + 0.5) * k + margin - 0.5;
            let x0 = u.floor().clamp(0.0, (sw - 1) as f64) as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let fx = (u - u.floor()).clamp(0.0, 1.0) as f32;
            let mut out = [0f32; 4];
            for (ch, plane) in planes.iter().enumerate() {
                let a = plane[y0 * sw + x0] * (1.0 - fx) + plane[y0 * sw + x1] * fx;
                let b = plane[y1 * sw + x0] * (1.0 - fx) + plane[y1 * sw + x1] * fx;
                out[ch] = a * (1.0 - fy) + b * fy;
            }
            out[3] = 1.0;
            over(p, out);
        }
    }
    fill(c, [0.0, 0.0, 0.0, 0.12]);
}

/// The paper's soft drop shadow under a free cell (CSS `0 8px 24px rgba(0,0,0,.35)`).
fn draw_shadow(c: &mut Canvas, g: &CellGeom, s: f64) {
    let sigma = 12.0 * s;
    let off = 8.0 * s;
    let (cx, cy) = ((g.x + g.w / 2.0) * s, (g.y + g.h / 2.0) * s + off);
    let (hw, hh) = (g.w * s / 2.0, g.h * s / 2.0);
    let (sin, cos) = g.rot.to_radians().sin_cos();
    let ex = cos.abs() * hw + sin.abs() * hh + 3.0 * sigma + 2.0;
    let ey = sin.abs() * hw + cos.abs() * hh + 3.0 * sigma + 2.0;
    let (x0, y0) = ((cx - ex).floor() as i64, (cy - ey).floor() as i64);
    let (bw, bh) = ((2.0 * ex).ceil() as usize + 1, (2.0 * ey).ceil() as usize + 1);
    let mut a = vec![0f32; bw * bh];
    for y in 0..bh {
        for x in 0..bw {
            let (dx, dy) = ((x0 + x as i64) as f64 + 0.5 - cx, (y0 + y as i64) as f64 + 0.5 - cy);
            let (u, v) = (dx * cos + dy * sin, -dx * sin + dy * cos);
            a[y * bw + x] = coverage(sd_round_rect(u, v, hw, hh, g.radius * s));
        }
    }
    overlay::blur(&mut a, bw, bh, sigma);
    let (w, h) = (c.w as i64, c.h as i64);
    for y in 0..bh {
        let py = y0 + y as i64;
        if py < 0 || py >= h {
            continue;
        }
        for x in 0..bw {
            let px = x0 + x as i64;
            if px < 0 || px >= w {
                continue;
            }
            let v = a[y * bw + x] * 0.35;
            if v > 0.0 {
                let i = ((py * w + px) * 4) as usize;
                over(&mut c.px[i..i + 4], [0.0, 0.0, 0.0, v]);
            }
        }
    }
}

/// One cell: paper (free layouts), then the photo clipped to its rounded window or the empty fill.
fn draw_cell(c: &mut Canvas, g: &CellGeom, s: f64, photo: Option<(&Mips, &super::model::Slot)>, preview: bool) {
    if g.free {
        draw_shadow(c, g, s);
    }
    let (cx, cy) = ((g.x + g.w / 2.0) * s, (g.y + g.h / 2.0) * s);
    let (hw, hh) = (g.w * s / 2.0, g.h * s / 2.0);
    let (sin, cos) = g.rot.to_radians().sin_cos();
    let ex = cos.abs() * hw + sin.abs() * hh + 1.0;
    let ey = sin.abs() * hw + cos.abs() * hh + 1.0;
    let xa = (cx - ex).floor().max(0.0) as i64;
    let xb = (cx + ex).ceil().min(c.w as f64) as i64;
    let ya = (cy - ey).floor().max(0.0) as i64;
    let yb = (cy + ey).ceil().min(c.h as f64) as i64;
    // the photo window, centred coordinates in the cell's frame
    let iw = g.inner.w * s;
    let ih = g.inner.h * s;
    let icx = (g.inner.x - g.w / 2.0) * s + iw / 2.0;
    let icy = (g.inner.y - g.h / 2.0) * s + ih / 2.0;
    let placed = photo.map(|(m, slot)| {
        let r = place_image(g.inner.w, g.inner.h, m.width(), m.height(), slot);
        let k = m.width() / (r.w * s);
        let (lvl, ls) = m.level(k);
        (lvl, ls, r, m.width(), m.height())
    });
    let paper = rgba([0xfb, 0xfb, 0xf8], 1.0);
    let empty = if preview { [0.0, 0.0, 0.0, 0.06] } else { rgba([127, 127, 127], 0.18) };
    let w = c.w as i64;
    for y in ya..yb {
        for x in xa..xb {
            let (dx, dy) = (x as f64 + 0.5 - cx, y as f64 + 0.5 - cy);
            let (u, v) = (dx * cos + dy * sin, -dx * sin + dy * cos);
            let i = ((y * w + x) * 4) as usize;
            if g.free {
                let a = coverage(sd_round_rect(u, v, hw, hh, g.radius * s));
                if a > 0.0 {
                    over(&mut c.px[i..i + 4], paper.map(|ch| ch * a));
                }
            }
            let a = coverage(sd_round_rect(u - icx, v - icy, iw / 2.0, ih / 2.0, g.inner_radius * s));
            if a <= 0.0 {
                continue;
            }
            let col = match &placed {
                Some((lvl, ls, r, pw, ph)) => {
                    // window-relative design units → photo px
                    let wx = (u - icx + iw / 2.0) / s;
                    let wy = (v - icy + ih / 2.0) / s;
                    let su = (wx - r.x) / r.w * pw * ls;
                    let sv = (wy - r.y) / r.h * ph * ls;
                    sample(lvl, su, sv)
                }
                None => empty,
            };
            over(&mut c.px[i..i + 4], col.map(|ch| ch * a));
        }
    }
}

/// Draw the collage at `scale` output px per design unit. `photo` gives a cell's photo (the
/// export loads one at a time); `bg` is the photo behind a blurred background. Text layers are
/// drawn when `fonts` is given (the preview shows them as separate images).
pub fn render(
    d: &CollageDoc,
    width: u32,
    height: u32,
    scale: f64,
    bg: Option<&Mips>,
    photo: &mut dyn FnMut(&str) -> Option<std::sync::Arc<Mips>>,
    fonts: Option<&mut Fonts>,
    preview: bool,
    progress: &dyn Fn(f32),
) -> Canvas {
    let mut c = Canvas::new(width, height);
    match &d.bg {
        Background::Solid(col) => fill(&mut c, rgba(*col, 1.0)),
        Background::Gradient(id) => fill_gradient(&mut c, gradient(id)),
        Background::Blur(amount) => {
            fill(&mut c, rgba([0x11, 0x11, 0x11], 1.0));
            if let Some(m) = bg {
                fill_blurred(&mut c, m, amount * scale);
            }
        }
    }
    progress(0.08);
    let cells = layout_cells(d);
    let n = cells.len().max(1);
    for (k, g) in cells.iter().enumerate() {
        let slot = &d.slots[g.index];
        let m = slot.id.as_deref().and_then(&mut *photo);
        draw_cell(&mut c, g, scale, m.as_deref().map(|m| (m, slot)), preview);
        progress(0.08 + (k + 1) as f32 / n as f32 * 0.72);
    }
    if let Some(f) = fonts {
        for l in &d.layers {
            let r = f.render_text(l, scale);
            c.draw_raster(&r, l.x * scale, l.y * scale, l.rotation, scale / r.scale, l.opacity);
        }
    }
    progress(0.85);
    c
}

/// Straight RGBA of a premultiplied canvas (the encoder wants it).
pub fn unpremultiply(mut c: Canvas) -> RgbaImage {
    for p in c.px.as_chunks_mut::<4>().0.iter_mut() {
        if p[3] > 0 && p[3] < 255 {
            let a = p[3] as u32;
            for ch in &mut p[..3] {
                *ch = ((*ch as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    RgbaImage::from_raw(c.w, c.h, c.px).expect("same size")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collage::model::CollageDoc;
    use std::sync::Arc;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, image::Rgba(c))
    }

    fn px(c: &Canvas, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * c.w + x) * 4) as usize;
        [c.px[i], c.px[i + 1], c.px[i + 2], c.px[i + 3]]
    }

    #[test]
    fn mips_halve_down_to_small() {
        let m = Mips::new(solid(1000, 500, [10, 20, 30, 255]));
        assert_eq!(m.0.len(), 4);
        assert_eq!(m.0[3].dimensions(), (125, 62));
        assert_eq!(m.level(1.0).1, 1.0);
        assert_eq!(m.level(4.5).1, 0.25);
        assert_eq!(m.0[2].get_pixel(3, 3).0, [10, 20, 30, 255]);
    }

    #[test]
    fn grid_cells_show_photos_on_the_background() {
        let mut d = CollageDoc::new(&["a".into(), "b".into()]);
        d.aspect = "1:1";
        d.count = 2;
        d.template = "columns";
        d.radius = 200.0;
        let red = Arc::new(Mips::new(solid(300, 200, [255, 0, 0, 255])));
        let blue = Arc::new(Mips::new(solid(300, 200, [0, 0, 255, 255])));
        let mut get = |id: &str| Some(if id == "a" { red.clone() } else { blue.clone() });
        let c = render(&d, 200, 200, 0.1, None, &mut get, None, false, &|_| {});
        // padding 24 → 2.4 px of white, then the first photo; gap between the columns
        assert_eq!(px(&c, 1, 100), [255, 255, 255, 255]);
        assert_eq!(px(&c, 50, 100), [255, 0, 0, 255]);
        assert_eq!(px(&c, 150, 100), [0, 0, 255, 255]);
        assert_eq!(px(&c, 100, 100), [255, 255, 255, 255]);
        // rounded corner: the very corner of the cell stays background
        assert_eq!(px(&c, 4, 4), [255, 255, 255, 255]);
        assert_eq!(px(&c, 20, 20), [255, 0, 0, 255]);
    }

    #[test]
    fn empty_cells_and_gradients() {
        let mut d = CollageDoc::new(&[]);
        d.aspect = "1:1";
        d.bg = Background::Gradient("night");
        let mut none = |_: &str| None;
        let c = render(&d, 100, 100, 0.05, None, &mut none, None, false, &|_| {});
        // night: #1e1b4b at the top, #020617 at the bottom
        let top = px(&c, 0, 0);
        let bottom = px(&c, 0, 99);
        assert!(top[2] > 60 && bottom[2] < 30, "{top:?} {bottom:?}");
        // an empty cell is grey over the gradient
        let mid = px(&c, 25, 25);
        assert!(mid[0] > top[0], "{mid:?}");
    }

    #[test]
    fn free_cells_have_paper_and_shadow() {
        let mut d = CollageDoc::new(&["a".into()]);
        d.aspect = "1:1";
        d.count = 2;
        d.template = "polaroid";
        let img = Arc::new(Mips::new(solid(100, 100, [0, 128, 0, 255])));
        let mut get = |id: &str| (id == "a").then(|| img.clone());
        let c = render(&d, 400, 400, 0.2, None, &mut get, None, true, &|_| {});
        let cells = layout_cells(&d);
        let g = cells[0];
        let (cx, cy) = (((g.x + g.w / 2.0) * 0.2) as u32, ((g.y + g.h / 2.0) * 0.2) as u32);
        let p = px(&c, cx, cy - 10);
        assert!(p[1] > 100 && p[0] < 30, "photo {p:?}");
        // the paper's bottom strip
        let (sin, cos) = g.rot.to_radians().sin_cos();
        let off = (g.h / 2.0 - g.w * 0.1) * 0.2;
        let (bx, by) = (cx as f64 - off * sin, cy as f64 + off * cos);
        let paper = px(&c, bx as u32, by as u32);
        assert!(paper[0] > 240 && paper[1] > 240, "paper {paper:?}");
    }

    #[test]
    fn blurred_background_is_dimmed_photo() {
        let mut d = CollageDoc::new(&[]);
        d.bg = Background::Blur(40.0);
        // wide padding: the corner pixel is background, not an empty cell
        d.padding = 200.0;
        let m = Mips::new(solid(64, 64, [200, 200, 200, 255]));
        let mut none = |_: &str| None;
        let c = render(&d, 80, 100, 0.05, Some(&m), &mut none, None, false, &|_| {});
        let p = px(&c, 1, 1);
        // 200 · 0.88 ≈ 176
        assert!((p[0] as i32 - 176).abs() <= 2, "{p:?}");
    }
}
