//! Trip detection (mirrors `trips.ts`) — pure and deterministic.
//!
//! Home is computed PER YEAR: the place group with the most distinct photo days (plus groups lived in
//! for a while). A trip = consecutive away days (gap ≤ 1 empty day); kept when it spans ≥ 2 days or
//! has ≥ 10 items.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::geo::{distance_km, is_valid_gps_opt};
use crate::js;
use crate::memories::{by_local_then_id, spread_pick};
use crate::time::{local_day_key, local_day_start, DAY_MS};
use crate::types::{Memory, MemoryKind};

/// Input item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TripItem {
    /// media id
    pub id: String,
    /// wall clock
    pub local_at: i64,
    /// latitude
    #[serde(default)]
    pub lat: Option<f64>,
    /// longitude
    #[serde(default)]
    pub lon: Option<f64>,
    /// PlaceInfo.key
    #[serde(default)]
    pub place_key: Option<String>,
    /// display name of the place group
    #[serde(default)]
    pub place_name: Option<String>,
    /// country
    #[serde(default)]
    pub cc: Option<String>,
}

/// Options of [`find_trips`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TripOptions {
    /// default 50
    pub min_km: Option<f64>,
    /// default 2
    pub min_days: Option<i64>,
    /// default 10
    pub min_items: Option<usize>,
    /// empty days allowed between two trip days (default 1)
    pub max_gap_days: Option<i64>,
    /// default 6
    pub covers: Option<usize>,
}

/// A home place of one year.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Home {
    /// group key
    pub key: String,
    /// median latitude
    #[serde(with = "js::num")]
    pub lat: f64,
    /// median longitude
    #[serde(with = "js::num")]
    pub lon: f64,
    /// distinct days
    pub days: i64,
}

fn median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let m = s.len() >> 1;
    if s.len() % 2 == 1 {
        s[m]
    } else {
        (s[m - 1] + s[m]) / 2.0
    }
}

fn group_key(it: &TripItem) -> String {
    match it.place_key.as_deref() {
        Some(k) if !k.is_empty() => k.to_string(),
        _ => {
            let h = |v: f64| js::num_to_string(js::round(v * 2.0) / 2.0);
            format!(
                "@{},{}",
                h(it.lat.unwrap_or(f64::NAN)),
                h(it.lon.unwrap_or(f64::NAN))
            )
        }
    }
}

struct GroupAcc {
    days: HashSet<i64>,
    lats: Vec<f64>,
    lons: Vec<f64>,
    first: i64,
    last: i64,
}

/// Home place(s) per calendar year (of localAt).
pub fn homes_by_year(items: &[TripItem]) -> BTreeMap<i64, Vec<Home>> {
    let mut years: BTreeMap<i64, HashMap<String, GroupAcc>> = BTreeMap::new();
    for it in items {
        if !is_valid_gps_opt(it.lat, it.lon) {
            continue;
        }
        let y = js::utc_year(it.local_at);
        let g = years.entry(y).or_default();
        let day = local_day_start(it.local_at);
        let e = g.entry(group_key(it)).or_insert_with(|| GroupAcc {
            days: HashSet::new(),
            lats: vec![],
            lons: vec![],
            first: day,
            last: day,
        });
        e.days.insert(day);
        e.lats.push(it.lat.unwrap_or(0.0));
        e.lons.push(it.lon.unwrap_or(0.0));
        e.first = e.first.min(day);
        e.last = e.last.max(day);
    }
    let mut out = BTreeMap::new();
    for (y, g) in years {
        let mut ranked: Vec<(&String, &GroupAcc)> = g.iter().collect();
        ranked.sort_by(|a, b| {
            b.1.days
                .len()
                .cmp(&a.1.days.len())
                .then_with(|| js::cmp16(a.0, b.0))
        });
        let top = ranked[0].1.days.len() as f64;
        let mut homes = vec![];
        for (i, (key, e)) in ranked.iter().enumerate() {
            let lived =
                e.days.len() as f64 >= 3f64.max(top / 2.0) && e.last - e.first >= 60 * DAY_MS;
            if i == 0 || lived {
                homes.push(Home {
                    key: (*key).clone(),
                    lat: median(&e.lats),
                    lon: median(&e.lons),
                    days: e.days.len() as i64,
                });
            }
        }
        out.insert(y, homes);
    }
    out
}

struct Day {
    start: i64,
    gps: i64,
    away: i64,
}

#[derive(Clone, Copy)]
struct Span {
    first: i64,
    last: i64,
    away_days: i64,
}

