//! Adapters between the server and the engines (pa-jobs, pa-link, pa-remote): the traits those
//! crates define over the library database, scanner, thumbnail service and event hub.
//!
//! Every adapter holds a `Weak<Ctx>` (the engines live inside the context); a call after the
//! context is gone is a harmless no-op / 503.

use crate::ctx::Ctx;
use crate::events::EventHub;
use crate::routes::util::{media_abs, media_row};
use pa_core::{MediaReason, ServerEvent};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

pub(crate) fn gone() -> pa_jobs::HttpError {
    pa_jobs::HttpError::new(503, "Server is shutting down")
}

/// `ctx.events` (+ `scanner.onGroupsChanged` = explore invalidation) for the engines.
pub struct ServerEvents {
    pub events: EventHub,
    pub ctx: Weak<Ctx>,
}

impl pa_jobs::EventSink for ServerEvents {
    fn emit(&self, ev: ServerEvent) {
        self.events.emit(ev);
    }
    fn media_changed(&self, folder_id: &str, dir: &str, reason: MediaReason) {
        self.events.media_changed(folder_id, dir, reason);
    }
    fn bump(&self) {
        self.events.bump();
    }
    fn groups_changed(&self) {
        if let Some(c) = self.ctx.upgrade() {
            c.explore.invalidate(&c, true);
        }
    }
}

/// The library side of the export jobs (`registerJobRunners`).
pub struct JobHost(pub Weak<Ctx>);

impl JobHost {
    fn ctx(&self) -> Result<Arc<Ctx>, pa_jobs::HttpError> {
        self.0.upgrade().ok_or_else(gone)
    }
}

#[async_trait::async_trait]
impl pa_jobs::MediaLookup for JobHost {
    fn media(&self, id: &str) -> Result<pa_jobs::JobMedia, pa_jobs::HttpError> {
        let ctx = self.ctx()?;
        let r = media_row(&ctx, id)?;
        let abs = media_abs(&ctx, &r)
            .map(|(_, a)| PathBuf::from(a))
            .map_err(pa_jobs::HttpError::from);
        Ok(pa_jobs::JobMedia {
            id: r.id,
            folder_id: r.folder_id,
            dir: r.dir.clone(),
            name: r.name.clone(),
            kind: crate::db::kind_of_str(&r.kind),
            width: r.width.map(|v| v as f64),
            height: r.height.map(|v| v as f64),
            duration: r.duration,
            extra: r
                .extra
                .as_deref()
                .and_then(|e| serde_json::from_str(e).ok()),
            abs,
        })
    }

    fn target_dir(&self, folder_id: &str, dir: &str) -> Result<(i64, PathBuf), pa_jobs::HttpError> {
        let ctx = self.ctx()?;
        let (f, abs) = crate::routes::util::folder_path(&ctx, folder_id, dir)?;
        Ok((f.id, abs))
    }

    fn edit_suffix(&self) -> String {
        self.0
            .upgrade()
            .map(|c| c.settings().edit_suffix)
            .unwrap_or_else(|| "_edit".into())
    }

    async fn upsert_file(&self, folder_id: i64, rel: &str) {
        if let Some(c) = self.0.upgrade() {
            crate::scanner::upsert_file(&c, folder_id, rel).await;
        }
    }
}

/// the export runners
pub fn exporter(ctx: &Ctx, weak: Weak<Ctx>) -> pa_jobs::Exporter {
    pa_jobs::Exporter {
        jobs: ctx.jobs.clone(),
        ffmpeg: ctx.ffmpeg.clone(),
        host: Arc::new(JobHost(weak)),
        temp_dir: ctx.cfg.temp_dir.clone(),
    }
}

/// `p` as a string (lossy)
pub fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

// ───────────────────────── the library database for pa-link / pa-remote ─────────────────────────

use crate::db::{num_i, text, FolderRow, MediaRow};
use crate::p;
use pa_jobs::library::{BaseThumb, ListScope, ListedFile, ThumbRow as EThumbRow, ThumbScope};
use pa_jobs::HttpError as EHttp;
use serde_json::Value;

/// Every DB / scanner / thumbnail trait of the engines, over the server context.
/// The database handle is held directly: the engines read it while the context is being built
/// (pa-link's folder owners, pa-remote's persisted quota pause, the MEGA session).
pub struct Store {
    pub ctx: Weak<Ctx>,
    pub db: Arc<crate::db::Db>,
}

impl Store {
    fn ctx(&self) -> Option<Arc<Ctx>> {
        self.ctx.upgrade().filter(|c| !c.db.is_closed())
    }
    fn db(&self) -> Option<&crate::db::Db> {
        (!self.db.is_closed()).then_some(&*self.db)
    }
}

