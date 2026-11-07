//! The renditions of `media/thumbs.ts` (sizes, qualities, formats), as pure functions over decoded
//! images plus the cache-file helpers. The server's ThumbService adds the cache lookups, in-flight
//! de-duplication and the remote (phone / MEGA) sources on top.
//!
//! | rendition | size | format |
//! |---|---|---|
//! | grid thumbnails | 320 / 640, `fit: inside`, no enlargement | WebP q 78 / 80 (± `thumbQuality`) |
//! | large thumbnail | 1600 | WebP q 85 (±) |
//! | thumbhash | 640 rendition → 100×100 inside | base64 |
//! | display (bounded, viewer zoom ≤ 4096) | `max` | WebP q 88 for formats that may carry alpha, else JPEG q 88 (mozjpeg, white background) |
//! | display (full size, HEIC/TIFF/…) | original | JPEG q 90 (mozjpeg, white background) |

use crate::decode::{decode_bytes, decode_path, DecodeOptions};
use crate::encode::{self, JpegOptions};
use crate::error::{MediaError, Result};
use crate::resize::{into_inside, resize_inside};
use crate::thumbhash::rgba_to_thumb_hash;
use image::DynamicImage;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::path::{Path, PathBuf};

pub const THUMB_SIZES: [u32; 3] = [320, 640, 1600];
/// the viewer's zoom rendition never exceeds this (phones never get 50 MP originals)
pub const MAX_DISPLAY: u32 = 4096;

/// the smallest standard size ≥ s (1600 for anything larger)
pub fn snap_size(s: f64) -> u32 {
    THUMB_SIZES
        .iter()
        .copied()
        .find(|&t| s <= t as f64)
        .unwrap_or(1600)
}

/// Settings.thumbQuality
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThumbQuality {
    Low,
    #[default]
    Normal,
    High,
}

impl ThumbQuality {
    /// WebP quality of a rendition whose 'normal' quality is `base`
    pub fn q(self, base: u8) -> u8 {
        match self {
            ThumbQuality::Low => base.saturating_sub(14),
            ThumbQuality::Normal => base,
            ThumbQuality::High => (base + 8).min(95),
        }
    }
}

/// extensions whose display rendition is WebP (may carry transparency)
pub fn display_keeps_alpha(ext: &str) -> bool {
    ["png", "webp", "gif", "avif", "svg", "ico", "tif", "tiff"].contains(&ext)
}

/// Output of the base decode: 640 + 320 WebP (+ 1600 for video frames) and the thumbhash.
#[derive(Debug, Clone)]
pub struct BaseRenditions {
    pub webp640: Vec<u8>,
    pub webp320: Vec<u8>,
    /// videos only: the 1600 rendition comes from the same frame
    pub webp1600: Option<Vec<u8>>,
    /// base64 ThumbHash (None if it could not be computed)
    pub thumbhash: Option<String>,
    /// size of the 640 rendition
    pub width: u32,
    pub height: u32,
}

/// base64 thumbhash of an image: `resize(100, 100, { fit: 'inside' })` (enlarging small ones, like
/// the TS code), RGBA.
pub fn thumbhash_of(img: &DynamicImage) -> Option<String> {
    use base64::Engine;
    let small = resize_inside(img, 100, 100, false).ok()?;
    let rgba = small.to_rgba8();
    let hash =
        rgba_to_thumb_hash(rgba.width() as usize, rgba.height() as usize, rgba.as_raw()).ok()?;
    Some(base64::engine::general_purpose::STANDARD.encode(hash))
}

/// 640 + 320 + thumbhash from a decoded still (the 320 and the hash are made from the 640 pixels,
/// as the TS code made them from the 640 WebP).
pub fn render_base(src: &DynamicImage, q: ThumbQuality) -> Result<BaseRenditions> {
    let r640 = resize_inside(src, 640, 640, true)?;
    let webp640 = encode::webp(&r640, q.q(80) as f32)?;
    let r320 = resize_inside(&r640, 320, 320, true)?;
    let webp320 = encode::webp(&r320, q.q(78) as f32)?;
    let thumbhash = thumbhash_of(&r640);
    Ok(BaseRenditions {
        webp640,
        webp320,
        webp1600: None,
        thumbhash,
        width: r640.width(),
        height: r640.height(),
    })
}

/// Base renditions of a video frame (PNG from ffmpeg): 1600 too, all from the one frame.
pub fn render_base_frame(frame_png: &[u8], q: ThumbQuality) -> Result<BaseRenditions> {
    let frame = decode_bytes(frame_png, DecodeOptions::default())?;
    let r1600 = resize_inside(&frame, 1600, 1600, true)?;
    let webp1600 = encode::webp(&r1600, q.q(85) as f32)?;
    let mut base = render_base(&frame, q)?;
    base.webp1600 = Some(webp1600);
    Ok(base)
}

/// Decode a still file for the base renditions (JPEGs decode at the DCT scale covering 640 px).
pub fn decode_for_base(path: &Path) -> Result<DynamicImage> {
    decode_path(path, DecodeOptions::default().fit(640, 640))
}

/// The 1600 WebP thumbnail of a still (or of already decoded pixels).
pub fn render_1600(src: DynamicImage, q: ThumbQuality) -> Result<Vec<u8>> {
    let r = into_inside(src, 1600, 1600, true)?;
    encode::webp(&r, q.q(85) as f32)
}

pub fn render_1600_file(path: &Path, q: ThumbQuality) -> Result<Vec<u8>> {
    render_1600(
        decode_path(path, DecodeOptions::default().fit(1600, 1600))?,
        q,
    )
}

