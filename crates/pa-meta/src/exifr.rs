//! Container handling + output merging of exifr 7 (`exifr.parse(file, options)`), for the two calls the
//! TypeScript server makes: the grouping metadata pass (TIFF + EXIF + GPS + XMP, raw values) and the info
//! panel (`readExif`: TIFF + EXIF + GPS, revived / translated values).
//!
//! Supported like exifr: JPEG (first APP1 Exif + first APP1 XMP before SOS), TIFF (IFD0 + XMP tag),
//! HEIC / AVIF (ISO-BMFF `Exif` item; only when the compatible brands list `heic` / `avif`), PNG
//! (`eXIf` chunk + `iTXt` "XML:com.adobe.xmp"). Anything else (WebP, GIF, BMP, …) has no metadata.

use crate::source::Source;
use crate::tiff::{self, TiffOpts};
use crate::value::{Tags, Val};
use crate::xmp;

enum Kind {
    Jpeg,
    Tiff,
    Heif,
    Png,
}

fn sniff(src: &dyn Source) -> Option<Kind> {
    let h = src.read(0, 64)?;
    if h.len() < 2 {
        return None;
    }
    let first = u16::from_be_bytes([h[0], h[1]]);
    if first == 0xFFD8 {
        return Some(Kind::Jpeg);
    }
    if first == 0x4949 || first == 0x4D4D {
        return Some(Kind::Tiff);
    }
    if first == 0 && h.len() >= 4 {
        // HeifFileParser.canHandle: ftyp length ≤ 50, compatible brands (from byte 16) include heic / avif
        let ftyp_len = u16::from_be_bytes([h[2], h[3]]) as usize;
        if ftyp_len <= 50 {
            let mut brands = vec![];
            let mut off = 16;
            while off < ftyp_len {
                brands.push(h.get(off..(off + 4).min(h.len())).unwrap_or(&[]).to_vec());
                off += 4;
            }
            if brands.iter().any(|b| b == b"heic" || b == b"avif") {
                return Some(Kind::Heif);
            }
        }
        return None;
    }
    if h.len() >= 8 && &h[..8] == b"\x89PNG\r\n\x1a\n" {
        return Some(Kind::Png);
    }
    None
}

fn be16(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]))
}

/// JPEG: (first Exif TIFF payload, first XMP payload)
fn jpeg_segments(src: &dyn Source) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    let len = src.len();
    let (mut tiff, mut xmp) = (None, None);
    let mut off: u64 = 0;
    let end = len.saturating_sub(2);
    while off < end {
        let Some(hdr) = src.read(off, 4) else { break };
        if hdr.first() != Some(&0xFF) || hdr.len() < 2 {
            off += 1;
            continue;
        }
        let m2 = hdr[1];
        if (0xE0..=0xEF).contains(&m2) {
            let Some(length) = be16(&hdr, 2) else { break };
            if m2 == 0xE1 {
                let head = src.read(off + 4, 35).unwrap_or_default();
                let seg_len = length as u64 + 2;
                if head.len() >= 6 && &head[..4] == b"Exif" && head[4] == 0 && head[5] == 0 {
                    if tiff.is_none() {
                        tiff = src.read(off + 10, seg_len.saturating_sub(10) as usize);
                    }
                } else if head.starts_with(b"http://ns.adobe.com/") && xmp.is_none() {
                    let ext = head.starts_with(b"http://ns.adobe.com/xmp/extension/");
                    let hl = if ext { 79 } else { 33 };
                    xmp = src.read(off + hl, seg_len.saturating_sub(hl) as usize);
                }
                if tiff.is_some() && xmp.is_some() {
                    break;
                }
            }
            off += length as u64 + 2;
            continue;
        }
        if matches!(m2, 0xC0 | 0xC2 | 0xC4 | 0xDB | 0xDD | 0xDA | 0xFE) {
            if m2 == 0xDA {
                break; // compressed data follows
            }
            let Some(length) = be16(&hdr, 2) else { break };
            off += length as u64 + 2;
            continue;
        }
        off += 1;
    }
    (tiff, xmp)
}

/// PNG: (first eXIf chunk, last iTXt XMP chunk — a later one replaces the parser, tEXt key/values)
type PngSegments = (Option<Vec<u8>>, Option<Vec<u8>>, Vec<(String, Val)>);

