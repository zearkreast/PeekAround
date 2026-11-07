//! Explore "insights" (mirrors `insights.ts`).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::color::{ColorBucket, COLOR_BUCKETS};
use crate::js;
use crate::memories::spread_pick;
use crate::sun::{is_night_hour, GOLDEN_MAX, GOLDEN_MIN, NIGHT_MAX};
use crate::types::{Memory, MemoryKind};

/// Orientation bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Orientation {
    /// taller than wide
    Portrait,
    /// wider than tall
    Landscape,
    /// within 5 %
    Square,
    /// panorama flag
    Panorama,
}

/// `ORIENTATIONS`
pub const ORIENTATIONS: [Orientation; 4] = [
    Orientation::Portrait,
    Orientation::Landscape,
    Orientation::Square,
    Orientation::Panorama,
];

/// Video length bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoLength {
    /// < 15 s
    Short,
    /// > 5 min
    Long,
}

/// `VIDEO_LENGTHS`
pub const VIDEO_LENGTHS: [VideoLength; 2] = [VideoLength::Short, VideoLength::Long];

/// Night shots / golden hour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DayTime {
    /// night
    Night,
    /// golden hour
    Golden,
}

/// `DAY_TIMES`
pub const DAY_TIMES: [DayTime; 2] = [DayTime::Night, DayTime::Golden];

/// short clips: < 15 s
pub const SHORT_VIDEO_S: f64 = 15.0;
/// long videos: > 5 min
pub const LONG_VIDEO_S: f64 = 300.0;
/// a year gets a "year in review" memory from this many items on
pub const YEAR_REVIEW_MIN: usize = 12;

const PANORAMA_FLAG: u32 = 4;

/// orientation bucket (same rules as the server's SQL filter)
pub fn orientation_of(w: Option<f64>, h: Option<f64>, flags: u32) -> Option<Orientation> {
    if flags & PANORAMA_FLAG != 0 {
        return Some(Orientation::Panorama);
    }
    let (w, h) = (w.unwrap_or(0.0), h.unwrap_or(0.0));
    // `!w` is also true for NaN
    if w.is_nan() || h.is_nan() || w == 0.0 || h == 0.0 || w <= 0.0 || h <= 0.0 {
        return None;
    }
    if (w - h).abs() <= 0.05 * w.max(h) {
        return Some(Orientation::Square);
    }
    Some(if h > w {
        Orientation::Portrait
    } else {
        Orientation::Landscape
    })
}

/// short / long video bucket
pub fn video_length_of(duration: Option<f64>) -> Option<VideoLength> {
    let d = duration?;
    if !d.is_finite() || d <= 0.0 {
        return None;
    }
    if d < SHORT_VIDEO_S {
        Some(VideoLength::Short)
    } else if d > LONG_VIDEO_S {
        Some(VideoLength::Long)
    } else {
        None
    }
}

/// night / golden hour of one item (sun altitude when known, else night by the wall clock)
pub fn day_time_of(sun: Option<f64>, local_at: i64, clock_known: bool) -> Option<DayTime> {
    if let Some(s) = sun.filter(|s| s.is_finite()) {
        if s < NIGHT_MAX {
            return Some(DayTime::Night);
        }
        if (GOLDEN_MIN..=GOLDEN_MAX).contains(&s) {
            return Some(DayTime::Golden);
        }
        return None;
    }
    if !clock_known {
        return None;
    }
    if is_night_hour(js::utc_parts(local_at).hour) {
        Some(DayTime::Night)
    } else {
        None
    }
}

// ───────────────────────── result types ─────────────────────────

/// Stats header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExploreStats {
    /// images
    pub photos: i64,
    /// videos
    pub videos: i64,
    /// distinct countries
    pub countries: i64,
    /// distinct cities
    pub cities: i64,
    /// first capture year
    pub first_year: Option<i64>,
    /// last capture year
    pub last_year: Option<i64>,
    /// total bytes (local + cloud)
    pub bytes: i64,
    /// part of `bytes` in cloud folders
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_bytes: Option<i64>,
    /// items with a location
    pub with_location: i64,
}

/// One facet value with its covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FacetGroup<T> {
    /// value
    pub value: T,
    /// items
    pub count: i64,
    /// cover
    pub cover_id: String,
    /// a few more covers (newest first)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_ids: Option<Vec<String>>,
}

