//! Remote (streaming) folders (mirrors `remote/folders.ts`): a MEGA folder or file link added to a
//! library WITHOUT downloading it.
//!
//!  - listing: the link's tree → media rows (dir = path inside the link, mtime = node ts);
//!    re-listed every 30 min and on demand (diff: new rows added, vanished ones deleted);
//!  - bytes are fetched on demand with HTTP ranges (AES-CTR → random access), ≤ 4 transfers per
//!    link (2 for background work), originals that were viewed land in a size-capped LRU cache;
//!  - thumbnails: MEGA file attributes (preview → 120px thumbnail) → full download (images ≤ 30 MB)
//!    → a video frame via ffmpeg over the internal loopback URL;
//!  - metadata: background, rate limited: the first 256 KB of images → pa-meta, videos → ffmpeg
//!    probe over loopback; rows wait in meta_state 3 until then;
//!  - transfer quota (509 / EOVERQUOTA): no MEGA transfer until it resets (state 'quota',
//!    retryAt); an unreachable MEGA keeps the old rows (state 'offline') and retries with backoff.
//!
//! The link (with its key) lives in `folders.remote` and never leaves the server except through
//! [`RemoteService::link_of`] (the local-only `GET /api/remote/folders/:id/link`).

use crate::backend::MegaBackend;
use crate::cache::RemoteCache;
use crate::import::{parse_url_value, ImportOptions, Importer};
use crate::store::{ImportHost, NewRemoteFolder, RemoteFolderRow, RemoteMedia, RemoteStore};
use crate::{hx, mega_error, to_mega};
use bytes::Bytes;
use futures_util::future::{BoxFuture, Shared};
use futures_util::{FutureExt, Stream, StreamExt, TryStreamExt};
use pa_core::{
    FileRef, Job, LinkKind, MediaKind, MediaReason, Playback, Progress, RemoteErrorCode,
    RemoteFolderCreated, RemoteFolderInfo, RemoteFolderState, RemoteFolderStatus, RemoteProvider,
    ServerEvent,
};
use pa_jobs::library::{
    content_disposition, http_date, mime_of, parse_range, single_range, BodyStream, FileReply,
    ListScope, ListedFile, MediaRowSink, ThumbCache, ThumbScope,
};
use pa_jobs::paths::{join_rel, normalize_rel};
use pa_jobs::{EventSink, HttpError, JobService};
use pa_media::Ffmpeg;
use pa_mega::client::ByteStream;
use pa_mega::transfer::LaneGuard;
use pa_mega::tree::{remote_paths, MAX_REMOTE_NODES};
use pa_mega::{parse_mega_url, LinkType, MegaError, MegaLink, MegaNode, Priority, TransferGate};
use pa_meta::ExtractedMeta;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

pub const REMOTE_REFRESH_MS: u64 = 30 * 60_000;
const OFFLINE_RETRY_MS: [i64; 5] = [60_000, 2 * 60_000, 5 * 60_000, 10 * 60_000, 30 * 60_000];
const HEAD_BYTES: u64 = 256 * 1024;
const FULL_THUMB_MAX: i64 = 30 * 1024 * 1024;
/// meta_state of remote rows waiting for enrichment
pub const META_REMOTE_PENDING: i64 = 3;
pub const REMOTE_SCHEME: &str = "remote://";

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `folders.remote` (holds the link WITH its key)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSpec {
    pub kind: String,
    pub url: String,
    pub link_id: String,
    pub link_kind: LinkKind,
}

/// A loaded link: rel path → node (names made unique per dir like the rows).
pub struct Tree {
    pub link: MegaLink,
    pub root: Arc<MegaNode>,
    /// rel path → file node, in listing order
    pub files: Vec<(String, MegaNode)>,
    file_ix: HashMap<String, usize>,
    /// rel path → directory node id ('' = the root of a folder link)
    pub dirs: Vec<(String, String)>,
    pub at: i64,
}

impl std::fmt::Debug for Tree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tree")
            .field("link", &self.link.id)
            .field("files", &self.files.len())
            .field("dirs", &self.dirs.len())
            .field("at", &self.at)
            .finish()
    }
}

impl Tree {
    pub fn file(&self, rel: &str) -> Option<&MegaNode> {
        self.file_ix.get(rel).map(|i| &self.files[*i].1)
    }
    pub fn dir_id(&self, rel: &str) -> Option<&str> {
        self.dirs.iter().find(|d| d.0 == rel).map(|d| d.1.as_str())
    }
}

fn build_tree(link: MegaLink, root: MegaNode) -> Tree {
    let (files, dirs) = {
        let p = remote_paths(&root, MAX_REMOTE_NODES);
        let files: Vec<(String, MegaNode)> = p
            .files
            .iter()
            .map(|(r, n)| {
                let mut n = (*n).clone();
                n.children = Vec::new();
                (r.clone(), n)
            })
            .collect();
        let dirs: Vec<(String, String)> = p
            .dirs
            .iter()
            .map(|(r, n)| (r.clone(), n.id.clone()))
            .collect();
        (files, dirs)
    };
    let file_ix = files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.0.clone(), i))
        .collect();
    Tree {
        link,
        root: Arc::new(root),
        files,
        file_ix,
        dirs,
        at: now_ms(),
    }
}

fn playback_of(kind: MediaKind, ext: &str) -> Playback {
    match kind {
        MediaKind::Image if pa_core::BROWSER_IMAGE_EXT.contains(&ext) => Playback::Direct,
        MediaKind::Image => Playback::Convert,
        MediaKind::Video if pa_core::is_browser_playable(ext, None) => Playback::Direct,
        MediaKind::Video => Playback::Transcode,
        _ => Playback::Direct,
    }
}

/// `errorCode(err, msg)` of folders.ts
pub fn error_code(err: &HttpError) -> RemoteErrorCode {
    let msg = &err.message;
    if err.status == 429 {
        RemoteErrorCode::Quota
    } else if msg.contains("taken down") {
        RemoteErrorCode::Blocked
    } else if err.status == 404 {
        RemoteErrorCode::NotFound
    } else if err.status == 400 {
        RemoteErrorCode::Key
    } else if err.status == 502 && msg.to_lowercase().contains("reach") {
        RemoteErrorCode::Network
    } else if err.status == 503 {
        RemoteErrorCode::Unavailable
    } else {
        RemoteErrorCode::Other
    }
}

/// `${folder.path}/${joinRel(dir, name)}` (scanner.abs of remote folders)
pub fn abs_of(folder_path: &str, dir: &str, name: &str) -> String {
    format!("{folder_path}/{}", join_rel(dir, name))
}

/// What a [`RemoteService`] runs on.
pub struct RemoteDeps {
    pub store: Arc<dyn RemoteStore>,
    pub rows: Arc<dyn MediaRowSink>,
    pub thumbs: Arc<dyn ThumbCache>,
    pub events: Arc<dyn EventSink>,
    pub import: Arc<dyn ImportHost>,
    pub jobs: JobService,
    pub ffmpeg: Arc<Ffmpeg>,
    pub mega: Arc<dyn MegaBackend>,
    /// `<cacheDir>/remote`
    pub cache_dir: PathBuf,
    /// `Settings.remoteCacheMb * 1 MiB`
    pub cache_bytes: u64,
}

type TreeFuture = Shared<BoxFuture<'static, Result<Arc<Tree>, HttpError>>>;
type FetchFuture = Shared<BoxFuture<'static, Result<(PathBuf, bool), MegaError>>>;

