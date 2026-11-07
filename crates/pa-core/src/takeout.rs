//! Google Takeout (Google Photos export) sidecars (mirrors `takeout.ts`).
//!
//! Names Google produces: `IMG_1234.jpg.json`, `IMG_1234.jpg.supplemental-metadata.json`, cut variants
//! (whole name ≤ 51 chars), `IMG_1234(1).jpg` ↔ `IMG_1234.jpg(1).json`, `-edited` copies → the original's JSON.

use serde::{Deserialize, Serialize};

use crate::js;

/// longest sidecar name Google writes, ".json" included
pub const TAKEOUT_MAX_NAME: usize = 51;
const SUPPLEMENTAL: &str = "supplemental-metadata";
const EDITED: &[&str] = &[
    "-edited",
    "-bearbeitet",
    "-modifié",
    "-modifie",
    "-düzenlendi",
    "-duzenlendi",
    "-editado",
    "-modificato",
    "-bewerkt",
    "-redigerad",
    "-edytowane",
    "-編集済み",
    "-已编辑",
    "-편집됨",
];

/// What a sidecar file name points at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarKey {
    /// media name the sidecar describes (possibly cut)
    pub target: String,
    /// duplicate index from "(n)", 0 = none
    pub dup: u32,
    /// the sidecar name was cut (prefix matching allowed)
    pub truncated: bool,
}

/// trailing `(\d{1,3})` → (index, byte length of the match)
fn dup_suffix(s: &str) -> Option<(u32, usize)> {
    let b = s.as_bytes();
    if b.last() != Some(&b')') {
        return None;
    }
    let mut i = b.len() - 1;
    let end = i;
    while i > 0 && b[i - 1].is_ascii_digit() && end - (i - 1) <= 3 {
        i -= 1;
    }
    let digits = end - i;
    if digits == 0 || i == 0 || b[i - 1] != b'(' {
        return None;
    }
    let n: u32 = s[i..end].parse().ok()?;
    Some((n, digits + 2))
}

/// what a sidecar file name points at; None when it is not a sidecar-looking name
pub fn parse_sidecar_name(json_name: &str) -> Option<SidecarKey> {
    if json_name.len() < 5 || !json_name.as_bytes()[json_name.len() - 5..].eq_ignore_ascii_case(b".json") {
        return None;
    }
    let mut key = &json_name[..json_name.len() - 5];
    let mut dup = 0;
    let mut dm_len = 0;
    if let Some((n, l)) = dup_suffix(key) {
        dup = n;
        dm_len = l;
        key = &key[..key.len() - l];
    }
    let truncated = js::len16(json_name) - dm_len >= TAKEOUT_MAX_NAME - 5;
    if let Some(dot) = key.rfind('.').filter(|d| *d > 0) {
        let tail = key[dot + 1..].to_lowercase();
        if !tail.is_empty() && SUPPLEMENTAL.starts_with(&tail) {
            key = &key[..dot];
        }
    }
    if key.is_empty() {
        return None;
    }
    Some(SidecarKey {
        target: key.to_string(),
        dup,
        truncated,
    })
}

fn stem_of(n: &str) -> &str {
    match n.rfind('.') {
        Some(d) if d > 0 => &n[..d],
        _ => n,
    }
}

/// Base names of a media file (without "(n)", plus edited-suffix variants).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameVariants {
    /// candidate names
    pub names: Vec<String>,
    /// duplicate index
    pub dup: u32,
}

/// media name → base name without "(n)" + its duplicate index + edited-suffix variants
pub fn media_name_variants(name: &str) -> NameVariants {
    let (mut stem, ext) = match name.rfind('.') {
        Some(d) if d > 0 => (&name[..d], &name[d..]),
        _ => (name, ""),
    };
    let mut dup = 0;
    if let Some((n, l)) = dup_suffix(stem) {
        dup = n;
        stem = &stem[..stem.len() - l];
    }
    let mut names = vec![format!("{stem}{ext}")];
    let lower = stem.to_lowercase();
    for e in EDITED {
        if lower.ends_with(e) {
            let cut = js::len16(stem).saturating_sub(js::len16(e));
            names.push(format!("{}{ext}", js::slice16_to(stem, cut)));
            break;
        }
    }
    NameVariants { names, dup }
}

