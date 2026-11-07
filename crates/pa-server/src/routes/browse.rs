//! Browse routes (mirrors `routes/browse.ts`): folder pages, the exact layout skeleton, geo
//! points, the timeline and search.

use crate::ctx::{Ctx, C};
use crate::db::{query_all, FolderRow, ITEM_COLS};
use crate::error::{ApiResult, HttpError};
use crate::http::{ok_json, BodyScope, Req};
use crate::p;
use crate::router::{h, Router};
use crate::scope::{self, parse_browse_query, resolve_scope, where_sql, Bq, Scope};
use crate::security::Limit::Global;
use axum::body::Body;
use axum::http::{header, HeaderValue, Response};
use pa_core::{BrowseResult, Crumb, DirTarget, GeoPoint, GeoPoints, MediaItem, SubFolder};
use std::sync::Arc;

/// parse + album device visibility of the asking viewer
pub fn query_of(ctx: &Ctx, req: &Req) -> ApiResult<Bq> {
    let mut q = parse_browse_query(&req.query)?;
    if q.album_id.is_some() && crate::devices::hide_device_media(ctx, req) {
        q.no_devices = true;
    }
    Ok(q)
}

/// skeleton JSON (cached per media generation)
pub async fn build_skeleton(ctx: &C, q: &Bq) -> ApiResult<Arc<Vec<u8>>> {
    let mut kq = q.clone();
    kq.offset = Some(0.0);
    kq.limit = Some(0.0);
    let key = format!(
        "{}|{}",
        ctx.events.gen(),
        serde_json::to_string(&kq).unwrap_or_default()
    );
    if let Some((_, v)) = ctx.skeletons.lock().iter().find(|(k, _)| *k == key) {
        return Ok(v.clone());
    }
    let c = ctx.clone();
    let q2 = q.clone();
    let v = tokio::task::spawn_blocking(move || compute_skeleton(&c, &q2))
        .await
        .map_err(HttpError::internal)??;
    let v = Arc::new(v);
    let mut cache = ctx.skeletons.lock();
    if cache.len() >= 8 {
        cache.pop_front();
    }
    cache.push_back((key, v.clone()));
    Ok(v)
}

fn compute_skeleton(ctx: &Ctx, q: &Bq) -> ApiResult<Vec<u8>> {
    let rs = resolve_scope(ctx, q)?;
    let s = rs.s;
    let with_days = q.sort.as_deref().unwrap_or("takenAt") == "takenAt"
        && q.dup.is_none()
        && q.by_added.is_none()
        && !(q.album_id.is_some() && q.by_position == Some(true));
    let sql = format!(
        "SELECT id, width, height, kind{} FROM media {} {}",
        if with_days { ", taken_local, city" } else { "" },
        where_sql(&s),
        scope::order_sql(q)
    );
    struct R {
        id: i64,
        w: Option<f64>,
        h: Option<f64>,
        k: u8,
        t: i64,
        c: Option<String>,
    }
    let rows: Vec<R> = ctx.db.read(|c| {
        query_all(c, &sql, s.params.clone(), |r| {
            use crate::db::{num_f, num_i};
            let k = match r.get_ref(3)? {
                rusqlite::types::ValueRef::Text(t) => match t {
                    b"image" => 1,
                    b"video" => 2,
                    b"audio" => 3,
                    _ => 0,
                },
                _ => 0,
            };
            Ok(R {
                id: num_i(r, 0).unwrap_or(0),
                w: num_f(r, 1),
                h: num_f(r, 2),
                k,
                t: if with_days {
                    num_i(r, 4).unwrap_or(0)
                } else {
                    0
                },
                c: if with_days {
                    match r.get_ref(5)? {
                        rusqlite::types::ValueRef::Text(t) if !t.is_empty() => {
                            Some(String::from_utf8_lossy(t).into_owned())
                        }
                        _ => None,
                    }
                } else {
                    None
                },
            })
        })
    })?;
    let n = rows.len();
    let mut max_id = 0;
    let mut aspects = Vec::with_capacity(n);
    let (mut ki, mut kv, mut ka) = (0i64, 0i64, 0i64);
    let mut days: Vec<(String, i64, Option<String>)> = Vec::new();
    let mut cur_day = i64::MIN;
    let mut cities: indexmap::IndexMap<String, i64> = indexmap::IndexMap::new();
    let close_day = |days: &mut Vec<(String, i64, Option<String>)>,
                     cities: &indexmap::IndexMap<String, i64>| {
        if let Some(last) = days.last_mut() {
            let mut best: Option<&String> = None;
            let mut bn = 0;
            for (c, k) in cities {
                if *k > bn {
                    best = Some(c);
                    bn = *k;
                }
            }
            last.2 = best.cloned();
        }
    };
    for r in &rows {
        if r.id > max_id {
            max_id = r.id;
        }
        let a = match (r.w, r.h) {
            (Some(w), Some(h)) if w != 0.0 && h != 0.0 => w / h,
            _ => 1.0,
        };
        aspects.push(pa_core::aspect_to_byte(a));
        match r.k {
            1 => ki += 1,
            2 => kv += 1,
            3 => ka += 1,
            _ => {}
        }
        if with_days {
            let d = r.t.div_euclid(pa_core::DAY_MS);
            if d != cur_day {
                if !days.is_empty() {
                    close_day(&mut days, &cities);
                }
                cur_day = d;
                cities.clear();
                days.push((pa_core::local_day_key(r.t), 0, None));
            }
            if let Some(last) = days.last_mut() {
                last.1 += 1;
            }
            if let Some(c) = &r.c {
                *cities.entry(c.clone()).or_insert(0) += 1;
            }
        }
    }
    if with_days && !days.is_empty() {
        close_day(&mut days, &cities);
    }
    use base64::Engine;
    let sk = pa_core::BrowseSkeleton {
        gen: format!("{}.{n}.{max_id}", ctx.events.gen()),
        total: n as i64,
        kind_counts: pa_core::KindCounts {
            image: ki,
            video: kv,
            audio: ka,
        },
        aspects: base64::engine::general_purpose::STANDARD.encode(&aspects),
        days: with_days.then(|| {
            days.into_iter()
                .map(|(key, count, place)| pa_core::SkeletonDay { key, count, place })
                .collect()
        }),
    };
    Ok(serde_json::to_vec(&sk)?)
}

