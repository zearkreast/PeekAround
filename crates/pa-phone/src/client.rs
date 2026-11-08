//! The phone side of PeekAround Link v1 (1.x `LinkClient.kt`, docs/link-protocol.md): one control
//! WebSocket to `GET /api/devices/connect`, data replies as `PUT /api/devices/reply/:rid`, PC →
//! phone files pulled from one-time URLs. The phone only reads files inside the shared folders
//! and writes new files (unique names) into them: it never deletes, renames or overwrites.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Map, Value};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::{mpsc, Notify, Semaphore};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use crate::codec::{self, Incoming, LinkError};
use crate::ops::{self, MetaCache, Platform};
use crate::prefs::{LinkFolder, LinkPrefs, LinkStatus};
use crate::watch::FolderWatch;

const PING: Duration = Duration::from_secs(30);
const IDLE: Duration = Duration::from_secs(75);
/// the server keeps at most 8 requests in flight per device
const WORKERS: usize = 8;

/// why the link stopped for good (the service stops and tells the user)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkEnd {
    /// stopped by the user / the app
    Stopped,
    /// not configured (sharing off, no server, no folders)
    NotConfigured,
    /// the PC rejected the pairing
    Rejected,
    /// the PC forgot this phone
    Forgotten,
    /// the server speaks another protocol version
    Unsupported,
}

enum SessionEnd {
    Stopped,
    Fatal(LinkEnd),
    /// WS close code; 5000 + HTTP status when the upgrade failed; -1 network error; 0 reconnect
    Closed(i32, String),
}

struct Inner {
    files: PathBuf,
    platform: Arc<dyn Platform>,
    app: String,
    model: String,
    stop: CancellationToken,
    reconnect: Notify,
    folders_changed: Notify,
    http: reqwest::Client,
    cache: MetaCache,
    status: Mutex<LinkStatus>,
    folders: RwLock<Vec<LinkFolder>>,
    base: RwLock<String>,
    device_id: RwLock<String>,
    token: RwLock<Option<String>>,
    out: Mutex<Option<mpsc::UnboundedSender<Message>>>,
    watch: Mutex<Option<FolderWatch>>,
    jobs: Mutex<HashMap<String, CancellationToken>>,
    active: AtomicUsize,
    sem: Arc<Semaphore>,
}

/// A running link (owned by the foreground service).
pub struct LinkClient {
    inner: Arc<Inner>,
    task: tokio::task::JoinHandle<LinkEnd>,
}