#[derive(Default)]
struct State {
    trees: HashMap<i64, Arc<Tree>>,
    listing: HashMap<i64, TreeFuture>,
    failures: HashMap<i64, u32>,
    /// thumbnails that came from MEGA's 120px square (their size says nothing about the aspect)
    squares: HashSet<String>,
    fetching: HashMap<String, FetchFuture>,
    meta_running: bool,
    thumb_running: bool,
    progress_pending: bool,
    /// (self url, internal token) of the loopback inputs
    loopback: Option<(String, String)>,
}

/// `ctx.remote`
pub struct RemoteService {
    deps: RemoteDeps,
    st: Mutex<State>,
    closed: AtomicBool,
    /// quota pause + API back-off + per-link lanes; `shutdown` aborts every transfer
    pub gate: TransferGate,
    /// the persisted quota pause (`remoteQuota.until`, 0 = none)
    quota_mark: AtomicU64,
    pub cache: RemoteCache,
    importer: Importer,
    tracker: TaskTracker,
    me: Weak<RemoteService>,
    /// test hooks
    pub meta_gap_ms: AtomicU64,
    pub refresh_ms: AtomicU64,
}

/// A transfer stream that holds its lane slot until it ends / is dropped, notes MEGA errors
/// (quota) and stops its transfer token when dropped.
struct Guarded {
    inner: ByteStream,
    guard: Option<LaneGuard>,
    token: CancellationToken,
    svc: Weak<RemoteService>,
}

impl Stream for Guarded {
    type Item = Result<Bytes, MegaError>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let r = self.inner.poll_next_unpin(cx);
        match &r {
            Poll::Ready(None) => self.guard = None,
            Poll::Ready(Some(Err(e))) => {
                if !e.is_aborted() {
                    if let Some(s) = self.svc.upgrade() {
                        s.note_error(e);
                    }
                }
                self.guard = None;
            }
            _ => {}
        }
        r
    }
}

impl Drop for Guarded {
    fn drop(&mut self) {
        self.guard = None;
        self.token.cancel();
    }
}

fn io_err(e: MegaError) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

fn empty_body() -> BodyStream {
    Box::pin(futures_util::stream::empty())
}

/// parse a route / spec id like `Number(id)` (NaN / fractions → None)
fn id_of(s: &str) -> Option<i64> {
    let n = pa_core::js::number(s);
    (n.is_finite() && n.fract() == 0.0).then_some(n as i64)
}

impl RemoteService {
    /// `new RemoteService(ctx)`: restores the persisted quota pause, starts the 60 s tick.
    pub fn new(deps: RemoteDeps) -> Arc<Self> {
        let cache = RemoteCache::new(&deps.cache_dir, deps.cache_bytes);
        let importer = Importer {
            jobs: deps.jobs.clone(),
            mega: deps.mega.clone(),
            host: deps.import.clone(),
            events: deps.events.clone(),
        };
        let gate = TransferGate::new(4, 2);
        let mut mark = 0u64;
        if let Some(until) = deps
            .store
            .get_setting("remoteQuota")
            .and_then(|q| q.get("until").and_then(Value::as_f64))
        {
            if until > now_ms() as f64 {
                mark = until as u64;
                gate.set_quota_until(mark);
            }
        }
        let svc = Arc::new_cyclic(|me| RemoteService {
            deps,
            st: Mutex::new(State::default()),
            closed: AtomicBool::new(false),
            gate,
            quota_mark: AtomicU64::new(mark),
            cache,
            importer,
            tracker: TaskTracker::new(),
            me: me.clone(),
            meta_gap_ms: AtomicU64::new(150),
            refresh_ms: AtomicU64::new(REMOTE_REFRESH_MS),
        });
        let weak = Arc::downgrade(&svc);
        svc.tracker.spawn(async move {
            let shutdown = match weak.upgrade() {
                Some(s) => s.gate.shutdown.clone(),
                None => return,
            };
            let mut iv = tokio::time::interval(Duration::from_secs(60));
            iv.tick().await;
            loop {
                tokio::select! {
                    _ = iv.tick() => {}
                    _ = shutdown.cancelled() => return,
                }
                match weak.upgrade() {
                    Some(s) => s.tick(),
                    None => return,
                }
            }
        });
        svc
    }

