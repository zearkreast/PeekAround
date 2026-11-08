//! The one-pass photo shader (renderer/gl.ts): view → frame → (straighten, flips, quarter
//! turns) → source uv, then blur / sharpen taps, white balance, tone, colour, split tone, fade,
//! vignette and grain. The GPU runs it as WGSL ([`SHADER`], editor/gpu.rs); [`Cpu`] is the same
//! maths per pixel for machines without a usable adapter. Rendering the same parameters into a
//! screen-sized target (preview) or full-resolution tiles (export) keeps both identical.

use std::sync::Arc;

use image::RgbaImage;

use super::doc::{self, EditDoc, Rect, RenderAdjust};

#[derive(Debug, Clone, Copy)]
pub struct RenderParams {
    pub adjust: RenderAdjust,
    /// natural source size (the texture may be smaller)
    pub src_w: f64,
    pub src_h: f64,
    pub rot: u8,
    pub flip_h: bool,
    pub flip_v: bool,
    /// degrees
    pub straighten: f64,
    pub frame_w: f64,
    pub frame_h: f64,
    /// output pixel (0, 0) maps to frame point (ox, oy); one output pixel = k frame pixels
    pub ox: f64,
    pub oy: f64,
    pub k: f64,
    /// pixels outside this frame rect are transparent
    pub clip: Rect,
    /// output rect (vignette reference), frame pixels
    pub crop: Rect,
    /// grain cell size in frame pixels
    pub grain_cell: f64,
    /// sharpen kernel step in source pixels
    pub sharpen_step: f64,
}

impl RenderParams {
    /// The uniform block of [`SHADER`] (12 × vec4).
    pub fn uniforms(&self, tex_scale: f64, max_lod: f32) -> [f32; 48] {
        let a = &self.adjust;
        let zoom = doc::straighten_zoom(self.frame_w, self.frame_h, self.straighten);
        let blur_radius = if a.blur > 0.0 { a.blur as f64 * self.src_w.min(self.src_h) * 0.025 } else { 0.0 };
        let s = &a.split;
        [
            self.ox as f32,
            self.oy as f32,
            self.k as f32,
            0.0,
            self.clip.x as f32,
            self.clip.y as f32,
            self.clip.w as f32,
            self.clip.h as f32,
            self.crop.x as f32,
            self.crop.y as f32,
            self.crop.w as f32,
            self.crop.h as f32,
            self.src_w as f32,
            self.src_h as f32,
            self.frame_w as f32,
            self.frame_h as f32,
            self.straighten.to_radians() as f32,
            zoom as f32,
            self.rot as f32,
            tex_scale as f32,
            if self.flip_h { 1.0 } else { 0.0 },
            if self.flip_v { 1.0 } else { 0.0 },
            self.grain_cell as f32,
            self.sharpen_step as f32,
            a.exposure,
            a.brightness,
            a.contrast,
            a.highlights,
            a.shadows,
            a.saturation,
            a.vibrance,
            a.temperature,
            a.tint,
            a.hue,
            a.fade,
            a.vignette,
            a.grain,
            a.sharpen,
            blur_radius as f32,
            max_lod,
            s.shadows[0],
            s.shadows[1],
            s.shadows[2],
            0.0,
            s.highlights[0],
            s.highlights[1],
            s.highlights[2],
            0.0,
        ]
    }
}

pub const SHADER: &str = r#"
struct U {
  view: vec4f,     // ox, oy, k, -
  clip: vec4f,
  crop: vec4f,
  dims: vec4f,     // srcW, srcH, frameW, frameH
  geo: vec4f,      // angle, zoom, rot, texScale
  misc: vec4f,     // flipX, flipY, grainCell, sharpStep
  t0: vec4f,       // exposure, brightness, contrast, highlights
  t1: vec4f,       // shadows, saturation, vibrance, temperature
  t2: vec4f,       // tint, hue, fade, vignette
  t3: vec4f,       // grain, sharpen, blurRadius, maxLod
  split_s: vec4f,
  split_h: vec4f,
}
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var smp: sampler;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
  var p = array<vec2f, 3>(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
  return vec4f(p[i], 0.0, 1.0);
}

