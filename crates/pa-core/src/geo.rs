//! Offline reverse geocoding (mirrors `geo.ts`).
//!
//! Data: GeoNames (<https://www.geonames.org>), CC BY 4.0 — built by `scripts/build-geo.mjs` into
//! `assets/geo/cities.bin` (gzip). Load it with [`decode_geo_asset`] (gzip or raw bytes) or
//! [`GeoIndex::from_bytes`] (raw "PKG1" bytes).
//!
//! Binary layout (little endian):
//!   "PKG1" | u32 jsonLength | JSON header (UTF-8) | pad to 4 |
//!   i32 lat×1e5 \[n\] | i32 lon×1e5 \[n\] | u32 population \[n\] | u32 nameIndex \[n\] | u16 adminIndex \[n\] | u8 featureIndex \[n\]
//! Every column is byte-plane shuffled.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::js;
use crate::types::PlaceInfo;

/// Path of the asset inside the web app's public folder.
pub const GEO_ASSET_PATH: &str = "geo/cities.bin";
/// Default search radius.
pub const GEO_MAX_KM: f64 = 80.0;

/// JSON header of the asset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoHeader {
    /// format version
    pub v: u32,
    /// number of places
    pub n: usize,
    /// GeoNames attribution
    pub attribution: String,
    /// country codes
    pub cc: Vec<String>,
    /// `[ccIndex, code, name]`
    pub admin: Vec<(usize, String, String)>,
    /// feature codes
    pub fcode: Vec<String>,
    /// place names
    pub names: Vec<String>,
}

/// Result of [`GeoIndex::nearest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeoMatch {
    /// nearest populated place
    pub city: String,
    /// first-level admin area name
    pub admin1: Option<String>,
    /// ISO 3166-1 alpha-2
    pub cc: String,
    /// `${cc}-${admin1 code || '00'}`
    pub key: String,
    /// place id (row in the asset)
    pub id: usize,
    /// GeoNames admin1 code ('34' for İstanbul), None when unknown
    pub admin1_code: Option<String>,
    /// place latitude
    #[serde(with = "js::num")]
    pub lat: f64,
    /// place longitude
    #[serde(with = "js::num")]
    pub lon: f64,
    /// distance to the query point
    #[serde(with = "js::num")]
    pub km: f64,
}

/// Errors while loading the asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GeoError {
    /// gzip bytes passed to [`GeoIndex::from_bytes`]
    Compressed,
    /// not a "PKG1" asset / truncated
    Format(String),
}

impl std::fmt::Display for GeoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GeoError::Compressed => write!(f, "geo asset is gzip compressed — decompress it first"),
            GeoError::Format(m) => write!(f, "not a PeekAround geo asset: {m}"),
        }
    }
}

impl std::error::Error for GeoError {}

const R: f64 = 6371.0088;
const RAD: f64 = std::f64::consts::PI / 180.0;

/// Haversine distance in km.
pub fn distance_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let d_lat = (lat2 - lat1) * RAD;
    let d_lon = (lon2 - lon1) * RAD;
    let a = (d_lat / 2.0).sin().powi(2)
        + (lat1 * RAD).cos() * (lat2 * RAD).cos() * (d_lon / 2.0).sin().powi(2);
    2.0 * R * a.sqrt().min(1.0).asin()
}

/// finite, in range and not the (0,0) "null island" placeholder
pub fn is_valid_gps(lat: f64, lon: f64) -> bool {
    if !lat.is_finite() || !lon.is_finite() {
        return false;
    }
    if lat.abs() > 90.0 || lon.abs() > 180.0 {
        return false;
    }
    !(lat.abs() < 1e-4 && lon.abs() < 1e-4)
}

/// [`is_valid_gps`] for optional values (missing → false).
pub fn is_valid_gps_opt(lat: Option<f64>, lon: Option<f64>) -> bool {
    matches!((lat, lon), (Some(a), Some(b)) if is_valid_gps(a, b))
}

