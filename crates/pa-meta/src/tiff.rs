//! TIFF / EXIF IFD reader with the exact semantics of exifr 7 (`segment-parsers/tiff-exif.mjs`):
//! value typing (strings, numbers, plain vs typed arrays), error granularity (a bad IFD0 drops the whole
//! segment, a bad EXIF / GPS / Interop IFD keeps the tags read before the error), tag names
//! (`translateKeys`), `mergeOutput` and the computed `latitude` / `longitude`.

use indexmap::IndexMap;

use crate::source::Source;
use crate::tags;
use crate::value::{js_add, Tags, Val};

const TAG_IFD_EXIF: u16 = 0x8769;
const TAG_IFD_GPS: u16 = 0x8825;
const TAG_IFD_INTEROP: u16 = 0xA005;
const TAG_XMP: u16 = 0x02BC;
const TAG_IPTC: u16 = 0x83BB;
const TAG_ICC: u16 = 0x8773;
const TAG_MAKERNOTE: u16 = 0x927C;
const TAG_USERCOMMENT: u16 = 0x9286;

/// Options matching the two exifr calls of the TypeScript server.
#[derive(Debug, Clone)]
pub(crate) struct TiffOpts {
    /// keep the XMP tag of IFD0 (`xmp: true`)
    pub xmp: bool,
    /// `reviveValues` + `translateValues` (the `readExif` call); zone for revived dates
    pub revive_tz: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Tag(u16),
    Name(&'static str),
}

type Block = IndexMap<Key, Val>;

/// Parsed TIFF segment.
#[derive(Debug, Default)]
pub(crate) struct TiffOut {
    /// merged + translated tags
    pub tags: Tags,
    /// raw IFD0 XMP tag (TIFF files carry their XMP packet there); Err = exifr's XMP parser would throw
    pub xmp: Option<Result<Vec<u8>, ()>>,
}

struct Reader<'a> {
    src: &'a dyn Source,
    len: u64,
    le: bool,
}

#[derive(Debug)]
struct Fail;

impl Reader<'_> {
    fn bytes(&self, off: u64, n: u64) -> Result<Vec<u8>, Fail> {
        let end = off.checked_add(n).ok_or(Fail)?;
        if end > self.len {
            return Err(Fail);
        }
        self.src
            .exact(off, usize::try_from(n).map_err(|_| Fail)?)
            .ok_or(Fail)
    }
    fn u16(&self, off: u64) -> Result<u16, Fail> {
        let x = self.bytes(off, 2)?;
        Ok(if self.le {
            u16::from_le_bytes([x[0], x[1]])
        } else {
            u16::from_be_bytes([x[0], x[1]])
        })
    }
    fn u32(&self, off: u64) -> Result<u32, Fail> {
        let x: [u8; 4] = self
            .bytes(off, 4)?
            .as_slice()
            .try_into()
            .map_err(|_| Fail)?;
        Ok(if self.le {
            u32::from_le_bytes(x)
        } else {
            u32::from_be_bytes(x)
        })
    }
    fn i32(&self, off: u64) -> Result<i32, Fail> {
        Ok(self.u32(off)? as i32)
    }
    fn u64(&self, off: u64) -> Result<u64, Fail> {
        let x: [u8; 8] = self
            .bytes(off, 8)?
            .as_slice()
            .try_into()
            .map_err(|_| Fail)?;
        Ok(if self.le {
            u64::from_le_bytes(x)
        } else {
            u64::from_be_bytes(x)
        })
    }

    fn value(&self, typ: u16, off: u64) -> Result<f64, Fail> {
        Ok(match typ {
            1 => self.bytes(off, 1)?[0] as f64,
            3 => self.u16(off)? as f64,
            4 | 13 => self.u32(off)? as f64,
            5 => self.u32(off)? as f64 / self.u32(off + 4)? as f64,
            6 => self.bytes(off, 1)?[0] as i8 as f64,
            8 => self.u16(off)? as i16 as f64,
            9 => self.i32(off)? as f64,
            10 => self.i32(off)? as f64 / self.i32(off + 4)? as f64,
            11 => f32::from_bits(self.u32(off)?) as f64,
            12 => f64::from_bits(self.u64(off)?),
            _ => return Err(Fail),
        })
    }

    fn tag(&self, entry: u64) -> Result<Val, Fail> {
        let typ = self.u16(entry + 2)?;
        let count = self.u32(entry + 4)? as u64;
        let size: Option<u64> = match typ {
            1 | 2 | 6 | 7 => Some(1),
            3 | 8 => Some(2),
            4 | 9 | 11 | 13 => Some(4),
            5 | 10 | 12 => Some(8),
            _ => None,
        };
        // `totalSize <= 4` is false for NaN (unknown type) → the value offset is read
        let off = match size {
            Some(s) if s * count <= 4 => entry + 8,
            _ => self.u32(entry + 8)? as u64,
        };
        let size = size.ok_or(Fail)?; // invalid TIFF value type
        if off > self.len {
            return Err(Fail);
        }
        match typ {
            1 | 7 => Ok(Val::Bytes(self.bytes(off, count)?)),
            2 => Ok(normalize_string(&self.bytes(off, count)?)),
            _ if count == 1 => Ok(Val::Num(self.value(typ, off)?)),
            _ => {
                self.bytes(off, size * count)?;
                let vals = (0..count)
                    .map(|i| self.value(typ, off + i * size))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(match typ {
                    5 | 10 | 13 => Val::Arr(vals.into_iter().map(Val::Num).collect()),
                    _ => Val::Typed(vals),
                })
            }
        }
    }

    /// `parseTags`: entries in file order; a failing entry stops the block (tags read so far stay)
    fn block(&self, off: u64, skip: &[u16], into: &mut Block) -> Result<(), Fail> {
        let n = self.u16(off)? as u64;
        let mut e = off + 2;
        for _ in 0..n {
            let tag = self.u16(e)?;
            if !skip.contains(&tag) {
                let v = self.tag(e)?;
                into.insert(Key::Tag(tag), v);
            }
            e += 12;
        }
        Ok(())
    }
}