fn frame_to_src(f: vec2f) -> vec2f {
  let frame = u.dims.zw;
  let src = u.dims.xy;
  let c = frame * 0.5;
  let d = (f - c) / u.geo.y;
  let cs = cos(-u.geo.x);
  let sn = sin(-u.geo.x);
  var p = c + vec2f(cs * d.x - sn * d.y, sn * d.x + cs * d.y);
  if (u.misc.x > 0.5) { p.x = frame.x - p.x; }
  if (u.misc.y > 0.5) { p.y = frame.y - p.y; }
  let rot = i32(u.geo.z + 0.5);
  if (rot == 1) { return vec2f(p.y, src.y - p.x); }
  if (rot == 2) { return vec2f(src.x - p.x, src.y - p.y); }
  if (rot == 3) { return vec2f(src.x - p.y, p.x); }
  return p;
}

fn luma(c: vec3f) -> f32 { return dot(c, vec3f(0.2126, 0.7152, 0.0722)); }

fn hash(p: vec2f) -> f32 {
  let q = vec2u(vec2i(p) + vec2i(65536));
  var h = (q.x * 1597334677u) ^ (q.y * 3812015801u);
  h = (h ^ (h >> 16u)) * 0x7feb352du;
  h = (h ^ (h >> 15u)) * 0x846ca68bu;
  h = h ^ (h >> 16u);
  return f32(h) / 4294967295.0;
}

fn hue_rotate(c: vec3f, a: f32) -> vec3f {
  let k = vec3f(0.57735026);
  let ca = cos(a);
  return c * ca + cross(k, c) * sin(a) + k * dot(k, c) * (1.0 - ca);
}

@fragment
fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
  let f = u.view.xy + pos.xy * u.view.z;
  if (f.x < u.clip.x || f.y < u.clip.y || f.x > u.clip.x + u.clip.z || f.y > u.clip.y + u.clip.w) {
    return vec4f(0.0);
  }
  let src = u.dims.xy;
  let tex_scale = u.geo.w;
  let max_lod = u.t3.w;
  let uv = frame_to_src(f) / src;
  let lod = clamp(log2(max(1.0, u.view.z * tex_scale / u.geo.y)), 0.0, max_lod);
  let base = textureSampleLevel(tex, smp, uv, lod);
  var c = base.rgb;

  let blur = u.t3.z;
  if (blur > 0.0) {
    let bl = clamp(log2(max(1.0, blur * tex_scale / 3.0)), 0.0, max_lod);
    var acc = textureSampleLevel(tex, smp, uv, bl).rgb;
    for (var i = 0; i < 24; i++) {
      let fi = f32(i) + 0.5;
      let rr = sqrt(fi / 24.0) * blur;
      let th = fi * 2.3999632;
      acc += textureSampleLevel(tex, smp, uv + vec2f(cos(th), sin(th)) * rr / src, bl).rgb;
    }
    c = acc / 25.0;
  }
  let sharpen = u.t3.y;
  if (sharpen > 0.0) {
    let st = vec2f(u.misc.w) / src;
    let n = textureSampleLevel(tex, smp, uv + vec2f(st.x, 0.0), lod).rgb
          + textureSampleLevel(tex, smp, uv - vec2f(st.x, 0.0), lod).rgb
          + textureSampleLevel(tex, smp, uv + vec2f(0.0, st.y), lod).rgb
          + textureSampleLevel(tex, smp, uv - vec2f(0.0, st.y), lod).rgb;
    c += (c - n * 0.25) * sharpen * 2.2;
  }

  let temp = u.t1.w;
  let tint = u.t2.x;
  // white balance
  c *= vec3f(1.0 + temp * 0.2, 1.0 - tint * 0.14 + abs(temp) * 0.02, 1.0 - temp * 0.2);
  // exposure (in stops) & brightness (midtone gamma)
  c = pow(pow(max(c, vec3f(0.0)), vec3f(2.2)) * exp2(u.t0.x * 1.6), vec3f(1.0 / 2.2));
  c = pow(max(c, vec3f(0.0)), vec3f(exp2(-u.t0.y * 0.75)));
  // contrast around mid grey
  let contrast = u.t0.z;
  let ck = select(1.0 + contrast * 0.75, 1.0 + contrast * 1.1, contrast >= 0.0);
  c = (c - 0.5) * ck + 0.5;
  // highlights & shadows
  let l = clamp(luma(c), 0.0, 1.0);
  let sh = 1.0 - smoothstep(0.0, 0.62, l);
  let hi = smoothstep(0.38, 1.0, l);
  c += vec3f(u.t1.x * 0.32 * sh + u.t0.w * 0.3 * hi);
  // saturation & vibrance
  var lm = luma(c);
  let mx = max(c.r, max(c.g, c.b));
  let mn = min(c.r, min(c.g, c.b));
  let vib = u.t1.z * (1.0 - smoothstep(0.0, 0.7, mx - mn)) * 1.2;
  c = mix(vec3f(lm), c, max(0.0, 1.0 + u.t1.y + vib));
  if (u.t2.y != 0.0) { c = hue_rotate(c, u.t2.y); }
  // split toning
  lm = clamp(luma(c), 0.0, 1.0);
  c += u.split_s.xyz * (1.0 - lm) + u.split_h.xyz * lm;
  // fade / matte
  c = mix(c, c * 0.8 + 0.13, u.t2.z);
  // vignette, relative to the output rect
  let q = (f - u.crop.xy) / u.crop.zw - 0.5;
  let v = smoothstep(0.3, 1.12, length(q) * 1.41421);
  let vignette = u.t2.w;
  if (vignette > 0.0) {
    c *= 1.0 - vignette * 0.88 * v;
  } else {
    c = mix(c, vec3f(1.0), -vignette * 0.75 * v);
  }
  // film grain
  let grain = u.t3.x;
  if (grain > 0.0) {
    let n = hash(floor(f / u.misc.z)) - 0.5;
    let lg = clamp(luma(c), 0.0, 1.0);
    c += vec3f(n * grain * 0.22 * (1.0 - abs(lg - 0.5)));
  }
  c = clamp(c, vec3f(0.0), vec3f(1.0));
  return vec4f(c * base.a, base.a);
}

