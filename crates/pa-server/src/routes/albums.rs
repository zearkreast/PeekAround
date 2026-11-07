//! Albums (mirrors `routes/albums.ts`): user-curated collections (migration 005).

use crate::ctx::Ctx;
use crate::db::query_one;
use crate::error::{ApiResult, HttpError};
use crate::events::now_ms;
use crate::http::{ok_json, BodyScope};
use crate::p;
use crate::router::{h, Router};
use crate::security::Limit::Fs;
use pa_core::{Album, AlbumOrder, ServerEvent};
use serde_json::{json, Value};

const MAX_IDS: usize = 20_000;

const LIST_SQL: &str = "
SELECT a.*, count(m.id) n, sum(m.kind = 'image') p, sum(m.kind = 'video') v, min(m.taken_local) f, max(m.taken_local) t,
  (SELECT ai2.media_id FROM album_items ai2 JOIN media m2 ON m2.id = ai2.media_id
    WHERE ai2.album_id = a.id AND m2.kind IN ('image','video') ORDER BY m2.taken_local DESC LIMIT 1) auto
FROM albums a
LEFT JOIN album_items ai ON ai.album_id = a.id
LEFT JOIN media m ON m.id = ai.media_id
";

fn order_of(s: &str) -> Option<AlbumOrder> {
    serde_json::from_value(Value::String(s.to_string())).ok()
}

fn to_album(r: &rusqlite::Row) -> rusqlite::Result<Album> {
    use crate::db::{num_i, text};
    let n = num_i(r, "n").unwrap_or(0);
    let cover = num_i(r, "cover_media_id").or(num_i(r, "auto"));
    Ok(Album {
        id: num_i(r, "id").unwrap_or(0).to_string(),
        name: text(r, "name").unwrap_or_default(),
        cover_id: cover.map(|c| c.to_string()),
        cover_picked: num_i(r, "cover_media_id").is_some(),
        count: n,
        photos: num_i(r, "p").unwrap_or(0),
        videos: num_i(r, "v").unwrap_or(0),
        from: if n != 0 { num_i(r, "f") } else { None },
        to: if n != 0 { num_i(r, "t") } else { None },
        order: text(r, "item_order")
            .and_then(|o| order_of(&o))
            .unwrap_or(AlbumOrder::Desc),
        created: num_i(r, "created").unwrap_or(0),
        updated: num_i(r, "updated").unwrap_or(0),
        sort: num_i(r, "sort").unwrap_or(0),
    })
}

/// all albums (sort order)
pub fn list(ctx: &Ctx) -> ApiResult<Vec<Album>> {
    Ok(ctx.db.all(
        &format!("{LIST_SQL} GROUP BY a.id ORDER BY a.sort, a.id"),
        p![],
        to_album,
    )?)
}

fn num(v: &Value) -> f64 {
    pa_core::js::number_of(v)
}

/// one album (404)
pub fn get(ctx: &Ctx, id: f64) -> ApiResult<Album> {
    ctx.db
        .with(|c| {
            query_one(
                c,
                &format!("{LIST_SQL} WHERE a.id = ? GROUP BY a.id"),
                p![id],
                to_album,
            )
        })?
        .ok_or_else(|| HttpError::new(404, "Album not found"))
}

fn exists(ctx: &Ctx, id: f64) -> ApiResult<i64> {
    ctx.db
        .int("SELECT id FROM albums WHERE id = ?", p![id])?
        .ok_or_else(|| HttpError::new(404, "Album not found"))
}

fn changed(ctx: &Ctx, ids: &[i64], layout: bool) {
    if layout {
        ctx.events.bump();
    }
    ctx.events.emit(ServerEvent::Albums {
        ids: Some(ids.iter().map(|i| i.to_string()).collect()),
    });
}

fn clean_name(v: Option<&Value>) -> ApiResult<String> {
    let s = match v {
        Some(Value::String(s)) => {
            let collapsed: String = s
                .split(pa_core::js::is_space)
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            pa_core::js::slice16_to(pa_core::js::trim(&collapsed), 120).to_string()
        }
        _ => String::new(),
    };
    if s.is_empty() {
        return Err(HttpError::new(400, "Album name required"));
    }
    Ok(s)
}

fn media_ids(ids: Option<&Value>) -> ApiResult<Vec<i64>> {
    let arr = match ids {
        None => return Ok(vec![]),
        Some(Value::Array(a)) => a,
        Some(_) => return Err(HttpError::new(400, "ids must be an array")),
    };
    if arr.len() > MAX_IDS {
        return Err(HttpError::new(413, "Too many items"));
    }
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for v in arr {
        let n = num(v);
        if n.is_finite()
            && n.fract() == 0.0
            && n > 0.0
            && n <= 9_007_199_254_740_991.0
            && seen.insert(n as i64)
        {
            out.push(n as i64);
        }
    }
    Ok(out)
}

