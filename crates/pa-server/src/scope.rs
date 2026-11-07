//! Browse queries → SQL (mirrors the query half of `routes/browse.ts` + `insights.ts` filters):
//! THE where-clause builder used by browse, skeleton, geo points, explore and search.

use crate::ctx::Ctx;
use crate::db::{FolderRow, Params, Sql, SUN_NA};
use crate::error::{ApiResult, HttpError};
use crate::events::now_ms;
use crate::library;
use crate::p;
use pa_core::{DayTime, Library, MediaFlagName, Orientation, VideoLength};
use std::collections::HashMap;

/// `BrowseQuery` as the server handles it (kind is any string, like the TS cast).
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Bq {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub library_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recursive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub q: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flag: Option<MediaFlagName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gps: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<pa_core::ColorBucket>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orient: Option<Orientation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub daytime: Option<DayTime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vlen: Option<VideoLength>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dup: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_added: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_position: Option<bool>,
    /// internal: album queries of viewers without device access
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub no_devices: bool,
}

fn enum_of<T: serde::de::DeserializeOwned>(s: Option<&str>) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(s?.to_string())).ok()
}

const SORT_KEYS: [&str; 5] = ["takenAt", "name", "mtime", "size", "kind"];

fn truthy(v: Option<&str>) -> bool {
    matches!(v, Some("1") | Some("true"))
}

fn non_empty(v: Option<&str>) -> Option<String> {
    v.filter(|s| !s.is_empty()).map(String::from)
}

/// `Number(v) || 0`
pub fn num_or(v: Option<&str>, d: f64) -> f64 {
    let n = v.map(pa_core::js::number).unwrap_or(f64::NAN);
    if n.is_nan() || n == 0.0 {
        d
    } else {
        n
    }
}

/// Querystring → query (`parseBrowseQuery`).
pub fn parse_browse_query(raw: &HashMap<String, String>) -> ApiResult<Bq> {
    let g = |k: &str| raw.get(k).map(String::as_str);
    let num = |v: Option<&str>| -> Option<f64> {
        let v = v?;
        if v.is_empty() {
            return None;
        }
        let n = pa_core::js::number(v);
        n.is_finite().then_some(n)
    };
    Ok(Bq {
        library_id: non_empty(g("libraryId")),
        folder_id: non_empty(g("folderId")),
        dir: Some(crate::paths::normalize_rel(g("dir"))?),
        recursive: Some(truthy(g("recursive"))),
        sort: Some(
            g("sort")
                .filter(|s| SORT_KEYS.contains(s))
                .unwrap_or("takenAt")
                .to_string(),
        ),
        order: Some(
            if g("order") == Some("asc") {
                "asc"
            } else {
                "desc"
            }
            .to_string(),
        ),
        kind: Some(non_empty(g("kind")).unwrap_or_else(|| "all".into())),
        q: g("q")
            .map(pa_core::js::trim)
            .filter(|s| !s.is_empty())
            .map(String::from),
        offset: Some(num_or(g("offset"), 0.0)),
        limit: Some(num_or(g("limit"), 600.0)),
        from: num(g("from")),
        to: num(g("to")),
        place: non_empty(g("place")),
        camera: non_empty(g("camera")),
        flag: enum_of(g("flag")),
        gps: g("gps")
            .filter(|s| *s == "none" || *s == "any")
            .map(String::from),
        color: g("color").and_then(pa_core::parse_color_bucket),
        orient: enum_of(g("orient")),
        daytime: enum_of(g("daytime")),
        vlen: enum_of(g("vlen")),
        dup: truthy(g("dup")).then_some(true),
        city: non_empty(g("city")).map(|c| pa_core::js::slice16_to(&c, 200).to_string()),
        by_added: truthy(g("byAdded")).then_some(true),
        album_id: g("albumId")
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .map(String::from),
        by_position: truthy(g("byPosition")).then_some(true),
        no_devices: false,
    })
}

/// WHERE parts + parameters.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub where_: Vec<String>,
    pub params: Params,
}

impl Scope {
    fn push(&mut self, w: impl Into<String>) {
        self.where_.push(w.into());
    }
}

/// `whereSql(s)`
pub fn where_sql(s: &Scope) -> String {
    if s.where_.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", s.where_.join(" AND "))
    }
}