"#;

/// Mipmap generation: each level samples the one above it (linear filter = 2×2 average).
pub const MIP_SHADER: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
  var p = array<vec2f, 3>(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
  return vec4f(p[i], 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
  let size = max(floor(vec2f(textureDimensions(src)) * 0.5), vec2f(1.0));
  return textureSampleLevel(src, smp, pos.xy / size, 0.0);
}
"#;

/// Mip chain length of a w × h texture.
pub fn mip_levels(w: u32, h: u32) -> u32 {
    32 - w.max(h).max(1).leading_zeros()
}

/// A renderer of [`RenderParams`]: the GPU one or [`Cpu`].
pub trait Renderer: Send {
    /// "gpu: <adapter>" / "cpu"
    fn name(&self) -> String;
    /// largest output tile in one pass
    fn max_tile(&self) -> u32;
    fn set_source(&mut self, img: Arc<RgbaImage>);
    fn has_source(&self) -> bool;
    /// free the source (editor closed)
    fn clear_source(&mut self);
    /// premultiplied RGBA8, `w * h * 4` bytes
    fn render(&mut self, p: &RenderParams, w: u32, h: u32) -> Vec<u8>;
}

/// The GPU renderer when an adapter is available (feature `gpu`), else the CPU one.
pub fn create() -> Box<dyn Renderer> {
    #[cfg(feature = "gpu")]
    match super::gpu::Gpu::new() {
        Ok(g) => return Box::new(g),
        Err(e) => log::warn!("photo editor: no GPU renderer ({e}), using the CPU path"),
    }
    Box::new(Cpu::default())
}

/// Render the edit at full resolution (in tiles, so huge photos fit GPU limits); straight RGBA.
pub fn render_full(r: &mut dyn Renderer, doc: &EditDoc, src_w: f64, src_h: f64, progress: &dyn Fn(f32)) -> RgbaImage {
    let (fw, fh) = doc::frame_size(src_w, src_h, doc.geo.rot);
    let crop = doc::crop_px(&doc.geo, fw, fh);
    let (cw, ch) = (crop.w as u32, crop.h as u32);
    let mut out = RgbaImage::new(cw, ch);
    let tile = r.max_tile().clamp(256, 4096);
    let adjust = doc::effective_adjust(&doc.adjust, Some(&doc.filter));
    let tiles = cw.div_ceil(tile) * ch.div_ceil(tile);
    let mut done = 0;
    for ty in (0..ch).step_by(tile as usize) {
        for tx in (0..cw).step_by(tile as usize) {
            let tw = tile.min(cw - tx);
            let th = tile.min(ch - ty);
            let p = RenderParams {
                adjust,
                src_w,
                src_h,
                rot: doc.geo.rot,
                flip_h: doc.geo.flip_h,
                flip_v: doc.geo.flip_v,
                straighten: doc.geo.straighten,
                frame_w: fw,
                frame_h: fh,
                ox: crop.x + tx as f64,
                oy: crop.y + ty as f64,
                k: 1.0,
                clip: crop,
                crop,
                grain_cell: doc::grain_cell_for(crop),
                sharpen_step: 1.0,
            };
            let px = r.render(&p, tw, th);
            for y in 0..th {
                let row = &px[(y * tw * 4) as usize..((y + 1) * tw * 4) as usize];
                for x in 0..tw {
                    let s = &row[(x * 4) as usize..(x * 4 + 4) as usize];
                    out.put_pixel(tx + x, ty + y, image::Rgba(unpremultiply([s[0], s[1], s[2], s[3]])));
                }
            }
            done += 1;
            progress(done as f32 / tiles as f32);
        }
    }
    out
}

fn unpremultiply(p: [u8; 4]) -> [u8; 4] {
    match p[3] {
        255 => p,
        0 => [0, 0, 0, 0],
        a => {
            let f = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
            [f(p[0]), f(p[1]), f(p[2]), a]
        }
    }
}

// ───────────── CPU path ─────────────

struct Level {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

/// The shader evaluated per pixel on the CPU (threads over rows), trilinear mip sampling.
#[derive(Default)]
pub struct Cpu {
    levels: Vec<Level>,
    tex_scale: f64,
}

impl Cpu {
    fn texel(l: &Level, x: i64, y: i64) -> [f32; 4] {
        let x = x.clamp(0, l.w as i64 - 1) as usize;
        let y = y.clamp(0, l.h as i64 - 1) as usize;
        let i = (y * l.w as usize + x) * 4;
        let p = &l.px[i..i + 4];
        [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, p[3] as f32 / 255.0]
    }

    fn bilinear(l: &Level, u: f32, v: f32) -> [f32; 4] {
        let x = u * l.w as f32 - 0.5;
        let y = v * l.h as f32 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let a = Self::texel(l, x0, y0);
        let b = Self::texel(l, x0 + 1, y0);
        let c = Self::texel(l, x0, y0 + 1);
        let d = Self::texel(l, x0 + 1, y0 + 1);
        let mut o = [0.0; 4];
        for i in 0..4 {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bot = c[i] + (d[i] - c[i]) * fx;
            o[i] = top + (bot - top) * fy;
        }
        o
    }

    fn sample(&self, u: f32, v: f32, lod: f32) -> [f32; 4] {
        let lod = lod.clamp(0.0, (self.levels.len() - 1) as f32);
        let l0 = lod.floor() as usize;
        let t = lod - l0 as f32;
        let a = Self::bilinear(&self.levels[l0], u, v);
        if t < 1e-3 || l0 + 1 >= self.levels.len() {
            return a;
        }
        let b = Self::bilinear(&self.levels[l0 + 1], u, v);
        [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t]
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn luma(c: [f32; 3]) -> f32 {
    c[0] * 0.2126 + c[1] * 0.7152 + c[2] * 0.0722
}

fn hash(px: f32, py: f32) -> f32 {
    let qx = (px as i32).wrapping_add(65536) as u32;
    let qy = (py as i32).wrapping_add(65536) as u32;
    let mut h = qx.wrapping_mul(1597334677) ^ qy.wrapping_mul(3812015801);
    h = (h ^ (h >> 16)).wrapping_mul(0x7feb352d);
    h = (h ^ (h >> 15)).wrapping_mul(0x846ca68b);
    h ^= h >> 16;
    h as f32 / 4294967295.0
}

/// One output pixel of the shader; `u` is [`RenderParams::uniforms`], (x, y) a pixel centre.
fn shade(cpu: &Cpu, u: &[f32; 48], x: f32, y: f32) -> [f32; 4] {
    let (ox, oy, k) = (u[0], u[1], u[2]);
    let fx = ox + x * k;
    let fy = oy + y * k;
    let (clx, cly, clw, clh) = (u[4], u[5], u[6], u[7]);
    if fx < clx || fy < cly || fx > clx + clw || fy > cly + clh {
        return [0.0; 4];
    }
    let (src_w, src_h, frame_w, frame_h) = (u[12], u[13], u[14], u[15]);
    let (angle, zoom, rot, tex_scale) = (u[16], u[17], u[18], u[19]);
    let max_lod = u[39];
    // frame → source
    let (cx, cy) = (frame_w * 0.5, frame_h * 0.5);
    let (dx, dy) = ((fx - cx) / zoom, (fy - cy) / zoom);
    let (cs, sn) = ((-angle).cos(), (-angle).sin());
    let mut px = cx + cs * dx - sn * dy;
    let mut py = cy + sn * dx + cs * dy;
    if u[20] > 0.5 {
        px = frame_w - px;
    }
    if u[21] > 0.5 {
        py = frame_h - py;
    }
    let (sx, sy) = match (rot + 0.5) as i32 {
        1 => (py, src_h - px),
        2 => (src_w - px, src_h - py),
        3 => (src_w - py, px),
        _ => (px, py),
    };
    let (uu, vv) = (sx / src_w, sy / src_h);
    let lod = (k * tex_scale / zoom).max(1.0).log2().clamp(0.0, max_lod);
    let base = cpu.sample(uu, vv, lod);
    let mut c = [base[0], base[1], base[2]];

    let blur = u[38];
    if blur > 0.0 {
        let bl = (blur * tex_scale / 3.0).max(1.0).log2().clamp(0.0, max_lod);
        let s = cpu.sample(uu, vv, bl);
        let mut acc = [s[0], s[1], s[2]];
        for i in 0..24 {
            let fi = i as f32 + 0.5;
            let rr = (fi / 24.0).sqrt() * blur;
            let th = fi * 2.399_963_2;
            let t = cpu.sample(uu + th.cos() * rr / src_w, vv + th.sin() * rr / src_h, bl);
            acc[0] += t[0];
            acc[1] += t[1];
            acc[2] += t[2];
        }
        c = acc.map(|v| v / 25.0);
    }
    let sharpen = u[37];
    if sharpen > 0.0 {
        let (stx, sty) = (u[23] / src_w, u[23] / src_h);
        let a = cpu.sample(uu + stx, vv, lod);
        let b = cpu.sample(uu - stx, vv, lod);
        let d = cpu.sample(uu, vv + sty, lod);
        let e = cpu.sample(uu, vv - sty, lod);
        for i in 0..3 {
            let n = a[i] + b[i] + d[i] + e[i];
            c[i] += (c[i] - n * 0.25) * sharpen * 2.2;
        }
    }
    let (exposure, brightness, contrast, highlights) = (u[24], u[25], u[26], u[27]);
    let (shadows, saturation, vibrance, temp) = (u[28], u[29], u[30], u[31]);
    let (tint, hue, fade, vignette) = (u[32], u[33], u[34], u[35]);
    // white balance
    c[0] *= 1.0 + temp * 0.2;
    c[1] *= 1.0 - tint * 0.14 + temp.abs() * 0.02;
    c[2] *= 1.0 - temp * 0.2;
    // exposure & brightness
    let ex = (exposure * 1.6).exp2();
    let bg = (-brightness * 0.75).exp2();
    for v in &mut c {
        *v = (v.max(0.0).powf(2.2) * ex).powf(1.0 / 2.2);
        *v = v.max(0.0).powf(bg);
    }
    // contrast
    let ck = if contrast >= 0.0 { 1.0 + contrast * 1.1 } else { 1.0 + contrast * 0.75 };
    for v in &mut c {
        *v = (*v - 0.5) * ck + 0.5;
    }
    // highlights & shadows
    let l = luma(c).clamp(0.0, 1.0);
    let sh = 1.0 - smoothstep(0.0, 0.62, l);
    let hi = smoothstep(0.38, 1.0, l);
    let add = shadows * 0.32 * sh + highlights * 0.3 * hi;
    for v in &mut c {
        *v += add;
    }
    // saturation & vibrance
    let lm = luma(c);
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    let vib = vibrance * (1.0 - smoothstep(0.0, 0.7, mx - mn)) * 1.2;
    let s = (1.0 + saturation + vib).max(0.0);
    for v in &mut c {
        *v = lm + (*v - lm) * s;
    }
    if hue != 0.0 {
        let kk = 0.577_350_26_f32;
        let (ca, sa) = (hue.cos(), hue.sin());
        let d = kk * (c[0] + c[1] + c[2]);
        let cr = [kk * (c[2] - c[1]), kk * (c[0] - c[2]), kk * (c[1] - c[0])];
        c = [
            c[0] * ca + cr[0] * sa + kk * d * (1.0 - ca),
            c[1] * ca + cr[1] * sa + kk * d * (1.0 - ca),
            c[2] * ca + cr[2] * sa + kk * d * (1.0 - ca),
        ];
    }
    // split toning
    let lm = luma(c).clamp(0.0, 1.0);
    for i in 0..3 {
        c[i] += u[40 + i] * (1.0 - lm) + u[44 + i] * lm;
    }
    // fade
    for v in &mut c {
        *v += (*v * 0.8 + 0.13 - *v) * fade;
    }
    // vignette
    let qx = (fx - u[8]) / u[10] - 0.5;
    let qy = (fy - u[9]) / u[11] - 0.5;
    let vv2 = smoothstep(0.3, 1.12, (qx * qx + qy * qy).sqrt() * std::f32::consts::SQRT_2);
    if vignette > 0.0 {
        let m = 1.0 - vignette * 0.88 * vv2;
        c = c.map(|v| v * m);
    } else {
        let t = -vignette * 0.75 * vv2;
        c = c.map(|v| v + (1.0 - v) * t);
    }
    // grain
    let grain = u[36];
    if grain > 0.0 {
        let n = hash((fx / u[22]).floor(), (fy / u[22]).floor()) - 0.5;
        let lg = luma(c).clamp(0.0, 1.0);
        let g = n * grain * 0.22 * (1.0 - (lg - 0.5).abs());
        c = c.map(|v| v + g);
    }
    let a = base[3];
    [c[0].clamp(0.0, 1.0) * a, c[1].clamp(0.0, 1.0) * a, c[2].clamp(0.0, 1.0) * a, a]
}

impl Renderer for Cpu {
    fn name(&self) -> String {
        "cpu".into()
    }

    fn max_tile(&self) -> u32 {
        4096
    }

    fn set_source(&mut self, img: Arc<RgbaImage>) {
        let (w, h) = img.dimensions();
        let mut levels = vec![Level { w, h, px: img.as_raw().clone() }];
        while levels.last().is_some_and(|l| l.w > 1 || l.h > 1) {
            let p = levels.last().unwrap();
            let (nw, nh) = ((p.w / 2).max(1), (p.h / 2).max(1));
            let mut px = vec![0u8; (nw * nh * 4) as usize];
            for y in 0..nh {
                for x in 0..nw {
                    for ch in 0..4 {
                        let mut s = 0u32;
                        for (ddx, ddy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            let sx = (x * 2 + ddx).min(p.w - 1);
                            let sy = (y * 2 + ddy).min(p.h - 1);
                            s += p.px[((sy * p.w + sx) * 4 + ch) as usize] as u32;
                        }
                        px[((y * nw + x) * 4 + ch) as usize] = ((s + 2) / 4) as u8;
                    }
                }
            }
            levels.push(Level { w: nw, h: nh, px });
        }
        self.levels = levels;
        self.tex_scale = 1.0;
    }

    fn has_source(&self) -> bool {
        !self.levels.is_empty()
    }

    fn clear_source(&mut self) {
        self.levels.clear();
    }

    fn render(&mut self, p: &RenderParams, w: u32, h: u32) -> Vec<u8> {
        let mut out = vec![0u8; (w * h * 4) as usize];
        if self.levels.is_empty() || w == 0 || h == 0 {
            return out;
        }
        let u = p.uniforms(self.tex_scale, (self.levels.len() - 1) as f32);
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16);
        let rows_per = (h as usize).div_ceil(threads).max(1);
        let me = &*self;
        std::thread::scope(|s| {
            for (ci, chunk) in out.chunks_mut(rows_per * w as usize * 4).enumerate() {
                let u = &u;
                s.spawn(move || {
                    let y0 = ci * rows_per;
                    for (ry, row) in chunk.chunks_mut(w as usize * 4).enumerate() {
                        let y = (y0 + ry) as f32 + 0.5;
                        for x in 0..w as usize {
                            let c = shade(me, u, x as f32 + 0.5, y);
                            for i in 0..4 {
                                row[x * 4 + i] = (c[i] * 255.0 + 0.5) as u8;
                            }
                        }
                    }
                });
            }
        });
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::doc::{neutral_render, Geo};

    fn params(w: f64, h: f64, adjust: RenderAdjust) -> RenderParams {
        let full = Rect { x: 0.0, y: 0.0, w, h };
        RenderParams {
            adjust,
            src_w: w,
            src_h: h,
            rot: 0,
            flip_h: false,
            flip_v: false,
            straighten: 0.0,
            frame_w: w,
            frame_h: h,
            ox: 0.0,
            oy: 0.0,
            k: 1.0,
            clip: full,
            crop: full,
            grain_cell: 1.0,
            sharpen_step: 1.0,
        }
    }

    fn gradient(w: u32, h: u32) -> Arc<RgbaImage> {
        Arc::new(RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x * 255 / (w - 1)) as u8, (y * 255 / (h - 1)) as u8, 128, 255])))
    }

    #[test]
    fn neutral_render_is_identity() {
        let img = gradient(64, 48);
        let mut cpu = Cpu::default();
        cpu.set_source(img.clone());
        let out = cpu.render(&params(64.0, 48.0, neutral_render()), 64, 48);
        let max = out.iter().zip(img.as_raw()).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(max <= 1, "max diff {max}");
    }

    #[test]
    fn quarter_turn_and_flip_move_pixels() {
        let img = gradient(40, 20);
        let mut cpu = Cpu::default();
        cpu.set_source(img.clone());
        let mut p = params(40.0, 20.0, neutral_render());
        p.rot = 1;
        (p.frame_w, p.frame_h) = (20.0, 40.0);
        p.clip = Rect { x: 0.0, y: 0.0, w: 20.0, h: 40.0 };
        p.crop = p.clip;
        let out = cpu.render(&p, 20, 40);
        // clockwise: the source's bottom-left corner lands top-left
        assert_eq!(&out[0..3], &img.get_pixel(0, 19).0[0..3]);
        let mut f = params(40.0, 20.0, neutral_render());
        f.flip_h = true;
        let out = cpu.render(&f, 40, 20);
        assert_eq!(&out[0..3], &img.get_pixel(39, 0).0[0..3]);
    }

    #[test]
    fn full_render_crops_and_tiles() {
        let img = gradient(300, 200);
        let mut cpu = Cpu::default();
        cpu.set_source(img.clone());
        let mut d = EditDoc {
            geo: Geo { crop: Rect { x: 0.5, y: 0.5, w: 0.5, h: 0.5 }, ..Geo::default() },
            ..Default::default()
        };
        let out = render_full(&mut cpu, &d, 300.0, 200.0, &|_| {});
        assert_eq!(out.dimensions(), (150, 100));
        assert_eq!(out.get_pixel(0, 0).0, img.get_pixel(150, 100).0);
        d.adjust[0] = 50.0;
        let lit = render_full(&mut cpu, &d, 300.0, 200.0, &|_| {});
        assert!(lit.get_pixel(10, 10).0[0] > out.get_pixel(10, 10).0[0]);
    }
}
