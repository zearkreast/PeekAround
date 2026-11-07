//! Capture-time helpers (mirrors `time.ts`).
//!
//! Two clocks are kept for every item:
//!  - instant (`takenAt`): true UTC ms epoch
//!  - local (`localAt`): wall-clock time at the place of capture, encoded as if it were UTC.
//!
//! Offsets are minutes EAST of UTC (Istanbul = +180).

use jiff::tz::TimeZone;
use serde::{Deserialize, Serialize};

use crate::js;

const MIN: i64 = 60_000;
/// Milliseconds per day.
pub const DAY_MS: i64 = js::DAY_MS;

/// Wall clock + optional trailing offset parsed from a date-time string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalWithOffset {
    /// wall clock as UTC ms
    pub local: i64,
    /// minutes east of UTC
    pub offset: Option<i32>,
}

/// Parse an EXIF / ISO-like date-time string into wall-clock ms (as UTC).
/// Accepts "2024:07:01 10:00:00", "2024-07-01T10:00:00.123", "2024:07:01 10:00:00+03:00" (offset ignored)
/// and date-only "2024:07:01".
pub fn parse_exif_date_time(s: &str) -> Option<i64> {
    parse_date_time_with_offset(s).map(|r| r.local)
}

fn digits(b: &[u8], i: &mut usize, min: usize, max: usize) -> Option<i64> {
    let start = *i;
    while *i < b.len() && *i - start < max && b[*i].is_ascii_digit() {
        *i += 1;
    }
    if *i - start < min {
        return None;
    }
    std::str::from_utf8(&b[start..*i]).ok()?.parse().ok()
}

/// Like [`parse_exif_date_time`] but also returns a trailing offset ("Z", "+03:00", "+0300") when present.
pub fn parse_date_time_with_offset(s: &str) -> Option<LocalWithOffset> {
    let str = js::trim(s);
    if js::len16(str) > 64 {
        return None;
    }
    // /^\s*(\d{4})[:\-/](\d{1,2})[:\-/](\d{1,2})(?:[ T](\d{1,2}):(\d{2})(?::(\d{2})(?:[.,](\d+))?)?)?\s*(Z|[+-]\d{2}:?\d{2})?\s*$/i
    let b = str.as_bytes();
    let mut i = 0;
    let sep = |c: u8| c == b':' || c == b'-' || c == b'/';
    let y = digits(b, &mut i, 4, 4)?;
    if i >= b.len() || !sep(b[i]) {
        return None;
    }
    i += 1;
    let mo = digits(b, &mut i, 1, 2)?;
    if i >= b.len() || !sep(b[i]) {
        return None;
    }
    i += 1;
    let d = digits(b, &mut i, 1, 2)?;
    let (mut h, mut mi, mut se, mut frac) = (0, 0, 0, None::<&[u8]>);
    // optional time part (the regex backtracks out of it when it does not match fully)
    if i < b.len() && (b[i] == b' ' || b[i] == b'T' || b[i] == b't') {
        let save = i;
        let mut j = i + 1;
        let time = (|| {
            let hh = digits(b, &mut j, 1, 2)?;
            if j >= b.len() || b[j] != b':' {
                return None;
            }
            j += 1;
            let mm = digits(b, &mut j, 2, 2)?;
            let mut ss = 0;
            let mut fr = None;
            if j < b.len() && b[j] == b':' {
                let mut k = j + 1;
                if let Some(v) = digits(b, &mut k, 2, 2) {
                    ss = v;
                    j = k;
                    if j < b.len() && (b[j] == b'.' || b[j] == b',') {
                        let fs = j + 1;
                        let mut k2 = fs;
                        while k2 < b.len() && b[k2].is_ascii_digit() {
                            k2 += 1;
                        }
                        if k2 > fs {
                            fr = Some(&b[fs..k2]);
                            j = k2;
                        }
                    }
                }
            }
            Some((hh, mm, ss, fr))
        })();
        match time {
            Some((hh, mm, ss, fr)) if tail_ok(b, j) => {
                h = hh;
                mi = mm;
                se = ss;
                frac = fr;
                i = j;
            }
            _ => i = save,
        }
    }
    let offset_str = tail(b, i)?;
    if !(1971..=2200).contains(&y) || !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    let ms = match frac {
        Some(f) => {
            let s = format!("0.{}", std::str::from_utf8(f).ok()?);
            js::round(s.parse::<f64>().ok()? * 1000.0) as i64
        }
        None => 0,
    };
    let local = js::date_utc(y, mo - 1, d, h, mi, se, ms);
    Some(LocalWithOffset {
        local,
        offset: offset_str.and_then(parse_offset),
    })
}