fn png_segments(src: &dyn Source) -> PngSegments {
    let len = src.len();
    let (mut tiff, mut xmp) = (None, None);
    let mut text = vec![];
    let mut has_ihdr = false;
    let mut off: u64 = 8;
    while off < len {
        let Some(h) = src.exact(off, 8) else { break };
        let size = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        let typ = h[4..8].to_ascii_lowercase();
        let start = off + 8;
        // exifr reads `size || 64 KiB` bytes for every metadata chunk
        let take = if size == 0 { 65_536 } else { size } as usize;
        if typ == b"ihdr" {
            has_ihdr = true;
        } else if typ == b"text" {
            if let Some(c) = src.read(start, size as usize) {
                // `getString(start, size).split('\0')` → [key, value]
                let s = crate::tiff::decode_utf8(&c);
                let mut it = s.split('\0');
                let key = it.next().unwrap_or("").to_string();
                text.push((
                    key,
                    it.next()
                        .map(|v| Val::Str(v.to_string()))
                        .unwrap_or(Val::Undef),
                ));
            }
        }
        if typ == b"exif" && tiff.is_none() {
            tiff = src.read(start, take);
        } else if typ == b"itxt" {
            if let Some(c) = src.read(start, take) {
                if c.starts_with(b"XML:com.adobe.xmp") {
                    xmp = Some(c);
                }
            }
        }
        off = start + size + 4;
    }
    // text chunks land in the IHDR parser, which only exists when there is an IHDR chunk
    if !has_ihdr {
        text.clear();
    }
    (tiff, xmp, text)
}

struct BoxHead {
    length: u64,
    kind: [u8; 4],
    start: u64,
}

fn box_head(src: &dyn Source, offset: u64) -> Option<BoxHead> {
    let h = src.exact(offset, 8)?;
    let mut length = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
    let kind = [h[4], h[5], h[6], h[7]];
    let mut start = offset + 8;
    if length == 1 {
        let l = src.exact(offset + 8, 8)?;
        length = u64::from_be_bytes(l.try_into().ok()?);
        start += 8;
    }
    Some(BoxHead {
        length,
        kind,
        start,
    })
}

fn uint(src: &dyn Source, off: u64, n: u8) -> Option<u64> {
    let b = src.exact(off, n as usize)?;
    Some(match n {
        1 => b[0] as u64,
        2 => u16::from_be_bytes([b[0], b[1]]) as u64,
        4 => u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as u64,
        8 => u64::from_be_bytes(b.try_into().ok()?),
        _ => return None,
    })
}

/// HEIF / AVIF: the Exif item (exifr's HeifFileParser.findExif, including its simplifications)
fn heif_exif(src: &dyn Source) -> Option<Vec<u8>> {
    let len = src.len();
    let mut next = uint(src, 0, 4)?;
    let meta = loop {
        let b = box_head(src, next)?;
        if &b.kind == b"meta" {
            break b;
        }
        if b.length == 0 {
            return None;
        }
        next += b.length;
    };
    // full box header; "sub boxes" run from the meta payload to the end of the file
    let mut boxes = vec![];
    let mut off = meta.start + 4;
    while off + 4 < len {
        let Some(b) = box_head(src, off) else { break };
        let l = b.length;
        boxes.push(b);
        if l == 0 {
            break;
        }
        off += l;
    }
    let find = |k: &[u8; 4]| boxes.iter().find(|b| &b.kind == k);
    let iinf = find(b"iinf")?;
    let iloc = find(b"iloc")?;
    // iinf: entry count as u16 (also for version 1), infe v2/v3 item_type 'Exif'
    let mut o = iinf.start + 4;
    let mut count = uint(src, o, 2)?;
    o += 2;
    let mut exif_id = None;
    while count > 0 {
        count -= 1;
        let infe = box_head(src, o)?;
        let version = uint(src, infe.start, 1)?;
        let io = infe.start + 4;
        if version >= 2 {
            let id_size = if version == 3 { 4 } else { 2 };
            if src.exact(io + id_size as u64 + 2, 4)?.as_slice() == b"Exif" {
                exif_id = Some(uint(src, io, id_size)?);
                break;
            }
        }
        o += infe.length;
    }
    let want = exif_id?;
    let version = uint(src, iloc.start, 1)?;
    let mut o = iloc.start + 4;
    let n1 = uint(src, o, 1)? as u8;
    let n2 = uint(src, o + 1, 1)? as u8;
    o += 2;
    let (offset_size, length_size, base_size, index_size) = (n1 >> 4, n1 & 15, n2 >> 4, n2 & 15);
    let item_id_size = if version == 2 { 4 } else { 2 };
    let const_size = if version == 1 || version == 2 { 2 } else { 0 };
    let extent_size = (index_size + offset_size + length_size) as u64;
    let count_size = if version == 2 { 4 } else { 2 };
    let mut items = uint(src, o, count_size)?;
    o += count_size as u64;
    while items > 0 {
        items -= 1;
        let id = uint(src, o, item_id_size)?;
        o += item_id_size as u64 + const_size + 2 + base_size as u64;
        let extents = uint(src, o, 2)?;
        o += 2;
        if id == want {
            let ex_off = uint(src, o + index_size as u64, offset_size)?;
            let ex_len = uint(src, o + index_size as u64 + offset_size as u64, length_size)?;
            let name_size = uint(src, ex_off, 4)?;
            let shift = 4 + name_size;
            return src.read(ex_off + shift, ex_len.saturating_sub(shift) as usize);
        }
        o += extents * extent_size;
    }
    None
}