fn device_child_dirs(ctx: &Ctx, folder_id: i64, dir: &str) -> ApiResult<Vec<String>> {
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    let rows: Vec<String> = ctx.db.all(
        "SELECT DISTINCT dir FROM media WHERE folder_id = ? AND substr(dir, 1, ?) = ? AND dir != ?
     UNION SELECT dir FROM device_dirs WHERE folder_id = ? AND substr(dir, 1, ?) = ? AND dir != ?",
        p![
            folder_id,
            pa_core::js::len16(&prefix) as i64,
            prefix.clone(),
            dir,
            folder_id,
            pa_core::js::len16(&prefix) as i64,
            prefix.clone(),
            dir
        ],
        |r| Ok(crate::db::text(r, 0).unwrap_or_default()),
    )?;
    let mut names = indexmap::IndexSet::new();
    for d in rows {
        let child = d
            .get(prefix.len()..)
            .unwrap_or("")
            .split('/')
            .next()
            .unwrap_or("")
            .to_string();
        if !child.is_empty()
            && !crate::scanner::skip_dir(ctx, folder_id, &format!("{prefix}{child}"))
        {
            names.insert(child);
        }
    }
    Ok(names.into_iter().collect())
}

/// `localeCompare(b, undefined, { numeric: true, sensitivity: 'base' })`
pub fn compare_numeric_base(a: &str, b: &str) -> std::cmp::Ordering {
    use icu_collator::options::{CollatorOptions, Strength};
    use icu_collator::preferences::CollationNumericOrdering;
    use icu_collator::{Collator, CollatorPreferences};
    thread_local! {
        static C: Option<icu_collator::CollatorBorrowed<'static>> = {
            let mut o = CollatorOptions::default();
            o.strength = Some(Strength::Primary);
            let mut prefs = CollatorPreferences::default();
            prefs.numeric_ordering = Some(CollationNumericOrdering::True);
            Collator::try_new(prefs, o).ok()
        };
    }
    C.with(|c| match c {
        Some(c) => c.compare(a, b),
        None => a.cmp(b),
    })
}