impl LinkClient {
    /// Start on the current tokio runtime with the settings in `files/link.json`.
    /// `app`: version sent in hello; `model`: maker + model.
    pub fn start(files: &Path, platform: Arc<dyn Platform>, app: &str, model: &str) -> LinkClient {
        let http = reqwest::Client::builder()
            .user_agent(concat!("PeekAround/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("http client");
        let inner = Arc::new(Inner {
            files: files.to_path_buf(),
            platform,
            app: app.to_string(),
            model: model.to_string(),
            stop: CancellationToken::new(),
            reconnect: Notify::new(),
            folders_changed: Notify::new(),
            http,
            cache: MetaCache::default(),
            status: Mutex::new(LinkStatus::default()),
            folders: RwLock::default(),
            base: RwLock::default(),
            device_id: RwLock::default(),
            token: RwLock::default(),
            out: Mutex::default(),
            watch: Mutex::default(),
            jobs: Mutex::default(),
            active: AtomicUsize::new(0),
            sem: Arc::new(Semaphore::new(WORKERS)),
        });
        let i2 = inner.clone();
        let task = tokio::spawn(async move { run(i2).await });
        LinkClient { inner, task }
    }

    /// stop (closes the socket, cancels running requests)
    pub fn stop(&self) {
        self.inner.stop.cancel();
    }

    /// link.json's folders changed: tell the server, re-watch
    pub fn folders_changed(&self) {
        self.inner.folders_changed.notify_one();
    }

    /// reconnect now (url / name / token changed)
    pub fn reconnect(&self) {
        self.inner.reconnect.notify_one();
    }

    /// current state
    pub fn status(&self) -> LinkStatus {
        self.inner.status.lock().unwrap().clone()
    }

    /// wait until the link ends
    pub async fn finished(self) -> LinkEnd {
        self.task.await.unwrap_or(LinkEnd::Stopped)
    }
}

impl Inner {
    fn set_status(&self, state: &str, code: Option<String>, error: Option<String>) {
        let s = {
            let mut s = self.status.lock().unwrap();
            s.state = state.to_string();
            s.code = code;
            s.error = error;
            s.clone()
        };
        s.save(&self.files);
        self.platform.status(&s);
    }

    fn set_active(&self, n: usize) {
        let s = {
            let mut s = self.status.lock().unwrap();
            s.active = n;
            s.clone()
        };
        s.save(&self.files);
        self.platform.active(n);
    }

    /// load-modify-save of link.json (the UI process writes it too)
    fn update_prefs(&self, f: impl FnOnce(&mut LinkPrefs)) {
        let mut p = LinkPrefs::load(&self.files);
        f(&mut p);
        if let Err(e) = p.save(&self.files) {
            log::warn!("link.json: {e}");
        }
    }

    fn send(&self, text: String) {
        if let Some(tx) = self.out.lock().unwrap().as_ref() {
            let _ = tx.send(Message::text(text));
        }
    }

    fn authed(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let rb = rb.header("X-Peek-Device", self.device_id.read().unwrap().clone());
        match self.token.read().unwrap().clone() {
            Some(t) => rb.header("Authorization", format!("Bearer {t}")),
            None => rb,
        }
    }

    fn cancel_all(&self) {
        for (_, t) in self.jobs.lock().unwrap().drain() {
            t.cancel();
        }
    }
}

async fn run(inner: Arc<Inner>) -> LinkEnd {
    let mut attempt: u32 = 0;
    let end = loop {
        if inner.stop.is_cancelled() {
            break LinkEnd::Stopped;
        }
        let mut prefs = LinkPrefs::load(&inner.files);
        if !prefs.enabled {
            break LinkEnd::Stopped;
        }
        let Some(url) = prefs.url.clone().filter(|_| !prefs.folders.is_empty()) else {
            inner.set_status("error", None, Some("not configured".into()));
            break LinkEnd::NotConfigured;
        };
        if prefs.device_id.as_ref().is_none_or(|s| s.len() < 8) {
            prefs.ensure_device_id();
            let id = prefs.device_id.clone();
            inner.update_prefs(|p| p.device_id = id);
        }
        *inner.base.write().unwrap() = url.trim_end_matches('/').to_string();
        *inner.device_id.write().unwrap() = prefs.device_id.clone().unwrap_or_default();
        *inner.token.write().unwrap() = prefs.token.clone();
        *inner.folders.write().unwrap() = prefs.folders.clone();
        let (pending, code) = {
            let s = inner.status.lock().unwrap();
            (s.state == "pending", s.code.clone())
        };
        let err = inner.status.lock().unwrap().error.clone();
        if pending {
            inner.set_status("pending", code, err);
        } else {
            inner.set_status("connecting", None, err);
        }
        let end = session(&inner, &prefs, &mut attempt).await;
        *inner.out.lock().unwrap() = None;
        *inner.watch.lock().unwrap() = None;
        inner.cancel_all();
        let (code, reason) = match end {
            SessionEnd::Stopped => break LinkEnd::Stopped,
            SessionEnd::Fatal(e) => break e,
            SessionEnd::Closed(code, reason) => (code, reason),
        };
        log::info!("link closed {code} {reason}");
        if code == 0 {
            attempt = 0;
            continue;
        }
        let err = match code {
            4409 => "replaced".to_string(),
            5000..=5999 => format!("HTTP {}", code - 5000),
            -1 => reason,
            c => format!("closed ({c})"),
        };
        let st = if inner.status.lock().unwrap().state == "pending" {
            "pending"
        } else {
            "connecting"
        };
        let code_now = inner.status.lock().unwrap().code.clone();
        inner.set_status(st, code_now.filter(|_| st == "pending"), Some(err));
        // replaced by another connection of this device: back off longer so two sockets don't fight
        let delay = if code == 4409 {
            Duration::from_secs(60)
        } else {
            let d = codec::backoff_ms(attempt, rand::random::<f64>());
            attempt += 1;
            Duration::from_millis(d)
        };
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = inner.reconnect.notified() => attempt = 0,
            _ = inner.stop.cancelled() => break LinkEnd::Stopped,
        }
    };
    *inner.out.lock().unwrap() = None;
    *inner.watch.lock().unwrap() = None;
    inner.cancel_all();
    match &end {
        LinkEnd::Stopped => inner.set_status("off", None, None),
        LinkEnd::NotConfigured => {}
        LinkEnd::Rejected => {
            inner.update_prefs(|p| {
                p.enabled = false;
                p.token = None;
            });
            inner.set_status("rejected", None, None);
        }
        LinkEnd::Forgotten => {
            inner.update_prefs(|p| {
                p.enabled = false;
                p.token = None;
            });
            inner.set_status("error", None, Some("forgotten".into()));
        }
        LinkEnd::Unsupported => {
            inner.update_prefs(|p| p.enabled = false);
            inner.set_status("error", None, Some("unsupported server version".into()));
        }
    }
    end
}

fn ws_url(base: &str) -> String {
    let rest = base
        .strip_prefix("https://")
        .map(|r| format!("wss://{r}"))
        .or_else(|| base.strip_prefix("http://").map(|r| format!("ws://{r}")))
        .unwrap_or_else(|| base.to_string());
    format!("{rest}/api/devices/connect?v={}", codec::VERSION)
}

async fn session(inner: &Arc<Inner>, prefs: &LinkPrefs, attempt: &mut u32) -> SessionEnd {
    let base = inner.base.read().unwrap().clone();
    let mut req = match ws_url(&base).into_client_request() {
        Ok(r) => r,
        Err(_) => return SessionEnd::Fatal(LinkEnd::NotConfigured),
    };
    let id = inner.device_id.read().unwrap().clone();
    if let Ok(v) = id.parse() {
        req.headers_mut().insert("X-Peek-Device", v);
    }
    if let Some(t) = &prefs.token {
        if let Ok(v) = format!("Bearer {t}").parse() {
            req.headers_mut().insert("Authorization", v);
        }
    }
    let connect = tokio::time::timeout(
        Duration::from_secs(15),
        tokio_tungstenite::connect_async(req),
    );
    let ws = tokio::select! {
        r = connect => match r {
            Err(_) => return SessionEnd::Closed(-1, "timeout".into()),
            Ok(Err(tokio_tungstenite::tungstenite::Error::Http(resp))) => {
                return SessionEnd::Closed(5000 + resp.status().as_u16() as i32, "upgrade refused".into())
            }
            Ok(Err(e)) => return SessionEnd::Closed(-1, e.to_string()),
            Ok(Ok((ws, _))) => ws,
        },
        _ = inner.stop.cancelled() => return SessionEnd::Stopped,
    };
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    *inner.out.lock().unwrap() = Some(tx.clone());
    let writer = tokio::spawn(async move {
        while let Some(m) = rx.recv().await {
            let close = matches!(m, Message::Close(_));
            if sink.send(m).await.is_err() || close {
                break;
            }
        }
        let _ = sink.close().await;
    });

    let name = prefs
        .device_name
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| inner.model.clone());
    let folders = inner.folders.read().unwrap().clone();
    inner.send(codec::hello(&id, &name, &inner.model, &inner.app, &folders));
    start_watch(inner, &folders);

