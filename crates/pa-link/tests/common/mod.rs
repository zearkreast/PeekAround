//! Test support: in-memory DeviceStore / MediaRowSink / ThumbCache / TransferHost, an axum server
//! with the device routes, and a Rust port of test/helpers/fake-phone.ts.
#![allow(dead_code)]

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, put};
use axum::Router;
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use pa_core::{FolderDevice, FolderRef, MediaKind, ServerEvent};
use pa_jobs::library::{
    BaseThumb, ListScope, ListedFile, MediaRowSink, ThumbCache, ThumbRow, ThumbScope,
};
use pa_jobs::{HttpError, JobService, MemoryEvents};
use pa_link::hub::{DeviceHub, HubDeps};
use pa_link::media::{serve_device_file, FileRow};
use pa_link::store::{DeviceFolderRow, DeviceRow, DeviceStore};
use pa_link::transfer::{DeviceMedia, HostFolder, TransferHost, Transfers};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, Notify};

// ───────────────────────── in-memory library ─────────────────────────

#[derive(Debug, Clone)]
pub struct Row {
    pub id: i64,
    pub folder_id: i64,
    pub dir: String,
    pub name: String,
    pub kind: MediaKind,
    pub size: i64,
    pub mtime: i64,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub duration: Option<f64>,
    pub taken: Option<i64>,
    pub playback: pa_core::Playback,
    pub thumb_state: i64,
    pub thumbhash: Option<String>,
    pub seen: bool,
}

#[derive(Default)]
pub struct Db {
    pub devices: HashMap<String, DeviceRow>,
    /// folder id → (row, path)
    pub folders: Vec<(DeviceFolderRow, String)>,
    pub next_folder: i64,
    pub rows: Vec<Row>,
    pub next_row: i64,
    pub dirs: HashSet<(i64, String)>,
}

#[derive(Clone, Default)]
pub struct Mem {
    pub db: Arc<Mutex<Db>>,
    pub hub: Arc<Mutex<Option<Arc<DeviceHub>>>>,
    /// local library folder (id 1000)
    pub lib: Arc<Mutex<PathBuf>>,
    pub upserts: Arc<Mutex<Vec<(i64, String)>>>,
}

fn in_scope(r: &Row, s: &ListScope) -> bool {
    r.folder_id == s.folder_id
        && (s.dir.is_empty() || r.dir == s.dir || r.dir.starts_with(&format!("{}/", s.dir)))
}

impl Mem {
    pub fn hub(&self) -> Arc<DeviceHub> {
        self.hub.lock().unwrap().clone().unwrap()
    }
    pub fn rows_of(&self, folder_id: i64) -> Vec<Row> {
        self.db
            .lock()
            .unwrap()
            .rows
            .iter()
            .filter(|r| r.folder_id == folder_id)
            .cloned()
            .collect()
    }
    pub fn row(&self, folder_id: i64, dir: &str, name: &str) -> Option<Row> {
        self.db
            .lock()
            .unwrap()
            .rows
            .iter()
            .find(|r| r.folder_id == folder_id && r.dir == dir && r.name == name)
            .cloned()
    }
    pub fn row_by_id(&self, id: i64) -> Option<Row> {
        self.db
            .lock()
            .unwrap()
            .rows
            .iter()
            .find(|r| r.id == id)
            .cloned()
    }
    pub fn dirs_of(&self, folder_id: i64) -> Vec<String> {
        let mut v: Vec<String> = self
            .db
            .lock()
            .unwrap()
            .dirs
            .iter()
            .filter(|d| d.0 == folder_id)
            .map(|d| d.1.clone())
            .collect();
        v.sort();
        v
    }
    pub fn folder_of_row(&self, id: i64) -> Option<(DeviceFolderRow, String)> {
        let db = self.db.lock().unwrap();
        let r = db.rows.iter().find(|r| r.id == id)?;
        db.folders.iter().find(|f| f.0.id == r.folder_id).cloned()
    }
}