fn add(ctx: &Ctx, aid: i64, media: &[i64], emit: bool) -> ApiResult<i64> {
    let added = ctx.db.tx(|c| {
        let mut pos: f64 = c.query_row("SELECT max(position) p FROM album_items WHERE album_id = ?", [aid], |r| Ok(crate::db::num_f(r, 0))).ok().flatten().unwrap_or(0.0);
        let now = now_ms();
        let mut added = 0;
        for m in media {
            let ok: Option<i64> = c.query_row("SELECT 1 FROM media WHERE id = ?", [m], |r| r.get(0)).ok();
            if ok.is_none() {
                continue;
            }
            let n = c.prepare_cached("INSERT OR IGNORE INTO album_items (album_id, media_id, added, position) VALUES (?, ?, ?, ?)")?.execute(rusqlite::params![aid, m, now, pos + 1.0])?;
            if n > 0 {
                pos += 1.0;
                added += 1;
            }
        }
        c.execute("UPDATE albums SET updated = ? WHERE id = ?", rusqlite::params![now, aid])?;
        Ok(added)
    })?;
    if emit {
        changed(ctx, &[aid], true);
    }
    Ok(added)
}

pub fn routes(r: &mut Router) {
    let m = BodyScope::Main;
    r.add(
        "GET",
        "/api/albums",
        Fs,
        m,
        h(|ctx, _req| async move { ok_json(&json!({ "albums": list(&ctx)? })) }),
    );
    r.add(
        "POST",
        "/api/albums",
        Fs,
        m,
        h(|ctx, req| async move {
            let name_v = req.body.opt("name").cloned().map(|v| Value::String(crate::http::js_string(&v))).unwrap_or(Value::String(String::new()));
            let ids = req.body.opt("ids");
            let now = now_ms();
            let clean = clean_name(Some(&name_v))?;
            let media = media_ids(ids)?;
            ctx.db.run("UPDATE albums SET sort = sort + 1", p![])?;
            let id = ctx.db.insert("INSERT INTO albums (name, item_order, created, updated, sort) VALUES (?, ?, ?, ?, 0)", p![clean, "desc", now, now])?;
            if !media.is_empty() {
                add(&ctx, id, &media, false)?;
            }
            changed(&ctx, &[id], true);
            ok_json(&get(&ctx, id as f64)?)
        }),
    );
    r.add(
        "POST",
        "/api/albums/order",
        Fs,
        m,
        h(|ctx, req| async move {
            let want = media_ids(req.body.opt("ids"))?;
            let all: Vec<i64> =
                ctx.db
                    .all("SELECT id FROM albums ORDER BY sort, id", p![], |r| {
                        r.get(0)
                    })?;
            let known: std::collections::HashSet<i64> = all.iter().copied().collect();
            let want: Vec<i64> = want.into_iter().filter(|x| known.contains(x)).collect();
            let wanted: std::collections::HashSet<i64> = want.iter().copied().collect();
            let rest: Vec<i64> = all
                .iter()
                .copied()
                .filter(|x| !wanted.contains(x))
                .collect();
            ctx.db.tx(|c| {
                for (i, x) in want.iter().chain(rest.iter()).enumerate() {
                    c.prepare_cached("UPDATE albums SET sort = ? WHERE id = ?")?
                        .execute(rusqlite::params![i as i64, x])?;
                }
                Ok(())
            })?;
            changed(&ctx, &all, false);
            ok_json(&json!({ "albums": list(&ctx)? }))
        }),
    );
    r.add(
        "POST",
        "/api/albums/membership",
        Fs,
        m,
        h(|ctx, req| async move {
            let media = media_ids(req.body.opt("ids"))?;
            let mut out: indexmap::IndexMap<String, i64> = indexmap::IndexMap::new();
            for chunk in media.chunks(500) {
                let sql = format!("SELECT album_id, count(*) c FROM album_items WHERE media_id IN ({}) GROUP BY album_id", vec!["?"; chunk.len()].join(","));
                let rows = ctx.db.all(&sql, chunk.iter().map(|i| crate::db::Sql::sql(*i)).collect(), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
                for (a, c) in rows {
                    *out.entry(a.to_string()).or_insert(0) += c;
                }
            }
            ok_json(&json!({ "counts": out }))
        }),
    );
    r.add(
        "GET",
        "/api/albums/:id",
        Fs,
        m,
        h(|ctx, req| async move { ok_json(&get(&ctx, pa_core::js::number(req.param("id")))?) }),
    );
    r.add(
        "PATCH",
        "/api/albums/:id",
        Fs,
        m,
        h(|ctx, req| async move {
            let aid = exists(&ctx, pa_core::js::number(req.param("id")))?;
            let b = |k: &str| req.body.or_empty(k);
            if let Some(Value::String(n)) = b("name") {
                ctx.db.run(
                    "UPDATE albums SET name = ? WHERE id = ?",
                    p![clean_name(Some(&Value::String(n.clone())))?, aid],
                )?;
            }
            if let Some(c) = b("coverId") {
                match c {
                    Value::Null => {
                        ctx.db.run(
                            "UPDATE albums SET cover_media_id = NULL WHERE id = ?",
                            p![aid],
                        )?;
                    }
                    Value::String(s) if s.is_empty() => {
                        ctx.db.run(
                            "UPDATE albums SET cover_media_id = NULL WHERE id = ?",
                            p![aid],
                        )?;
                    }
                    other => {
                        let mid = ctx.db.int(
                            "SELECT media_id FROM album_items WHERE album_id = ? AND media_id = ?",
                            p![aid, num(other)],
                        )?;
                        let Some(mid) = mid else {
                            return Err(HttpError::new(
                                400,
                                "The cover must be an item of the album",
                            ));
                        };
                        ctx.db.run(
                            "UPDATE albums SET cover_media_id = ? WHERE id = ?",
                            p![mid, aid],
                        )?;
                    }
                }
            }
            if let Some(o) = b("order") {
                let ok = o.as_str().and_then(order_of);
                let Some(_) = ok else {
                    return Err(HttpError::new(400, "Unknown order"));
                };
                ctx.db.run(
                    "UPDATE albums SET item_order = ? WHERE id = ?",
                    p![o.as_str().unwrap_or("desc"), aid],
                )?;
            }
            ctx.db.run(
                "UPDATE albums SET updated = ? WHERE id = ?",
                p![now_ms(), aid],
            )?;
            changed(&ctx, &[aid], false);
            ok_json(&get(&ctx, aid as f64)?)
        }),
    );
    r.add(
        "DELETE",
        "/api/albums/:id",
        Fs,
        m,
        h(|ctx, req| async move {
            let aid = exists(&ctx, pa_core::js::number(req.param("id")))?;
            ctx.db.run("DELETE FROM albums WHERE id = ?", p![aid])?;
            changed(&ctx, &[aid], true);
            ok_json(&json!({ "ok": true }))
        }),
    );
    r.add(
        "POST",
        "/api/albums/:id/items",
        Fs,
        m,
        h(|ctx, req| async move {
            let id = pa_core::js::number(req.param("id"));
            let aid = exists(&ctx, id)?;
            let media = media_ids(req.body.opt("ids"))?;
            let added = add(&ctx, aid, &media, true)?;
            ok_json(&json!({ "added": added, "album": get(&ctx, id)? }))
        }),
    );
    r.add(
        "POST",
        "/api/albums/:id/items/remove",
        Fs,
        m,
        h(|ctx, req| async move {
            let id = pa_core::js::number(req.param("id"));
            let aid = exists(&ctx, id)?;
            let media = media_ids(req.body.opt("ids"))?;
            let n = ctx.db.tx(|c| {
                let mut n = 0;
                for m in &media {
                    n += c.prepare_cached("DELETE FROM album_items WHERE album_id = ? AND media_id = ?")?.execute(rusqlite::params![aid, m])?;
                }
                c.execute(
                    "UPDATE albums SET cover_media_id = NULL WHERE id = ? AND cover_media_id IS NOT NULL AND cover_media_id NOT IN (SELECT media_id FROM album_items WHERE album_id = ?)",
                    rusqlite::params![aid, aid],
                )?;
                c.execute("UPDATE albums SET updated = ? WHERE id = ?", rusqlite::params![now_ms(), aid])?;
                Ok(n)
            })?;
            changed(&ctx, &[aid], true);
            ok_json(&json!({ "removed": n, "album": get(&ctx, id)? }))
        }),
    );
    r.add(
        "POST",
        "/api/albums/:id/reorder",
        Fs,
        m,
        h(|ctx, req| async move {
            let aid = exists(&ctx, pa_core::js::number(req.param("id")))?;
            let mv = media_ids(req.body.opt("ids"))?;
            let order: Vec<i64> = ctx.db.all("SELECT media_id FROM album_items WHERE album_id = ? ORDER BY position, added, media_id", p![aid], |r| r.get(0))?;
            let members: std::collections::HashSet<i64> = order.iter().copied().collect();
            let moving: Vec<i64> = mv.into_iter().filter(|m| members.contains(m)).collect();
            if !moving.is_empty() {
                let mset: std::collections::HashSet<i64> = moving.iter().copied().collect();
                let rest: Vec<i64> = order.iter().copied().filter(|m| !mset.contains(m)).collect();
                let at = match req.body.opt("before") {
                    None | Some(Value::Null) => rest.len(),
                    Some(Value::String(s)) if s.is_empty() => rest.len(),
                    Some(b) => {
                        let n = num(b);
                        rest.iter().position(|x| *x as f64 == n).unwrap_or(rest.len())
                    }
                };
                let mut next = rest[..at].to_vec();
                next.extend(&moving);
                next.extend(&rest[at..]);
                ctx.db.tx(|c| {
                    for (i, m) in next.iter().enumerate() {
                        c.prepare_cached("UPDATE album_items SET position = ? WHERE album_id = ? AND media_id = ?")?.execute(rusqlite::params![(i + 1) as f64, aid, m])?;
                    }
                    c.execute("UPDATE albums SET item_order = 'custom', updated = ? WHERE id = ?", rusqlite::params![now_ms(), aid])?;
                    Ok(())
                })?;
                changed(&ctx, &[aid], true);
            }
            ok_json(&json!({ "ok": true }))
        }),
    );
}
