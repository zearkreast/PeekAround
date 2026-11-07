//! Free-text search parsing (mirrors `search.ts`): "istanbul 2023 video", "Ağustos 2024", "haziran selfie".

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::js;
use crate::types::MediaFlagName;

fn tr_fold(c: char) -> char {
    match c {
        'İ' | 'I' | 'ı' | 'i' => 'i',
        'Ş' | 'ş' => 's',
        'Ğ' | 'ğ' => 'g',
        'Ü' | 'ü' => 'u',
        'Ö' | 'ö' => 'o',
        'Ç' | 'ç' => 'c',
        _ => c,
    }
}

/// Case + diacritic folding with an explicit Turkish table ("İstanbul", "ISTANBUL", "ıstanbul" → "istanbul").
pub fn fold_text(s: Option<&str>) -> String {
    let Some(s) = s.filter(|s| !s.is_empty()) else {
        return String::new();
    };
    let pre: String = s.chars().map(tr_fold).collect();
    let n: String = pre
        .nfkd()
        .filter(|c| !('\u{300}'..='\u{36f}').contains(c))
        .collect();
    n.replace('ß', "ss").to_lowercase()
}

/// \[month, full names, abbreviations (only honoured next to a year)\]
const MONTH_NAMES: &[(u32, &[&str], &[&str])] = &[
    (1, &["ocak", "january"], &["oca", "jan"]),
    (2, &["subat", "february"], &["sub", "feb"]),
    (3, &["mart", "march"], &["mar"]),
    (4, &["nisan", "april"], &["nis", "apr"]),
    (5, &["mayis", "may"], &[]),
    (6, &["haziran", "june"], &["haz", "jun"]),
    (7, &["temmuz", "july"], &["tem", "jul"]),
    (8, &["agustos", "august"], &["agu", "aug"]),
    (9, &["eylul", "september"], &["eyl", "sep", "sept"]),
    (10, &["ekim", "october"], &["eki", "oct"]),
    (11, &["kasim", "november"], &["kas", "nov"]),
    (12, &["aralik", "december"], &["ara", "dec"]),
];

fn month_full(w: &str) -> Option<u32> {
    MONTH_NAMES
        .iter()
        .find(|(_, f, _)| f.contains(&w))
        .map(|(m, _, _)| *m)
}

fn month_short(w: &str) -> Option<u32> {
    MONTH_NAMES
        .iter()
        .find(|(_, _, s)| s.contains(&w))
        .map(|(m, _, _)| *m)
}

/// multi-word phrases first (matched on folded text)
const FLAG_WORDS: &[(&str, MediaFlagName)] = &[
    ("ekran goruntuleri", MediaFlagName::Screenshot),
    ("ekran goruntusu", MediaFlagName::Screenshot),
    ("ekran resmi", MediaFlagName::Screenshot),
    ("screen shots", MediaFlagName::Screenshot),
    ("screenshots", MediaFlagName::Screenshot),
    ("screenshot", MediaFlagName::Screenshot),
    ("videolar", MediaFlagName::Video),
    ("videos", MediaFlagName::Video),
    ("video", MediaFlagName::Video),
    ("selfies", MediaFlagName::Selfie),
    ("selfieler", MediaFlagName::Selfie),
    ("selfie", MediaFlagName::Selfie),
    ("ozcekim", MediaFlagName::Selfie),
    ("panoramalar", MediaFlagName::Panorama),
    ("panoramas", MediaFlagName::Panorama),
    ("panorama", MediaFlagName::Panorama),
    ("animasyonlar", MediaFlagName::Animated),
    ("animasyon", MediaFlagName::Animated),
    ("animated", MediaFlagName::Animated),
    ("animations", MediaFlagName::Animated),
    ("hareketli", MediaFlagName::Animated),
    ("gifler", MediaFlagName::Animated),
    ("gifs", MediaFlagName::Animated),
    ("gif", MediaFlagName::Animated),
];

/// Parsed search input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ParsedSearch {
    /// remaining free text (folded, single spaced)
    pub text: String,
    /// remaining folded words; each must match name / place / camera
    pub terms: Vec<String>,
    /// localAt range start
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<i64>,
    /// localAt range end (exclusive)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<i64>,
    /// month (1-12) across all years when no year was given
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month: Option<u32>,
    /// type word
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<MediaFlagName>,
}

static NON_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[^\p{L}\p{N}\t\n\x0B\x0C\r \u{A0}\u{1680}\u{2000}-\u{200A}\u{2028}\u{2029}\u{202F}\u{205F}\u{3000}\u{FEFF}'._-]+").expect("re")
});

const PUNCT: &[char] = &['.', '_', '\'', '-'];

fn is_year(w: &str) -> bool {
    // /^(19[7-9]\d|2[01]\d\d)$/
    let b = w.as_bytes();
    b.len() == 4
        && b.iter().all(u8::is_ascii_digit)
        && ((b[0] == b'1' && b[1] == b'9' && (b'7'..=b'9').contains(&b[2]))
            || (b[0] == b'2' && (b[1] == b'0' || b[1] == b'1')))
}

/// Parse a free-text query (deterministic; relative words are not supported yet).
pub fn parse_search(input: &str) -> ParsedSearch {
    let folded = fold_text(Some(input));
    let cleaned = NON_WORD.replace_all(&folded, " ");
    let single: Vec<&str> = cleaned
        .split(js::is_space)
        .filter(|w| !w.is_empty())
        .collect();
    let mut s = format!(" {} ", single.join(" "));
    let mut flag = None;
    for (w, f) in FLAG_WORDS {
        let needle = format!(" {w} ");
        if s.contains(&needle) {
            if flag.is_none() {
                flag = Some(*f);
            }
            s = s.replacen(&needle, " ", 1);
        }
    }
    let words: Vec<&str> = s
        .split(' ')
        .map(|w| w.trim_matches(PUNCT))
        .filter(|w| !w.is_empty())
        .collect();
    let year_at = words.iter().position(|w| is_year(w));
    let year: Option<i64> = year_at.map(|i| words[i].parse().unwrap_or(0));
    let mut month = None;
    let mut terms = vec![];
    for (i, w) in words.iter().enumerate() {
        if Some(i) == year_at {
            continue;
        }
        let m = month_full(w).or_else(|| if year.is_some() { month_short(w) } else { None });
        if m.is_some() && month.is_none() {
            month = m;
        } else {
            terms.push(w.to_string());
        }
    }
    let mut out = ParsedSearch {
        text: terms.join(" "),
        terms,
        flag,
        ..Default::default()
    };
    match (year, month) {
        (Some(y), Some(m)) => {
            out.from = Some(js::date_utc(y, m as i64 - 1, 1, 0, 0, 0, 0));
            out.to = Some(js::date_utc(y, m as i64, 1, 0, 0, 0, 0));
        }
        (Some(y), None) => {
            out.from = Some(js::date_utc(y, 0, 1, 0, 0, 0, 0));
            out.to = Some(js::date_utc(y + 1, 0, 1, 0, 0, 0, 0));
        }
        (None, Some(m)) => out.month = Some(m),
        (None, None) => {}
    }
    out
}