impl DeviceStore for Mem {
    fn device(&self, id: &str) -> Option<DeviceRow> {
        self.db.lock().unwrap().devices.get(id).cloned()
    }
    fn paired_devices(&self) -> Vec<DeviceRow> {
        let mut v: Vec<DeviceRow> = self
            .db
            .lock()
            .unwrap()
            .devices
            .values()
            .filter(|d| d.paired)
            .cloned()
            .collect();
        v.sort_by_key(|d| std::cmp::Reverse(d.last_seen));
        v
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
        let mut db = self.db.lock().unwrap();
        let created = db.devices.get(id).map(|d| d.created_at).unwrap_or(now);
        db.devices.insert(
            id.into(),
            DeviceRow {
                id: id.into(),
                name: name.into(),
                model: model.into(),
                token_hash: Some(token_hash.into()),
                paired: true,
                caps: Some(caps.into()),
                created_at: created,
                last_seen: now,
            },
        );
    }
    fn update_online(&self, id: &str, name: &str, model: &str, caps: &str, now: i64) {
        if let Some(d) = self.db.lock().unwrap().devices.get_mut(id) {
            d.name = name.into();
            d.model = model.into();
            d.caps = Some(caps.into());
            d.last_seen = now;
        }
    }
    fn set_last_seen(&self, id: &str, now: i64) {
        if let Some(d) = self.db.lock().unwrap().devices.get_mut(id) {
            d.last_seen = now;
        }
    }
    fn delete_device(&self, id: &str) {
        let mut db = self.db.lock().unwrap();
        db.devices.remove(id);
        let gone: HashSet<i64> = db
            .folders
            .iter()
            .filter(|f| f.0.device_id == id)
            .map(|f| f.0.id)
            .collect();
        db.folders.retain(|f| f.0.device_id != id);
        db.rows.retain(|r| !gone.contains(&r.folder_id));
        db.dirs.retain(|d| !gone.contains(&d.0));
    }
    fn device_folders(&self, device_id: &str) -> Vec<DeviceFolderRow> {
        let mut v: Vec<DeviceFolderRow> = self
            .db
            .lock()
            .unwrap()
            .folders
            .iter()
            .filter(|f| f.0.device_id == device_id)
            .map(|f| f.0.clone())
            .collect();
        v.sort_by_key(|f| (f.sort, f.id));
        v
    }
    fn all_device_folders(&self) -> Vec<(i64, String)> {
        self.db
            .lock()
            .unwrap()
            .folders
            .iter()
            .map(|f| (f.0.id, f.0.device_id.clone()))
            .collect()
    }
    fn device_folder(&self, folder_id: i64) -> Option<DeviceFolderRow> {
        self.db
            .lock()
            .unwrap()
            .folders
            .iter()
            .find(|f| f.0.id == folder_id)
            .map(|f| f.0.clone())
    }
    fn sync_device_folders(
        &self,
        device_id: &str,
        update: &[(i64, String, i64)],
        insert: &[(String, String, i64)],
        delete: &[i64],
    ) -> Vec<i64> {
        let mut db = self.db.lock().unwrap();
        for (id, name, sort) in update {
            if let Some(f) = db.folders.iter_mut().find(|f| f.0.id == *id) {
                f.0.name = name.clone();
                f.0.sort = *sort;
            }
        }
        let mut ids = Vec::new();
        for (key, name, sort) in insert {
            db.next_folder += 1;
            let id = db.next_folder;
            db.folders.push((
                DeviceFolderRow {
                    id,
                    name: name.clone(),
                    sort: *sort,
                    device_id: device_id.into(),
                    device_key: key.clone(),
                },
                format!("device://{device_id}/{key}"),
            ));
            ids.push(id);
        }
        db.folders.retain(|f| !delete.contains(&f.0.id));
        db.rows.retain(|r| !delete.contains(&r.folder_id));
        ids
    }
    fn folder_refs(&self, device_id: &str) -> Vec<FolderRef> {
        let look = self
            .hub
            .lock()
            .unwrap()
            .clone()
            .map(|h| h.device_lookup(device_id));
        let db = self.db.lock().unwrap();
        let mut fs: Vec<&(DeviceFolderRow, String)> = db
            .folders
            .iter()
            .filter(|f| f.0.device_id == device_id)
            .collect();
        fs.sort_by_key(|f| (f.0.sort, f.0.id));
        fs.into_iter()
            .map(|(f, path)| FolderRef {
                id: f.id.to_string(),
                path: path.clone(),
                name: f.name.clone(),
                library_id: None,
                temp: false,
                expires_at: None,
                source_name: None,
                media_count: db.rows.iter().filter(|r| r.folder_id == f.id).count() as i64,
                device: look.clone().map(|l| FolderDevice {
                    id: l.id,
                    name: l.name,
                    online: l.online,
                }),
                remote: None,
            })
            .collect()
    }
}

