//! Explore (mirrors `explore.ts` + `duplicates.ts`): automatic grouping results, cached per query
//! and invalidated by media / folder events, computed on a read-only connection off the async
//! runtime; slow results are served stale for a while (STALE_FACTOR) during indexing. Exact
//! duplicates: lazy content hashes of same-size files.

use crate::ctx::{Ctx, C};
use crate::db::{query_all, Params, SUN_NA};
use crate::events::now_ms;
use crate::geo::place_key;
use crate::p;
use crate::scope::{self, Bq};
use futures_util::future::{BoxFuture, FutureExt, Shared};
use pa_core::{
    CameraGroup, ExploreResult, InsightRow, MediaKind, PlaceGroup, ServerEvent, TypeGroup,
    YearGroup,
};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const EXPLORE_EVENT_MS: i64 = 10_000;
const MAX_TRIPS: usize = 24;
const FILL_CHUNK: i64 = 2_000;
const FILL_GAP: Duration = Duration::from_millis(25);
const STALE_FACTOR: f64 = 20.0;
const STALE_MIN_COMPUTE_MS: f64 = 250.0;
const STALE_MAX_MS: f64 = 60_000.0;

struct CacheEntry {
    r: Arc<ExploreResult>,
    gen: u64,
    at: Instant,
    ms: f64,
}

type Job = Shared<BoxFuture<'static, Result<Arc<ExploreResult>, String>>>;

/// Explore state.
#[derive(Default)]
pub struct Explore {
    cache: Mutex<HashMap<String, CacheEntry>>,
    gen: AtomicU64,
    timer: AtomicBool,
    last_event: Mutex<i64>,
    dirty: AtomicBool,
    filling: AtomicBool,
    refreshing: AtomicBool,
    computing: Mutex<HashMap<String, Job>>,
    /// last fillInsightColumns call (ms)
    pub last_fill: Mutex<i64>,
    dups_running: AtomicBool,
    dups_stopped: AtomicBool,
}

impl Explore {
    /// Mark cached results stale; `notify` also schedules a (throttled) explore event.
    pub fn invalidate(&self, ctx: &C, notify: bool) {
        self.gen.fetch_add(1, Ordering::SeqCst);
        if !notify {
            return;
        }
        self.dirty.store(true, Ordering::SeqCst);
        if self.timer.swap(true, Ordering::SeqCst) {
            return;
        }
        let last = *self.last_event.lock();
        let delay = (last + EXPLORE_EVENT_MS - now_ms()).max(1000) as u64;
        let c = ctx.clone();
        tokio_spawn(ctx, async move {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(delay)) => {}
                _ = c.shutdown.cancelled() => { return; }
            }
            let e = &c.explore;
            e.timer.store(false, Ordering::SeqCst);
            if !e.dirty.swap(false, Ordering::SeqCst) {
                return;
            }
            *e.last_event.lock() = now_ms();
            c.events.emit(ServerEvent::Explore);
        });
    }

    /// stop background work
    pub fn close(&self) {
        self.dups_stopped.store(true, Ordering::SeqCst);
    }

    /// duplicate hashing running
    pub fn dups_busy(&self) -> bool {
        self.dups_running.load(Ordering::SeqCst)
    }
}

fn tokio_spawn(ctx: &C, f: impl std::future::Future<Output = ()> + Send + 'static) {
    if let Ok(h) = tokio::runtime::Handle::try_current() {
        let _ = &ctx.tasks;
        h.spawn(f);
    }
}

fn fill_later(ctx: &C) {
    if ctx.explore.filling.swap(true, Ordering::SeqCst) {
        return;
    }
    let c = ctx.clone();
    tokio_spawn(ctx, async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(FILL_GAP) => {}
                _ = c.shutdown.cancelled() => { break; }
            }
            if c.db.is_closed() {
                break;
            }
            let c2 = c.clone();
            let done = tokio::task::spawn_blocking(move || {
                scope::fill_insight_columns(&c2, 0, FILL_CHUNK)
            })
            .await
            .unwrap_or(true);
            if done {
                c.explore.invalidate(&c, true);
                break;
            }
        }
        c.explore.filling.store(false, Ordering::SeqCst);
    });
}

