//! TR + EN strings: the web UI's tables (`packages/web/src/i18n/*.ts`, ported to `i18n/*.json` by
//! `tools/gen-i18n.mjs`) plus the native-only `i18n/native.json`. Same keys, `{var}` placeholders
//! and "one|many" plurals as `translate()` in `i18n/index.ts`; a missing English string falls back
//! to Turkish, then to the key. Dates are formatted like the `Intl.DateTimeFormat` calls of the web UI.

use std::collections::HashMap;
use std::sync::OnceLock;

/// UI language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    /// Türkçe (the default)
    #[default]
    Tr,
    /// English
    En,
}

impl Lang {
    /// "tr" / "en"
    pub fn code(self) -> &'static str {
        match self {
            Lang::Tr => "tr",
            Lang::En => "en",
        }
    }
    /// from "tr" / "en" (anything else: Turkish)
    pub fn parse(s: &str) -> Lang {
        if s.eq_ignore_ascii_case("en")
            || s.to_ascii_lowercase().starts_with("en-")
            || s.to_ascii_lowercase().starts_with("en_")
        {
            Lang::En
        } else {
            Lang::Tr
        }
    }
}

struct Tables {
    tr: HashMap<String, String>,
    en: HashMap<String, String>,
}

fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let mut tr: HashMap<String, String> =
            serde_json::from_str(include_str!("../i18n/tr.json")).expect("i18n/tr.json");
        let mut en: HashMap<String, String> =
            serde_json::from_str(include_str!("../i18n/en.json")).expect("i18n/en.json");
        let native: HashMap<String, HashMap<String, String>> =
            serde_json::from_str(include_str!("../i18n/native.json")).expect("i18n/native.json");
        tr.extend(native.get("tr").cloned().unwrap_or_default());
        en.extend(native.get("en").cloned().unwrap_or_default());
        Tables { tr, en }
    })
}

/// Raw lookup (fallback en → tr → key).
pub fn t(lang: Lang, key: &str) -> String {
    let tb = tables();
    let s = match lang {
        Lang::En => tb.en.get(key).or_else(|| tb.tr.get(key)),
        Lang::Tr => tb.tr.get(key),
    };
    s.cloned().unwrap_or_else(|| key.to_string())
}

/// Lookup + `{name}` substitution; "one|many" picks by the numeric value of `n`.
pub fn tv(lang: Lang, key: &str, vars: &[(&str, &str)]) -> String {
    let mut s = t(lang, key);
    if s.contains('|') {
        if let Some((_, n)) = vars.iter().find(|(k, _)| *k == "n") {
            let num: String = n
                .chars()
                .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
                .collect();
            let (one, many) = s
                .split_once('|')
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .unwrap();
            s = if num.parse::<f64>().ok() == Some(1.0) {
                one
            } else {
                many
            };
        }
    }
    for (k, v) in vars {
        s = s.replace(&format!("{{{k}}}"), v);
    }
    s
}

/// Does the table have this key (tests: every key the .slint files use must exist)?
pub fn has_key(key: &str) -> bool {
    tables().tr.contains_key(key)
}

/// Group thousands like `toLocaleString` ("12.345" in Turkish, "12,345" in English).
pub fn format_count(lang: Lang, n: i64) -> String {
    let sep = if lang == Lang::Tr { '.' } else { ',' };
    let s = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(sep);
        }
        out.push(c);
    }
    if n < 0 {
        out.insert(0, '-');
    }
    out
}