    fn arc(&self) -> Arc<Self> {
        self.me.upgrade().expect("RemoteService alive")
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// the server's own loopback URL + `X-Peek-Internal` token (video probe / frames / transcode)
    pub fn set_loopback(&self, self_url: &str, internal_token: &str) {
        self.st.lock().unwrap().loopback = Some((
            self_url.trim_end_matches('/').to_string(),
            internal_token.to_string(),
        ));
    }

    fn loopback(&self) -> Option<(String, String)> {
        self.st.lock().unwrap().loopback.clone()
    }

    /// `quotaUntil` (ms epoch, 0 = no pause)
    pub fn quota_until(&self) -> u64 {
        self.gate.quota_until()
    }

    /// test hook / admin: set (or with 0 lift) the quota pause without persisting it
    pub fn set_quota_until(&self, until: u64) {
        self.gate.set_quota_until(until);
        self.quota_mark.store(until, Ordering::SeqCst);
    }

    // ───────────────────────── folder rows / status ─────────────────────────

    /// `spec(folder)` (400 'Not a remote folder')
    pub fn spec(&self, folder: &RemoteFolderRow) -> Result<RemoteSpec, HttpError> {
        serde_json::from_str(&folder.remote).map_err(|_| HttpError::new(400, "Not a remote folder"))
    }

    fn stored_status(folder: &RemoteFolderRow) -> RemoteFolderStatus {
        let def = |state| RemoteFolderStatus {
            state,
            error: None,
            code: None,
            retry_at: None,
            listed_at: None,
        };
        if let Some(Ok(Value::Object(m))) = folder
            .remote_status
            .as_deref()
            .map(serde_json::from_str::<Value>)
        {
            let mut base = serde_json::to_value(def(RemoteFolderState::Ok)).unwrap();
            for (k, v) in m {
                base[k] = v;
            }
            if let Ok(s) = serde_json::from_value(base) {
                return s;
            }
        }
        def(RemoteFolderState::Listing)
    }

    fn set_status(&self, folder_id: i64, patch: impl FnOnce(&mut RemoteFolderStatus)) {
        let Ok(Some(row)) = self.deps.store.remote_folder(folder_id) else {
            return;
        };
        let mut next = Self::stored_status(&row);
        patch(&mut next);
        self.deps
            .store
            .set_remote_status(folder_id, &serde_json::to_string(&next).unwrap());
        self.deps
            .events
            .emit(ServerEvent::Folders { status: Some(true) });
    }

    /// `FolderRef.remote` — never contains the key (`lib.remoteLookup`)
    pub fn info(&self, folder: &RemoteFolderRow) -> Result<RemoteFolderInfo, HttpError> {
        let spec = self.spec(folder)?;
        let mut st = Self::stored_status(folder);
        let quota = self.gate.quota_until() > 0
            && st.state != RemoteFolderState::Error
            && st.state != RemoteFolderState::Listing;
        if quota {
            st.state = RemoteFolderState::Quota;
            st.code = Some(RemoteErrorCode::Quota);
            st.retry_at = Some(self.gate.quota_until() as i64);
            st.error = Some("MEGA transfer quota exceeded".into());
        }
        let (total, todo) = self.deps.store.indexing_counts(folder.id);
        let kind = match spec.link_kind {
            LinkKind::File => "file",
            LinkKind::Folder => "folder",
        };
        Ok(RemoteFolderInfo {
            status: st,
            provider: RemoteProvider::Mega,
            label: format!("mega.nz/{kind}/{}", spec.link_id),
            link_kind: spec.link_kind,
            indexing: Progress {
                done: total - todo,
                total,
            },
        })
    }

    /// the full link (with key) of a remote folder — local clients only (see [`link_is_local`])
    pub fn link_of(&self, folder: &RemoteFolderRow) -> Result<String, HttpError> {
        Ok(self.spec(folder)?.url)
    }

    fn folder_row(&self, id: i64) -> Result<RemoteFolderRow, HttpError> {
        self.deps
            .store
            .remote_folder(id)?
            .ok_or_else(|| HttpError::new(400, "Not a remote folder"))
    }

    // ───────────────────────── adding / listing ─────────────────────────

    /// `create(input)` (`POST /api/remote/folders`, after `requireWritable`): lists the link first.
    pub async fn create(&self, input: &Value) -> Result<RemoteFolderCreated, HttpError> {
        let link = parse_url_value(input.get("url"))?;
        let root = self
            .deps
            .mega
            .load(&link)
            .await
            .map_err(|e| mega_error(&e))?;
        let s = |k: &str| {
            input
                .get(k)
                .and_then(Value::as_str)
                .map(pa_core::js::trim)
                .filter(|s| !s.is_empty())
        };
        let name: String = pa_core::js::slice16_to(
            s("name").unwrap_or(if root.name.is_empty() {
                "MEGA"
            } else {
                &root.name
            }),
            120,
        )
        .to_string();
        let library_id = match input.get("libraryId").filter(|v| crate::truthy(v)) {
            Some(v) => self.deps.store.library(&pa_core::js::to_string_of(v))?.id,
            None => self
                .deps
                .store
                .create_library(s("libraryName").unwrap_or(&name))?,
        };
        let url = input
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let spec = RemoteSpec {
            kind: "mega".into(),
            url,
            link_id: link.id.clone(),
            link_kind: if link.kind == LinkType::Folder {
                LinkKind::Folder
            } else {
                LinkKind::File
            },
        };
        let path = format!(
            "{REMOTE_SCHEME}mega/{}{}",
            link.id,
            link.sub
                .as_ref()
                .map(|s| format!("-{}", s.id))
                .unwrap_or_default()
        );
        let status = RemoteFolderStatus {
            state: RemoteFolderState::Ok,
            error: None,
            code: None,
            retry_at: None,
            listed_at: Some(now_ms()),
        };
        let id = self.deps.store.add_remote_folder(
            &library_id,
            &NewRemoteFolder {
                path,
                name,
                remote: serde_json::to_string(&spec).unwrap(),
                status: serde_json::to_string(&status).unwrap(),
            },
        )?;
        let tree = Arc::new(build_tree(link, root));
        self.st.lock().unwrap().trees.insert(id, tree.clone());
        if let Ok(Some(row)) = self.deps.store.remote_folder(id) {
            self.ingest(&row, &tree);
        }
        self.kick();
        Ok(RemoteFolderCreated {
            library: self.deps.store.library(&library_id)?,
            folder: self.deps.store.folder_ref(id)?,
        })
    }

    /// re-list now (deduplicated); resolves when the rows are up to date (`scanner.remoteScan`).
    /// The listing runs to its end even when the returned future is dropped.
    pub fn refresh(&self, folder_id: i64) -> TreeFuture {
        let mut st = self.st.lock().unwrap();
        if let Some(f) = st.listing.get(&folder_id) {
            return f.clone();
        }
        let me = self.arc();
        let fut = async move {
            let r = me.relist(folder_id).await;
            me.st.lock().unwrap().listing.remove(&folder_id);
            r
        }
        .boxed()
        .shared();
        st.listing.insert(folder_id, fut.clone());
        drop(st);
        let f2 = fut.clone();
        self.tracker.spawn(async move {
            let _ = f2.await;
        });
        fut
    }

    async fn relist(&self, folder_id: i64) -> Result<Arc<Tree>, HttpError> {
        if self.is_closed() {
            return Err(HttpError::new(503, "shutting down"));
        }
        let folder = self.folder_row(folder_id)?;
        let spec = self.spec(&folder)?;
        let link = parse_mega_url(&spec.url).map_err(hx)?;
        let prev = Self::stored_status(&folder);
        self.set_status(folder_id, |s| s.state = RemoteFolderState::Listing);
        let root = match self.deps.mega.load(&link).await {
            Ok(r) => r,
            Err(e) => {
                let err = mega_error(&e);
                let n = {
                    let mut st = self.st.lock().unwrap();
                    let n = st.failures.entry(folder_id).or_insert(0);
                    *n += 1;
                    *n
                };
                let code = error_code(&err);
                let transient = matches!(
                    code,
                    RemoteErrorCode::Network
                        | RemoteErrorCode::Unavailable
                        | RemoteErrorCode::Other
                        | RemoteErrorCode::Quota
                );
                let msg = err.message.clone();
                self.set_status(folder_id, |s| {
                    s.state = match code {
                        RemoteErrorCode::Network => RemoteFolderState::Offline,
                        RemoteErrorCode::Quota => RemoteFolderState::Quota,
                        _ => RemoteFolderState::Error,
                    };
                    s.error = Some(msg);
                    s.code = Some(code);
                    s.retry_at = transient.then(|| {
                        now_ms()
                            + OFFLINE_RETRY_MS[(n as usize - 1).min(OFFLINE_RETRY_MS.len() - 1)]
                    });
                    s.listed_at = prev.listed_at;
                });
                return Err(err);
            }
        };
        if self.is_closed() {
            return Err(HttpError::new(503, "shutting down"));
        }
        self.st.lock().unwrap().failures.remove(&folder_id);
        let tree = Arc::new(build_tree(link, root));
        // the folder may have been removed while listing
        let still = match self.deps.store.remote_folder(folder_id) {
            Ok(Some(r)) => r,
            _ => return Err(HttpError::new(404, "Folder not found")),
        };
        self.st
            .lock()
            .unwrap()
            .trees
            .insert(folder_id, tree.clone());
        self.ingest(&still, &tree);
        self.set_status(folder_id, |s| {
            s.state = RemoteFolderState::Ok;
            s.error = None;
            s.code = None;
            s.retry_at = None;
            s.listed_at = Some(now_ms());
        });
        self.kick();
        Ok(tree)
    }

    /// the loaded tree of a folder (lists it once after a restart)
    pub async fn tree(&self, folder: &RemoteFolderRow) -> Result<Arc<Tree>, HttpError> {
        let failed = {
            let st = self.st.lock().unwrap();
            if let Some(t) = st.trees.get(&folder.id) {
                return Ok(t.clone());
            }
            st.failures.get(&folder.id).copied().unwrap_or(0) > 0
        };
        let s = Self::stored_status(folder);
        // offline / broken link: no request storm from every thumbnail until the next retry
        if matches!(
            s.state,
            RemoteFolderState::Offline | RemoteFolderState::Error
        ) && s.retry_at.is_none_or(|r| r > now_ms())
            && failed
        {
            return Err(HttpError::new(
                503,
                s.error.unwrap_or_else(|| "MEGA is not reachable".into()),
            ));
        }
        self.refresh(folder.id).await
    }

    /// tree → media rows (diff by dir/name/size/mtime), empty dirs → device_dirs
    fn ingest(&self, folder: &RemoteFolderRow, tree: &Tree) {
        let rows = &self.deps.rows;
        let scope = ListScope {
            folder_id: folder.id,
            dir: String::new(),
        };
        let max_before = rows.list_begin(&scope);
        let mut files = Vec::new();
        for (rel, node) in &tree.files {
            if rows.skip_file(folder.id, rel, node.size as i64) {
                continue;
            }
            let (dir, name) = match rel.rfind('/') {
                Some(i) => (&rel[..i], &rel[i + 1..]),
                None => ("", rel.as_str()),
            };
            let Some(kind) = pa_core::kind_of(name) else {
                continue;
            };
            let ext = pa_core::ext_of(name);
            let mtime = node.timestamp.filter(|t| *t > 0).unwrap_or(0);
            files.push(ListedFile {
                dir: dir.to_string(),
                name: name.to_string(),
                meta: ExtractedMeta {
                    playback: playback_of(kind, &ext),
                    ..Default::default()
                },
                ext,
                meta_state: if matches!(kind, MediaKind::Image | MediaKind::Video) {
                    META_REMOTE_PENDING
                } else {
                    1
                },
                kind,
                size: node.size as i64,
                mtime,
            });
        }
        let changed = rows.list_upsert(folder.id, &files);
        let dirs: Vec<String> = tree
            .dirs
            .iter()
            .map(|d| d.0.clone())
            .filter(|d| !d.is_empty() && !rows.skip_dir(folder.id, d))
            .collect();
        let (added, removed) = rows.list_finish(&scope, &dirs, max_before);
        let fid = folder.id.to_string();
        let ev = &self.deps.events;
        if added > 0 {
            ev.media_changed(&fid, "*", MediaReason::Add);
        } else if removed > 0 {
            ev.media_changed(&fid, "*", MediaReason::Remove);
        } else if changed > 0 {
            ev.media_changed(&fid, "*", MediaReason::Change);
        }
        if added > 0 || removed > 0 || changed > 0 {
            ev.groups_changed();
        }
        ev.emit(ServerEvent::Folders { status: None });
    }

    /// the folder row was deleted (`DELETE /api/folders/:id`)
    pub fn forget(&self, folder_id: i64) {
        let mut st = self.st.lock().unwrap();
        st.trees.remove(&folder_id);
        st.failures.remove(&folder_id);
    }

    /// startup: the scanner's scanAll() re-lists every remote folder (`scanner.remoteScan`)
    pub fn start(&self) {
        self.kick();
    }

    /// the MEGA account session changed (login / logout / expired): transfers now count against
    /// another quota → lift the quota pause and re-list folders that stopped on it.
    pub fn resume_transfers(&self) {
        if self.is_closed() {
            return;
        }
        self.set_quota_until(0);
        self.gate.resume();
        self.deps.store.set_setting("remoteQuota", json!({}));
        for f in self.deps.store.remote_folders() {
            let listing = self.st.lock().unwrap().listing.contains_key(&f.id);
            if !listing && Self::stored_status(&f).state == RemoteFolderState::Quota {
                drop(self.refresh(f.id));
            }
        }
        self.deps
            .events
            .emit(ServerEvent::Folders { status: Some(true) });
        self.kick();
    }

    /// `remote/account.ts` `changed()`: new download URLs, quota pause lifted, `megaAccount` event
    pub fn account_changed(&self) {
        self.deps.mega.clear_download_urls();
        self.resume_transfers();
        self.deps.events.emit(ServerEvent::MegaAccount);
    }

    /// the 60 s timer: quota reset, retries due, stale listings (public for tests)
    pub fn tick(&self) {
        if self.is_closed() {
            return;
        }
        let now = now_ms();
        let mark = self.quota_mark.load(Ordering::SeqCst);
        if mark > 0 && mark as i64 <= now {
            self.set_quota_until(0);
            self.deps.store.set_setting("remoteQuota", json!({}));
            self.deps
                .events
                .emit(ServerEvent::Folders { status: Some(true) });
            self.kick();
        }
        let refresh_ms = self.refresh_ms.load(Ordering::Relaxed) as i64;
        for f in self.deps.store.remote_folders() {
            if self.st.lock().unwrap().listing.contains_key(&f.id) {
                continue;
            }
            let s = Self::stored_status(&f);
            let due = s.retry_at.is_some_and(|r| r <= now) && s.state != RemoteFolderState::Ok;
            let stale =
                s.state == RemoteFolderState::Ok && s.listed_at.unwrap_or(0) + refresh_ms <= now;
            if due || stale {
                drop(self.refresh(f.id));
            }
        }
    }

    // ───────────────────────── nodes / transfers ─────────────────────────

    /// (running, waiting) transfers of the folder's link (`laneStats`)
    pub fn lane_stats(&self, folder: &RemoteFolderRow) -> Result<(usize, usize), HttpError> {
        Ok(self.gate.lanes(&self.spec(folder)?.link_id).stats())
    }

    /// the node of a row (404 'File is no longer in the MEGA link')
    pub async fn node_of(
        &self,
        folder: &RemoteFolderRow,
        dir: &str,
        name: &str,
    ) -> Result<(MegaLink, MegaNode), HttpError> {
        let t = self.tree(folder).await?;
        match t.file(&join_rel(dir, name)) {
            Some(n) => Ok((t.link.clone(), n.clone())),
            None => Err(HttpError::new(404, "File is no longer in the MEGA link")),
        }
    }

    fn check_quota(&self) -> Result<(), HttpError> {
        self.gate.check_quota().map_err(hx)
    }

    /// a failed transfer: quota → stop every transfer until it resets; congestion → short back-off
    pub fn note_error(&self, e: &MegaError) {
        if let Some(until) = self.gate.note_error(e) {
            self.quota_mark.store(until, Ordering::SeqCst);
            self.deps
                .store
                .set_setting("remoteQuota", json!({ "until": until }));
            self.deps
                .events
                .emit(ServerEvent::Folders { status: Some(true) });
        }
    }

    /// decrypted bytes [start, end]; the slot is held until the stream ends or is dropped
    async fn open_stream(
        &self,
        link: &MegaLink,
        node: &MegaNode,
        start: u64,
        end: u64,
        cancel: Option<&CancellationToken>,
        prio: Priority,
    ) -> Result<Guarded, MegaError> {
        let token = self.gate.token(cancel);
        let guard = match self.gate.slot(&link.id, prio, &token).await {
            Ok(g) => g,
            Err(e) => {
                token.cancel();
                return Err(e);
            }
        };
        if token.is_cancelled() {
            return Err(MegaError::Aborted);
        }
        match self.deps.mega.read(link, node, start, end, token.clone()) {
            Ok(inner) => Ok(Guarded {
                inner,
                guard: Some(guard),
                token,
                svc: self.me.clone(),
            }),
            Err(e) => {
                token.cancel();
                Err(e)
            }
        }
    }

    async fn read_buffer(
        &self,
        link: &MegaLink,
        node: &MegaNode,
        start: u64,
        end: u64,
        prio: Priority,
    ) -> Result<Vec<u8>, MegaError> {
        let work = async {
            let mut s = self.open_stream(link, node, start, end, None, prio).await?;
            let mut out = Vec::new();
            while let Some(c) = s.next().await {
                out.extend_from_slice(&c?);
            }
            Ok(out)
        };
        tokio::time::timeout(Duration::from_secs(120), work)
            .await
            .unwrap_or(Err(MegaError::Aborted))
    }

    /// one bounded transfer (attribute fetch) in a slot
    async fn attr_of(
        &self,
        link: &MegaLink,
        node: &MegaNode,
        kind: u32,
        prio: Priority,
    ) -> Result<Option<Vec<u8>>, MegaError> {
        let token = self.gate.token(None);
        let r = async {
            let _g = self.gate.slot(&link.id, prio, &token).await?;
            self.deps.mega.attr(link, node, kind, &token).await
        }
        .await;
        token.cancel();
        if let Err(e) = &r {
            self.note_error(e);
        }
        r
    }

    /// the whole original on local disk: from the LRU cache, else downloaded (temp = caller deletes it)
    pub async fn fetch_file(
        &self,
        folder: &RemoteFolderRow,
        r: &RemoteMedia,
        prio: Priority,
    ) -> Result<(PathBuf, bool), MegaError> {
        let abs = abs_of(&folder.path, &r.dir, &r.name);
        let key = RemoteCache::key(&abs, r.mtime, r.size);
        if let Some(hit) = self.cache.get(&key).await {
            return Ok((hit, false));
        }
        let cacheable = self.cache.accepts(r.size.max(0) as u64);
        if !cacheable {
            return self.download_file(folder, r, &key, prio, false).await;
        }
        let fut = {
            let mut st = self.st.lock().unwrap();
            if let Some(f) = st.fetching.get(&key) {
                f.clone()
            } else {
                let me = self.arc();
                let (folder, r, k2) = (folder.clone(), r.clone(), key.clone());
                let fut = async move {
                    let res = me.download_file(&folder, &r, &k2, prio, true).await;
                    me.st.lock().unwrap().fetching.remove(&k2);
                    res
                }
                .boxed()
                .shared();
                st.fetching.insert(key, fut.clone());
                let f2 = fut.clone();
                self.tracker.spawn(async move {
                    let _ = f2.await;
                });
                fut
            }
        };
        fut.await
    }

    async fn download_file(
        &self,
        folder: &RemoteFolderRow,
        r: &RemoteMedia,
        key: &str,
        prio: Priority,
        cacheable: bool,
    ) -> Result<(PathBuf, bool), MegaError> {
        let (link, node) = self
            .node_of(folder, &r.dir, &r.name)
            .await
            .map_err(to_mega)?;
        let tmp = self.cache.tmp_file(key);
        let res = async {
            let mut f = tokio::fs::File::create(&tmp).await?;
            if node.size > 0 {
                let mut s = self
                    .open_stream(&link, &node, 0, node.size - 1, None, prio)
                    .await?;
                while let Some(c) = s.next().await {
                    f.write_all(&c?).await?;
                }
            }
            f.flush().await?;
            drop(f);
            let len = tokio::fs::metadata(&tmp).await?.len();
            if len != node.size {
                return Err(to_mega(HttpError::new(
                    502,
                    "Incomplete download from MEGA",
                )));
            }
            if cacheable {
                return Ok((self.cache.commit(key, &tmp).await?, false));
            }
            Ok((tmp.clone(), true))
        }
        .await;
        if res.is_err() {
            let _ = tokio::fs::remove_file(&tmp).await;
        }
        res
    }

    // ───────────────────────── RemoteImageSource (ThumbService) ─────────────────────────

    async fn resolve(
        &self,
        abs: &str,
    ) -> Result<(RemoteFolderRow, RemoteMedia, MegaLink, MegaNode), HttpError> {
        let rest = abs
            .strip_prefix("remote://mega/")
            .ok_or_else(|| HttpError::new(400, "Not a remote path"))?;
        let (lid, rel) = rest
            .split_once('/')
            .ok_or_else(|| HttpError::new(400, "Not a remote path"))?;
        if lid.is_empty()
            || rel.is_empty()
            || !lid
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(HttpError::new(400, "Not a remote path"));
        }
        let base = format!("{REMOTE_SCHEME}mega/{lid}");
        let rel = normalize_rel(rel)?;
        let (dir, name) = match rel.rfind('/') {
            Some(i) => (rel[..i].to_string(), rel[i + 1..].to_string()),
            None => (String::new(), rel.clone()),
        };
        let row = self
            .deps
            .store
            .media_at(&base, &dir, &name)
            .ok_or_else(|| HttpError::new(404, "Media not found"))?;
        let folder = self
            .deps
            .store
            .remote_folder(row.folder_id)?
            .ok_or_else(|| HttpError::new(400, "Not a remote folder"))?;
        let (link, node) = self.node_of(&folder, &row.dir, &row.name).await?;
        Ok((folder, row, link, node))
    }