/// A latitude / longitude pair.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LatLon {
    /// latitude
    #[serde(with = "js::num")]
    pub lat: f64,
    /// longitude
    #[serde(with = "js::num")]
    pub lon: f64,
}

/// `[+-]\d+(?:\.\d+)?` at the start of `s` → its length
fn signed_num_len(s: &[u8]) -> Option<usize> {
    if s.is_empty() || (s[0] != b'+' && s[0] != b'-') {
        return None;
    }
    let mut i = 1;
    while i < s.len() && s[i].is_ascii_digit() {
        i += 1;
    }
    if i == 1 {
        return None;
    }
    if i + 1 < s.len() && s[i] == b'.' && s[i + 1].is_ascii_digit() {
        i += 1;
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
        }
    }
    Some(i)
}

/// ISO 6709 string as written by phones into video metadata: "+41.0123+028.9744+050.000/"
pub fn parse_iso6709(s: &str) -> Option<LatLon> {
    if js::len16(s) > 256 {
        return None;
    }
    // /^\s*([+-]\d+(?:\.\d+)?)([+-]\d+(?:\.\d+)?)([+-]\d+(?:\.\d+)?)?(?:CRS[^/]*)?\/?\s*$/
    let t = s.trim_start_matches(js::is_space);
    let b = t.as_bytes();
    let l1 = signed_num_len(b)?;
    let l2 = signed_num_len(&b[l1..])?;
    let mut i = l1 + l2;
    // the regex may backtrack the numbers' fraction parts; a trailing '.' never matches the tail anyway
    if let Some(l3) = signed_num_len(&b[i..]) {
        i += l3;
    }
    let mut rest = &t[i..];
    if let Some(r) = rest.strip_prefix("CRS") {
        rest = match r.find('/') {
            Some(p) => &r[p..],
            None => "",
        };
    }
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    if !rest.chars().all(js::is_space) {
        return None;
    }
    let lat = parse_deg(&t[..l1], 2);
    let lon = parse_deg(&t[l1..l1 + l2], 3);
    if is_valid_gps(lat, lon) {
        Some(LatLon { lat, lon })
    } else {
        None
    }
}

/// ±DD.DDD, ±DDMM.MMM, ±DDMMSS.SS (degree digits: 2 for lat, 3 for lon)
fn parse_deg(s: &str, deg_digits: usize) -> f64 {
    let sign = if s.starts_with('-') { -1.0 } else { 1.0 };
    let body = &s[1..];
    let int_len = body.split('.').next().unwrap_or("").len();
    if int_len <= deg_digits {
        return sign * js::number(body);
    }
    let deg = js::number(&body[..deg_digits]);
    if int_len <= deg_digits + 2 {
        return sign * (deg + js::number(&body[deg_digits..]) / 60.0);
    }
    sign * (deg
        + js::number(&body[deg_digits..deg_digits + 2]) / 60.0
        + js::number(&body[deg_digits + 2..]) / 3600.0)
}

/// true for gzip data (magic 1f 8b)
pub fn is_gzip(b: &[u8]) -> bool {
    b.len() > 2 && b[0] == 0x1f && b[1] == 0x8b
}

const CELL: f64 = 0.25;
const COLS: i64 = (360.0 / CELL) as i64;
const ROWS: i64 = (180.0 / CELL) as i64;

fn cell_of(lat: f64, lon: f64) -> (i64, i64) {
    let r = ((lat + 90.0) / CELL)
        .floor()
        .max(0.0)
        .min((ROWS - 1) as f64) as i64;
    let c = (((((lon + 180.0) / CELL).floor() as i64) % COLS) + COLS) % COLS;
    (r, c)
}

/// In-memory reverse geocoder.
#[derive(Debug, Clone)]
pub struct GeoIndex {
    header: GeoHeader,
    lat: Vec<f64>,
    lon: Vec<f64>,
    name_idx: Vec<u32>,
    admin_idx: Vec<u16>,
    cell_start: Vec<u32>,
    cell_items: Vec<u32>,
    admin_by_key: HashMap<String, String>,
}