/// `formatDuration`: "0:30", "1:02:03"
pub fn format_duration(s: Option<f64>) -> String {
    let Some(s) = s.filter(|s| s.is_finite()) else {
        return String::new();
    };
    let t = (s.max(0.0) + 0.5).floor() as i64;
    let (h, m, sec) = (t / 3600, (t % 3600) / 60, t % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

const MONTHS_SHORT: [[&str; 12]; 2] = [
    [
        "Oca", "Şub", "Mar", "Nis", "May", "Haz", "Tem", "Ağu", "Eyl", "Eki", "Kas", "Ara",
    ],
    [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ],
];
const MONTHS_LONG: [[&str; 12]; 2] = [
    [
        "Ocak", "Şubat", "Mart", "Nisan", "Mayıs", "Haziran", "Temmuz", "Ağustos", "Eylül", "Ekim",
        "Kasım", "Aralık",
    ],
    [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ],
];
/// Sunday first
const WEEKDAYS_SHORT: [[&str; 7]; 2] = [
    ["Paz", "Pzt", "Sal", "Çar", "Per", "Cum", "Cmt"],
    ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"],
];

fn li(lang: Lang) -> usize {
    match lang {
        Lang::Tr => 0,
        Lang::En => 1,
    }
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Civil date of a day number (inverse of [`days_from_civil`]).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn parse_key(key: &str) -> Option<(i64, u32, u32)> {
    let mut it = key.split('-');
    let y = it.next()?.parse().ok()?;
    let m = it.next().map(|s| s.parse().unwrap_or(1)).unwrap_or(1);
    let d = it.next().map(|s| s.parse().unwrap_or(1)).unwrap_or(1);
    Some((y, m.clamp(1, 12), d.clamp(1, 31)))
}

/// `formatDayKey`: 'YYYY-MM-DD' → "5 Kas Çar" / "Wed, Nov 5" (+ year when not `current_year`).
pub fn format_day_key(key: &str, lang: Lang, current_year: i64) -> String {
    let Some((y, m, d)) = parse_key(key) else {
        return key.to_string();
    };
    let wd = (days_from_civil(y, m, d) + 4).rem_euclid(7) as usize; // 1970-01-01 was a Thursday
    let l = li(lang);
    let (mon, day) = (MONTHS_SHORT[l][m as usize - 1], WEEKDAYS_SHORT[l][wd]);
    match (lang, y == current_year) {
        (Lang::Tr, true) => format!("{d} {mon} {day}"),
        (Lang::Tr, false) => format!("{d} {mon} {y} {day}"),
        (Lang::En, true) => format!("{day}, {mon} {d}"),
        (Lang::En, false) => format!("{day}, {mon} {d}, {y}"),
    }
}

/// `formatMonthKey`: 'YYYY-MM' → "Temmuz 2024" / "July 2024"
pub fn format_month_key(key: &str, lang: Lang) -> String {
    let Some((y, m, _)) = parse_key(key) else {
        return key.to_string();
    };
    format!("{} {y}", MONTHS_LONG[li(lang)][m as usize - 1])
}

/// Clock preference (Settings → Language & format).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HourCycle {
    /// by language
    Auto,
    /// 24 h
    H23,
    /// 12 h
    H12,
}

/// `{ dateStyle: 'medium', timeStyle: 'short' }` of a wall-clock instant ("5 Tem 2024 14:30",
/// "Jul 5, 2024, 2:30 PM", ISO "2024-07-05 14:30").
pub fn format_date_time(local_ms: i64, lang: Lang, iso: bool, hc: HourCycle) -> String {
    let days = local_ms.div_euclid(86_400_000);
    let mins = local_ms.rem_euclid(86_400_000) / 60_000;
    let (y, m, d) = civil_from_days(days);
    let time = format_time(mins, lang, hc, iso);
    if iso {
        return format!("{y}-{m:02}-{d:02} {time}");
    }
    let mon = MONTHS_SHORT[li(lang)][m as usize - 1];
    match lang {
        Lang::Tr => format!("{d} {mon} {y} {time}"),
        Lang::En => format!("{mon} {d}, {y}, {time}"),
    }
}

/// `{ timeStyle: 'short' }` of minutes since midnight.
pub fn format_time(mins: i64, lang: Lang, hc: HourCycle, iso: bool) -> String {
    let (h, mi) = (mins / 60, mins % 60);
    let h12 = match hc {
        HourCycle::H12 => true,
        HourCycle::H23 => false,
        HourCycle::Auto => lang == Lang::En && !iso,
    };
    if !h12 {
        return format!("{h:02}:{mi:02}");
    }
    let hh = (h + 11) % 12 + 1;
    match (lang, iso) {
        // tr-TR with hourCycle h12: "ÖS 02:30"
        (Lang::Tr, false) => format!("{} {hh:02}:{mi:02}", if h < 12 { "ÖÖ" } else { "ÖS" }),
        _ => format!("{hh}:{mi:02} {}", if h < 12 { "AM" } else { "PM" }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookups() {
        assert_eq!(t(Lang::Tr, "nav2.photos"), "Fotoğraflar");
        assert_eq!(t(Lang::En, "nav2.photos"), "Photos");
        assert_eq!(t(Lang::En, "no.such.key"), "no.such.key");
        assert_eq!(tv(Lang::En, "common.items", &[("n", "1")]), "1 item");
        assert_eq!(
            tv(Lang::En, "common.items", &[("n", "12,345")]),
            "12,345 items"
        );
        assert_eq!(
            tv(Lang::En, "home.stats", &[("photos", "91"), ("videos", "4")]),
            "91 photos · 4 videos"
        );
        assert_eq!(t(Lang::Tr, "native.soon.title"), "Yakında burada");
    }

    #[test]
    fn dates_match_intl() {
        // values from Node's Intl
        assert_eq!(format_day_key("2025-11-05", Lang::Tr, 2025), "5 Kas Çar");
        assert_eq!(format_day_key("2025-11-05", Lang::En, 2025), "Wed, Nov 5");
        assert_eq!(
            format_day_key("2025-11-05", Lang::Tr, 2026),
            "5 Kas 2025 Çar"
        );
        assert_eq!(
            format_day_key("2025-11-05", Lang::En, 2026),
            "Wed, Nov 5, 2025"
        );
        assert_eq!(format_month_key("2025-11", Lang::Tr), "Kasım 2025");
        assert_eq!(format_month_key("2024-07-05", Lang::En), "July 2024");
        let ms = days_from_civil(2025, 11, 5) * 86_400_000 + (14 * 60 + 30) * 60_000;
        assert_eq!(
            format_date_time(ms, Lang::Tr, false, HourCycle::Auto),
            "5 Kas 2025 14:30"
        );
        assert_eq!(
            format_date_time(ms, Lang::En, false, HourCycle::Auto),
            "Nov 5, 2025, 2:30 PM"
        );
        assert_eq!(
            format_date_time(ms, Lang::En, true, HourCycle::Auto),
            "2025-11-05 14:30"
        );
        assert_eq!(
            format_time(14 * 60 + 30, Lang::Tr, HourCycle::H12, false),
            "ÖS 02:30"
        );
        assert_eq!(
            format_time(14 * 60 + 30, Lang::En, HourCycle::H23, false),
            "14:30"
        );
        for z in [-800_000i64, -1, 0, 1, 19_000, 20_400, 2_000_000] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
    }

    #[test]
    fn numbers() {
        assert_eq!(format_count(Lang::Tr, 12345), "12.345");
        assert_eq!(format_count(Lang::En, 1234567), "1,234,567");
        assert_eq!(format_count(Lang::En, 999), "999");
        assert_eq!(format_duration(Some(30.04)), "0:30");
        assert_eq!(format_duration(Some(3723.0)), "1:02:03");
        assert_eq!(format_duration(None), "");
    }

    /// every `I18n.t("key"…)` used by the .slint files exists in the tables
    #[test]
    fn slint_keys_exist() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
        let re = regex_lite::Regex::new(r#"I18n\.t[12]?\("([A-Za-z0-9_.]+)""#).unwrap();
        let mut missing = vec![];
        let mut stack = vec![dir];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "slint") {
                    let src = std::fs::read_to_string(&p).unwrap();
                    for c in re.captures_iter(&src) {
                        if !has_key(&c[1]) {
                            missing.push(format!("{}: {}", p.display(), &c[1]));
                        }
                    }
                }
            }
        }
        assert!(missing.is_empty(), "missing i18n keys: {missing:#?}");
    }
}
