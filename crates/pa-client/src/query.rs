//! Query strings exactly like the web UI's `qs()` / `browseQs()`: empty / null / false values are
//! dropped, `true` is sent as `1`.

use std::fmt;

use pa_core::BrowseQuery;
use serde::Serialize;

/// Percent-encode one path segment / query value (`encodeURIComponent`).
pub fn encode_seg(s: &str) -> String {
    // encodeURIComponent keeps A-Z a-z 0-9 - _ . ! ~ * ' ( )
    const SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'!')
        .remove(b'~')
        .remove(b'*')
        .remove(b'\'')
        .remove(b'(')
        .remove(b')');
    percent_encoding::utf8_percent_encode(s, SET).to_string()
}

/// The JSON string of a unit enum (`SortKey::TakenAt` → "takenAt").
pub(crate) fn enum_str<T: Serialize>(v: &T) -> Option<String> {
    match serde_json::to_value(v).ok()? {
        serde_json::Value::String(s) => Some(s),
        _ => None,
    }
}

/// Query string builder; `Display` gives `""` or `"?a=1&b=2"`.
#[derive(Debug, Default, Clone)]
pub struct Qs(Vec<(String, String)>);

impl Qs {
    /// empty
    pub fn new() -> Qs {
        Qs(Vec::new())
    }
    /// a string value (skipped when empty)
    pub fn str(&mut self, k: &str, v: &str) -> &mut Qs {
        if !v.is_empty() {
            self.0.push((k.into(), v.into()));
        }
        self
    }
    /// an optional string value
    pub fn opt(&mut self, k: &str, v: Option<&str>) -> &mut Qs {
        if let Some(v) = v {
            self.str(k, v);
        }
        self
    }
    /// a number
    pub fn num(&mut self, k: &str, v: i64) -> &mut Qs {
        self.0.push((k.into(), v.to_string()));
        self
    }
    /// `1` when true, nothing otherwise
    pub fn flag(&mut self, k: &str, v: bool) -> &mut Qs {
        if v {
            self.0.push((k.into(), "1".into()));
        }
        self
    }
}

impl fmt::Display for Qs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, (k, v)) in self.0.iter().enumerate() {
            write!(
                f,
                "{}{}={}",
                if i == 0 { '?' } else { '&' },
                encode_seg(k),
                encode_seg(v)
            )?;
        }
        Ok(())
    }
}

/// `?libraryId=…&recursive=1&sort=takenAt…` for `/api/browse` and `/api/browse/skeleton`.
pub fn browse_qs(q: &BrowseQuery) -> String {
    let mut qs = Qs::new();
    // field order of the web UI's browseQs (the serde field names are the same camelCase keys)
    if let Ok(serde_json::Value::Object(map)) = serde_json::to_value(q) {
        for (k, v) in map {
            match v {
                serde_json::Value::Bool(b) => {
                    qs.flag(&k, b);
                }
                serde_json::Value::String(s) => {
                    qs.str(&k, &s);
                }
                serde_json::Value::Number(n) => {
                    qs.0.push((k, n.to_string()));
                }
                _ => {}
            }
        }
    }
    qs.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pa_core::{KindFilter, SortDir, SortKey};

    #[test]
    fn browse_query_string() {
        let q = BrowseQuery {
            recursive: Some(true),
            sort: Some(SortKey::TakenAt),
            order: Some(SortDir::Desc),
            kind: Some(KindFilter::Favorites),
            q: Some("deniz & güneş".into()),
            offset: Some(300),
            limit: Some(300),
            dup: Some(false),
            ..Default::default()
        };
        let s = browse_qs(&q);
        assert!(s.starts_with('?'));
        let mut parts: Vec<&str> = s[1..].split('&').collect();
        parts.sort();
        assert_eq!(
            parts,
            [
                "kind=favorites",
                "limit=300",
                "offset=300",
                "order=desc",
                "q=deniz%20%26%20g%C3%BCne%C5%9F",
                "recursive=1",
                "sort=takenAt"
            ]
        );
        assert_eq!(browse_qs(&BrowseQuery::default()), "");
        assert_eq!(encode_seg("a/b c'(x)"), "a%2Fb%20c'(x)");
    }
}
