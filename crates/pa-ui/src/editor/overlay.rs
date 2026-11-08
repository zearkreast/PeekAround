//! Text layers and brush strokes on the CPU (lib/text.ts `drawTextLayer`, strokes.ts): parley lays
//! the text out with the fonts Slint knows (bundled, uploaded, system), swash draws the glyphs,
//! colour emoji included. The preview and the saved file go through the same code. Pixel buffers
//! here are premultiplied RGBA.

use std::borrow::Cow;

#[cfg(test)]
use parley::fontique::CollectionOptions;
use parley::fontique::{Collection, SourceCache};
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontFamilyName, FontStyle, FontWeight, GenericFamily, Layout,
    LayoutContext, LineHeight, PositionedLayoutItem, StyleProperty,
};
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Angle, Cap, Command, Format, Join, Mask, PathBuilder, Stroke as ZStroke, Transform, Vector};
use swash::FontRef;

use super::doc::{self, Align, EditDoc, Rgba, Stroke, TextLayer};

/// Largest side of one text raster; bigger text is drawn smaller rather than allocating gigabytes.
const MAX_SIDE: f32 = 12000.0;

pub struct Fonts {
    fcx: FontContext,
    lcx: LayoutContext<()>,
    scx: ScaleContext,
}

impl Fonts {
    pub fn new(collection: Collection) -> Fonts {
        Fonts {
            fcx: FontContext { collection, source_cache: SourceCache::new_shared() },
            lcx: LayoutContext::new(),
            scx: ScaleContext::new(),
        }
    }

    /// System fonts only (tests, no Slint backend).
    #[cfg(test)]
    pub fn system() -> Fonts {
        Fonts::new(Collection::new(CollectionOptions { shared: false, system_fonts: true }))
    }

    fn layout(&mut self, l: &TextLayer, s: f32) -> Layout<()> {
        let text = if l.text.is_empty() { " " } else { l.text.as_str() };
        let fam = [FontFamilyName::Named(Cow::Borrowed(l.family.as_str())), FontFamilyName::Generic(GenericFamily::Emoji)];
        let mut b = self.lcx.ranged_builder(&mut self.fcx, text, 1.0, false);
        b.push_default(StyleProperty::FontFamily(FontFamily::List(Cow::Borrowed(&fam))));
        b.push_default(StyleProperty::FontSize((l.size as f32 * s).max(0.5)));
        b.push_default(StyleProperty::FontWeight(FontWeight::new(l.weight)));
        b.push_default(StyleProperty::FontStyle(if l.italic { FontStyle::Italic } else { FontStyle::Normal }));
        b.push_default(StyleProperty::LetterSpacing(l.letter_spacing as f32 * s));
        b.push_default(StyleProperty::LineHeight(LineHeight::FontSizeRelative(l.line_height as f32)));
        let mut layout = b.build(text);
        layout.break_all_lines(None);
        let align = match l.align {
            Align::Left => Alignment::Left,
            Align::Center => Alignment::Center,
            Align::Right => Alignment::Right,
        };
        layout.align(align, AlignmentOptions { align_when_overflowing: true });
        layout
    }

    /// The layer's box (text plus background padding) in frame pixels.
    pub fn measure(&mut self, l: &TextLayer) -> (f64, f64) {
        let layout = self.layout(l, 1.0);
        let p = l.background.map_or(0.0, |b| b.padding);
        ((layout.width() as f64 + 2.0 * p).max(8.0), (layout.height() as f64 + 2.0 * p).max(8.0))
    }

