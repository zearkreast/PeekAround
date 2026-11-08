//! Shared folder watching (1.x `LinkWatch.kt`, FileObserver): file changes become
//! `{"t":"changed","key","dirs"}` frames, debounced and grouped per folder.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::codec;
use crate::prefs::LinkFolder;

/// debounce of the change reports (the server debounces another 2 s)
const DEBOUNCE: Duration = Duration::from_millis(1500);

/// pending (folder key → dirs)
type Pending = Arc<Mutex<HashMap<String, BTreeSet<String>>>>;

/// watches the shared folders while sharing runs
pub struct FolderWatch {
    _watcher: Option<notify::RecommendedWatcher>,
    pending: Pending,
    wake: mpsc::UnboundedSender<()>,
}

/// the directory (relative to its folder) a changed path belongs to
fn dir_of(roots: &[(String, PathBuf)], p: &Path) -> Option<(String, String)> {
    let (key, root) = roots.iter().find(|(_, r)| p.starts_with(r))?;
    let rel = codec::rel_of(root, p);
    if rel.split('/').any(codec::hidden) {
        return None;
    }
    // the server re-lists the reported dir sub tree: the parent covers files and directories
    let parent = match rel.rfind('/') {
        Some(i) => rel[..i].to_string(),
        None => String::new(),
    };
    Some((key.clone(), parent))
}

impl FolderWatch {
    /// Start watching; `send` gets the `changed` frames.
    pub fn start(
        folders: &[LinkFolder],
        send: impl Fn(String) + Send + Sync + 'static,
    ) -> FolderWatch {
        let roots: Vec<(String, PathBuf)> = folders
            .iter()
            .flat_map(|f| {
                // events of deleted files carry the raw path (no canonical form any more)
                let raw = PathBuf::from(&f.path);
                let canon = raw.canonicalize().ok();
                [canon, Some(raw)]
                    .into_iter()
                    .flatten()
                    .map(|p| (f.key.clone(), p))
                    .collect::<Vec<_>>()
            })
            .collect();
        let pending: Pending = Arc::default();
        let (wake, mut rx) = mpsc::unbounded_channel::<()>();
        let p2 = pending.clone();
        let w2 = wake.clone();
        let r2 = roots.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            if matches!(ev.kind, notify::EventKind::Access(_)) {
                return;
            }
            let mut any = false;
            for p in &ev.paths {
                let p = p.canonicalize().unwrap_or_else(|_| p.clone());
                if let Some((k, d)) = dir_of(&r2, &p) {
                    p2.lock().unwrap().entry(k).or_default().insert(d);
                    any = true;
                }
            }
            if any {
                let _ = w2.send(());
            }
        });
        let watcher = match watcher {
            Ok(mut w) => {
                for f in folders {
                    let r = Path::new(&f.path);
                    if let Err(e) = w.watch(r, RecursiveMode::Recursive) {
                        log::warn!("watch {}: {e}", r.display());
                    }
                }
                Some(w)
            }
            Err(e) => {
                log::warn!("folder watch: {e}");
                None
            }
        };
        let p3 = pending.clone();
        tokio::spawn(async move {
            while rx.recv().await.is_some() {
                tokio::time::sleep(DEBOUNCE).await;
                while rx.try_recv().is_ok() {}
                let batch: Vec<(String, BTreeSet<String>)> =
                    p3.lock().unwrap().drain().collect();
                for (k, dirs) in batch {
                    let dirs: Vec<String> = dirs.into_iter().collect();
                    send(codec::changed(&k, &dirs));
                }
            }
        });
        FolderWatch {
            _watcher: watcher,
            pending,
            wake,
        }
    }

    /// report a directory now (after a `receive` wrote into it)
    pub fn touched(&self, key: &str, dir: &str) {
        self.pending
            .lock()
            .unwrap()
            .entry(key.to_string())
            .or_default()
            .insert(dir.to_string());
        let _ = self.wake.send(());
    }
}
