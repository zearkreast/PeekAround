//! Explore routes (mirrors `routes/explore.ts`): explore, calendar, duplicates, metadata status
//! and re-read.

use crate::ctx::{Ctx, C};
use crate::db::{query_all, ITEM_COLS};
use crate::error::{ApiResult, HttpError};
use crate::explore;
use crate::http::{ok_json, BodyScope};
use crate::p;
use crate::router::{h, Router};
use crate::scope::{parse_browse_query, resolve_scope, where_sql, Bq, Scope};
use crate::security::Limit::{Fs, Global};
use pa_core::{MetaLibraryStatus, MetaStatus};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

fn explore_scope(ctx: &Ctx, raw: &HashMap<String, String>) -> ApiResult<Scope> {
    let q = parse_browse_query(raw)?;
    let eq = Bq {
        library_id: q.library_id,
        folder_id: q.folder_id,
        dir: q.dir,
        kind: Some(if raw.get("kind").is_some_and(|k| !k.is_empty()) {
            q.kind.unwrap_or_default()
        } else {
            "image-video".into()
        }),
        recursive: Some(true),
        ..Default::default()
    };
    Ok(resolve_scope(ctx, &eq)?.s)
}

/// per-library metadata diagnostics (slow results reused for 10× their compute time)
pub fn meta_status(ctx: &Ctx, fresh: bool) -> ApiResult<MetaStatus> {
    if !fresh {
        if let Some((at, ms, v)) = &*ctx.meta_memo.lock() {
            if *ms >= 30.0
                && at.elapsed() < Duration::from_millis((10_000f64).min(ms * 10.0) as u64)
            {
                return Ok(v.clone());
            }
        }
    }
    let t0 = Instant::now();
    let v = meta_status_now(ctx)?;
    *ctx.meta_memo.lock() = Some((
        Instant::now(),
        t0.elapsed().as_secs_f64() * 1000.0,
        v.clone(),
    ));
    Ok(v)
}

fn meta_status_now(ctx: &Ctx) -> ApiResult<MetaStatus> {
    let msg = pa_core::MESSAGING_SQL
        .replace("lower(name)", "lower(m.name)")
        .replace("(name", "(m.name")
        .replace(" dir", " m.dir")
        .replace("(dir", "(m.dir");
    let msg = regex::Regex::new(r"\b(name|dir)\b")
        .map(|re| re.replace_all(pa_core::MESSAGING_SQL, "m.$1").into_owned())
        .unwrap_or(msg);
    let sql = format!(
        "SELECT l.id lib, l.name name, count(m.id) total,
       coalesce(sum(m.lat IS NOT NULL), 0) loc,
       coalesce(sum(m.lat IS NULL AND (m.meta_state IN (0, 3) OR (m.meta_state = 1 AND m.meta_v < ?))), 0) pending,
       coalesce(sum(m.meta_state = 2), 0) bad,
       coalesce(sum(m.lat IS NULL AND {msg}), 0) msg
     FROM libraries l
     LEFT JOIN folders f ON f.library_id = l.id AND f.temp = 0
     LEFT JOIN media m ON m.folder_id = f.id AND m.kind IN ('image','video')
     GROUP BY l.id ORDER BY l.sort, l.id"
    );
    let libraries = ctx.db.all(&sql, p![pa_meta::META_V], |r| {
        use crate::db::{num_i, text};
        let total = num_i(r, 2).unwrap_or(0);
        let loc = num_i(r, 3).unwrap_or(0);
        let pending = num_i(r, 4).unwrap_or(0);
        let bad = num_i(r, 5).unwrap_or(0);
        Ok(MetaLibraryStatus {
            id: num_i(r, 0).unwrap_or(0).to_string(),
            name: text(r, 1).unwrap_or_default(),
            total,
            with_location: loc,
            without_location: (total - loc - pending - bad).max(0),
            pending,
            unreadable: bad,
            messaging: num_i(r, 6).unwrap_or(0),
            sidecar: None,
        })
    })?;
    Ok(MetaStatus {
        libraries,
        enriching: ctx.scanner.enriching() || ctx.scanner.busy(),
        indexing: crate::scanner::indexing(ctx),
    })
}

/// re-read metadata (enrichment queue)
pub fn reread_meta(ctx: &C, library_id: Option<&str>) -> ApiResult<i64> {
    let (w, params) = match library_id {
        Some(l) => ("folder_id IN (SELECT id FROM folders WHERE library_id = ? AND temp = 0 AND device_id IS NULL AND remote IS NULL)", p![pa_core::js::number(l)]),
        None => ("folder_id IN (SELECT id FROM folders WHERE temp = 0 AND device_id IS NULL AND remote IS NULL)", p![]),
    };
    if let Some(l) = library_id {
        if ctx
            .db
            .int(
                "SELECT id FROM libraries WHERE id = ?",
                p![pa_core::js::number(l)],
            )?
            .is_none()
        {
            return Err(HttpError::new(404, "Library not found"));
        }
    }
    let n = ctx.db.tx(|c| {
        let a = c.execute(&format!("UPDATE media SET meta_v = 0 WHERE meta_state = 1 AND kind IN ('image','video') AND {w}"), rusqlite::params_from_iter(params.clone()))?;
        let b = c.execute(&format!("UPDATE media SET meta_state = 0 WHERE meta_state = 2 AND kind IN ('image','video') AND {w}"), rusqlite::params_from_iter(params.clone()))?;
        Ok((a + b) as i64)
    })?;
    pa_meta::clear_sidecar_cache();
    crate::scanner::kick(ctx);
    crate::scanner::kick_enrich(ctx);
    if !ctx.rekick.swap(true, Ordering::SeqCst) {
        let c = ctx.clone();
        tokio::spawn(async move {
            let started = Instant::now();
            loop {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(3)) => {}
                    _ = c.shutdown.cancelled() => break,
                }
                let left = c
                    .db
                    .int("SELECT count(*) c FROM media WHERE meta_state = 1 AND meta_v < ? AND kind IN ('image','video') AND folder_id IN (SELECT id FROM folders WHERE remote IS NULL)", p![pa_meta::META_V])
                    .ok()
                    .flatten()
                    .unwrap_or(0);
                if left == 0 || started.elapsed() > Duration::from_secs(6 * 3600) {
                    break;
                }
                crate::scanner::kick_enrich(&c);
            }
            c.rekick.store(false, Ordering::SeqCst);
        });
    }
    ctx.explore.invalidate(ctx, true);
    Ok(n)
}

