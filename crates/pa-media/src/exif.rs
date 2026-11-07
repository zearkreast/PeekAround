//! Minimal EXIF access: the orientation tag (0x0112) of JPEG / PNG / WebP / TIFF files, read from
//! the container without decoding pixels. Full EXIF/XMP extraction lives in `pa-meta`.

/// The TIFF payload of the EXIF block of a JPEG / PNG / WebP / TIFF file (as far as `data` reaches).
pub fn exif_block(data: &[u8]) -> Option<&[u8]> {
    if data.len() < 12 {
        return None;
    }
    if data.starts_with(&[0xff, 0xd8]) {
        return jpeg_exif(data);
    }
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return png_exif(data);
    }
    if data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        return webp_exif(data);
    }
    if data.starts_with(b"II*\0") || data.starts_with(b"MM\0*") {
        return Some(data);
    }
    None
}

fn strip_exif_prefix(b: &[u8]) -> &[u8] {
    if b.starts_with(b"Exif\0\0") {
        &b[6..]
    } else {
        b
    }
}

fn jpeg_exif(d: &[u8]) -> Option<&[u8]> {
    let mut p = 2;
    while p + 4 <= d.len() {
        if d[p] != 0xff {
            return None;
        }
        let m = d[p + 1];
        if m == 0xff {
            p += 1;
            continue;
        }
        if m == 0xd8 || (0xd0..=0xd7).contains(&m) || m == 0x01 {
            p += 2;
            continue;
        }
        if m == 0xda || m == 0xd9 {
            return None;
        }
        let len = u16::from_be_bytes([d[p + 2], d[p + 3]]) as usize;
        if len < 2 {
            return None;
        }
        let start = p + 4;
        let end = (p + 2 + len).min(d.len());
        if m == 0xe1 && d[start.min(end)..end].starts_with(b"Exif\0\0") {
            return Some(&d[start + 6..end]);
        }
        p += 2 + len;
    }
    None
}

fn png_exif(d: &[u8]) -> Option<&[u8]> {
    let mut p = 8;
    while p + 8 <= d.len() {
        let len = u32::from_be_bytes([d[p], d[p + 1], d[p + 2], d[p + 3]]) as usize;
        let kind = &d[p + 4..p + 8];
        let start = p + 8;
        let end = start.checked_add(len)?;
        if kind == b"eXIf" {
            return Some(strip_exif_prefix(&d[start.min(d.len())..end.min(d.len())]));
        }
        if kind == b"IEND" {
            return None;
        }
        p = end + 4;
    }
    None
}

/// RIFF chunks of a WebP file: (fourcc, payload)
pub(crate) fn riff_chunks(d: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    let mut p = 12usize;
    std::iter::from_fn(move || {
        if p + 8 > d.len() {
            return None;
        }
        let kind = &d[p..p + 4];
        let len = u32::from_le_bytes([d[p + 4], d[p + 5], d[p + 6], d[p + 7]]) as usize;
        let start = p + 8;
        let end = start.saturating_add(len).min(d.len());
        p = start.saturating_add(len).saturating_add(len & 1);
        Some((kind, &d[start..end]))
    })
}

fn webp_exif(d: &[u8]) -> Option<&[u8]> {
    riff_chunks(d)
        .find(|(k, _)| *k == b"EXIF")
        .map(|(_, v)| strip_exif_prefix(v))
}

/// Orientation (1–8) from a TIFF/EXIF block's IFD0.
pub fn tiff_orientation(t: &[u8]) -> Option<u8> {
    if t.len() < 8 {
        return None;
    }
    let le = match &t[0..2] {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |o: usize| -> Option<u16> {
        let b = t.get(o..o + 2)?;
        Some(if le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    };
    let u32_at = |o: usize| -> Option<u32> {
        let b = t.get(o..o + 4)?;
        Some(if le {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    let ifd = u32_at(4)? as usize;
    let count = u16_at(ifd)? as usize;
    for i in 0..count.min(512) {
        let e = ifd + 2 + i * 12;
        if u16_at(e)? == 0x0112 {
            let typ = u16_at(e + 2)?;
            let v = match typ {
                3 => u16_at(e + 8)? as u32,
                4 => u32_at(e + 8)?,
                _ => return None,
            };
            return (1..=8).contains(&v).then_some(v as u8);
        }
    }
    None
}

/// EXIF orientation of an image file's bytes (None: no EXIF / no orientation tag).
pub fn orientation(data: &[u8]) -> Option<u8> {
    exif_block(data).and_then(tiff_orientation)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiff(o: u16, le: bool) -> Vec<u8> {
        let mut v = Vec::new();
        let w16 = |v: &mut Vec<u8>, x: u16| {
            v.extend_from_slice(&if le { x.to_le_bytes() } else { x.to_be_bytes() })
        };
        let w32 = |v: &mut Vec<u8>, x: u32| {
            v.extend_from_slice(&if le { x.to_le_bytes() } else { x.to_be_bytes() })
        };
        v.extend_from_slice(if le { b"II" } else { b"MM" });
        w16(&mut v, 42);
        w32(&mut v, 8);
        w16(&mut v, 1);
        w16(&mut v, 0x0112);
        w16(&mut v, 3);
        w32(&mut v, 1);
        w16(&mut v, o);
        w16(&mut v, 0);
        w32(&mut v, 0);
        v
    }

    #[test]
    fn reads_orientation_from_containers() {
        assert_eq!(tiff_orientation(&tiff(6, true)), Some(6));
        assert_eq!(tiff_orientation(&tiff(8, false)), Some(8));
        assert_eq!(tiff_orientation(&tiff(9, false)), None);
        // JPEG: SOI, APP0, APP1 Exif
        let t = tiff(3, false);
        let mut j = vec![0xff, 0xd8, 0xff, 0xe0, 0, 4, 1, 2, 0xff, 0xe1];
        j.extend_from_slice(&((t.len() + 8) as u16).to_be_bytes());
        j.extend_from_slice(b"Exif\0\0");
        j.extend_from_slice(&t);
        j.extend_from_slice(&[0xff, 0xda, 0, 2]);
        assert_eq!(orientation(&j), Some(3));
        // PNG eXIf
        let t = tiff(5, true);
        let mut p = b"\x89PNG\r\n\x1a\n".to_vec();
        p.extend_from_slice(&(t.len() as u32).to_be_bytes());
        p.extend_from_slice(b"eXIf");
        p.extend_from_slice(&t);
        p.extend_from_slice(&[0; 4]);
        assert_eq!(orientation(&p), Some(5));
        assert_eq!(orientation(b"not an image at all"), None);
    }
}
