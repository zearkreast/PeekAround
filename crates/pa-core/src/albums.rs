//! Albums and per-library options (mirrors `albums.ts`).

use std::collections::{BTreeMap, HashSet};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::js;

/// How an album orders its items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AlbumOrder {
    /// newest first
    #[default]
    Desc,
    /// oldest first
    Asc,
    /// album_items.position
    Custom,
}

/// A user-curated album.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    /// id
    pub id: String,
    /// name
    pub name: String,
    /// chosen cover, or the newest item (None for an empty album)
    pub cover_id: Option<String>,
    /// coverId was picked by the user
    pub cover_picked: bool,
    /// items
    pub count: i64,
    /// photos
    pub photos: i64,
    /// videos
    pub videos: i64,
    /// localAt range start
    pub from: Option<i64>,
    /// localAt range end
    pub to: Option<i64>,
    /// order
    pub order: AlbumOrder,
    /// ms epoch
    pub created: i64,
    /// ms epoch
    pub updated: i64,
    /// sort position
    pub sort: i64,
}

/// Which albums contain some media (album picker check marks).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AlbumMembership {
    /// album id → how many of the asked items it contains
    pub counts: BTreeMap<String, i64>,
}

/// Per-library scan options.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LibraryOptions {
    /// names / glob patterns to skip (case-insensitive)
    pub exclude: Vec<String>,
    /// skip images smaller than this many bytes; 0 = keep everything
    pub min_bytes: i64,
    /// index hidden files of this library; None = global setting
    pub show_hidden: Option<bool>,
}

/// `DEFAULT_LIBRARY_OPTIONS`
pub fn default_library_options() -> LibraryOptions {
    LibraryOptions::default()
}

fn clean_pattern(s: &str) -> String {
    js::trim(s).replace('\\', "/").trim_matches('/').to_string()
}

/// Normalise untrusted library options (`normalizeLibraryOptions`), from any JSON value.
pub fn normalize_library_options(o: Option<&serde_json::Value>) -> LibraryOptions {
    use serde_json::Value;
    let obj = o.and_then(Value::as_object);
    let get = |k: &str| obj.and_then(|m| m.get(k));
    let exclude: Vec<String> = match get("exclude") {
        Some(Value::Array(a)) => {
            let mut seen = HashSet::new();
            a.iter()
                .map(|s| clean_pattern(&js::to_string_of(s)))
                .filter(|s| !s.is_empty())
                .filter(|s| seen.insert(s.clone()))
                .take(100)
                .map(|s| js::slice16_to(&s, 200).to_string())
                .collect()
        }
        _ => vec![],
    };
    let min = match get("minBytes") {
        Some(v) => js::number_of(v),
        None => f64::NAN,
    };
    LibraryOptions {
        exclude,
        min_bytes: if min.is_finite() && min > 0.0 {
            js::round(min).min(1024.0 * 1024.0 * 1024.0) as i64
        } else {
            0
        },
        show_hidden: get("showHidden").and_then(Value::as_bool),
    }
}

/// Library statistics (`GET /api/libraries/:id/stats`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStats {
    /// items
    pub items: i64,
    /// photos
    pub photos: i64,
    /// videos
    pub videos: i64,
    /// audio
    pub audio: i64,
    /// bytes
    pub bytes: i64,
    /// localAt range start
    pub from: Option<i64>,
    /// localAt range end
    pub to: Option<i64>,
    /// distinct directories
    pub folders: i64,
    /// last completed scan
    pub scanned_at: Option<i64>,
    /// a scan is running
    pub scanning: bool,
    /// items not ready yet
    pub pending: i64,
}

fn glob_to_regex(glob: &str) -> Option<Regex> {
    let g: Vec<char> = glob.chars().collect();
    let mut re = String::new();
    let mut i = 0;
    while i < g.len() {
        let c = g[i];
        if c == '*' {
            if g.get(i + 1) == Some(&'*') {
                if g.get(i + 2) == Some(&'/') {
                    re.push_str("(?:.*/)?");
                    i += 2;
                } else {
                    re.push_str(".*");
                    i += 1;
                }
            } else {
                re.push_str("[^/]*");
            }
        } else if c == '?' {
            re.push_str("[^/]");
        } else if ".+^${}()|[]\\".contains(c) {
            re.push('\\');
            re.push(c);
        } else {
            re.push(c);
        }
        i += 1;
    }
    Regex::new(&format!("(?i)^{re}$")).ok()
}

/// Compiled exclusion patterns (see [`LibraryOptions::exclude`]).
#[derive(Debug, Clone, Default)]
pub struct ExcludeMatcher {
    names: Vec<Regex>,
    paths: Vec<Regex>,
}

impl ExcludeMatcher {
    /// Compile patterns (invalid ones are ignored).
    pub fn new<S: AsRef<str>>(patterns: &[S]) -> Self {
        let mut m = ExcludeMatcher::default();
        for p in patterns {
            let s = clean_pattern(p.as_ref());
            if s.is_empty() {
                continue;
            }
            if let Some(r) = glob_to_regex(&s) {
                if s.contains('/') {
                    m.paths.push(r);
                } else {
                    m.names.push(r);
                }
            }
        }
        m
    }

    /// no patterns
    pub fn is_empty(&self) -> bool {
        self.names.is_empty() && self.paths.is_empty()
    }

    /// a single name (any segment) is excluded
    pub fn name(&self, n: &str) -> bool {
        self.names.iter().any(|r| r.is_match(n))
    }

    /// a relative path ('/' separated, file or dir) is excluded
    pub fn path(&self, rel: &str) -> bool {
        if rel.is_empty() {
            return false;
        }
        let segs: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
        if !self.names.is_empty() && segs.iter().any(|s| self.name(s)) {
            return true;
        }
        if self.paths.is_empty() {
            return false;
        }
        let mut acc = String::new();
        for s in segs {
            if !acc.is_empty() {
                acc.push('/');
            }
            acc.push_str(s);
            if self.paths.iter().any(|r| r.is_match(&acc)) {
                return true;
            }
        }
        false
    }
}

/// `excludeMatcher(patterns)`
pub fn exclude_matcher<S: AsRef<str>>(patterns: &[S]) -> ExcludeMatcher {
    ExcludeMatcher::new(patterns)
}