    let mut last_seen = Instant::now();
    let mut beat = tokio::time::interval_at(tokio::time::Instant::now() + PING, PING);
    let close = |code: u16, why: &str| {
        let _ = tx.send(Message::Close(Some(
            tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: CloseCode::from(code),
                reason: why.to_string().into(),
            },
        )));
    };
    let end = loop {
        tokio::select! {
            _ = inner.stop.cancelled() => { close(1000, "stopped"); break SessionEnd::Stopped; }
            _ = inner.reconnect.notified() => { close(1000, "reconnect"); break SessionEnd::Closed(0, "reconnect".into()); }
            _ = inner.folders_changed.notified() => {
                let p = LinkPrefs::load(&inner.files);
                *inner.folders.write().unwrap() = p.folders.clone();
                start_watch(inner, &p.folders);
                if inner.status.lock().unwrap().state == "online" {
                    inner.send(codec::folders(&p.folders));
                }
            }
            _ = beat.tick() => {
                if last_seen.elapsed() > IDLE {
                    break SessionEnd::Closed(-1, "no traffic".into());
                }
                inner.send(codec::ping());
            }
            m = stream.next() => {
                last_seen = Instant::now();
                match m {
                    None => break SessionEnd::Closed(-1, "socket closed".into()),
                    Some(Err(e)) => break SessionEnd::Closed(-1, e.to_string()),
                    Some(Ok(Message::Close(f))) => {
                        let code = f.as_ref().map(|f| u16::from(f.code) as i32).unwrap_or(1005);
                        let reason = f.map(|f| f.reason.to_string()).unwrap_or_default();
                        match code {
                            4403 => break SessionEnd::Fatal(LinkEnd::Rejected),
                            4401 => break SessionEnd::Fatal(LinkEnd::Forgotten),
                            4426 => break SessionEnd::Fatal(LinkEnd::Unsupported),
                            c => break SessionEnd::Closed(c, reason),
                        }
                    }
                    Some(Ok(Message::Text(t))) => {
                        if let Some(end) = on_frame(inner, codec::parse(t.as_str()), attempt) {
                            close(1000, "rejected");
                            break end;
                        }
                    }
                    Some(Ok(_)) => {}
                }
            }
        }
    };
    drop(tx);
    *inner.out.lock().unwrap() = None;
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
    end
}

