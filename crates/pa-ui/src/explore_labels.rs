//! Explore labels — the pure helpers of `features/explore/labels.ts` (country names, memory titles,
//! date ranges, bytes / numbers, routes → browse queries). Dates are localAt values (wall clock
//! encoded as UTC), formatted like `Intl.DateTimeFormat(…, { timeZone: 'UTC' })`.

use pa_core::{BrowseQuery, KindFilter, Memory, MemoryKind, SortDir, SortKey};

use crate::i18n::{self, civil_from_days, days_from_civil, Lang};

/// a day in ms
pub const DAY_MS: i64 = 86_400_000;

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
const WEEKDAYS_LONG: [[&str; 7]; 2] = [
    [
        "Pazar",
        "Pazartesi",
        "Salı",
        "Çarşamba",
        "Perşembe",
        "Cuma",
        "Cumartesi",
    ],
    [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ],
];

fn li(lang: Lang) -> usize {
    match lang {
        Lang::Tr => 0,
        Lang::En => 1,
    }
}

fn t(lang: Lang, key: &str) -> String {
    i18n::t(lang, key)
}

fn tv(lang: Lang, key: &str, vars: &[(&str, &str)]) -> String {
    i18n::tv(lang, key, vars)
}

/// (year, month 1-12, day) of a localAt
pub fn ymd(local_at: i64) -> (i64, u32, u32) {
    civil_from_days(local_at.div_euclid(DAY_MS))
}

/// localAt of a civil date (month 1-12; overflowing months roll into the next year)
pub fn utc(y: i64, m: i64, d: u32) -> i64 {
    let (y, m) = (y + (m - 1).div_euclid(12), (m - 1).rem_euclid(12) + 1);
    days_from_civil(y, m as u32, d) * DAY_MS
}

/// ISO 3166 code → "Türkiye" / "Germany" (the code itself when unknown)
pub fn country_name(cc: &str, lang: Lang) -> String {
    let up = cc.to_ascii_uppercase();
    match crate::explore_countries::COUNTRIES.binary_search_by(|r| r.0.cmp(up.as_str())) {
        Ok(i) => {
            let r = crate::explore_countries::COUNTRIES[i];
            match lang {
                Lang::Tr => r.1,
                Lang::En => r.2,
            }
            .to_string()
        }
        Err(_) => up,
    }
}

/// "Ağu 2024" / "August 2024"
#[cfg_attr(not(test), allow(dead_code))]
pub fn month_year(local_at: i64, lang: Lang, long: bool) -> String {
    let (y, m, _) = ymd(local_at);
    let names = if long { &MONTHS_LONG } else { &MONTHS_SHORT };
    format!("{} {y}", names[li(lang)][m as usize - 1])
}

/// short month name ("Oca" / "Jan"), m = 1..12
pub fn month_short(m: u32, lang: Lang) -> &'static str {
    MONTHS_SHORT[li(lang)][(m.clamp(1, 12) - 1) as usize]
}

/// `{ day, month: 'short', year }`: "5 Kas 2024" / "Nov 5, 2024"
pub fn date_short(local_at: i64, lang: Lang) -> String {
    let (y, m, d) = ymd(local_at);
    let mon = MONTHS_SHORT[li(lang)][m as usize - 1];
    match lang {
        Lang::Tr => format!("{d} {mon} {y}"),
        Lang::En => format!("{mon} {d}, {y}"),
    }
}

/// `{ day, month: 'short' }`: "5 Eyl" / "Sep 5"
pub fn day_month(local_at: i64, lang: Lang) -> String {
    let (_, m, d) = ymd(local_at);
    let mon = MONTHS_SHORT[li(lang)][m as usize - 1];
    match lang {
        Lang::Tr => format!("{d} {mon}"),
        Lang::En => format!("{mon} {d}"),
    }
}

