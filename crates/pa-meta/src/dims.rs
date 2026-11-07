//! Header-only image size probe with the results of the TypeScript server's `sharp(file).metadata()`
//! (libvips 8.18): width / height (EXIF orientation 5–8 swaps them), frame count (`pages`) for GIF,
//! animated WebP, TIFF and AVIF. Formats sharp cannot open (BMP, ICO, HEIC by extension) give nothing.

use crate::source::Source;

/// Result of [`probe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Dims {
    /// px (after orientation)
    pub width: u32,
    /// px (after orientation; one frame of animated images)
    pub height: u32,
    /// frames / pages when the format reports them
    pub pages: Option<u32>,
}

fn be16(b: &[u8], i: usize) -> Option<u32> {
    Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32)
}

fn be32(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

fn le16(b: &[u8], i: usize) -> Option<u32> {
    Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32)
}

fn le24(b: &[u8], i: usize) -> Option<u32> {
    Some(*b.get(i)? as u32 | (*b.get(i + 1)? as u32) << 8 | (*b.get(i + 2)? as u32) << 16)
}

fn le32(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

/// orientation tag (0x0112) of IFD0 of a TIFF block
fn tiff_orientation(t: &dyn Source) -> Option<u32> {
    let h = t.exact(0, 8)?;
    let le = &h[..2] == b"II";
    let r16 = |b: &[u8], i: usize| if le { le16(b, i) } else { be16(b, i) };
    let r32 = |b: &[u8], i: usize| if le { le32(b, i) } else { be32(b, i) };
    let ifd = r32(&h, 4)? as u64;
    let n = r16(&t.exact(ifd, 2)?, 0)?;
    let entries = t.exact(ifd + 2, n as usize * 12)?;
    for k in 0..n as usize {
        let e = &entries[k * 12..k * 12 + 12];
        if r16(e, 0)? == 0x0112 {
            let typ = r16(e, 2)?;
            return match typ {
                3 => r16(e, 8),
                4 => r32(e, 8),
                _ => None,
            };
        }
    }
    None
}

fn oriented(w: u32, h: u32, orientation: Option<u32>, pages: Option<u32>) -> Option<Dims> {
    let o = orientation.map(|o| o.clamp(1, 8)).unwrap_or(1);
    let (width, height) = if o >= 5 { (h, w) } else { (w, h) };
    (width > 0 && height > 0).then_some(Dims {
        width,
        height,
        pages,
    })
}

fn jpeg(src: &dyn Source) -> Option<Dims> {
    let len = src.len();
    let mut off = 2u64;
    let mut orientation = None;
    let mut seen_exif = false;
    while off + 4 <= len {
        let h = src.exact(off, 4)?;
        if h[0] != 0xFF {
            return None;
        }
        let m = h[1];
        if m == 0xFF {
            off += 1;
            continue;
        }
        if matches!(m, 0xD8 | 0x01 | 0xD0..=0xD7) {
            off += 2;
            continue;
        }
        let seg = be16(&h, 2)? as u64;
        if m == 0xE1 && !seen_exif {
            if let Some(p) = src.exact(off + 4, 6) {
                if &p == b"Exif\0\0" {
                    seen_exif = true;
                    let t = src.read(off + 10, seg.saturating_sub(8) as usize)?;
                    orientation = tiff_orientation(&t);
                }
            }
        }
        // SOFn (not DHT / JPG / DAC)
        if matches!(m, 0xC0..=0xCF) && !matches!(m, 0xC4 | 0xC8 | 0xCC) {
            let f = src.exact(off + 4, 5)?;
            let (hh, ww) = (be16(&f, 1)?, be16(&f, 3)?);
            return oriented(ww, hh, orientation, None);
        }
        if m == 0xDA {
            return None;
        }
        off += 2 + seg;
    }
    None
}

fn png(src: &dyn Source) -> Option<Dims> {
    let ihdr = src.exact(8, 24)?;
    if &ihdr[4..8] != b"IHDR" {
        return None;
    }
    let (w, h) = (be32(&ihdr, 8)?, be32(&ihdr, 12)?);
    // eXIf orientation (libvips reads it from anywhere before the image data ends)
    let len = src.len();
    let mut off = 8u64;
    let mut orientation = None;
    while off + 8 <= len {
        let c = src.exact(off, 8)?;
        let size = be32(&c, 0)? as u64;
        if &c[4..8] == b"eXIf" {
            let t = src.read(off + 8, size as usize)?;
            orientation = tiff_orientation(&t);
            break;
        }
        if &c[4..8] == b"IEND" {
            break;
        }
        off += 12 + size;
    }
    oriented(w, h, orientation, None)
}

fn gif(src: &dyn Source) -> Option<Dims> {
    let h = src.exact(0, 13)?;
    let (w, ht) = (le16(&h, 6)?, le16(&h, 8)?);
    // count image descriptors
    let len = src.len();
    let mut off = 13u64;
    if h[10] & 0x80 != 0 {
        off += 3 * (1u64 << ((h[10] & 7) + 1));
    }
    let mut frames = 0u32;
    let skip_sub = |mut o: u64| -> Option<u64> {
        loop {
            let n = src.exact(o, 1)?[0] as u64;
            o += 1;
            if n == 0 {
                return Some(o);
            }
            o += n;
        }
    };
    while off < len {
        let Some(b) = src.exact(off, 1) else { break };
        match b[0] {
            0x2C => {
                frames += 1;
                let Some(d) = src.exact(off, 10) else { break };
                off += 10;
                if d[9] & 0x80 != 0 {
                    off += 3 * (1u64 << ((d[9] & 7) + 1));
                }
                off += 1; // LZW min code size
                match skip_sub(off) {
                    Some(o) => off = o,
                    None => break,
                }
            }
            0x21 => match skip_sub(off + 2) {
                Some(o) => off = o,
                None => break,
            },
            _ => break,
        }
    }
    oriented(w, ht, None, Some(frames.max(1)))
}

fn webp(src: &dyn Source) -> Option<Dims> {
    let h = src.exact(0, 30)?;
    if &h[..4] != b"RIFF" || &h[8..12] != b"WEBP" {
        return None;
    }
    let len = src.len();
    let (mut w, mut ht) = (0, 0);
    let mut frames = 0u32;
    let mut animated = false;
    let mut orientation = None;
    let mut off = 12u64;
    while off + 8 <= len {
        let c = src.exact(off, 8)?;
        let size = le32(&c, 4)? as u64;
        let body = off + 8;
        match &c[..4] {
            b"VP8X" => {
                let d = src.exact(body, 10)?;
                animated = d[0] & 0x02 != 0;
                w = le24(&d, 4)? + 1;
                ht = le24(&d, 7)? + 1;
            }
            b"VP8 " if w == 0 => {
                let d = src.exact(body, 10)?;
                w = le16(&d, 6)? & 0x3fff;
                ht = le16(&d, 8)? & 0x3fff;
            }
            b"VP8L" if w == 0 => {
                let d = src.exact(body, 5)?;
                let bits = le32(&d, 1)?;
                w = (bits & 0x3fff) + 1;
                ht = ((bits >> 14) & 0x3fff) + 1;
            }
            b"ANMF" => frames += 1,
            b"EXIF" => {
                let mut t = src.read(body, size as usize)?;
                if t.starts_with(b"Exif\0\0") {
                    t.drain(..6);
                }
                orientation = tiff_orientation(&t);
            }
            _ => {}
        }
        off = body + size + (size & 1);
    }
    let pages = if animated { Some(frames.max(1)) } else { None };
    oriented(w, ht, orientation, pages)
}

fn tiff(src: &dyn Source) -> Option<Dims> {
    let h = src.exact(0, 8)?;
    let le = &h[..2] == b"II";
    let r16 = |b: &[u8], i: usize| if le { le16(b, i) } else { be16(b, i) };
    let r32 = |b: &[u8], i: usize| if le { le32(b, i) } else { be32(b, i) };
    let mut ifd = r32(&h, 4)? as u64;
    let (mut w, mut ht, mut orientation) = (0, 0, None);
    let mut pages = 0u32;
    let mut guard = 0;
    while ifd != 0 && guard < 10_000 {
        guard += 1;
        let n = r16(&src.exact(ifd, 2)?, 0)? as u64;
        let entries = src.exact(ifd + 2, (n * 12) as usize)?;
        if pages == 0 {
            for k in 0..n as usize {
                let e = &entries[k * 12..k * 12 + 12];
                let (tag, typ) = (r16(e, 0)?, r16(e, 2)?);
                let v = if typ == 3 { r16(e, 8) } else { r32(e, 8) };
                match tag {
                    0x0100 => w = v?,
                    0x0101 => ht = v?,
                    0x0112 => orientation = v,
                    _ => {}
                }
            }
        }
        pages += 1;
        ifd = r32(
            &src.exact(ifd + 2 + n * 12, 4).unwrap_or_else(|| vec![0; 4]),
            0,
        )? as u64;
    }
    oriented(w, ht, orientation, Some(pages.max(1)))
}

/// AVIF / HEIF (libheif): primary image size (`ispe`, `irot` applied), number of top-level images
fn heif(src: &dyn Source) -> Option<Dims> {
    let len = src.len();
    let head = |o: u64| -> Option<(u64, [u8; 4], u64)> {
        let h = src.exact(o, 8)?;
        let mut l = be32(&h, 0)? as u64;
        let mut start = o + 8;
        if l == 1 {
            l = u64::from_be_bytes(src.exact(o + 8, 8)?.try_into().ok()?);
            start += 8;
        } else if l == 0 {
            l = len - o;
        }
        Some((l, [h[4], h[5], h[6], h[7]], start))
    };
    let children = |from: u64, to: u64| -> Vec<(u64, [u8; 4], u64)> {
        let mut v = vec![];
        let mut o = from;
        while o + 8 <= to {
            let Some(b) = head(o) else { break };
            if b.0 < 8 {
                break;
            }
            v.push((o, b.1, b.2));
            o += b.0;
        }
        v
    };
    let top = children(0, len);
    let (moff, _, mstart) = *top.iter().find(|b| &b.1 == b"meta")?;
    let mlen = head(moff)?.0;
    let meta = children(mstart + 4, moff + mlen);
    let get = |k: &[u8; 4]| meta.iter().find(|b| &b.1 == k).copied();
    let primary = match get(b"pitm") {
        Some((_, _, s)) => {
            let v = src.exact(s, 1)?[0];
            if v == 0 {
                be16(&src.exact(s + 4, 2)?, 0)?
            } else {
                be32(&src.exact(s + 4, 4)?, 0)?
            }
        }
        None => 1,
    };
    // item types (to count top-level images: not thumbnails / aux / hidden — approximated by 'iref' thmb/auxl)
    let mut images = vec![];
    if let Some((o, _, s)) = get(b"iinf") {
        let l = head(o)?.0;
        let ver = src.exact(s, 1)?[0];
        let first = if ver == 0 { s + 6 } else { s + 8 };
        for (_, _, is) in children(first, o + l) {
            let v = src.exact(is, 1)?[0];
            if v >= 2 {
                let id = if v == 2 {
                    be16(&src.exact(is + 4, 2)?, 0)?
                } else {
                    be32(&src.exact(is + 4, 4)?, 0)?
                };
                let t = src.exact(is + 4 + if v == 2 { 2 } else { 4 } + 2, 4)?;
                if matches!(
                    &t[..],
                    b"av01" | b"hvc1" | b"grid" | b"iden" | b"jpeg" | b"j2k1" | b"vvc1" | b"unci"
                ) {
                    images.push(id);
                }
            }
        }
    }
    let mut not_top = std::collections::HashSet::new();
    if let Some((o, _, s)) = get(b"iref") {
        let l = head(o)?.0;
        let ver = src.exact(s, 1)?[0];
        let idsz = if ver == 0 { 2 } else { 4 };
        for (ro, kind, rs) in children(s + 4, o + l) {
            let rl = head(ro)?.0;
            let from = if idsz == 2 {
                be16(&src.exact(rs, 2)?, 0)?
            } else {
                be32(&src.exact(rs, 4)?, 0)?
            };
            if matches!(&kind, b"thmb" | b"auxl" | b"dimg" | b"base") {
                not_top.insert(from);
                if &kind == b"dimg" {
                    // grid tiles are referenced from the grid item
                    let cnt = be16(&src.exact(rs + idsz, 2)?, 0)? as u64;
                    for k in 0..cnt {
                        let at = rs + idsz + 2 + k * idsz;
                        if at + idsz <= ro + rl {
                            let id = if idsz == 2 {
                                be16(&src.exact(at, 2)?, 0)?
                            } else {
                                be32(&src.exact(at, 4)?, 0)?
                            };
                            not_top.insert(id);
                        }
                    }
                    not_top.remove(&from);
                }
            }
        }
    }
    let pages = images
        .iter()
        .filter(|i| !not_top.contains(i))
        .count()
        .max(1) as u32;
    // properties of the primary item
    let (po, _, ps) = get(b"iprp")?;
    let pl = head(po)?.0;
    let iprp = children(ps, po + pl);
    let (co, _, cs) = *iprp.iter().find(|b| &b.1 == b"ipco")?;
    let cl = head(co)?.0;
    let props = children(cs, co + cl);
    let (_, _, as_) = *iprp.iter().find(|b| &b.1 == b"ipma")?;
    let av = src.exact(as_, 4)?;
    let (ver, flags) = (av[0], av[3]);
    let mut o = as_ + 4;
    let n = be32(&src.exact(o, 4)?, 0)?;
    o += 4;
    let mut assoc = vec![];
    for _ in 0..n {
        let id = if ver < 1 {
            let v = be16(&src.exact(o, 2)?, 0)?;
            o += 2;
            v
        } else {
            let v = be32(&src.exact(o, 4)?, 0)?;
            o += 4;
            v
        };
        let cnt = src.exact(o, 1)?[0];
        o += 1;
        for _ in 0..cnt {
            let idx = if flags & 1 != 0 {
                let v = be16(&src.exact(o, 2)?, 0)? & 0x7fff;
                o += 2;
                v
            } else {
                let v = (src.exact(o, 1)?[0] & 0x7f) as u32;
                o += 1;
                v
            };
            if id == primary {
                assoc.push(idx);
            }
        }
    }
    let (mut w, mut h, mut rot) = (0, 0, 0);
    for idx in assoc {
        let Some(&(_, kind, s)) = props.get((idx as usize).wrapping_sub(1)) else {
            continue;
        };
        match &kind {
            b"ispe" => {
                let d = src.exact(s + 4, 8)?;
                w = be32(&d, 0)?;
                h = be32(&d, 4)?;
            }
            b"irot" => rot = src.exact(s, 1)?[0] & 3,
            _ => {}
        }
    }
    let (w, h) = if rot % 2 == 1 { (h, w) } else { (w, h) };
    oriented(w, h, None, Some(pages))
}

/// SVG size at 72 DPI (librsvg): width/height attributes with units, else the viewBox.
fn svg(src: &dyn Source) -> Option<Dims> {
    let head = src.read(0, 64 * 1024)?;
    let text = String::from_utf8_lossy(&head);
    let start = text.find("<svg")?;
    let end = start + text[start..].find('>')?;
    let tag = &text[start..end];
    let attr = |name: &str| -> Option<String> {
        let mut from = 0;
        while let Some(p) = tag[from..].find(name) {
            let i = from + p;
            let before = tag[..i].chars().last();
            let rest = tag[i + name.len()..].trim_start();
            if before.is_some_and(char::is_whitespace) {
                if let Some(r) = rest.strip_prefix('=') {
                    let r = r.trim_start();
                    let q = r.chars().next()?;
                    if q == '"' || q == '\'' {
                        let v = &r[1..];
                        return v.find(q).map(|e| v[..e].to_string());
                    }
                }
            }
            from = i + name.len();
        }
        None
    };
    let vb: Option<(f64, f64)> = attr("viewBox").and_then(|v| {
        let n: Vec<f64> = v
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|x| !x.is_empty())
            .filter_map(|x| x.parse().ok())
            .collect();
        (n.len() == 4 && n[2] > 0.0 && n[3] > 0.0).then(|| (n[2], n[3]))
    });
    let len = |v: Option<String>, vb_len: Option<f64>| -> Option<f64> {
        let v = v?;
        let v = v.trim();
        let num_end = v
            .find(|c: char| {
                !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E')
            })
            .unwrap_or(v.len());
        let (n, unit) = (v[..num_end].parse::<f64>().ok()?, v[num_end..].trim());
        Some(match unit {
            "" | "px" => n,
            "in" => n * 72.0,
            "cm" => n * 72.0 / 2.54,
            "mm" => n * 72.0 / 25.4,
            "pt" => n * 72.0 / 72.0,
            "pc" => n * 12.0,
            "em" => n * 12.0,
            "ex" => n * 6.0,
            "%" => vb_len? * n / 100.0,
            _ => return None,
        })
    };
    let w = len(attr("width"), vb.map(|v| v.0)).or(vb.map(|v| v.0));
    let h = len(attr("height"), vb.map(|v| v.1)).or(vb.map(|v| v.1));
    let (w, h) = match (w, h) {
        (Some(w), Some(h)) => (w, h),
        // one side given: keep the viewBox aspect
        (Some(w), None) => (w, vb.map(|v| w * v.1 / v.0).unwrap_or(w)),
        (None, Some(h)) => (vb.map(|v| h * v.0 / v.1).unwrap_or(h), h),
        (None, None) => return None,
    };
    oriented(w.round() as u32, h.round() as u32, None, None)
}

/// Size of an image the way the TypeScript server's sharp metadata reported it (by content, not extension).
pub fn probe(src: &dyn Source) -> Option<Dims> {
    let h = src.read(0, 32)?;
    if h.starts_with(&[0xFF, 0xD8]) {
        return jpeg(src);
    }
    if h.starts_with(b"\x89PNG\r\n\x1a\n") {
        return png(src);
    }
    if h.starts_with(b"GIF87a") || h.starts_with(b"GIF89a") {
        return gif(src);
    }
    if h.starts_with(b"RIFF") && h.get(8..12) == Some(b"WEBP") {
        return webp(src);
    }
    if h.starts_with(b"II*\0") || h.starts_with(b"MM\0*") {
        return tiff(src);
    }
    if h.get(4..8) == Some(b"ftyp") {
        return heif(src);
    }
    let text = String::from_utf8_lossy(&h);
    let t = text.trim_start_matches('\u{feff}').trim_start();
    if t.starts_with("<?xml")
        || t.starts_with("<svg")
        || t.starts_with("<!--")
        || t.starts_with("<!DOCTYPE")
    {
        return svg(src);
    }
    None
}