    /// Rasterize a layer, unrotated and at full opacity, at `s` raster px per frame px. The box
    /// is centred in the raster; the margin around it holds the outline, shadow and overhangs.
    pub fn render_text(&mut self, l: &TextLayer, s: f64) -> TextRaster {
        let (bw0, bh0) = self.measure(l);
        let mut s = s as f32;
        let margin0 = l.size as f32 * 0.35
            + l.stroke.map_or(0.0, |o| o.width.max(0.0) as f32)
            + l.shadow.map_or(0.0, |sh| sh.blur as f32 * 1.5 + sh.x.abs().max(sh.y.abs()) as f32);
        let side = (bw0.max(bh0) as f32 + 2.0 * margin0) * s;
        if side > MAX_SIDE {
            s *= MAX_SIDE / side;
        }
        let layout = self.layout(l, s);
        let pad = l.background.map_or(0.0, |b| b.padding as f32 * s);
        let bw = (layout.width() + 2.0 * pad).max(8.0 * s);
        let bh = (layout.height() + 2.0 * pad).max(8.0 * s);
        let margin = (margin0 * s).ceil() + 2.0;
        let w = (bw + 2.0 * margin).ceil().max(1.0) as u32;
        let h = (bh + 2.0 * margin).ceil().max(1.0) as u32;
        let ox = (w as f32 - bw) / 2.0;
        let oy = (h as f32 - bh) / 2.0;
        let mut out = Canvas::new(w, h);

        if let Some(bg) = l.background {
            let r = (bg.radius as f32 * s).min(bw / 2.0).min(bh / 2.0).max(0.0);
            let mut path: Vec<Command> = Vec::new();
            if r > 0.0 {
                path.add_round_rect([ox, oy], bw, bh, r, r);
            } else {
                path.add_rect([ox, oy], bw, bh);
            }
            out.fill_path(&path, None, premul(bg.color, 1.0));
        }

        let mut ink = Canvas::new(w, h);
        let outline = l.stroke.filter(|o| o.width > 0.0);
        let (tx, ty) = (ox + pad, oy + pad);
        for line in layout.lines() {
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(gr) = item else { continue };
                let run = gr.run();
                let font = run.font();
                let Some(fref) = FontRef::from_index(font.data.as_ref(), font.index as usize) else { continue };
                let synth = run.synthesis();
                let size = run.font_size();
                let skew = synth.skew().map(|deg| Transform::skew(Angle::from_degrees(deg), Angle::ZERO));
                let bold = if synth.embolden() { size / 40.0 } else { 0.0 };
                let mut scaler =
                    self.scx.builder(fref).size(size).hint(false).normalized_coords(run.normalized_coords()).build();
                for g in gr.positioned_glyphs() {
                    let (gx, gy) = (tx + g.x, ty + g.y);
                    let (fx, fy) = (gx.floor(), gy.floor());
                    let off = Vector::new(gx - fx, gy - fy);
                    let id = g.id as u16;
                    if let Some(o) = outline {
                        let st = ZStroke::new(o.width as f32 * 2.0 * s).join(Join::Round).to_owned();
                        if let Some(img) =
                            Render::new(&[Source::Outline]).style(st).offset(off).transform(skew).embolden(bold).render(&mut scaler, id)
                        {
                            let p = img.placement;
                            ink.blend_mask(&img.data, p.width, p.height, fx as i32 + p.left, fy as i32 - p.top, premul(o.color, 1.0));
                        }
                    }
                    let img = Render::new(&[Source::ColorOutline(0), Source::ColorBitmap(StrikeWith::BestFit), Source::Outline])
                        .format(Format::Alpha)
                        .offset(off)
                        .transform(skew)
                        .embolden(bold)
                        .render(&mut scaler, id);
                    let Some(img) = img else { continue };
                    let p = img.placement;
                    let (x0, y0) = (fx as i32 + p.left, fy as i32 - p.top);
                    match img.content {
                        Content::Mask => ink.blend_mask(&img.data, p.width, p.height, x0, y0, premul(l.color, 1.0)),
                        Content::Color => ink.blend_rgba(&img.data, p.width, p.height, x0, y0),
                        Content::SubpixelMask => {}
                    }
                }
            }
        }

