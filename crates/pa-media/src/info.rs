//! Cheap image header reads — the equivalent of `sharp(file).metadata()`: format, stored
//! width/height, page count of animated / multi-page images and the EXIF orientation, without
//! decoding pixels.

use crate::error::{MediaError, Result};
use crate::exif;
use serde::Serialize;
use std::io::Read;
use std::path::Path;

/// Container formats (names as sharp reports them in `metadata().format`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Jpeg,
    Png,
    Webp,
    Gif,
    Tiff,
    /// HEIC / HEIF / AVIF (sharp: "heif")
    Heif,
    Svg,
    Bmp,
    Ico,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Jpeg => "jpeg",
            Format::Png => "png",
            Format::Webp => "webp",
            Format::Gif => "gif",
            Format::Tiff => "tiff",
            Format::Heif => "heif",
            Format::Svg => "svg",
            Format::Bmp => "bmp",
            Format::Ico => "ico",
        }
    }
}

/// Sniff the format from the first bytes (≥ 32 recommended); `ext_hint` only decides for SVG.
pub fn detect_format(head: &[u8], ext_hint: Option<&str>) -> Option<Format> {
    if head.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some(Format::Jpeg);
    }
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(Format::Png);
    }
    if head.len() >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        return Some(Format::Webp);
    }
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return Some(Format::Gif);
    }
    if head.starts_with(b"II*\0") || head.starts_with(b"MM\0*") {
        return Some(Format::Tiff);
    }
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        let brand = &head[8..12];
        const HEIF: [&[u8; 4]; 12] = [
            b"heic", b"heix", b"hevc", b"hevx", b"heim", b"heis", b"mif1", b"msf1", b"avif",
            b"avis", b"mif2", b"miaf",
        ];
        if HEIF.iter().any(|b| *b == brand) {
            return Some(Format::Heif);
        }
    }
    if head.starts_with(b"BM") && head.len() >= 26 {
        return Some(Format::Bmp);
    }
    if head.len() >= 6 && head[0..4] == [0, 0, 1, 0] && head[4..6] != [0, 0] {
        return Some(Format::Ico);
    }
    let text = String::from_utf8_lossy(&head[..head.len().min(1024)]).to_ascii_lowercase();
    let t = text.trim_start_matches('\u{feff}').trim_start();
    if t.starts_with("<svg")
        || ((t.starts_with("<?xml") || t.starts_with("<!--") || t.starts_with("<!doctype svg"))
            && text.contains("<svg"))
    {
        return Some(Format::Svg);
    }
    if matches!(ext_hint, Some("svg")) && t.starts_with('<') {
        return Some(Format::Svg);
    }
    None
}

/// What `sharp.metadata()` reports (stored dimensions, NOT rotated by the orientation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImageInfo {
    pub format: Format,
    pub width: u32,
    pub height: u32,
    /// frames of an animated GIF / WebP, IFDs of a TIFF, top-level images of a HEIF; None for
    /// formats without pages (JPEG, PNG, still WebP, SVG) — like sharp's `pages`
    pub pages: Option<u32>,
    /// EXIF orientation 1–8 when the file has one
    pub orientation: Option<u8>,
}

