//! Scanner (mirrors `media/scanner.ts`): the folder walk, the metadata pass, the thumbnail pass and
//! the low-priority enrichment loop (rows of an older META_V), with batched conditional writes,
//! library exclusions / hidden folders / minimum size, and row relocation (ids survive renames).

use crate::ctx::{Ctx, C};
use crate::db::{FolderRow, MediaRow, Sql};
use crate::events::now_ms;
use crate::library::{self, DEVICE_SCHEME, REMOTE_SCHEME};
use crate::p;
use crate::paths::{is_hidden_name, trim_chars};
use crate::thumbs::{Prio, ThumbRow};
use pa_core::{ExcludeMatcher, MediaReason, ServerEvent};
use parking_lot::{Mutex, RwLock};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// the trash folder inside every library folder
pub const TRASH_DIR: &str = ".peekaround-trash";

const UNCHANGED: &str = "size = ? AND mtime = ?";
const CHANGED: &str = "media.size != excluded.size OR media.mtime != excluded.mtime";

fn upsert_sql() -> String {
    format!(
        "INSERT INTO media (folder_id, dir, name, ext, kind, size, mtime, taken_at, taken_local, time_src, seen)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 1)
ON CONFLICT (folder_id, dir, name) DO UPDATE SET
  seen = 1,
  meta_state = CASE WHEN {CHANGED} THEN 0 ELSE media.meta_state END,
  thumb_state = CASE WHEN {CHANGED} THEN 0 ELSE media.thumb_state END,
  meta_v = CASE WHEN {CHANGED} THEN 0 ELSE media.meta_v END,
  taken_at = CASE WHEN {CHANGED} THEN excluded.taken_at ELSE media.taken_at END,
  taken_local = CASE WHEN {CHANGED} THEN excluded.taken_local ELSE media.taken_local END,
  time_src = CASE WHEN {CHANGED} THEN excluded.time_src ELSE media.time_src END,
  size = excluded.size,
  mtime = excluded.mtime"
    )
}

/// per-folder scan rules from the library options
pub struct FolderRules {
    matcher: ExcludeMatcher,
    min_bytes: i64,
    show_hidden: Option<bool>,
}

type WriteFn = Box<dyn FnOnce(&Connection) -> rusqlite::Result<()> + Send>;
type AfterFn = Box<dyn FnOnce() + Send>;

/// how often the enrichment loop publishes its progress
const ENRICH_FLUSH: Duration = Duration::from_secs(30);

/// Scanner state.
pub struct Scanner {
    scanning: Mutex<HashSet<i64>>,
    meta_running: AtomicBool,
    thumb_running: AtomicBool,
    enrich_running: AtomicBool,
    home_tz: RwLock<String>,
    progress: Mutex<(i64, i64)>,
    last_progress: Mutex<Option<Instant>>,
    stopped: AtomicBool,
    show_hidden: AtomicBool,
    concurrency: AtomicUsize,
    rules: Arc<Mutex<HashMap<i64, Arc<FolderRules>>>>,
    rescan_after: Mutex<HashSet<i64>>,
    writes: Mutex<(Vec<(WriteFn, Option<AfterFn>)>, Instant)>,
    sub_key: AtomicI64,
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

impl Scanner {
    pub fn new() -> Self {
        Scanner {
            scanning: Mutex::new(HashSet::new()),
            meta_running: AtomicBool::new(false),
            thumb_running: AtomicBool::new(false),
            enrich_running: AtomicBool::new(false),
            home_tz: RwLock::new(crate::db::system_time_zone()),
            progress: Mutex::new((0, 0)),
            last_progress: Mutex::new(None),
            stopped: AtomicBool::new(false),
            show_hidden: AtomicBool::new(false),
            concurrency: AtomicUsize::new(2),
            rules: Arc::new(Mutex::new(HashMap::new())),
            rescan_after: Mutex::new(HashSet::new()),
            writes: Mutex::new((Vec::new(), Instant::now())),
            sub_key: AtomicI64::new(-1),
        }
    }

    /// the rule cache (cleared on folder events)
    pub fn rules_handle(&self) -> Arc<Mutex<HashMap<i64, Arc<FolderRules>>>> {
        self.rules.clone()
    }

    /// a scan, meta or thumbnail pass is running
    pub fn busy(&self) -> bool {
        !self.scanning.lock().is_empty()
            || self.meta_running.load(Ordering::SeqCst)
            || self.thumb_running.load(Ordering::SeqCst)
    }

    /// nothing left to do (no enrichment either)
    pub fn idle(&self) -> bool {
        !self.busy() && !self.enrich_running.load(Ordering::SeqCst)
    }

    /// a full walk of one of these folders is running
    pub fn busy_for(&self, ids: &[i64]) -> bool {
        let s = self.scanning.lock();
        ids.iter().any(|i| s.contains(i))
    }

    /// enrichment loop running
    pub fn enriching(&self) -> bool {
        self.enrich_running.load(Ordering::SeqCst)
    }

    /// Settings.showHidden
    pub fn set_show_hidden(&self, v: bool) {
        self.show_hidden.store(v, Ordering::SeqCst);
    }

    /// global hidden-name setting
    pub fn show_hidden(&self) -> bool {
        self.show_hidden.load(Ordering::SeqCst)
    }

    /// background items at once
    pub fn set_concurrency(&self, n: usize) {
        self.concurrency.store(n.max(1), Ordering::SeqCst);
    }

    /// (meta, thumb) concurrency
    pub fn concurrency(&self) -> (usize, usize) {
        let n = self.concurrency.load(Ordering::SeqCst);
        (n, n)
    }