/// Country group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CountryGroup {
    /// country code
    pub cc: String,
    /// items
    pub count: i64,
    /// cover
    pub cover_id: String,
    /// distinct admin1 regions
    pub regions: i64,
    /// distinct cities
    pub cities: i64,
    /// centre
    #[serde(with = "js::num")]
    pub lat: f64,
    /// centre
    #[serde(with = "js::num")]
    pub lon: f64,
}

/// City group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CityGroup {
    /// `${placeKey}|${city}`
    pub key: String,
    /// country
    pub cc: String,
    /// PlaceGroup key
    pub place_key: String,
    /// name
    pub name: String,
    /// items
    pub count: i64,
    /// cover
    pub cover_id: String,
    /// centre
    #[serde(with = "js::num")]
    pub lat: f64,
    /// centre
    #[serde(with = "js::num")]
    pub lon: f64,
}

/// One item of a ranked preview row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RankedItem {
    /// media id
    pub id: String,
    /// bytes / added time
    pub value: i64,
}

/// Duplicates summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateSummary {
    /// groups
    pub groups: i64,
    /// files in them
    pub files: i64,
    /// bytes that could be freed
    pub wasted: i64,
    /// candidates waiting to be hashed
    pub pending: i64,
    /// covers
    pub cover_ids: Vec<String>,
}

/// Optional Explore sections (`ExploreExtras`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExploreExtras {
    /// stats header
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<ExploreStats>,
    /// year in review memories, newest first
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year_reviews: Option<Vec<Memory>>,
    /// countries
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub countries: Option<Vec<CountryGroup>>,
    /// cities
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cities: Option<Vec<CityGroup>>,
    /// night / golden hour
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub day_times: Option<Vec<FacetGroup<DayTime>>>,
    /// colours
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub colors: Option<Vec<FacetGroup<ColorBucket>>>,
    /// orientations
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orientations: Option<Vec<FacetGroup<Orientation>>>,
    /// video lengths
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub video_lengths: Option<Vec<FacetGroup<VideoLength>>>,
    /// largest files
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub largest: Option<Vec<RankedItem>>,
    /// recently added
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent: Option<Vec<RankedItem>>,
    /// years that have items, newest first
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_years: Option<Vec<i64>>,
    /// image/video items without a location
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_location: Option<i64>,
    /// duplicates (the server appends it after the computed insights)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicates: Option<DuplicateSummary>,
}

/// Calendar heatmap of one year.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarYear {
    /// year
    pub year: i64,
    /// items per day of the year (index 0 = Jan 1)
    pub days: Vec<i64>,
    /// items
    pub total: i64,
    /// busiest day
    pub max: i64,
}

/// A group of identical files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuplicateGroup {
    /// content hash
    pub hash: String,
    /// bytes per file
    pub size: i64,
    /// media ids
    pub ids: Vec<String>,
}

/// Metadata status of one library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaLibraryStatus {
    /// library id
    pub id: String,
    /// name
    pub name: String,
    /// items
    pub total: i64,
    /// with a location
    pub with_location: i64,
    /// read, no location
    pub without_location: i64,
    /// waiting for (re-)reading
    pub pending: i64,
    /// unreadable
    pub unreadable: i64,
    /// messaging app files
    pub messaging: i64,
    /// location from Takeout sidecars
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidecar: Option<i64>,
}

/// `GET /api/meta/status`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetaStatus {
    /// libraries
    pub libraries: Vec<MetaLibraryStatus>,
    /// enrichment running
    pub enriching: bool,
    /// progress
    pub indexing: crate::types::Progress,
}

// ───────────────────────── aggregation ─────────────────────────

/// One image/video row for [`compute_insights`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct InsightRow {
    /// media id
    pub id: String,
    /// 'image' / 'video'
    pub kind: String,
    /// wall clock
    pub local_at: i64,
    /// bytes
    pub size: i64,
    /// px
    pub width: Option<f64>,
    /// px
    pub height: Option<f64>,
    /// seconds
    pub duration: Option<f64>,
    /// latitude
    pub lat: Option<f64>,
    /// longitude
    pub lon: Option<f64>,
    /// country
    pub cc: Option<String>,
    /// PlaceGroup key
    pub place_key: Option<String>,
    /// city
    pub city: Option<String>,
    /// MEDIA_FLAG bits
    pub flags: u32,
    /// index into COLOR_BUCKETS, -1/None = unknown
    #[serde(default)]
    pub color: Option<i64>,
    /// sun altitude
    #[serde(default)]
    pub sun: Option<f64>,
    /// capture clock known (default true)
    #[serde(default)]
    pub clock_known: Option<bool>,
    /// added to the library (ms)
    #[serde(default)]
    pub added_at: Option<i64>,
    /// mtime
    pub mtime: i64,
    /// lives in a cloud folder
    #[serde(default)]
    pub cloud: Option<bool>,
}

