//! Font file facts for the font manager (what `fontkit` gave the TS server): family / subfamily
//! names (name table IDs 1 / 2, English), OS/2 weight, post italic angle — from TTF / OTF / TTC,
//! WOFF (zlib) and WOFF2 (brotli; only untransformed tables are needed).

use std::collections::HashMap;
use std::io::Read;

/// Parsed facts.
#[derive(Debug, Clone, PartialEq)]
pub struct FontFacts {
    pub family: Option<String>,
    pub subfamily: Option<String>,
    pub weight: Option<u16>,
    pub italic_angle: f64,
}

fn be16(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]))
}
fn be32(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *b.get(i)?,
        *b.get(i + 1)?,
        *b.get(i + 2)?,
        *b.get(i + 3)?,
    ]))
}

fn sfnt_tables(b: &[u8], base: usize) -> Option<HashMap<[u8; 4], Vec<u8>>> {
    let n = be16(b, base + 4)? as usize;
    let mut m = HashMap::new();
    for i in 0..n {
        let r = base + 12 + i * 16;
        let tag: [u8; 4] = b.get(r..r + 4)?.try_into().ok()?;
        let off = be32(b, r + 8)? as usize;
        let len = be32(b, r + 12)? as usize;
        m.insert(tag, b.get(off..off.checked_add(len)?)?.to_vec());
    }
    Some(m)
}

fn woff_tables(b: &[u8]) -> Option<HashMap<[u8; 4], Vec<u8>>> {
    let n = be16(b, 12)? as usize;
    let mut m = HashMap::new();
    for i in 0..n {
        let r = 44 + i * 20;
        let tag: [u8; 4] = b.get(r..r + 4)?.try_into().ok()?;
        let off = be32(b, r + 4)? as usize;
        let clen = be32(b, r + 8)? as usize;
        let olen = be32(b, r + 12)? as usize;
        let data = b.get(off..off.checked_add(clen)?)?;
        let t = if clen < olen {
            let mut out = Vec::with_capacity(olen);
            flate2::read::ZlibDecoder::new(data)
                .read_to_end(&mut out)
                .ok()?;
            out
        } else {
            data.to_vec()
        };
        m.insert(tag, t);
    }
    Some(m)
}

const WOFF2_TAGS: [&[u8; 4]; 63] = [
    b"cmap", b"head", b"hhea", b"hmtx", b"maxp", b"name", b"OS/2", b"post", b"cvt ", b"fpgm",
    b"glyf", b"loca", b"prep", b"CFF ", b"VORG", b"EBDT", b"EBLC", b"gasp", b"hdmx", b"kern",
    b"LTSH", b"PCLT", b"VDMX", b"vhea", b"vmtx", b"BASE", b"GDEF", b"GPOS", b"GSUB", b"EBSC",
    b"JSTF", b"MATH", b"CBDT", b"CBLC", b"COLR", b"CPAL", b"SVG ", b"sbix", b"acnt", b"avar",
    b"bdat", b"bloc", b"bsln", b"cvar", b"fdsc", b"feat", b"fmtx", b"fvar", b"gvar", b"hsty",
    b"just", b"lcar", b"mort", b"morx", b"opbd", b"prop", b"trak", b"Zapf", b"Silf", b"Glat",
    b"Gloc", b"Feat", b"Sill",
];