impl ImageInfo {
    /// width/height as displayed (swapped for orientations 5–8), like `metadata.ts`
    pub fn display_size(&self) -> (u32, u32) {
        if self.orientation.is_some_and(|o| o >= 5) {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }
}

/// bytes read from a file for a header probe (formats that need more are read in full)
const HEAD: usize = 512 * 1024;
const MAX_FULL: u64 = 512 * 1024 * 1024;

/// Header read of a file. GIF / TIFF / HEIF / SVG files are read whole (frames / IFDs / boxes can be
/// anywhere), everything else only its first 512 KB.
pub fn image_info_path(path: impl AsRef<Path>) -> Result<ImageInfo> {
    let path = path.as_ref();
    let mut f = std::fs::File::open(path)?;
    let mut head = Vec::with_capacity(HEAD.min(64 * 1024));
    f.by_ref().take(HEAD as u64).read_to_end(&mut head)?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    let fmt = detect_format(&head, ext.as_deref()).ok_or(MediaError::Unsupported)?;
    if head.len() == HEAD && matches!(fmt, Format::Gif | Format::Tiff | Format::Heif | Format::Svg)
    {
        let len = f.metadata()?.len();
        if len > MAX_FULL {
            return Err(MediaError::TooLarge(len));
        }
        f.read_to_end(&mut head)?;
    }
    image_info_fmt(&head, fmt)
}

/// Header read of an in-memory image (possibly only its first bytes, like the 256 KB head of a
/// remote file).
pub fn image_info(data: &[u8], ext_hint: Option<&str>) -> Result<ImageInfo> {
    let fmt = detect_format(data, ext_hint).ok_or(MediaError::Unsupported)?;
    image_info_fmt(data, fmt)
}

fn image_info_fmt(data: &[u8], format: Format) -> Result<ImageInfo> {
    let mut pages = None;
    let mut orientation = None;
    let (width, height) = match format {
        Format::Svg => svg_size(data)?,
        Format::Heif => {
            let (w, h, n) = heif_info(data)?;
            pages = Some(n);
            (w, h)
        }
        _ => {
            let s = imagesize::blob_size(data).map_err(|e| MediaError::Decode(e.to_string()))?;
            (s.width as u32, s.height as u32)
        }
    };
    match format {
        Format::Jpeg | Format::Png | Format::Webp => orientation = exif::orientation(data),
        Format::Tiff => orientation = Some(exif::tiff_orientation(data).unwrap_or(1)),
        _ => {}
    }
    match format {
        Format::Gif => pages = Some(gif_frames(data).max(1)),
        Format::Webp => pages = webp_frames(data),
        Format::Tiff => pages = Some(tiff_pages(data).max(1)),
        _ => {}
    }
    if width == 0 || height == 0 {
        return Err(MediaError::Decode("image has no size".into()));
    }
    Ok(ImageInfo {
        format,
        width,
        height,
        pages,
        orientation,
    })
}

#[cfg(feature = "svg")]
fn svg_size(data: &[u8]) -> Result<(u32, u32)> {
    let tree = crate::decode::svg_tree(data)?;
    let s = tree.size();
    Ok((
        s.width().round().max(1.0) as u32,
        s.height().round().max(1.0) as u32,
    ))
}

#[cfg(not(feature = "svg"))]
fn svg_size(_: &[u8]) -> Result<(u32, u32)> {
    Err(MediaError::NotBuiltIn("SVG"))
}

#[cfg(feature = "heif")]
fn heif_info(data: &[u8]) -> Result<(u32, u32, u32)> {
    let ctx = libheif_rs::HeifContext::read_from_bytes(data)
        .map_err(|e| MediaError::Decode(e.to_string()))?;
    let h = ctx
        .primary_image_handle()
        .map_err(|e| MediaError::Decode(e.to_string()))?;
    Ok((
        h.width(),
        h.height(),
        ctx.top_level_image_handles().len() as u32,
    ))
}

#[cfg(not(feature = "heif"))]
fn heif_info(data: &[u8]) -> Result<(u32, u32, u32)> {
    let s = imagesize::blob_size(data).map_err(|e| MediaError::Decode(e.to_string()))?;
    Ok((s.width as u32, s.height as u32, 1))
}

/// Number of image descriptors in a GIF (walks the block structure, no LZW decoding).
pub fn gif_frames(d: &[u8]) -> u32 {
    if d.len() < 13 {
        return 0;
    }
    let mut p = 13usize;
    let flags = d[10];
    if flags & 0x80 != 0 {
        p += 3 * (1usize << ((flags & 7) + 1));
    }
    let skip_sub_blocks = |mut p: usize| -> Option<usize> {
        loop {
            let n = *d.get(p)? as usize;
            p += 1;
            if n == 0 {
                return Some(p);
            }
            p += n;
        }
    };
    let mut frames = 0;
    while let Some(&b) = d.get(p) {
        match b {
            0x21 => match skip_sub_blocks(p + 2) {
                Some(n) => p = n,
                None => break,
            },
            0x2c => {
                frames += 1;
                let Some(&lf) = d.get(p + 9) else { break };
                p += 10;
                if lf & 0x80 != 0 {
                    p += 3 * (1usize << ((lf & 7) + 1));
                }
                match skip_sub_blocks(p + 1) {
                    Some(n) => p = n,
                    None => break,
                }
            }
            _ => break, // 0x3b trailer or garbage
        }
    }
    frames
}

/// Frame count of an animated WebP (None for a still image, like sharp).
pub fn webp_frames(d: &[u8]) -> Option<u32> {
    let mut animated = false;
    let mut frames = 0;
    for (kind, body) in exif::riff_chunks(d) {
        match kind {
            b"VP8X" => animated = body.first().is_some_and(|f| f & 0x02 != 0),
            b"ANMF" => frames += 1,
            _ => {}
        }
    }
    (animated && frames > 0).then_some(frames)
}

/// IFD chain length of a TIFF.
pub fn tiff_pages(t: &[u8]) -> u32 {
    if t.len() < 8 {
        return 0;
    }
    let le = &t[0..2] == b"II";
    let u16_at = |o: usize| {
        t.get(o..o + 2).map(|b| {
            if le {
                u16::from_le_bytes([b[0], b[1]])
            } else {
                u16::from_be_bytes([b[0], b[1]])
            }
        })
    };
    let u32_at = |o: usize| {
        t.get(o..o + 4).map(|b| {
            if le {
                u32::from_le_bytes([b[0], b[1], b[2], b[3]])
            } else {
                u32::from_be_bytes([b[0], b[1], b[2], b[3]])
            }
        })
    };
    let mut seen = std::collections::HashSet::new();
    let mut off = u32_at(4).unwrap_or(0) as usize;
    let mut n = 0;
    while off != 0 && seen.insert(off) && n < 100_000 {
        let Some(count) = u16_at(off) else { break };
        n += 1;
        off = u32_at(off + 2 + count as usize * 12).unwrap_or(0) as usize;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_formats() {
        assert_eq!(
            detect_format(&[0xff, 0xd8, 0xff, 0xe0], None),
            Some(Format::Jpeg)
        );
        assert_eq!(
            detect_format(b"\0\0\0\x18ftypheic\0\0\0\0", None),
            Some(Format::Heif)
        );
        assert_eq!(
            detect_format(b"\0\0\0\x1cftypavif\0\0\0\0", None),
            Some(Format::Heif)
        );
        assert_eq!(
            detect_format(b"<?xml version=\"1.0\"?>\n<svg xmlns=\"...\">", None),
            Some(Format::Svg)
        );
        assert_eq!(detect_format(b"hello", None), None);
    }
}
