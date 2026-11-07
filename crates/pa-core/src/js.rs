//! Small helpers that reproduce JavaScript semantics the TypeScript reference relies on
//! (`Number()`, `Math.round`, `Date.UTC`, number → string, UTF-16 lengths, `\s`, JSON numbers).

use serde::{Serialize, Serializer};

/// Milliseconds per day.
pub const DAY_MS: i64 = 86_400_000;

/// JavaScript `Math.round` (half-way cases round towards +∞).
pub fn round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let f = x.floor();
    if x - f >= 0.5 {
        f + 1.0
    } else {
        f
    }
}

/// JavaScript whitespace (`\s`, `String.prototype.trim`): WhiteSpace + LineTerminator.
pub fn is_space(c: char) -> bool {
    matches!(
        c,
        '\u{9}' | '\u{a}' | '\u{b}' | '\u{c}' | '\u{d}' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// JavaScript `String.prototype.trim`.
pub fn trim(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// Length in UTF-16 code units (`string.length`).
pub fn len16(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Byte index of UTF-16 offset `n` (clamped to the string; never splits a char).
fn byte_at16(s: &str, n: usize) -> usize {
    let mut units = 0;
    for (i, c) in s.char_indices() {
        if units >= n {
            return i;
        }
        units += c.len_utf16();
    }
    s.len()
}

/// `s.slice(0, n)` with UTF-16 offsets.
pub fn slice16_to(s: &str, n: usize) -> &str {
    &s[..byte_at16(s, n)]
}

/// `s.slice(n)` with UTF-16 offsets.
pub fn slice16_from(s: &str, n: usize) -> &str {
    &s[byte_at16(s, n)..]
}

/// JavaScript `<` on strings (UTF-16 code unit order).
pub fn cmp16(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// JavaScript `Number(string)`.
pub fn number(s: &str) -> f64 {
    let t = trim(s);
    if t.is_empty() {
        return 0.0;
    }
    let (neg, body) = match t.as_bytes()[0] {
        b'-' => (true, &t[1..]),
        b'+' => (false, &t[1..]),
        _ => (false, t),
    };
    let radix = |p: &str, r: u32| -> f64 {
        if p.is_empty() || !p.chars().all(|c| c.is_digit(r)) {
            return f64::NAN;
        }
        p.chars().fold(0.0, |acc, c| {
            acc * r as f64 + c.to_digit(r).unwrap_or(0) as f64
        })
    };
    // 0x / 0o / 0b prefixes are only valid without a sign
    if body.len() > 2 && body.as_bytes()[0] == b'0' && body.len() == t.len() {
        match body.as_bytes()[1] {
            b'x' | b'X' => return radix(&body[2..], 16),
            b'o' | b'O' => return radix(&body[2..], 8),
            b'b' | b'B' => return radix(&body[2..], 2),
            _ => {}
        }
    }
    let v = if body == "Infinity" {
        f64::INFINITY
    } else {
        // StrDecimalLiteral: digits [. digits] [e[+-]digits] | . digits [e…]
        let b = body.as_bytes();
        let mut i = 0;
        let mut digits = 0;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
        if i < b.len() && b[i] == b'.' {
            i += 1;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
                digits += 1;
            }
        }
        if digits == 0 {
            return f64::NAN;
        }
        if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
            let mut j = i + 1;
            if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                j += 1;
            }
            let start = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j == start {
                return f64::NAN;
            }
            i = j;
        }
        if i != b.len() {
            return f64::NAN;
        }
        let norm = body.strip_suffix('.').unwrap_or(body);
        norm.parse::<f64>().unwrap_or(f64::NAN)
    };
    if neg {
        -v
    } else {
        v
    }
}

/// JavaScript `Number(value)` for a JSON value.
pub fn number_of(v: &serde_json::Value) -> f64 {
    use serde_json::Value;
    match v {
        Value::Null => 0.0,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => number(s),
        Value::Array(_) => number(&to_string_of(v)),
        Value::Object(_) => f64::NAN,
    }
}

/// JavaScript `String(value)` for a JSON value.
pub fn to_string_of(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => num_to_string(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => s.clone(),
        Value::Array(a) => a
            .iter()
            .map(|x| {
                if x.is_null() {
                    String::new()
                } else {
                    to_string_of(x)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

/// JavaScript `Number.prototype.toString()` (shortest round-trip digits, JS exponent rules).
pub fn num_to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    if x == 0.0 {
        return "0".into();
    }
    let a = x.abs();
    if (1e-6..1e21).contains(&a) {
        // Rust's Display prints the shortest round-trip digits without an exponent
        return format!("{x}");
    }
    let e = format!("{x:e}"); // e.g. "1.5e-7", "1e21"
    match e.split_once('e') {
        Some((m, exp)) if !exp.starts_with('-') => format!("{m}e+{exp}"),
        _ => e,
    }
}

/// Serialize an `f64` like `JSON.stringify` does (integral values without ".0", non-finite → null).
pub fn ser_f64<S: Serializer>(x: f64, s: S) -> Result<S::Ok, S::Error> {
    if !x.is_finite() {
        return s.serialize_none();
    }
    if x == x.trunc() && x.abs() < 9_007_199_254_740_992.0 {
        return s.serialize_i64(x as i64);
    }
    s.serialize_f64(x)
}

/// `#[serde(with = "pa_core::js::num")]` for `f64` fields: JSON numbers formatted like JavaScript.
pub mod num {
    use serde::{Deserialize, Deserializer, Serializer};
    /// see [`super::ser_f64`]
    pub fn serialize<S: Serializer>(x: &f64, s: S) -> Result<S::Ok, S::Error> {
        super::ser_f64(*x, s)
    }
    /// plain `f64`
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
        f64::deserialize(d)
    }
}

/// `#[serde(with = "pa_core::js::num_opt")]` for `Option<f64>` fields.
pub mod num_opt {
    use serde::{Deserialize, Deserializer, Serializer};
    /// `None` → null, `Some(x)` → [`super::ser_f64`]
    pub fn serialize<S: Serializer>(x: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
        match x {
            Some(v) => super::ser_f64(*v, s),
            None => s.serialize_none(),
        }
    }
    /// plain `Option<f64>`
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
        Option::<f64>::deserialize(d)
    }
}

/// `#[serde(with = "pa_core::js::num_vec")]` for `Vec<f64>` fields.
pub mod num_vec {
    use serde::ser::SerializeSeq;
    use serde::{Deserialize, Deserializer, Serializer};
    /// every element through [`super::JsF64`]
    pub fn serialize<S: Serializer>(v: &[f64], s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(v.len()))?;
        for x in v {
            seq.serialize_element(&super::JsF64(*x))?;
        }
        seq.end()
    }
    /// plain `Vec<f64>`
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<f64>, D::Error> {
        Vec::<f64>::deserialize(d)
    }
}

/// An `f64` that serializes like JavaScript (see [`ser_f64`]).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct JsF64(pub f64);

impl Serialize for JsF64 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        ser_f64(self.0, s)
    }
}

impl<'de> serde::Deserialize<'de> for JsF64 {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        f64::deserialize(d).map(JsF64)
    }
}