    /// `thumb(abs, size, prio)`: MEGA preview → 120 px square → the original (images ≤ 30 MB,
    /// JPEG ≤ 1600 q88) → a video frame (PNG) via ffmpeg over loopback; 404 'No thumbnail'.
    pub async fn thumb(&self, abs: &str, _size: u32, prio: Priority) -> Result<Vec<u8>, HttpError> {
        let (folder, row, link, node) = self.resolve(abs).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        let types: HashSet<u32> = pa_mega::crypto::parse_fa(node.fa.as_deref())
            .iter()
            .map(|t| t.kind)
            .collect();
        let attr = |kind: u32| {
            let (link, node) = (&link, &node);
            let has = types.contains(&kind);
            async move {
                if !has {
                    return Ok(None);
                }
                match tokio::time::timeout_at(deadline, self.attr_of(link, node, kind, prio)).await
                {
                    Ok(Ok(Some(b))) if !b.is_empty() => Ok(Some(b)),
                    Ok(Err(e)) if mega_error(&e).status == 429 => Err(mega_error(&e)),
                    _ => Ok(None),
                }
            }
        };
        // 1. MEGA preview (~1000 px, aspect kept)
        if let Some(p) = attr(1).await? {
            self.st.lock().unwrap().squares.remove(abs);
            return Ok(p);
        }
        // 2. MEGA thumbnail (120 px square)
        if let Some(sq) = attr(0).await? {
            self.st.lock().unwrap().squares.insert(abs.to_string());
            return Ok(sq);
        }
        // 3. the original (small images only)
        if row.kind == MediaKind::Image && row.size <= FULL_THUMB_MAX {
            let (file, temp) = self
                .fetch_file(&folder, &row, prio)
                .await
                .map_err(|e| mega_error(&e))?;
            let ext = row.ext.clone();
            let f2 = file.clone();
            let out = tokio::task::spawn_blocking(move || {
                let img = pa_media::decode_path(
                    &f2,
                    pa_media::DecodeOptions::default().ext(&ext).fit(1600, 1600),
                )
                .ok()?;
                pa_media::render::render_jpeg_1600(img).ok()
            })
            .await
            .ok()
            .flatten();
            if temp {
                let _ = tokio::fs::remove_file(&file).await;
            }
            let out = out.ok_or_else(|| HttpError::new(415, "Cannot decode"))?;
            self.st.lock().unwrap().squares.remove(abs);
            return Ok(out);
        }
        // 4. a video frame, read with seeks through the internal loopback URL
        if row.kind == MediaKind::Video && self.deps.ffmpeg.available() && self.loopback().is_some()
        {
            self.check_quota()?;
            let at = row
                .duration
                .filter(|d| *d != 0.0)
                .map(|d| (d * 0.1).min(3.0))
                .unwrap_or(1.0);
            let mut args = vec!["-ss".to_string(), format!("{at:.3}")];
            args.extend(self.ffmpeg_input(row.id)?);
            args.extend(
                [
                    "-frames:v",
                    "1",
                    "-vf",
                    "scale='min(1600,iw)':-2",
                    "-f",
                    "image2pipe",
                    "-vcodec",
                    "png",
                    "pipe:1",
                ]
                .map(String::from),
            );
            let frame = self
                .deps
                .ffmpeg
                .run(
                    &args,
                    pa_media::ffmpeg::RunOptions {
                        timeout: Some(Duration::from_secs(90)),
                        input: None,
                    },
                )
                .await
                .unwrap_or_default();
            if !frame.is_empty() {
                self.st.lock().unwrap().squares.remove(abs);
                return Ok(frame);
            }
        }
        Err(HttpError::new(404, "No thumbnail"))
    }