fn refresh_later(ctx: &C, ms: u64) {
    if ctx.explore.refreshing.swap(true, Ordering::SeqCst) {
        return;
    }
    let c = ctx.clone();
    tokio_spawn(ctx, async move {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(ms)) => {}
            _ = c.shutdown.cancelled() => { return; }
        }
        c.explore.refreshing.store(false, Ordering::SeqCst);
        c.explore.invalidate(&c, true);
    });
}

/// `explore.get(q)`
pub async fn get(ctx: &C, q: Bq, now: i64) -> Result<ExploreResult, String> {
    let now_local = pa_core::instant_to_local(now, &ctx.scanner.home_time_zone());
    let key = format!(
        "{}|{}",
        pa_core::local_day_key(now_local),
        serde_json::to_string(&q).unwrap_or_default()
    );
    let gen_now = ctx.explore.gen.load(Ordering::SeqCst);
    {
        let cache = ctx.explore.cache.lock();
        if let Some(hit) = cache.get(&key) {
            if hit.gen != gen_now {
                let keep = if hit.ms >= STALE_MIN_COMPUTE_MS {
                    STALE_MAX_MS.min(hit.ms * STALE_FACTOR)
                } else {
                    0.0
                };
                let left = keep - hit.at.elapsed().as_secs_f64() * 1000.0;
                if left > 0.0 {
                    let r = (*hit.r).clone();
                    drop(cache);
                    refresh_later(ctx, left as u64 + 50);
                    return Ok(with_indexing(ctx, r));
                }
            } else {
                let r = (*hit.r).clone();
                drop(cache);
                return Ok(with_indexing(ctx, r));
            }
        }
    }
    let job = {
        let mut computing = ctx.explore.computing.lock();
        match computing.get(&key) {
            Some(j) => j.clone(),
            None => {
                let c = ctx.clone();
                let k2 = key.clone();
                let task = tokio::spawn(async move {
                    let t0 = Instant::now();
                    let c2 = c.clone();
                    let q2 = q.clone();
                    let res = tokio::task::spawn_blocking(move || compute(&c2, &q2, now_local))
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r);
                    if let Ok(r) = &res {
                        let mut cache = c.explore.cache.lock();
                        if cache.len() > 32 {
                            cache.clear();
                        }
                        cache.insert(
                            k2.clone(),
                            CacheEntry {
                                r: r.clone(),
                                gen: gen_now,
                                at: Instant::now(),
                                ms: t0.elapsed().as_secs_f64() * 1000.0,
                            },
                        );
                    }
                    c.explore.computing.lock().remove(&k2);
                    res
                });
                let j: Job = async move { task.await.map_err(|e| e.to_string()).and_then(|r| r) }
                    .boxed()
                    .shared();
                computing.insert(key.clone(), j.clone());
                j
            }
        }
    };
    let r = job.await?;
    Ok(with_indexing(ctx, (*r).clone()))
}

fn with_indexing(ctx: &Ctx, mut r: ExploreResult) -> ExploreResult {
    r.indexing = crate::scanner::indexing(ctx);
    r
}

struct Row {
    id: i64,
    k: String,
    t: i64,
    lat: Option<f64>,
    lon: Option<f64>,
    cc: Option<String>,
    a1: Option<String>,
    city: Option<String>,
    make: Option<String>,
    model: Option<String>,
    flags: u32,
}