/// `Option<Option<T>>` for TypeScript `x?: T | null`: missing → `None`, `null` → `Some(None)`.
/// Use with `#[serde(default, skip_serializing_if = "Option::is_none", with = "pa_core::js::double_option")]`.
pub mod double_option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    /// `Some(None)` → null, `Some(Some(v))` → v (`None` must be skipped by the field attribute)
    pub fn serialize<T: Serialize, S: Serializer>(
        v: &Option<Option<T>>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        match v {
            Some(inner) => inner.serialize(s),
            None => s.serialize_none(),
        }
    }
    /// a present field (null or value) → `Some(..)`
    pub fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(d).map(Some)
    }
}

// ───────────────────────── Dates (proleptic Gregorian, UTC) ─────────────────────────

/// days since 1970-01-01 of a civil date (month 1..12)
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// civil date (year, month 1..12, day) of days since 1970-01-01
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// JavaScript `Date.UTC(y, month0, d, h, mi, s, ms)` with the same overflow normalisation.
pub fn date_utc(y: i64, month0: i64, d: i64, h: i64, mi: i64, s: i64, ms: i64) -> i64 {
    let y = y + month0.div_euclid(12);
    let m = month0.rem_euclid(12) + 1;
    let days = days_from_civil(y, m, 1) + d - 1;
    days * DAY_MS + h * 3_600_000 + mi * 60_000 + s * 1000 + ms
}