fn base128(b: &[u8], i: &mut usize) -> Option<u32> {
    let mut v: u32 = 0;
    for k in 0..5 {
        let c = *b.get(*i)?;
        *i += 1;
        if k == 0 && c == 0x80 {
            return None;
        }
        v = v.checked_mul(128)? | (c & 0x7f) as u32;
        if c & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

fn woff2_tables(b: &[u8]) -> Option<HashMap<[u8; 4], Vec<u8>>> {
    let n = be16(b, 12)? as usize;
    let comp_len = be32(b, 20)? as usize;
    let mut i = 48;
    let mut dir = Vec::new();
    for _ in 0..n {
        let flags = *b.get(i)?;
        i += 1;
        let tag: [u8; 4] = if flags & 0x3f == 63 {
            let t = b.get(i..i + 4)?.try_into().ok()?;
            i += 4;
            t
        } else {
            *WOFF2_TAGS[(flags & 0x3f) as usize]
        };
        let version = (flags >> 6) & 3;
        let orig = base128(b, &mut i)? as usize;
        let transformed = if &tag == b"glyf" || &tag == b"loca" {
            version == 0
        } else {
            version != 0
        };
        let len = if transformed {
            base128(b, &mut i)? as usize
        } else {
            orig
        };
        dir.push((tag, len, transformed));
    }
    if &b.get(4..8)? == b"ttcf" {
        return None;
    }
    let data = b.get(i..i.checked_add(comp_len)?)?;
    let mut out = Vec::new();
    brotli_decompressor::Decompressor::new(data, 4096)
        .read_to_end(&mut out)
        .ok()?;
    let mut m = HashMap::new();
    let mut off = 0usize;
    for (tag, len, transformed) in dir {
        let t = out.get(off..off.checked_add(len)?)?;
        if !transformed {
            m.insert(tag, t.to_vec());
        }
        off += len;
    }
    Some(m)
}

fn mac_roman(b: &[u8]) -> String {
    b.iter()
        .map(|&c| if c < 0x80 { c as char } else { '?' })
        .collect()
}

fn utf16be(b: &[u8]) -> String {
    let u: Vec<u16> = b
        .chunks_exact(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&u)
}

fn name_of(t: &[u8], id: u16) -> Option<String> {
    let count = be16(t, 2)? as usize;
    let str_off = be16(t, 4)? as usize;
    let mut best: Option<(u8, String)> = None;
    for i in 0..count {
        let r = 6 + i * 12;
        let (pid, eid, lid, nid, len, off) = (
            be16(t, r)?,
            be16(t, r + 2)?,
            be16(t, r + 4)?,
            be16(t, r + 6)?,
            be16(t, r + 8)? as usize,
            be16(t, r + 10)? as usize,
        );
        if nid != id {
            continue;
        }
        let Some(raw) = t.get(str_off + off..str_off + off + len) else {
            continue;
        };
        let (rank, s) = match pid {
            3 if lid == 0x409 => (0, utf16be(raw)),
            1 if lid == 0 && eid == 0 => (1, mac_roman(raw)),
            0 => (2, utf16be(raw)),
            3 => (3, utf16be(raw)),
            _ => (4, mac_roman(raw)),
        };
        if best.as_ref().is_none_or(|(b, _)| rank < *b) {
            best = Some((rank, s));
        }
    }
    best.map(|b| b.1).filter(|s| !s.is_empty())
}

/// Parse a font file; None = invalid / unsupported.
pub fn parse_font(b: &[u8]) -> Option<FontFacts> {
    let sig = b.get(0..4)?;
    let tables = match sig {
        b"wOFF" => woff_tables(b)?,
        b"wOF2" => woff2_tables(b)?,
        b"ttcf" => {
            let first = be32(b, 12)? as usize;
            sfnt_tables(b, first)?
        }
        [0, 1, 0, 0] | b"OTTO" | b"true" | b"typ1" => sfnt_tables(b, 0)?,
        _ => return None,
    };
    if !tables.contains_key(b"head")
        && !tables.contains_key(b"CFF ")
        && !tables.contains_key(b"name")
    {
        return None;
    }
    let name = tables.get(b"name");
    let os2 = tables.get(b"OS/2");
    let post = tables.get(b"post");
    Some(FontFacts {
        family: name.and_then(|t| name_of(t, 1)),
        subfamily: name.and_then(|t| name_of(t, 2)),
        weight: os2.and_then(|t| be16(t, 4)),
        italic_angle: post
            .and_then(|t| be32(t, 4))
            .map(|v| v as i32 as f64 / 65536.0)
            .unwrap_or(0.0),
    })
}
