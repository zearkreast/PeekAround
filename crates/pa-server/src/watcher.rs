//! Keeps the index in sync with disk changes made outside PeekAround (mirrors `media/watcher.ts`,
//! chokidar → notify): add / change after the file was stable for 1.2 s (awaitWriteFinish),
//! unlink / unlinkDir, new directories announced; polling mode for network shares.

use crate::ctx::C;
use crate::library;
use crate::paths::to_posix;
use notify::{EventKind, RecursiveMode, Watcher as _};
use pa_core::MediaReason;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::mpsc;

struct Running {
    _watcher: Box<dyn notify::Watcher + Send>,
    task: tokio::task::JoinHandle<()>,
}

/// Watcher state.
#[derive(Default)]
pub struct Watcher {
    watchers: Mutex<HashMap<i64, Running>>,
}

const STABLE: Duration = Duration::from_millis(1200);
/// chokidar's `atomic` delay: an unlink is reported once the path stayed missing this long — and,
/// as in Node (where the route's synchronous code runs before any watcher callback), after
/// PeekAround's own rename / move updated the rows (`relocateFile`)
const UNLINK_DELAY: Duration = Duration::from_millis(100);
const POLL: Duration = Duration::from_millis(250);

fn rel_of(root: &Path, p: &Path) -> Option<String> {
    p.strip_prefix(root).ok().map(to_posix)
}

fn ignored(ctx: &C, folder_id: i64, rel: &str) -> bool {
    if rel.is_empty() || rel.starts_with("..") {
        return false;
    }
    crate::scanner::skip_dir(ctx, folder_id, rel) || rel.split('/').any(|s| s.ends_with(".part"))
}

/// start / stop watchers so they match the folder rows
pub fn sync(ctx: &C) {
    let rows: Vec<_> = library::all_folder_rows(ctx)
        .unwrap_or_default()
        .into_iter()
        .filter(|f| f.temp == 0 && !f.is_device() && !f.is_remote())
        .collect();
    let mut w = ctx.watcher.watchers.lock();
    let want: std::collections::HashSet<i64> = rows.iter().map(|r| r.id).collect();
    let stale: Vec<i64> = w.keys().copied().filter(|id| !want.contains(id)).collect();
    for id in stale {
        if let Some(r) = w.remove(&id) {
            r.task.abort();
        }
    }
    for r in rows {
        if w.contains_key(&r.id) {
            continue;
        }
        if let Some(run) = start(ctx, r.id, PathBuf::from(&r.path)) {
            w.insert(r.id, run);
        }
    }
}

fn start(ctx: &C, folder_id: i64, root: PathBuf) -> Option<Running> {
    let (tx, rx) = mpsc::unbounded_channel::<notify::Event>();
    let handler = move |res: notify::Result<notify::Event>| {
        if let Ok(e) = res {
            let _ = tx.send(e);
        }
    };
    let mut watcher: Box<dyn notify::Watcher + Send> = if ctx.cfg.watch_poll {
        Box::new(
            notify::PollWatcher::new(
                handler,
                notify::Config::default().with_poll_interval(Duration::from_millis(3000)),
            )
            .ok()?,
        )
    } else {
        Box::new(notify::RecommendedWatcher::new(handler, notify::Config::default()).ok()?)
    };
    watcher.watch(&root, RecursiveMode::Recursive).ok()?;
    let c = ctx.clone();
    let task = tokio::spawn(run(c, folder_id, root, rx));
    Some(Running {
        _watcher: watcher,
        task,
    })
}

fn stat(p: &Path) -> Option<(bool, u64, Option<SystemTime>)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.is_dir(), m.len(), m.modified().ok()))
}