/// UTF-8 (TextDecoder: lossy, BOM dropped) → trailing NULs removed → trimmed; '' → undefined
pub(crate) fn decode_utf8(b: &[u8]) -> String {
    let s = String::from_utf8_lossy(b);
    s.strip_prefix('\u{feff}').unwrap_or(&s).to_string()
}

fn normalize_string(b: &[u8]) -> Val {
    let s = decode_utf8(b);
    let s = pa_core::js::trim(s.trim_end_matches('\0')).to_string();
    if s.is_empty() {
        Val::Undef
    } else {
        Val::Str(s)
    }
}

/// exifr's `ConvertDMSToDD(...gps.get(LAT), gps.get(LATREF))` with JS semantics: the value is spread into
/// the (degrees, minutes, seconds, direction) parameters, so the reference only reaches `direction` for
/// three-part values; strings spread into characters and `+` concatenates them.
fn dms_to_dd(v: Option<&Val>, dir: Option<&Val>) -> Result<Val, Fail> {
    let mut args: Vec<Val> = match v {
        Some(Val::Arr(a)) => a.clone(),
        Some(Val::Typed(a)) => a.iter().map(|x| Val::Num(*x)).collect(),
        Some(Val::Bytes(a)) => a.iter().map(|x| Val::Num(*x as f64)).collect(),
        Some(Val::Str(s)) => s.chars().map(|c| Val::Str(c.to_string())).collect(),
        _ => return Err(Fail), // TypeError: not iterable
    };
    args.push(dir.cloned().unwrap_or(Val::Undef));
    let at = |i: usize| args.get(i).cloned().unwrap_or(Val::Undef);
    let dd = js_add(
        &js_add(&at(0), at(1).number() / 60.0),
        at(2).number() / 3600.0,
    );
    let d = at(3);
    Ok(if d == Val::Str("S".into()) || d == Val::Str("W".into()) {
        Val::Num(-dd.number())
    } else {
        dd
    })
}

fn offset_of(v: Option<&Val>) -> Option<Result<u64, Fail>> {
    match v {
        None | Some(Val::Undef) => None,
        Some(Val::Num(n)) if *n >= 0.0 && n.is_finite() => Some(Ok(*n as u64)),
        Some(_) => Some(Err(Fail)),
    }
}