/// Decompress (when gzip'ed) and load the GeoNames asset.
#[cfg(feature = "gzip")]
pub fn decode_geo_asset(bytes: &[u8]) -> Result<GeoIndex, GeoError> {
    if is_gzip(bytes) {
        use std::io::Read;
        let mut out = Vec::with_capacity(bytes.len() * 3);
        flate2::read::GzDecoder::new(bytes)
            .read_to_end(&mut out)
            .map_err(|e| GeoError::Format(e.to_string()))?;
        GeoIndex::from_bytes(&out)
    } else {
        GeoIndex::from_bytes(bytes)
    }
}

impl GeoIndex {
    /// Load the raw (decompressed) "PKG1" asset.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, GeoError> {
        if is_gzip(bytes) {
            return Err(GeoError::Compressed);
        }
        if bytes.len() < 8 || &bytes[..4] != b"PKG1" {
            return Err(GeoError::Format("bad magic".into()));
        }
        let jl = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
        let json = bytes
            .get(8..8 + jl)
            .ok_or_else(|| GeoError::Format("truncated header".into()))?;
        let header: GeoHeader =
            serde_json::from_slice(json).map_err(|e| GeoError::Format(e.to_string()))?;
        let n = header.n;
        let mut off = (8 + jl + 3) & !3;
        let need = off + n * (4 + 4 + 4 + 4 + 2 + 1);
        if bytes.len() < need {
            return Err(GeoError::Format("truncated columns".into()));
        }
        let col = |off: &mut usize, size: usize| -> Vec<u64> {
            let mut out = vec![0u64; n];
            for (i, o) in out.iter_mut().enumerate() {
                let mut v = 0u64;
                for k in (0..size).rev() {
                    v = v * 256 + bytes[*off + k * n + i] as u64;
                }
                *o = v;
            }
            *off += n * size;
            out
        };
        let lat: Vec<f64> = col(&mut off, 4)
            .into_iter()
            .map(|v| v as u32 as i32 as f64 / 1e5)
            .collect();
        let lon: Vec<f64> = col(&mut off, 4)
            .into_iter()
            .map(|v| v as u32 as i32 as f64 / 1e5)
            .collect();
        off += n * 4; // population
        let name_idx: Vec<u32> = col(&mut off, 4).into_iter().map(|v| v as u32).collect();
        let admin_idx: Vec<u16> = col(&mut off, 2).into_iter().map(|v| v as u16).collect();
        for (&a, &nm) in admin_idx.iter().zip(&name_idx) {
            let (ci, _, _) = header
                .admin
                .get(a as usize)
                .ok_or_else(|| GeoError::Format("admin index".into()))?;
            if *ci >= header.cc.len() || nm as usize >= header.names.len() {
                return Err(GeoError::Format("index out of range".into()));
            }
        }

