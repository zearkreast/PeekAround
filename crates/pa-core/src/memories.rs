//! "On this day" memories (mirrors `memories.ts`) — pure and deterministic (`now` is a parameter).

use serde::{Deserialize, Serialize};

use crate::js;
use crate::time::{local_day_start, DAY_MS};
use crate::types::{Memory, MemoryKind};

/// Input item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryItem {
    /// media id
    pub id: String,
    /// wall clock
    pub local_at: i64,
}

/// Options of [`on_this_day`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OnThisDayOptions {
    /// ± days around the month-day (default 3)
    pub window_days: Option<i64>,
    /// minimum items per year (default 3)
    pub min_items: Option<usize>,
    /// cover ids per memory (default 6)
    pub covers: Option<usize>,
    /// how many years back to look (default 30)
    pub max_years: Option<i64>,
}

/// Pick up to n ids spread evenly over the list.
pub fn spread_pick<'a>(list: &[&'a str], n: usize) -> Vec<&'a str> {
    if list.len() <= n {
        return list.to_vec();
    }
    let len = list.len() as f64;
    (0..n)
        .map(|k| list[(((k as f64 + 0.5) * len) / n as f64).floor() as usize])
        .collect()
}

fn lower_bound(items: &[&MemoryItem], v: i64) -> usize {
    items.partition_point(|x| x.local_at < v)
}

/// Sort like the TypeScript comparator `a.localAt - b.localAt || (a.id < b.id ? -1 : 1)`.
pub(crate) fn by_local_then_id(a_t: i64, a_id: &str, b_t: i64, b_id: &str) -> std::cmp::Ordering {
    a_t.cmp(&b_t).then_with(|| js::cmp16(a_id, b_id))
}

/// Items of previous years taken within ±windowDays of today's month-day. Newest year first.
/// `now_local`: current wall-clock time in the home zone (as UTC ms).
pub fn on_this_day(items: &[MemoryItem], now_local: i64, opts: &OnThisDayOptions) -> Vec<Memory> {
    let win = opts.window_days.unwrap_or(3);
    let min = opts.min_items.unwrap_or(3);
    let covers = opts.covers.unwrap_or(6);
    let mut sorted: Vec<&MemoryItem> = items.iter().collect();
    sorted.sort_by(|a, b| by_local_then_id(a.local_at, &a.id, b.local_at, &b.id));
    if sorted.is_empty() {
        return vec![];
    }
    let now = js::utc_parts(local_day_start(now_local));
    let mut out = vec![];
    for ago in 1..=opts.max_years.unwrap_or(30) {
        let y = now.year - ago;
        let mut day = js::date_utc(y, now.month0, now.day, 0, 0, 0, 0);
        if js::utc_parts(day).month0 != now.month0 {
            day = js::date_utc(y, now.month0 + 1, 0, 0, 0, 0, 0);
        }
        let from = day - win * DAY_MS;
        let to = day + (win + 1) * DAY_MS;
        if to <= sorted[0].local_at {
            break;
        }
        let a = lower_bound(&sorted, from);
        let b = lower_bound(&sorted, to);
        if b.saturating_sub(a) < min || b < a {
            continue;
        }
        let slice: Vec<&str> = sorted[a..b].iter().map(|x| x.id.as_str()).collect();
        out.push(Memory {
            id: format!("otd-{}", js::iso_day(day)),
            kind: MemoryKind::Onthisday,
            years_ago: Some(ago),
            place: None,
            cc: None,
            from,
            to,
            count: slice.len() as i64,
            cover_ids: spread_pick(&slice, covers)
                .into_iter()
                .map(str::to_string)
                .collect(),
            months: None,
        });
    }
    out
}