    /// zone for UTC-only timestamps
    pub fn home_time_zone(&self) -> String {
        self.home_tz.read().clone()
    }

    /// set without re-deriving (start-up)
    pub fn init_home_time_zone(&self, tz: &str) {
        *self.home_tz.write() = tz.to_string();
    }

    /// shutting down
    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    fn local_of(&self, instant: i64) -> i64 {
        pa_core::instant_to_local(instant, &self.home_tz.read())
    }

    /// forget cached library options
    pub fn invalidate_rules(&self) {
        self.rules.lock().clear();
    }

    /// trash + hidden names (global setting)
    pub fn skip_name(&self, name: &str) -> bool {
        name == TRASH_DIR || (!self.show_hidden() && is_hidden_name(name))
    }
}

/// absolute path (or pseudo path) of a row
pub fn abs(folder: &FolderRow, dir: &str, name: &str) -> String {
    if folder.path.starts_with(DEVICE_SCHEME) || folder.path.starts_with(REMOTE_SCHEME) {
        return format!("{}/{}", folder.path, pa_core::join_rel(dir, name));
    }
    let mut p = PathBuf::from(&folder.path);
    for s in dir.split('/').filter(|s| !s.is_empty()) {
        p.push(s);
    }
    p.push(name);
    p.to_string_lossy().into_owned()
}

fn rules_of(ctx: &Ctx, folder_id: i64) -> Arc<FolderRules> {
    if let Some(r) = ctx.scanner.rules.lock().get(&folder_id) {
        return r.clone();
    }
    let o = library::options_of_folder(ctx, folder_id);
    let r = Arc::new(FolderRules {
        matcher: pa_core::exclude_matcher(&o.exclude),
        min_bytes: o.min_bytes,
        show_hidden: o.show_hidden,
    });
    ctx.scanner.rules.lock().insert(folder_id, r.clone());
    r
}

fn skip_name_in(ctx: &Ctx, name: &str, r: &FolderRules) -> bool {
    if name == TRASH_DIR {
        return true;
    }
    !r.show_hidden.unwrap_or_else(|| ctx.scanner.show_hidden()) && is_hidden_name(name)
}

/// a directory (folder-relative) that must not be indexed / listed
pub fn skip_dir(ctx: &Ctx, folder_id: i64, rel: &str) -> bool {
    if rel.is_empty() {
        return false;
    }
    let r = rules_of(ctx, folder_id);
    if rel.split('/').any(|s| skip_name_in(ctx, s, &r)) {
        return true;
    }
    !r.matcher.is_empty() && r.matcher.path(rel)
}

/// a file that must not be indexed
pub fn skip_file(ctx: &Ctx, folder_id: i64, rel: &str, size: Option<i64>) -> bool {
    let r = rules_of(ctx, folder_id);
    if rel.split('/').any(|s| skip_name_in(ctx, s, &r)) {
        return true;
    }
    if !r.matcher.is_empty() && r.matcher.path(rel) {
        return true;
    }
    if let Some(sz) = size {
        if r.min_bytes > 0
            && sz < r.min_bytes
            && pa_core::kind_of(rel) == Some(pa_core::MediaKind::Image)
        {
            return true;
        }
    }
    false
}

/// Home zone changed: re-derive the clock that was inferred from it.
pub fn set_home_time_zone(ctx: &C, tz: &str) {
    if *ctx.scanner.home_tz.read() == tz {
        return;
    }
    *ctx.scanner.home_tz.write() = tz.to_string();
    let _ = ctx.db.tx(|c| {
        c.execute(
            "UPDATE media SET taken_local = peek_local(taken_at, ?) WHERE time_src IN (0, 3)",
            [tz],
        )?;
        c.execute(
            "UPDATE media SET taken_at = peek_instant(taken_local, ?) WHERE time_src = 1",
            [tz],
        )?;
        Ok(())
    });
    if let Ok(rows) = library::all_folder_rows(ctx) {
        for f in rows {
            ctx.events
                .media_changed(&f.id.to_string(), "*", MediaReason::Meta);
        }
    }
    ctx.explore.invalidate(ctx, true);
}

/// scan every folder, one after the other
pub async fn scan_all(ctx: &C) {
    let Ok(rows) = library::all_folder_rows(ctx) else {
        return;
    };
    for f in rows {
        scan_folder(ctx, f.id, "").await;
    }
}

fn emit_scan(ctx: &Ctx, force: bool) {
    {
        let mut last = ctx.scanner.last_progress.lock();
        if !force && last.is_some_and(|l| l.elapsed() < Duration::from_millis(700)) {
            return;
        }
        *last = Some(Instant::now());
    }
    let (done, total) = *ctx.scanner.progress.lock();
    ctx.events.emit(ServerEvent::Scan {
        scanning: ctx.scanner.busy(),
        done,
        total,
    });
}

/// (Re)scan one folder — or only `sub_dir` of it — and drop rows of files that disappeared.
pub fn scan_folder<'a>(
    ctx: &'a C,
    folder_id: i64,
    sub_dir: &'a str,
) -> futures_util::future::BoxFuture<'a, ()> {
    use futures_util::FutureExt;
    async move {
        if ctx.scanner.stopped() {
            return;
        }
        let row = ctx
            .db
            .get(
                "SELECT device_id, remote FROM folders WHERE id = ?",
                p![folder_id],
                |r| Ok((crate::db::text(r, 0), crate::db::text(r, 1))),
            )
            .ok()
            .flatten();
        if let Some((dev, remote)) = &row {
            if remote.as_deref().is_some_and(|s| !s.is_empty()) {
                ctx.remote.scan(ctx, folder_id);
                return;
            }
            if dev.as_deref().is_some_and(|s| !s.is_empty()) {
                ctx.devices.scan(ctx, folder_id, sub_dir).await;
                return;
            }
        }
        let lock_key = if sub_dir.is_empty() {
            folder_id
        } else {
            ctx.scanner.sub_key.fetch_sub(1, Ordering::SeqCst) - 1_000_000_000
        };
        {
            let mut s = ctx.scanner.scanning.lock();
            if sub_dir.is_empty() && s.contains(&lock_key) {
                ctx.scanner.rescan_after.lock().insert(folder_id);
                return;
            }
            s.insert(lock_key);
        }
        emit_scan(ctx, true);
        let c2 = ctx.clone();
        let sub = trim_chars(sub_dir, "/").to_string();
        let _ = tokio::task::spawn_blocking(move || walk_folder(&c2, folder_id, &sub)).await;
        ctx.scanner.scanning.lock().remove(&lock_key);
        if sub_dir.is_empty() && ctx.scanner.rescan_after.lock().remove(&folder_id) {
            let c3 = ctx.clone();
            ctx.tasks
                .spawn(async move { scan_folder(&c3, folder_id, "").await });
        }
        kick(ctx);
    }
    .boxed()
}