/// One pass over the scope's rows (blocking; reader connection).
pub fn compute(ctx: &C, q: &Bq, now_local: i64) -> Result<Arc<ExploreResult>, String> {
    let mut eq = q.clone();
    eq.kind = Some(match q.kind.as_deref() {
        Some(k) if k != "all" => k.to_string(),
        _ => "image-video".into(),
    });
    eq.recursive = Some(true);
    let rs = scope::resolve_scope(ctx, &eq).map_err(|e| e.message)?;
    let s = rs.s;
    if !scope::fill_insight_columns(ctx, 0, FILL_CHUNK) {
        fill_later(ctx);
    }
    let cloud: std::collections::HashSet<i64> = ctx
        .db
        .all(
            "SELECT id FROM folders WHERE remote IS NOT NULL",
            p![],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?
        .into_iter()
        .collect();
    let sql = format!(
        "SELECT id, kind, taken_local, lat, lon, cc, admin1, city, make, model, flags, size, width, height, duration, color, sun, time_src, added_at, mtime, folder_id FROM media {} ORDER BY taken_local DESC, id DESC",
        scope::where_sql(&s)
    );
    let params: Params = s.params.clone();
    let (rows, irows) = ctx
        .db
        .read(|c| {
            use crate::db::{num_f, num_i, text};
            let mut rows = Vec::new();
            let mut irows = Vec::new();
            query_all(c, &sql, params, |r| {
                let id = num_i(r, 0).unwrap_or(0);
                let k = text(r, 1).unwrap_or_default();
                let t = num_i(r, 2).unwrap_or(0);
                let (lat, lon) = (num_f(r, 3), num_f(r, 4));
                let cc = text(r, 5);
                let a1 = text(r, 6);
                let city = text(r, 7);
                let flags = num_i(r, 10).unwrap_or(0) as u32;
                let sun = num_f(r, 16);
                let src = num_i(r, 17).unwrap_or(0);
                let folder_id = num_i(r, 20).unwrap_or(0);
                irows.push(InsightRow {
                    id: id.to_string(),
                    kind: k.clone(),
                    local_at: t,
                    size: num_i(r, 11).unwrap_or(0),
                    width: num_f(r, 12),
                    height: num_f(r, 13),
                    duration: num_f(r, 14),
                    lat,
                    lon,
                    place_key: cc.as_deref().map(|c| place_key(c, a1.as_deref())),
                    cc: cc.clone(),
                    city: city.clone(),
                    flags,
                    color: num_i(r, 15),
                    sun: sun.filter(|s| *s != SUN_NA),
                    clock_known: Some(src != 0),
                    added_at: num_i(r, 18),
                    mtime: num_i(r, 19).unwrap_or(0),
                    cloud: Some(!cloud.is_empty() && cloud.contains(&folder_id)),
                });
                rows.push(Row {
                    id,
                    k,
                    t,
                    lat,
                    lon,
                    cc,
                    a1,
                    city,
                    make: text(r, 8),
                    model: text(r, 9),
                    flags,
                });
                Ok(())
            })?;
            Ok((rows, irows))
        })
        .map_err(|e| e.to_string())?;
    let geo = &ctx.geo;

    // ── memories ──
    let mitems: Vec<pa_core::MemoryItem> = rows
        .iter()
        .map(|r| pa_core::MemoryItem {
            id: r.id.to_string(),
            local_at: r.t,
        })
        .collect();
    let otd = pa_core::on_this_day(&mitems, now_local, &Default::default());
    let group_name = |cc: &str, a1: Option<&str>, city: Option<&str>| {
        geo.admin1_name(cc, a1)
            .or(city.map(String::from))
            .unwrap_or_else(|| cc.to_string())
    };
    let titems: Vec<pa_core::TripItem> = rows
        .iter()
        .map(|r| pa_core::TripItem {
            id: r.id.to_string(),
            local_at: r.t,
            lat: r.lat,
            lon: r.lon,
            place_key: r
                .cc
                .as_deref()
                .filter(|c| !c.is_empty())
                .map(|c| place_key(c, r.a1.as_deref())),
            place_name: r
                .cc
                .as_deref()
                .filter(|c| !c.is_empty())
                .map(|c| group_name(c, r.a1.as_deref(), r.city.as_deref())),
            cc: r.cc.clone(),
        })
        .collect();
    let mut trips = pa_core::find_trips(&titems, &Default::default());
    trips.truncate(MAX_TRIPS);

    // ── places ──
    struct P {
        g: PlaceGroup,
        lat: f64,
        lon: f64,
        n: f64,
        cities: indexmap::IndexMap<String, i64>,
    }
    let mut places: indexmap::IndexMap<String, P> = indexmap::IndexMap::new();
    for r in &rows {
        let (Some(cc), Some(lat), Some(lon)) =
            (r.cc.as_deref().filter(|c| !c.is_empty()), r.lat, r.lon)
        else {
            continue;
        };
        let k = place_key(cc, r.a1.as_deref());
        let p = places.entry(k.clone()).or_insert_with(|| P {
            g: PlaceGroup {
                key: k,
                cc: cc.to_string(),
                name: String::new(),
                city: None,
                count: 0,
                cover_id: r.id.to_string(),
                lat: 0.0,
                lon: 0.0,
            },
            lat: 0.0,
            lon: 0.0,
            n: 0.0,
            cities: indexmap::IndexMap::new(),
        });
        p.g.count += 1;
        p.lat += lat;
        p.lon += lon;
        p.n += 1.0;
        if let Some(c) = r.city.as_deref().filter(|c| !c.is_empty()) {
            *p.cities.entry(c.to_string()).or_insert(0) += 1;
        }
    }
    let mut place_list: Vec<PlaceGroup> = places
        .into_values()
        .map(|p| {
            let mut city: Option<String> = None;
            let mut best = 0;
            for (c, n) in &p.cities {
                if *n > best {
                    city = Some(c.clone());
                    best = *n;
                }
            }
            let a1 = p.g.key[p.g.cc.len() + 1..].to_string();
            let name = geo
                .admin1_name(&p.g.cc, if a1 == "00" { None } else { Some(&a1) })
                .or(city.clone())
                .unwrap_or_else(|| p.g.cc.clone());
            PlaceGroup {
                name,
                city,
                lat: pa_core::js::round(p.lat / p.n * 1e5) / 1e5,
                lon: pa_core::js::round(p.lon / p.n * 1e5) / 1e5,
                ..p.g
            }
        })
        .collect();
    place_list.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| pa_core::locale_compare(&a.name, &b.name))
    });

    // ── types ──
    use pa_core::media_flag as F;
    use pa_core::MediaFlagName as N;
    let tests: [(N, Box<dyn Fn(&Row) -> bool>); 5] = [
        (N::Video, Box::new(|r: &Row| r.k == "video")),
        (N::Selfie, Box::new(|r: &Row| r.flags & F::SELFIE != 0)),
        (
            N::Screenshot,
            Box::new(|r: &Row| r.flags & F::SCREENSHOT != 0),
        ),
        (N::Panorama, Box::new(|r: &Row| r.flags & F::PANORAMA != 0)),
        (N::Animated, Box::new(|r: &Row| r.flags & F::ANIMATED != 0)),
    ];
    let mut types = Vec::new();
    for (flag, test) in tests.iter() {
        let mut count = 0;
        let mut cover = String::new();
        for r in &rows {
            if !test(r) {
                continue;
            }
            if count == 0 {
                cover = r.id.to_string();
            }
            count += 1;
        }
        if count > 0 {
            types.push(TypeGroup {
                flag: *flag,
                count,
                cover_id: cover,
            });
        }
    }

    // ── cameras ──
    let mut cams: indexmap::IndexMap<String, CameraGroup> = indexmap::IndexMap::new();
    for r in &rows {
        let Some(model) = r.model.as_deref().filter(|m| !m.is_empty()) else {
            continue;
        };
        let k = pa_core::camera_key(r.make.as_deref(), model);
        match cams.get_mut(&k) {
            Some(c) => c.count += 1,
            None => {
                cams.insert(
                    k.clone(),
                    CameraGroup {
                        key: k,
                        make: r.make.clone(),
                        model: model.to_string(),
                        count: 1,
                        cover_id: r.id.to_string(),
                    },
                );
            }
        }
    }
    let mut cameras: Vec<CameraGroup> = cams.into_values().collect();
    cameras.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| pa_core::locale_compare(&a.model, &b.model))
    });

    // ── years ──
    let mut years: indexmap::IndexMap<i64, YearGroup> = indexmap::IndexMap::new();
    for r in &rows {
        let y = pa_core::js::utc_year(r.t);
        match years.get_mut(&y) {
            Some(g) => g.count += 1,
            None => {
                years.insert(
                    y,
                    YearGroup {
                        year: y,
                        count: 1,
                        cover_id: r.id.to_string(),
                    },
                );
            }
        }
    }
    let mut years: Vec<YearGroup> = years.into_values().collect();
    years.sort_by_key(|y| std::cmp::Reverse(y.year));

    // ── insights ──
    let mut extras = pa_core::compute_insights(&irows, &Default::default());
    let dups = summary(ctx, &scope::where_sql(&s), &s.params);
    if dups.pending > 0 {
        kick_dups(ctx);
    }
    extras.duplicates = if dups.groups != 0 || dups.pending != 0 {
        Some(dups)
    } else {
        None
    };
    let mut memories = otd;
    memories.extend(trips);
    let _ = MediaKind::Image;
    Ok(Arc::new(ExploreResult {
        extras,
        memories,
        places: place_list,
        types,
        cameras,
        years,
        indexing: pa_core::Progress { done: 0, total: 0 },
    }))
}