/// Pick the sidecar for `media_name` from the JSON file names of its directory.
pub fn match_sidecar<'a, I: IntoIterator<Item = &'a str>>(
    media_name: &str,
    json_names: I,
) -> Option<&'a str> {
    let NameVariants { names, dup } = media_name_variants(media_name);
    let mut best = None;
    let mut best_score = -1.0;
    for j in json_names {
        let Some(k) = parse_sidecar_name(j) else {
            continue;
        };
        if k.dup != dup {
            continue;
        }
        for (vi, n) in names.iter().enumerate() {
            let vi = vi as f64;
            let score = if k.target == *n {
                100.0 - vi
            } else if k.target.to_lowercase() == n.to_lowercase() {
                90.0 - vi
            } else if k.truncated && js::len16(&k.target) >= 8 && n.starts_with(&k.target) {
                50.0 + (js::len16(&k.target).min(40) as f64) / 10.0 - vi
            } else if stem_of(&k.target).to_lowercase() == stem_of(n).to_lowercase()
                && stem_of(n) != n
            {
                20.0 - vi
            } else {
                -1.0
            };
            if score > best_score {
                best_score = score;
                best = Some(j);
            }
        }
    }
    best
}

/// Fields of a Takeout JSON.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeoutMeta {
    /// capture instant (UTC ms)
    pub taken_at: Option<i64>,
    /// latitude
    #[serde(with = "js::num_opt")]
    pub lat: Option<f64>,
    /// longitude
    #[serde(with = "js::num_opt")]
    pub lon: Option<f64>,
}

fn geo_of(g: Option<&serde_json::Value>) -> Option<(f64, f64)> {
    let o = g?;
    // arrays are objects in JS but have no latitude → NaN → null
    let obj = match o {
        serde_json::Value::Object(m) => Some(m),
        serde_json::Value::Array(_) => None,
        _ => return None,
    };
    let num = |k: &str| {
        obj.and_then(|m| m.get(k))
            .map(js::number_of)
            .unwrap_or(f64::NAN)
    };
    let (lat, lon) = (num("latitude"), num("longitude"));
    if !lat.is_finite() || !lon.is_finite() || lat.abs() > 90.0 || lon.abs() > 180.0 {
        return None;
    }
    if lat.abs() < 1e-4 && lon.abs() < 1e-4 {
        return None;
    }
    Some((lat, lon))
}

/// fields of a parsed Takeout JSON (geoData = location shown in Google Photos, geoDataExif = camera's)
pub fn parse_takeout_json(j: &serde_json::Value) -> Option<TakeoutMeta> {
    let o = match j {
        serde_json::Value::Object(m) => m,
        _ => return None,
    };
    let ts = |v: Option<&serde_json::Value>| -> Option<f64> {
        let n = match v {
            Some(serde_json::Value::Object(m)) => {
                m.get("timestamp").map(js::number_of).unwrap_or(f64::NAN)
            }
            _ => f64::NAN,
        };
        if n.is_finite() && n > 0.0 {
            Some(n * 1000.0)
        } else {
            None
        }
    };
    let taken_at = ts(o.get("photoTakenTime"));
    let geo = geo_of(o.get("geoData")).or_else(|| geo_of(o.get("geoDataExif")));
    if taken_at.is_none() && geo.is_none() {
        return None;
    }
    Some(TakeoutMeta {
        taken_at: taken_at.map(|t| t as i64),
        lat: geo.map(|g| js::round(g.0 * 1e6) / 1e6),
        lon: geo.map(|g| js::round(g.1 * 1e6) / 1e6),
    })
}