fn key_name(block: &str, tag: u16) -> Option<&'static str> {
    let dict = match block {
        "ifd0" => tags::IFD0_KEYS,
        "exif" => tags::EXIF_KEYS,
        "gps" => tags::GPS_KEYS,
        _ => tags::INTEROP_KEYS,
    };
    dict.binary_search_by_key(&tag, |e| e.0)
        .ok()
        .map(|i| dict[i].1)
}

/// exifr `reviveDate`: "YYYY:MM:DD HH:MM:SS" as a LOCAL time of `tz` (`new Date(y, m-1, d)` + setHours…)
fn revive_date(v: &Val, tz: &str) -> Val {
    let Val::Str(s) = v else { return Val::Undef };
    let p: Vec<f64> = pa_core::js::trim(s)
        .split(['-', ':', ' '])
        .map(pa_core::js::number)
        .collect();
    let g = |i: usize| p.get(i).copied().unwrap_or(f64::NAN);
    let (mut y, mo, d) = (g(0), g(1), g(2));
    if !(y.is_finite() && mo.is_finite() && d.is_finite()) {
        return v.clone();
    }
    // the Date constructor maps two-digit years to 19xx
    y = y.trunc();
    if (0.0..=99.0).contains(&y) {
        y += 1900.0;
    }
    // `!Number.isNaN(x)` is also true for a missing part (undefined) — setHours(undefined) then gives an
    // invalid date, and the string is returned
    let parts = [p.get(3).copied(), p.get(4).copied(), p.get(5).copied()];
    let (hh, mm, ss) = if parts.iter().all(|x| x.is_none_or(|x| !x.is_nan())) {
        match parts {
            [Some(h), Some(mi), Some(se)] if h.is_finite() && mi.is_finite() && se.is_finite() => {
                (h.trunc() as i64, mi.trunc() as i64, se.trunc() as i64)
            }
            _ => return v.clone(),
        }
    } else {
        (0, 0, 0)
    };
    let local = pa_core::js::date_utc(
        y as i64,
        mo.trunc() as i64 - 1,
        d.trunc() as i64,
        hh,
        mm,
        ss,
        0,
    );
    if local.abs() > 8_640_000_000_000_000 {
        return v.clone();
    }
    Val::Num(pa_core::local_to_instant(local, tz) as f64)
}

fn translate_value(block: &str, tag: u16, v: Val) -> Val {
    let k = v.js_string();
    match tags::VALUES
        .iter()
        .find(|e| e.0 == block && e.1 == tag && e.2 == k)
    {
        Some(e) => Val::Str(e.3.to_string()),
        None => v,
    }
}