fn sorted_entries(dir: &Path) -> Option<Vec<std::fs::DirEntry>> {
    let mut v: Vec<std::fs::DirEntry> = std::fs::read_dir(dir).ok()?.flatten().collect();
    // libuv's scandir sorts with strcmp
    v.sort_by(|a, b| {
        a.file_name()
            .as_encoded_bytes()
            .cmp(b.file_name().as_encoded_bytes())
    });
    Some(v)
}

fn walk_folder(ctx: &C, folder_id: i64, sub: &str) {
    let Ok(folder) = library::folder_row(ctx, folder_id) else {
        return;
    };
    let rules = rules_of(ctx, folder_id);
    let (scope, scope_params): (&str, crate::db::Params) = if sub.is_empty() {
        ("", vec![])
    } else {
        (
            " AND (dir = ? OR substr(dir, 1, ?) = ?)",
            p![sub, pa_core::js::len16(sub) as i64 + 1, format!("{sub}/")],
        )
    };
    let mut ps = p![folder_id];
    ps.extend(scope_params.clone());
    let _ = ctx.db.run(
        &format!("UPDATE media SET seen = 0 WHERE folder_id = ?{scope}"),
        ps.clone(),
    );
    let max_before = ctx
        .db
        .int("SELECT max(id) m FROM media", p![])
        .ok()
        .flatten()
        .unwrap_or(0);
    let upsert = upsert_sql();
    let mut batch: Vec<crate::db::Params> = Vec::new();
    let flush = |batch: &mut Vec<crate::db::Params>| {
        if batch.is_empty() || ctx.scanner.stopped() {
            return;
        }
        let rows = std::mem::take(batch);
        let _ = ctx.db.tx(|c| {
            let mut st = c.prepare_cached(&upsert)?;
            for r in rows {
                st.execute(rusqlite::params_from_iter(r))?;
            }
            Ok(())
        });
    };
    fn walk(
        ctx: &C,
        folder_id: i64,
        rules: &FolderRules,
        dir_abs: &Path,
        rel: &str,
        batch: &mut Vec<crate::db::Params>,
        flush: &dyn Fn(&mut Vec<crate::db::Params>),
    ) {
        if ctx.scanner.stopped() {
            return;
        }
        let Some(entries) = sorted_entries(dir_abs) else {
            return;
        };
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            if skip_name_in(ctx, &name, rules) {
                continue;
            }
            let p = e.path();
            let child_rel = pa_core::join_rel(rel, &name);
            let ft = match e.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_dir() {
                if !rules.matcher.is_empty() && rules.matcher.path(&child_rel) {
                    continue;
                }
                walk(ctx, folder_id, rules, &p, &child_rel, batch, flush);
                continue;
            }
            let Some(kind) = pa_core::kind_of(&name) else {
                continue;
            };
            if !rules.matcher.is_empty() && rules.matcher.path(&child_rel) {
                continue;
            }
            let Ok(st) = std::fs::metadata(&p) else {
                continue;
            };
            if !st.is_file() {
                continue;
            }
            let size = st.len() as i64;
            if rules.min_bytes > 0 && kind == pa_core::MediaKind::Image && size < rules.min_bytes {
                continue;
            }
            let mt = mtime_ms(&st);
            batch.push(p![
                folder_id,
                rel,
                name.clone(),
                pa_core::ext_of(&name),
                kind.as_str(),
                size,
                mt,
                mt,
                ctx.scanner.local_of(mt)
            ]);
            if batch.len() >= 400 {
                flush(batch);
            }
        }
    }
    let root = PathBuf::from(&folder.path);
    if sub.is_empty() || !skip_dir(ctx, folder_id, sub) {
        let start = if sub.is_empty() {
            root.clone()
        } else {
            sub.split('/').fold(root.clone(), |p, s| p.join(s))
        };
        walk(ctx, folder_id, &rules, &start, sub, &mut batch, &flush);
    }
    if ctx.scanner.stopped() {
        return;
    }
    flush(&mut batch);
    let removed = ctx
        .db
        .run(
            &format!("DELETE FROM media WHERE folder_id = ? AND seen = 0{scope}"),
            ps,
        )
        .unwrap_or(0);
    if sub.is_empty() {
        let _ = ctx.db.run(
            "UPDATE folders SET scanned_at = ? WHERE id = ?",
            p![now_ms(), folder_id],
        );
    }
    let added = ctx
        .db
        .int(
            "SELECT count(*) c FROM media WHERE folder_id = ? AND id > ?",
            p![folder_id, max_before],
        )
        .ok()
        .flatten()
        .unwrap_or(0);
    let fid = folder_id.to_string();
    if added > 0 {
        ctx.events.media_changed(&fid, "*", MediaReason::Add);
    } else if removed > 0 {
        ctx.events.media_changed(&fid, "*", MediaReason::Remove);
    }
    if added > 0 || removed > 0 {
        ctx.explore.invalidate(ctx, true);
    }
    ctx.events.emit(ServerEvent::Folders { status: None });
}

