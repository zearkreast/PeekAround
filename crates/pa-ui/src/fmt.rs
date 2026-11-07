//! Formatting helpers of web/src/lib/format.ts (bytes, relative time, "time left", month ranges).

use crate::i18n::{self, Lang};

/// `formatBytes`: 0 B, 512 B, 1.5 KB, 12.3 MB, 120 GB
pub fn format_bytes(n: i64) -> String {
    if n <= 0 {
        return "0 B".into();
    }
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let nf = n as f64;
    let i = ((nf.ln() / 1024f64.ln()).floor() as usize).min(U.len() - 1);
    let v = nf / 1024f64.powi(i as i32);
    let digits = if i == 0 || v >= 100.0 { 0 } else { 1 };
    format!("{v:.digits$} {}", U[i])
}

/// ms since the epoch
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `relativeTime` (Intl.RelativeTimeFormat numeric: "auto")
pub fn relative_time(ms: i64, lang: Lang) -> String {
    relative_time_at(ms, now_ms(), lang)
}

/// [`relative_time`] against a given "now" (tests)
pub fn relative_time_at(ms: i64, now: i64, lang: Lang) -> String {
    let diff = (ms - now) as f64;
    let abs = diff.abs();
    let (n, unit) = if abs < 3_600_000.0 {
        ((diff / 60_000.0).round() as i64, 0)
    } else if abs < 86_400_000.0 {
        ((diff / 3_600_000.0).round() as i64, 1)
    } else {
        ((diff / 86_400_000.0).round() as i64, 2)
    };
    let tr = lang == Lang::Tr;
    match (unit, n) {
        (0, 0) => if tr { "bu dakika" } else { "this minute" }.into(),
        (1, 0) => if tr { "bu saat" } else { "this hour" }.into(),
        (2, 0) => if tr { "bugün" } else { "today" }.into(),
        (2, -1) => if tr { "dün" } else { "yesterday" }.into(),
        (2, 1) => if tr { "yarın" } else { "tomorrow" }.into(),
        _ => {
            let a = n.abs();
            if tr {
                let u = ["dakika", "saat", "gün"][unit];
                format!("{a} {u} {}", if n < 0 { "önce" } else { "sonra" })
            } else {
                let u = ["minute", "hour", "day"][unit];
                let s = if a == 1 { "" } else { "s" };
                if n < 0 {
                    format!("{a} {u}{s} ago")
                } else {
                    format!("in {a} {u}{s}")
                }
            }
        }
    }
}

/// `durationUntil`: "3 sa 12 dk" / "3h 12m"
pub fn duration_until(ms: i64, lang: Lang) -> String {
    let diff = (ms - now_ms()).max(0);
    let h = diff / 3_600_000;
    let m = (diff % 3_600_000) / 60_000;
    match (lang, h) {
        (Lang::Tr, 0) => format!("{m} dk"),
        (Lang::Tr, _) => format!("{h} sa {m} dk"),
        (_, 0) => format!("{m}m"),
        _ => format!("{h}h {m}m"),
    }
}

fn month_key(ms: i64) -> String {
    let (y, m, _) = i18n::civil_from_days(ms.div_euclid(86_400_000));
    format!("{y:04}-{m:02}")
}

/// `rangeLabel`: "Mar 2023 – Jun 2024" (or one month) of a localAt range
pub fn range_label(from: Option<i64>, to: Option<i64>, lang: Lang) -> Option<String> {
    let (a, b) = (from?, to?);
    let a = i18n::format_month_key(&month_key(a), lang);
    let b = i18n::format_month_key(&month_key(b), lang);
    Some(if a == b { a } else { format!("{a} – {b}") })
}

/// fold for search (case, Turkish letters, accents): "Çığ Öö" → "cig oo"
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let r = match c {
            'İ' | 'I' | 'ı' | 'i' | 'Î' | 'î' | 'Ï' | 'ï' | 'Í' | 'í' | 'Ì' | 'ì' => 'i',
            'Ç' | 'ç' => 'c',
            'Ğ' | 'ğ' => 'g',
            'Ö' | 'ö' | 'Ó' | 'ó' | 'Ò' | 'ò' | 'Ô' | 'ô' | 'Õ' | 'õ' => 'o',
            'Ş' | 'ş' => 's',
            'Ü' | 'ü' | 'Ú' | 'ú' | 'Ù' | 'ù' | 'Û' | 'û' => 'u',
            'Â' | 'â' | 'Á' | 'á' | 'À' | 'à' | 'Ä' | 'ä' | 'Ã' | 'ã' | 'Å' | 'å' => {
                'a'
            }
            'É' | 'é' | 'È' | 'è' | 'Ê' | 'ê' | 'Ë' | 'ë' => 'e',
            'Ñ' | 'ñ' => 'n',
            c => c.to_lowercase().next().unwrap_or(c),
        };
        out.push(r);
    }
    out
}

/// every word of `query` appears in `text` (folded) — settings search `matchesQuery`
pub fn matches_query(text: &str, query: &str) -> bool {
    let hay = fold(text);
    fold(query).split_whitespace().all(|w| hay.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(150 * 1024 * 1024), "150 MB");
        assert_eq!(
            format_bytes(3 * 1024 * 1024 * 1024 + 1024 * 1024 * 300),
            "3.3 GB"
        );
    }

    #[test]
    fn relative() {
        let now = 1_700_000_000_000;
        assert_eq!(
            relative_time_at(now - 5 * 60_000, now, Lang::Tr),
            "5 dakika önce"
        );
        assert_eq!(
            relative_time_at(now - 5 * 60_000, now, Lang::En),
            "5 minutes ago"
        );
        assert_eq!(
            relative_time_at(now - 3 * 3_600_000, now, Lang::En),
            "3 hours ago"
        );
        assert_eq!(relative_time_at(now - 86_400_000, now, Lang::Tr), "dün");
        assert_eq!(
            relative_time_at(now + 2 * 86_400_000, now, Lang::En),
            "in 2 days"
        );
        assert_eq!(relative_time_at(now, now, Lang::En), "this minute");
    }

    #[test]
    fn search_fold() {
        assert!(matches_query(
            "Önbellek küçük resimler thumbnail",
            "onbellek"
        ));
        assert!(matches_query("Dil Language", "LANG"));
        assert!(matches_query("Işık tema", "isik"));
        assert!(!matches_query("Tema", "dil"));
        assert!(matches_query("anything", "  "));
    }
}