/// Trips, newest first.
pub fn find_trips(items: &[TripItem], opts: &TripOptions) -> Vec<Memory> {
    let min_km = opts.min_km.unwrap_or(50.0);
    let min_days = opts.min_days.unwrap_or(2);
    let min_items = opts.min_items.unwrap_or(10);
    let max_gap = opts.max_gap_days.unwrap_or(1);
    let covers = opts.covers.unwrap_or(6);
    let homes = homes_by_year(items);
    let empty: Vec<Home> = vec![];

    let known: Vec<i64> = homes
        .iter()
        .filter(|(_, h)| h[0].days >= 3)
        .map(|(y, _)| *y)
        .collect();
    let homes_for = |y: i64| -> &Vec<Home> {
        if let Some(h) = homes.get(&y) {
            if h[0].days >= 3 {
                return h;
            }
        }
        if known.is_empty() {
            return homes.get(&y).unwrap_or(&empty);
        }
        let near = known.iter().copied().fold(known[0], |best, k| {
            if (k - y).abs() < (best - y).abs() {
                k
            } else {
                best
            }
        });
        &homes[&near]
    };

    let mut sorted: Vec<&TripItem> = items.iter().collect();
    sorted.sort_by(|a, b| by_local_then_id(a.local_at, &a.id, b.local_at, &b.id));
    let mut days: Vec<Day> = vec![];
    for it in &sorted {
        let start = local_day_start(it.local_at);
        if days.last().is_none_or(|d| d.start != start) {
            days.push(Day {
                start,
                gps: 0,
                away: 0,
            });
        }
        if !is_valid_gps_opt(it.lat, it.lon) {
            continue;
        }
        let d = days.last_mut().expect("pushed above");
        d.gps += 1;
        let hs = homes_for(js::utc_year(start));
        let k = group_key(it);
        let (lat, lon) = (it.lat.unwrap_or(0.0), it.lon.unwrap_or(0.0));
        let away = !hs.is_empty()
            && hs
                .iter()
                .all(|h| h.key != k && distance_km(h.lat, h.lon, lat, lon) >= min_km);
        if away {
            d.away += 1;
        }
    }

    let mut trips: Vec<Span> = vec![];
    let mut cur: Option<Span> = None;
    for d in &days {
        if d.gps == 0 {
            continue;
        }
        let is_away = d.away * 2 >= d.gps;
        if !is_away {
            if let Some(c) = cur.take() {
                trips.push(c);
            }
            continue;
        }
        match cur.as_mut() {
            Some(c) if d.start - c.last <= (max_gap + 1) * DAY_MS => {
                c.last = d.start;
                c.away_days += 1;
            }
            _ => {
                if let Some(c) = cur.take() {
                    trips.push(c);
                }
                cur = Some(Span {
                    first: d.start,
                    last: d.start,
                    away_days: 1,
                });
            }
        }
    }
    if let Some(c) = cur {
        trips.push(c);
    }

    let mut out = vec![];
    for t in trips {
        let from = t.first;
        let to = t.last + DAY_MS;
        let inside: Vec<&&TripItem> = sorted
            .iter()
            .filter(|it| it.local_at >= from && it.local_at < to)
            .collect();
        if t.away_days < min_days && inside.len() < min_items {
            continue;
        }
        // dominant place group (insertion order breaks ties, like the stable JS sort)
        let mut tally: Vec<(String, i64, String, Option<String>)> = vec![];
        for it in &inside {
            let Some(name) = it.place_name.as_deref().filter(|n| !n.is_empty()) else {
                continue;
            };
            if !is_valid_gps_opt(it.lat, it.lon) {
                continue;
            }
            let k = group_key(it);
            match tally.iter_mut().find(|e| e.0 == k) {
                Some(e) => e.1 += 1,
                None => tally.push((k, 1, name.to_string(), it.cc.clone())),
            }
        }
        let mut top: Option<&(String, i64, String, Option<String>)> = None;
        for e in &tally {
            if top.is_none_or(|t| e.1 > t.1) {
                top = Some(e);
            }
        }
        let ids: Vec<&str> = inside.iter().map(|it| it.id.as_str()).collect();
        out.push(Memory {
            id: format!("trip-{}", local_day_key(from)),
            kind: MemoryKind::Trip,
            years_ago: None,
            place: top.map(|t| t.2.clone()),
            cc: top.and_then(|t| t.3.clone()),
            from,
            to,
            count: inside.len() as i64,
            cover_ids: spread_pick(&ids, covers)
                .into_iter()
                .map(str::to_string)
                .collect(),
            months: None,
        });
    }
    out.sort_by_key(|m| std::cmp::Reverse(m.from));
    out
}