    /// `display(abs, max)`: the original, ≤ max px, flattened on white, JPEG q88 (mozjpeg)
    pub async fn display(&self, abs: &str, max: u32) -> Result<Vec<u8>, HttpError> {
        let (folder, row, _, _) = self.resolve(abs).await?;
        if row.kind != MediaKind::Image {
            return Err(HttpError::new(400, "Not an image"));
        }
        let (file, temp) = self
            .fetch_file(&folder, &row, Priority::High)
            .await
            .map_err(|e| mega_error(&e))?;
        let ext = row.ext.clone();
        let f2 = file.clone();
        let out = tokio::task::spawn_blocking(move || {
            let img = pa_media::decode_path(
                &f2,
                pa_media::DecodeOptions::default().ext(&ext).fit(max, max),
            )
            .ok()?;
            let img = pa_media::resize::into_inside(img, max, max, true).ok()?;
            let flat =
                image::DynamicImage::ImageRgb8(pa_media::encode::flatten(&img, [255, 255, 255]));
            pa_media::encode::jpeg(&flat, pa_media::encode::JpegOptions::new(88)).ok()
        })
        .await
        .ok()
        .flatten();
        if temp {
            let _ = tokio::fs::remove_file(&file).await;
        }
        out.ok_or_else(|| HttpError::new(415, "Cannot convert"))
    }