/// UTC calendar fields of a ms epoch value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtcParts {
    /// full year
    pub year: i64,
    /// 0..11 (like `getUTCMonth`)
    pub month0: i64,
    /// 1..31
    pub day: i64,
    /// 0..23
    pub hour: i64,
    /// 0..59
    pub minute: i64,
    /// 0..59
    pub second: i64,
    /// 0..999
    pub ms: i64,
}

/// `getUTCFullYear()` … `getUTCMilliseconds()` of a ms epoch value.
pub fn utc_parts(t: i64) -> UtcParts {
    let days = t.div_euclid(DAY_MS);
    let rem = t.rem_euclid(DAY_MS);
    let (year, m, day) = civil_from_days(days);
    UtcParts {
        year,
        month0: m - 1,
        day,
        hour: rem / 3_600_000,
        minute: rem / 60_000 % 60,
        second: rem / 1000 % 60,
        ms: rem % 1000,
    }
}

/// `new Date(t).getUTCFullYear()`
pub fn utc_year(t: i64) -> i64 {
    utc_parts(t).year
}

/// `new Date(t).toISOString().slice(0, 10)`
pub fn iso_day(t: i64) -> String {
    let p = utc_parts(t);
    let y = if (0..=9999).contains(&p.year) {
        format!("{:04}", p.year)
    } else if p.year < 0 {
        format!("-{:06}", -p.year)
    } else {
        format!("+{:06}", p.year)
    };
    // toISOString().slice(0, 10) on extended years cuts inside the date — mirror that
    let full = format!("{y}-{:02}-{:02}", p.month0 + 1, p.day);
    full.chars().take(10).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(number(" 12 "), 12.0);
        assert_eq!(number(""), 0.0);
        assert_eq!(number("0x1A"), 26.0);
        assert!(number("-0x1A").is_nan());
        assert_eq!(number("1e3"), 1000.0);
        assert_eq!(number(".5"), 0.5);
        assert_eq!(number("5."), 5.0);
        assert!(number("1,5").is_nan());
        assert!(number("abc").is_nan());
        assert_eq!(number("-Infinity"), f64::NEG_INFINITY);
        assert_eq!(num_to_string(41.0), "41");
        assert_eq!(num_to_string(28.5), "28.5");
        assert_eq!(num_to_string(1e21), "1e+21");
        assert_eq!(num_to_string(1.5e-7), "1.5e-7");
        assert_eq!(num_to_string(0.000001), "0.000001");
        assert_eq!(num_to_string(-0.0), "0");
        assert_eq!(round(-0.5), 0.0);
        assert_eq!(round(2.5), 3.0);
        assert_eq!(round(0.49999999999999994), 0.0);
    }

    #[test]
    fn dates() {
        assert_eq!(date_utc(2024, 0, 1, 0, 0, 0, 0), 1_704_067_200_000);
        assert_eq!(
            date_utc(2024, 12, 1, 0, 0, 0, 0),
            date_utc(2025, 0, 1, 0, 0, 0, 0)
        );
        assert_eq!(
            date_utc(2023, 2, 0, 0, 0, 0, 0),
            date_utc(2023, 1, 28, 0, 0, 0, 0)
        );
        assert_eq!(iso_day(date_utc(2024, 1, 29, 23, 0, 0, 0)), "2024-02-29");
        let p = utc_parts(-1);
        assert_eq!(
            (p.year, p.month0, p.day, p.hour, p.ms),
            (1969, 11, 31, 23, 999)
        );
    }
}