/// `{ day, month: 'long', year [, weekday: 'long'] }`: "5 Mart 2024 Salı" / "Tuesday, March 5, 2024"
pub fn date_long(local_at: i64, lang: Lang, weekday: bool) -> String {
    let days = local_at.div_euclid(DAY_MS);
    let (y, m, d) = civil_from_days(days);
    let mon = MONTHS_LONG[li(lang)][m as usize - 1];
    let wd = WEEKDAYS_LONG[li(lang)][(days + 4).rem_euclid(7) as usize];
    match (lang, weekday) {
        (Lang::Tr, false) => format!("{d} {mon} {y}"),
        (Lang::Tr, true) => format!("{d} {mon} {y} {wd}"),
        (Lang::En, false) => format!("{mon} {d}, {y}"),
        (Lang::En, true) => format!("{wd}, {mon} {d}, {y}"),
    }
}

/// `dateRange`: [from, to) → "2–6 Tem 2024", "30 Haz – 2 Tem 2024", one day → "5 Kas 2024"
/// (Intl `formatRange` with `{ day, month: 'short', year }`)
pub fn date_range(from: i64, to: i64, lang: Lang) -> String {
    let last = from.max(to - 1);
    if from.div_euclid(DAY_MS) == last.div_euclid(DAY_MS) {
        return date_short(from, lang);
    }
    let ((y1, m1, d1), (y2, m2, d2)) = (ymd(from), ymd(last));
    let l = li(lang);
    let (a, b) = (
        MONTHS_SHORT[l][m1 as usize - 1],
        MONTHS_SHORT[l][m2 as usize - 1],
    );
    match lang {
        Lang::Tr => {
            if y1 != y2 {
                format!("{d1} {a} {y1} – {d2} {b} {y2}")
            } else if m1 != m2 {
                format!("{d1} {a} – {d2} {b} {y2}")
            } else {
                format!("{d1}–{d2} {b} {y2}")
            }
        }
        Lang::En => {
            if y1 != y2 {
                format!("{a} {d1}, {y1} – {b} {d2}, {y2}")
            } else if m1 != m2 {
                format!("{a} {d1} – {b} {d2}, {y2}")
            } else {
                format!("{a} {d1} – {d2}, {y2}")
            }
        }
    }
}

/// Title + subtitle of a memory card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryLabel {
    /// "2 yıl önce bugün" / "Antalya gezisi" / "2024 yılınız"
    pub title: String,
    /// "5 Kasım 2023" / "Türkiye · 2–6 Tem 2024"
    pub subtitle: String,
}

/// `memoryLabel`
pub fn memory_label(m: &Memory, lang: Lang) -> MemoryLabel {
    match m.kind {
        MemoryKind::Year => {
            let year = ymd(m.from).0;
            let months = m
                .months
                .as_ref()
                .map(|ms| ms.iter().filter(|n| **n > 0).count())
                .unwrap_or(0);
            let n = m.count.to_string();
            MemoryLabel {
                title: tv(
                    lang,
                    "insights.yearReviewTitle",
                    &[("year", &year.to_string())],
                ),
                subtitle: if m.count > 0 {
                    if months > 0 {
                        tv(
                            lang,
                            "insights.yearReviewSub",
                            &[("n", &n), ("months", &months.to_string())],
                        )
                    } else {
                        tv(lang, "explore.count", &[("n", &n)])
                    }
                } else {
                    year.to_string()
                },
            }
        }
        MemoryKind::Onthisday => {
            // the window is ±3 days around the anniversary → its middle day is the actual date
            let mid = m.from + ((m.to - m.from) / 2 / DAY_MS) * DAY_MS;
            MemoryLabel {
                title: tv(
                    lang,
                    "explore.memory.onthisday",
                    &[("n", &m.years_ago.unwrap_or(1).to_string())],
                ),
                subtitle: date_long(mid, lang, false),
            }
        }
        MemoryKind::Trip => {
            let title = match &m.place {
                Some(p) if !p.is_empty() => tv(lang, "explore.memory.trip", &[("place", p)]),
                _ => t(lang, "explore.memory.tripNoPlace"),
            };
            let country =
                m.cc.as_deref()
                    .filter(|c| !c.is_empty())
                    .map(|c| country_name(c, lang))
                    .unwrap_or_default();
            let range = date_range(m.from, m.to, lang);
            let mut parts = vec![];
            if !country.is_empty() && Some(country.as_str()) != m.place.as_deref() {
                parts.push(country);
            }
            parts.push(range);
            MemoryLabel {
                title,
                subtitle: parts.join(" · "),
            }
        }
    }
}