fn scope_sql(scope: &ListScope) -> (String, crate::db::Params) {
    if scope.dir.is_empty() {
        (String::new(), vec![])
    } else {
        (
            " AND (dir = ? OR substr(dir, 1, ?) = ?)".into(),
            p![
                scope.dir.clone(),
                pa_core::js::len16(&scope.dir) as i64 + 1,
                format!("{}/", scope.dir)
            ],
        )
    }
}

fn playback_s(p: pa_core::Playback) -> &'static str {
    crate::routes::util::playback_str(p)
}

fn device_row(r: &rusqlite::Row) -> rusqlite::Result<pa_link::DeviceRow> {
    Ok(pa_link::DeviceRow {
        id: text(r, "id").unwrap_or_default(),
        name: text(r, "name").unwrap_or_default(),
        model: text(r, "model").unwrap_or_default(),
        token_hash: text(r, "token_hash"),
        paired: num_i(r, "paired").unwrap_or(0) != 0,
        caps: text(r, "caps"),
        created_at: num_i(r, "created_at").unwrap_or(0),
        last_seen: num_i(r, "last_seen").unwrap_or(0),
    })
}

fn dev_folder_row(r: &rusqlite::Row) -> rusqlite::Result<pa_link::DeviceFolderRow> {
    Ok(pa_link::DeviceFolderRow {
        id: num_i(r, "id").unwrap_or(0),
        name: text(r, "name").unwrap_or_default(),
        sort: num_i(r, "sort").unwrap_or(0),
        device_id: text(r, "device_id").unwrap_or_default(),
        device_key: text(r, "device_key").unwrap_or_default(),
    })
}

fn remote_folder_row(f: &FolderRow) -> Option<pa_remote::RemoteFolderRow> {
    Some(pa_remote::RemoteFolderRow {
        id: f.id,
        name: f.name.clone(),
        path: f.path.clone(),
        remote: f.remote.clone()?,
        remote_status: f.remote_status.clone(),
    })
}

fn remote_media(r: &MediaRow) -> pa_remote::RemoteMedia {
    pa_remote::RemoteMedia {
        id: r.id,
        folder_id: r.folder_id,
        dir: r.dir.clone(),
        name: r.name.clone(),
        ext: r.ext.clone(),
        kind: crate::db::kind_of_str(&r.kind),
        size: r.size,
        mtime: r.mtime,
        width: r.width.map(|v| v as f64),
        height: r.height.map(|v| v as f64),
        duration: r.duration,
        playback: crate::db::playback_of_str(&r.playback),
    }
}

/// the row facts of a remote media row for `group_columns`
fn row_info(r: &pa_remote::RemoteMedia) -> pa_meta::RowInfo {
    pa_meta::RowInfo {
        name: r.name.clone(),
        dir: r.dir.clone(),
        ext: r.ext.clone(),
        kind: kind_str(r.kind).into(),
        mtime: r.mtime,
    }
}

fn kind_str(k: pa_core::MediaKind) -> &'static str {
    match k {
        pa_core::MediaKind::Image => "image",
        pa_core::MediaKind::Video => "video",
        pa_core::MediaKind::Audio => "audio",
        _ => "archive",
    }
}

fn group_params(g: &pa_meta::GroupColumns) -> crate::db::Params {
    use crate::db::Sql;
    pa_meta::group_params(g).into_iter().map(Sql::sql).collect()
}