/// Options of [`compute_insights`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct InsightOptions {
    /// compute colours (default true)
    pub colors: Option<bool>,
    /// compute day times (default true)
    pub day_times: Option<bool>,
    /// covers per facet (default 4)
    pub covers: Option<usize>,
    /// ranked list length (default 20)
    pub ranked: Option<usize>,
    /// max cities (default 800)
    pub max_cities: Option<usize>,
}

fn top_k(list: &mut Vec<RankedItem>, k: usize, it: RankedItem) {
    if k == 0 {
        return;
    }
    if list.len() < k {
        list.push(it.clone());
    } else if it.value <= list[k - 1].value {
        return;
    } else {
        list[k - 1] = it.clone();
    }
    let mut i = list.len() - 1;
    while i > 0 && list[i - 1].value < it.value {
        list[i] = list[i - 1].clone();
        i -= 1;
    }
    list[i] = it;
}

/// Locale string comparison like `String.prototype.localeCompare` (ICU root collation).
pub fn locale_compare(a: &str, b: &str) -> std::cmp::Ordering {
    use icu_collator::options::CollatorOptions;
    use icu_collator::{Collator, CollatorBorrowed};
    use std::sync::OnceLock;
    static C: OnceLock<Option<CollatorBorrowed<'static>>> = OnceLock::new();
    let c =
        C.get_or_init(|| Collator::try_new(Default::default(), CollatorOptions::default()).ok());
    match c {
        Some(c) => c.compare(a, b),
        None => a.cmp(b),
    }
}

struct FacetMap<T> {
    groups: HashMap<T, FacetGroup<T>>,
}

impl<T: Copy + Eq + std::hash::Hash> FacetMap<T> {
    fn new() -> Self {
        FacetMap {
            groups: HashMap::new(),
        }
    }
    fn add(&mut self, v: T, id: &str, covers: usize) {
        match self.groups.get_mut(&v) {
            None => {
                self.groups.insert(
                    v,
                    FacetGroup {
                        value: v,
                        count: 1,
                        cover_id: id.into(),
                        cover_ids: Some(vec![id.into()]),
                    },
                );
            }
            Some(g) => {
                g.count += 1;
                let ids = g.cover_ids.get_or_insert_with(Vec::new);
                if ids.len() < covers {
                    ids.push(id.into());
                }
            }
        }
    }
    fn ordered(mut self, order: &[T]) -> Vec<FacetGroup<T>> {
        order.iter().filter_map(|v| self.groups.remove(v)).collect()
    }
}

struct CountryAcc {
    g: CountryGroup,
    la: f64,
    lo: f64,
    regions: HashSet<String>,
    cities: HashSet<String>,
}

struct CityAcc {
    g: CityGroup,
    la: f64,
    lo: f64,
}

fn round5(v: f64) -> f64 {
    js::round(v * 1e5) / 1e5
}