// ───────────────────────── duplicates ─────────────────────────

const FULL_HASH_MAX: u64 = 256 * 1024 * 1024;
const SAMPLE: u64 = 4 * 1024 * 1024;
const BATCH: i64 = 24;

const LOCAL_FOLDERS: &str = "SELECT id FROM folders WHERE temp = 0 AND device_id IS NULL AND path NOT LIKE 'remote://%' AND path NOT LIKE 'device://%'";

fn candidates() -> String {
    format!(
        "FROM media WHERE chash IS NULL AND size > 0 AND kind IN ('image','video') AND folder_id IN ({LOCAL_FOLDERS})
  AND size IN (SELECT size FROM media INDEXED BY media_size WHERE size > 0 AND kind IN ('image','video') GROUP BY size HAVING count(*) > 1)"
    )
}

/// sha1 of the file (≤ 256 MiB) or of size + three 4 MiB samples
pub fn content_hash(file: &std::path::Path, size: u64) -> std::io::Result<String> {
    use sha1::{Digest, Sha1};
    use std::io::{Read, Seek, SeekFrom};
    let mut h = Sha1::new();
    let mut f = std::fs::File::open(file)?;
    if size <= FULL_HASH_MAX {
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        return Ok(hex::encode(h.finalize()));
    }
    h.update(format!("size:{size}").as_bytes());
    let mut buf = vec![0u8; SAMPLE as usize];
    let mid = (size as f64 / 2.0 - SAMPLE as f64 / 2.0).floor().max(0.0) as u64;
    for at in [0, mid, size.saturating_sub(SAMPLE)] {
        f.seek(SeekFrom::Start(at))?;
        let mut read = 0;
        while read < buf.len() {
            let n = f.read(&mut buf[read..])?;
            if n == 0 {
                break;
            }
            read += n;
        }
        h.update(&buf[..read]);
    }
    Ok(format!("s{}", hex::encode(h.finalize())))
}