/// ms mtime rounded like `Math.round(st.mtimeMs)`
pub fn mtime_ms(md: &std::fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .map(|t| match t.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => (d.as_secs_f64() * 1000.0).round() as i64,
            Err(e) => -((e.duration().as_secs_f64() * 1000.0).round() as i64),
        })
        .unwrap_or(0)
}

fn split_rel(rel: &str) -> (String, String) {
    let rel = trim_chars(rel, "/");
    match rel.rfind('/') {
        Some(i) => (rel[..i].to_string(), rel[i + 1..].to_string()),
        None => (String::new(), rel.to_string()),
    }
}

fn posix(rel: &str) -> String {
    rel.replace('\\', "/")
}

/// Incremental update for a single file (watcher, uploads, edits).
pub async fn upsert_file(ctx: &C, folder_id: i64, rel_path: &str) -> Option<i64> {
    if ctx.scanner.stopped() {
        return None;
    }
    let folder = library::folder_row(ctx, folder_id).ok()?;
    if folder.is_device() || folder.is_remote() {
        return None;
    }
    let rel = trim_chars(&posix(rel_path), "/").to_string();
    let (dir, name) = split_rel(&rel);
    if skip_file(ctx, folder_id, &rel, None) {
        return None;
    }
    let kind = pa_core::kind_of(&name)?;
    let st = match tokio::fs::metadata(abs(&folder, &dir, &name)).await {
        Ok(st) => st,
        Err(_) => {
            remove_file(ctx, folder_id, &rel);
            return None;
        }
    };
    if !st.is_file() || ctx.scanner.stopped() {
        return None;
    }
    let size = st.len() as i64;
    if skip_file(ctx, folder_id, &rel, Some(size)) {
        remove_file(ctx, folder_id, &rel);
        return None;
    }
    let mt = mtime_ms(&st);
    let before = ctx
        .db
        .get(
            "SELECT size, mtime FROM media WHERE folder_id = ? AND dir = ? AND name = ?",
            p![folder_id, dir.clone(), name.clone()],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .ok()
        .flatten();
    let _ = ctx.db.run(
        &upsert_sql(),
        p![
            folder_id,
            dir.clone(),
            name.clone(),
            pa_core::ext_of(&name),
            kind.as_str(),
            size,
            mt,
            mt,
            ctx.scanner.local_of(mt)
        ],
    );
    let fid = folder_id.to_string();
    match before {
        None => ctx.events.media_changed(&fid, &dir, MediaReason::Add),
        Some((s, m)) if s != size || m != mt => {
            ctx.events.media_changed(&fid, &dir, MediaReason::Change)
        }
        _ => {}
    }
    kick(ctx);
    ctx.db
        .int(
            "SELECT id FROM media WHERE folder_id = ? AND dir = ? AND name = ?",
            p![folder_id, dir, name],
        )
        .ok()
        .flatten()
}

/// drop the row of a file
pub fn remove_file(ctx: &Ctx, folder_id: i64, rel_path: &str) {
    if ctx.scanner.stopped() {
        return;
    }
    let (dir, name) = split_rel(&posix(rel_path));
    let n = ctx
        .db
        .run(
            "DELETE FROM media WHERE folder_id = ? AND dir = ? AND name = ?",
            p![folder_id, dir.clone(), name],
        )
        .unwrap_or(0);
    if n > 0 {
        ctx.events
            .media_changed(&folder_id.to_string(), &dir, MediaReason::Remove);
    }
}

/// drop the rows below a directory
pub fn remove_dir(ctx: &Ctx, folder_id: i64, rel_dir: &str) {
    if ctx.scanner.stopped() {
        return;
    }
    let p2 = posix(rel_dir);
    let dir = trim_chars(&p2, "/");
    let _ = ctx.db.run(
        "DELETE FROM media WHERE folder_id = ? AND (dir = ? OR substr(dir, 1, ?) = ?)",
        p![
            folder_id,
            dir,
            pa_core::js::len16(dir) as i64 + 1,
            format!("{dir}/")
        ],
    );
    let fid = folder_id.to_string();
    ctx.events
        .media_changed(&fid, crate::paths::dir_of_rel(dir), MediaReason::Remove);
    ctx.events.media_changed(&fid, dir, MediaReason::Remove);
}

fn folder_kind(ctx: &Ctx, id: i64) -> Option<(Option<String>, Option<String>)> {
    ctx.db
        .get(
            "SELECT device_id, remote FROM folders WHERE id = ?",
            p![id],
            |r| Ok((crate::db::text(r, 0), crate::db::text(r, 1))),
        )
        .ok()
        .flatten()
}

/// A file renamed / moved by PeekAround: move its row (the media id survives).
pub fn relocate_file(
    ctx: &C,
    from_folder: i64,
    from_rel: &str,
    to_folder: i64,
    to_rel: &str,
) -> bool {
    if ctx.scanner.stopped() {
        return false;
    }
    let src = trim_chars(&posix(from_rel), "/").to_string();
    let dst = trim_chars(&posix(to_rel), "/").to_string();
    let (fd, fname) = split_rel(&src);
    let (td, tn) = split_rel(&dst);
    let row = ctx
        .db
        .get(
            "SELECT id, kind, size FROM media WHERE folder_id = ? AND dir = ? AND name = ?",
            p![from_folder, fd.clone(), fname],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    crate::db::text(r, 1).unwrap_or_default(),
                    r.get::<_, i64>(2)?,
                ))
            },
        )
        .ok()
        .flatten();
    let Some((id, kind, size)) = row else {
        return false;
    };
    let Some((dev, rem)) = folder_kind(ctx, to_folder) else {
        return false;
    };
    if dev.is_some_and(|d| !d.is_empty())
        || rem.is_some_and(|d| !d.is_empty())
        || pa_core::kind_of(&tn).map(|k| k.as_str()) != Some(kind.as_str())
    {
        return false;
    }
    if skip_file(ctx, to_folder, &dst, Some(size)) {
        remove_file(ctx, from_folder, &src);
        return true;
    }
    let _ = ctx.db.tx(|c| {
        c.execute(
            "DELETE FROM media WHERE folder_id = ? AND dir = ? AND name = ? AND id != ?",
            rusqlite::params![to_folder, td, tn, id],
        )?;
        c.execute(
            "UPDATE media SET folder_id = ?, dir = ?, name = ?, ext = ? WHERE id = ?",
            rusqlite::params![to_folder, td, tn, pa_core::ext_of(&tn), id],
        )?;
        Ok(())
    });
    ctx.events
        .media_changed(&from_folder.to_string(), &fd, MediaReason::Remove);
    ctx.events
        .media_changed(&to_folder.to_string(), &td, MediaReason::Add);
    ctx.explore.invalidate(ctx, true);
    true
}