/// rows: images + videos, newest (localAt) first
pub fn compute_insights(rows: &[InsightRow], opts: &InsightOptions) -> ExploreExtras {
    let covers = opts.covers.unwrap_or(4);
    let ranked = opts.ranked.unwrap_or(20);
    let do_day_times = opts.day_times != Some(false);
    let do_colors = opts.colors != Some(false);
    let mut stats = ExploreStats {
        cloud_bytes: Some(0),
        ..Default::default()
    };
    let mut cloud_bytes = 0i64;
    let mut day_times = FacetMap::new();
    let mut colors = FacetMap::new();
    let mut orients = FacetMap::new();
    let mut lengths = FacetMap::new();
    let mut countries: Vec<CountryAcc> = vec![];
    let mut country_idx: HashMap<String, usize> = HashMap::new();
    let mut cities: Vec<CityAcc> = vec![];
    let mut city_idx: HashMap<String, usize> = HashMap::new();
    let mut years: Vec<(i64, Vec<&InsightRow>)> = vec![];
    let mut year_idx: HashMap<i64, usize> = HashMap::new();
    let mut largest = vec![];
    let mut recent = vec![];
    let mut no_location = 0;

    for r in rows {
        if r.kind == "video" {
            stats.videos += 1;
        } else {
            stats.photos += 1;
        }
        stats.bytes += r.size;
        if r.cloud == Some(true) {
            cloud_bytes += r.size;
        }
        let y = js::utc_year(r.local_at);
        let yi = *year_idx.entry(y).or_insert_with(|| {
            years.push((y, vec![]));
            years.len() - 1
        });
        years[yi].1.push(r);

        let has_gps = r.lat.is_some() && r.lon.is_some();
        if has_gps {
            stats.with_location += 1;
        } else {
            no_location += 1;
        }
        if let (Some(cc), true) = (r.cc.as_deref().filter(|c| !c.is_empty()), has_gps) {
            let (lat, lon) = (r.lat.unwrap_or(0.0), r.lon.unwrap_or(0.0));
            let ci = *country_idx.entry(cc.to_string()).or_insert_with(|| {
                countries.push(CountryAcc {
                    g: CountryGroup {
                        cc: cc.into(),
                        count: 0,
                        cover_id: r.id.clone(),
                        regions: 0,
                        cities: 0,
                        lat: 0.0,
                        lon: 0.0,
                    },
                    la: 0.0,
                    lo: 0.0,
                    regions: HashSet::new(),
                    cities: HashSet::new(),
                });
                countries.len() - 1
            });
            let c = &mut countries[ci];
            c.g.count += 1;
            c.la += lat;
            c.lo += lon;
            if let Some(pk) = r.place_key.as_deref().filter(|p| !p.is_empty()) {
                c.regions.insert(pk.to_string());
            }
            if let Some(city) = r.city.as_deref().filter(|p| !p.is_empty()) {
                c.cities.insert(city.to_string());
                let pk = r.place_key.clone().unwrap_or_else(|| format!("{cc}-00"));
                let key = format!("{pk}|{city}");
                let ki = *city_idx.entry(key.clone()).or_insert_with(|| {
                    cities.push(CityAcc {
                        g: CityGroup {
                            key: key.clone(),
                            cc: cc.into(),
                            place_key: pk.clone(),
                            name: city.into(),
                            count: 0,
                            cover_id: r.id.clone(),
                            lat: 0.0,
                            lon: 0.0,
                        },
                        la: 0.0,
                        lo: 0.0,
                    });
                    cities.len() - 1
                });
                let ci = &mut cities[ki];
                ci.g.count += 1;
                ci.la += lat;
                ci.lo += lon;
            }
        }

        if do_day_times {
            if let Some(dt) = day_time_of(r.sun, r.local_at, r.clock_known.unwrap_or(true)) {
                day_times.add(dt, &r.id, covers);
            }
        }
        if do_colors {
            if let Some(c) = r
                .color
                .filter(|c| *c >= 0 && (*c as usize) < COLOR_BUCKETS.len())
            {
                colors.add(COLOR_BUCKETS[c as usize], &r.id, covers);
            }
        }
        if let Some(o) = orientation_of(r.width, r.height, r.flags) {
            orients.add(o, &r.id, covers);
        }
        if r.kind == "video" {
            if let Some(l) = video_length_of(r.duration) {
                lengths.add(l, &r.id, covers);
            }
        }
        if r.size > 0 {
            top_k(
                &mut largest,
                ranked,
                RankedItem {
                    id: r.id.clone(),
                    value: r.size,
                },
            );
        }
        top_k(
            &mut recent,
            ranked,
            RankedItem {
                id: r.id.clone(),
                value: r.added_at.unwrap_or(r.mtime),
            },
        );
    }

    let mut year_list: Vec<i64> = years.iter().map(|(y, _)| *y).collect();
    year_list.sort_by(|a, b| b.cmp(a));
    stats.first_year = year_list.last().copied();
    stats.last_year = year_list.first().copied();
    stats.countries = countries.len() as i64;
    stats.cloud_bytes = Some(cloud_bytes);
    stats.cities = countries.iter().map(|c| c.cities.len() as i64).sum();

    let year_reviews = year_list
        .iter()
        .filter_map(|y| {
            let items: Vec<(&str, i64)> = years[year_idx[y]]
                .1
                .iter()
                .map(|r| (r.id.as_str(), r.local_at))
                .collect();
            year_review(*y, &items, YEAR_REVIEW_MIN)
        })
        .collect();

    let mut country_list: Vec<CountryGroup> = countries
        .into_iter()
        .map(|c| {
            let n = c.g.count as f64;
            CountryGroup {
                regions: c.regions.len() as i64,
                cities: c.cities.len() as i64,
                lat: round5(c.la / n),
                lon: round5(c.lo / n),
                ..c.g
            }
        })
        .collect();
    country_list.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| locale_compare(&a.cc, &b.cc))
    });
    let mut city_list: Vec<CityGroup> = cities
        .into_iter()
        .map(|c| {
            let n = c.g.count as f64;
            CityGroup {
                lat: round5(c.la / n),
                lon: round5(c.lo / n),
                ..c.g
            }
        })
        .collect();
    city_list.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| locale_compare(&a.name, &b.name))
    });
    city_list.truncate(opts.max_cities.unwrap_or(800));

    ExploreExtras {
        stats: Some(stats),
        year_reviews: Some(year_reviews),
        countries: Some(country_list),
        cities: Some(city_list),
        day_times: if do_day_times {
            Some(day_times.ordered(&DAY_TIMES))
        } else {
            None
        },
        colors: if do_colors {
            Some(colors.ordered(&COLOR_BUCKETS))
        } else {
            None
        },
        orientations: Some(orients.ordered(&ORIENTATIONS)),
        video_lengths: Some(lengths.ordered(&VIDEO_LENGTHS)),
        largest: Some(largest),
        recent: Some(recent),
        calendar_years: Some(year_list),
        no_location: Some(no_location),
        duplicates: None,
    }
}