/// Parse one TIFF segment (JPEG APP1 payload after "Exif\0\0", PNG eXIf, HEIF Exif item, TIFF file).
/// `Err` = exifr threw in IFD0 / the header (no tags at all from this segment).
pub(crate) fn parse(src: &dyn Source, opts: &TiffOpts) -> Result<TiffOut, ()> {
    let head = src.exact(0, 2).ok_or(())?;
    // byte order: 'II' little endian, anything else is read big endian (exifr leaves `le` undefined)
    let r = Reader {
        src,
        len: src.len(),
        le: head == b"II",
    };
    let mut ifd0_skip = vec![TAG_IPTC, TAG_ICC];
    if !opts.xmp {
        ifd0_skip.push(TAG_XMP);
    }
    let exif_skip = [TAG_MAKERNOTE, TAG_USERCOMMENT];

    let ifd0_off = r.u32(4).map_err(|_| ())? as u64;
    if ifd0_off < 8 {
        return Err(()); // Malformed EXIF data
    }
    let mut ifd0 = Block::new();
    r.block(ifd0_off, &ifd0_skip, &mut ifd0).map_err(|_| ())?;
    let mut exif = Block::new();
    let mut gps = Block::new();
    let mut interop = Block::new();
    let mut xmp = None;
    if !ifd0.is_empty() {
        let exif_off = ifd0.get(&Key::Tag(TAG_IFD_EXIF)).cloned();
        let mut interop_off = ifd0.get(&Key::Tag(TAG_IFD_INTEROP)).cloned();
        let gps_off = ifd0.get(&Key::Tag(TAG_IFD_GPS)).cloned();
        // what exifr's XMP parser accepts as input (`BufferView.from`): strings, Uint8Array, plain arrays
        // (→ Uint8Array), numbers (→ zero-filled buffer); other typed arrays throw (`xmp_error`)
        xmp = match ifd0.get(&Key::Tag(TAG_XMP)) {
            Some(Val::Bytes(t)) => Some(Ok(t.clone())),
            Some(Val::Str(s)) => Some(Ok(s.as_bytes().to_vec())),
            Some(Val::Arr(a)) => Some(Ok(a.iter().map(|x| to_uint8(x.number())).collect())),
            Some(Val::Num(n)) if *n == 0.0 => None,
            Some(Val::Num(n)) if n.trunc() < 0.0 => Some(Err(())),
            Some(Val::Num(_)) => Some(Ok(vec![])),
            Some(Val::Undef) | None => None,
            Some(_) => Some(Err(())),
        };
        for t in [
            TAG_IFD_EXIF,
            TAG_IFD_INTEROP,
            TAG_IFD_GPS,
            TAG_XMP,
            TAG_IPTC,
            TAG_ICC,
        ] {
            ifd0.shift_remove(&Key::Tag(t));
        }
        // EXIF IFD (errors are swallowed: silentErrors)
        // (a failing EXIF IFD skips the post-processing below, like the exception in parseExifBlock)
        if let Some(Ok(off)) = offset_of(exif_off.as_ref()) {
            if r.block(off, &exif_skip, &mut exif).is_ok() {
                if interop_off
                    .as_ref()
                    .is_none_or(|v| v.number() == 0.0 || v.number().is_nan() || v.is_undef())
                {
                    interop_off = exif.get(&Key::Tag(TAG_IFD_INTEROP)).cloned();
                }
                for t in [TAG_IFD_INTEROP, TAG_MAKERNOTE, TAG_USERCOMMENT] {
                    exif.shift_remove(&Key::Tag(t));
                }
                for t in [0xA300u16, 0xA301] {
                    if let Some(v) = exif.get_mut(&Key::Tag(t)) {
                        match v {
                            Val::Typed(a) if a.len() == 1 => *v = Val::Num(a[0]),
                            Val::Bytes(a) if a.len() == 1 => *v = Val::Num(a[0] as f64),
                            Val::Arr(a) if a.len() == 1 => *v = a[0].clone(),
                            _ => {}
                        }
                    }
                }
            }
        }
        // GPS IFD + computed latitude / longitude
        if let Some(Ok(off)) = offset_of(gps_off.as_ref()) {
            if r.block(off, &[], &mut gps).is_ok()
                && gps.contains_key(&Key::Tag(2))
                && gps.contains_key(&Key::Tag(4))
            {
                if let Ok(lat) = dms_to_dd(gps.get(&Key::Tag(2)), gps.get(&Key::Tag(1))) {
                    gps.insert(Key::Name("latitude"), lat);
                    if let Ok(lon) = dms_to_dd(gps.get(&Key::Tag(4)), gps.get(&Key::Tag(3))) {
                        gps.insert(Key::Name("longitude"), lon);
                    }
                }
            }
        }
        // Interop IFD
        if let Some(Ok(off)) = offset_of(interop_off.as_ref()) {
            let _ = r.block(off, &[], &mut interop);
        }
    }

    let mut out = Tags::new();
    for (name, block) in [
        ("ifd0", &ifd0),
        ("exif", &exif),
        ("gps", &gps),
        ("interop", &interop),
    ] {
        for (k, v) in block {
            let (key, mut v) = match k {
                Key::Tag(t) => (
                    key_name(name, *t)
                        .map(str::to_string)
                        .unwrap_or_else(|| t.to_string()),
                    v.clone(),
                ),
                Key::Name(n) => (n.to_string(), v.clone()),
            };
            if let (Some(tz), Key::Tag(t)) = (&opts.revive_tz, k) {
                // a throwing reviver rejects the whole TIFF segment (silentErrors drops it)
                v = match revive(name, *t, &v, tz).map_err(|_| ())? {
                    Some(r) => r,
                    None => translate_value(name, *t, v),
                };
            }
            out.insert(key, v);
        }
    }
    Ok(TiffOut { tags: out, xmp })
}

