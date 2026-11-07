//! Decoding every still format the server handles into 8-bit RGB / RGBA with the EXIF orientation
//! applied (what `sharp(file, { failOn: 'none' }).rotate()` gives the TypeScript server).
//!
//! Embedded ICC profiles of JPEG / WebP / TIFF are converted to sRGB (like sharp; not PNG / HEIF,
//! which sharp / heic-decode leave as they are).
//!
//! | format | decoder | notes |
//! |---|---|---|
//! | JPEG / JFIF | mozjpeg (libjpeg-turbo) | DCT-domain shrink-on-load (1/2, 1/4, 1/8) like libvips when a target box is given; CMYK/YCCK → `image` (zune-jpeg) |
//! | PNG, GIF (first frame), WebP (first frame), TIFF (first page), BMP, ICO (largest entry) | `image` | sharp's prebuilt libvips cannot read BMP / ICO at all — here they work |
//! | HEIC / HEIF / AVIF | libheif | `irot`/`imir` applied by libheif (EXIF orientation of HEIF is ignored, like heic-decode) |
//! | SVG | resvg | rendered directly at the target size (sharp renders SVG at the size it needs too) |

use crate::error::{MediaError, Result};
use crate::exif;
use crate::info::{detect_format, Format};
use crate::resize::fit_inside;
use image::metadata::Orientation;
use image::{DynamicImage, ImageReader, RgbImage};
use std::io::Cursor;
use std::path::Path;

static HEIF_THREADS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Decoder threads libheif may use per HEIC/AVIF image (Settings.indexIntensity; 0 = libheif's
/// default, one per core). Every other decoder is single-threaded.
pub fn set_heif_threads(n: u32) {
    HEIF_THREADS.store(n, std::sync::atomic::Ordering::Relaxed);
}

/// sharp's `limitInputPixels: 1e9`
pub const MAX_INPUT_PIXELS: u64 = 1_000_000_000;

#[derive(Debug, Clone, Copy)]
pub struct DecodeOptions<'a> {
    /// lower-case file extension (decides only for SVG, whose content sniffing can be ambiguous)
    pub ext: Option<&'a str>,
    /// the box the caller will resize into (`fit: 'inside'`): JPEGs decode at the smallest DCT
    /// scale that still covers it, SVGs render at the fitted size. None = full size.
    pub fit: Option<(u32, u32)>,
    pub max_pixels: u64,
}

impl Default for DecodeOptions<'_> {
    fn default() -> Self {
        Self {
            ext: None,
            fit: None,
            max_pixels: MAX_INPUT_PIXELS,
        }
    }
}

impl<'a> DecodeOptions<'a> {
    pub fn ext(mut self, ext: &'a str) -> Self {
        self.ext = Some(ext);
        self
    }
    pub fn fit(mut self, w: u32, h: u32) -> Self {
        self.fit = Some((w, h));
        self
    }
}

/// Decode a file (see [`decode_bytes`]).
pub fn decode_path(path: impl AsRef<Path>, opts: DecodeOptions<'_>) -> Result<DynamicImage> {
    let path = path.as_ref();
    let data = std::fs::read(path)?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    let opts = DecodeOptions {
        ext: opts.ext.or(ext.as_deref()),
        ..opts
    };
    decode_bytes(&data, opts)
}