        let cells = (ROWS * COLS) as usize;
        let mut cell_start = vec![0u32; cells + 1];
        let mut cell_of_item = vec![0u32; n];
        for i in 0..n {
            let (r, c) = cell_of(lat[i], lon[i]);
            let k = (r * COLS + c) as usize;
            cell_of_item[i] = k as u32;
            cell_start[k + 1] += 1;
        }
        for k in 0..cells {
            cell_start[k + 1] += cell_start[k];
        }
        let mut fill = cell_start[..cells].to_vec();
        let mut cell_items = vec![0u32; n];
        for (i, &k) in cell_of_item.iter().enumerate() {
            let k = k as usize;
            cell_items[fill[k] as usize] = i as u32;
            fill[k] += 1;
        }
        let mut admin_by_key = HashMap::new();
        for (ci, code, name) in &header.admin {
            if let Some(cc) = header.cc.get(*ci) {
                admin_by_key.insert(format!("{cc}-{code}"), name.clone());
            }
        }
        Ok(GeoIndex {
            header,
            lat,
            lon,
            name_idx,
            admin_idx,
            cell_start,
            cell_items,
            admin_by_key,
        })
    }

    /// number of places
    pub fn size(&self) -> usize {
        self.header.n
    }

    /// GeoNames attribution text
    pub fn attribution(&self) -> &str {
        &self.header.attribution
    }

    /// place for an id returned by [`GeoIndex::nearest`]
    pub fn place(&self, id: usize) -> Option<PlaceInfo> {
        if id >= self.header.n {
            return None;
        }
        let (ci, code, name) = &self.header.admin[self.admin_idx[id] as usize];
        let cc = self.header.cc[*ci].clone();
        let key = format!("{cc}-{}", if code.is_empty() { "00" } else { code });
        Some(PlaceInfo {
            city: self.header.names[self.name_idx[id] as usize].clone(),
            admin1: if name.is_empty() {
                None
            } else {
                Some(name.clone())
            },
            cc,
            key,
        })
    }

    /// admin1 display name for `${cc}-${code}` (or None)
    pub fn admin1_name(&self, cc: &str, code: Option<&str>) -> Option<String> {
        self.admin_by_key
            .get(&format!("{cc}-{}", code.unwrap_or("")))
            .filter(|s| !s.is_empty())
            .cloned()
    }

    /// nearest populated place within `max_km` (default [`GEO_MAX_KM`]); (0,0) and invalid coordinates → None
    pub fn nearest(&self, q_lat: f64, q_lon: f64, max_km: Option<f64>) -> Option<GeoMatch> {
        let max_km = max_km.unwrap_or(GEO_MAX_KM);
        if !is_valid_gps(q_lat, q_lon) {
            return None;
        }
        let cos = (q_lat * RAD).cos().max(0.01);
        let (r0, c0) = cell_of(q_lat, q_lon);
        let cell_km = CELL * 111.2 * cos.min(1.0);
        let max_ring = ((COLS / 2) as f64).min((max_km / cell_km).ceil() + 1.0);
        let mut best: i64 = -1;
        let mut best_km = max_km;
        let visit = |r: i64, c: i64, best: &mut i64, best_km: &mut f64| {
            if !(0..ROWS).contains(&r) {
                return;
            }
            let k = (r * COLS + (((c % COLS) + COLS) % COLS)) as usize;
            for j in self.cell_start[k]..self.cell_start[k + 1] {
                let i = self.cell_items[j as usize] as usize;
                let mut d_lon = (self.lon[i] - q_lon).abs();
                if d_lon > 180.0 {
                    d_lon = 360.0 - d_lon;
                }
                let x = d_lon * cos;
                let y = self.lat[i] - q_lat;
                let km = (x * x + y * y).sqrt() * RAD * R;
                if km < *best_km {
                    *best_km = km;
                    *best = i as i64;
                }
            }
        };
        let mut ring: i64 = 0;
        while (ring as f64) <= max_ring {
            if best >= 0 && (ring - 1) as f64 * cell_km > best_km {
                break;
            }
            if ring == 0 {
                visit(r0, c0, &mut best, &mut best_km);
            } else {
                for dc in -ring..=ring {
                    visit(r0 - ring, c0 + dc, &mut best, &mut best_km);
                    visit(r0 + ring, c0 + dc, &mut best, &mut best_km);
                }
                for dr in (-ring + 1)..=(ring - 1) {
                    visit(r0 + dr, c0 - ring, &mut best, &mut best_km);
                    visit(r0 + dr, c0 + ring, &mut best, &mut best_km);
                }
            }
            ring += 1;
        }
        if best < 0 {
            return None;
        }
        let b = best as usize;
        let p = self.place(b)?;
        let code = &self.header.admin[self.admin_idx[b] as usize].1;
        Some(GeoMatch {
            city: p.city,
            admin1: p.admin1,
            cc: p.cc,
            key: p.key,
            id: b,
            admin1_code: if !code.is_empty() && code != "00" {
                Some(code.clone())
            } else {
                None
            },
            lat: self.lat[b],
            lon: self.lon[b],
            km: best_km,
        })
    }
}