    /// the video editor (only user of filmstrips) cannot save into a remote folder
    pub async fn strip(&self) -> Result<Vec<u8>, HttpError> {
        Err(HttpError::new(404, "No filmstrip for MEGA videos"))
    }

    /// true when the thumbnail of `abs` came from the 120px square (dimensions must not be derived from it)
    pub fn is_square_thumb(&self, abs: &str) -> bool {
        self.st.lock().unwrap().squares.contains(abs)
    }

    /// ffmpeg input args reading a remote file through our own ranged /file endpoint (seekable)
    pub fn ffmpeg_input(&self, media_id: i64) -> Result<Vec<String>, HttpError> {
        let (url, token) = self
            .loopback()
            .ok_or_else(|| HttpError::new(501, "Transcoding MEGA videos is not available"))?;
        Ok(vec![
            "-headers".into(),
            format!("X-Peek-Internal: {token}\r\n"),
            "-i".into(),
            format!("{url}/api/media/{media_id}/file"),
        ])
    }

    // ───────────────────────── HTTP ─────────────────────────

    /// `GET/HEAD /api/media/:id/file` for remote media: Range → ranged MEGA read (or the LRU cache).
    /// `range` is the raw `Range` header. Errors (quota, offline, gone) come before any byte.
    pub async fn serve_file(
        &self,
        r: &RemoteMedia,
        folder: &RemoteFolderRow,
        head: bool,
        range: Option<&str>,
        download: bool,
    ) -> Result<FileReply, HttpError> {
        let total = r.size.max(0) as u64;
        let mut headers: Vec<(String, String)> = Vec::new();
        let common = |h: &mut Vec<(String, String)>| {
            h.push(("Accept-Ranges".into(), "bytes".into()));
            h.push(("Cache-Control".into(), "private, max-age=3600".into()));
            h.push(("Content-Type".into(), mime_of(&r.ext).into()));
            if r.mtime > 0 {
                h.push(("Last-Modified".into(), http_date(r.mtime)));
            }
            if download {
                h.push(("Content-Disposition".into(), content_disposition(&r.name)));
            }
        };
        if head {
            common(&mut headers);
            headers.push(("Content-Length".into(), total.to_string()));
            return Ok(FileReply {
                status: 200,
                headers,
                body: None,
            });
        }
        let one = single_range(range);
        let range = match parse_range(one.as_deref(), total) {
            Some(Err(())) => {
                return Ok(FileReply {
                    status: 416,
                    headers: vec![("Content-Range".into(), format!("bytes */{total}"))],
                    body: None,
                })
            }
            Some(Ok(r)) => Some(r),
            None => None,
        };
        let (start, end) = range.unwrap_or((0, total.saturating_sub(1)));
        let mut body = empty_body();
        if total > 0 {
            let abs = abs_of(&folder.path, &r.dir, &r.name);
            let mut cached = self
                .cache
                .get(&RemoteCache::key(&abs, r.mtime, r.size))
                .await;
            // whole images: fetch once into the cache, later views (zoom, info, re-open) are local
            if cached.is_none()
                && range.is_none()
                && r.kind == MediaKind::Image
                && self.cache.accepts(total)
            {
                cached = Some(
                    self.fetch_file(folder, r, Priority::High)
                        .await
                        .map_err(|e| mega_error(&e))?
                        .0,
                );
            }
            if let Some(path) = cached {
                body = file_range(&path, start, end)
                    .await
                    .map_err(|e| HttpError::new(500, e.to_string()))?;
            } else {
                let res = async {
                    let (link, node) = self
                        .node_of(folder, &r.dir, &r.name)
                        .await
                        .map_err(to_mega)?;
                    let s = self
                        .open_stream(&link, &node, start, end, None, Priority::High)
                        .await?;
                    first_bytes(Box::pin(s)).await
                }
                .await;
                match res {
                    Ok(b) => body = b,
                    Err(e) => {
                        self.note_error(&e);
                        return Err(mega_error(&e));
                    }
                }
            }
        }
        common(&mut headers);
        let mut status = 200;
        if range.is_some() {
            status = 206;
            headers.push((
                "Content-Range".into(),
                format!("bytes {start}-{end}/{total}"),
            ));
        }
        headers.push((
            "Content-Length".into(),
            if total > 0 {
                (end - start + 1).to_string()
            } else {
                "0".into()
            },
        ));
        Ok(FileReply {
            status,
            headers,
            body: Some(body),
        })
    }

    /// zip entry (`/api/download`): opened when the zip writer first polls it
    pub fn zip_entry(&self, folder: &RemoteFolderRow, r: &RemoteMedia) -> BodyStream {
        let me = self.arc();
        let (folder, r) = (folder.clone(), r.clone());
        let open = async move {
            if r.size <= 0 {
                return Ok(empty_body());
            }
            let (link, node) = me
                .node_of(&folder, &r.dir, &r.name)
                .await
                .map_err(|e| io_err(to_mega(e)))?;
            let s = me
                .open_stream(
                    &link,
                    &node,
                    0,
                    node.size.saturating_sub(1),
                    None,
                    Priority::High,
                )
                .await
                .map_err(io_err)?;
            Ok::<BodyStream, std::io::Error>(Box::pin(s.map_err(io_err)))
        };
        Box::pin(futures_util::stream::once(open).try_flatten())
    }

    // ───────────────────────── copy to this computer ─────────────────────────

