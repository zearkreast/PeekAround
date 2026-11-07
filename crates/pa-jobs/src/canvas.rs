//! Slide canvases: decode (EXIF-rotated, HEIC-capable via pa-media), flatten onto the background,
//! `fit: 'contain'` (enlarging, Lanczos3) and centre-pad to the exact canvas size → raw RGB24.

use image::DynamicImage;
use pa_media::decode::{decode_path, DecodeOptions};
use std::path::Path;

/// `sharp(src).flatten({background}).resize(w, h, { fit: 'contain', background }).removeAlpha().raw()`
pub fn contain_rgb(abs: &Path, width: u32, height: u32, bg: [u8; 3]) -> Result<Vec<u8>, String> {
    let img =
        decode_path(abs, DecodeOptions::default().fit(width, height)).map_err(|e| e.to_string())?;
    contain_image(&img, width, height, bg)
}

/// [`contain_rgb`] for an already decoded image.
pub fn contain_image(
    img: &DynamicImage,
    width: u32,
    height: u32,
    bg: [u8; 3],
) -> Result<Vec<u8>, String> {
    let flat = DynamicImage::ImageRgb8(pa_media::encode::flatten(img, bg));
    let (w, h) = pa_media::resize::fit_inside(flat.width(), flat.height(), width, height, false);
    let (w, h) = (w.min(width).max(1), h.min(height).max(1));
    let scaled = pa_media::resize::resize_exact(&flat, w, h).map_err(|e| e.to_string())?;
    let scaled = scaled.to_rgb8();
    let left = (width - w) / 2;
    let top = (height - h) / 2;
    let mut out = vec![0u8; width as usize * height as usize * 3];
    for px in out.chunks_exact_mut(3) {
        px.copy_from_slice(&bg);
    }
    let row = width as usize * 3;
    for y in 0..h as usize {
        let src = &scaled.as_raw()[y * w as usize * 3..(y + 1) * w as usize * 3];
        let at = (top as usize + y) * row + left as usize * 3;
        out[at..at + src.len()].copy_from_slice(src);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_to_the_canvas() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            40,
            10,
            image::Rgb([200, 10, 10]),
        ));
        let out = contain_image(&img, 20, 20, [1, 2, 3]).unwrap();
        assert_eq!(out.len(), 20 * 20 * 3);
        // 40×10 → 20×5, centred: rows 7..12
        assert_eq!(&out[0..3], &[1, 2, 3]);
        let mid = (10 * 20 + 10) * 3;
        assert!(out[mid] > 150 && out[mid + 1] < 60);
    }
}