/// `memoryShort`: "1 yıl önce" / "Antalya · Ağu 2024" / "2024" (the timeline's memories strip)
#[cfg_attr(not(test), allow(dead_code))]
pub fn memory_short(m: &Memory, lang: Lang) -> String {
    match m.kind {
        MemoryKind::Onthisday => tv(
            lang,
            "explore.memory.onthisdayShort",
            &[("n", &m.years_ago.unwrap_or(1).to_string())],
        ),
        MemoryKind::Year => ymd(m.from).0.to_string(),
        MemoryKind::Trip => {
            let mut parts = vec![];
            if let Some(p) = m.place.as_deref().filter(|p| !p.is_empty()) {
                parts.push(p.to_string());
            }
            parts.push(month_year(m.from, lang, false));
            parts.join(" · ")
        }
    }
}

/// `memoryFromId`: a memory id no longer in the Explore list (deep link, the day changed)
pub fn memory_from_id(id: &str, now: i64) -> Option<Memory> {
    if let Some(y) = id.strip_prefix("year-") {
        let year: i64 = y.parse().ok().filter(|_| y.len() == 4)?;
        return Some(Memory {
            id: id.into(),
            kind: MemoryKind::Year,
            years_ago: None,
            place: None,
            cc: None,
            from: utc(year, 1, 1),
            to: utc(year + 1, 1, 1),
            count: 0,
            cover_ids: vec![],
            months: None,
        });
    }
    let key = id.strip_prefix("otd-")?;
    let day = day_range(key)?.0;
    let years_ago = (ymd(now).0 - key[..4].parse::<i64>().ok()?).max(1);
    Some(Memory {
        id: id.into(),
        kind: MemoryKind::Onthisday,
        years_ago: Some(years_ago),
        place: None,
        cc: None,
        from: day - 3 * DAY_MS,
        to: day + 4 * DAY_MS,
        count: 0,
        cover_ids: vec![],
        months: None,
    })
}

/// "2024-03-05" → localAt range of that day
pub fn day_range(key: &str) -> Option<(i64, i64)> {
    let b = key.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let y: i64 = key[..4].parse().ok()?;
    let m: u32 = key[5..7].parse().ok()?;
    let d: u32 = key[8..10].parse().ok()?;
    if !(1..=12).contains(&m) || d == 0 {
        return None;
    }
    let from = days_from_civil(y, m, d) * DAY_MS;
    // reject 2024-02-31 & co (Date.UTC would roll over)
    if ymd(from) != (y, m, d) {
        return None;
    }
    Some((from, from + DAY_MS))
}

/// 'YYYY-MM-DD' of a localAt (`localDayKey`)
pub fn day_key(local_at: i64) -> String {
    let (y, m, d) = ymd(local_at);
    format!("{y:04}-{m:02}-{d:02}")
}

/// "5 Mart 2024" (+ weekday) of a day key
pub fn day_label(key: &str, lang: Lang, weekday: bool) -> String {
    match day_range(key) {
        Some((from, _)) => date_long(from, lang, weekday),
        None => key.to_string(),
    }
}

/// thousands separator of the language
pub fn num_label(n: i64, lang: Lang) -> String {
    i18n::format_count(lang, n)
}

/// `bytesLabel`: "12,3 GB" / "12.3 GB" (no trailing ",0")
pub fn bytes_label(n: i64, lang: Lang) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n.max(0) as f64;
    let mut u = 0;
    while v >= 1024.0 && u < U.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    let s = if v >= 100.0 || u == 0 {
        num_label(v.round() as i64, lang)
    } else {
        let r = (v * 10.0).round() / 10.0;
        if r.fract() == 0.0 {
            num_label(r as i64, lang)
        } else {
            let s = format!("{r:.1}");
            if lang == Lang::Tr {
                s.replace('.', ",")
            } else {
                s
            }
        }
    };
    format!("{s} {}", U[u])
}