async fn run(
    ctx: C,
    folder_id: i64,
    root: PathBuf,
    mut rx: mpsc::UnboundedReceiver<notify::Event>,
) {
    // files waiting to be stable: path → (last size/mtime, since)
    let mut pending: HashMap<PathBuf, (Option<(u64, Option<SystemTime>)>, Instant)> =
        HashMap::new();
    let mut unlinks: HashMap<PathBuf, Instant> = HashMap::new();
    let mut tick = tokio::time::interval(POLL);
    let mut fast = tokio::time::interval(UNLINK_DELAY);
    loop {
        tokio::select! {
            ev = rx.recv() => {
                let Some(ev) = ev else { break };
                if ctx.scanner.stopped() { break; }
                handle_event(&ctx, folder_id, &root, ev, &mut pending, &mut unlinks).await;
            }
            _ = fast.tick(), if !unlinks.is_empty() => {
                let now = Instant::now();
                let due: Vec<PathBuf> = unlinks.iter().filter(|(_, t)| now.duration_since(**t) >= UNLINK_DELAY).map(|(p, _)| p.clone()).collect();
                for p in due {
                    unlinks.remove(&p);
                    if stat(&p).is_none() {
                        if let Some(rel) = rel_of(&root, &p) {
                            unlink(&ctx, folder_id, &rel);
                        }
                    }
                }
            }
            _ = tick.tick() => {
                if pending.is_empty() { continue; }
                let now = Instant::now();
                let mut ready = Vec::new();
                for (p, (last, since)) in pending.iter_mut() {
                    match stat(p) {
                        None => ready.push((p.clone(), false)),
                        Some((true, _, _)) => ready.push((p.clone(), false)),
                        Some((false, size, mtime)) => {
                            let cur = Some((size, mtime));
                            if *last != cur { *last = cur; *since = now; }
                            else if now.duration_since(*since) >= STABLE { ready.push((p.clone(), true)); }
                        }
                    }
                }
                for (p, ok) in ready {
                    pending.remove(&p);
                    if ok {
                        if let Some(rel) = rel_of(&root, &p) {
                            crate::scanner::upsert_file(&ctx, folder_id, &rel).await;
                        }
                    }
                }
            }
            _ = ctx.shutdown.cancelled() => break,
        }
    }
}

fn add_tree(
    ctx: &C,
    folder_id: i64,
    root: &Path,
    dir: &Path,
    pending: &mut HashMap<PathBuf, (Option<(u64, Option<SystemTime>)>, Instant)>,
) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let Some(rel) = rel_of(root, &p) else {
            continue;
        };
        if ignored(ctx, folder_id, &rel) {
            continue;
        }
        match e.file_type() {
            Ok(t) if t.is_dir() => {
                ctx.events.media_changed(
                    &folder_id.to_string(),
                    crate::paths::dir_of_rel(&rel),
                    MediaReason::Change,
                );
                add_tree(ctx, folder_id, root, &p, pending);
            }
            Ok(t) if t.is_file() => {
                pending.insert(p, (None, Instant::now()));
            }
            _ => {}
        }
    }
}

async fn handle_event(
    ctx: &C,
    folder_id: i64,
    root: &Path,
    ev: notify::Event,
    pending: &mut HashMap<PathBuf, (Option<(u64, Option<SystemTime>)>, Instant)>,
    unlinks: &mut HashMap<PathBuf, Instant>,
) {
    if matches!(ev.kind, EventKind::Access(_)) {
        return;
    }
    for p in ev.paths {
        let Some(rel) = rel_of(root, &p) else {
            continue;
        };
        if rel.is_empty() || ignored(ctx, folder_id, &rel) {
            continue;
        }
        match stat(&p) {
            Some((true, _, _)) => {
                // addDir (new or moved in): its files come as adds
                if matches!(
                    ev.kind,
                    EventKind::Create(_) | EventKind::Modify(notify::event::ModifyKind::Name(_))
                ) {
                    ctx.events.media_changed(
                        &folder_id.to_string(),
                        crate::paths::dir_of_rel(&rel),
                        MediaReason::Change,
                    );
                    add_tree(ctx, folder_id, root, &p, pending);
                }
            }
            Some((false, _, _)) => {
                if pa_core::kind_of(&rel).is_some() {
                    pending.entry(p.clone()).or_insert((None, Instant::now()));
                }
            }
            None => {
                pending.remove(&p);
                unlinks.insert(p.clone(), Instant::now());
            }
        }
    }
}

/// unlink / unlinkDir of a path that stayed missing
fn unlink(ctx: &C, folder_id: i64, rel: &str) {
    let (dir, name) = match rel.rfind('/') {
        Some(i) => (rel[..i].to_string(), rel[i + 1..].to_string()),
        None => (String::new(), rel.to_string()),
    };
    let is_file = ctx
        .db
        .int(
            "SELECT 1 FROM media WHERE folder_id = ? AND dir = ? AND name = ?",
            crate::p![folder_id, dir, name],
        )
        .ok()
        .flatten()
        .is_some();
    if is_file {
        crate::scanner::remove_file(ctx, folder_id, rel);
    } else {
        let under = ctx
            .db
            .int("SELECT 1 FROM media WHERE folder_id = ? AND (dir = ? OR substr(dir, 1, ?) = ?) LIMIT 1", crate::p![folder_id, rel, pa_core::js::len16(rel) as i64 + 1, format!("{rel}/")])
            .ok()
            .flatten()
            .is_some();
        if under || pa_core::kind_of(rel).is_none() {
            crate::scanner::remove_dir(ctx, folder_id, rel);
        }
    }
}

/// stop every watcher
pub fn close(ctx: &C) {
    for (_, r) in ctx.watcher.watchers.lock().drain() {
        r.task.abort();
    }
}