/// Format of a display rendition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayFormat {
    Jpeg,
    Webp,
}

impl DisplayFormat {
    pub fn ext(self) -> &'static str {
        match self {
            DisplayFormat::Jpeg => "jpg",
            DisplayFormat::Webp => "webp",
        }
    }
}

/// cache variant name of a display rendition (`display-<max>.<ext>` / `full.jpg`)
pub fn display_variant(ext: &str, max: Option<u32>) -> String {
    match max {
        Some(m) => format!(
            "display-{m}.{}",
            if display_keeps_alpha(ext) {
                "webp"
            } else {
                "jpg"
            }
        ),
        None => "full.jpg".into(),
    }
}

/// Browser friendly rendition: bounded to `max` (long edge) or full size.
pub fn render_display(
    src: DynamicImage,
    ext: &str,
    max: Option<u32>,
) -> Result<(Vec<u8>, DisplayFormat)> {
    match max {
        Some(m) => {
            let sized = into_inside(src, m, m, true)?;
            if display_keeps_alpha(ext) {
                Ok((encode::webp(&sized, 88.0)?, DisplayFormat::Webp))
            } else {
                Ok((
                    encode::jpeg(&sized, JpegOptions::new(88))?,
                    DisplayFormat::Jpeg,
                ))
            }
        }
        None => Ok((
            encode::jpeg(&src, JpegOptions::new(90))?,
            DisplayFormat::Jpeg,
        )),
    }
}

pub fn render_display_file(
    path: &Path,
    ext: &str,
    max: Option<u32>,
) -> Result<(Vec<u8>, DisplayFormat)> {
    let opts = DecodeOptions {
        ext: Some(ext),
        ..Default::default()
    };
    let opts = match max {
        Some(m) => opts.fit(m, m),
        None => opts,
    };
    render_display(decode_path(path, opts)?, ext, max)
}

/// JPEG ≤ 1600 (q 88) of a downloaded original — the remote-folder thumbnail fallback.
pub fn render_jpeg_1600(src: DynamicImage) -> Result<Vec<u8>> {
    encode::jpeg(&into_inside(src, 1600, 1600, true)?, JpegOptions::new(88))
}

/// A JS number as `String(n)` would print it (integers without a fraction).
fn js_num(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e21 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Cache key of a source: sha1(`abs|mtime|size`) in hex (same as the TS ThumbService, so caches
/// keyed by it stay valid).
pub fn cache_key(abs: &str, mtime: f64, size: f64) -> String {
    let mut h = Sha1::new();
    h.update(format!("{abs}|{}|{}", js_num(mtime), js_num(size)).as_bytes());
    hex::encode(h.finalize())
}

/// `<dir>/<key[0..2]>/<key>-<variant>`
pub fn cache_file(dir: &Path, key: &str, variant: &str) -> PathBuf {
    dir.join(&key[..2.min(key.len())])
        .join(format!("{key}-{variant}"))
}

/// Write a file so that it appears only complete (temp file + rename); creates the parent dir.
pub async fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(d) = path.parent() {
        tokio::fs::create_dir_all(d).await?;
    }
    let tmp = PathBuf::from(format!(
        "{}.{}.{:08x}.tmp",
        path.display(),
        std::process::id(),
        rand::random::<u32>()
    ));
    if let Err(e) = tokio::fs::write(&tmp, data).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e.into());
    }
    if let Err(e) = tokio::fs::rename(&tmp, path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e.into());
    }
    Ok(())
}

/// Blocking variant of [`write_atomic`].
pub fn write_atomic_sync(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = PathBuf::from(format!(
        "{}.{}.{:08x}.tmp",
        path.display(),
        std::process::id(),
        rand::random::<u32>()
    ));
    let r = std::fs::write(&tmp, data).and_then(|_| std::fs::rename(&tmp, path));
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r.map_err(MediaError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_quality() {
        assert_eq!(snap_size(100.0), 320);
        assert_eq!(snap_size(321.0), 640);
        assert_eq!(snap_size(5000.0), 1600);
        assert_eq!(ThumbQuality::Low.q(80), 66);
        assert_eq!(ThumbQuality::High.q(88), 95);
        assert_eq!(ThumbQuality::Normal.q(78), 78);
        assert_eq!(display_variant("png", Some(4096)), "display-4096.webp");
        assert_eq!(display_variant("heic", Some(2048)), "display-2048.jpg");
        assert_eq!(display_variant("heic", None), "full.jpg");
    }

    #[test]
    fn cache_key_matches_node() {
        // node -e "crypto.createHash('sha1').update('/a/b.jpg|1700000000000|1234').digest('hex')"
        assert_eq!(
            cache_key("/a/b.jpg", 1_700_000_000_000.0, 1234.0),
            "59a7b9d9556e04b1729c1bd7cb6c55d050fb8596"
        );
        assert!(cache_file(Path::new("/c"), "abcdef", "640.webp").ends_with("ab/abcdef-640.webp"));
    }

    #[test]
    fn base_renditions() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(1200, 900, |x, y| {
            image::Rgb([(x / 5) as u8, (y / 4) as u8, 100])
        }));
        let b = render_base(&img, ThumbQuality::Normal).unwrap();
        assert_eq!((b.width, b.height), (640, 480));
        let t = image::load_from_memory(&b.webp320).unwrap();
        assert_eq!((t.width(), t.height()), (320, 240));
        assert!(b.thumbhash.is_some());
    }
}