        if let Some(sh) = l.shadow.filter(|sh| sh.color[3] > 0) {
            let mut a: Vec<f32> = ink.px.as_chunks::<4>().0.iter().map(|p| p[3] as f32 / 255.0).collect();
            blur(&mut a, w as usize, h as usize, sh.blur * s as f64 / 2.0);
            let c = premul(sh.color, 1.0);
            let (dx, dy) = ((sh.x * s as f64).round() as i64, (sh.y * s as f64).round() as i64);
            for y in 0..h as i64 {
                let sy = y - dy;
                if sy < 0 || sy >= h as i64 {
                    continue;
                }
                for x in 0..w as i64 {
                    let sx = x - dx;
                    if sx < 0 || sx >= w as i64 {
                        continue;
                    }
                    let v = a[(sy * w as i64 + sx) as usize];
                    if v > 0.0 {
                        let i = ((y * w as i64 + x) * 4) as usize;
                        over(&mut out.px[i..i + 4], [c[0] * v, c[1] * v, c[2] * v, c[3] * v]);
                    }
                }
            }
        }
        out.blend_rgba(&ink.px, w, h, 0, 0);
        TextRaster { w, h, px: out.px, scale: s as f64, box_w: bw as f64, box_h: bh as f64 }
    }
}

/// A rasterized text layer; its box (box_w × box_h raster px) is centred in the raster.
pub struct TextRaster {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
    /// raster px per frame px (smaller than asked for when the layer hit [`MAX_SIDE`])
    pub scale: f64,
    pub box_w: f64,
    pub box_h: f64,
}

/// Premultiplied RGBA canvas.
pub struct Canvas {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl Canvas {
    pub fn new(w: u32, h: u32) -> Canvas {
        Canvas { w, h, px: vec![0; w as usize * h as usize * 4] }
    }

    pub fn from_px(w: u32, h: u32, px: Vec<u8>) -> Canvas {
        Canvas { w, h, px }
    }

    /// Source-over an alpha mask placed at (x0, y0) in `color` (premultiplied, 0..1).
    fn blend_mask(&mut self, mask: &[u8], mw: u32, mh: u32, x0: i32, y0: i32, color: [f32; 4]) {
        let (w, h) = (self.w as i32, self.h as i32);
        for my in 0..mh as i32 {
            let y = y0 + my;
            if y < 0 || y >= h {
                continue;
            }
            for mx in 0..mw as i32 {
                let x = x0 + mx;
                if x < 0 || x >= w {
                    continue;
                }
                let m = mask[(my * mw as i32 + mx) as usize];
                if m == 0 {
                    continue;
                }
                let k = m as f32 / 255.0;
                let i = ((y * w + x) * 4) as usize;
                over(&mut self.px[i..i + 4], [color[0] * k, color[1] * k, color[2] * k, color[3] * k]);
            }
        }
    }

    /// Source-over a premultiplied RGBA image placed at (x0, y0).
    fn blend_rgba(&mut self, src: &[u8], sw: u32, sh: u32, x0: i32, y0: i32) {
        let (w, h) = (self.w as i32, self.h as i32);
        for sy in 0..sh as i32 {
            let y = y0 + sy;
            if y < 0 || y >= h {
                continue;
            }
            for sx in 0..sw as i32 {
                let x = x0 + sx;
                if x < 0 || x >= w {
                    continue;
                }
                let j = ((sy * sw as i32 + sx) * 4) as usize;
                if src[j + 3] == 0 {
                    continue;
                }
                let c = [src[j] as f32 / 255.0, src[j + 1] as f32 / 255.0, src[j + 2] as f32 / 255.0, src[j + 3] as f32 / 255.0];
                let i = ((y * w + x) * 4) as usize;
                over(&mut self.px[i..i + 4], c);
            }
        }
    }

    /// Fill (style None) or stroke a path given in canvas pixels, clipped to the canvas.
    fn fill_path(&mut self, path: &[Command], stroke: Option<ZStroke>, color: [f32; 4]) {
        let mut m = Mask::new(path);
        if let Some(st) = stroke {
            m.style(st);
        }
        let (mask, p) = m.render();
        self.blend_mask(&mask, p.width, p.height, p.left, p.top, color);
    }

    /// Brush strokes; frame point (x, y) lands on ((x − ox)·s, (y − oy)·s).
    pub fn draw_strokes(&mut self, strokes: &[Stroke], s: f64, ox: f64, oy: f64) {
        for st in strokes {
            self.draw_stroke(st, s, ox, oy);
        }
    }