pub fn routes(r: &mut Router) {
    let m = BodyScope::Main;
    r.add(
        "GET",
        "/api/explore",
        Global,
        m,
        h(|ctx, req| async move {
            let q = parse_browse_query(&req.query)?;
            let eq = Bq {
                library_id: q.library_id,
                folder_id: q.folder_id,
                dir: q.dir,
                kind: if req.q("kind").is_some_and(|k| !k.is_empty()) {
                    q.kind
                } else {
                    None
                },
                ..Default::default()
            };
            let res = explore::get(&ctx, eq, crate::events::now_ms())
                .await
                .map_err(|e| HttpError::new(if e.contains("not found") { 404 } else { 500 }, e))?;
            ok_json(&res)
        }),
    );
    r.add(
        "GET",
        "/api/explore/calendar",
        Global,
        m,
        h(|ctx, req| async move {
            let year = pa_core::js::number(req.qs("year"));
            if !(year.is_finite() && year.fract() == 0.0) || !(1900.0..=2200.0).contains(&year) {
                return Err(HttpError::new(400, "Invalid year"));
            }
            let year = year as i64;
            let s = explore_scope(&ctx, &req.query)?;
            let from = pa_core::js::date_utc(year, 0, 1, 0, 0, 0, 0);
            let to = pa_core::js::date_utc(year + 1, 0, 1, 0, 0, 0, 0);
            let mut w = s.clone();
            w.where_.push("taken_local >= ?".into());
            w.where_.push("taken_local < ?".into());
            let mut params = p![from];
            params.extend(s.params.clone());
            params.extend(p![from, to]);
            let sql = format!("SELECT CAST((taken_local - ?) / 86400000 AS INTEGER) d, count(*) c FROM media {} GROUP BY d", where_sql(&Scope { where_: w.where_, params: vec![] }));
            let rows = ctx.db.read(|c| query_all(c, &sql, params, |r| Ok((crate::db::num_i(r, 0).unwrap_or(-1), r.get::<_, i64>(1)?))))?;
            let mut cal = pa_core::calendar_of(year, std::iter::empty());
            for (d, c) in rows {
                if d < 0 || d as usize >= cal.days.len() {
                    continue;
                }
                cal.days[d as usize] = c;
                cal.total += c;
                cal.max = cal.max.max(c);
            }
            ok_json(&cal)
        }),
    );
    r.add(
        "GET",
        "/api/explore/duplicates",
        Fs,
        m,
        h(|ctx, req| async move {
            let s = explore_scope(&ctx, &req.query)?;
            let offset = crate::scope::num_or(req.q("offset"), 0.0).max(0.0) as usize;
            let limit = crate::scope::num_or(req.q("limit"), 100.0).clamp(1.0, 500.0) as usize;
            let (groups, total) = explore::groups(&ctx, &where_sql(&s), &s.params, offset, limit);
            let ids: Vec<i64> = groups.iter().flat_map(|g| g.ids.iter().filter_map(|i| i.parse().ok())).collect();
            let mut items = Vec::new();
            let mut paths: indexmap::IndexMap<String, String> = indexmap::IndexMap::new();
            for chunk in ids.chunks(400) {
                let sql = format!("SELECT {ITEM_COLS}, (SELECT name FROM folders f WHERE f.id = media.folder_id) fname FROM media WHERE id IN ({})", vec!["?"; chunk.len()].join(","));
                let rows = ctx.db.all(&sql, chunk.iter().map(|i| crate::db::Sql::sql(*i)).collect(), |r| Ok((crate::db::MediaRow::from_row(r)?, crate::db::text(r, "fname"))))?;
                for (r, fname) in rows {
                    let p = [fname.unwrap_or_default(), r.dir.clone()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("/");
                    paths.insert(r.id.to_string(), p);
                    items.push(r.to_item());
                }
            }
            let pending = explore::pending(&ctx);
            if pending > 0 {
                explore::kick_dups(&ctx);
            }
            ok_json(&json!({ "groups": groups, "total": total, "items": items, "pending": pending, "paths": paths }))
        }),
    );
    r.add(
        "GET",
        "/api/meta/status",
        Global,
        m,
        h(|ctx, _req| async move { ok_json(&meta_status(&ctx, false)?) }),
    );
    r.add(
        "POST",
        "/api/meta/reread",
        Fs,
        m,
        h(|ctx, req| async move {
            let id = req
                .body
                .opt("libraryId")
                .filter(|v| !v.is_null() && v.as_str() != Some("") && v.as_bool() != Some(false))
                .map(crate::http::js_string);
            let queued = reread_meta(&ctx, id.as_deref())?;
            ok_json(&json!({ "queued": queued, "status": meta_status(&ctx, true)? }))
        }),
    );
}