fn start_watch(inner: &Arc<Inner>, folders: &[LinkFolder]) {
    let weak = Arc::downgrade(inner);
    let w = FolderWatch::start(folders, move |frame| {
        if let Some(i) = weak.upgrade() {
            i.send(frame);
        }
    });
    *inner.watch.lock().unwrap() = Some(w);
}

fn on_frame(inner: &Arc<Inner>, f: Incoming, attempt: &mut u32) -> Option<SessionEnd> {
    match f {
        Incoming::Pending(code) => {
            // the server does not (or no longer) accept our token: forget it, wait for approval
            *inner.token.write().unwrap() = None;
            inner.update_prefs(|p| p.token = None);
            inner.set_status("pending", Some(code), None);
        }
        Incoming::Paired(token) => {
            *inner.token.write().unwrap() = Some(token.clone());
            inner.update_prefs(|p| p.token = Some(token));
            *attempt = 0;
            inner.set_status("online", None, None);
        }
        Incoming::Welcome(server) => {
            *attempt = 0;
            if !server.is_empty() {
                inner.update_prefs(|p| p.server_name = Some(server));
            }
            inner.set_status("online", None, None);
        }
        Incoming::Rejected => return Some(SessionEnd::Fatal(LinkEnd::Rejected)),
        Incoming::Ping => inner.send(codec::pong()),
        Incoming::Pong | Incoming::Unknown => {}
        Incoming::Req { rid, op, args } => dispatch(inner, rid, op, args),
        Incoming::Cancel(rid) => {
            if let Some(t) = inner.jobs.lock().unwrap().remove(&rid) {
                t.cancel();
            }
        }
    }
    None
}

fn dispatch(inner: &Arc<Inner>, rid: String, op: String, args: Map<String, Value>) {
    let token = CancellationToken::new();
    {
        let mut jobs = inner.jobs.lock().unwrap();
        if jobs.contains_key(&rid) {
            return;
        }
        jobs.insert(rid.clone(), token.clone());
    }
    let inner = inner.clone();
    tokio::spawn(async move {
        let permit = tokio::select! {
            p = inner.sem.clone().acquire_owned() => p.ok(),
            _ = token.cancelled() => None,
        };
        if let Some(_permit) = permit {
            let n = inner.active.fetch_add(1, Ordering::SeqCst) + 1;
            inner.set_active(n);
            let r = tokio::select! {
                r = run_job(&inner, &rid, &op, &args, &token) => r,
                _ = token.cancelled() => Ok(()),
            };
            if let Err(e) = r {
                if !token.is_cancelled() {
                    if e.status >= 500 {
                        log::warn!("op {op} failed: {e}");
                    }
                    inner.send(codec::err(&rid, e.status, &e.message));
                }
            }
            let n = inner.active.fetch_sub(1, Ordering::SeqCst) - 1;
            inner.set_active(n);
        }
        inner.jobs.lock().unwrap().remove(&rid);
    });
}

