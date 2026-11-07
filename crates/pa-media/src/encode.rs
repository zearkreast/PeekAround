//! Encoders with sharp's settings: WebP (libwebp, lossy, method 4, alpha quality 100), JPEG
//! (mozjpeg defaults = sharp's `mozjpeg: true`: progressive, trellis quantisation, optimised scans;
//! 4:2:0 chroma unless asked otherwise) and PNG. No metadata is written (sharp strips EXIF/ICC).

use crate::error::{MediaError, Result};
use image::DynamicImage;

/// Lossy WebP at `quality` (0–100); RGBA keeps its alpha channel.
pub fn webp(img: &DynamicImage, quality: f32) -> Result<Vec<u8>> {
    let (w, h) = (img.width(), img.height());
    let enc = match img {
        DynamicImage::ImageRgb8(i) => webp::Encoder::from_rgb(i.as_raw(), w, h),
        DynamicImage::ImageRgba8(i) => webp::Encoder::from_rgba(i.as_raw(), w, h),
        other => return webp(&crate::decode::normalize(other.clone()), quality),
    };
    let mem = enc
        .encode_simple(false, quality.clamp(0.0, 100.0))
        .map_err(|e| MediaError::Encode(format!("webp: {e:?}")))?;
    Ok(mem.to_vec())
}

#[derive(Debug, Clone, Copy)]
pub struct JpegOptions {
    pub quality: u8,
    /// mozjpeg's optimisations (sharp `mozjpeg: true`); false = baseline libjpeg-turbo defaults
    pub mozjpeg: bool,
    /// 4:4:4 instead of 4:2:0 (sharp `chromaSubsampling: '4:4:4'`)
    pub chroma444: bool,
}

impl JpegOptions {
    pub fn new(quality: u8) -> Self {
        Self {
            quality,
            mozjpeg: true,
            chroma444: false,
        }
    }
}

/// Composite an RGBA image over a solid background (sharp `flatten({ background })`).
pub fn flatten(img: &DynamicImage, bg: [u8; 3]) -> image::RgbImage {
    match img {
        DynamicImage::ImageRgba8(i) => {
            let mut out = image::RgbImage::new(i.width(), i.height());
            for (o, p) in out.pixels_mut().zip(i.pixels()) {
                let a = p[3] as u32;
                for c in 0..3 {
                    o[c] = ((p[c] as u32 * a + bg[c] as u32 * (255 - a) + 127) / 255) as u8;
                }
            }
            out
        }
        DynamicImage::ImageRgb8(i) => i.clone(),
        other => other.to_rgb8(),
    }
}

/// JPEG; images with alpha are flattened on white first.
pub fn jpeg(img: &DynamicImage, opts: JpegOptions) -> Result<Vec<u8>> {
    let owned;
    let rgb = match img {
        DynamicImage::ImageRgb8(i) => i,
        other => {
            owned = flatten(other, [255, 255, 255]);
            &owned
        }
    };
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let run = || -> std::io::Result<Vec<u8>> {
        let mut c = mozjpeg::Compress::new(mozjpeg::ColorSpace::JCS_RGB);
        if !opts.mozjpeg {
            c.set_fastest_defaults();
        }
        c.set_size(w, h);
        c.set_quality(opts.quality.clamp(1, 100) as f32);
        if opts.chroma444 {
            c.set_chroma_sampling_pixel_sizes((1, 1), (1, 1));
        }
        let mut s = c.start_compress(Vec::with_capacity(w * h / 4))?;
        s.write_scanlines(rgb.as_raw())?;
        s.finish()
    };
    match std::panic::catch_unwind(run) {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(MediaError::Encode(e.to_string())),
        Err(_) => Err(MediaError::Encode("libjpeg error".into())),
    }
}

/// PNG (sharp `png({ compressionLevel: 8 })` ≈ `image`'s best compression).
pub fn png(img: &DynamicImage) -> Result<Vec<u8>> {
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    let mut out = Vec::new();
    let enc = PngEncoder::new_with_quality(&mut out, CompressionType::Best, FilterType::Adaptive);
    img.write_with_encoder(enc)
        .map_err(|e| MediaError::Encode(e.to_string()))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_webp_jpeg_png() {
        let img = DynamicImage::ImageRgba8(image::RgbaImage::from_fn(64, 48, |x, y| {
            image::Rgba([x as u8 * 4, y as u8 * 5, 90, if x < 32 { 255 } else { 0 }])
        }));
        let w = webp(&img, 80.0).unwrap();
        assert_eq!(&w[0..4], b"RIFF");
        let back = image::load_from_memory(&w).unwrap();
        assert_eq!((back.width(), back.height()), (64, 48));
        assert!(back.color().has_alpha());
        let j = jpeg(&img, JpegOptions::new(88)).unwrap();
        assert_eq!(&j[0..2], &[0xff, 0xd8]);
        let back = image::load_from_memory(&j).unwrap().to_rgb8();
        // transparent half became white
        assert!(back.get_pixel(60, 10)[0] > 240);
        let p = png(&img).unwrap();
        assert_eq!(&p[1..4], b"PNG");
    }
}