/// Decode an image into `ImageRgb8` (no alpha channel) or `ImageRgba8`, oriented upright.
pub fn decode_bytes(data: &[u8], opts: DecodeOptions<'_>) -> Result<DynamicImage> {
    let fmt = detect_format(data, opts.ext).ok_or(MediaError::Unsupported)?;
    if !matches!(fmt, Format::Svg | Format::Heif) {
        if let Ok(s) = imagesize::blob_size(data) {
            let px = s.width as u64 * s.height as u64;
            if px > opts.max_pixels {
                return Err(MediaError::TooLarge(px));
            }
        }
    }
    let img = match fmt {
        Format::Jpeg => {
            let o = exif::orientation(data).unwrap_or(1);
            let img = decode_jpeg(data, opts.fit, o)
                .or_else(|_| decode_generic(data, image::ImageFormat::Jpeg).map(|x| x.0))?;
            let mut img = normalize(img);
            if let Some(icc) = crate::icc::jpeg_icc(data) {
                crate::icc::to_srgb(&mut img, &icc);
            }
            orient(img, o)
        }
        Format::Heif => decode_heif(data, opts.max_pixels)?,
        Format::Svg => decode_svg(data, opts.fit)?,
        other => {
            let f = match other {
                Format::Png => image::ImageFormat::Png,
                Format::Webp => image::ImageFormat::WebP,
                Format::Gif => image::ImageFormat::Gif,
                Format::Tiff => image::ImageFormat::Tiff,
                Format::Bmp => image::ImageFormat::Bmp,
                Format::Ico => image::ImageFormat::Ico,
                _ => return Err(MediaError::Unsupported),
            };
            let (img, icc) = match decode_generic(data, f) {
                Err(e) if other == Format::Ico => (ico_png_entry(data).ok_or(e)?, None),
                r => r?,
            };
            let mut img = normalize(img);
            // sharp (0.35, prebuilt libvips) converts embedded profiles of JPEG / WebP / TIFF to sRGB
            // but ignores the iCCP chunk of PNGs — mirrored here (a PNG profile is kept as is)
            if let Some(icc) = icc.filter(|_| other != Format::Png) {
                crate::icc::to_srgb(&mut img, &icc);
            }
            match other {
                Format::Png | Format::Webp | Format::Tiff => {
                    orient(img, exif::orientation(data).unwrap_or(1))
                }
                _ => img,
            }
        }
    };
    Ok(normalize(img))
}

/// RGB8 for opaque layouts, RGBA8 for layouts with an alpha channel
pub fn normalize(img: DynamicImage) -> DynamicImage {
    match img {
        DynamicImage::ImageRgb8(_) | DynamicImage::ImageRgba8(_) => img,
        i if i.color().has_alpha() => DynamicImage::ImageRgba8(i.to_rgba8()),
        i => DynamicImage::ImageRgb8(i.to_rgb8()),
    }
}

fn orient(mut img: DynamicImage, o: u8) -> DynamicImage {
    if o > 1 {
        if let Some(o) = Orientation::from_exif(o) {
            img.apply_orientation(o);
        }
    }
    img
}

/// decode with the `image` crate: (pixels, embedded ICC profile)
fn decode_generic(data: &[u8], f: image::ImageFormat) -> Result<(DynamicImage, Option<Vec<u8>>)> {
    use image::ImageDecoder;
    let err = |e: image::ImageError| match e {
        image::ImageError::Unsupported(_) => MediaError::Unsupported,
        e => MediaError::Decode(e.to_string()),
    };
    let mut r = ImageReader::with_format(Cursor::new(data), f);
    r.no_limits();
    let mut dec = r.into_decoder().map_err(err)?;
    let icc = dec.icc_profile().ok().flatten();
    Ok((DynamicImage::from_decoder(dec).map_err(err)?, icc))
}

/// The largest PNG-compressed entry of an ICO (the `image` ICO decoder rejects RGB PNG entries).
fn ico_png_entry(d: &[u8]) -> Option<DynamicImage> {
    let n = u16::from_le_bytes([*d.get(4)?, *d.get(5)?]) as usize;
    let mut best: Option<(u32, &[u8])> = None;
    for i in 0..n {
        let e = d.get(6 + i * 16..6 + i * 16 + 16)?;
        let dim = |b: u8| if b == 0 { 256 } else { b as u32 };
        let area = dim(e[0]) * dim(e[1]);
        let size = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize;
        let off = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize;
        let Some(body) = d.get(off..off.checked_add(size)?) else {
            continue;
        };
        if body.starts_with(b"\x89PNG") && best.is_none_or(|(a, _)| area > a) {
            best = Some((area, body));
        }
    }
    decode_generic(best?.1, image::ImageFormat::Png)
        .ok()
        .map(|x| x.0)
}