    /// `copy(spec)`: remote media / dirs into a local folder (the MEGA import job, nothing
    /// re-listed). The route checked `to` (a local, writable folder) before.
    pub async fn copy(
        &self,
        refs_in: &[FileRef],
        ids: &[String],
        to_folder: &str,
        to_dir: &str,
    ) -> Result<Job, HttpError> {
        let mut refs: Vec<FileRef> = refs_in.to_vec();
        for id in ids {
            let r = id_of(id)
                .and_then(|i| self.deps.store.media(i))
                .ok_or_else(|| HttpError::new(404, "Media not found"))?;
            refs.push(FileRef {
                folder_id: r.folder_id.to_string(),
                rel: join_rel(&r.dir, &r.name),
            });
        }
        if refs.is_empty() {
            return Err(HttpError::new(400, "Nothing selected"));
        }
        let folder_ids: HashSet<&str> = refs.iter().map(|r| r.folder_id.as_str()).collect();
        if folder_ids.len() != 1 {
            return Err(HttpError::new(400, "Copy from one MEGA link at a time"));
        }
        let fid = id_of(refs[0].folder_id.as_str())
            .ok_or_else(|| HttpError::new(404, "Folder not found"))?;
        let folder = self.folder_row(fid)?;
        let spec = self.spec(&folder)?;
        let tree = self.tree(&folder).await?;
        let mut nodes = Vec::new();
        for r in &refs {
            let rel = normalize_rel(&r.rel)?;
            let id = tree
                .file(&rel)
                .map(|n| n.id.clone())
                .or_else(|| tree.dir_id(&rel).map(str::to_string))
                .ok_or_else(|| HttpError::new(404, format!("Not in the MEGA link: {rel}")))?;
            nodes.push(id);
        }
        self.check_quota()?;
        let me = self.me.clone();
        self.importer.start(
            &json!({ "url": spec.url, "mode": "extract", "folderId": to_folder, "dir": to_dir, "nodes": nodes, "zip": "keep" }),
            ImportOptions {
                root: Some(tree.root.clone()),
                relative: true,
                title: Some(folder.name.clone()),
                on_error: Some(Arc::new(move |e: &MegaError| {
                    if let Some(s) = me.upgrade() {
                        s.note_error(e);
                    }
                })),
            },
        )
    }

    /// `POST /api/remote/import` (`startRemoteImport`)
    pub fn start_import(&self, spec: &Value) -> Result<Job, HttpError> {
        self.importer.start(spec, ImportOptions::default())
    }

    /// `POST /api/remote/inspect {url}`
    pub async fn inspect(&self, body: &Value) -> Result<pa_mega::RemoteInspect, HttpError> {
        let link = parse_url_value(body.get("url"))?;
        let root = self
            .deps
            .mega
            .load(&link)
            .await
            .map_err(|e| mega_error(&e))?;
        Ok(pa_mega::inspect_tree(&root))
    }

    // ───────────────────────── background: metadata + thumbnails ─────────────────────────

    /// start the background loops (metadata, thumbnails) when they are not running
    pub fn kick(&self) {
        if self.is_closed() {
            return;
        }
        let (meta, thumb) = {
            let mut st = self.st.lock().unwrap();
            let m = !st.meta_running;
            let t = !st.thumb_running;
            st.meta_running = true;
            st.thumb_running = true;
            (m, t)
        };
        if meta {
            let me = self.arc();
            self.tracker.spawn(async move {
                me.meta_loop().await;
                me.st.lock().unwrap().meta_running = false;
            });
        }
        if thumb {
            let me = self.arc();
            self.tracker.spawn(async move {
                me.thumb_loop().await;
                me.st.lock().unwrap().thumb_running = false;
            });
        }
    }

    fn blocked(&self) -> bool {
        self.is_closed() || self.gate.quota_until() > 0
    }

    async fn meta_loop(&self) {
        let mut skip: HashSet<i64> = HashSet::new();
        loop {
            if self.blocked() {
                break;
            }
            let rows: Vec<RemoteMedia> = self
                .deps
                .store
                .pending_meta()
                .into_iter()
                .filter(|r| !skip.contains(&r.id))
                .collect();
            if rows.is_empty() {
                break;
            }
            let mut touched: Vec<(i64, Vec<String>)> = Vec::new();
            let mut stop = false;
            for r in &rows {
                if self.blocked() {
                    stop = true;
                    break;
                }
                match self.enrich_one(r).await {
                    Ok(()) => {}
                    Err(e) if matches!(e.status, 429 | 502 | 503) => {
                        stop = true;
                        skip.insert(r.id);
                        break;
                    }
                    // keep the listing data; never retried until the file changes
                    Err(_) => self.deps.store.meta_failed(r.id),
                }
                match touched.iter_mut().find(|t| t.0 == r.folder_id) {
                    Some(t) if !t.1.contains(&r.dir) => t.1.push(r.dir.clone()),
                    Some(_) => {}
                    None => touched.push((r.folder_id, vec![r.dir.clone()])),
                }
                let gap = self.meta_gap_ms.load(Ordering::Relaxed);
                if gap > 0 {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(gap)) => {}
                        _ = self.gate.shutdown.cancelled() => {}
                    }
                }
            }
            for (fid, dirs) in &touched {
                for d in dirs {
                    self.deps
                        .events
                        .media_changed(&fid.to_string(), d, MediaReason::Meta);
                }
            }
            if !touched.is_empty() {
                self.deps.events.groups_changed();
                self.progress_changed();
            }
            if stop {
                break;
            }
        }
    }

    /// read what the metadata needs from MEGA (head of images, probe of videos) and update the row
    async fn enrich_one(&self, r: &RemoteMedia) -> Result<(), HttpError> {
        let folder = self.folder_row(r.folder_id)?;
        let mut m = ExtractedMeta {
            playback: r.playback,
            ..Default::default()
        };
        if r.kind == MediaKind::Image && r.size > 0 {
            let (link, node) = self.node_of(&folder, &r.dir, &r.name).await?;
            let end = (r.size as u64).min(HEAD_BYTES) - 1;
            let head = self
                .read_buffer(&link, &node, 0, end, Priority::Low)
                .await
                .map_err(|e| mega_error(&e))?;
            let ext = r.ext.clone();
            let im = tokio::task::spawn_blocking(move || pa_meta::image_meta(&head, &ext, false))
                .await
                .map_err(|e| HttpError::new(500, e.to_string()))?;
            m = ExtractedMeta {
                playback: r.playback,
                ..im
            };
        } else if r.kind == MediaKind::Video && self.deps.ffmpeg.available() {
            if let Some((url, token)) = self.loopback() {
                self.check_quota()?;
                let p = self
                    .deps
                    .ffmpeg
                    .probe(
                        &format!("{url}/api/media/{}/file", r.id),
                        &pa_media::ProbeOptions {
                            headers: Some(format!("X-Peek-Internal: {token}\r\n")),
                            timeout: None,
                        },
                    )
                    .await;
                if let Some(p) = p {
                    m = pa_meta::av_meta(MediaKind::Video, &r.ext, Some(&convert_probe(p)));
                } else if self.gate.quota_until() > 0 {
                    return Err(HttpError::new(429, "quota"));
                }
            }
        }
        if self.is_closed() {
            return Ok(());
        }
        self.deps.store.apply_meta(r, &m);
        Ok(())
    }

    async fn thumb_loop(&self) {
        let failed: Mutex<HashSet<i64>> = Mutex::new(HashSet::new());
        loop {
            if self.blocked() {
                break;
            }
            let rows: Vec<_> = {
                let f = failed.lock().unwrap();
                self.deps
                    .thumbs
                    .pending_thumbs(ThumbScope::Remote)
                    .into_iter()
                    .filter(|r| !f.contains(&r.id))
                    .collect()
            };
            if rows.is_empty() {
                break;
            }
            let stop = AtomicBool::new(false);
            futures_util::stream::iter(rows.iter())
                .for_each_concurrent(2, |r| {
                    let (stop, failed) = (&stop, &failed);
                    async move {
                        if stop.load(Ordering::Relaxed) || self.blocked() {
                            return;
                        }
                        let abs = r.abs();
                        let fid = r.folder_id.to_string();
                        match self.deps.thumbs.generate_base(&abs, r.id).await {
                            Ok(res) => {
                                if self.is_closed() {
                                    return;
                                }
                                let dims = res.is_some()
                                    && r.kind == MediaKind::Image
                                    && (r.width.is_none_or(|w| w == 0.0)
                                        || r.height.is_none_or(|h| h == 0.0))
                                    && !self.is_square_thumb(&abs);
                                self.deps.thumbs.set_thumb(
                                    r.id,
                                    res.as_ref().and_then(|x| x.thumbhash.as_deref()),
                                    if res.is_some() { 1 } else { 2 },
                                    res.as_ref().filter(|_| dims).map(|x| x.width),
                                    res.as_ref().filter(|_| dims).map(|x| x.height),
                                );
                                self.deps.events.media_changed(
                                    &fid,
                                    &r.dir,
                                    if dims {
                                        MediaReason::Meta
                                    } else {
                                        MediaReason::Thumb
                                    },
                                );
                            }
                            Err(e) => {
                                if self.is_closed() {
                                    return;
                                }
                                if e.status == 429
                                    || e.status == 503
                                    || (e.status == 502
                                        && e.message.to_lowercase().contains("reach"))
                                {
                                    stop.store(true, Ordering::Relaxed);
                                    return;
                                }
                                failed.lock().unwrap().insert(r.id);
                                self.deps.thumbs.set_thumb(r.id, None, 2, None, None);
                                self.deps
                                    .events
                                    .media_changed(&fid, &r.dir, MediaReason::Thumb);
                            }
                        }
                    }
                })
                .await;
            self.progress_changed();
            if stop.load(Ordering::Relaxed) {
                break;
            }
        }
    }

    /// indexing progress (FolderRef.remote.indexing) changed: one 'folders' event per 2 s at most
    fn progress_changed(&self) {
        {
            let mut st = self.st.lock().unwrap();
            if st.progress_pending || self.is_closed() {
                return;
            }
            st.progress_pending = true;
        }
        let me = self.arc();
        self.tracker.spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                _ = me.gate.shutdown.cancelled() => {}
            }
            me.st.lock().unwrap().progress_pending = false;
            if !me.is_closed() {
                me.deps
                    .events
                    .emit(ServerEvent::Folders { status: Some(true) });
            }
        });
    }

    /// shutdown: in-flight MEGA transfers stop instead of holding it up; waits for the loops
    pub async fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.gate.shutdown.cancel();
        self.tracker.close();
        self.tracker.wait().await;
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