impl MediaRowSink for Mem {
    fn skip_name(&self, name: &str) -> bool {
        name.starts_with('.') || name == ".peek-trash"
    }
    fn skip_file(&self, _f: i64, rel: &str, _size: i64) -> bool {
        rel.split('/').any(|s| MediaRowSink::skip_name(self, s))
    }
    fn skip_dir(&self, _f: i64, rel: &str) -> bool {
        !rel.is_empty() && rel.split('/').any(|s| MediaRowSink::skip_name(self, s))
    }
    fn list_begin(&self, scope: &ListScope) -> i64 {
        let mut db = self.db.lock().unwrap();
        for r in db.rows.iter_mut().filter(|r| in_scope(r, scope)) {
            r.seen = false;
        }
        db.next_row
    }
    fn list_upsert(&self, folder_id: i64, files: &[ListedFile]) -> usize {
        let mut db = self.db.lock().unwrap();
        let mut changed = 0;
        for f in files {
            if let Some(r) = db
                .rows
                .iter_mut()
                .find(|r| r.folder_id == folder_id && r.dir == f.dir && r.name == f.name)
            {
                if r.size == f.size && r.mtime == f.mtime {
                    r.seen = true;
                    continue;
                }
                r.size = f.size;
                r.mtime = f.mtime;
                r.kind = f.kind;
                r.width = f.meta.width;
                r.height = f.meta.height;
                r.duration = f.meta.duration;
                r.taken = f.meta.instant;
                r.playback = f.meta.playback;
                r.thumb_state = 0;
                r.thumbhash = None;
                r.seen = true;
            } else {
                db.next_row += 1;
                let id = db.next_row;
                db.rows.push(Row {
                    id,
                    folder_id,
                    dir: f.dir.clone(),
                    name: f.name.clone(),
                    kind: f.kind,
                    size: f.size,
                    mtime: f.mtime,
                    width: f.meta.width,
                    height: f.meta.height,
                    duration: f.meta.duration,
                    taken: f.meta.instant,
                    playback: f.meta.playback,
                    thumb_state: 0,
                    thumbhash: None,
                    seen: true,
                });
            }
            changed += 1;
        }
        changed
    }
    fn list_abort(&self, scope: &ListScope) {
        for r in self
            .db
            .lock()
            .unwrap()
            .rows
            .iter_mut()
            .filter(|r| in_scope(r, scope))
        {
            r.seen = true;
        }
    }
    fn list_finish(&self, scope: &ListScope, dirs: &[String], max_before: i64) -> (usize, usize) {
        let mut db = self.db.lock().unwrap();
        let before = db.rows.len();
        db.rows.retain(|r| !(in_scope(r, scope) && !r.seen));
        let removed = before - db.rows.len();
        let d = scope.dir.clone();
        db.dirs.retain(|(f, x)| {
            !(*f == scope.folder_id && (d.is_empty() || *x == d || x.starts_with(&format!("{d}/"))))
        });
        for x in dirs {
            db.dirs.insert((scope.folder_id, x.clone()));
        }
        let added = db
            .rows
            .iter()
            .filter(|r| r.folder_id == scope.folder_id && r.id > max_before)
            .count();
        (added, removed)
    }
}

#[async_trait]
impl ThumbCache for Mem {
    async fn generate_base(
        &self,
        abs: &str,
        _media_id: i64,
    ) -> Result<Option<BaseThumb>, HttpError> {
        // the server's ThumbService asks the phone for 640 px and derives the rest
        let data = self.hub().thumb(abs, 640, false).await?;
        Ok(Some(BaseThumb {
            thumbhash: Some(format!("th{}", data.len())),
            width: 64,
            height: 36,
        }))
    }
    fn set_thumb(
        &self,
        media_id: i64,
        thumbhash: Option<&str>,
        state: i64,
        width: Option<u32>,
        height: Option<u32>,
    ) {
        if let Some(r) = self
            .db
            .lock()
            .unwrap()
            .rows
            .iter_mut()
            .find(|r| r.id == media_id)
        {
            r.thumbhash = thumbhash.map(str::to_string);
            r.thumb_state = state;
            r.width = r.width.or(width.map(f64::from));
            r.height = r.height.or(height.map(f64::from));
        }
    }
    fn pending_thumbs(&self, scope: ThumbScope<'_>) -> Vec<ThumbRow> {
        let ThumbScope::Device(dev) = scope else {
            return Vec::new();
        };
        let db = self.db.lock().unwrap();
        db.rows
            .iter()
            .filter(|r| r.thumb_state == 0 && matches!(r.kind, MediaKind::Image | MediaKind::Video))
            .filter_map(|r| {
                let f = db
                    .folders
                    .iter()
                    .find(|f| f.0.id == r.folder_id && f.0.device_id == dev)?;
                Some(ThumbRow {
                    id: r.id,
                    folder_id: r.folder_id,
                    dir: r.dir.clone(),
                    name: r.name.clone(),
                    kind: r.kind,
                    width: r.width,
                    height: r.height,
                    folder_path: f.1.clone(),
                })
            })
            .take(16)
            .collect()
    }
}