/// the query narrows the result beyond a folder (→ flatten sub folders)
fn has_filters(q: &Bq) -> bool {
    q.q.is_some()
        || q.from.is_some()
        || q.to.is_some()
        || q.place.is_some()
        || q.camera.is_some()
        || q.flag.is_some()
        || has_insight_filters(q)
}

/// an insight filter is set
pub fn has_insight_filters(q: &Bq) -> bool {
    q.gps.is_some()
        || q.color.is_some()
        || q.orient.is_some()
        || q.daytime.is_some()
        || q.vlen.is_some()
        || q.dup.is_some()
        || q.city.is_some()
}

fn kind_clause(kind: Option<&str>, s: &mut Scope) {
    match kind {
        Some("image") => s.push("kind = 'image'"),
        Some("video") => s.push("kind = 'video'"),
        Some("favorites") => s.push("favorite = 1 AND kind IN ('image','video')"),
        Some("image-video") => s.push("kind IN ('image','video')"),
        _ => s.push("kind IN ('image','video','audio')"),
    }
}

fn dir_clause(folder_id: i64, dir: &str, recursive: bool, s: &mut Scope) {
    s.push("folder_id = ?");
    s.params.push(folder_id.sql());
    if recursive {
        if !dir.is_empty() {
            s.push("(dir = ? OR substr(dir, 1, ?) = ?)");
            s.params.extend(p![
                dir,
                pa_core::js::len16(dir) as i64 + 1,
                format!("{dir}/")
            ]);
        }
    } else {
        s.push("dir = ?");
        s.params.push(dir.sql());
    }
}

fn flag_clause(flag: MediaFlagName, s: &mut Scope) {
    match flag.bit() {
        None => s.push("kind = 'video'"),
        Some(b) => {
            s.push("(flags & ?) != 0");
            s.params.push(b.sql());
        }
    }
}

/// derived colour / sun columns (`fillInsightColumns`): false when rows are left
pub fn fill_insight_columns(ctx: &Ctx, min_gap_ms: i64, limit: i64) -> bool {
    {
        let mut last = ctx.explore.last_fill.lock();
        let now = now_ms();
        if min_gap_ms > 0 && now - *last < min_gap_ms {
            return true;
        }
        *last = now;
    }
    let a = ctx
        .db
        .run("UPDATE media SET color = peek_color(thumbhash) WHERE id IN (SELECT id FROM media WHERE color IS NULL AND thumbhash IS NOT NULL LIMIT ?)", p![limit])
        .unwrap_or(0) as i64;
    let b = ctx
        .db
        .run("UPDATE media SET sun = peek_sun(taken_at, taken_local, time_src, lat, lon) WHERE id IN (SELECT id FROM media WHERE sun IS NULL LIMIT ?)", p![limit])
        .unwrap_or(0) as i64;
    a < limit && b < limit
}

const HOUR_SQL: &str = "CAST(strftime('%H', taken_local / 1000, 'unixepoch') AS INTEGER)";

fn js_num(n: f64) -> String {
    pa_core::js::num_to_string(n)
}

fn insight_clauses(ctx: &Ctx, q: &Bq, s: &mut Scope) {
    if q.color.is_some() || q.daytime.is_some() {
        fill_insight_columns(ctx, 2000, 1_000_000);
    }
    match q.gps.as_deref() {
        Some("none") => s.push("lat IS NULL"),
        Some("any") => s.push("lat IS NOT NULL"),
        _ => {}
    }
    if let Some(c) = q.color {
        s.push("color = ?");
        s.params.push((c.index() as i64).sql());
    }
    if let Some(o) = q.orient {
        let pano = format!("(flags & {}) != 0", pa_core::media_flag::PANORAMA);
        if o == Orientation::Panorama {
            s.push(pano);
        } else {
            s.push(format!("NOT {pano}"));
            s.push("width > 0 AND height > 0");
            s.push(match o {
                Orientation::Square => "abs(width - height) <= 0.05 * max(width, height)",
                Orientation::Portrait => {
                    "height > width AND abs(width - height) > 0.05 * max(width, height)"
                }
                _ => "width > height AND abs(width - height) > 0.05 * max(width, height)",
            });
        }
    }
    match q.daytime {
        Some(DayTime::Golden) => s.push(format!(
            "sun >= {} AND sun <= {}",
            js_num(pa_core::GOLDEN_MIN),
            js_num(pa_core::GOLDEN_MAX)
        )),
        Some(DayTime::Night) => s.push(format!(
            "(sun < {} OR (sun = {} AND time_src != 0 AND ({HOUR_SQL} >= 22 OR {HOUR_SQL} < 5)))",
            js_num(pa_core::NIGHT_MAX),
            js_num(SUN_NA)
        )),
        None => {}
    }
    match q.vlen {
        Some(VideoLength::Short) => s.push(format!(
            "kind = 'video' AND duration > 0 AND duration < {}",
            js_num(pa_core::SHORT_VIDEO_S)
        )),
        Some(VideoLength::Long) => s.push(format!(
            "kind = 'video' AND duration > {}",
            js_num(pa_core::LONG_VIDEO_S)
        )),
        None => {}
    }
    if q.dup.is_some() {
        s.push("chash IN (SELECT chash FROM media WHERE chash IS NOT NULL GROUP BY chash HAVING count(*) > 1)");
    }
    if let Some(c) = &q.city {
        s.push("city = ?");
        s.params.push(c.sql());
    }
}

