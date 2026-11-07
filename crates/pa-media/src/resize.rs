//! Resizing with sharp's geometry (`fit: 'inside'`, `withoutEnlargement`) and a Lanczos3 kernel
//! (sharp's default), alpha premultiplied during the convolution like libvips.

use crate::error::{MediaError, Result};
use fast_image_resize::{FilterType, ResizeAlg, ResizeOptions, Resizer};
use image::DynamicImage;

/// Output size of `resize(bw, bh, { fit: 'inside', withoutEnlargement })` for a `w`×`h` input —
/// sharp's `ResolveShrink` (canvas MAX) followed by libvips' rounding of `size / shrink`.
pub fn fit_inside(w: u32, h: u32, bw: u32, bh: u32, without_enlargement: bool) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (w, h);
    }
    let mut shrink = (w as f64 / bw.max(1) as f64).max(h as f64 / bh.max(1) as f64);
    if without_enlargement {
        shrink = shrink.max(1.0);
    }
    let hs = shrink.min(w as f64);
    let vs = shrink.min(h as f64);
    let r = |x: f64| ((x + 0.5).floor() as u32).max(1);
    (r(w as f64 / hs), r(h as f64 / vs))
}

/// Resize to exactly `w`×`h` (Lanczos3; RGBA is premultiplied while filtering).
pub fn resize_exact(img: &DynamicImage, w: u32, h: u32) -> Result<DynamicImage> {
    if img.width() == w && img.height() == h {
        return Ok(img.clone());
    }
    let mut dst = match img {
        DynamicImage::ImageRgba8(_) => DynamicImage::ImageRgba8(image::RgbaImage::new(w, h)),
        DynamicImage::ImageRgb8(_) => DynamicImage::ImageRgb8(image::RgbImage::new(w, h)),
        other => return resize_exact(&crate::decode::normalize(other.clone()), w, h),
    };
    let opts = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3));
    Resizer::new()
        .resize(img, &mut dst, &opts)
        .map_err(|e| MediaError::Decode(format!("resize: {e}")))?;
    Ok(dst)
}

/// `resize(bw, bh, { fit: 'inside', withoutEnlargement })`
pub fn resize_inside(
    img: &DynamicImage,
    bw: u32,
    bh: u32,
    without_enlargement: bool,
) -> Result<DynamicImage> {
    let (w, h) = fit_inside(img.width(), img.height(), bw, bh, without_enlargement);
    resize_exact(img, w, h)
}

/// Like [`resize_inside`] but consumes the image (no copy when nothing changes).
pub fn into_inside(
    img: DynamicImage,
    bw: u32,
    bh: u32,
    without_enlargement: bool,
) -> Result<DynamicImage> {
    let (w, h) = fit_inside(img.width(), img.height(), bw, bh, without_enlargement);
    if (w, h) == (img.width(), img.height()) {
        return Ok(img);
    }
    resize_exact(&img, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_matches_sharp() {
        // sizes observed from the TypeScript server on the fixtures
        assert_eq!(fit_inside(1080, 1440, 640, 640, true), (480, 640));
        assert_eq!(fit_inside(1170, 2532, 640, 640, true), (296, 640));
        assert_eq!(fit_inside(1080, 2340, 640, 640, true), (295, 640));
        assert_eq!(fit_inside(1600, 1067, 640, 640, true), (640, 427));
        assert_eq!(fit_inside(1800, 810, 1600, 1600, true), (1600, 720));
        assert_eq!(fit_inside(500, 300, 640, 640, true), (500, 300));
        assert_eq!(fit_inside(500, 300, 640, 640, false), (640, 384));
        assert_eq!(fit_inside(30000, 2, 100, 100, true), (100, 1));
    }

    #[test]
    fn resizes_rgb_and_rgba() {
        let img = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            400,
            200,
            image::Rgba([10, 200, 30, 128]),
        ));
        let out = resize_inside(&img, 100, 100, true).unwrap();
        assert_eq!((out.width(), out.height()), (100, 50));
        let p = out.as_rgba8().unwrap().get_pixel(50, 25);
        assert!(
            (p[1] as i32 - 200).abs() <= 1 && (p[3] as i32 - 128).abs() <= 1,
            "{p:?}"
        );
        let rgb =
            DynamicImage::ImageRgb8(image::RgbImage::from_pixel(10, 10, image::Rgb([1, 2, 3])));
        assert_eq!(resize_exact(&rgb, 5, 7).unwrap().width(), 5);
    }
}