#[async_trait]
impl TransferHost for Mem {
    fn folder(&self, folder_id: &str) -> Result<HostFolder, HttpError> {
        if folder_id == "1000" {
            return Ok(HostFolder {
                id: 1000,
                name: "Lib".into(),
                path: self.lib.lock().unwrap().clone(),
                device_id: None,
            });
        }
        let id: i64 = folder_id
            .parse()
            .map_err(|_| HttpError::new(404, "Folder not found"))?;
        let db = self.db.lock().unwrap();
        let f = db
            .folders
            .iter()
            .find(|f| f.0.id == id)
            .ok_or_else(|| HttpError::new(404, "Folder not found"))?;
        Ok(HostFolder {
            id,
            name: f.0.name.clone(),
            path: PathBuf::from(&f.1),
            device_id: Some(f.0.device_id.clone()),
        })
    }
    fn device_folder(&self, folder_id: &str) -> Result<DeviceFolderRow, HttpError> {
        let id: i64 = folder_id
            .parse()
            .map_err(|_| HttpError::new(404, "Folder not found"))?;
        self.db
            .lock()
            .unwrap()
            .folders
            .iter()
            .find(|f| f.0.id == id)
            .map(|f| f.0.clone())
            .ok_or_else(|| HttpError::bad("Not a device folder"))
    }
    fn find_media(&self, folder_id: i64, dir: &str, name: &str) -> Option<DeviceMedia> {
        self.row(folder_id, dir, name).map(|r| DeviceMedia {
            dir: r.dir,
            name: r.name,
            size: r.size,
            mtime: r.mtime,
        })
    }
    fn media_under(&self, folder_id: i64, rel: &str) -> Vec<DeviceMedia> {
        let mut v: Vec<Row> = self
            .rows_of(folder_id)
            .into_iter()
            .filter(|r| rel.is_empty() || r.dir == rel || r.dir.starts_with(&format!("{rel}/")))
            .collect();
        v.sort_by(|a, b| (&a.dir, &a.name).cmp(&(&b.dir, &b.name)));
        v.into_iter()
            .map(|r| DeviceMedia {
                dir: r.dir,
                name: r.name,
                size: r.size,
                mtime: r.mtime,
            })
            .collect()
    }
    fn skip_name(&self, name: &str) -> bool {
        name.starts_with('.')
    }
    async fn upsert_file(&self, folder_id: i64, rel: &str) {
        self.upserts.lock().unwrap().push((folder_id, rel.into()));
    }
}

// ───────────────────────── server ─────────────────────────

pub struct Server {
    pub base: String,
    pub mem: Mem,
    pub hub: Arc<DeviceHub>,
    pub events: Arc<MemoryEvents>,
    pub jobs: JobService,
    pub transfers: Transfers,
    pub task: tokio::task::JoinHandle<()>,
}

#[derive(Clone)]
struct St {
    mem: Mem,
    hub: Arc<DeviceHub>,
}

async fn connect(State(s): State<St>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    let meta = pa_link::axum::connect_meta(&headers, headers.contains_key("x-test-lan"));
    let hub = s.hub.clone();
    ws.on_upgrade(move |socket| pa_link::axum::serve_socket(hub, socket, meta))
}

async fn reply(
    State(s): State<St>,
    Path(rid): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    pa_link::axum::reply_put(&s.hub, &rid, &headers, body).await
}

async fn pull(State(s): State<St>, Path(token): Path<String>, headers: HeaderMap) -> Response {
    pa_link::axum::pull_get(&s.hub, &token, &headers).await
}