/// sharp's JPEG shrink-on-load: the largest of 8/4/2 that the overall shrink allows
fn jpeg_scale(w: usize, h: usize, fit: Option<(u32, u32)>, orientation: u8) -> u8 {
    let Some((bw, bh)) = fit else { return 8 };
    let (ow, oh) = if orientation >= 5 { (h, w) } else { (w, h) };
    let shrink = (ow as f64 / bw.max(1) as f64).max(oh as f64 / bh.max(1) as f64);
    if shrink >= 8.0 {
        1
    } else if shrink >= 4.0 {
        2
    } else if shrink >= 2.0 {
        4
    } else {
        8
    }
}

fn decode_jpeg(data: &[u8], fit: Option<(u32, u32)>, orientation: u8) -> Result<DynamicImage> {
    let run = || -> std::io::Result<Option<DynamicImage>> {
        let mut d = mozjpeg::Decompress::builder().from_mem(data)?;
        use mozjpeg::ColorSpace as Cs;
        if matches!(d.color_space(), Cs::JCS_CMYK | Cs::JCS_YCCK) {
            return Ok(None); // libjpeg cannot convert these to RGB
        }
        let (w, h) = d.size();
        d.scale(jpeg_scale(w, h, fit, orientation));
        let mut s = d.rgb()?;
        let (w, h) = (s.width(), s.height());
        let px: Vec<u8> = s.read_scanlines()?;
        // trailing garbage / truncation: libjpeg warns, pixels are what it could read
        let _ = s.finish();
        Ok(RgbImage::from_raw(w as u32, h as u32, px).map(DynamicImage::ImageRgb8))
    };
    match std::panic::catch_unwind(run) {
        Ok(Ok(Some(img))) => Ok(img),
        Ok(Ok(None)) => Err(MediaError::Unsupported),
        Ok(Err(e)) => Err(MediaError::Decode(e.to_string())),
        Err(p) => Err(MediaError::Decode(
            p.downcast_ref::<String>()
                .cloned()
                .unwrap_or_else(|| "libjpeg error".into()),
        )),
    }
}

#[cfg(feature = "heif")]
fn decode_heif(data: &[u8], max_pixels: u64) -> Result<DynamicImage> {
    use libheif_rs::{ColorSpace, HeifContext, LibHeif, RgbChroma};
    let err = |e: libheif_rs::HeifError| MediaError::Decode(e.to_string());
    let lib = heif_lib();
    let mut ctx = HeifContext::read_from_bytes(data).map_err(err)?;
    let threads = HEIF_THREADS.load(std::sync::atomic::Ordering::Relaxed);
    if threads > 0 {
        ctx.set_max_decoding_threads(threads);
    }
    let handle = ctx.primary_image_handle().map_err(err)?;
    let px = handle.width() as u64 * handle.height() as u64;
    if px > max_pixels {
        return Err(MediaError::TooLarge(px));
    }
    let alpha = handle.has_alpha_channel();
    let chroma = if alpha {
        RgbChroma::Rgba
    } else {
        RgbChroma::Rgb
    };
    let img = lib
        .decode(&handle, ColorSpace::Rgb(chroma), None)
        .map_err(err)?;
    let planes = img.planes();
    let plane = planes
        .interleaved
        .ok_or_else(|| MediaError::Decode("libheif returned no interleaved plane".into()))?;
    let (w, h) = (plane.width as usize, plane.height as usize);
    let ch = if alpha { 4 } else { 3 };
    let mut out = Vec::with_capacity(w * h * ch);
    for row in plane.data.chunks(plane.stride).take(h) {
        out.extend_from_slice(&row[..w * ch]);
    }
    let _: &LibHeif = lib;
    Ok(if alpha {
        DynamicImage::ImageRgba8(
            image::RgbaImage::from_raw(w as u32, h as u32, out)
                .ok_or_else(|| MediaError::Decode("bad plane".into()))?,
        )
    } else {
        DynamicImage::ImageRgb8(
            RgbImage::from_raw(w as u32, h as u32, out)
                .ok_or_else(|| MediaError::Decode("bad plane".into()))?,
        )
    })
}