/// Which exifr call to emulate.
#[derive(Debug, Clone)]
pub enum Mode {
    /// `extractMeta`: tiff/exif/gps/xmp, `reviveValues: false`, `translateValues: false`
    Group,
    /// `readExif`: tiff/exif/gps (no XMP), revived dates (local time of the zone) + translated values
    Panel {
        /// zone of the server process (the TS server's `new Date(y, m, d)` uses it)
        local_tz: String,
    },
}

/// `exifr.parse(file, …)` with `mergeOutput: true` → tag name → value (None = nothing / unsupported).
pub fn parse(src: &dyn Source, mode: &Mode) -> Option<Tags> {
    let kind = sniff(src)?;
    let (want_xmp, opts) = match mode {
        Mode::Group => (
            true,
            TiffOpts {
                xmp: true,
                revive_tz: None,
            },
        ),
        Mode::Panel { local_tz } => (
            false,
            TiffOpts {
                xmp: false,
                revive_tz: Some(local_tz.clone()),
            },
        ),
    };
    let mut png_text = vec![];
    let (tiff_chunk, xmp_chunk, whole_tiff) = match kind {
        Kind::Jpeg => {
            let (t, x) = jpeg_segments(src);
            (t, x, false)
        }
        Kind::Png => {
            let (t, x, text) = png_segments(src);
            png_text = text;
            (t, x, false)
        }
        Kind::Heif => (heif_exif(src), None, false),
        Kind::Tiff => (None, None, true),
    };
    let mut out = Tags::new();
    // `readExif` keeps exifr's default `ihdr` parser: PNG tEXt entries are merged first (EXIF wins)
    if matches!(mode, Mode::Panel { .. }) {
        for (k, v) in png_text {
            out.insert(k, v);
        }
    }
    let mut tiff_out = None;
    let parsed = match &tiff_chunk {
        Some(t) => Some(tiff::parse(t, &opts)),
        None if whole_tiff => Some(tiff::parse(src, &opts)),
        None => None,
    };
    if let Some(p) = parsed {
        match p {
            Ok(o) => tiff_out = Some(o),
            // a TIFF file whose IFD0 fails stops the whole parse (file parser throws)
            Err(()) if whole_tiff => return Some(out),
            Err(()) => {}
        }
    }
    let xmp_bytes = if whole_tiff {
        match tiff_out.as_ref().and_then(|t| t.xmp.clone()) {
            // TiffFileParser injects the IFD0 XMP tag; an input BufferView rejects aborts the whole parse
            Some(Err(())) if want_xmp => return Some(Tags::new()),
            Some(Ok(x)) => Some(x),
            _ => None,
        }
    } else {
        xmp_chunk
    };
    // the XMP parser resolves first, the TIFF parser's output is assigned last (it wins on conflicts)
    if want_xmp {
        if let Some(x) = xmp_bytes {
            xmp::merge_into(&mut out, &tiff::decode_utf8(&x));
        }
    }
    if let Some(t) = tiff_out {
        for (k, v) in t.tags {
            out.insert(k, v);
        }
    }
    Some(out)
}

/// Keys of the info panel (`readExif`).
pub const PANEL_KEYS: [&str; 15] = [
    "Make",
    "Model",
    "LensModel",
    "FNumber",
    "ExposureTime",
    "ISO",
    "FocalLength",
    "FocalLengthIn35mmFormat",
    "Flash",
    "WhiteBalance",
    "DateTimeOriginal",
    "Software",
    "latitude",
    "longitude",
    "GPSAltitude",
];

/// Picked EXIF values for the info panel (`readExif`): numbers rounded to 4 decimals, strings cut to
/// 120 chars, dates as ms epoch (EXIF wall clock read in `local_tz`). None when nothing was found.
pub fn read_exif(
    src: &dyn Source,
    local_tz: &str,
) -> Option<indexmap::IndexMap<String, pa_core::ExifValue>> {
    let ex = parse(
        src,
        &Mode::Panel {
            local_tz: local_tz.into(),
        },
    )?;
    let mut pick = indexmap::IndexMap::new();
    for k in PANEL_KEYS {
        let Some(v) = ex.get(k) else { continue };
        let out = match v {
            Val::Undef => continue,
            // revived dates are numbers here (ms epoch)
            Val::Num(n) if k == "DateTimeOriginal" => pa_core::ExifValue::Num(*n),
            Val::Num(n) => pa_core::ExifValue::Num(pa_core::js::round(n * 10000.0) / 10000.0),
            other => pa_core::ExifValue::Str(
                pa_core::js::slice16_to(&other.js_string(), 120).to_string(),
            ),
        };
        pick.insert(k.to_string(), out);
    }
    if pick.is_empty() {
        None
    } else {
        Some(pick)
    }
}