fn arg_str<'a>(a: &'a Map<String, Value>, k: &str) -> &'a str {
    a.get(k).and_then(Value::as_str).unwrap_or("")
}

fn arg_u32(a: &Map<String, Value>, k: &str, def: u32) -> u32 {
    a.get(k)
        .and_then(Value::as_f64)
        .filter(|x| x.is_finite() && *x >= 0.0)
        .map(|x| x as u32)
        .unwrap_or(def)
}

/// an existing regular file at args.p inside the shared folder
fn file_arg(root: &Path, a: &Map<String, Value>) -> Result<PathBuf, LinkError> {
    let p = arg_str(a, "p");
    if p.is_empty() {
        return Err(LinkError::new(400, "p required"));
    }
    let f = codec::resolve(root, p)?;
    if !f.is_file() {
        return Err(LinkError::new(404, "not found"));
    }
    Ok(f)
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, LinkError> + Send + 'static,
) -> Result<T, LinkError> {
    tokio::task::spawn_blocking(f)
        .await
        .unwrap_or_else(|_| Err(LinkError::new(500, "worker failed")))
}

async fn run_job(
    inner: &Arc<Inner>,
    rid: &str,
    op: &str,
    a: &Map<String, Value>,
    token: &CancellationToken,
) -> Result<(), LinkError> {
    let folder = inner
        .folders
        .read()
        .unwrap()
        .iter()
        .find(|f| f.key == arg_str(a, "key"))
        .cloned()
        .ok_or_else(|| LinkError::new(404, "unknown folder"))?;
    let root = PathBuf::from(&folder.path);
    if !root.is_dir() {
        return Err(LinkError::new(404, "folder not available"));
    }
    let pf = inner.platform.clone();
    match op {
        "list" => op_list(inner, rid, &root, arg_str(a, "dir"), token).await,
        "thumb" | "display" | "strip" | "details" => {
            let f = file_arg(&root, a)?;
            let name = f.to_string_lossy().into_owned();
            let kind = codec::kind(&name).unwrap_or("image");
            let (s, max, n, h) = (
                arg_u32(a, "s", 320),
                arg_u32(a, "max", 4096),
                arg_u32(a, "n", 10),
                arg_u32(a, "h", 90),
            );
            let op2 = op.to_string();
            let kind2 = kind.to_string();
            let (body, ct) = blocking(move || {
                let k = kind2.as_str();
                Ok(match op2.as_str() {
                    "thumb" if k == "image" || k == "video" => {
                        (ops::thumb(&f, k, s, pf.as_ref())?, "image/jpeg")
                    }
                    "thumb" => return Err(LinkError::new(404, "no thumbnail")),
                    "display" => (ops::display(&f, k, max, pf.as_ref())?, "image/jpeg"),
                    "strip" => (ops::strip(&f, k, n, h, pf.as_ref())?, "image/jpeg"),
                    _ => (
                        ops::details(&f, k, pf.as_ref()).to_string().into_bytes(),
                        "application/json",
                    ),
                })
            })
            .await?;
            put(inner, rid, &[("Content-Type", ct.to_string())], body.into(), token).await;
            Ok(())
        }
        "file" => {
            let f = file_arg(&root, a)?;
            let meta = std::fs::metadata(&f)?;
            let mtime = mtime_ms(&meta);
            let name = f.to_string_lossy().into_owned();
            let range = a.get("range").and_then(Value::as_str);
            let reply = codec::file_reply(range, meta.len(), mtime, codec::mime(&name));
            let mut file = tokio::fs::File::open(&f).await?;
            if reply.offset > 0 {
                file.seek(std::io::SeekFrom::Start(reply.offset)).await?;
            }
            let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::with_capacity(
                file.take(reply.length),
                128 * 1024,
            ));
            let headers: Vec<(&str, String)> = reply
                .headers
                .iter()
                .map(|(k, v)| (k.as_str(), v.clone()))
                .collect();
            put(inner, rid, &headers, body, token).await;
            Ok(())
        }
        "receive" => op_receive(inner, rid, &folder, &root, a, token).await,
        other => Err(LinkError::new(400, format!("unknown op {other}"))),
    }
}