/// A directory renamed / moved by PeekAround: move the rows under it. Returns the rows moved.
pub fn relocate_dir(
    ctx: &C,
    from_folder: i64,
    from_rel: &str,
    to_folder: i64,
    to_rel: &str,
) -> usize {
    if ctx.scanner.stopped() {
        return 0;
    }
    let src = trim_chars(&posix(from_rel), "/").to_string();
    let dst = trim_chars(&posix(to_rel), "/").to_string();
    if src.is_empty() || dst.is_empty() {
        return 0;
    }
    let Some((dev, rem)) = folder_kind(ctx, to_folder) else {
        return 0;
    };
    if dev.is_some_and(|d| !d.is_empty()) || rem.is_some_and(|d| !d.is_empty()) {
        return 0;
    }
    let under = "(dir = ? OR substr(dir, 1, ?) = ?)";
    let n = ctx
        .db
        .tx(|c| {
            c.execute(&format!("DELETE FROM media WHERE folder_id = ? AND {under}"), rusqlite::params![to_folder, dst, pa_core::js::len16(&dst) as i64 + 1, format!("{dst}/")])?;
            c.execute(
                &format!("UPDATE media SET folder_id = ?, dir = CASE WHEN dir = ? THEN ? ELSE ? || substr(dir, ?) END WHERE folder_id = ? AND {under}"),
                rusqlite::params![to_folder, src, dst, dst, pa_core::js::len16(&src) as i64 + 1, from_folder, src, pa_core::js::len16(&src) as i64 + 1, format!("{src}/")],
            )
        })
        .unwrap_or(0);
    if n > 0 {
        ctx.events.media_changed(
            &from_folder.to_string(),
            crate::paths::dir_of_rel(&src),
            MediaReason::Remove,
        );
        ctx.events
            .media_changed(&from_folder.to_string(), &src, MediaReason::Remove);
        ctx.events
            .media_changed(&to_folder.to_string(), &dst, MediaReason::Add);
        ctx.explore.invalidate(ctx, true);
    }
    n
}

/// Start the background metadata + thumbnail workers if idle.
pub fn kick(ctx: &C) {
    if ctx.scanner.stopped() {
        return;
    }
    if ctx
        .scanner
        .meta_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        let c = ctx.clone();
        ctx.tasks.spawn(async move { meta_loop(&c).await });
    }
    kick_thumbs(ctx);
}

fn kick_thumbs(ctx: &C) {
    if ctx.scanner.stopped() {
        return;
    }
    if ctx
        .scanner
        .thumb_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        let c = ctx.clone();
        ctx.tasks.spawn(async move { thumb_loop(&c).await });
    }
}

/// Start the low-priority enrichment loop once scanning + the meta pass are done.
pub fn kick_enrich(ctx: &C) {
    let s = &ctx.scanner;
    if s.stopped() || s.meta_running.load(Ordering::SeqCst) || !s.scanning.lock().is_empty() {
        return;
    }
    if s.enrich_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        let c = ctx.clone();
        ctx.tasks.spawn(async move { enrich_loop(&c).await });
    }
}

/// Stop background work (server shutdown).
pub fn stop(ctx: &Ctx) {
    ctx.scanner.stopped.store(true, Ordering::SeqCst);
    ctx.thumbs.bg.close();
}