/// "Year in review": the whole calendar year, covers spread over the months.
/// `items`: (id, localAt) of that year, newest first.
pub fn year_review(year: i64, items: &[(&str, i64)], min: usize) -> Option<Memory> {
    if items.len() < min {
        return None;
    }
    let mut months: Vec<Vec<(&str, i64)>> = vec![vec![]; 12];
    for it in items {
        months[js::utc_parts(it.1).month0 as usize].push(*it);
    }
    let filled: Vec<&Vec<(&str, i64)>> = months.iter().filter(|m| !m.is_empty()).collect();
    let want = 12.min(6.max(filled.len()));
    let per = 1.max(want.checked_div(filled.len()).unwrap_or(usize::MAX));
    let mut covers: Vec<String> = vec![];
    for m in filled.iter().rev() {
        if covers.len() >= want {
            break;
        }
        let ids: Vec<&str> = m.iter().map(|x| x.0).collect();
        let picked = spread_pick(&ids, per);
        let room = want - covers.len();
        covers.extend(picked.into_iter().take(room).map(str::to_string));
    }
    Some(Memory {
        id: format!("year-{year}"),
        kind: MemoryKind::Year,
        years_ago: None,
        place: None,
        cc: None,
        from: js::date_utc(year, 0, 1, 0, 0, 0, 0),
        to: js::date_utc(year + 1, 0, 1, 0, 0, 0, 0),
        count: items.len() as i64,
        cover_ids: covers,
        months: Some(months.iter().map(|m| m.len() as i64).collect()),
    })
}

/// items per day of `year` from localAt values
pub fn calendar_of(year: i64, local_ats: impl IntoIterator<Item = i64>) -> CalendarYear {
    let start = js::date_utc(year, 0, 1, 0, 0, 0, 0);
    let n = ((js::date_utc(year + 1, 0, 1, 0, 0, 0, 0) - start) / js::DAY_MS) as usize;
    let mut days = vec![0i64; n];
    let mut total = 0;
    for t in local_ats {
        let d = (t - start).div_euclid(js::DAY_MS);
        if d < 0 || d as usize >= n {
            continue;
        }
        days[d as usize] += 1;
        total += 1;
    }
    let max = days.iter().copied().max().unwrap_or(0).max(0);
    CalendarYear {
        year,
        days,
        total,
        max,
    }
}

/// "2024-03-05" of day index `d` of `year`
pub fn calendar_day_key(year: i64, d: i64) -> String {
    js::iso_day(js::date_utc(year, 0, 1, 0, 0, 0, 0) + d * js::DAY_MS)
}