/// remainder after the date/time: `\s*(Z|[+-]\d{2}:?\d{2})?\s*$` → Some(offset text or None), None = no match
fn tail(b: &[u8], i: usize) -> Option<Option<&str>> {
    let s = std::str::from_utf8(&b[i..]).ok()?;
    let rest = s.trim_start_matches(js::is_space);
    let body = rest.trim_end_matches(js::is_space);
    if body.is_empty() {
        return Some(None);
    }
    let bb = body.as_bytes();
    let ok = if bb == b"Z" || bb == b"z" {
        true
    } else if (bb[0] == b'+' || bb[0] == b'-') && (bb.len() == 5 || bb.len() == 6) {
        let d = &bb[1..];
        if d.len() == 5 {
            d[0].is_ascii_digit()
                && d[1].is_ascii_digit()
                && d[2] == b':'
                && d[3].is_ascii_digit()
                && d[4].is_ascii_digit()
        } else {
            d.iter().all(u8::is_ascii_digit)
        }
    } else {
        false
    };
    if ok {
        Some(Some(body))
    } else {
        None
    }
}

fn tail_ok(b: &[u8], i: usize) -> bool {
    tail(b, i).is_some()
}

/// "+03:00" / "-0530" / "Z" → minutes east of UTC, or None.
pub fn parse_offset(s: &str) -> Option<i32> {
    let t = js::trim(s);
    if t == "z" || t == "Z" {
        return Some(0);
    }
    // /^([+-])(\d{1,2}):?(\d{2})?$/ (with the regex's backtracking over the hour digits)
    let b = t.as_bytes();
    if b.is_empty() || (b[0] != b'+' && b[0] != b'-') {
        return None;
    }
    let rest = &b[1..];
    let all_digits = |x: &[u8]| x.iter().all(u8::is_ascii_digit);
    let mut found = None;
    for hl in [2usize, 1] {
        if rest.len() < hl || !all_digits(&rest[..hl]) {
            continue;
        }
        let mut r = &rest[hl..];
        if r.first() == Some(&b':') {
            r = &r[1..];
        }
        if r.is_empty() || (r.len() == 2 && all_digits(r)) {
            let h: i64 = std::str::from_utf8(&rest[..hl]).ok()?.parse().ok()?;
            let m: i64 = if r.is_empty() {
                0
            } else {
                std::str::from_utf8(r).ok()?.parse().ok()?
            };
            found = Some((h, m));
            break;
        }
    }
    let (h, m) = found?;
    let v = (if b[0] == b'-' { -1 } else { 1 }) * (h * 60 + m);
    if v.abs() <= 14 * 60 {
        Some(v as i32)
    } else {
        None
    }
}

/// Offset string forms Intl accepts as a time zone ("+03:00", "-0530", "+03").
fn fixed_offset_zone(tz: &str) -> Option<i32> {
    let b = tz.as_bytes();
    if b.len() < 3 || (b[0] != b'+' && b[0] != b'-') || b.len() > 6 {
        return None;
    }
    let body = &tz[1..];
    let (h, m) = match body.len() {
        2 => (body, "00"),
        4 if !body.contains(':') => (&body[..2], &body[2..]),
        5 if body.as_bytes()[2] == b':' => (&body[..2], &body[3..]),
        _ => return None,
    };
    if !h.bytes().chain(m.bytes()).all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (h, m): (i32, i32) = (h.parse().ok()?, m.parse().ok()?);
    if h > 23 || m > 59 {
        return None;
    }
    Some(if b[0] == b'-' {
        -(h * 60 + m)
    } else {
        h * 60 + m
    })
}

enum Zone {
    Tz(TimeZone),
    Fixed(i32),
}

fn zone(time_zone: &str) -> Option<Zone> {
    if time_zone.is_empty() {
        return None;
    }
    if let Some(m) = fixed_offset_zone(time_zone) {
        return Some(Zone::Fixed(m));
    }
    jiff::tz::db().get(time_zone).ok().map(Zone::Tz)
}

/// true when `time_zone` is an IANA zone (or an offset zone like "+03:00") this runtime knows.
pub fn is_valid_time_zone(time_zone: &str) -> bool {
    zone(time_zone).is_some()
}

/// Offset (minutes east of UTC) of `time_zone` at a given instant — DST correct.
/// Unknown zones → 0 (like the TypeScript version).
pub fn tz_offset_at(instant: i64, time_zone: &str) -> i32 {
    if time_zone == "UTC" {
        return 0;
    }
    let Some(z) = zone(time_zone) else { return 0 };
    let secs = instant.div_euclid(1000);
    let off_secs = match z {
        Zone::Fixed(m) => return m,
        Zone::Tz(tz) => match jiff::Timestamp::from_second(secs) {
            Ok(ts) => tz.to_offset(ts).seconds() as i64,
            Err(_) => return 0,
        },
    };
    // Intl reports the wall clock at second precision: (secs + off) - secs = off seconds
    js::round(off_secs as f64 * 1000.0 / MIN as f64) as i32
}

/// Instant → wall clock in `time_zone`.
pub fn instant_to_local(instant: i64, time_zone: &str) -> i64 {
    instant + tz_offset_at(instant, time_zone) as i64 * MIN
}

