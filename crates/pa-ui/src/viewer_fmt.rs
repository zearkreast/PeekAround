//! Formatting of the viewer / info panel (the `formatLocal` / `formatDate` / `formatBytes` /
//! `formatTime` calls of Viewer.tsx, InfoPanel.tsx and VideoPlayer.tsx, Intl-compatible output).

use crate::i18n::{civil_from_days, format_time, HourCycle, Lang};

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
const MONTHS_SHORT: [[&str; 12]; 2] = [
    [
        "Oca", "Şub", "Mar", "Nis", "May", "Haz", "Tem", "Ağu", "Eyl", "Eki", "Kas", "Ara",
    ],
    [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
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

fn li(l: Lang) -> usize {
    match l {
        Lang::Tr => 0,
        Lang::En => 1,
    }
}

fn split(local_ms: i64) -> (i64, u32, u32, usize, i64) {
    let days = local_ms.div_euclid(86_400_000);
    let (y, m, d) = civil_from_days(days);
    let wd = (days + 4).rem_euclid(7) as usize;
    (y, m, d, wd, local_ms.rem_euclid(86_400_000) / 60_000)
}

/// `{ dateStyle: 'long' }`: "5 Temmuz 2024" / "July 5, 2024"
pub fn date_long(local_ms: i64, lang: Lang) -> String {
    let (y, m, d, _, _) = split(local_ms);
    let mon = MONTHS_LONG[li(lang)][m as usize - 1];
    match lang {
        Lang::Tr => format!("{d} {mon} {y}"),
        Lang::En => format!("{mon} {d}, {y}"),
    }
}

/// `{ dateStyle: 'full' }`: "5 Temmuz 2024 Cuma" / "Friday, July 5, 2024"
pub fn date_full(local_ms: i64, lang: Lang) -> String {
    let (_, _, _, wd, _) = split(local_ms);
    let day = WEEKDAYS_LONG[li(lang)][wd];
    match lang {
        Lang::Tr => format!("{} {day}", date_long(local_ms, lang)),
        Lang::En => format!("{day}, {}", date_long(local_ms, lang)),
    }
}

/// `{ timeStyle: 'short' }`
pub fn time_short(local_ms: i64, lang: Lang, hc: HourCycle) -> String {
    let (.., mins) = split(local_ms);
    format_time(mins, lang, hc, false)
}

/// `{ dateStyle: 'long', timeStyle: 'short' }`: "5 Temmuz 2024 14:30" / "July 5, 2024 at 2:30 PM"
pub fn date_long_time(local_ms: i64, lang: Lang, hc: HourCycle) -> String {
    let t = time_short(local_ms, lang, hc);
    match lang {
        Lang::Tr => format!("{} {t}", date_long(local_ms, lang)),
        Lang::En => format!("{} at {t}", date_long(local_ms, lang)),
    }
}

/// `{ dateStyle: 'medium' }`: "5 Tem 2024" / "Jul 5, 2024"
pub fn date_medium(local_ms: i64, lang: Lang) -> String {
    let (y, m, d, _, _) = split(local_ms);
    let mon = MONTHS_SHORT[li(lang)][m as usize - 1];
    match lang {
        Lang::Tr => format!("{d} {mon} {y}"),
        Lang::En => format!("{mon} {d}, {y}"),
    }
}

/// `{ dateStyle: 'medium', timeStyle: 'short' }`: "5 Tem 2024 14:30" / "Jul 5, 2024, 2:30 PM"
pub fn date_medium_time(local_ms: i64, lang: Lang, hc: HourCycle) -> String {
    let (y, m, d, _, _) = split(local_ms);
    let mon = MONTHS_SHORT[li(lang)][m as usize - 1];
    let t = time_short(local_ms, lang, hc);
    match lang {
        Lang::Tr => format!("{d} {mon} {y} {t}"),
        Lang::En => format!("{mon} {d}, {y}, {t}"),
    }
}

static TIME_ZONE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// The machine's IANA time zone; the platform crates set it once at start-up
/// (the UI crate has no access to the system zone, notably on wasm).
pub fn set_time_zone(tz: String) {
    let _ = TIME_ZONE.set(tz);
}

/// The machine's UTC offset (ms) at `instant` — `formatDate` of UTC instants shows them in local
/// time. Without a zone set: `PA_TZ_OFFSET_MIN`, else UTC.
pub fn local_offset_at(instant: i64) -> i64 {
    if let Some(tz) = TIME_ZONE.get() {
        return pa_core::time::tz_offset_at(instant, tz) as i64 * 60_000;
    }
    std::env::var("PA_TZ_OFFSET_MIN")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .map(|m| m * 60_000)
        .unwrap_or(0)
}

/// `formatTime` of the video player: "0:05", "12:34", "1:02:03"
pub fn clock(t: f64) -> String {
    let t = if t.is_finite() {
        t.max(0.0).floor() as i64
    } else {
        0
    };
    let (h, m, s) = (t / 3600, (t % 3600) / 60, t % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// `cameraLabel`: "Apple iPhone 15" — the make is dropped when the model already starts with it.
pub fn camera_label(make: Option<&str>, model: Option<&str>) -> String {
    let make = make.map(str::trim).filter(|s| !s.is_empty());
    let model = model.map(str::trim).filter(|s| !s.is_empty());
    match (make, model) {
        (Some(a), Some(b)) => {
            let first = a.split_whitespace().next().unwrap_or(a).to_lowercase();
            if b.to_lowercase().starts_with(&first) {
                b.to_string()
            } else {
                format!("{a} {b}")
            }
        }
        (Some(a), None) => a.to_string(),
        (None, Some(b)) => b.to_string(),
        (None, None) => String::new(),
    }
}

/// Country name of an ISO code (the code itself when unknown).
pub fn country(cc: &str, lang: Lang) -> String {
    crate::explore_labels::country_name(cc, lang)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::days_from_civil;

    #[test]
    fn dates() {
        let ms = days_from_civil(2024, 7, 5) * 86_400_000 + (14 * 60 + 30) * 60_000;
        assert_eq!(date_long(ms, Lang::Tr), "5 Temmuz 2024");
        assert_eq!(date_full(ms, Lang::Tr), "5 Temmuz 2024 Cuma");
        assert_eq!(date_full(ms, Lang::En), "Friday, July 5, 2024");
        assert_eq!(
            date_long_time(ms, Lang::En, HourCycle::Auto),
            "July 5, 2024 at 2:30 PM"
        );
        assert_eq!(
            date_long_time(ms, Lang::Tr, HourCycle::Auto),
            "5 Temmuz 2024 14:30"
        );
        assert_eq!(
            date_medium_time(ms, Lang::Tr, HourCycle::Auto),
            "5 Tem 2024 14:30"
        );
    }

    #[test]
    fn misc() {
        assert_eq!(clock(65.7), "1:05");
        assert_eq!(clock(3723.0), "1:02:03");
        assert_eq!(
            camera_label(Some("Apple"), Some("iPhone 15")),
            "Apple iPhone 15"
        );
        assert_eq!(
            camera_label(Some("Canon"), Some("Canon EOS R6")),
            "Canon EOS R6"
        );
        assert_eq!(camera_label(None, Some("X")), "X");
        assert_eq!(country("rs", Lang::Tr), "Sırbistan");
        assert_eq!(country("HR", Lang::En), "Croatia");
    }
}