/// JS `ToUint8` (Uint8Array element conversion)
fn to_uint8(n: f64) -> u8 {
    if !n.is_finite() {
        return 0;
    }
    n.trunc().rem_euclid(256.0) as u8
}

/// exifr's tag revivers (`dicts/tiff-revivers.mjs`): Ok(None) = no reviver for this tag, Err = it throws
fn revive(block: &str, tag: u16, v: &Val, tz: &str) -> Result<Option<Val>, Fail> {
    let arr_from = |v: &Val| -> Result<Vec<Val>, Fail> {
        match v {
            Val::Undef => Err(Fail),
            Val::Arr(a) => Ok(a.clone()),
            Val::Typed(a) => Ok(a.iter().map(|x| Val::Num(*x)).collect()),
            Val::Bytes(a) => Ok(a.iter().map(|x| Val::Num(*x as f64)).collect()),
            Val::Str(s) => Ok(s.chars().map(|c| Val::Str(c.to_string())).collect()),
            _ => Ok(vec![]),
        }
    };
    let join = |a: &[Val], sep: &str| {
        a.iter()
            .map(|x| {
                if x.is_undef() {
                    String::new()
                } else {
                    x.js_string()
                }
            })
            .collect::<Vec<_>>()
            .join(sep)
    };
    Ok(Some(match (block, tag) {
        ("ifd0", 0x0132) | ("exif", 0x9003) | ("exif", 0x9004) => revive_date(v, tz),
        // toAsciiString: TextDecoder only takes strings / buffers
        ("ifd0", 0xC68B) => match v {
            Val::Str(_) => v.clone(),
            Val::Undef => Val::Str(String::new()),
            Val::Bytes(b) => Val::Str(decode_utf8(b)),
            Val::Typed(_) => v.clone(),
            _ => return Err(Fail),
        },
        // reviveUcs2String
        ("ifd0", 0x9C9B..=0x9C9F) => match v {
            Val::Str(_) => v.clone(),
            Val::Undef => return Err(Fail),
            Val::Arr(_) | Val::Typed(_) | Val::Bytes(_) => {
                let a: Vec<f64> = arr_from(v)?.iter().map(Val::number).collect();
                let i32_of = |x: Option<&f64>| {
                    x.map(|x| {
                        if x.is_finite() {
                            x.trunc().rem_euclid(4_294_967_296.0) as u32 as i32
                        } else {
                            0
                        }
                    })
                    .unwrap_or(0)
                };
                let le = a.get(1) == Some(&0.0) && a.last() == Some(&0.0);
                let mut s = String::new();
                for i in (0..a.len()).step_by(2) {
                    let (hi, lo) = if le {
                        (a.get(i + 1), a.get(i))
                    } else {
                        (a.get(i), a.get(i + 1))
                    };
                    let cp = i32_of(hi).wrapping_shl(8) | i32_of(lo);
                    let c = u32::try_from(cp)
                        .ok()
                        .filter(|c| *c <= 0x10FFFF)
                        .ok_or(Fail)?;
                    s.push(char::from_u32(c).unwrap_or('\u{fffd}'));
                }
                normalize_string(s.as_bytes())
            }
            _ => Val::Undef,
        },
        // reviveVersion
        ("exif", 0xA000) | ("exif", 0x9000) => {
            let a = arr_from(v)?;
            Val::Str(join(a.get(1..).unwrap_or(&[]), "."))
        }
        ("exif", 0xA002) | ("exif", 0xA003) => match v {
            Val::Typed(a) => a.first().map(|x| Val::Num(*x)).unwrap_or(Val::Undef),
            Val::Bytes(a) => a.first().map(|x| Val::Num(*x as f64)).unwrap_or(Val::Undef),
            Val::Arr(a) => a.first().cloned().unwrap_or(Val::Undef),
            other => other.clone(),
        },
        ("gps", 0x0000) => Val::Str(join(&arr_from(v)?, ".")),
        ("gps", 0x0007) => Val::Str(join(&arr_from(v)?, ":")),
        _ => return Ok(None),
    }))
}