/// candidates still waiting for a hash
pub fn pending(ctx: &Ctx) -> i64 {
    ctx.db
        .int(&format!("SELECT count(*) c {}", candidates()), p![])
        .ok()
        .flatten()
        .unwrap_or(0)
}

/// start hashing candidates if idle
pub fn kick_dups(ctx: &C) {
    let e = &ctx.explore;
    if e.dups_stopped.load(Ordering::SeqCst) || e.dups_running.swap(true, Ordering::SeqCst) {
        return;
    }
    let c = ctx.clone();
    ctx.tasks.spawn(async move {
        run_dups(&c).await;
        c.explore.dups_running.store(false, Ordering::SeqCst);
    });
}

async fn run_dups(ctx: &C) {
    let stopped = || ctx.explore.dups_stopped.load(Ordering::SeqCst) || ctx.scanner.stopped();
    let mut wrote = 0;
    let mut last_notify = Instant::now();
    loop {
        if stopped() {
            break;
        }
        while !stopped() && ctx.scanner.busy() {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(1500)) => {}
                _ = ctx.shutdown.cancelled() => {}
            }
        }
        if stopped() {
            break;
        }
        let rows = ctx
            .db
            .all(
                &format!(
                    "SELECT id, folder_id, dir, name, size {} ORDER BY size DESC LIMIT {BATCH}",
                    candidates()
                ),
                p![],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        crate::db::text(r, 2).unwrap_or_default(),
                        crate::db::text(r, 3).unwrap_or_default(),
                        r.get::<_, i64>(4)?,
                    ))
                },
            )
            .unwrap_or_default();
        if rows.is_empty() {
            break;
        }
        for (id, folder_id, dir, name, size) in rows {
            if stopped() {
                break;
            }
            let c2 = ctx.clone();
            let hash = ctx
                .thumbs
                .bg
                .run(async move {
                    if c2.explore.dups_stopped.load(Ordering::SeqCst) {
                        return None;
                    }
                    let Ok(folder) = crate::library::folder_row(&c2, folder_id) else {
                        return Some(format!("x:{id}"));
                    };
                    let a = crate::scanner::abs(&folder, &dir, &name);
                    let h = tokio::task::spawn_blocking(move || {
                        let st = std::fs::metadata(&a).ok()?;
                        if st.len() as i64 != size {
                            return None;
                        }
                        content_hash(std::path::Path::new(&a), st.len()).ok()
                    })
                    .await
                    .ok()
                    .flatten();
                    Some(h.unwrap_or_else(|| format!("x:{id}")))
                })
                .await;
            let Some(hash) = hash else { break };
            if stopped() {
                break;
            }
            let _ = ctx.db.run(
                "UPDATE media SET chash = ? WHERE id = ? AND size = ?",
                p![hash, id, size],
            );
            wrote += 1;
        }
        if wrote > 0 && last_notify.elapsed() > Duration::from_secs(15) {
            last_notify = Instant::now();
            wrote = 0;
            ctx.explore.invalidate(ctx, true);
        }
    }
    if wrote > 0 {
        ctx.explore.invalidate(ctx, true);
    }
}