fn mtime_ms(m: &std::fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// the reply PUT (Content-Length from the headers / the body)
async fn put(
    inner: &Arc<Inner>,
    rid: &str,
    headers: &[(&str, String)],
    body: reqwest::Body,
    token: &CancellationToken,
) {
    if token.is_cancelled() {
        return;
    }
    let base = inner.base.read().unwrap().clone();
    let mut rb = inner.authed(inner.http.put(format!("{base}/api/devices/reply/{rid}")));
    for (k, v) in headers {
        rb = rb.header(*k, v.as_str());
    }
    let r = tokio::select! {
        r = rb.body(body).send() => r,
        _ = token.cancelled() => return,
    };
    match r {
        // 499: the consumer went away; 410: unknown / expired rid
        Ok(res) if res.status().as_u16() == 499 || res.status().as_u16() == 410 => token.cancel(),
        Ok(res) if res.status().as_u16() >= 400 => {
            log::warn!("reply {rid} → {}", res.status())
        }
        Ok(_) => {}
        Err(e) => {
            if !token.is_cancelled() {
                log::warn!("reply {rid} failed: {e}");
            }
        }
    }
}

async fn op_list(
    inner: &Arc<Inner>,
    rid: &str,
    root0: &Path,
    dir: &str,
    token: &CancellationToken,
) -> Result<(), LinkError> {
    let root = root0.canonicalize()?;
    let start = codec::resolve(&root, dir)?;
    if !start.is_dir() {
        return Err(LinkError::new(404, "not found"));
    }
    let (tx, rx) = mpsc::channel::<Bytes>(64);
    let i2 = inner.clone();
    let t2 = token.clone();
    tokio::task::spawn_blocking(move || {
        let mut buf = String::with_capacity(64 * 1024);
        let mut last = Instant::now();
        let flush = |buf: &mut String| -> bool {
            if buf.is_empty() {
                return true;
            }
            tx.blocking_send(Bytes::from(std::mem::take(buf))).is_ok()
        };
        let mut stack = vec![start];
        while let Some(d) = stack.pop() {
            if t2.is_cancelled() {
                return;
            }
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            let mut entries: Vec<std::fs::DirEntry> = rd.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            let mut files = 0;
            let mut subdirs = vec![];
            for e in entries {
                let name = e.file_name().to_string_lossy().into_owned();
                if codec::hidden(&name) {
                    continue;
                }
                let p = e.path();
                let Ok(md) = std::fs::metadata(&p) else { continue };
                let is_link = e.file_type().map(|t| t.is_symlink()).unwrap_or(false);
                if is_link {
                    // a symlink out of the shared folder is not followed
                    match p.canonicalize() {
                        Ok(c) if codec::is_inside(&root, &c) => {}
                        _ => continue,
                    }
                }
                if md.is_dir() {
                    subdirs.push(p);
                    continue;
                }
                let Some(k) = codec::kind(&name) else { continue };
                let mtime = mtime_ms(&md);
                let meta = i2.cache.get(&p, k, md.len(), mtime, i2.platform.as_ref());
                buf.push_str(&codec::list_line(&codec::rel_of(&root, &p), k, md.len(), mtime, &meta));
                buf.push('\n');
                files += 1;
                if buf.len() > 32 * 1024 || last.elapsed() > Duration::from_millis(500) {
                    if !flush(&mut buf) {
                        return;
                    }
                    last = Instant::now();
                }
            }
            let rel = codec::rel_of(&root, &d);
            if files == 0 && !rel.is_empty() {
                buf.push_str(&codec::dir_line(&rel));
                buf.push('\n');
            }
            subdirs.sort();
            stack.extend(subdirs.into_iter().rev());
        }
        flush(&mut buf);
    });
    let body = reqwest::Body::wrap_stream(futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|b| (Ok::<Bytes, std::io::Error>(b), rx))
    }));
    put(
        inner,
        rid,
        &[("Content-Type", "application/x-ndjson".into())],
        body,
        token,
    )
    .await;
    Ok(())
}