/// `cameraLabel`
pub fn camera_label(make: Option<&str>, model: Option<&str>) -> String {
    let md = model.unwrap_or("").trim();
    let mk = make.unwrap_or("").trim();
    if md.is_empty() {
        return mk.to_string();
    }
    let ml = md.to_lowercase();
    if mk.is_empty() || ml.starts_with(&mk.to_lowercase()) {
        return md.to_string();
    }
    if mk == "Apple" && (ml.starts_with("iphone") || ml.starts_with("ipad")) {
        return md.to_string();
    }
    format!("{mk} {md}")
}

/// `exploreQuery`: images + videos, newest first, all folders
pub fn explore_query() -> BrowseQuery {
    BrowseQuery {
        recursive: Some(true),
        sort: Some(SortKey::TakenAt),
        order: Some(SortDir::Desc),
        kind: Some(KindFilter::ImageVideo),
        ..Default::default()
    }
}

/// localAt range of a calendar year
pub fn year_range(year: i64) -> (i64, i64) {
    (utc(year, 1, 1), utc(year + 1, 1, 1))
}

/// `heatLevel`: 0 = empty, 1..4
pub fn heat_level(n: i64, max: i64) -> i32 {
    if n <= 0 || max <= 0 {
        return 0;
    }
    let l = ((n as f64).ln_1p() / (max as f64).ln_1p() * 4.0).ceil() as i32;
    l.clamp(1, 4)
}

/// `defaultCalendarYear`: newest year with a good share of the items
pub fn default_calendar_year(years: &[(i64, i64)]) -> Option<i64> {
    let max = years.iter().map(|y| y.1).max().unwrap_or(0);
    let min = ((max as f64) * 0.25).max(1.0);
    years
        .iter()
        .find(|y| y.1 as f64 >= min)
        .or(years.first())
        .map(|y| y.0)
}

/// Weekday (0 = Sunday) the calendar columns start on (Settings → first day of the week).
pub fn week_start_day(pref: &str) -> i64 {
    match pref {
        "sun" => 0,
        "sat" => 6,
        _ => 1,
    }
}