fn scoped(scope_where: &str) -> String {
    if scope_where.is_empty() {
        "WHERE".into()
    } else {
        format!("{scope_where} AND")
    }
}

/// summary for the Explore card
pub fn summary(ctx: &Ctx, scope_where: &str, params: &Params) -> pa_core::DuplicateSummary {
    let rows = ctx
        .db
        .all(
            &format!(
                "SELECT chash, count(*) n, max(size) size, max(id) cover FROM media INDEXED BY media_chash {} chash IS NOT NULL AND chash < 'x'
       GROUP BY chash HAVING count(*) > 1 ORDER BY max(size) * (count(*) - 1) DESC",
                scoped(scope_where)
            ),
            params.clone(),
            |r| Ok((r.get::<_, i64>(1)?, crate::db::num_i(r, 2).unwrap_or(0), crate::db::num_i(r, 3).unwrap_or(0))),
        )
        .unwrap_or_default();
    let mut files = 0;
    let mut wasted = 0;
    for (n, size, _) in &rows {
        files += n;
        wasted += size * (n - 1);
    }
    pa_core::DuplicateSummary {
        groups: rows.len() as i64,
        files,
        wasted,
        pending: pending(ctx),
        cover_ids: rows.iter().take(4).map(|r| r.2.to_string()).collect(),
    }
}

/// duplicate groups (biggest waste first) of a scope
pub fn groups(
    ctx: &Ctx,
    scope_where: &str,
    params: &Params,
    offset: usize,
    limit: usize,
) -> (Vec<pa_core::DuplicateGroup>, i64) {
    let all = ctx
        .db
        .all(
            &format!(
                "SELECT chash, max(size) size, group_concat(id) ids FROM media INDEXED BY media_chash {} chash IS NOT NULL AND chash < 'x'
       GROUP BY chash HAVING count(*) > 1 ORDER BY max(size) * (count(*) - 1) DESC, chash",
                scoped(scope_where)
            ),
            params.clone(),
            |r| Ok((crate::db::text(r, 0).unwrap_or_default(), crate::db::num_i(r, 1).unwrap_or(0), crate::db::text(r, 2).unwrap_or_default())),
        )
        .unwrap_or_default();
    let total = all.len() as i64;
    let page = all
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(hash, size, ids)| {
            let mut v: Vec<i64> = ids.split(',').filter_map(|s| s.parse().ok()).collect();
            v.sort();
            pa_core::DuplicateGroup {
                hash,
                size,
                ids: v.into_iter().map(|i| i.to_string()).collect(),
            }
        })
        .collect();
    (page, total)
}