/// One GPS coordinate value as EXIF / XMP deliver it.
#[derive(Debug, Clone, PartialEq)]
pub enum GpsValue<'a> {
    /// a number
    Num(f64),
    /// `[deg, min, sec]` (a JS array; missing parts = 0)
    Parts(&'a [f64]),
    /// a string in any of the forms below
    Text(&'a str),
    /// anything else (typed arrays, objects, missing) → None
    Other,
}

/// One GPS coordinate from EXIF / XMP in any of the forms cameras and editors write:
/// numbers, \[deg, min, sec\] arrays, "41.0123", "41,0.738N" (XMP DD,MM.mmmk), "41,0,44.28N" (DD,MM,SSk),
/// "41 0 44.28", "41°0'44.28\"N", "N 41.0123". `hemi_ref` ('N'/'S'/'E'/'W') applies when the value has none.
pub fn parse_gps_coordinate(v: GpsValue<'_>, hemi_ref: Option<&str>) -> Option<f64> {
    let mut sign = 1.0;
    let ref_of = |r: Option<&str>| -> Option<char> {
        r.and_then(|r| js::trim(r).chars().next()).map(upper1)
    };
    let mut hemi = ref_of(hemi_ref);
    let val: f64 = match v {
        GpsValue::Num(n) => n,
        GpsValue::Parts(p) => {
            if p.is_empty() || !p.iter().all(|x| x.is_finite()) {
                return None;
            }
            p[0] + p.get(1).copied().unwrap_or(0.0) / 60.0
                + p.get(2).copied().unwrap_or(0.0) / 3600.0
        }
        GpsValue::Text(t) => {
            let mut s = js::trim(t);
            if s.is_empty() || js::len16(s) > 64 {
                return None;
            }
            // leading /^([NSEW])\s*/i
            if let Some(c) = s.chars().next().filter(|c| "NSEWnsew".contains(*c)) {
                hemi = Some(c.to_ascii_uppercase());
                s = s[1..].trim_start_matches(js::is_space);
            }
            // trailing /\s*([NSEW])$/i
            if let Some(c) = s.chars().last().filter(|c| "NSEWnsew".contains(*c)) {
                hemi = Some(c.to_ascii_uppercase());
                s = s[..s.len() - 1].trim_end_matches(js::is_space);
            }
            if let Some(r) = s.strip_prefix('-') {
                sign = -1.0;
                s = r;
            } else if let Some(r) = s.strip_prefix('+') {
                s = r;
            }
            let is_sep = |c: char| c == ',' || js::is_space(c) || "°'′\"″:".contains(c);
            let parts: Vec<&str> = s.split(is_sep).filter(|x| !x.is_empty()).collect();
            let num_ok = |x: &str| {
                let mut it = x.splitn(2, '.');
                let a = it.next().unwrap_or("");
                let b = it.next();
                !a.is_empty()
                    && a.bytes().all(|c| c.is_ascii_digit())
                    && b.is_none_or(|b| !b.is_empty() && b.bytes().all(|c| c.is_ascii_digit()))
            };
            if parts.is_empty() || parts.len() > 3 || !parts.iter().all(|x| num_ok(x)) {
                return None;
            }
            let n: Vec<f64> = parts.iter().map(|x| js::number(x)).collect();
            if n.len() > 1 && (n[1] >= 60.0 || n.get(2).copied().unwrap_or(0.0) >= 60.0) {
                return None;
            }
            n[0] + n.get(1).copied().unwrap_or(0.0) / 60.0
                + n.get(2).copied().unwrap_or(0.0) / 3600.0
        }
        GpsValue::Other => return None,
    };
    if !val.is_finite() {
        return None;
    }
    let abs = val.abs();
    if hemi == Some('S') || hemi == Some('W') {
        return Some(-abs);
    }
    Some(if val < 0.0 { -abs } else { sign * abs })
}

fn upper1(c: char) -> char {
    // `charAt(0).toUpperCase()` — only the hemisphere letters matter
    c.to_uppercase().next().unwrap_or(c)
}