async fn sub_folders(ctx: &C, folder: &FolderRow, dir: &str) -> ApiResult<Vec<SubFolder>> {
    let mut names: Vec<String> = if folder.is_device() || folder.is_remote() {
        device_child_dirs(ctx, folder.id, dir)?
    } else {
        let abs = crate::paths::resolve_in_root(&folder.path, dir)?;
        let entries = tokio::task::spawn_blocking(move || {
            let mut v: Vec<(String, bool)> = std::fs::read_dir(&abs)
                .map(|rd| {
                    rd.flatten()
                        .map(|e| {
                            (
                                e.file_name().to_string_lossy().into_owned(),
                                e.file_type().map(|t| t.is_dir()).unwrap_or(false),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            v.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
            v
        })
        .await
        .map_err(HttpError::internal)?;
        let pre = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        entries
            .into_iter()
            .filter(|(n, d)| *d && !crate::scanner::skip_dir(ctx, folder.id, &format!("{pre}{n}")))
            .map(|(n, _)| n)
            .collect()
    };
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    let mut counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let rows = ctx.db.all(
        "SELECT dir, count(*) c FROM media WHERE folder_id = ? AND kind IN ('image','video') AND substr(dir, 1, ?) = ? AND dir != ? GROUP BY dir",
        p![folder.id, pa_core::js::len16(&prefix) as i64, prefix.clone(), dir],
        |r| Ok((crate::db::text(r, 0).unwrap_or_default(), r.get::<_, i64>(1)?)),
    )?;
    for (d, c) in rows {
        let child = d
            .get(prefix.len()..)
            .unwrap_or("")
            .split('/')
            .next()
            .unwrap_or("")
            .to_string();
        *counts.entry(child).or_insert(0) += c;
    }
    names.sort_by(|a, b| compare_numeric_base(a, b));
    let mut out = Vec::new();
    for name in names {
        let d = format!("{prefix}{name}");
        let covers = ctx.db.all(
            "SELECT id, taken_local t FROM media WHERE folder_id = ? AND kind IN ('image','video') AND (dir = ? OR substr(dir, 1, ?) = ?) ORDER BY taken_local DESC LIMIT 3",
            p![folder.id, d.clone(), pa_core::js::len16(&d) as i64 + 1, format!("{d}/")],
            |r| Ok((r.get::<_, i64>(0)?, crate::db::num_i(r, 1))),
        )?;
        out.push(SubFolder {
            media_count: counts.get(&name).copied().unwrap_or(0),
            cover_ids: covers.iter().map(|c| c.0.to_string()).collect(),
            latest: Some(covers.first().and_then(|c| c.1)),
            name,
            dir: d,
        });
    }
    Ok(out)
}

fn archives(ctx: &Ctx, folder_id: i64, dir: &str) -> ApiResult<Vec<MediaItem>> {
    let rows = ctx.db.all(
        &format!("SELECT {ITEM_COLS} FROM media WHERE folder_id = ? AND dir = ? AND kind = 'archive' ORDER BY name"),
        p![folder_id, dir],
        crate::db::MediaRow::from_row,
    )?;
    Ok(rows.iter().map(|r| r.to_item()).collect())
}

fn json_bytes(v: Arc<Vec<u8>>) -> Response<Body> {
    let mut r = Response::new(Body::from(v.as_ref().clone()));
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    r
}

pub fn routes(r: &mut Router) {
    let m = BodyScope::Main;
    r.add(
        "GET",
        "/api/browse",
        Global,
        m,
        h(|ctx, req| async move {
            let q = query_of(&ctx, &req)?;
            let c2 = ctx.clone();
            let q2 = q.clone();
            let rs = resolve_scope(&ctx, &q)?;
            let offset = q.offset.unwrap_or(0.0) != 0.0;
            if let Some(folder) = rs.folder {
                let lib = rs.lib;
                let dir = rs.dir;
                let mut crumbs = Vec::new();
                if let Some(l) = &lib {
                    crumbs.push(Crumb {
                        label: l.name.clone(),
                        library_id: Some(l.id.clone()),
                        folder_id: None,
                        dir: None,
                    });
                }
                if lib.as_ref().is_none_or(|l| l.folders.len() > 1) || folder.temp != 0 {
                    crumbs.push(Crumb {
                        label: folder.name.clone(),
                        library_id: lib.as_ref().map(|l| l.id.clone()),
                        folder_id: Some(folder.id.to_string()),
                        dir: Some(String::new()),
                    });
                }
                let mut acc = String::new();
                for seg in dir.split('/').filter(|s| !s.is_empty()) {
                    acc = if acc.is_empty() {
                        seg.to_string()
                    } else {
                        format!("{acc}/{seg}")
                    };
                    crumbs.push(Crumb {
                        label: seg.to_string(),
                        library_id: lib.as_ref().map(|l| l.id.clone()),
                        folder_id: Some(folder.id.to_string()),
                        dir: Some(acc.clone()),
                    });
                }
                let s: Scope = rs.s;
                let (total, gen, items) =
                    tokio::task::spawn_blocking(move || scope::page(&c2, &s, &q2))
                        .await
                        .map_err(HttpError::internal)??;
                let folders = if offset {
                    vec![]
                } else {
                    sub_folders(&ctx, &folder, &dir).await?
                };
                let arch = if offset {
                    vec![]
                } else {
                    archives(&ctx, folder.id, &dir)?
                };
                let target = if ctx.cfg.readonly || folder.is_device() || folder.is_remote() {
                    None
                } else {
                    Some(DirTarget {
                        folder_id: folder.id.to_string(),
                        dir: dir.clone(),
                    })
                };
                return ok_json(&BrowseResult {
                    crumbs,
                    folders,
                    roots: vec![],
                    items,
                    total,
                    archives: arch,
                    target,
                    gen: Some(gen),
                });
            }
            let s: Scope = rs.s;
            let lib = rs.lib;
            let (total, gen, items) =
                tokio::task::spawn_blocking(move || scope::page(&c2, &s, &q2))
                    .await
                    .map_err(HttpError::internal)??;
            if let Some(l) = lib {
                return ok_json(&BrowseResult {
                    crumbs: vec![Crumb {
                        label: l.name.clone(),
                        library_id: Some(l.id.clone()),
                        folder_id: None,
                        dir: None,
                    }],
                    folders: vec![],
                    roots: l.folders,
                    items,
                    total,
                    archives: vec![],
                    target: None,
                    gen: Some(gen),
                });
            }
            ok_json(&BrowseResult {
                crumbs: vec![],
                folders: vec![],
                roots: vec![],
                items,
                total,
                archives: vec![],
                target: None,
                gen: Some(gen),
            })
        }),
    );
    r.add(
        "GET",
        "/api/browse/skeleton",
        Global,
        m,
        h(|ctx, req| async move {
            let q = query_of(&ctx, &req)?;
            Ok(json_bytes(build_skeleton(&ctx, &q).await?))
        }),
    );
    r.add(
        "GET",
        "/api/geo/points",
        Global,
        m,
        h(|ctx, req| async move {
            let q = query_of(&ctx, &req)?;
            let c = ctx.clone();
            let pts = tokio::task::spawn_blocking(move || -> ApiResult<Vec<GeoPoint>> {
                let rs = resolve_scope(&c, &q)?;
                let mut w = rs.s.clone();
                w.where_.push("lat IS NOT NULL".into());
                w.where_.push("lon IS NOT NULL".into());
                let sql = format!(
                    "SELECT id, lat, lon FROM media {} ORDER BY id",
                    where_sql(&w)
                );
                Ok(c.db.read(|cn| {
                    query_all(cn, &sql, rs.s.params.clone(), |r| {
                        let lat = crate::db::num_f(r, 1).unwrap_or(0.0);
                        let lon = crate::db::num_f(r, 2).unwrap_or(0.0);
                        Ok(GeoPoint(
                            r.get::<_, i64>(0)?.to_string(),
                            pa_core::js::round(lat * 1e5) / 1e5,
                            pa_core::js::round(lon * 1e5) / 1e5,
                        ))
                    })
                })?)
            })
            .await
            .map_err(HttpError::internal)??;
            ok_json(&GeoPoints { points: pts })
        }),
    );
    r.add(
        "GET",
        "/api/timeline",
        Global,
        m,
        h(|ctx, req| async move {
            let mut raw = std::collections::HashMap::new();
            if let Some(l) = req.q("libraryId") {
                raw.insert("libraryId".to_string(), l.to_string());
            }
            raw.insert("recursive".into(), "1".into());
            let mut q = parse_browse_query(&raw)?;
            q.kind = Some(match req.q("kind") {
                Some(k) if !k.is_empty() && k != "all" => k.to_string(),
                _ => "image-video".into(),
            });
            let rs = resolve_scope(&ctx, &q)?;
            let rows = ctx.db.all(
                &format!("SELECT strftime('%Y-%m', taken_local / 1000, 'unixepoch') key, count(*) c FROM media {} GROUP BY key ORDER BY key DESC", where_sql(&rs.s)),
                rs.s.params.clone(),
                |r| Ok(pa_core::TimelineBucket { key: crate::db::text(r, 0).unwrap_or_default(), count: r.get(1)? }),
            )?;
            ok_json(&pa_core::TimelineResponse { buckets: rows })
        }),
    );
    r.add(
        "GET",
        "/api/search",
        Global,
        m,
        h(|ctx, req| async move {
            let term = pa_core::js::trim(req.qs("q")).to_string();
            if term.is_empty() {
                return ok_json(&serde_json::json!({ "items": [], "total": 0 }));
            }
            let q = Bq {
                q: Some(term),
                kind: Some("image-video".into()),
                limit: Some(scope::num_or(req.q("limit"), 200.0)),
                sort: Some("takenAt".into()),
                order: Some("desc".into()),
                ..Default::default()
            };
            let rs = resolve_scope(&ctx, &q)?;
            let (total, _gen, items) = scope::page(&ctx, &rs.s, &q)?;
            ok_json(&pa_core::SearchResponse { items, total })
        }),
    );
}