fn write(ctx: &Ctx, run: WriteFn, after: Option<AfterFn>) {
    if ctx.scanner.stopped() {
        return;
    }
    let flush_now = {
        let mut w = ctx.scanner.writes.lock();
        if w.0.is_empty() {
            w.1 = Instant::now();
        }
        w.0.push((run, after));
        w.0.len() >= 48 || w.1.elapsed() > Duration::from_secs(1)
    };
    if flush_now {
        flush_writes(ctx);
    }
}

fn flush_writes(ctx: &Ctx) {
    let list = std::mem::take(&mut ctx.scanner.writes.lock().0);
    if list.is_empty() || ctx.scanner.stopped() {
        return;
    }
    let mut afters = Vec::new();
    let r = ctx.db.tx(|c| {
        for (run, after) in list {
            let _ = run(c);
            if let Some(a) = after {
                afters.push(a);
            }
        }
        Ok(())
    });
    if r.is_err() {
        return;
    }
    for a in afters {
        a();
    }
}

fn pending_count(ctx: &Ctx) -> i64 {
    ctx.db.int("SELECT count(*) c FROM media WHERE meta_state = 0", p![]).ok().flatten().unwrap_or(0)
        + ctx
            .db
            .int(
                "SELECT count(*) c FROM media WHERE thumb_state = 0 AND kind IN ('image','video') AND folder_id IN (SELECT id FROM folders WHERE device_id IS NULL AND remote IS NULL)",
                p![],
            )
            .ok()
            .flatten()
            .unwrap_or(0)
}

/// `bg.each(items, fn, stop)`: at most `size` items at once, every item in a slot of the shared pool
async fn each<F, Fut>(
    ctx: &C,
    rows: Vec<MediaRow>,
    stop: impl Fn() -> bool + Send + Sync + Clone + 'static,
    f: F,
) where
    F: Fn(C, MediaRow) -> Fut + Send + Sync + Clone + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let (_, size) = ctx.thumbs.bg.stats();
    let n = rows.len().min(size.max(1));
    let rows = Arc::new(rows);
    let next = Arc::new(AtomicUsize::new(0));
    let mut lanes = Vec::new();
    for _ in 0..n {
        let (rows, next, stop, f, ctx) = (
            rows.clone(),
            next.clone(),
            stop.clone(),
            f.clone(),
            ctx.clone(),
        );
        lanes.push(tokio::spawn(async move {
            loop {
                if stop() {
                    break;
                }
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(r) = rows.get(i).cloned() else { break };
                let (stop2, f2, c2) = (stop.clone(), f.clone(), ctx.clone());
                ctx.thumbs
                    .bg
                    .run(async move {
                        if !stop2() {
                            f2(c2, r).await
                        }
                    })
                    .await;
            }
        }));
    }
    for l in lanes {
        let _ = l.await;
    }
}

fn convert_probe(p: pa_media::ProbeResult) -> pa_core::ProbeResult {
    pa_core::ProbeResult {
        duration: p.duration,
        width: p.width.map(i64::from),
        height: p.height.map(i64::from),
        rotation: p.rotation as i32,
        codec: p.codec,
        audio_codec: p.audio_codec,
        fps: p.fps,
        bitrate: p.bitrate,
        container: p.container,
        creation_time: p.creation_time.map(|t| t as i64),
        has_audio: p.has_audio,
        tags: p.tags,
    }
}

/// `extractMeta(abs, row, ffmpeg, known?)`
pub async fn extract_meta(
    ctx: &Ctx,
    abs: &str,
    r: &MediaRow,
    known: bool,
) -> Result<pa_meta::ExtractedMeta, String> {
    let kind = crate::db::kind_of_str(&r.kind);
    let probe = if r.kind == "video" || r.kind == "audio" {
        ctx.ffmpeg
            .probe(abs, &pa_media::ProbeOptions::default())
            .await
            .map(convert_probe)
    } else {
        None
    };
    let known = if known {
        Some(pa_meta::Known {
            width: r.width.map(|w| w as f64),
            height: r.height.map(|h| h as f64),
        })
    } else {
        None
    };
    let abs_p = PathBuf::from(abs);
    let ext = r.ext.clone();
    let is_media = matches!(r.kind.as_str(), "image" | "video" | "audio");
    tokio::task::spawn_blocking(move || {
        if !is_media {
            return pa_meta::ExtractedMeta::default();
        }
        pa_meta::extract_meta(&abs_p, kind, &ext, probe.as_ref(), known)
    })
    .await
    .map_err(|e| e.to_string())
}

/// `groupColumns(r, m, geo, homeTimeZone)`
pub fn group_columns(ctx: &Ctx, r: &MediaRow, m: &pa_meta::ExtractedMeta) -> pa_meta::GroupColumns {
    let info = pa_meta::RowInfo {
        name: r.name.clone(),
        dir: r.dir.clone(),
        ext: r.ext.clone(),
        kind: r.kind.clone(),
        mtime: r.mtime,
    };
    pa_meta::group_columns(&info, m, ctx.geo.index(), &ctx.scanner.home_time_zone())
}

fn group_params(g: &pa_meta::GroupColumns) -> crate::db::Params {
    pa_meta::group_params(g).into_iter().map(Sql::sql).collect()
}

fn folder_of(ctx: &Ctx, id: i64) -> Option<FolderRow> {
    library::folder_row(ctx, id).ok()
}