impl pa_link::DeviceStore for Store {
    fn device(&self, id: &str) -> Option<pa_link::DeviceRow> {
        self.db()?
            .get("SELECT * FROM devices WHERE id = ?", p![id], device_row)
            .ok()
            .flatten()
    }
    fn paired_devices(&self) -> Vec<pa_link::DeviceRow> {
        self.db()
            .and_then(|d| {
                d.all(
                    "SELECT * FROM devices WHERE paired = 1 ORDER BY last_seen DESC",
                    p![],
                    device_row,
                )
                .ok()
            })
            .unwrap_or_default()
    }
    fn save_paired(
        &self,
        id: &str,
        name: &str,
        model: &str,
        token_hash: &str,
        caps: &str,
        now: i64,
    ) {
        if let Some(c) = self.ctx() {
            let _ = c.db.run(
                "INSERT INTO devices (id, name, model, token_hash, paired, caps, created_at, last_seen) VALUES (?, ?, ?, ?, 1, ?, ?, ?)
                 ON CONFLICT(id) DO UPDATE SET name = excluded.name, model = excluded.model, token_hash = excluded.token_hash, paired = 1, caps = excluded.caps, last_seen = excluded.last_seen",
                p![id, name, model, token_hash, caps, now, now],
            );
        }
    }
    fn update_online(&self, id: &str, name: &str, model: &str, caps: &str, now: i64) {
        if let Some(c) = self.ctx() {
            let _ = c.db.run(
                "UPDATE devices SET name = ?, model = ?, caps = ?, last_seen = ? WHERE id = ?",
                p![name, model, caps, now, id],
            );
        }
    }
    fn set_last_seen(&self, id: &str, now: i64) {
        if let Some(c) = self.ctx() {
            let _ =
                c.db.run("UPDATE devices SET last_seen = ? WHERE id = ?", p![now, id]);
        }
    }
    fn delete_device(&self, id: &str) {
        if let Some(c) = self.ctx() {
            let _ = c.db.run("DELETE FROM devices WHERE id = ?", p![id]);
        }
    }
    fn device_folders(&self, device_id: &str) -> Vec<pa_link::DeviceFolderRow> {
        self.ctx()
            .and_then(|c| {
                c.db.all(
                    "SELECT * FROM folders WHERE device_id = ? ORDER BY sort, id",
                    p![device_id],
                    dev_folder_row,
                )
                .ok()
            })
            .unwrap_or_default()
    }
    fn all_device_folders(&self) -> Vec<(i64, String)> {
        self.db()
            .and_then(|d| {
                d.all(
                    "SELECT id, device_id FROM folders WHERE device_id IS NOT NULL",
                    p![],
                    |r| Ok((r.get::<_, i64>(0)?, text(r, 1).unwrap_or_default())),
                )
                .ok()
            })
            .unwrap_or_default()
    }
    fn device_folder(&self, folder_id: i64) -> Option<pa_link::DeviceFolderRow> {
        self.ctx()?
            .db
            .get(
                "SELECT * FROM folders WHERE id = ? AND device_id IS NOT NULL",
                p![folder_id],
                dev_folder_row,
            )
            .ok()
            .flatten()
    }
    fn sync_device_folders(
        &self,
        device_id: &str,
        update: &[(i64, String, i64)],
        insert: &[(String, String, i64)],
        delete: &[i64],
    ) -> Vec<i64> {
        let Some(c) = self.ctx() else { return vec![] };
        c.db.tx(|cx| {
            for (id, name, sort) in update {
                cx.execute("UPDATE folders SET name = ?, sort = ? WHERE id = ?", rusqlite::params![name, sort, id])?;
            }
            let mut ids = Vec::new();
            for (key, name, sort) in insert {
                cx.execute(
                    "INSERT INTO folders (library_id, path, name, temp, sort, device_id, device_key) VALUES (NULL, ?, ?, 0, ?, ?, ?)",
                    rusqlite::params![format!("{}{device_id}/{key}", crate::library::DEVICE_SCHEME), name, sort, device_id, key],
                )?;
                ids.push(cx.last_insert_rowid());
            }
            for id in delete {
                cx.execute("DELETE FROM folders WHERE id = ?", rusqlite::params![id])?;
            }
            Ok(ids)
        })
        .unwrap_or_default()
    }
    fn folder_refs(&self, device_id: &str) -> Vec<pa_core::FolderRef> {
        let Some(c) = self.ctx() else { return vec![] };
        let counts = crate::library::counts(&c).unwrap_or_default();
        let rows =
            c.db.all(
                "SELECT * FROM folders WHERE device_id = ? ORDER BY sort, id",
                p![device_id],
                FolderRow::from_row,
            )
            .unwrap_or_default();
        rows.iter()
            .map(|f| crate::library::folder_ref(&c, f, Some(&counts)))
            .collect()
    }
}