#[cfg(feature = "heif")]
fn heif_lib() -> &'static libheif_rs::LibHeif {
    static LIB: std::sync::OnceLock<libheif_rs::LibHeif> = std::sync::OnceLock::new();
    LIB.get_or_init(libheif_rs::LibHeif::new)
}

#[cfg(not(feature = "heif"))]
fn decode_heif(_: &[u8], _: u64) -> Result<DynamicImage> {
    Err(MediaError::NotBuiltIn("HEIF"))
}

#[cfg(feature = "svg")]
pub(crate) fn svg_tree(data: &[u8]) -> Result<resvg::usvg::Tree> {
    use resvg::usvg;
    static FONTS: std::sync::OnceLock<std::sync::Arc<usvg::fontdb::Database>> =
        std::sync::OnceLock::new();
    let fonts = FONTS.get_or_init(|| {
        let mut db = usvg::fontdb::Database::new();
        db.load_system_fonts();
        std::sync::Arc::new(db)
    });
    let opt = usvg::Options {
        fontdb: fonts.clone(),
        ..Default::default()
    };
    usvg::Tree::from_data(data, &opt).map_err(|e| MediaError::Decode(e.to_string()))
}

#[cfg(feature = "svg")]
fn decode_svg(data: &[u8], fit: Option<(u32, u32)>) -> Result<DynamicImage> {
    use resvg::tiny_skia;
    let tree = svg_tree(data)?;
    let size = tree.size();
    let (sw, sh) = (
        size.width().round().max(1.0) as u32,
        size.height().round().max(1.0) as u32,
    );
    let (w, h) = match fit {
        Some((bw, bh)) => fit_inside(sw, sh, bw, bh, true),
        None => (sw, sh),
    };
    if w as u64 * h as u64 > MAX_INPUT_PIXELS {
        return Err(MediaError::TooLarge(w as u64 * h as u64));
    }
    let mut pixmap =
        tiny_skia::Pixmap::new(w, h).ok_or_else(|| MediaError::Decode("empty SVG".into()))?;
    let t = tiny_skia::Transform::from_scale(w as f32 / size.width(), h as f32 / size.height());
    resvg::render(&tree, t, &mut pixmap.as_mut());
    let mut px = pixmap.take();
    // tiny-skia is premultiplied
    for p in px.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut p[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Ok(DynamicImage::ImageRgba8(
        image::RgbaImage::from_raw(w, h, px)
            .ok_or_else(|| MediaError::Decode("bad SVG buffer".into()))?,
    ))
}

#[cfg(not(feature = "svg"))]
fn decode_svg(_: &[u8], _: Option<(u32, u32)>) -> Result<DynamicImage> {
    Err(MediaError::NotBuiltIn("SVG"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jpeg_scale_mirrors_sharp() {
        assert_eq!(jpeg_scale(4032, 3024, Some((640, 640)), 1), 2); // shrink 6.3 → 1/4
        assert_eq!(jpeg_scale(6000, 4500, Some((640, 640)), 6), 1); // 9.4 → 1/8
        assert_eq!(jpeg_scale(1000, 800, Some((640, 640)), 1), 8);
        assert_eq!(jpeg_scale(1280, 800, Some((640, 640)), 1), 4);
        assert_eq!(jpeg_scale(4032, 3024, None, 1), 8);
    }
}