async fn meta_loop(ctx: &C) {
    *ctx.scanner.progress.lock() = (0, pending_count(ctx));
    loop {
        if ctx.scanner.stopped() {
            break;
        }
        let rows = ctx
            .db
            .all(
                "SELECT * FROM media WHERE meta_state = 0 LIMIT 64",
                p![],
                MediaRow::from_row,
            )
            .unwrap_or_default();
        if rows.is_empty() {
            break;
        }
        let c0 = ctx.clone();
        each(ctx, rows, move || c0.scanner.stopped(), |ctx: C, r: MediaRow| async move {
            if ctx.scanner.stopped() {
                return;
            }
            let Some(folder) = folder_of(&ctx, r.folder_id) else { return };
            let a = abs(&folder, &r.dir, &r.name);
            let done = {
                let ctx = ctx.clone();
                let (fid, dir) = (r.folder_id.to_string(), r.dir.clone());
                move || {
                    ctx.events.media_changed(&fid, &dir, MediaReason::Meta);
                    ctx.scanner.progress.lock().0 += 1;
                    emit_scan(&ctx, false);
                }
            };
            match extract_meta(&ctx, &a, &r, false).await {
                Ok(m) => {
                    if ctx.scanner.stopped() {
                        return;
                    }
                    let g = group_columns(&ctx, &r, &m);
                    let mut ps = p![m.width, m.height, m.duration, crate::routes::util::playback_str(m.playback), m.extra.as_ref().and_then(|e| serde_json::to_string(e).ok()), pa_meta::META_V];
                    ps.extend(group_params(&g));
                    ps.extend(p![r.id, r.size, r.mtime]);
                    let sql = format!("UPDATE media SET width = ?, height = ?, duration = ?, playback = ?, extra = ?, meta_state = 1, meta_v = ?, {} WHERE id = ? AND {UNCHANGED}", pa_meta::GROUP_SET_SQL);
                    write(&ctx, Box::new(move |c| c.prepare_cached(&sql)?.execute(rusqlite::params_from_iter(ps)).map(|_| ())), Some(Box::new(done)));
                }
                Err(_) => {
                    let ps = p![pa_meta::META_V, r.id, r.size, r.mtime];
                    write(
                        &ctx,
                        Box::new(move |c| c.prepare_cached(&format!("UPDATE media SET meta_state = 2, meta_v = ? WHERE id = ? AND {UNCHANGED}"))?.execute(rusqlite::params_from_iter(ps)).map(|_| ())),
                        Some(Box::new(done)),
                    );
                }
            }
        })
        .await;
        flush_writes(ctx);
        ctx.explore.invalidate(ctx, true);
    }
    flush_writes(ctx);
    ctx.scanner.meta_running.store(false, Ordering::SeqCst);
    kick_thumbs(ctx);
    kick_enrich(ctx);
    emit_scan(ctx, true);
}

async fn thumb_loop(ctx: &C) {
    loop {
        if ctx.scanner.stopped() {
            break;
        }
        let rows = ctx
            .db
            .all(
                "SELECT * FROM media WHERE thumb_state = 0 AND meta_state != 0 AND kind IN ('image','video') AND folder_id IN (SELECT id FROM folders WHERE device_id IS NULL AND remote IS NULL) ORDER BY taken_at DESC LIMIT 32",
                p![],
                MediaRow::from_row,
            )
            .unwrap_or_default();
        if rows.is_empty() {
            break;
        }
        let c0 = ctx.clone();
        each(ctx, rows, move || c0.scanner.stopped(), |ctx: C, r: MediaRow| async move {
            if ctx.scanner.stopped() {
                return;
            }
            let Some(folder) = folder_of(&ctx, r.folder_id) else { return };
            let a = abs(&folder, &r.dir, &r.name);
            let res = ctx.thumbs.generate_base(&a, &ThumbRow::from(&r), Prio::Low).await;
            if ctx.scanner.stopped() {
                return;
            }
            let need_dims = (r.width.unwrap_or(0) == 0 || r.height.unwrap_or(0) == 0) && r.kind == "image";
            let reason = if matches!(res, Ok(Some(_))) && need_dims { MediaReason::Meta } else { MediaReason::Thumb };
            let done = {
                let ctx = ctx.clone();
                let (fid, dir) = (r.folder_id.to_string(), r.dir.clone());
                move || {
                    ctx.events.media_changed(&fid, &dir, reason);
                    ctx.scanner.progress.lock().0 += 1;
                    emit_scan(&ctx, false);
                }
            };
            match res {
                Ok(Some(b)) => {
                    let ps = p![b.thumbhash, if need_dims { Some(b.width as i64) } else { None }, if need_dims { Some(b.height as i64) } else { None }, r.id, r.size, r.mtime];
                    write(
                        &ctx,
                        Box::new(move |c| {
                            c.prepare_cached(&format!("UPDATE media SET thumbhash = ?, thumb_state = 1, width = COALESCE(width, ?), height = COALESCE(height, ?) WHERE id = ? AND {UNCHANGED}"))?
                                .execute(rusqlite::params_from_iter(ps))
                                .map(|_| ())
                        }),
                        Some(Box::new(done)),
                    );
                }
                _ => {
                    let ps = p![r.id, r.size, r.mtime];
                    write(
                        &ctx,
                        Box::new(move |c| c.prepare_cached(&format!("UPDATE media SET thumb_state = 2 WHERE id = ? AND {UNCHANGED}"))?.execute(rusqlite::params_from_iter(ps)).map(|_| ())),
                        Some(Box::new(done)),
                    );
                }
            }
        })
        .await;
        flush_writes(ctx);
    }
    flush_writes(ctx);
    ctx.scanner.thumb_running.store(false, Ordering::SeqCst);
    emit_scan(ctx, true);
    kick_enrich(ctx);
}