/// Calendar layout of a year: `lead` empty cells before Jan 1, then every day; month labels at the
/// week column of their first day.
pub fn calendar_layout(year: i64, week_start: i64) -> (i64, i64, Vec<(u32, i64)>) {
    let jan1 = days_from_civil(year, 1, 1);
    let lead = ((jan1 + 4).rem_euclid(7) - week_start + 7) % 7;
    let days = days_from_civil(year + 1, 1, 1) - jan1;
    let months = (1..=12)
        .map(|m| {
            let d = days_from_civil(year, m, 1) - jan1;
            (m, (d + lead) / 7)
        })
        .collect();
    (lead, days, months)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i64, m: i64, day: u32) -> i64 {
        utc(y, m, day)
    }

    #[test]
    fn ranges() {
        assert_eq!(
            date_range(d(2024, 7, 2), d(2024, 7, 7), Lang::Tr),
            "2–6 Tem 2024"
        );
        assert_eq!(
            date_range(d(2024, 6, 30), d(2024, 7, 3), Lang::Tr),
            "30 Haz – 2 Tem 2024"
        );
        assert_eq!(
            date_range(d(2023, 12, 30), d(2024, 1, 3), Lang::Tr),
            "30 Ara 2023 – 2 Oca 2024"
        );
        assert_eq!(
            date_range(d(2024, 11, 5), d(2024, 11, 6), Lang::Tr),
            "5 Kas 2024"
        );
        assert_eq!(
            date_range(d(2024, 7, 2), d(2024, 7, 7), Lang::En),
            "Jul 2 – 6, 2024"
        );
        assert_eq!(
            date_range(d(2024, 6, 30), d(2024, 7, 3), Lang::En),
            "Jun 30 – Jul 2, 2024"
        );
        assert_eq!(
            date_range(d(2023, 12, 30), d(2024, 1, 3), Lang::En),
            "Dec 30, 2023 – Jan 2, 2024"
        );
        assert_eq!(date_long(d(2024, 3, 5), Lang::Tr, true), "5 Mart 2024 Salı");
        assert_eq!(
            date_long(d(2024, 3, 5), Lang::En, true),
            "Tuesday, March 5, 2024"
        );
        assert_eq!(month_year(d(2024, 8, 5), Lang::Tr, false), "Ağu 2024");
        assert_eq!(day_month(d(2024, 9, 5), Lang::En), "Sep 5");
    }

    #[test]
    fn numbers() {
        assert_eq!(bytes_label(512, Lang::Tr), "512 B");
        assert_eq!(
            bytes_label((12.34 * 1024.0 * 1024.0 * 1024.0) as i64, Lang::Tr),
            "12,3 GB"
        );
        assert_eq!(
            bytes_label((12.34 * 1024.0 * 1024.0 * 1024.0) as i64, Lang::En),
            "12.3 GB"
        );
        assert_eq!(bytes_label(12 * 1024 * 1024, Lang::En), "12 MB");
        assert_eq!(bytes_label(150 * 1024 * 1024, Lang::En), "150 MB");
        assert_eq!(num_label(12345, Lang::Tr), "12.345");
    }

    #[test]
    fn countries_cameras() {
        assert_eq!(country_name("tr", Lang::Tr), "Türkiye");
        assert_eq!(country_name("DE", Lang::Tr), "Almanya");
        assert_eq!(country_name("DE", Lang::En), "Germany");
        assert_eq!(country_name("QQ", Lang::En), "QQ");
        assert_eq!(
            camera_label(Some("NIKON"), Some("NIKON D750")),
            "NIKON D750"
        );
        assert_eq!(camera_label(Some("Apple"), Some("iPhone 13")), "iPhone 13");
        assert_eq!(camera_label(Some("Canon"), Some("EOS R6")), "Canon EOS R6");
        assert_eq!(camera_label(Some("Sony"), None), "Sony");
        assert!(crate::explore_flags::cell("tr").is_some());
        assert!(crate::explore_flags::cell("QQ").is_none());
    }

    #[test]
    fn days_and_memories() {
        assert_eq!(day_range("2024-03-05").map(|r| r.1 - r.0), Some(DAY_MS));
        assert_eq!(day_range("2024-02-30"), None);
        assert_eq!(day_range("2024-3-05"), None);
        assert_eq!(day_key(d(2024, 3, 5) + 3_600_000), "2024-03-05");
        let now = d(2026, 11, 7);
        let m = memory_from_id("otd-2023-11-07", now).unwrap();
        assert_eq!(m.years_ago, Some(3));
        assert_eq!(m.to - m.from, 7 * DAY_MS);
        let l = memory_label(&m, Lang::Tr);
        assert_eq!(l.subtitle, "7 Kasım 2023");
        let y = memory_from_id("year-2024", now).unwrap();
        assert_eq!(y.kind, MemoryKind::Year);
        assert_eq!(memory_label(&y, Lang::En).subtitle, "2024");
        assert!(memory_from_id("trip-x", now).is_none());
        let trip = Memory {
            id: "trip-1".into(),
            kind: MemoryKind::Trip,
            years_ago: None,
            place: Some("Antalya".into()),
            cc: Some("TR".into()),
            from: d(2024, 7, 2),
            to: d(2024, 7, 7),
            count: 12,
            cover_ids: vec![],
            months: None,
        };
        let l = memory_label(&trip, Lang::Tr);
        assert_eq!(l.subtitle, "Türkiye · 2–6 Tem 2024");
        assert_eq!(memory_short(&trip, Lang::Tr), "Antalya · Tem 2024");
    }

    #[test]
    fn calendar() {
        assert_eq!(heat_level(0, 10), 0);
        assert_eq!(heat_level(10, 10), 4);
        assert_eq!(heat_level(1, 100), 1);
        assert_eq!(
            default_calendar_year(&[(2025, 3), (2024, 400), (2023, 50)]),
            Some(2024)
        );
        // 2024-01-01 was a Monday: no lead with Monday weeks, 1 with Sunday weeks
        let (lead, days, months) = calendar_layout(2024, 1);
        assert_eq!((lead, days), (0, 366));
        assert_eq!(months[0], (1, 0));
        // Feb 1 2024 is day 31 → column 4
        assert_eq!(months[1], (2, 4));
        assert_eq!(calendar_layout(2024, 0).0, 1);
        // 2026-01-01 is a Thursday: 3 empty cells before it (Mon weeks)
        assert_eq!(calendar_layout(2026, 1).0, 3);
    }
}