/// normalised camera key → stored make/model
fn camera_lookup(ctx: &Ctx) -> HashMap<String, (Option<String>, String)> {
    let rows = ctx
        .db
        .all(
            "SELECT DISTINCT make, model FROM media WHERE model IS NOT NULL",
            p![],
            |r| {
                Ok((
                    crate::db::text(r, 0),
                    crate::db::text(r, 1).unwrap_or_default(),
                ))
            },
        )
        .unwrap_or_default();
    let mut m = HashMap::new();
    for (make, model) in rows {
        m.insert(pa_core::camera_key(make.as_deref(), &model), (make, model));
    }
    m
}

fn filter_clauses(ctx: &Ctx, q: &Bq, s: &mut Scope) {
    if let Some(f) = q.from {
        s.push("taken_local >= ?");
        s.params.push(f.sql());
    }
    if let Some(t) = q.to {
        s.push("taken_local < ?");
        s.params.push(t.sql());
    }
    if let Some(place) = &q.place {
        let mut parts = place.splitn(3, '-');
        let cc = parts.next().unwrap_or("");
        let code = parts.next();
        s.push("cc = ?");
        s.params.push(cc.to_uppercase().sql());
        if let Some(code) = code {
            if code == "00" || code.is_empty() {
                s.push("(admin1 IS NULL OR admin1 = '')");
            } else {
                s.push("admin1 = ?");
                s.params.push(code.sql());
            }
        }
    }
    if let Some(cam) = &q.camera {
        match camera_lookup(ctx).get(&cam.to_lowercase()) {
            None => s.push("0"),
            Some((make, model)) => {
                s.push("model = ? AND make IS ?");
                s.params.extend(p![model.clone(), make.clone()]);
            }
        }
    }
    if let Some(f) = q.flag {
        flag_clause(f, s);
    }
    insight_clauses(ctx, q, s);
    if let Some(text) = &q.q {
        let ps = pa_core::parse_search(text);
        if let Some(f) = ps.flag {
            if Some(f) != q.flag {
                flag_clause(f, s);
            }
        }
        if let (Some(f), Some(t)) = (ps.from, ps.to) {
            s.push("taken_local >= ? AND taken_local < ?");
            s.params.extend(p![f, t]);
        }
        if let Some(m) = ps.month {
            s.push("CAST(strftime('%m', taken_local / 1000, 'unixepoch') AS INTEGER) = ?");
            s.params.push(m.sql());
        }
        for t in ps.terms {
            s.push("instr(peek_search(name, city, cc, admin1, make, model), ?) > 0");
            s.params.push(t.sql());
        }
    }
}

/// `resolveScope` result
pub struct ResolvedScope {
    pub s: Scope,
    pub items: bool,
    pub folder: Option<FolderRow>,
    pub lib: Option<Library>,
    pub dir: String,
}