impl pa_jobs::library::MediaRowSink for Store {
    fn skip_name(&self, name: &str) -> bool {
        self.ctx().is_some_and(|c| c.scanner.skip_name(name))
    }
    fn skip_file(&self, folder_id: i64, rel: &str, size: i64) -> bool {
        self.ctx()
            .is_some_and(|c| crate::scanner::skip_file(&c, folder_id, rel, Some(size)))
    }
    fn skip_dir(&self, folder_id: i64, rel: &str) -> bool {
        self.ctx()
            .is_some_and(|c| crate::scanner::skip_dir(&c, folder_id, rel))
    }
    fn list_begin(&self, scope: &ListScope) -> i64 {
        let Some(c) = self.ctx() else { return 0 };
        let max =
            c.db.int("SELECT max(id) m FROM media", p![])
                .ok()
                .flatten()
                .unwrap_or(0);
        let (s, sp) = scope_sql(scope);
        let mut ps = p![scope.folder_id];
        ps.extend(sp);
        let _ = c.db.run(
            &format!("UPDATE media SET seen = 0 WHERE folder_id = ?{s}"),
            ps,
        );
        max
    }
    fn list_upsert(&self, folder_id: i64, files: &[ListedFile]) -> usize {
        let Some(c) = self.ctx() else { return 0 };
        let tz = c.scanner.home_time_zone();
        let geo = c.geo.index();
        let set_excluded = regex::Regex::new(r"(\w+) = \?")
            .expect("re")
            .replace_all(pa_meta::GROUP_SET_SQL, "$1 = excluded.$1")
            .into_owned();
        c.db.tx(|cx| {
            let mut n = 0;
            for e in files {
                let same = cx.execute(
                    "UPDATE media SET seen = 1 WHERE folder_id = ? AND dir = ? AND name = ? AND size = ? AND mtime = ?",
                    rusqlite::params![folder_id, e.dir, e.name, e.size, e.mtime],
                )?;
                if same > 0 {
                    continue;
                }
                let info = pa_meta::RowInfo { name: e.name.clone(), dir: e.dir.clone(), ext: e.ext.clone(), kind: kind_str(e.kind).into(), mtime: e.mtime };
                let g = pa_meta::group_columns(&info, &e.meta, geo, &tz);
                // MEGA rows (meta_state 3) forget their old facts: the background pass re-reads them
                let on_conflict = if e.meta_state == pa_remote::META_REMOTE_PENDING {
                    "width = NULL, height = NULL, duration = NULL, extra = NULL, playback = excluded.playback, meta_state = excluded.meta_state, meta_v = excluded.meta_v, thumb_state = 0, thumbhash = NULL"
                } else {
                    "width = excluded.width, height = excluded.height, duration = excluded.duration, playback = excluded.playback, meta_state = 1, meta_v = excluded.meta_v, thumb_state = 0, thumbhash = NULL"
                };
                let sql = format!(
                    "INSERT INTO media (folder_id, dir, name, ext, kind, size, mtime, width, height, duration, playback, meta_state, meta_v, thumb_state, seen,
                        taken_at, taken_local, tz_off, time_src, lat, lon, geo_id, cc, admin1, city, make, model, lens, flags)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                     ON CONFLICT (folder_id, dir, name) DO UPDATE SET
                       seen = 1, ext = excluded.ext, kind = excluded.kind, size = excluded.size, mtime = excluded.mtime, {on_conflict}, {set_excluded}"
                );
                let mut ps = p![folder_id, e.dir.clone(), e.name.clone(), e.ext.clone(), kind_str(e.kind), e.size, e.mtime, e.meta.width, e.meta.height, e.meta.duration, playback_s(e.meta.playback), e.meta_state, pa_meta::META_V];
                ps.extend(group_params(&g));
                cx.execute(&sql, rusqlite::params_from_iter(ps))?;
                n += 1;
            }
            Ok(n)
        })
        .unwrap_or(0)
    }
    fn list_abort(&self, scope: &ListScope) {
        let Some(c) = self.ctx() else { return };
        let (s, sp) = scope_sql(scope);
        let mut ps = p![scope.folder_id];
        ps.extend(sp);
        let _ = c.db.run(
            &format!("UPDATE media SET seen = 1 WHERE folder_id = ?{s}"),
            ps,
        );
    }
    fn list_finish(&self, scope: &ListScope, dirs: &[String], max_before: i64) -> (usize, usize) {
        let Some(c) = self.ctx() else { return (0, 0) };
        let (s, sp) = scope_sql(scope);
        let mut ps = p![scope.folder_id];
        ps.extend(sp.clone());
        let removed =
            c.db.run(
                &format!("DELETE FROM media WHERE folder_id = ? AND seen = 0{s}"),
                ps.clone(),
            )
            .unwrap_or(0);
        let _ = c.db.tx(|cx| {
            cx.execute(
                &format!("DELETE FROM device_dirs WHERE folder_id = ?{s}"),
                rusqlite::params_from_iter(ps.clone()),
            )?;
            for d in dirs {
                cx.execute(
                    "INSERT OR IGNORE INTO device_dirs (folder_id, dir) VALUES (?, ?)",
                    rusqlite::params![scope.folder_id, d],
                )?;
            }
            Ok(())
        });
        let added =
            c.db.int(
                "SELECT count(*) c FROM media WHERE folder_id = ? AND id > ?",
                p![scope.folder_id, max_before],
            )
            .ok()
            .flatten()
            .unwrap_or(0);
        (added as usize, removed)
    }
}