/// Wall clock in `time_zone` → instant (DST correct; ambiguous/non-existent times pick a neighbour).
pub fn local_to_instant(local: i64, time_zone: &str) -> i64 {
    const HALF_DAY: i64 = 12 * 3_600_000;
    let t_a = local - tz_offset_at(local - HALF_DAY, time_zone) as i64 * MIN;
    let t_b = local - tz_offset_at(local + HALF_DAY, time_zone) as i64 * MIN;
    if t_a == t_b {
        return t_a;
    }
    let ok_a = instant_to_local(t_a, time_zone) == local;
    let ok_b = instant_to_local(t_b, time_zone) == local;
    if ok_a && ok_b {
        return t_a.min(t_b);
    }
    if ok_b {
        return t_b;
    }
    t_a
}

/// Offset implied by the camera's local time and the GPS (UTC) time of the same shot,
/// rounded to 15 minutes; None when implausible (> ±14h).
pub fn offset_from_gps(local: i64, gps_utc: i64) -> Option<i32> {
    let diff = js::round((local - gps_utc) as f64 / (15 * MIN) as f64) as i64 * 15;
    if diff.abs() <= 14 * 60 {
        Some(diff as i32)
    } else {
        None
    }
}

/// GPS time stamp as EXIF / XMP deliver it.
#[derive(Debug, Clone, PartialEq)]
pub enum GpsTime<'a> {
    /// `[h, m, s]` (missing parts = 0; non-finite parts make the result None)
    Parts(&'a [f64]),
    /// "10:00:00" / "10:00:00.5"
    Text(&'a str),
}

/// EXIF GPSDateStamp ("2024:07:01") + GPSTimeStamp ([h, m, s] or "10:00:00") → UTC ms.
pub fn parse_gps_date_time(date: &str, time: GpsTime<'_>) -> Option<i64> {
    // /^(\d{4})[:\-](\d{2})[:\-](\d{2})/
    let b = js::trim(date).as_bytes();
    if b.len() < 10
        || !b[..4].iter().all(u8::is_ascii_digit)
        || !(b[4] == b':' || b[4] == b'-')
        || !b[5..7].iter().all(u8::is_ascii_digit)
        || !(b[7] == b':' || b[7] == b'-')
        || !b[8..10].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> i64 {
        std::str::from_utf8(&b[r])
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    };
    let (y, mo, d) = (num(0..4), num(5..7), num(8..10));
    let (h, m, s): (f64, f64, f64) = match time {
        GpsTime::Parts(p) => (
            p.first().copied().unwrap_or(0.0),
            p.get(1).copied().unwrap_or(0.0),
            p.get(2).copied().unwrap_or(0.0),
        ),
        GpsTime::Text(t) => {
            // /^(\d{1,2}):(\d{1,2}):(\d{1,2}(?:\.\d+)?)/
            let tb = js::trim(t).as_bytes();
            let mut i = 0;
            let hh = digits(tb, &mut i, 1, 2)?;
            if i >= tb.len() || tb[i] != b':' {
                return None;
            }
            i += 1;
            let mm = digits(tb, &mut i, 1, 2)?;
            if i >= tb.len() || tb[i] != b':' {
                return None;
            }
            i += 1;
            let st = i;
            digits(tb, &mut i, 1, 2)?;
            if i + 1 < tb.len() && tb[i] == b'.' && tb[i + 1].is_ascii_digit() {
                i += 1;
                while i < tb.len() && tb[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let ss: f64 = std::str::from_utf8(&tb[st..i]).ok()?.parse().ok()?;
            (hh as f64, mm as f64, ss)
        }
    };
    if !(h.is_finite() && m.is_finite() && s.is_finite()) {
        return None;
    }
    let v = js::date_utc(y, mo - 1, d, 0, 0, 0, 0) as f64
        + js::round((h * 3600.0 + m * 60.0 + s) * 1000.0);
    if v.is_finite() && y > 1970 {
        Some(v as i64)
    } else {
        None
    }
}

/// Resolved capture time of one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedTime {
    /// true instant
    pub taken_at: i64,
    /// wall clock as UTC
    pub local_at: i64,
    /// minutes east of UTC when actually known, else None
    pub tz_offset: Option<i32>,
}

/// Wall clock known (EXIF); offset optional → fall back to the home zone for the instant.
pub fn resolve_from_local(local: i64, offset: Option<i32>, home_time_zone: &str) -> ResolvedTime {
    match offset {
        Some(o) => ResolvedTime {
            taken_at: local - o as i64 * MIN,
            local_at: local,
            tz_offset: Some(o),
        },
        None => ResolvedTime {
            taken_at: local_to_instant(local, home_time_zone),
            local_at: local,
            tz_offset: None,
        },
    }
}

/// Only an instant is known (video creation_time, mtime) → wall clock in the home zone.
pub fn resolve_from_instant(instant: i64, home_time_zone: &str) -> ResolvedTime {
    ResolvedTime {
        taken_at: instant,
        local_at: instant_to_local(instant, home_time_zone),
        tz_offset: None,
    }
}

/// 'YYYY-MM-DD' of a localAt value.
pub fn local_day_key(local_at: i64) -> String {
    js::iso_day(local_at)
}

/// Start of the (UTC-encoded) local day.
pub fn local_day_start(local_at: i64) -> i64 {
    local_at.div_euclid(DAY_MS) * DAY_MS
}