async fn op_receive(
    inner: &Arc<Inner>,
    rid: &str,
    folder: &LinkFolder,
    root0: &Path,
    a: &Map<String, Value>,
    token: &CancellationToken,
) -> Result<(), LinkError> {
    let root = root0.canonicalize()?;
    let dir_rel = codec::clean_rel(arg_str(a, "dir"))?;
    let name = codec::clean_name(arg_str(a, "name"))?;
    if codec::hidden(&name) {
        return Err(LinkError::new(400, "bad file name"));
    }
    let url = arg_str(a, "url");
    if !url.starts_with("/api/devices/pull/") {
        return Err(LinkError::new(400, "bad url"));
    }
    let size = a.get("size").and_then(Value::as_i64).unwrap_or(-1);
    let mut mtime = a.get("mtime").and_then(Value::as_i64).unwrap_or(0);
    // create the target dir inside the folder, every step re-checked after canonicalisation
    let mut dir = root.clone();
    if !dir_rel.is_empty() {
        for seg in dir_rel.split('/') {
            let next = dir.join(seg);
            if !next.exists() && std::fs::create_dir(&next).is_err() && !next.is_dir() {
                return Err(LinkError::new(500, "cannot create folder"));
            }
            dir = next.canonicalize()?;
            if !codec::is_inside(&root, &dir) || !dir.is_dir() {
                return Err(LinkError::new(403, "outside shared folder"));
            }
        }
    }
    let part_name = codec::unique_name(&format!("{name}.part"), |n| dir.join(n).exists());
    let part = dir.join(&part_name);
    let base = inner.base.read().unwrap().clone();
    let res = tokio::select! {
        r = inner.authed(inner.http.get(format!("{base}{url}"))).send() => r.map_err(|e| LinkError::new(500, e.to_string()))?,
        _ = token.cancelled() => return Ok(()),
    };
    if let Some(m) = res
        .headers()
        .get("X-Peek-Mtime")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|m| *m > 0)
    {
        mtime = m;
    }
    if !res.status().is_success() {
        let st = res.status().as_u16();
        return Err(LinkError::new(
            if st == 404 { 404 } else { 500 },
            format!("pull failed ({st})"),
        ));
    }
    let write = async {
        let mut out = tokio::fs::File::create(&part).await?;
        let mut n: i64 = 0;
        let mut body = res.bytes_stream();
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| LinkError::new(500, e.to_string()))?;
            out.write_all(&chunk).await?;
            n += chunk.len() as i64;
        }
        out.sync_all().await?;
        if size >= 0 && n != size {
            return Err(LinkError::new(500, format!("size mismatch ({n} of {size} bytes)")));
        }
        Ok::<(), LinkError>(())
    };
    let r = tokio::select! {
        r = write => r,
        _ = token.cancelled() => Err(LinkError::new(499, "cancelled")),
    };
    if let Err(e) = r {
        let _ = std::fs::remove_file(&part);
        return if e.status == 499 { Ok(()) } else { Err(e) };
    }
    // unique final name: never overwrite an existing file
    let mut dest = dir.join(codec::unique_name(&name, |n| dir.join(n).exists()));
    let mut tries = 0;
    while std::fs::rename(&part, &dest).is_err() {
        tries += 1;
        if tries > 3 {
            let _ = std::fs::remove_file(&part);
            return Err(LinkError::new(500, "cannot rename"));
        }
        dest = dir.join(codec::unique_name(&name, |n| dir.join(n).exists()));
    }
    if mtime > 0 {
        if let Ok(f) = std::fs::File::options().write(true).open(&dest) {
            let _ = f.set_modified(
                std::time::UNIX_EPOCH + Duration::from_millis(mtime as u64),
            );
        }
    }
    inner.platform.scanned(&dest);
    let rel = codec::rel_of(&root, &dest);
    inner.send(codec::res(rid, json!({ "p": rel })));
    if let Some(w) = inner.watch.lock().unwrap().as_ref() {
        w.touched(&folder.key, &codec::rel_of(&root, &dir));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_urls() {
        assert_eq!(
            ws_url("http://10.0.0.2:1313"),
            "ws://10.0.0.2:1313/api/devices/connect?v=1"
        );
        assert_eq!(
            ws_url("https://photos.example.com"),
            "wss://photos.example.com/api/devices/connect?v=1"
        );
    }
}