#[async_trait::async_trait]
impl pa_jobs::library::ThumbCache for Store {
    async fn generate_base(&self, abs: &str, media_id: i64) -> Result<Option<BaseThumb>, EHttp> {
        let c = self.ctx().ok_or_else(gone)?;
        let r = crate::routes::util::media_row_n(&c, media_id)?;
        let row = crate::thumbs::ThumbRow::from(&r);
        match c
            .thumbs
            .generate_base(abs, &row, crate::thumbs::Prio::Low)
            .await
        {
            Ok(b) => Ok(b.map(|b| BaseThumb {
                thumbhash: b.thumbhash,
                width: b.width,
                height: b.height,
            })),
            Err(e) if e.contains("offline") => Err(EHttp::new(503, "device offline")),
            Err(e) => Err(EHttp::new(500, e)),
        }
    }
    fn set_thumb(
        &self,
        media_id: i64,
        thumbhash: Option<&str>,
        state: i64,
        width: Option<u32>,
        height: Option<u32>,
    ) {
        if let Some(c) = self.ctx() {
            let _ = c.db.run(
                "UPDATE media SET thumbhash = ?, thumb_state = ?, width = COALESCE(width, ?), height = COALESCE(height, ?) WHERE id = ?",
                p![thumbhash.map(String::from), state, width.map(|v| v as i64), height.map(|v| v as i64), media_id],
            );
        }
    }
    fn pending_thumbs(&self, scope: ThumbScope<'_>) -> Vec<EThumbRow> {
        let Some(c) = self.ctx() else { return vec![] };
        let (w, ps) = match scope {
            ThumbScope::Device(d) => ("f.device_id = ?", p![d]),
            ThumbScope::Remote => ("f.remote IS NOT NULL", p![]),
        };
        c.db.all(
            &format!("SELECT m.*, f.path fpath FROM media m JOIN folders f ON f.id = m.folder_id WHERE {w} AND m.thumb_state = 0 AND m.kind IN ('image','video') ORDER BY m.taken_local DESC LIMIT 16"),
            ps,
            |r| {
                let m = MediaRow::from_row(r)?;
                Ok(EThumbRow {
                    id: m.id,
                    folder_id: m.folder_id,
                    dir: m.dir.clone(),
                    name: m.name.clone(),
                    kind: crate::db::kind_of_str(&m.kind),
                    width: m.width.map(|v| v as f64),
                    height: m.height.map(|v| v as f64),
                    folder_path: text(r, "fpath").unwrap_or_default(),
                })
            },
        )
        .unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl pa_link::TransferHost for Store {
    fn folder(&self, folder_id: &str) -> Result<pa_link::HostFolder, EHttp> {
        let c = self.ctx().ok_or_else(gone)?;
        let f = crate::library::folder_row_str(&c, folder_id)?;
        Ok(pa_link::HostFolder {
            id: f.id,
            name: f.name.clone(),
            path: PathBuf::from(&f.path),
            device_id: f.device_id.clone(),
        })
    }
    fn device_folder(&self, folder_id: &str) -> Result<pa_link::DeviceFolderRow, EHttp> {
        let c = self.ctx().ok_or_else(gone)?;
        let f = crate::library::folder_row_str(&c, folder_id)?;
        if !f.is_device() {
            return Err(EHttp::new(400, "Not a device folder"));
        }
        Ok(pa_link::DeviceFolderRow {
            id: f.id,
            name: f.name,
            sort: f.sort,
            device_id: f.device_id.unwrap_or_default(),
            device_key: f.device_key.unwrap_or_default(),
        })
    }
    fn find_media(
        &self,
        folder_id: i64,
        dir: &str,
        name: &str,
    ) -> Option<pa_link::transfer::DeviceMedia> {
        self.ctx()?
            .db
            .get("SELECT dir, name, size, mtime FROM media WHERE folder_id = ? AND dir = ? AND name = ?", p![folder_id, dir, name], |r| {
                Ok(pa_link::transfer::DeviceMedia { dir: text(r, 0).unwrap_or_default(), name: text(r, 1).unwrap_or_default(), size: num_i(r, 2).unwrap_or(0), mtime: num_i(r, 3).unwrap_or(0) })
            })
            .ok()
            .flatten()
    }
    fn media_under(&self, folder_id: i64, rel: &str) -> Vec<pa_link::transfer::DeviceMedia> {
        let Some(c) = self.ctx() else { return vec![] };
        let (s, sp) = scope_sql(&ListScope {
            folder_id,
            dir: rel.to_string(),
        });
        let mut ps = p![folder_id];
        ps.extend(sp);
        c.db.all(&format!("SELECT dir, name, size, mtime FROM media WHERE folder_id = ?{s} ORDER BY dir, name"), ps, |r| {
            Ok(pa_link::transfer::DeviceMedia { dir: text(r, 0).unwrap_or_default(), name: text(r, 1).unwrap_or_default(), size: num_i(r, 2).unwrap_or(0), mtime: num_i(r, 3).unwrap_or(0) })
        })
        .unwrap_or_default()
    }
    fn skip_name(&self, name: &str) -> bool {
        self.ctx().is_some_and(|c| c.scanner.skip_name(name))
    }
    async fn upsert_file(&self, folder_id: i64, rel: &str) {
        if let Some(c) = self.ctx() {
            crate::scanner::upsert_file(&c, folder_id, rel).await;
        }
    }
}

impl pa_remote::RemoteStore for Store {
    fn remote_folders(&self) -> Vec<pa_remote::RemoteFolderRow> {
        let Some(c) = self.ctx() else { return vec![] };
        c.db.all(
            "SELECT * FROM folders WHERE remote IS NOT NULL ORDER BY id",
            p![],
            FolderRow::from_row,
        )
        .unwrap_or_default()
        .iter()
        .filter_map(remote_folder_row)
        .collect()
    }
    fn remote_folder(&self, id: i64) -> Result<Option<pa_remote::RemoteFolderRow>, EHttp> {
        let c = self.ctx().ok_or_else(gone)?;
        let f = crate::library::folder_row(&c, id)?;
        Ok(remote_folder_row(&f))
    }
    fn set_remote_status(&self, id: i64, status_json: &str) {
        if let Some(c) = self.ctx() {
            let _ = c.db.run(
                "UPDATE folders SET remote_status = ? WHERE id = ?",
                p![status_json, id],
            );
        }
    }
    fn indexing_counts(&self, folder_id: i64) -> (i64, i64) {
        let Some(c) = self.ctx() else { return (0, 0) };
        c.db.get(
            "SELECT count(*) total, coalesce(sum(CASE WHEN meta_state = 3 OR thumb_state = 0 THEN 1 ELSE 0 END), 0) todo FROM media WHERE folder_id = ? AND kind IN ('image','video')",
            p![folder_id],
            |r| Ok((num_i(r, 0).unwrap_or(0), num_i(r, 1).unwrap_or(0))),
        )
        .ok()
        .flatten()
        .unwrap_or((0, 0))
    }
    fn get_setting(&self, key: &str) -> Option<Value> {
        self.db()?.get_setting(key)
    }
    fn set_setting(&self, key: &str, value: Value) {
        if let Some(d) = self.db() {
            let _ = d.set_setting(key, &value);
        }
    }
    fn library(&self, id: &str) -> Result<pa_core::Library, EHttp> {
        let c = self.ctx().ok_or_else(gone)?;
        Ok(crate::library::library(&c, id)?)
    }
    fn create_library(&self, name: &str) -> Result<String, EHttp> {
        let c = self.ctx().ok_or_else(gone)?;
        Ok(crate::library::create_library(&c, name, None, Some("cloud".into()), &[])?.id)
    }
    fn add_remote_folder(
        &self,
        library_id: &str,
        f: &pa_remote::NewRemoteFolder,
    ) -> Result<i64, EHttp> {
        let c = self.ctx().ok_or_else(gone)?;
        let lid = pa_core::js::number(library_id);
        if let Some(id) =
            c.db.int(
                "SELECT id FROM folders WHERE library_id = ? AND path = ?",
                p![lid, f.path.clone()],
            )
            .map_err(crate::error::HttpError::from)?
        {
            return Ok(id);
        }
        let n =
            c.db.int(
                "SELECT count(*) c FROM folders WHERE library_id = ?",
                p![lid],
            )
            .map_err(crate::error::HttpError::from)?
            .unwrap_or(0);
        let name = pa_core::js::trim(&f.name);
        let id = c
            .db
            .insert(
                "INSERT INTO folders (library_id, path, name, temp, sort, remote, remote_status) VALUES (?, ?, ?, 0, ?, ?, ?)",
                p![lid, f.path.clone(), if name.is_empty() { "MEGA" } else { name }, n, f.remote.clone(), f.status.clone()],
            )
            .map_err(crate::error::HttpError::from)?;
        c.events.emit(ServerEvent::Folders { status: None });
        Ok(id)
    }
    fn folder_ref(&self, folder_id: i64) -> Result<pa_core::FolderRef, EHttp> {
        let c = self.ctx().ok_or_else(gone)?;
        let f = crate::library::folder_row(&c, folder_id)?;
        Ok(crate::library::folder_ref(&c, &f, None))
    }
    fn media_at(&self, folder_path: &str, dir: &str, name: &str) -> Option<pa_remote::RemoteMedia> {
        self.ctx()?
            .db
            .get("SELECT m.* FROM media m JOIN folders f ON f.id = m.folder_id WHERE f.path = ? AND m.dir = ? AND m.name = ? LIMIT 1", p![folder_path, dir, name], MediaRow::from_row)
            .ok()
            .flatten()
            .map(|r| remote_media(&r))
    }
    fn media(&self, id: i64) -> Option<pa_remote::RemoteMedia> {
        self.ctx()?
            .db
            .get(
                "SELECT * FROM media WHERE id = ?",
                p![id],
                MediaRow::from_row,
            )
            .ok()
            .flatten()
            .map(|r| remote_media(&r))
    }
    fn pending_meta(&self) -> Vec<pa_remote::RemoteMedia> {
        let Some(c) = self.ctx() else { return vec![] };
        c.db.all(
            "SELECT m.* FROM media m JOIN folders f ON f.id = m.folder_id WHERE f.remote IS NOT NULL AND m.kind IN ('image','video') AND (m.meta_state = 3 OR (m.meta_state = 1 AND m.meta_v < ?)) ORDER BY m.taken_local DESC LIMIT 24",
            p![pa_meta::META_V],
            MediaRow::from_row,
        )
        .unwrap_or_default()
        .iter()
        .map(remote_media)
        .collect()
    }
    fn apply_meta(&self, row: &pa_remote::RemoteMedia, meta: &pa_meta::ExtractedMeta) {
        let Some(c) = self.ctx() else { return };
        let g = pa_meta::group_columns(
            &row_info(row),
            meta,
            c.geo.index(),
            &c.scanner.home_time_zone(),
        );
        let extra = meta
            .extra
            .as_ref()
            .and_then(|e| serde_json::to_string(e).ok());
        let mut ps = p![
            meta.width,
            meta.height,
            meta.duration,
            playback_s(meta.playback),
            extra,
            pa_meta::META_V
        ];
        ps.extend(group_params(&g));
        ps.push(crate::db::Sql::sql(row.id));
        let _ = c.db.run(
            &format!("UPDATE media SET width = COALESCE(?, width), height = COALESCE(?, height), duration = COALESCE(?, duration), playback = ?, extra = COALESCE(?, extra), meta_state = 1, meta_v = ?, {} WHERE id = ?", pa_meta::GROUP_SET_SQL),
            ps,
        );
    }
    fn meta_failed(&self, id: i64) {
        if let Some(c) = self.ctx() {
            let _ = c.db.run(
                "UPDATE media SET meta_state = 1, meta_v = ? WHERE id = ?",
                p![pa_meta::META_V, id],
            );
        }
    }
}

#[async_trait::async_trait]
impl pa_remote::ImportHost for Store {
    fn require_writable(&self) -> Result<(), EHttp> {
        let c = self.ctx().ok_or_else(gone)?;
        Ok(crate::routes::util::require_writable(&c)?)
    }
    fn zip_action(&self) -> String {
        self.ctx()
            .map(|c| {
                c.settings_json()
                    .get("zipAction")
                    .and_then(|v| v.as_str().map(String::from))
                    .unwrap_or_else(|| "ask".into())
            })
            .unwrap_or_default()
    }
    fn target(&self, folder_id: &str, dir: &str) -> Result<(i64, PathBuf, PathBuf), EHttp> {
        let c = self.ctx().ok_or_else(gone)?;
        let (f, abs) = crate::routes::util::folder_path(&c, folder_id, dir)?;
        Ok((f.id, PathBuf::from(&f.path), abs))
    }
    fn temp_dir(&self) -> PathBuf {
        self.ctx()
            .map(|c| c.cfg.temp_dir.clone())
            .unwrap_or_default()
    }
    async fn create_temp_folder(&self, content: &Path, name: &str) -> Result<i64, EHttp> {
        let c = self.ctx().ok_or_else(gone)?;
        let hours = crate::ctx::setting_f64(&c.settings_json(), "tempSessionHours").unwrap_or(24.0);
        Ok(crate::library::create_temp_folder(&c, content, name, hours)?.id)
    }
    async fn delete_temp(&self, folder_id: i64) {
        if let Some(c) = self.ctx() {
            if let Ok(f) = crate::library::folder_row(&c, folder_id) {
                let _ = crate::library::delete_temp(&c, &f).await;
            }
        }
    }
    async fn upsert_file(&self, folder_id: i64, rel: &str) {
        if let Some(c) = self.ctx() {
            crate::scanner::upsert_file(&c, folder_id, rel).await;
        }
    }
    async fn scan_folder(&self, folder_id: i64, rel: &str) {
        if let Some(c) = self.ctx() {
            crate::scanner::scan_folder(&c, folder_id, rel).await;
        }
    }
    async fn extract_zip(&self, zip: &Path, dest: &Path) -> Result<(), EHttp> {
        let (z, d) = (zip.to_path_buf(), dest.to_path_buf());
        tokio::task::spawn_blocking(move || {
            crate::zip::extract_zip(&z, &d, crate::zip::MAX_EXTRACT)
        })
        .await
        .map_err(|e| EHttp::new(500, e.to_string()))?
        .map(|_| ())
        .map_err(|e| EHttp::new(500, e.to_string()))
    }
    async fn zip_to_temp(&self, zip: &Path, name: &str) -> Result<String, EHttp> {
        let c = self.ctx().ok_or_else(gone)?;
        Ok(crate::routes::files::zip_to_temp(&c, zip, name).await?.id)
    }
}

impl pa_mega::SessionStore for Store {
    fn load(&self) -> pa_mega::StoredSession {
        self.db()
            .and_then(|d| d.get_setting("megaSession"))
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default()
    }
    fn save(&self, s: &pa_mega::StoredSession) {
        if let (Some(d), Ok(v)) = (self.db(), serde_json::to_value(s)) {
            let _ = d.set_setting("megaSession", &v);
        }
    }
}

// ───────────────────────── thumbnail sources (device:// and remote://) ─────────────────────────

use futures_util::future::{BoxFuture, FutureExt};

/// ThumbService.remote: images of linked phones through the tunnel (pa-link)
pub struct DeviceImages(pub Weak<Ctx>);

impl crate::thumbs::RemoteImageSource for DeviceImages {
    fn thumb(
        &self,
        abs: &str,
        size: u32,
        prio: crate::thumbs::Prio,
    ) -> BoxFuture<'static, Result<Vec<u8>, String>> {
        let (w, abs) = (self.0.clone(), abs.to_string());
        async move {
            let c = w.upgrade().ok_or("device media unavailable")?;
            c.devices
                .link
                .thumb(&abs, size, prio == crate::thumbs::Prio::High)
                .await
                .map_err(|e| e.message)
        }
        .boxed()
    }
    fn display(&self, abs: &str, max: u32) -> BoxFuture<'static, Result<Vec<u8>, String>> {
        let (w, abs) = (self.0.clone(), abs.to_string());
        async move {
            let c = w.upgrade().ok_or("device media unavailable")?;
            c.devices
                .link
                .display(&abs, max)
                .await
                .map_err(|e| e.message)
        }
        .boxed()
    }
    fn strip(&self, abs: &str, n: u32, h: u32) -> BoxFuture<'static, Result<Vec<u8>, String>> {
        let (w, abs) = (self.0.clone(), abs.to_string());
        async move {
            let c = w.upgrade().ok_or("device media unavailable")?;
            c.devices
                .link
                .strip(&abs, n, h)
                .await
                .map_err(|e| e.message)
        }
        .boxed()
    }
}