    pub fn draw_stroke(&mut self, st: &Stroke, s: f64, ox: f64, oy: f64) {
        if st.points.is_empty() {
            return;
        }
        let pts: Vec<[f32; 2]> = st.points.iter().map(|p| [((p[0] - ox) * s) as f32, ((p[1] - oy) * s) as f32]).collect();
        let width = (st.size * s).max(0.5) as f32;
        let r = width / 2.0 + 2.0;
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for p in &pts {
            x0 = x0.min(p[0]);
            y0 = y0.min(p[1]);
            x1 = x1.max(p[0]);
            y1 = y1.max(p[1]);
        }
        // only the part on the canvas is rendered (a stroke zoomed in can be huge)
        let bx0 = (x0 - r).floor().max(0.0);
        let by0 = (y0 - r).floor().max(0.0);
        let bx1 = (x1 + r).ceil().min(self.w as f32);
        let by1 = (y1 + r).ceil().min(self.h as f32);
        if bx1 <= bx0 || by1 <= by0 {
            return;
        }
        let mut path: Vec<Command> = Vec::new();
        if pts.len() == 1 {
            path.add_circle(pts[0], width / 2.0);
        } else {
            // midpoint quadratics through the points, as strokes.ts
            path.move_to(pts[0]);
            for i in 1..pts.len() - 1 {
                let mid = [(pts[i][0] + pts[i + 1][0]) / 2.0, (pts[i][1] + pts[i + 1][1]) / 2.0];
                path.quad_to(pts[i], mid);
            }
            path.line_to(pts[pts.len() - 1]);
        }
        let (bw, bh) = ((bx1 - bx0) as u32, (by1 - by0) as u32);
        let mut m = Mask::new(&path[..]);
        if pts.len() > 1 {
            m.style(ZStroke::new(width).cap(Cap::Round).join(Join::Round).to_owned());
        }
        m.offset(Vector::new(-bx0, -by0)).size(bw, bh);
        let (mask, _) = m.render();
        self.blend_mask(&mask, bw, bh, bx0 as i32, by0 as i32, premul(st.color, st.opacity as f32));
    }