async fn file(
    State(s): State<St>,
    Path(id): Path<i64>,
    method: Method,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let Some(row) = s.mem.row_by_id(id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some((f, _)) = s.mem.folder_of_row(id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let fr = FileRow {
        dir: row.dir.clone(),
        name: row.name.clone(),
        ext: pa_core::ext_of(&row.name),
        size: row.size,
    };
    let range = headers.get("range").and_then(|v| v.to_str().ok());
    match serve_device_file(
        &s.hub,
        &f.device_id,
        &f.device_key,
        &fr,
        method == Method::HEAD,
        range,
        q.contains_key("download"),
    )
    .await
    {
        Ok(r) => pa_link::axum::file_response(r),
        Err(e) => pa_link::axum::error_response(&e),
    }
}

pub async fn start(lib: PathBuf) -> Server {
    let mem = Mem::default();
    *mem.lib.lock().unwrap() = lib;
    let events = Arc::new(MemoryEvents::default());
    let hub = DeviceHub::new(HubDeps {
        store: Arc::new(mem.clone()),
        rows: Arc::new(mem.clone()),
        thumbs: Arc::new(mem.clone()),
        events: events.clone(),
        server_name: "test-pc".into(),
    });
    *mem.hub.lock().unwrap() = Some(hub.clone());
    let st = St {
        mem: mem.clone(),
        hub: hub.clone(),
    };
    let app = Router::new()
        .route("/api/devices/connect", get(connect))
        .route("/api/devices/reply/{rid}", put(reply))
        .route("/api/devices/pull/{token}", get(pull))
        .route("/api/media/{id}/file", any(file))
        .with_state(st);
    // fixed socket buffers (no autotuning up to tcp_rmem max): the backpressure test measures our pipeline
    let sock = tokio::net::TcpSocket::new_v4().unwrap();
    sock.set_recv_buffer_size(256 * 1024).unwrap();
    sock.set_send_buffer_size(256 * 1024).unwrap();
    sock.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let listener = sock.listen(1024).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let jobs = JobService::new(events.clone(), std::env::temp_dir().join("pa-link-jobs"));
    let transfers = Transfers {
        hub: hub.clone(),
        jobs: jobs.clone(),
        host: Arc::new(mem.clone()),
    };
    Server {
        base,
        mem,
        hub,
        events,
        jobs,
        transfers,
        task,
    }
}

pub async fn until<T>(mut f: impl FnMut() -> Option<T>, ms: u64) -> T {
    let end = std::time::Instant::now() + Duration::from_millis(ms);
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(std::time::Instant::now() < end, "condition not met in time");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub fn pair_events(ev: &MemoryEvents) -> Vec<pa_core::DeviceInfo> {
    ev.snapshot()
        .into_iter()
        .filter_map(|e| {
            if let ServerEvent::Pair { device } = e {
                Some(device)
            } else {
                None
            }
        })
        .collect()
}

// ───────────────────────── fake phone ─────────────────────────

pub fn virtual_byte(i: u64) -> u8 {
    ((i * 31 + (i >> 8)) & 0xff) as u8
}

pub fn virtual_bytes(start: u64, end: u64) -> Vec<u8> {
    (start..end).map(virtual_byte).collect()
}

pub fn sha(b: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(b))
}

#[derive(Clone)]
pub struct PhoneFolder {
    pub key: String,
    pub name: String,
    pub root: PathBuf,
}

#[derive(Clone, Copy)]
pub struct VirtualFile {
    pub size: u64,
    pub mtime: i64,
}

pub type Intercept = Box<dyn FnMut(&Value) -> bool + Send>;

pub struct PhoneState {
    pub frames: Vec<Value>,
    pub token: Option<String>,
    pub close_code: Option<u16>,
    pub list_body: Option<String>,
    pub virtual_files: HashMap<String, VirtualFile>,
    pub ignore: HashSet<String>,
    pub intercept: Option<Intercept>,
    pub intercept_gen: u64,
    pub puts: HashMap<String, tokio::sync::watch::Receiver<Option<(u16, u64)>>>,
    pub last_pull_headers: HashMap<String, String>,
    pub written: u64,
    pub closed: bool,
    pub conn_gen: u64,
}

#[derive(Clone)]
pub struct FakePhone {
    pub base: String,
    pub id: String,
    pub folders: Vec<PhoneFolder>,
    pub name: String,
    pub caps: (bool, bool),
    pub st: Arc<Mutex<PhoneState>>,
    pub notify: Arc<Notify>,
    out: Arc<Mutex<Option<mpsc::UnboundedSender<tokio_tungstenite::tungstenite::Message>>>>,
    pub http: reqwest::Client,
}

impl FakePhone {
    pub fn new(
        base: &str,
        id: &str,
        folders: Vec<PhoneFolder>,
        token: Option<String>,
        name: &str,
    ) -> Self {
        FakePhone {
            base: base.into(),
            id: id.into(),
            folders,
            name: name.into(),
            caps: (true, true),
            st: Arc::new(Mutex::new(PhoneState {
                frames: Vec::new(),
                token,
                close_code: None,
                list_body: None,
                virtual_files: HashMap::new(),
                ignore: HashSet::new(),
                intercept: None,
                intercept_gen: 0,
                puts: HashMap::new(),
                last_pull_headers: HashMap::new(),
                written: 0,
                closed: false,
                conn_gen: 0,
            })),
            notify: Arc::new(Notify::new()),
            out: Arc::default(),
            http: reqwest::Client::new(),
        }
    }

    pub fn token(&self) -> Option<String> {
        self.st.lock().unwrap().token.clone()
    }

    pub fn hello_frame(&self) -> Value {
        json!({
            "t": "hello", "v": 1, "id": self.id, "name": self.name, "model": "Google Pixel 8", "app": "1.1.0",
            "folders": self.folders.iter().map(|f| json!({ "key": f.key, "name": f.name, "path": format!("/storage/emulated/0/{}", f.name) })).collect::<Vec<_>>(),
            "caps": { "read": self.caps.0, "receive": self.caps.1 },
        })
    }

    /// open the socket, send hello; resolves with the server's first answer (welcome / pending / rejected)
    pub async fn connect(&self, hello: Option<Value>, extra: &[(&str, &str)]) -> Value {
        let url = format!(
            "{}/api/devices/connect?v=1",
            self.base.replacen("http", "ws", 1)
        );
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = url.into_client_request().unwrap();
        req.headers_mut()
            .insert("X-Peek-Device", self.id.parse().unwrap());
        if let Some(t) = self.token() {
            req.headers_mut()
                .insert("Authorization", format!("Bearer {t}").parse().unwrap());
        }
        for (k, v) in extra {
            let name = axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap();
            req.headers_mut().insert(name, v.parse().unwrap());
        }
        let gen = {
            let mut s = self.st.lock().unwrap();
            s.close_code = None;
            s.closed = false;
            s.conn_gen += 1;
            s.conn_gen
        };
        let (ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
        let (mut tx, mut rx) = ws.split();
        let (otx, mut orx) = mpsc::unbounded_channel();
        *self.out.lock().unwrap() = Some(otx.clone());
        tokio::spawn(async move {
            while let Some(m) = orx.recv().await {
                if tx.send(m).await.is_err() {
                    break;
                }
            }
            let _ = tx.close().await;
        });
        let me = self.clone();
        tokio::spawn(async move {
            use tokio_tungstenite::tungstenite::Message;
            let mut code = 1006u16;
            while let Some(m) = rx.next().await {
                match m {
                    Ok(Message::Text(t)) => {
                        let f: Value = serde_json::from_str(t.as_str()).unwrap();
                        me.on_frame(f);
                    }
                    Ok(Message::Close(c)) => {
                        code = c.map(|c| u16::from(c.code)).unwrap_or(1005);
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            // only the current socket's close counts
            let mut s = me.st.lock().unwrap();
            if s.conn_gen == gen {
                s.close_code = Some(code);
                s.closed = true;
            }
            drop(s);
            me.notify.notify_waiters();
        });
        self.send(&hello.unwrap_or_else(|| self.hello_frame()));
        self.wait_frame(
            |f| matches!(f["t"].as_str(), Some("welcome" | "pending" | "rejected")),
            10_000,
        )
        .await
    }

    pub fn send(&self, f: &Value) {
        if let Some(o) = self.out.lock().unwrap().as_ref() {
            let _ = o.send(tokio_tungstenite::tungstenite::Message::Text(
                f.to_string().into(),
            ));
        }
    }

    pub fn close(&self) {
        // dropping the sender ends the writer task, which closes the socket
        *self.out.lock().unwrap() = None;
    }

    pub async fn closed(&self) -> u16 {
        loop {
            let n = self.notify.notified();
            if let Some(c) = self.st.lock().unwrap().close_code {
                return c;
            }
            tokio::time::timeout(Duration::from_secs(10), n)
                .await
                .expect("socket close");
        }
    }

    pub async fn wait_frame(&self, pred: impl Fn(&Value) -> bool, ms: u64) -> Value {
        let end = tokio::time::Instant::now() + Duration::from_millis(ms);
        loop {
            let n = self.notify.notified();
            {
                let mut s = self.st.lock().unwrap();
                if let Some(i) = s.frames.iter().position(&pred) {
                    return s.frames.remove(i);
                }
            }
            if tokio::time::timeout_at(end, n).await.is_err() {
                panic!(
                    "timeout waiting for frame; have {:?}",
                    self.st.lock().unwrap().frames
                );
            }
        }
    }

    pub fn set_intercept(&self, i: Option<Intercept>) {
        let mut s = self.st.lock().unwrap();
        s.intercept = i;
        s.intercept_gen += 1;
    }

    pub fn clear_frames(&self) {
        self.st.lock().unwrap().frames.clear();
    }

    fn on_frame(&self, f: Value) {
        if f["t"] == "paired" {
            self.st.lock().unwrap().token = f["token"].as_str().map(str::to_string);
        }
        self.st.lock().unwrap().frames.push(f.clone());
        self.notify.notify_waiters();
        if f["t"] == "req" {
            let me = self.clone();
            tokio::spawn(async move { me.handle(f).await });
        }
    }

    fn auth(&self) -> Vec<(String, String)> {
        vec![
            (
                "Authorization".into(),
                format!("Bearer {}", self.token().unwrap_or_default()),
            ),
            ("X-Peek-Device".into(), self.id.clone()),
        ]
    }

    fn folder(&self, key: &str) -> &PhoneFolder {
        self.folders
            .iter()
            .find(|f| f.key == key)
            .expect("unknown key")
    }

    /// phone-side path guard (relative, no .., stays inside the root)
    fn resolve(&self, key: &str, rel: &str) -> Option<PathBuf> {
        if rel.is_empty() || rel.starts_with('/') || rel.split('/').any(|s| s == "..") {
            return None;
        }
        let mut p = self.folder(key).root.clone();
        for s in rel.split('/') {
            p.push(s);
        }
        Some(p)
    }

    pub fn err(&self, rid: &str, status: u16, message: &str) {
        self.send(&json!({ "t": "err", "rid": rid, "status": status, "message": message }));
    }

    async fn handle(&self, f: Value) {
        let taken = {
            let mut s = self.st.lock().unwrap();
            s.intercept.take().map(|i| (i, s.intercept_gen))
        };
        if let Some((mut i, gen)) = taken {
            let took = i(&f);
            let mut s = self.st.lock().unwrap();
            if s.intercept_gen == gen && s.intercept.is_none() {
                s.intercept = Some(i);
            }
            if took {
                return;
            }
        }
        if self
            .st
            .lock()
            .unwrap()
            .ignore
            .contains(f["op"].as_str().unwrap_or(""))
        {
            return;
        }
        let rid = f["rid"].as_str().unwrap().to_string();
        let a = &f["args"];
        let key = a["key"].as_str().unwrap_or("");
        if !self.folders.iter().any(|x| x.key == key) {
            return self.err(&rid, 404, "unknown folder");
        }
        match f["op"].as_str().unwrap() {
            "list" => {
                let body = self.st.lock().unwrap().list_body.take();
                let body = match body {
                    Some(b) => b,
                    None => self.walk(key, a["dir"].as_str().unwrap_or("")),
                };
                self.reply(
                    &rid,
                    vec![("Content-Type", "application/x-ndjson".into())],
                    Bytes::from(body).into(),
                );
            }
            "thumb" | "display" => {
                let Some(_abs) = self.resolve(key, a["p"].as_str().unwrap_or("")) else {
                    return self.err(&rid, 403, "outside shared folder");
                };
                let jpg = Bytes::from_static(&[0xff, 0xd8, 0xff, 0xd9]);
                self.reply(
                    &rid,
                    vec![
                        ("Content-Type", "image/jpeg".into()),
                        ("Content-Length", "4".into()),
                    ],
                    jpg.into(),
                );
            }
            "strip" => {
                let jpg = Bytes::from_static(&[0xff, 0xd8, 0xff, 0xd9]);
                self.reply(
                    &rid,
                    vec![
                        ("Content-Type", "image/jpeg".into()),
                        ("Content-Length", "4".into()),
                    ],
                    jpg.into(),
                );
            }
            "details" => {
                let body = json!({ "exif": { "Make": "Google", "Model": "Pixel 8", "FNumber": 1.7 }, "gps": { "lat": 40.99, "lon": 29.03 }, "make": "Google", "model": "Pixel 8", "lens": null }).to_string();
                let len = body.len().to_string();
                self.reply(
                    &rid,
                    vec![
                        ("Content-Type", "application/json".into()),
                        ("Content-Length", len),
                    ],
                    Bytes::from(body).into(),
                );
            }
            "file" => self.file(&rid, a).await,
            "receive" => self.receive(&rid, a).await,
            _ => self.err(&rid, 400, "unknown op"),
        }
    }

    fn walk(&self, key: &str, dir: &str) -> String {
        let f = self.folder(key).clone();
        let mut lines = Vec::new();
        fn go(abs: &std::path::Path, rel: &str, lines: &mut Vec<String>) {
            let mut entries: Vec<_> = std::fs::read_dir(abs)
                .map(|r| r.filter_map(|e| e.ok()).collect())
                .unwrap_or_default();
            entries.sort_by_key(|e| e.file_name());
            if entries.is_empty() && !rel.is_empty() {
                lines.push(json!({ "dir": rel }).to_string());
            }
            for e in entries {
                let name = e.file_name().to_string_lossy().into_owned();
                let p = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}/{name}")
                };
                if e.file_type().unwrap().is_dir() {
                    go(&e.path(), &p, lines);
                } else if !name.ends_with(".part") {
                    let st = e.metadata().unwrap();
                    let m = st
                        .modified()
                        .unwrap()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as i64;
                    lines.push(json!({ "p": p, "k": pa_core::kind_of(&name).map(|k| k.as_str()), "s": st.len(), "m": m, "w": null, "h": null, "d": null, "t": null, "id": null }).to_string());
                }
            }
        }
        let start = if dir.is_empty() {
            f.root.clone()
        } else {
            f.root.join(dir)
        };
        go(&start, dir, &mut lines);
        for (k, v) in self.st.lock().unwrap().virtual_files.iter() {
            let (vk, p) = k.split_once('/').unwrap();
            if vk == key && (dir.is_empty() || p.starts_with(&format!("{dir}/"))) {
                lines.push(json!({ "p": p, "k": pa_core::kind_of(p).map(|k| k.as_str()), "s": v.size, "m": v.mtime, "w": 1920, "h": 1080, "d": 60, "t": v.mtime, "id": null }).to_string());
            }
        }
        lines.join("\n") + "\n"
    }

    async fn file(&self, rid: &str, a: &Value) {
        let key = a["key"].as_str().unwrap();
        let p = a["p"].as_str().unwrap();
        let v = self
            .st
            .lock()
            .unwrap()
            .virtual_files
            .get(&format!("{key}/{p}"))
            .copied();
        let (size, mtime, abs) = if let Some(v) = v {
            (v.size, v.mtime, None)
        } else {
            let Some(abs) = self.resolve(key, p) else {
                return self.err(rid, 403, "outside shared folder");
            };
            let Ok(st) = std::fs::metadata(&abs) else {
                return self.err(rid, 404, "not found");
            };
            let m = st
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            (st.len(), m, Some(abs))
        };
        let (mut start, mut end, mut status) = (0u64, size.saturating_sub(1), 200);
        if let Some(r) = a["range"].as_str() {
            let (x, y) = r.strip_prefix("bytes=").unwrap().split_once('-').unwrap();
            if x.is_empty() {
                start = size.saturating_sub(y.parse().unwrap());
            } else {
                start = x.parse().unwrap();
                if !y.is_empty() {
                    end = (size - 1).min(y.parse().unwrap());
                }
            }
            if start >= size || start > end {
                self.reply(
                    rid,
                    vec![
                        ("X-Peek-Status", "416".into()),
                        ("X-Peek-Content-Range", format!("bytes */{size}")),
                        ("X-Peek-Total", size.to_string()),
                        ("Content-Length", "0".into()),
                    ],
                    Bytes::new().into(),
                );
                return;
            }
            status = 206;
        }
        let len = end - start + 1;
        let mut h = vec![
            ("X-Peek-Status", status.to_string()),
            (
                "Content-Type",
                if pa_core::kind_of(p) == Some(MediaKind::Video) {
                    "video/mp4".into()
                } else {
                    "application/octet-stream".into()
                },
            ),
            ("Content-Length", len.to_string()),
            ("X-Peek-Total", size.to_string()),
            ("X-Peek-Mtime", mtime.to_string()),
        ];
        if status == 206 {
            h.push((
                "X-Peek-Content-Range",
                format!("bytes {start}-{end}/{size}"),
            ));
        }
        let body = match abs {
            Some(abs) => {
                let data = std::fs::read(abs).unwrap();
                Bytes::from(data[start as usize..=end as usize].to_vec()).into()
            }
            None => {
                let st = self.st.clone();
                let stream = futures_util::stream::unfold(start, move |pos| {
                    let st = st.clone();
                    async move {
                        if pos > end {
                            return None;
                        }
                        let n = (64 * 1024).min(end + 1 - pos);
                        let b = virtual_bytes(pos, pos + n);
                        st.lock().unwrap().written += n;
                        Some((Ok::<_, std::io::Error>(Bytes::from(b)), pos + n))
                    }
                });
                reqwest::Body::wrap_stream(stream)
            }
        };
        self.reply(rid, h, body);
    }

    async fn receive(&self, rid: &str, a: &Value) {
        let key = a["key"].as_str().unwrap();
        let dir = a["dir"].as_str().unwrap_or("");
        let dir_abs = if dir.is_empty() {
            Some(self.folder(key).root.clone())
        } else {
            self.resolve(key, dir)
        };
        let Some(dir_abs) = dir_abs else {
            return self.err(rid, 403, "outside shared folder");
        };
        std::fs::create_dir_all(&dir_abs).unwrap();
        let mut req = self
            .http
            .get(format!("{}{}", self.base, a["url"].as_str().unwrap()));
        for (k, v) in self.auth() {
            req = req.header(k, v);
        }
        let res = req.send().await.unwrap();
        if res.status() != 200 {
            return self.err(rid, 502, &format!("pull failed {}", res.status().as_u16()));
        }
        let headers: HashMap<String, String> = res
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap().to_string()))
            .collect();
        let body = res.bytes().await.unwrap();
        self.st.lock().unwrap().last_pull_headers = headers.clone();
        let name0 = a["name"].as_str().unwrap();
        let ext = pa_jobs::paths::extname(name0);
        let mut name = name0.to_string();
        let mut i = 1;
        while dir_abs.join(&name).exists() {
            name = format!("{} ({i}){ext}", &name0[..name0.len() - ext.len()]);
            i += 1;
        }
        std::fs::write(dir_abs.join(&name), &body).unwrap();
        let mt = headers
            .get("x-peek-mtime")
            .and_then(|m| m.parse::<i64>().ok())
            .unwrap_or(a["mtime"].as_i64().unwrap());
        pa_jobs::paths::set_mtime(&dir_abs.join(&name), mt).await;
        let p = if dir.is_empty() {
            name
        } else {
            format!("{dir}/{name}")
        };
        self.send(&json!({ "t": "res", "rid": rid, "ok": true, "data": { "p": p } }));
        self.send(&json!({ "t": "changed", "key": key, "dirs": [dir] }));
    }

    /// stream a reply PUT; the result (status, bytes sent) lands in `puts[rid]`
    pub fn reply(&self, rid: &str, headers: Vec<(&str, String)>, body: reqwest::Body) {
        let (tx, rx) = tokio::sync::watch::channel(None);
        self.st.lock().unwrap().puts.insert(rid.to_string(), rx);
        let me = self.clone();
        let rid = rid.to_string();
        let headers: Vec<(String, String)> = headers
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        tokio::spawn(async move {
            let auth = me.auth();
            let r = me.put(&rid, headers, body, auth).await;
            let _ = tx.send(Some(r));
        });
    }

    pub async fn put(
        &self,
        rid: &str,
        headers: Vec<(String, String)>,
        body: reqwest::Body,
        auth: Vec<(String, String)>,
    ) -> (u16, u64) {
        let mut req = self
            .http
            .put(format!("{}/api/devices/reply/{rid}", self.base));
        for (k, v) in auth.into_iter().chain(headers) {
            req = req.header(k, v);
        }
        let before = self.st.lock().unwrap().written;
        match req.body(body).send().await {
            Ok(r) => (
                r.status().as_u16(),
                self.st.lock().unwrap().written - before,
            ),
            Err(_) => (0, self.st.lock().unwrap().written - before),
        }
    }

    pub async fn put_result(&self, rid: &str) -> (u16, u64) {
        let mut rx = until(|| self.st.lock().unwrap().puts.get(rid).cloned(), 10_000).await;
        let v = rx.wait_for(|v| v.is_some()).await.unwrap();
        v.unwrap()
    }
}