/// THE where-clause builder.
pub fn resolve_scope(ctx: &Ctx, q: &Bq) -> ApiResult<ResolvedScope> {
    let mut s = Scope::default();
    kind_clause(q.kind.as_deref(), &mut s);
    if let Some(aid) = &q.album_id {
        s.push("id IN (SELECT media_id FROM album_items WHERE album_id = ?)");
        s.params.push(aid.parse::<f64>().unwrap_or(f64::NAN).sql());
        if q.no_devices {
            s.push("+folder_id IN (SELECT id FROM folders WHERE device_id IS NULL)");
        }
        filter_clauses(ctx, q, &mut s);
        return Ok(ResolvedScope {
            s,
            items: true,
            folder: None,
            lib: None,
            dir: String::new(),
        });
    }
    let mut folder_id = q.folder_id.clone();
    let mut lib = match &q.library_id {
        Some(l) => Some(library::library(ctx, l)?),
        None => None,
    };
    if folder_id.is_none() {
        if let Some(l) = &lib {
            if l.folders.len() == 1 {
                folder_id = Some(l.folders[0].id.clone());
            }
        }
    }
    let dir = q.dir.clone().unwrap_or_default();
    let mut items = true;
    let mut folder = None;
    if let Some(fid) = folder_id {
        let f = library::folder_row_str(ctx, &fid)?;
        if lib.is_none() {
            if let Some(l) = f.library_id {
                lib = Some(library::library(ctx, &l.to_string())?);
            }
        }
        dir_clause(
            f.id,
            &dir,
            q.recursive == Some(true) || has_filters(q),
            &mut s,
        );
        folder = Some(f);
    } else if let Some(l) = &lib {
        let ids: Vec<i64> = l.folders.iter().filter_map(|f| f.id.parse().ok()).collect();
        if ids.is_empty() {
            s.push("0");
        } else {
            s.push(format!(
                "+folder_id IN ({})",
                vec!["?"; ids.len()].join(",")
            ));
            s.params.extend(ids.into_iter().map(Sql::sql));
        }
        items = q.recursive == Some(true) || has_filters(q);
    } else {
        s.push("+folder_id IN (SELECT id FROM folders WHERE temp = 0 AND device_id IS NULL)");
    }
    filter_clauses(ctx, q, &mut s);
    if !items {
        s.push("0");
    }
    Ok(ResolvedScope {
        s,
        items,
        folder,
        lib,
        dir,
    })
}

/// `orderSql(q)`
pub fn order_sql(q: &Bq) -> String {
    if q.dup.is_some() {
        return "ORDER BY size DESC, chash, id".into();
    }
    if let (Some(a), Some(true)) = (&q.album_id, q.by_position) {
        let n: i64 = a.parse().unwrap_or(0);
        return format!("ORDER BY (SELECT position FROM album_items WHERE album_id = {n} AND media_id = media.id), id");
    }
    if q.by_added.is_some() {
        return "ORDER BY coalesce(added_at, mtime) DESC, id DESC".into();
    }
    let col = match q.sort.as_deref().unwrap_or("takenAt") {
        "name" => "name COLLATE NOCASE",
        "mtime" => "mtime",
        "size" => "size",
        "kind" => "kind",
        _ => "taken_local",
    };
    let dir = if q.order.as_deref() == Some("asc") {
        "asc"
    } else {
        "desc"
    };
    format!("ORDER BY {col} {dir}, id {dir}")
}

/// count + gen of a scope
pub fn count_and_gen(ctx: &Ctx, s: &Scope) -> ApiResult<(i64, String)> {
    let r = ctx
        .db
        .get(
            &format!("SELECT count(*) c, max(id) m FROM media {}", where_sql(s)),
            s.params.clone(),
            |r| Ok((r.get::<_, i64>(0)?, crate::db::num_i(r, 1))),
        )?
        .unwrap_or((0, None));
    Ok((
        r.0,
        format!("{}.{}.{}", ctx.events.gen(), r.0, r.1.unwrap_or(0)),
    ))
}

/// a page of items
pub fn page(ctx: &Ctx, s: &Scope, q: &Bq) -> ApiResult<(i64, String, Vec<pa_core::MediaItem>)> {
    let limit = q
        .limit
        .filter(|l| !l.is_nan() && *l != 0.0)
        .unwrap_or(600.0)
        .clamp(1.0, 5000.0) as i64;
    let offset = q.offset.filter(|o| !o.is_nan()).unwrap_or(0.0).max(0.0) as i64;
    let (total, gen) = count_and_gen(ctx, s)?;
    let mut ps = s.params.clone();
    ps.extend(p![limit, offset]);
    let rows = ctx.db.all(
        &format!(
            "SELECT {} FROM media {} {} LIMIT ? OFFSET ?",
            crate::db::ITEM_COLS,
            where_sql(s),
            order_sql(q)
        ),
        ps,
        crate::db::MediaRow::from_row,
    )?;
    Ok((total, gen, rows.iter().map(|r| r.to_item()).collect()))
}

/// 404 helper for missing ids
pub fn not_found(what: &str) -> HttpError {
    HttpError::new(404, format!("{what} not found"))
}