/// image/video rows still waiting for the grouping metadata (ExploreResult.indexing)
pub fn indexing(ctx: &Ctx) -> pa_core::Progress {
    let r = ctx
        .db
        .get(
            "SELECT count(*) total, coalesce(sum(CASE WHEN meta_v < ? AND meta_state != 2 THEN 1 ELSE 0 END), 0) todo FROM media WHERE kind IN ('image','video') AND folder_id IN (SELECT id FROM folders WHERE temp = 0 AND device_id IS NULL)",
            p![pa_meta::META_V],
            |r| Ok((r.get::<_, i64>(0)?, crate::db::num_i(r, 1).unwrap_or(0))),
        )
        .ok()
        .flatten()
        .unwrap_or((0, 0));
    pa_core::Progress {
        done: r.0 - r.1,
        total: r.0,
    }
}

async fn enrich_loop(ctx: &C) {
    let touched: Arc<Mutex<HashMap<i64, indexmap::IndexSet<String>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let changed = Arc::new(AtomicBool::new(false));
    let mut last_flush = Instant::now();
    let flush = |ctx: &C| {
        let t = std::mem::take(&mut *touched.lock());
        for (fid, dirs) in t {
            for d in dirs {
                ctx.events
                    .media_changed(&fid.to_string(), &d, MediaReason::Meta);
            }
        }
        if changed.swap(false, Ordering::SeqCst) {
            ctx.events.bump_throttled();
            ctx.explore.invalidate(ctx, true);
        }
    };
    let mut last_id = 0i64;
    loop {
        let s = &ctx.scanner;
        if s.stopped() || s.meta_running.load(Ordering::SeqCst) || !s.scanning.lock().is_empty() {
            break;
        }
        if s.thumb_running.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        let rows = ctx
            .db
            .all(
                "SELECT * FROM media WHERE meta_v < ? AND meta_state = 1 AND kind IN ('image','video') AND id > ? AND folder_id IN (SELECT id FROM folders WHERE remote IS NULL) ORDER BY id LIMIT 32",
                p![pa_meta::META_V, last_id],
                MediaRow::from_row,
            )
            .unwrap_or_default();
        let Some(last) = rows.last() else { break };
        last_id = last.id;
        let c0 = ctx.clone();
        let (touched2, changed2) = (touched.clone(), changed.clone());
        each(
            ctx,
            rows,
            move || c0.scanner.stopped() || c0.scanner.meta_running.load(Ordering::SeqCst) || !c0.scanner.scanning.lock().is_empty(),
            move |ctx: C, r: MediaRow| {
                let (touched, changed) = (touched2.clone(), changed2.clone());
                async move {
                    if ctx.scanner.stopped() {
                        return;
                    }
                    let folder = folder_of(&ctx, r.folder_id);
                    let a = folder.as_ref().map(|f| abs(f, &r.dir, &r.name));
                    let st = match &a {
                        Some(a) => tokio::fs::metadata(a).await.ok(),
                        None => None,
                    };
                    let (Some(a), Some(_)) = (a, st) else {
                        let ps = p![pa_meta::META_V, r.id];
                        write(&ctx, Box::new(move |c| c.prepare_cached("UPDATE media SET meta_v = ? WHERE id = ?")?.execute(rusqlite::params_from_iter(ps)).map(|_| ())), None);
                        return;
                    };
                    match extract_meta(&ctx, &a, &r, true).await {
                        Ok(mut m) => {
                            if ctx.scanner.stopped() {
                                return;
                            }
                            if m.width.is_none_or(|w| w == 0.0) || m.height.is_none_or(|h| h == 0.0) {
                                m.width = r.width.map(|w| w as f64);
                                m.height = r.height.map(|h| h as f64);
                            }
                            let g = group_columns(&ctx, &r, &m);
                            let mut ps = p![pa_meta::META_V, m.width, m.height];
                            ps.extend(group_params(&g));
                            ps.extend(p![r.id, r.size, r.mtime]);
                            let sql = format!("UPDATE media SET meta_v = ?, width = COALESCE(width, ?), height = COALESCE(height, ?), {} WHERE id = ? AND {UNCHANGED}", pa_meta::GROUP_SET_SQL);
                            let new_local = g.taken_local;
                            let had_width = r.width.unwrap_or(0) != 0;
                            let got_width = m.width.is_some_and(|w| w != 0.0);
                            write(
                                &ctx,
                                Box::new(move |c| c.prepare_cached(&sql)?.execute(rusqlite::params_from_iter(ps)).map(|_| ())),
                                Some(Box::new(move || {
                                    changed.store(true, Ordering::SeqCst);
                                    if Some(new_local) != r.taken_local || (!had_width && got_width) {
                                        touched.lock().entry(r.folder_id).or_default().insert(r.dir.clone());
                                    }
                                })),
                            );
                        }
                        Err(_) => {
                            let ps = p![pa_meta::META_V, r.id, r.size, r.mtime];
                            write(
                                &ctx,
                                Box::new(move |c| c.prepare_cached(&format!("UPDATE media SET meta_v = ? WHERE id = ? AND {UNCHANGED}"))?.execute(rusqlite::params_from_iter(ps)).map(|_| ())),
                                None,
                            );
                        }
                    }
                }
            },
        )
        .await;
        flush_writes(ctx);
        if last_flush.elapsed() > ENRICH_FLUSH {
            flush(ctx);
            last_flush = Instant::now();
        }
    }
    flush_writes(ctx);
    ctx.scanner.enrich_running.store(false, Ordering::SeqCst);
    flush(ctx);
}