/// ThumbService.cloud: MEGA links streamed without downloading (pa-remote)
pub struct CloudImages(pub Weak<Ctx>);

impl crate::thumbs::RemoteImageSource for CloudImages {
    fn thumb(
        &self,
        abs: &str,
        size: u32,
        prio: crate::thumbs::Prio,
    ) -> BoxFuture<'static, Result<Vec<u8>, String>> {
        let (w, abs) = (self.0.clone(), abs.to_string());
        async move {
            let c = w.upgrade().ok_or("remote media unavailable")?;
            let p = if prio == crate::thumbs::Prio::High {
                pa_remote::Priority::High
            } else {
                pa_remote::Priority::Low
            };
            c.remote
                .svc
                .thumb(&abs, size, p)
                .await
                .map_err(|e| e.message)
        }
        .boxed()
    }
    fn display(&self, abs: &str, max: u32) -> BoxFuture<'static, Result<Vec<u8>, String>> {
        let (w, abs) = (self.0.clone(), abs.to_string());
        async move {
            let c = w.upgrade().ok_or("remote media unavailable")?;
            c.remote.svc.display(&abs, max).await.map_err(|e| e.message)
        }
        .boxed()
    }
    fn strip(&self, _abs: &str, _n: u32, _h: u32) -> BoxFuture<'static, Result<Vec<u8>, String>> {
        let w = self.0.clone();
        async move {
            let c = w.upgrade().ok_or("remote media unavailable")?;
            c.remote.svc.strip().await.map_err(|e| e.message)
        }
        .boxed()
    }
}