/// Wait for the first chunk (or the end) of a stream — fails with its error, so a failing MEGA
/// read still becomes a status code instead of a cut-off 200/206.
async fn first_bytes(mut s: Pin<Box<Guarded>>) -> Result<BodyStream, MegaError> {
    match s.next().await {
        None => Ok(empty_body()),
        Some(Err(e)) => Err(e),
        Some(Ok(c)) => Ok(Box::pin(
            futures_util::stream::once(async move { Ok(c) }).chain(s.map_err(io_err)),
        )),
    }
}

/// bytes [start, end] of a local file
async fn file_range(path: &Path, start: u64, end: u64) -> std::io::Result<BodyStream> {
    let mut f = tokio::fs::File::open(path).await?;
    f.seek(std::io::SeekFrom::Start(start)).await?;
    let r = f.take(end + 1 - start);
    Ok(Box::pin(tokio_util::io::ReaderStream::new(r)))
}

/// `GET /api/remote/folders/:id/link` may answer: the desktop window (not the LAN listener), or in
/// server mode a browser on the machine itself (loopback, not through a reverse proxy).
pub fn link_is_local(desktop: bool, lan: bool, loopback_ip: bool, forwarded_headers: bool) -> bool {
    if desktop {
        !lan
    } else {
        loopback_ip && !forwarded_headers
    }
}

/// `REMOTE_READONLY` (403 of every write into a MEGA folder; checked by the server's routes)
pub const REMOTE_READONLY: &str =
    "Files in a MEGA link can only be viewed and copied to this computer";

/// `details(r, folder).path`: `[folder.name, rel].filter(Boolean).join('/')` (the server builds the
/// rest of `MediaDetails` from the stored row: `exif: null` — no EXIF read on demand —, gps, place,
/// camera, tzOffset, and `video` from [`details_video`])
pub fn details_path(folder_name: &str, dir: &str, name: &str) -> String {
    let rel = join_rel(dir, name);
    [folder_name, rel.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("/")
}

/// `details(r, folder).video`: from the `extra` column of a video row (None for images / no extra)
pub fn details_video(kind: MediaKind, extra: Option<&str>) -> Option<pa_core::VideoInfo> {
    if kind != MediaKind::Video {
        return None;
    }
    let e: Value = serde_json::from_str(extra?).ok()?;
    if e.is_null() {
        return None;
    }
    let s = |k: &str| e.get(k).and_then(Value::as_str).map(str::to_string);
    let n = |k: &str| e.get(k).and_then(Value::as_f64);
    Some(pa_core::VideoInfo {
        codec: s("codec"),
        fps: n("fps"),
        bitrate: n("bitrate"),
        audio_codec: s("audioCodec"),
        rotation: e
            .get("rotation")
            .and_then(Value::as_f64)
            .map(|r| r as i32)
            .unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_helpers_and_codes() {
        assert_eq!(details_path("Tatil", "Sub", "a.jpg"), "Tatil/Sub/a.jpg");
        assert_eq!(details_path("", "", "a.jpg"), "a.jpg");
        let v = details_video(MediaKind::Video, Some(r#"{"codec":"h264","audioCodec":"aac","fps":25,"bitrate":1000,"rotation":90,"hasAudio":true}"#)).unwrap();
        assert_eq!(
            (v.codec.as_deref(), v.rotation, v.fps),
            (Some("h264"), 90, Some(25.0))
        );
        assert!(details_video(MediaKind::Image, Some("{}")).is_none());
        assert!(details_video(MediaKind::Video, None).is_none());
        assert_eq!(
            error_code(&HttpError::new(404, "This MEGA link has been taken down")),
            RemoteErrorCode::Blocked
        );
        assert_eq!(
            error_code(&HttpError::new(502, "Cannot reach MEGA")),
            RemoteErrorCode::Network
        );
        assert_eq!(
            error_code(&HttpError::new(502, "MEGA error: x")),
            RemoteErrorCode::Other
        );
        assert_eq!(
            error_code(&HttpError::new(400, "Invalid MEGA link key")),
            RemoteErrorCode::Key
        );
        assert_eq!(
            error_code(&HttpError::new(503, "x")),
            RemoteErrorCode::Unavailable
        );
        assert_eq!(
            error_code(&HttpError::new(429, "x")),
            RemoteErrorCode::Quota
        );
        assert_eq!(
            abs_of("remote://mega/AbCd1234", "", "a.jpg"),
            "remote://mega/AbCd1234/a.jpg"
        );
        assert!(link_is_local(true, false, false, true));
        assert!(!link_is_local(true, true, true, false));
        assert!(!link_is_local(false, false, false, false));
        // stored status: defaults merged like `{ state: 'ok', …, ...stored }`, garbage → listing
        let row = |s: Option<&str>| RemoteFolderRow {
            id: 1,
            name: "x".into(),
            path: "remote://mega/x".into(),
            remote: "{}".into(),
            remote_status: s.map(str::to_string),
        };
        assert_eq!(
            RemoteService::stored_status(&row(Some(r#"{"listedAt":5}"#))).state,
            RemoteFolderState::Ok
        );
        assert_eq!(
            RemoteService::stored_status(&row(Some(r#"{"listedAt":5}"#))).listed_at,
            Some(5)
        );
        assert_eq!(
            RemoteService::stored_status(&row(Some("nope"))).state,
            RemoteFolderState::Listing
        );
        assert_eq!(
            RemoteService::stored_status(&row(None)).state,
            RemoteFolderState::Listing
        );
    }
}
