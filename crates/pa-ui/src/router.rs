//! Routes with parameters (React router paths: `/album/:id`, `/library/:id`, `/folder/:id/*dir`)
//! on top of the `Page` enum the .slint side switches on.

use crate::Page;

/// Where the app is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// `/` timeline
    Photos,
    /// `/explore`
    Explore,
    /// `/favorites`
    Favorites,
    /// `/albums`
    Albums,
    /// `/library` (hub)
    Library,
    /// `/settings` (+ section)
    Settings,
    /// `/search?q=`
    Search(String),
    /// `/album/:id`
    Album(String),
    /// `/library/:id`
    Lib(String),
    /// `/folder/:id/*dir`
    Folder(String, String),
    /// `/explore/<kind>/<key>` result grids: place, country, city, camera, type, year, memory,
    /// color, time, shape, length, day (+ largest, recent, nogps without a key)
    ExploreResult(String, String),
    /// `/explore/places`
    ExplorePlaces,
    /// `/explore/duplicates`
    ExploreDuplicates,
    /// `/explore/map[?place=…][?focus=<id>&lat=&lon=]` (focus: id, lat, lon as given)
    ExploreMap {
        /// place key filter ("" = everything)
        place: String,
        /// "Show on map": centre on this item
        focus: Option<(String, String, String)>,
    },
}

impl Route {
    /// the route of a plain page
    pub fn of_page(p: Page, search: &str) -> Route {
        match p {
            Page::Photos => Route::Photos,
            Page::Explore => Route::Explore,
            Page::Favorites => Route::Favorites,
            Page::Albums => Route::Albums,
            Page::Library => Route::Library,
            Page::Settings => Route::Settings,
            Page::Search => Route::Search(search.to_string()),
            // pages with parameters are reached through `AppInner::go` only
            Page::Album | Page::Lib | Page::Folder => Route::Library,
            Page::ExploreResult | Page::ExplorePlaces | Page::ExploreDups | Page::ExploreMap => {
                Route::Explore
            }
        }
    }

    /// the page that renders it
    pub fn page(&self) -> Page {
        match self {
            Route::Photos => Page::Photos,
            Route::Explore => Page::Explore,
            Route::Favorites => Page::Favorites,
            Route::Albums => Page::Albums,
            Route::Library => Page::Library,
            Route::Settings => Page::Settings,
            Route::Search(_) => Page::Search,
            Route::Album(_) => Page::Album,
            Route::Lib(_) => Page::Lib,
            Route::Folder(..) => Page::Folder,
            Route::ExploreResult(..) => Page::ExploreResult,
            Route::ExplorePlaces => Page::ExplorePlaces,
            Route::ExploreDuplicates => Page::ExploreDups,
            Route::ExploreMap { .. } => Page::ExploreMap,
        }
    }

    /// parse a path (automation script / deep links): `/album/3`, `/library/1`, `/folder/2/a/b`
    pub fn parse(p: &str) -> Option<Route> {
        let p = p.trim().trim_start_matches('/');
        if let Some(rest) = p.strip_prefix("explore/") {
            return Self::parse_explore(rest);
        }
        let mut it = p.splitn(3, '/');
        let head = it.next().unwrap_or("");
        let a = it.next().map(str::to_string);
        let rest = it.next().unwrap_or("").to_string();
        Some(match (head, a) {
            ("" | "photos" | "timeline", None) => Route::Photos,
            ("explore", None) => Route::Explore,
            ("favorites", None) => Route::Favorites,
            ("albums", None) => Route::Albums,
            ("library", None) => Route::Library,
            ("settings", _) => Route::Settings,
            ("search", Some(q)) => Route::Search(q),
            ("album", Some(id)) => Route::Album(id),
            ("library", Some(id)) => Route::Lib(id),
            ("folder", Some(id)) => Route::Folder(id, rest),
            _ => return None,
        })
    }
}

impl Route {
    /// `places`, `duplicates`, `map?…`, `<kind>/<key>`, `largest` / `recent` / `no-location`
    fn parse_explore(rest: &str) -> Option<Route> {
        let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
        let q = |k: &str| {
            query
                .split('&')
                .filter_map(|kv| kv.split_once('='))
                .find(|(a, _)| *a == k)
                .map(|(_, v)| decode(v))
        };
        let (head, key) = path.split_once('/').unwrap_or((path, ""));
        let key = decode(key.trim_end_matches("/play"));
        Some(match head {
            "" => Route::Explore,
            "places" => Route::ExplorePlaces,
            "duplicates" => Route::ExploreDuplicates,
            "map" => Route::ExploreMap {
                place: q("place").unwrap_or_default(),
                focus: match (q("focus"), q("lat"), q("lon")) {
                    (Some(id), Some(la), Some(lo)) => Some((id, la, lo)),
                    _ => None,
                },
            },
            "largest" | "recent" => Route::ExploreResult(head.into(), String::new()),
            "no-location" => Route::ExploreResult("nogps".into(), String::new()),
            "place" | "country" | "city" | "camera" | "type" | "year" | "memory" | "color"
            | "time" | "shape" | "length" | "day"
                if !key.is_empty() =>
            {
                Route::ExploreResult(head.into(), key)
            }
            _ => return None,
        })
    }
}

/// `%xx` decoding of a route segment / query value
pub(crate) fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse() {
        assert_eq!(Route::parse("/album/3"), Some(Route::Album("3".into())));
        assert_eq!(Route::parse("library"), Some(Route::Library));
        assert_eq!(Route::parse("/library/1"), Some(Route::Lib("1".into())));
        assert_eq!(
            Route::parse("/folder/2/a/b"),
            Some(Route::Folder("2".into(), "a/b".into()))
        );
        assert_eq!(
            Route::parse("/folder/2"),
            Some(Route::Folder("2".into(), String::new()))
        );
        assert_eq!(Route::parse("/nope"), None);
        assert_eq!(Route::Folder("1".into(), "x".into()).page(), Page::Folder);
    }

    #[test]
    fn parse_explore() {
        assert_eq!(Route::parse("/explore"), Some(Route::Explore));
        assert_eq!(Route::parse("/explore/places"), Some(Route::ExplorePlaces));
        assert_eq!(
            Route::parse("/explore/city/TR-34%7CKad%C4%B1k%C3%B6y"),
            Some(Route::ExploreResult("city".into(), "TR-34|Kadıköy".into()))
        );
        assert_eq!(
            Route::parse("/explore/no-location"),
            Some(Route::ExploreResult("nogps".into(), String::new()))
        );
        assert_eq!(
            Route::parse("/explore/map?focus=12&lat=41.000000&lon=29.000000"),
            Some(Route::ExploreMap {
                place: String::new(),
                focus: Some(("12".into(), "41.000000".into(), "29.000000".into()))
            })
        );
        assert_eq!(
            Route::parse("/explore/map?place=TR-34").map(|r| r.page()),
            Some(Page::ExploreMap)
        );
        assert_eq!(Route::parse("/explore/place"), None);
    }
}
