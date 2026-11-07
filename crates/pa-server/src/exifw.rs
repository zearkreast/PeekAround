//! Minimal EXIF writer for edited copies (sharp's `withExif({ IFD0, IFD2 })`): capture date,
//! camera and lens carried over from the original, `Software: PeekAround`. JPEG (APP1) and WebP
//! (VP8X + EXIF chunk).

/// Tags to write: IFD0 (Make, Model, Software) and the Exif IFD (dates, lens).
#[derive(Debug, Clone, Default)]
pub struct ExifTags {
    pub make: Option<String>,
    pub model: Option<String>,
    pub software: Option<String>,
    pub date_time_original: Option<String>,
    pub date_time_digitized: Option<String>,
    pub lens_model: Option<String>,
}

fn ifd(
    entries: &[(u16, u16, Vec<u8>)],
    start: u32,
    next_ifd_ptr: Option<u32>,
) -> (Vec<u8>, Vec<u8>) {
    // returns (ifd bytes incl. next pointer, data area); offsets relative to the TIFF header
    let n = entries.len();
    let ifd_len = 2 + n * 12 + 4;
    let mut data = Vec::new();
    let mut out = Vec::with_capacity(ifd_len);
    out.extend_from_slice(&(n as u16).to_le_bytes());
    for (tag, typ, val) in entries {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&typ.to_le_bytes());
        let count = if *typ == 2 { val.len() as u32 } else { 1 };
        out.extend_from_slice(&count.to_le_bytes());
        if val.len() <= 4 {
            let mut v = val.clone();
            v.resize(4, 0);
            out.extend_from_slice(&v);
        } else {
            let off = start + ifd_len as u32 + data.len() as u32;
            out.extend_from_slice(&off.to_le_bytes());
            data.extend_from_slice(val);
            if data.len() % 2 == 1 {
                data.push(0);
            }
        }
    }
    out.extend_from_slice(&next_ifd_ptr.unwrap_or(0).to_le_bytes());
    (out, data)
}

fn ascii(s: &str) -> Vec<u8> {
    let mut v: Vec<u8> = s.bytes().collect();
    v.push(0);
    v
}

/// little-endian TIFF block (without the "Exif\0\0" prefix)
pub fn tiff_block(t: &ExifTags) -> Vec<u8> {
    let mut exif_entries: Vec<(u16, u16, Vec<u8>)> = Vec::new();
    if let Some(v) = &t.date_time_original {
        exif_entries.push((0x9003, 2, ascii(v)));
    }
    if let Some(v) = &t.date_time_digitized {
        exif_entries.push((0x9004, 2, ascii(v)));
    }
    if let Some(v) = &t.lens_model {
        exif_entries.push((0xA434, 2, ascii(v)));
    }
    let mut ifd0_entries: Vec<(u16, u16, Vec<u8>)> = Vec::new();
    if let Some(v) = &t.make {
        ifd0_entries.push((0x010F, 2, ascii(v)));
    }
    if let Some(v) = &t.model {
        ifd0_entries.push((0x0110, 2, ascii(v)));
    }
    if let Some(v) = &t.software {
        ifd0_entries.push((0x0131, 2, ascii(v)));
    }
    let has_exif = !exif_entries.is_empty();
    if has_exif {
        ifd0_entries.push((0x8769, 4, vec![0, 0, 0, 0]));
    }
    // first pass: layout of IFD0 to know where the Exif IFD goes
    let (probe, probe_data) = ifd(&ifd0_entries, 8, None);
    let exif_off = 8 + probe.len() as u32 + probe_data.len() as u32;
    if has_exif {
        if let Some(e) = ifd0_entries.iter_mut().find(|e| e.0 == 0x8769) {
            e.2 = exif_off.to_le_bytes().to_vec();
        }
    }
    let (i0, d0) = ifd(&ifd0_entries, 8, None);
    let mut out = b"II*\0".to_vec();
    out.extend_from_slice(&8u32.to_le_bytes());
    out.extend_from_slice(&i0);
    out.extend_from_slice(&d0);
    if has_exif {
        let (ie, de) = ifd(&exif_entries, exif_off, None);
        out.extend_from_slice(&ie);
        out.extend_from_slice(&de);
    }
    out
}

/// insert an APP1 Exif segment after the JPEG SOI
pub fn jpeg_with_exif(jpeg: &[u8], t: &ExifTags) -> Vec<u8> {
    if jpeg.len() < 2 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return jpeg.to_vec();
    }
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend_from_slice(&tiff_block(t));
    if payload.len() + 2 > 0xFFFF {
        return jpeg.to_vec();
    }
    let mut out = Vec::with_capacity(jpeg.len() + payload.len() + 4);
    out.extend_from_slice(&jpeg[..2]);
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&((payload.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&payload);
    out.extend_from_slice(&jpeg[2..]);
    out
}

/// WebP (simple VP8 / VP8L file) → extended format with an EXIF chunk
pub fn webp_with_exif(webp: &[u8], t: &ExifTags, width: u32, height: u32, alpha: bool) -> Vec<u8> {
    if webp.len() < 20 || &webp[0..4] != b"RIFF" || &webp[8..12] != b"WEBP" {
        return webp.to_vec();
    }
    let chunk = &webp[12..];
    if &chunk[0..4] == b"VP8X" {
        return webp.to_vec();
    }
    let exif = tiff_block(t);
    let mut body = b"WEBP".to_vec();
    body.extend_from_slice(b"VP8X");
    body.extend_from_slice(&10u32.to_le_bytes());
    let flags: u8 = 0x08 | if alpha { 0x10 } else { 0 };
    body.push(flags);
    body.extend_from_slice(&[0, 0, 0]);
    let w = width.saturating_sub(1);
    let h = height.saturating_sub(1);
    body.extend_from_slice(&w.to_le_bytes()[..3]);
    body.extend_from_slice(&h.to_le_bytes()[..3]);
    body.extend_from_slice(chunk);
    body.extend_from_slice(b"EXIF");
    body.extend_from_slice(&(exif.len() as u32).to_le_bytes());
    body.extend_from_slice(&exif);
    if exif.len() % 2 == 1 {
        body.push(0);
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}