    /// Source-over a text raster centred at (cx, cy) canvas px, rotated by `deg` clockwise and
    /// scaled by `k` (canvas px per raster px), with bilinear sampling.
    pub fn draw_raster(&mut self, r: &TextRaster, cx: f64, cy: f64, deg: f64, k: f64, opacity: f64) {
        if opacity <= 0.0 || r.w == 0 || r.h == 0 {
            return;
        }
        let (sin, cos) = deg.to_radians().sin_cos();
        let (hw, hh) = (r.w as f64 * k / 2.0, r.h as f64 * k / 2.0);
        let ex = cos.abs() * hw + sin.abs() * hh;
        let ey = sin.abs() * hw + cos.abs() * hh;
        let xa = (cx - ex).floor().max(0.0) as i64;
        let xb = (cx + ex).ceil().min(self.w as f64) as i64;
        let ya = (cy - ey).floor().max(0.0) as i64;
        let yb = (cy + ey).ceil().min(self.h as f64) as i64;
        let op = opacity.clamp(0.0, 1.0) as f32;
        for y in ya..yb {
            for x in xa..xb {
                let px = x as f64 + 0.5 - cx;
                let py = y as f64 + 0.5 - cy;
                let u = (px * cos + py * sin) / k + r.w as f64 / 2.0 - 0.5;
                let v = (-px * sin + py * cos) / k + r.h as f64 / 2.0 - 0.5;
                let c = sample(r, u, v);
                if c[3] <= 0.0 {
                    continue;
                }
                let i = ((y * self.w as i64 + x) * 4) as usize;
                over(&mut self.px[i..i + 4], [c[0] * op, c[1] * op, c[2] * op, c[3] * op]);
            }
        }
    }
}

/// Strokes, then text layers, onto the full-size render (straight RGBA of the crop).
pub fn compose(img: &mut image::RgbaImage, d: &EditDoc, src_w: f64, src_h: f64, fonts: Option<&mut Fonts>) {
    if d.strokes.is_empty() && d.layers.is_empty() {
        return;
    }
    let (fw, fh) = doc::frame_size(src_w, src_h, d.geo.rot);
    let crop = doc::crop_px(&d.geo, fw, fh);
    let (w, h) = img.dimensions();
    let mut px = std::mem::take(img).into_raw();
    for p in px.as_chunks_mut::<4>().0.iter_mut() {
        if p[3] < 255 {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
    let mut c = Canvas::from_px(w, h, px);
    c.draw_strokes(&d.strokes, 1.0, crop.x, crop.y);
    if let Some(f) = fonts {
        for l in &d.layers {
            let r = f.render_text(l, 1.0);
            c.draw_raster(&r, l.x - crop.x, l.y - crop.y, l.rotation, 1.0 / r.scale, l.opacity);
        }
    }
    for p in c.px.as_chunks_mut::<4>().0.iter_mut() {
        if p[3] > 0 && p[3] < 255 {
            let a = p[3] as u32;
            for ch in &mut p[..3] {
                *ch = ((*ch as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    *img = image::RgbaImage::from_raw(w, h, c.px).expect("same size");
}

fn sample(r: &TextRaster, u: f64, v: f64) -> [f32; 4] {
    let (x0, y0) = (u.floor(), v.floor());
    let (fx, fy) = ((u - x0) as f32, (v - y0) as f32);
    let (x0, y0) = (x0 as i64, y0 as i64);
    let mut acc = [0.0f32; 4];
    for (dy, wy) in [(0, 1.0 - fy), (1, fy)] {
        for (dx, wx) in [(0, 1.0 - fx), (1, fx)] {
            let (x, y) = (x0 + dx, y0 + dy);
            if x < 0 || y < 0 || x >= r.w as i64 || y >= r.h as i64 {
                continue;
            }
            let wgt = wx * wy;
            if wgt == 0.0 {
                continue;
            }
            let i = ((y * r.w as i64 + x) * 4) as usize;
            for (c, a) in acc.iter_mut().enumerate() {
                *a += r.px[i + c] as f32 / 255.0 * wgt;
            }
        }
    }
    acc
}

/// Premultiplied source-over into one u8 pixel.
fn over(d: &mut [u8], s: [f32; 4]) {
    let inv = 1.0 - s[3];
    for c in 0..4 {
        d[c] = ((s[c] + d[c] as f32 / 255.0 * inv) * 255.0).round().clamp(0.0, 255.0) as u8;
    }
}

fn premul(c: Rgba, opacity: f32) -> [f32; 4] {
    let a = c[3] as f32 / 255.0 * opacity;
    [c[0] as f32 / 255.0 * a, c[1] as f32 / 255.0 * a, c[2] as f32 / 255.0 * a, a]
}

/// Gaussian blur approximated by three box blurs (separable, running sums).
fn blur(a: &mut [f32], w: usize, h: usize, sigma: f64) {
    if sigma < 0.5 || w == 0 || h == 0 {
        return;
    }
    // box widths for three passes (Kutskir, "Fastest Gaussian blur")
    let n = 3.0;
    let wi = ((12.0 * sigma * sigma / n) + 1.0).sqrt().floor();
    let wl = if (wi as i64) % 2 == 0 { wi - 1.0 } else { wi };
    let wu = wl + 2.0;
    let m = ((12.0 * sigma * sigma - n * wl * wl - 4.0 * n * wl - 3.0 * n) / (-4.0 * wl - 4.0)).round();
    let mut tmp = vec![0.0f32; a.len()];
    for i in 0..3 {
        let r = ((if (i as f64) < m { wl } else { wu }) as usize - 1) / 2;
        box_pass(a, &mut tmp, w, h, r, true);
        box_pass(&tmp, a, w, h, r, false);
    }
}

fn box_pass(src: &[f32], dst: &mut [f32], w: usize, h: usize, r: usize, horizontal: bool) {
    let (len, lines) = if horizontal { (w, h) } else { (h, w) };
    let idx = |line: usize, i: usize| if horizontal { line * w + i } else { i * w + line };
    let norm = 1.0 / (2 * r + 1) as f32;
    for line in 0..lines {
        let mut acc = 0.0f32;
        for i in 0..=r.min(len - 1) {
            acc += src[idx(line, i)];
        }
        for i in 0..len {
            dst[idx(line, i)] = acc * norm;
            let add = i + r + 1;
            if add < len {
                acc += src[idx(line, add)];
            }
            if i >= r {
                acc -= src[idx(line, i - r)];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::doc::{TextBackground, TextShadow};

    fn layer() -> TextLayer {
        TextLayer {
            id: 1,
            text: "Hello".into(),
            family: "Arial".into(),
            weight: 700.0,
            italic: false,
            size: 40.0,
            color: [255, 255, 255, 255],
            align: Align::Center,
            line_height: 1.15,
            letter_spacing: 0.0,
            x: 100.0,
            y: 50.0,
            rotation: 0.0,
            opacity: 1.0,
            stroke: None,
            shadow: None,
            background: None,
        }
    }

    #[test]
    fn text_box_follows_lines_and_padding() {
        let mut f = Fonts::system();
        let l = layer();
        let (w, h) = f.measure(&l);
        assert!(w > 40.0 && w < 200.0, "{w}");
        assert!((h - 46.0).abs() < 1.0, "{h}");
        let two = TextLayer { text: "Hello\nworld".into(), ..l.clone() };
        assert!((f.measure(&two).1 - 92.0).abs() < 1.0);
        let bg = TextLayer { background: Some(TextBackground { color: [0, 0, 0, 128], padding: 10.0, radius: 6.0 }), ..l.clone() };
        assert!((f.measure(&bg).0 - (w + 20.0)).abs() < 0.01);
    }

    #[test]
    fn text_raster_has_ink_and_shadow() {
        let mut f = Fonts::system();
        let l = TextLayer { shadow: Some(TextShadow { color: [0, 0, 0, 200], blur: 6.0, x: 0.0, y: 4.0 }), ..layer() };
        let r = f.render_text(&l, 2.0);
        assert!(r.box_w > 80.0 && r.w as f64 > r.box_w);
        let white = r.px.as_chunks::<4>().0.iter().filter(|p| p[0] > 200 && p[3] > 200).count();
        let shade = r.px.as_chunks::<4>().0.iter().filter(|p| p[0] < 30 && p[3] > 20).count();
        assert!(white > 200, "{white}");
        assert!(shade > 200, "{shade}");
    }

    #[test]
    fn raster_lands_rotated_on_the_canvas() {
        let mut f = Fonts::system();
        let l = TextLayer { background: Some(TextBackground { color: [255, 0, 0, 255], padding: 4.0, radius: 0.0 }), ..layer() };
        let r = f.render_text(&l, 1.0);
        let mut c = Canvas::new(300, 300);
        c.draw_raster(&r, 150.0, 150.0, 90.0, 1.0, 1.0);
        let at = |x: usize, y: usize| c.px[(y * 300 + x) * 4 + 3];
        // turned a quarter: tall and narrow
        assert!(at(150, 150 - r.box_w as usize / 2 + 3) > 200);
        assert_eq!(at(150 - r.box_w as usize / 2 + 3, 150), 0);
    }

    #[test]
    fn strokes_draw_dots_and_lines() {
        let mut c = Canvas::new(100, 100);
        let dot = Stroke { color: [255, 0, 0, 255], size: 10.0, opacity: 1.0, points: vec![[20.0, 20.0]] };
        let line = Stroke { color: [0, 0, 255, 255], size: 4.0, opacity: 0.5, points: vec![[10.0, 80.0], [50.0, 80.0], [90.0, 80.0]] };
        c.draw_strokes(&[dot, line], 1.0, 0.0, 0.0);
        let px = |x: usize, y: usize| &c.px[(y * 100 + x) * 4..(y * 100 + x) * 4 + 4];
        assert_eq!(px(20, 20)[0], 255);
        assert_eq!(px(30, 30)[3], 0);
        let p = px(50, 80);
        assert!(p[2] > 100 && p[2] < 150 && p[3] > 100 && p[3] < 150, "{p:?}");
        let mut d = Canvas::new(100, 100);
        d.draw_strokes(&[Stroke { color: [0, 255, 0, 255], size: 4.0, opacity: 1.0, points: vec![[60.0, 60.0]] }], 2.0, 50.0, 50.0);
        assert_eq!(d.px[(20 * 100 + 20) * 4 + 1], 255);
    }
}
