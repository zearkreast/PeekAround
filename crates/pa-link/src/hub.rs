//! DeviceHub — server side of the phone ↔ PC link (mirrors `devices/hub.ts`, docs/link-protocol.md).
//!
//! One WebSocket per phone carries small JSON control frames; every data reply flows over a
//! phone-initiated `PUT /api/devices/reply/:rid` that is piped (with backpressure, never buffered)
//! into the waiting consumer. PC → phone pushes use one-time pull URLs.
//!
//! The hub is transport-agnostic: [`DeviceHub::attach`] returns a [`SocketHandle`] (feed it the
//! socket's incoming events) and a channel of [`WsOut`] (send them on the socket). The `axum`
//! feature has the glue (`pa_link::axum`).

use crate::ndjson::NdjsonLines;
use crate::store::{DeviceFolderRow, DeviceStore};
use base64::Engine;
use bytes::Bytes;
use futures_util::future::{BoxFuture, FutureExt, Shared};
use futures_util::{Stream, StreamExt};
use pa_core::{DeviceCaps, DeviceInfo, MediaKind, MediaReason, Playback, ServerEvent};
use pa_jobs::library::{ListScope, ListedFile, MediaRowSink, ThumbCache, ThumbScope};
use pa_jobs::paths::normalize_rel;
use pa_jobs::{EventSink, HttpError};
use pa_meta::ExtractedMeta;
use regex::Regex;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// 0 = on-demand (visible) thumbnail, 1 = file / display / details / strip / receive, 2 = list / background thumbs
pub type Prio = u8;

pub const MAX_INFLIGHT: usize = 8;
/// background work never takes more than this many of the in-flight slots
pub const MAX_INFLIGHT_LOW: usize = 4;
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const PING: Duration = Duration::from_secs(25);
const IDLE: Duration = Duration::from_secs(75);
const PULL_TTL_MS: i64 = 10 * 60_000;
const CHANGED_DEBOUNCE: Duration = Duration::from_secs(2);
const MAX_PENDING: usize = 16;
const RECEIVE_CONFIRM: Duration = Duration::from_secs(60);
/// chunks buffered between the reply PUT and its consumer (backpressure beyond that)
const PIPE_CHUNKS: usize = 4;

pub static ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z0-9._-]{8,64}$").unwrap());
static KEY_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{1,64}$").unwrap());
static BEARER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Bearer\s+([A-Za-z0-9_-]{16,128})$").unwrap());
pub static RID_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{22}$").unwrap());
static ABS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^device://([A-Za-z0-9._-]+)/([A-Za-z0-9_-]+)/(.+)$").unwrap());

/// `device://` folder path scheme
pub const DEVICE_SCHEME: &str = "device://";

/// A frame / control message for the phone's socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsOut {
    Text(String),
    Ping,
    /// close with a code + reason (the socket stays readable until the peer answers)
    Close(u16, String),
    /// drop the connection without a close handshake
    Terminate,
}

/// Credentials / origin of a connecting socket.
#[derive(Debug, Clone, Default)]
pub struct ConnectMeta {
    /// came through the desktop LAN listener
    pub lan: bool,
    /// `X-Peek-Device` header (raw)
    pub header_id: Option<String>,
    /// `Authorization: Bearer <token>` (validated with [`bearer_of`])
    pub token: Option<String>,
}

/// Timeouts that tests shorten.
#[derive(Debug)]
pub struct Timeouts {
    /// first byte of thumb / list / display / strip / details
    pub short_ms: AtomicU64,
    /// first byte of file / receive
    pub file_ms: AtomicU64,
}

/// `Authorization: Bearer <token>` (16..128 base64url chars) or None
pub fn bearer_of(authorization: Option<&str>) -> Option<String> {
    let h = authorization?;
    BEARER_RE
        .captures(pa_core::js::trim(h))
        .map(|c| c[1].to_string())
}

/// `X-Peek-Device` when it is a valid device id
pub fn device_header(h: Option<&str>) -> Option<String> {
    h.filter(|s| ID_RE.is_match(s)).map(str::to_string)
}

pub fn sha256_hex(s: &str) -> String {
    let d = Sha256::digest(s.as_bytes());
    d.iter().map(|b| format!("{b:02x}")).collect()
}

fn safe_eq(a: &str, b: &str) -> bool {
    let (x, y) = (a.as_bytes(), b.as_bytes());
    x.len() == y.len() && x.iter().zip(y).fold(0u8, |acc, (p, q)| acc | (p ^ q)) == 0
}

/// `clean(v, max, fallback)`: control chars removed, trimmed, ≤ max UTF-16 units, else fallback
pub fn clean(v: Option<&Value>, max: usize, fallback: &str) -> String {
    match v {
        Some(Value::String(s)) => {
            let no_ctl: String = s.chars().filter(|c| (*c as u32) >= 0x20).collect();
            let t = pa_core::js::slice16_to(pa_core::js::trim(&no_ctl), max).to_string();
            if t.is_empty() {
                fallback.to_string()
            } else {
                t
            }
        }
        _ => fallback.to_string(),
    }
}

fn base64url(b: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

fn random_b64(n: usize) -> String {
    let b: Vec<u8> = (0..n).map(|_| rand::random::<u8>()).collect();
    base64url(&b)
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Map a phone error status to the HTTP status a consumer gets.
pub fn consumer_status(s: i64) -> u16 {
    if [400, 403, 404, 409, 413, 416, 503].contains(&s) {
        s as u16
    } else {
        502
    }
}

/// The error a canceled request rejects with (`AbortError`); never sent to a client.
pub fn aborted() -> HttpError {
    HttpError::new(499, "aborted")
}

pub fn is_aborted(e: &HttpError) -> bool {
    e.status == 499 && e.message == "aborted"
}

fn offline() -> HttpError {
    HttpError::new(503, "device offline")
}

/// A shared folder announced by the phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloFolder {
    pub key: String,
    pub name: String,
    pub path: Option<String>,
}

pub fn parse_folders(v: Option<&Value>) -> Vec<HelloFolder> {
    let Some(Value::Array(list)) = v else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for f in list.iter().take(64) {
        let key = match f.get("key") {
            Some(Value::String(k)) => k.clone(),
            _ => String::new(),
        };
        if !KEY_RE.is_match(&key) || seen.contains(&key) {
            continue;
        }
        seen.insert(key.clone());
        let name = clean(f.get("name"), 120, &key);
        let path = clean(f.get("path"), 500, "");
        out.push(HelloFolder {
            key,
            name,
            path: (!path.is_empty()).then_some(path),
        });
    }
    out
}

pub fn parse_caps(s: Option<&str>) -> DeviceCaps {
    match s.map(serde_json::from_str::<Value>) {
        Some(Ok(c)) => DeviceCaps {
            read: c.get("read") != Some(&Value::Bool(false)),
            receive: c.get("receive") == Some(&Value::Bool(true)),
        },
        Some(Err(_)) => DeviceCaps {
            read: true,
            receive: false,
        },
        None => DeviceCaps {
            read: true,
            receive: false,
        },
    }
}

fn caps_json(c: DeviceCaps) -> String {
    format!("{{\"read\":{},\"receive\":{}}}", c.read, c.receive)
}

/// `["a", "a/b", "c"]` → `["a", "c"]`; `""` swallows everything
pub fn collapse_dirs(dirs: &[String]) -> Vec<String> {
    if dirs.iter().any(|d| d.is_empty()) {
        return vec![String::new()];
    }
    let sorted: BTreeSet<&String> = dirs.iter().collect();
    let mut out: Vec<String> = Vec::new();
    for d in sorted {
        if !out
            .iter()
            .any(|o| d == o || d.starts_with(&format!("{o}/")))
        {
            out.push(d.clone());
        }
    }
    out
}

fn add_dir_chain(set: &mut HashSet<String>, d: &str) {
    let mut cur = d.to_string();
    while !cur.is_empty() && !set.contains(&cur) {
        set.insert(cur.clone());
        cur = match cur.rfind('/') {
            Some(i) => cur[..i].to_string(),
            None => String::new(),
        };
    }
}

fn safe_rel(p: &str) -> Option<String> {
    normalize_rel(p).ok().filter(|s| !s.is_empty())
}

// ───────────────────────── tunnel replies ─────────────────────────

/// The reply PUT body; drop it to abort (→ `cancel` frame + 499 to the phone).
pub struct TunnelBody {
    rx: mpsc::Receiver<Result<Bytes, std::io::Error>>,
}

impl TunnelBody {
    /// an empty body (tests, synthesized replies)
    pub fn empty() -> Self {
        let (_tx, rx) = mpsc::channel(1);
        Self { rx }
    }
}

impl Stream for TunnelBody {
    type Item = Result<Bytes, std::io::Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

/// A reply PUT as the consumer sees it.
pub struct TunnelReply {
    /// `X-Peek-Status` (200 / 206 / 416 …)
    pub status: u16,
    /// reply PUT headers, lower-cased names
    pub headers: HashMap<String, String>,
    pub body: TunnelBody,
}

impl TunnelReply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

enum Reply {
    Tunnel(TunnelReply),
    Res(Value),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ReqState {
    Queued,
    Sent,
    Streaming,
    Done,
}

struct Req {
    rid: String,
    cid: u64,
    device_id: String,
    op: String,
    args: Value,
    prio: Prio,
    state: ReqState,
    timer: Option<tokio::task::AbortHandle>,
    first_byte: Duration,
    reply: Option<oneshot::Sender<Result<Reply, HttpError>>>,
    /// streaming: fail() destroys the body with this error
    kill: CancellationToken,
    kill_err: Arc<Mutex<Option<HttpError>>>,
    /// the consumer's signal (also after the reply resolved)
    signal: Option<CancellationToken>,
}

struct Conn {
    cid: u64,
    id: String,
    out: mpsc::UnboundedSender<WsOut>,
    open: bool,
    name: String,
    model: String,
    #[allow(dead_code)]
    app: String,
    caps: DeviceCaps,
    folders: Vec<HelloFolder>,
    paired: bool,
    code: Option<String>,
    lan: bool,
    queue: Vec<String>,
    inflight: Vec<String>,
    last_seen: Instant,
    last_seen_ms: i64,
    closed: bool,
    thumb_running: bool,
    changed: HashMap<String, BTreeSet<String>>,
    changed_timer: Option<tokio::task::AbortHandle>,
}

impl Conn {
    fn send(&mut self, frame: Value) {
        if self.open {
            let _ = self.out.send(WsOut::Text(frame.to_string()));
        }
    }
    fn close(&mut self, code: u16, reason: &str) {
        if self.open {
            self.open = false;
            let _ = self.out.send(WsOut::Close(code, reason.to_string()));
        }
    }
}

/// A one-time PC → phone file (`receive` op).
#[derive(Clone)]
pub struct Pull {
    pub device_id: String,
    pub abs: std::path::PathBuf,
    pub size: u64,
    pub mtime: i64,
    pub expires: i64,
    pub hooks: PullHooks,
}

/// Progress hooks of a pull (transfer job).
#[derive(Clone, Default)]
pub struct PullHooks {
    /// the GET started streaming: abort it with the token (job cancel)
    pub on_start: Option<Arc<dyn Fn(CancellationToken) + Send + Sync>>,
    pub on_bytes: Option<Arc<dyn Fn(u64) + Send + Sync>>,
    pub on_end: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// A de-duplicated listing (`relist`); it runs to the end even when dropped.
pub type RelistFuture = Shared<BoxFuture<'static, Result<(), HttpError>>>;

struct State {
    /// every connection (paired and pending) by connection id
    all: HashMap<u64, Conn>,
    conns: HashMap<String, u64>,
    pending: HashMap<String, u64>,
    reqs: HashMap<String, Req>,
    pulls: HashMap<String, Pull>,
    folder_owners: HashMap<i64, String>,
    closed: bool,
    listing: HashMap<(i64, String), RelistFuture>,
}

/// The `details` op reply.
#[derive(Debug, Clone, PartialEq, Default, serde::Deserialize)]
pub struct DeviceDetails {
    #[serde(default)]
    pub exif: Option<Map<String, Value>>,
    #[serde(default)]
    pub gps: Option<Value>,
    #[serde(default)]
    pub make: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub lens: Option<String>,
}

/// `lib.deviceLookup(id)`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceLookup {
    pub id: String,
    pub name: String,
    pub online: bool,
}

/// What a hub needs from the server.
#[derive(Clone)]
pub struct HubDeps {
    pub store: Arc<dyn DeviceStore>,
    pub rows: Arc<dyn MediaRowSink>,
    pub thumbs: Arc<dyn ThumbCache>,
    pub events: Arc<dyn EventSink>,
    /// `os.hostname()` (welcome frame)
    pub server_name: String,
}

/// The phone link hub (`ctx.devices`).
pub struct DeviceHub {
    st: Mutex<State>,
    deps: HubDeps,
    next_cid: AtomicU64,
    pub timeouts: Timeouts,
    /// lets the server feed ffmpeg from its own /file endpoint (requests carrying it bypass visibility)
    pub internal_token: String,
    /// loopback base URL of the main server (set once listening)
    self_url: Mutex<String>,
    me: Weak<DeviceHub>,
    loops: tokio_util::task::TaskTracker,
    heartbeat: Mutex<Option<tokio::task::AbortHandle>>,
}

/// The socket side of one connection: feed it what the socket receives.
pub struct SocketHandle {
    hub: Arc<DeviceHub>,
    meta: ConnectMeta,
    out: mpsc::UnboundedSender<WsOut>,
    cid: Mutex<Option<u64>>,
    /// the hello failed (socket being closed)
    dead: std::sync::atomic::AtomicBool,
    hello_timer: CancellationToken,
}

/// Request options.
#[derive(Clone, Default)]
pub struct ReqOpts {
    pub prio: Prio,
    /// consumer signal (client gone, job canceled); dropping the request future also aborts
    pub signal: Option<CancellationToken>,
    pub first_byte: Option<Duration>,
}

impl ReqOpts {
    pub fn prio(prio: Prio) -> Self {
        Self {
            prio,
            ..Default::default()
        }
    }
    pub fn signal(mut self, s: CancellationToken) -> Self {
        self.signal = Some(s);
        self
    }
}

impl DeviceHub {
    /// `new DeviceHub(ctx)`; starts the 25 s heartbeat (needs a tokio runtime).
    pub fn new(deps: HubDeps) -> Arc<Self> {
        let hub = Arc::new_cyclic(|me| DeviceHub {
            st: Mutex::new(State {
                all: HashMap::new(),
                conns: HashMap::new(),
                pending: HashMap::new(),
                reqs: HashMap::new(),
                pulls: HashMap::new(),
                folder_owners: HashMap::new(),
                closed: false,
                listing: HashMap::new(),
            }),
            deps,
            next_cid: AtomicU64::new(1),
            timeouts: Timeouts {
                short_ms: AtomicU64::new(20_000),
                file_ms: AtomicU64::new(30_000),
            },
            internal_token: random_b64(24),
            self_url: Mutex::new(String::new()),
            me: me.clone(),
            loops: tokio_util::task::TaskTracker::new(),
            heartbeat: Mutex::new(None),
        });
        hub.refresh_owners();
        let weak = Arc::downgrade(&hub);
        let h = tokio::spawn(async move {
            let mut iv = tokio::time::interval(PING);
            iv.tick().await;
            loop {
                iv.tick().await;
                let Some(h) = weak.upgrade() else { return };
                h.beat();
            }
        });
        *hub.heartbeat.lock().unwrap() = Some(h.abort_handle());
        hub
    }

    fn arc(&self) -> Arc<Self> {
        self.me.upgrade().expect("hub alive")
    }

    pub fn self_url(&self) -> String {
        self.self_url.lock().unwrap().clone()
    }
    /// loopback base URL of the main server (`http://127.0.0.1:<port>`), set once listening
    pub fn set_self_url(&self, url: impl Into<String>) {
        *self.self_url.lock().unwrap() = url.into();
    }

    // ───────────────────────── state / listing ─────────────────────────

    pub fn is_online(&self, device_id: &str) -> bool {
        self.st.lock().unwrap().conns.contains_key(device_id)
    }

    pub fn has_folders(&self) -> bool {
        !self.st.lock().unwrap().folder_owners.is_empty()
    }

    pub fn folder_owner(&self, folder_id: i64) -> Option<String> {
        self.st
            .lock()
            .unwrap()
            .folder_owners
            .get(&folder_id)
            .cloned()
    }

    fn refresh_owners(&self) {
        let all = self.deps.store.all_device_folders();
        let mut st = self.st.lock().unwrap();
        st.folder_owners = all.into_iter().collect();
    }

    /// `lib.deviceLookup(id)` for FolderRef.device
    pub fn device_lookup(&self, id: &str) -> DeviceLookup {
        let live = {
            let st = self.st.lock().unwrap();
            st.conns
                .get(id)
                .and_then(|c| st.all.get(c))
                .map(|c| c.name.clone())
        };
        let online = live.is_some();
        let name = live
            .or_else(|| self.deps.store.device(id).map(|d| d.name))
            .unwrap_or_else(|| id.to_string());
        DeviceLookup {
            id: id.to_string(),
            name,
            online,
        }
    }

    /// Paired (remembered) + pending devices, except `hide` (the asking phone itself).
    pub fn list(&self, hide: Option<&str>) -> Vec<DeviceInfo> {
        let paired = self.deps.store.paired_devices();
        let mut out = Vec::new();
        for d in paired {
            if Some(d.id.as_str()) == hide {
                continue;
            }
            let live = {
                let st = self.st.lock().unwrap();
                st.conns
                    .get(&d.id)
                    .and_then(|c| st.all.get(c))
                    .map(|c| (c.name.clone(), c.model.clone(), c.caps))
            };
            let folders = self.deps.store.folder_refs(&d.id);
            out.push(DeviceInfo {
                id: d.id.clone(),
                name: live.as_ref().map(|l| l.0.clone()).unwrap_or(d.name.clone()),
                model: live
                    .as_ref()
                    .map(|l| l.1.clone())
                    .unwrap_or(d.model.clone()),
                online: live.is_some(),
                paired: true,
                last_seen: if live.is_some() {
                    now_ms()
                } else {
                    d.last_seen
                },
                folders,
                code: None,
                caps: Some(
                    live.map(|l| l.2)
                        .unwrap_or_else(|| parse_caps(d.caps.as_deref())),
                ),
            });
        }
        let st = self.st.lock().unwrap();
        let mut pend: Vec<&Conn> = st
            .pending
            .values()
            .filter_map(|c| st.all.get(c))
            .filter(|c| Some(c.id.as_str()) != hide)
            .collect();
        // a JS Map iterates in insertion order: connection ids grow monotonically
        pend.sort_by_key(|c| c.cid);
        out.extend(pend.into_iter().map(pending_info));
        out
    }

    fn emit(&self, ev: ServerEvent) {
        self.deps.events.emit(ev);
    }

    fn emit_devices(&self) {
        self.emit(ServerEvent::Devices);
    }

    fn emit_folders(&self) {
        self.emit(ServerEvent::Folders { status: None });
    }

    // ───────────────────────── auth ─────────────────────────

    /// Phone → server HTTP auth (`Authorization: Bearer` + `X-Peek-Device`): 401 unknown device /
    /// bad token, 403 not paired. Returns the device id.
    pub fn authenticate(
        &self,
        x_peek_device: Option<&str>,
        authorization: Option<&str>,
    ) -> Result<String, HttpError> {
        let (Some(id), Some(token)) = (device_header(x_peek_device), bearer_of(authorization))
        else {
            return Err(HttpError::new(401, "Device token required"));
        };
        let row = self.deps.store.device(&id);
        let ok = row
            .as_ref()
            .and_then(|r| r.token_hash.as_deref())
            .is_some_and(|h| safe_eq(h, &sha256_hex(&token)));
        if !ok {
            return Err(HttpError::new(401, "Unknown device or invalid token"));
        }
        if !row.unwrap().paired {
            return Err(HttpError::new(403, "Device is not paired"));
        }
        Ok(id)
    }

    /// non-throwing variant
    pub fn paired_device(
        &self,
        x_peek_device: Option<&str>,
        authorization: Option<&str>,
    ) -> Option<String> {
        self.authenticate(x_peek_device, authorization).ok()
    }

    // ───────────────────────── control socket ─────────────────────────

    /// `WS /api/devices/connect`: a new socket. Pump the returned receiver into the socket and feed
    /// the socket's events to the handle.
    pub fn attach(
        self: &Arc<Self>,
        meta: ConnectMeta,
    ) -> (SocketHandle, mpsc::UnboundedReceiver<WsOut>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let handle = SocketHandle {
            hub: self.clone(),
            meta,
            out: tx.clone(),
            cid: Mutex::new(None),
            dead: false.into(),
            hello_timer: CancellationToken::new(),
        };
        if self.st.lock().unwrap().closed {
            let _ = tx.send(WsOut::Close(1001, "shutting down".into()));
            handle.dead.store(true, Ordering::Relaxed);
            return (handle, rx);
        }
        let t = handle.hello_timer.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = tokio::time::sleep(HELLO_TIMEOUT) => { let _ = tx.send(WsOut::Close(4408, "hello timeout".into())); }
                _ = t.cancelled() => {}
            }
        });
        (handle, rx)
    }

    fn on_hello(&self, h: &SocketHandle, msg: &Value) -> Option<u64> {
        let close = |code: u16, r: &str| {
            let _ = h.out.send(WsOut::Close(code, r.into()));
            None
        };
        if msg.get("t").and_then(Value::as_str) != Some("hello") {
            return close(4400, "expected hello");
        }
        if msg.get("v").and_then(Value::as_f64) != Some(1.0) {
            return close(4426, "unsupported protocol version");
        }
        let id = msg
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if !ID_RE.is_match(&id) || h.meta.header_id.as_deref().is_some_and(|x| x != id) {
            return close(4400, "bad device id");
        }
        let caps = msg.get("caps");
        let cid = self.next_cid.fetch_add(1, Ordering::Relaxed);
        let mut conn = Conn {
            cid,
            id: id.clone(),
            out: h.out.clone(),
            open: true,
            name: clean(msg.get("name"), 80, "Phone"),
            model: clean(msg.get("model"), 80, ""),
            app: clean(msg.get("app"), 20, ""),
            caps: DeviceCaps {
                read: caps.and_then(|c| c.get("read")) != Some(&Value::Bool(false)),
                receive: caps.and_then(|c| c.get("receive")) == Some(&Value::Bool(true)),
            },
            folders: parse_folders(msg.get("folders")),
            paired: false,
            code: None,
            lan: h.meta.lan,
            queue: Vec::new(),
            inflight: Vec::new(),
            last_seen: Instant::now(),
            last_seen_ms: now_ms(),
            closed: false,
            thumb_running: false,
            changed: HashMap::new(),
            changed_timer: None,
        };
        let row = self.deps.store.device(&id);
        let valid = row.as_ref().is_some_and(|r| {
            r.paired
                && r.token_hash
                    .as_deref()
                    .zip(h.meta.token.as_deref())
                    .is_some_and(|(hash, tok)| safe_eq(hash, &sha256_hex(tok)))
        });
        if valid {
            conn.send(json!({ "t": "welcome", "server": self.deps.server_name, "paired": true }));
            self.st.lock().unwrap().all.insert(cid, conn);
            self.go_online(cid);
            return Some(cid);
        }
        // unknown device, no token, or a token we no longer accept (forgotten) → ask the PC user
        let info = {
            let mut st = self.st.lock().unwrap();
            if let Some(old) = st.pending.remove(&id) {
                if let Some(o) = st.all.get_mut(&old) {
                    o.closed = true;
                    o.close(4409, "replaced");
                }
                st.all.remove(&old);
            }
            if st.pending.len() >= MAX_PENDING {
                drop(st);
                return close(4429, "too many pending pairings");
            }
            let code = format!("{:04}", rand::random::<u32>() % 10_000);
            conn.code = Some(code.clone());
            conn.send(json!({ "t": "pending", "code": code }));
            let info = pending_info(&conn);
            st.pending.insert(id, cid);
            st.all.insert(cid, conn);
            info
        };
        self.emit(ServerEvent::Pair { device: info });
        self.emit_devices();
        Some(cid)
    }

    /// `POST /api/devices/:id/approve` (after `require_approver`)
    pub fn approve(&self, id: &str) -> Result<(), HttpError> {
        let token = random_b64(32);
        let (cid, name, model, caps) = {
            let mut st = self.st.lock().unwrap();
            let Some(cid) = st.pending.remove(id) else {
                return Err(HttpError::new(404, "No pending pairing for this device"));
            };
            let c = st.all.get_mut(&cid).expect("pending conn");
            c.code = None;
            (cid, c.name.clone(), c.model.clone(), c.caps)
        };
        self.deps.store.save_paired(
            id,
            &name,
            &model,
            &sha256_hex(&token),
            &caps_json(caps),
            now_ms(),
        );
        if let Some(c) = self.st.lock().unwrap().all.get_mut(&cid) {
            c.send(json!({ "t": "paired", "token": token }));
        }
        self.go_online(cid);
        Ok(())
    }

    /// `POST /api/devices/:id/reject`
    pub fn reject(&self, id: &str) -> Result<(), HttpError> {
        {
            let mut st = self.st.lock().unwrap();
            let Some(cid) = st.pending.remove(id) else {
                return Err(HttpError::new(404, "No pending pairing for this device"));
            };
            if let Some(c) = st.all.get_mut(&cid) {
                c.closed = true;
                c.send(json!({ "t": "rejected" }));
                c.close(4403, "rejected");
            }
            st.all.remove(&cid);
        }
        self.emit_devices();
        Ok(())
    }

    /// `DELETE /api/devices/:id`: unpair, drop its folders and their media rows.
    pub fn forget(&self, id: &str) -> Result<(), HttpError> {
        let known = self.deps.store.device(id).is_some();
        let pending = self.st.lock().unwrap().pending.contains_key(id);
        if !known && !pending {
            return Err(HttpError::new(404, "Unknown device"));
        }
        if pending {
            let _ = self.reject(id);
        }
        let cid = self.st.lock().unwrap().conns.get(id).copied();
        if let Some(cid) = cid {
            self.detach(cid);
            if let Some(c) = self.st.lock().unwrap().all.get_mut(&cid) {
                c.close(4401, "forgotten");
            }
        }
        self.deps.store.delete_device(id);
        self.refresh_owners();
        self.deps.events.bump();
        self.emit_folders();
        self.emit_devices();
        Ok(())
    }

    fn go_online(&self, cid: u64) {
        let (id, name, model, caps, prev) = {
            let mut st = self.st.lock().unwrap();
            let Some(c) = st.all.get_mut(&cid) else {
                return;
            };
            c.paired = true;
            let (id, name, model, caps) = (c.id.clone(), c.name.clone(), c.model.clone(), c.caps);
            let prev = st.conns.get(&id).copied().filter(|p| *p != cid);
            (id, name, model, caps, prev)
        };
        if let Some(p) = prev {
            self.detach(p);
            let mut st = self.st.lock().unwrap();
            if let Some(c) = st.all.get_mut(&p) {
                c.close(4409, "replaced");
            }
            st.all.remove(&p);
        }
        self.st.lock().unwrap().conns.insert(id.clone(), cid);
        self.deps
            .store
            .update_online(&id, &name, &model, &caps_json(caps), now_ms());
        let rows = self.sync_folders(cid);
        self.emit_devices();
        self.emit_folders();
        if caps.read {
            let me = self.arc();
            for r in rows {
                let me = me.clone();
                tokio::spawn(async move {
                    let _ = me.relist(r.id, "").await;
                });
            }
        }
    }

    /// drop a connection (socket closed, idle, replaced, forgotten, shutdown)
    fn detach(&self, cid: u64) {
        let (fail, emit_folders) = {
            let mut st = self.st.lock().unwrap();
            let Some(c) = st.all.get(&cid) else { return };
            let id = c.id.clone();
            let in_conns = st.conns.get(&id) == Some(&cid);
            let in_pending = st.pending.get(&id) == Some(&cid);
            if c.closed && !in_conns && !in_pending {
                return;
            }
            let c = st.all.get_mut(&cid).unwrap();
            c.closed = true;
            if let Some(t) = c.changed_timer.take() {
                t.abort();
            }
            if in_pending {
                st.pending.remove(&id);
                drop(st);
                self.emit_devices();
                return;
            }
            if !in_conns {
                return;
            }
            st.conns.remove(&id);
            let c = st.all.get_mut(&cid).unwrap();
            let mut rids: Vec<String> = std::mem::take(&mut c.queue);
            rids.extend(c.inflight.iter().cloned());
            st.pulls.retain(|_, p| p.device_id != id);
            ((id, rids), true)
        };
        let (id, rids) = fail;
        self.deps.store.set_last_seen(&id, now_ms());
        for rid in rids {
            self.fail(&rid, offline(), false);
        }
        self.emit_devices();
        if emit_folders {
            self.emit_folders();
        }
    }

    fn beat(&self) {
        let now = Instant::now();
        let mut idle = Vec::new();
        {
            let mut st = self.st.lock().unwrap();
            let ids: Vec<u64> = st
                .conns
                .values()
                .chain(st.pending.values())
                .copied()
                .collect();
            for cid in ids {
                let Some(c) = st.all.get_mut(&cid) else {
                    continue;
                };
                if now.duration_since(c.last_seen) > IDLE {
                    let _ = c.out.send(WsOut::Terminate);
                    c.open = false;
                    idle.push(cid);
                } else if c.open {
                    let _ = c.out.send(WsOut::Ping);
                }
            }
            let t = now_ms();
            st.pulls.retain(|_, p| p.expires >= t);
        }
        for cid in idle {
            self.detach(cid);
        }
    }

    fn touch(&self, cid: u64) {
        if let Some(c) = self.st.lock().unwrap().all.get_mut(&cid) {
            c.last_seen = Instant::now();
            c.last_seen_ms = now_ms();
        }
    }

    fn on_frame(&self, cid: u64, msg: &Value) {
        let t = msg.get("t").and_then(Value::as_str).unwrap_or("");
        let paired = {
            let mut st = self.st.lock().unwrap();
            let Some(c) = st.all.get_mut(&cid) else {
                return;
            };
            match t {
                "ping" => {
                    c.send(json!({ "t": "pong" }));
                    return;
                }
                "pong" => return,
                _ => {}
            }
            c.paired
        };
        if !paired {
            return;
        }
        match t {
            "folders" => {
                let id = {
                    let mut st = self.st.lock().unwrap();
                    let Some(c) = st.all.get_mut(&cid) else {
                        return;
                    };
                    c.folders = parse_folders(msg.get("folders"));
                    c.id.clone()
                };
                let before: HashSet<i64> = self
                    .deps
                    .store
                    .device_folders(&id)
                    .iter()
                    .map(|r| r.id)
                    .collect();
                let rows = self.sync_folders(cid);
                self.emit_folders();
                self.emit_devices();
                let read = self
                    .st
                    .lock()
                    .unwrap()
                    .all
                    .get(&cid)
                    .is_some_and(|c| c.caps.read);
                if read {
                    for r in rows.into_iter().filter(|r| !before.contains(&r.id)) {
                        let me = self.arc();
                        tokio::spawn(async move {
                            let _ = me.relist(r.id, "").await;
                        });
                    }
                }
            }
            "changed" => {
                let key = msg
                    .get("key")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let mut st = self.st.lock().unwrap();
                let Some(c) = st.all.get_mut(&cid) else {
                    return;
                };
                if !c.folders.iter().any(|f| f.key == key) {
                    return;
                }
                let set = c.changed.entry(key).or_default();
                let dirs: Vec<Value> = match msg.get("dirs") {
                    Some(Value::Array(a)) => a.iter().take(256).cloned().collect(),
                    _ => vec![Value::String(String::new())],
                };
                for d in dirs {
                    if let Ok(n) = normalize_rel(&pa_core::js::to_string_of(&d)) {
                        set.insert(n);
                    }
                }
                if c.changed_timer.is_none() {
                    let me = self.arc();
                    let h = tokio::spawn(async move {
                        tokio::time::sleep(CHANGED_DEBOUNCE).await;
                        me.flush_changed(cid);
                    });
                    c.changed_timer = Some(h.abort_handle());
                }
            }
            "err" => {
                let rid = msg
                    .get("rid")
                    .map(pa_core::js::to_string_of)
                    .unwrap_or_default();
                let ok = {
                    let st = self.st.lock().unwrap();
                    st.reqs.get(&rid).is_some_and(|r| {
                        r.cid == cid && !matches!(r.state, ReqState::Streaming | ReqState::Done)
                    })
                };
                if !ok {
                    return;
                }
                let mut status = msg.get("status").map(pa_core::js::number_of).unwrap_or(0.0);
                if status.is_nan() || status == 0.0 {
                    status = 500.0;
                }
                let s = status as i64;
                let m = clean(
                    msg.get("message"),
                    200,
                    &format!("device error {}", pa_core::js::num_to_string(status)),
                );
                self.fail(&rid, HttpError::new(consumer_status(s), m), false);
            }
            "res" => {
                let rid = msg
                    .get("rid")
                    .map(pa_core::js::to_string_of)
                    .unwrap_or_default();
                let tx = {
                    let mut st = self.st.lock().unwrap();
                    let Some(r) = st.reqs.get_mut(&rid) else {
                        return;
                    };
                    if r.cid != cid || r.op != "receive" || r.state == ReqState::Done {
                        return;
                    }
                    r.reply.take()
                };
                self.finish_req(&rid);
                if let Some(tx) = tx {
                    if msg.get("ok") == Some(&Value::Bool(false)) {
                        let mut status =
                            msg.get("status").map(pa_core::js::number_of).unwrap_or(0.0);
                        if status.is_nan() || status == 0.0 {
                            status = 500.0;
                        }
                        let _ = tx.send(Err(HttpError::new(
                            consumer_status(status as i64),
                            clean(msg.get("message"), 200, "device error"),
                        )));
                    } else {
                        let data = match msg.get("data") {
                            None | Some(Value::Null) => json!({}),
                            Some(d) => d.clone(),
                        };
                        let _ = tx.send(Ok(Reply::Res(data)));
                    }
                }
            }
            _ => {}
        }
    }

    fn flush_changed(&self, cid: u64) {
        let (id, work) = {
            let mut st = self.st.lock().unwrap();
            if st.closed {
                return;
            }
            let Some(c) = st.all.get_mut(&cid) else {
                return;
            };
            c.changed_timer = None;
            (c.id.clone(), std::mem::take(&mut c.changed))
        };
        let rows = self.deps.store.device_folders(&id);
        for (k, dirs) in work {
            if let Some(row) = rows.iter().find(|r| r.device_key == k) {
                let dirs: Vec<String> = dirs.into_iter().collect();
                for d in collapse_dirs(&dirs) {
                    let me = self.arc();
                    let fid = row.id;
                    tokio::spawn(async move {
                        let _ = me.relist(fid, &d).await;
                    });
                }
            }
        }
    }

    // ───────────────────────── folders ─────────────────────────

    /// hello / folders → one folders row per shared folder; vanished ones are dropped (cascade).
    fn sync_folders(&self, cid: u64) -> Vec<DeviceFolderRow> {
        let (id, folders) = {
            let st = self.st.lock().unwrap();
            let Some(c) = st.all.get(&cid) else {
                return Vec::new();
            };
            (c.id.clone(), c.folders.clone())
        };
        let existing = self.deps.store.device_folders(&id);
        let mut keep = HashSet::new();
        let mut update = Vec::new();
        let mut insert = Vec::new();
        for (i, f) in folders.iter().enumerate() {
            if let Some(cur) = existing.iter().find(|r| r.device_key == f.key) {
                keep.insert(cur.id);
                if cur.name != f.name || cur.sort != i as i64 {
                    update.push((cur.id, f.name.clone(), i as i64));
                }
            } else {
                insert.push((f.key.clone(), f.name.clone(), i as i64));
            }
        }
        let delete: Vec<i64> = existing
            .iter()
            .filter(|r| !keep.contains(&r.id))
            .map(|r| r.id)
            .collect();
        self.deps
            .store
            .sync_device_folders(&id, &update, &insert, &delete);
        if !delete.is_empty() {
            self.deps.events.bump();
        }
        self.refresh_owners();
        self.deps.store.device_folders(&id)
    }

    // ───────────────────────── requests ─────────────────────────

    fn new_req(
        &self,
        device_id: &str,
        op: &str,
        args: Value,
        opts: &ReqOpts,
    ) -> Result<(String, oneshot::Receiver<Result<Reply, HttpError>>), HttpError> {
        if opts.signal.as_ref().is_some_and(|s| s.is_cancelled()) {
            return Err(aborted());
        }
        let (tx, rx) = oneshot::channel();
        let rid = random_b64(16);
        {
            let mut st = self.st.lock().unwrap();
            let Some(cid) = st.conns.get(device_id).copied() else {
                return Err(offline());
            };
            let first_byte = opts.first_byte.unwrap_or_else(|| {
                Duration::from_millis(if op == "file" || op == "receive" {
                    self.timeouts.file_ms.load(Ordering::Relaxed)
                } else {
                    self.timeouts.short_ms.load(Ordering::Relaxed)
                })
            });
            st.reqs.insert(
                rid.clone(),
                Req {
                    rid: rid.clone(),
                    cid,
                    device_id: device_id.to_string(),
                    op: op.to_string(),
                    args,
                    prio: opts.prio,
                    state: ReqState::Queued,
                    timer: None,
                    first_byte,
                    reply: Some(tx),
                    kill: CancellationToken::new(),
                    kill_err: Arc::new(Mutex::new(None)),
                    signal: opts.signal.clone(),
                },
            );
            // stable insert by priority
            let prio = opts.prio;
            let State { all, reqs, .. } = &mut *st;
            let c = all.get_mut(&cid).expect("online conn");
            let mut i = c.queue.len();
            while i > 0 && reqs.get(&c.queue[i - 1]).is_some_and(|q| q.prio > prio) {
                i -= 1;
            }
            c.queue.insert(i, rid.clone());
        }
        self.pump_conn(device_id);
        Ok((rid, rx))
    }

    async fn await_reply(
        &self,
        rid: String,
        rx: oneshot::Receiver<Result<Reply, HttpError>>,
        signal: Option<CancellationToken>,
    ) -> Result<Reply, HttpError> {
        struct Guard<'a>(&'a DeviceHub, Option<String>);
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                if let Some(rid) = self.1.take() {
                    self.0.fail(&rid, aborted(), true);
                }
            }
        }
        let mut g = Guard(self, Some(rid.clone()));
        let sig = signal.unwrap_or_default();
        let r = tokio::select! {
            r = rx => r.unwrap_or_else(|_| Err(aborted())),
            _ = sig.cancelled() => { self.fail(&rid, aborted(), true); Err(aborted()) }
        };
        g.1 = None;
        r
    }

    /// Send a request to a phone and resolve with its reply PUT. Errors: 503 offline, 504 no
    /// first byte in time, the phone's err status, or [`aborted`]. Dropping the future aborts.
    pub async fn request(
        &self,
        device_id: &str,
        op: &str,
        args: Value,
        opts: ReqOpts,
    ) -> Result<TunnelReply, HttpError> {
        let (rid, rx) = self.new_req(device_id, op, args, &opts)?;
        match self.await_reply(rid, rx, opts.signal).await? {
            Reply::Tunnel(t) => Ok(t),
            Reply::Res(_) => Err(HttpError::new(502, "unexpected device reply")),
        }
    }

    /// `receive` op: resolves with the `res` frame's data (`{ p }`).
    pub async fn receive(
        &self,
        device_id: &str,
        args: Value,
        opts: ReqOpts,
    ) -> Result<Value, HttpError> {
        let (rid, rx) = self.new_req(device_id, "receive", args, &opts)?;
        match self.await_reply(rid, rx, opts.signal).await? {
            Reply::Res(v) => Ok(v),
            Reply::Tunnel(_) => Err(HttpError::new(502, "unexpected device reply")),
        }
    }

    /// in-flight / queued counts (tests, diagnostics)
    pub fn stats(&self, device_id: &str) -> (usize, usize) {
        let st = self.st.lock().unwrap();
        st.conns
            .get(device_id)
            .and_then(|c| st.all.get(c))
            .map(|c| (c.inflight.len(), c.queue.len()))
            .unwrap_or((0, 0))
    }

    fn pump_conn(&self, device_id: &str) {
        let cid = self.st.lock().unwrap().conns.get(device_id).copied();
        if let Some(cid) = cid {
            self.pump(cid);
        }
    }

    fn pump(&self, cid: u64) {
        let mut st = self.st.lock().unwrap();
        let State { all, reqs, .. } = &mut *st;
        let Some(c) = all.get_mut(&cid) else { return };
        while c.inflight.len() < MAX_INFLIGHT && !c.queue.is_empty() && !c.closed {
            let low = c
                .inflight
                .iter()
                .filter(|r| reqs.get(*r).is_some_and(|x| x.prio == 2))
                .count();
            let Some(idx) = c.queue.iter().position(|r| {
                reqs.get(r)
                    .is_some_and(|x| x.prio < 2 || low < MAX_INFLIGHT_LOW)
            }) else {
                break;
            };
            let rid = c.queue.remove(idx);
            let Some(r) = reqs.get_mut(&rid) else {
                continue;
            };
            r.state = ReqState::Sent;
            c.inflight.push(rid.clone());
            let me = self.arc();
            let rid2 = rid.clone();
            let fb = r.first_byte;
            let h = tokio::spawn(async move {
                tokio::time::sleep(fb).await;
                me.fail(
                    &rid2,
                    HttpError::new(504, "The device did not answer in time"),
                    true,
                );
            });
            r.timer = Some(h.abort_handle());
            c.send(json!({ "t": "req", "rid": rid, "op": r.op, "args": r.args }));
        }
    }

    fn finish_req(&self, rid: &str) {
        let cid = {
            let mut st = self.st.lock().unwrap();
            let Some(r) = st.reqs.get_mut(rid) else {
                return;
            };
            if r.state == ReqState::Done {
                return;
            }
            r.state = ReqState::Done;
            if let Some(t) = r.timer.take() {
                t.abort();
            }
            let cid = r.cid;
            st.reqs.remove(rid);
            let Some(c) = st.all.get_mut(&cid) else {
                return;
            };
            c.queue.retain(|q| q != rid);
            let before = c.inflight.len();
            c.inflight.retain(|q| q != rid);
            (before != c.inflight.len()).then_some(cid)
        };
        if let Some(cid) = cid {
            self.pump(cid);
        }
    }

    fn fail(&self, rid: &str, err: HttpError, cancel: bool) {
        let tx = {
            let mut st = self.st.lock().unwrap();
            let State { all, reqs, .. } = &mut *st;
            let Some(r) = reqs.get_mut(rid) else { return };
            if r.state == ReqState::Done {
                return;
            }
            let was_sent = matches!(r.state, ReqState::Sent | ReqState::Streaming);
            if cancel && was_sent {
                if let Some(c) = all.get_mut(&r.cid) {
                    c.send(json!({ "t": "cancel", "rid": rid }));
                }
            }
            if r.state == ReqState::Streaming {
                *r.kill_err.lock().unwrap() = Some(err.clone());
                r.kill.cancel();
            }
            r.reply.take()
        };
        self.finish_req(rid);
        if let Some(tx) = tx {
            let _ = tx.send(Err(err));
        }
    }

    /// `PUT /api/devices/reply/:rid` (after [`DeviceHub::authenticate`]): pipe the body into the
    /// consumer. Returns the HTTP status for the PUT: 204 consumer finished, 499 consumer went away
    /// (answer with `Connection: close` and drop the connection), 410 unknown / expired rid.
    pub async fn accept_reply<S, E>(
        &self,
        rid: &str,
        device_id: &str,
        headers: Vec<(String, String)>,
        mut body: S,
    ) -> u16
    where
        S: Stream<Item = Result<Bytes, E>> + Unpin + Send,
        E: std::fmt::Display,
    {
        if !RID_RE.is_match(rid) {
            return 410;
        }
        let (tx_body, rx_body) = mpsc::channel(PIPE_CHUNKS);
        let (reply, kill, kill_err, signal, cid) = {
            let mut st = self.st.lock().unwrap();
            let Some(r) = st.reqs.get_mut(rid) else {
                return 410;
            };
            if r.device_id != device_id || r.state != ReqState::Sent || r.op == "receive" {
                return 410;
            }
            if let Some(t) = r.timer.take() {
                t.abort();
            }
            r.state = ReqState::Streaming;
            (
                r.reply.take(),
                r.kill.clone(),
                r.kill_err.clone(),
                r.signal.clone(),
                r.cid,
            )
        };
        let hmap: HashMap<String, String> = headers
            .into_iter()
            .map(|(k, v)| (k.to_lowercase(), v))
            .collect();
        let status = hmap
            .get("x-peek-status")
            .map(|s| pa_core::js::number(s))
            .filter(|n| *n != 0.0 && !n.is_nan())
            .map(|n| n as u16)
            .unwrap_or(200);
        let cancel_frame = |hub: &DeviceHub| {
            if let Some(c) = hub.st.lock().unwrap().all.get_mut(&cid) {
                c.send(json!({ "t": "cancel", "rid": rid }));
            }
        };
        let tr = TunnelReply {
            status,
            headers: hmap,
            body: TunnelBody { rx: rx_body },
        };
        let delivered = reply.is_some_and(|tx| tx.send(Ok(Reply::Tunnel(tr))).is_ok());
        if !delivered {
            // the consumer went away while the PUT was being opened
            cancel_frame(self);
            self.finish_req(rid);
            return 499;
        }
        let sig = signal.unwrap_or_default();
        loop {
            let next = tokio::select! {
                _ = kill.cancelled() => {
                    let e = kill_err.lock().unwrap().take().unwrap_or_else(aborted);
                    let _ = tx_body.try_send(Err(std::io::Error::other(e.message)));
                    return 499;
                }
                _ = tx_body.closed() => None,
                _ = sig.cancelled() => None,
                c = body.next() => Some(c),
            };
            let Some(c) = next else {
                // consumer went away before the phone finished sending
                cancel_frame(self);
                self.finish_req(rid);
                return 499;
            };
            match c {
                None => {
                    self.finish_req(rid);
                    return 204;
                }
                Some(Err(_)) => {
                    let _ = tx_body.try_send(Err(std::io::Error::other(
                        "The device closed the connection",
                    )));
                    self.finish_req(rid);
                    return 499;
                }
                Some(Ok(chunk)) => {
                    let sent = tokio::select! {
                        r = tx_body.send(Ok(chunk)) => r.is_ok(),
                        _ = kill.cancelled() => {
                            let e = kill_err.lock().unwrap().take().unwrap_or_else(aborted);
                            let _ = tx_body.try_send(Err(std::io::Error::other(e.message)));
                            return 499;
                        }
                        _ = sig.cancelled() => false,
                    };
                    if !sent {
                        cancel_frame(self);
                        self.finish_req(rid);
                        return 499;
                    }
                }
            }
        }
    }

    /// request + read the whole (small) body
    pub async fn fetch_buffer(
        &self,
        device_id: &str,
        op: &str,
        args: Value,
        prio: Prio,
        max: usize,
        signal: Option<CancellationToken>,
    ) -> Result<(u16, HashMap<String, String>, Vec<u8>), HttpError> {
        let mut tr = self
            .request(
                device_id,
                op,
                args,
                ReqOpts {
                    prio,
                    signal,
                    first_byte: None,
                },
            )
            .await?;
        let mut data = Vec::new();
        while let Some(c) = tr.body.next().await {
            let c = c.map_err(|e| HttpError::new(502, e.to_string()))?;
            if data.len() + c.len() > max {
                return Err(HttpError::new(413, "Device reply too large"));
            }
            data.extend_from_slice(&c);
        }
        Ok((tr.status, tr.headers, data))
    }

    // ───────────────────────── device paths ─────────────────────────

    /// `device://<id>/<key>/<rel>` → (device id, key, rel); 400 for anything else
    pub fn parse_abs(abs: &str) -> Result<(String, String, String), HttpError> {
        let c = ABS_RE
            .captures(abs)
            .ok_or_else(|| HttpError::bad("Not a device path"))?;
        Ok((c[1].to_string(), c[2].to_string(), normalize_rel(&c[3])?))
    }

    pub fn caps(&self, device_id: &str) -> Option<DeviceCaps> {
        let st = self.st.lock().unwrap();
        st.conns
            .get(device_id)
            .and_then(|c| st.all.get(c))
            .map(|c| c.caps)
    }

    /// RemoteImageSource.thumb (ThumbService): `high` = an on-demand thumbnail (prio 0)
    pub async fn thumb(&self, abs: &str, size: u32, high: bool) -> Result<Vec<u8>, HttpError> {
        let (d, key, rel) = Self::parse_abs(abs)?;
        let (status, _, data) = self
            .fetch_buffer(
                &d,
                "thumb",
                json!({ "key": key, "p": rel, "s": size }),
                if high { 0 } else { 2 },
                16 * 1024 * 1024,
                None,
            )
            .await?;
        if status != 200 || data.is_empty() {
            return Err(HttpError::new(502, "No thumbnail from the device"));
        }
        Ok(data)
    }

    /// RemoteImageSource.display
    pub async fn display(&self, abs: &str, max: u32) -> Result<Vec<u8>, HttpError> {
        let (d, key, rel) = Self::parse_abs(abs)?;
        let (status, _, data) = self
            .fetch_buffer(
                &d,
                "display",
                json!({ "key": key, "p": rel, "max": max }),
                1,
                64 * 1024 * 1024,
                None,
            )
            .await?;
        if status != 200 || data.is_empty() {
            return Err(HttpError::new(502, "No image from the device"));
        }
        Ok(data)
    }

    /// RemoteImageSource.strip
    pub async fn strip(&self, abs: &str, n: u32, h: u32) -> Result<Vec<u8>, HttpError> {
        let (d, key, rel) = Self::parse_abs(abs)?;
        let (status, _, data) = self
            .fetch_buffer(
                &d,
                "strip",
                json!({ "key": key, "p": rel, "n": n, "h": h }),
                1,
                64 * 1024 * 1024,
                None,
            )
            .await?;
        if status != 200 || data.is_empty() {
            return Err(HttpError::new(502, "No filmstrip from the device"));
        }
        Ok(data)
    }

    // ───────────────────────── listing → media rows ─────────────────────────

    /// (Re)list a device folder (or a sub tree of it) into media rows. No-op while offline.
    /// Concurrent calls for the same (folder, dir) share one listing.
    pub fn relist(&self, folder_id: i64, sub_dir: &str) -> RelistFuture {
        let key = (folder_id, sub_dir.to_string());
        let mut st = self.st.lock().unwrap();
        if st.closed {
            return futures_util::future::ready(Ok(())).boxed().shared();
        }
        if let Some(f) = st.listing.get(&key) {
            return f.clone();
        }
        let me = self.arc();
        let k2 = key.clone();
        let sub = sub_dir.to_string();
        let fut = async move {
            let r = me.do_list(folder_id, &sub).await;
            me.st.lock().unwrap().listing.remove(&k2);
            r
        }
        .boxed()
        .shared();
        st.listing.insert(key, fut.clone());
        // run it even when nobody awaits it (fire-and-forget callers)
        let f2 = fut.clone();
        self.loops.spawn(async move {
            let _ = f2.await;
        });
        fut
    }

    async fn do_list(&self, folder_id: i64, sub_dir: &str) -> Result<(), HttpError> {
        let Some(folder) = self.deps.store.device_folder(folder_id) else {
            return Ok(());
        };
        let (cid, read) = {
            let st = self.st.lock().unwrap();
            match st.conns.get(&folder.device_id).and_then(|c| st.all.get(c)) {
                Some(c) => (c.cid, c.caps.read),
                None => return Ok(()),
            }
        };
        if !read {
            return Ok(());
        }
        let dir = normalize_rel(sub_dir)?;
        let tr = match self
            .request(
                &folder.device_id,
                "list",
                json!({ "key": folder.device_key, "dir": dir, "recursive": true }),
                ReqOpts::prio(2),
            )
            .await
        {
            Ok(t) => t,
            // a reported directory that no longer exists on the phone → forget it
            Err(e) if !dir.is_empty() && e.status == 404 => TunnelReply {
                status: 200,
                headers: HashMap::new(),
                body: TunnelBody::empty(),
            },
            Err(e) => return Err(e),
        };
        if self.is_closed() {
            return Ok(());
        }
        if tr.status != 200 {
            return Err(HttpError::new(502, format!("list failed ({})", tr.status)));
        }
        let rows = &self.deps.rows;
        let scope = ListScope {
            folder_id,
            dir: dir.clone(),
        };
        let in_scope = |p: &str| dir.is_empty() || p == dir || p.starts_with(&format!("{dir}/"));
        let max_before = rows.list_begin(&scope);
        let mut dirs = HashSet::new();
        let mut changed = 0usize;
        let mut batch: Vec<ListedFile> = Vec::new();
        let mut lines = NdjsonLines::new(tr.body, 1024 * 1024);
        let res: Result<(), HttpError> = async {
            while let Some(line) = lines.next_line().await? {
                let Ok(o) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if !o.is_object() {
                    continue;
                }
                if let (Some(Value::String(d)), None) = (o.get("dir"), o.get("p")) {
                    if let Some(d) = safe_rel(d) {
                        if in_scope(&d) {
                            add_dir_chain(&mut dirs, &d);
                        }
                    }
                    continue;
                }
                let Some(p) = o.get("p").and_then(Value::as_str).and_then(safe_rel) else {
                    continue;
                };
                if !in_scope(&p) {
                    continue;
                }
                let mut segs: Vec<&str> = p.split('/').collect();
                if segs.iter().any(|s| rows.skip_name(s)) {
                    continue;
                }
                let name = segs.pop().unwrap().to_string();
                let d = segs.join("/");
                let Some(kind) = pa_core::kind_of(&name) else {
                    continue;
                };
                if !d.is_empty() {
                    add_dir_chain(&mut dirs, &d);
                }
                batch.push(listed_file(&o, d, name, kind));
                if batch.len() >= 500 {
                    if self.is_closed() {
                        return Err(HttpError::new(503, "shutting down"));
                    }
                    changed += rows.list_upsert(folder_id, &std::mem::take(&mut batch));
                }
            }
            if !batch.is_empty() {
                if self.is_closed() {
                    return Err(HttpError::new(503, "shutting down"));
                }
                changed += rows.list_upsert(folder_id, &std::mem::take(&mut batch));
            }
            Ok(())
        }
        .await;
        if let Err(e) = res {
            if !self.is_closed() {
                rows.list_abort(&scope);
            }
            return Err(e);
        }
        if self.is_closed() {
            return Ok(());
        }
        let mut dirs: Vec<String> = dirs.into_iter().collect();
        dirs.sort();
        let (added, removed) = rows.list_finish(&scope, &dirs, max_before);
        let fid = folder_id.to_string();
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
        self.emit_folders();
        self.start_thumbs(cid);
        Ok(())
    }

    fn is_closed(&self) -> bool {
        self.st.lock().unwrap().closed
    }

    fn start_thumbs(&self, cid: u64) {
        {
            let mut st = self.st.lock().unwrap();
            if st.closed {
                return;
            }
            let Some(c) = st.all.get_mut(&cid) else {
                return;
            };
            if c.thumb_running || c.closed {
                return;
            }
            c.thumb_running = true;
        }
        let me = self.arc();
        self.loops.spawn(async move {
            me.thumb_loop(cid).await;
            if let Some(c) = me.st.lock().unwrap().all.get_mut(&cid) {
                c.thumb_running = false;
            }
        });
    }

    fn conn_live(&self, cid: u64) -> Option<String> {
        let st = self.st.lock().unwrap();
        if st.closed {
            return None;
        }
        let c = st.all.get(&cid)?;
        (!c.closed && st.conns.get(&c.id) == Some(&cid)).then(|| c.id.clone())
    }

    /// Background thumbnails (+ thumbhash) of a device's media while it is online.
    async fn thumb_loop(&self, cid: u64) {
        let mut failed: HashSet<i64> = HashSet::new();
        while let Some(device_id) = self.conn_live(cid) {
            let todo: Vec<_> = self
                .deps
                .thumbs
                .pending_thumbs(ThumbScope::Device(&device_id))
                .into_iter()
                .filter(|r| !failed.contains(&r.id))
                .collect();
            if todo.is_empty() {
                break;
            }
            let offline = std::sync::atomic::AtomicBool::new(false);
            let failed_now = Mutex::new(Vec::new());
            futures_util::stream::iter(todo.iter())
                .for_each_concurrent(2, |r| {
                    let (offline, failed_now) = (&offline, &failed_now);
                    async move {
                        if offline.load(Ordering::Relaxed) {
                            return;
                        }
                        match self.deps.thumbs.generate_base(&r.abs(), r.id).await {
                            Ok(res) => {
                                if self.is_closed() {
                                    return;
                                }
                                let img = r.kind == MediaKind::Image;
                                self.deps.thumbs.set_thumb(
                                    r.id,
                                    res.as_ref().and_then(|x| x.thumbhash.as_deref()),
                                    if res.is_some() { 1 } else { 2 },
                                    res.as_ref().filter(|_| img).map(|x| x.width),
                                    res.as_ref().filter(|_| img).map(|x| x.height),
                                );
                                // dimensions filled in by the thumbnail change the layout: new generation
                                let dims = res.is_some()
                                    && img
                                    && (r.width.is_none() || r.height.is_none());
                                self.deps.events.media_changed(
                                    &r.folder_id.to_string(),
                                    &r.dir,
                                    if dims {
                                        MediaReason::Meta
                                    } else {
                                        MediaReason::Thumb
                                    },
                                );
                            }
                            Err(e) => {
                                if e.status == 503 || self.conn_live(cid).is_none() {
                                    offline.store(true, Ordering::Relaxed);
                                } else {
                                    failed_now.lock().unwrap().push(r.id);
                                    self.deps.thumbs.set_thumb(r.id, None, 2, None, None);
                                }
                            }
                        }
                    }
                })
                .await;
            failed.extend(failed_now.into_inner().unwrap());
            if offline.load(Ordering::Relaxed) {
                break;
            }
        }
    }

    // ───────────────────────── pull (PC → phone) ─────────────────────────

    pub fn create_pull(
        &self,
        device_id: &str,
        abs: std::path::PathBuf,
        size: u64,
        mtime: i64,
        hooks: PullHooks,
    ) -> String {
        let token = random_b64(24);
        self.st.lock().unwrap().pulls.insert(
            token.clone(),
            Pull {
                device_id: device_id.to_string(),
                abs,
                size,
                mtime,
                expires: now_ms() + PULL_TTL_MS,
                hooks,
            },
        );
        token
    }

    pub fn revoke_pull(&self, token: &str) {
        self.st.lock().unwrap().pulls.remove(token);
    }

    /// one-time: the token is consumed by the first GET (404 unknown / expired, 403 other device)
    pub fn take_pull(&self, token: &str, device_id: &str) -> Result<Pull, HttpError> {
        let mut st = self.st.lock().unwrap();
        let p = st
            .pulls
            .get(token)
            .filter(|p| p.expires >= now_ms())
            .ok_or_else(|| HttpError::new(404, "Unknown or expired pull token"))?;
        if p.device_id != device_id {
            return Err(HttpError::new(403, "Pull token belongs to another device"));
        }
        Ok(st.pulls.remove(token).unwrap())
    }

    /// a receive request whose pull started no longer needs its first-byte timer
    pub fn hold_receive(&self, rid: &str) {
        if let Some(r) = self.st.lock().unwrap().reqs.get_mut(rid) {
            if let Some(t) = r.timer.take() {
                t.abort();
            }
        }
    }

    /// after the pull finished the phone must confirm (`res`) within a minute
    pub fn await_receive_result(&self, rid: &str) {
        let me = self.arc();
        let mut st = self.st.lock().unwrap();
        let Some(r) = st.reqs.get_mut(rid) else {
            return;
        };
        if r.state == ReqState::Done {
            return;
        }
        if let Some(t) = r.timer.take() {
            t.abort();
        }
        let rid2 = rid.to_string();
        let h = tokio::spawn(async move {
            tokio::time::sleep(RECEIVE_CONFIRM).await;
            me.fail(
                &rid2,
                HttpError::new(504, "The device did not confirm the file"),
                true,
            );
        });
        r.timer = Some(h.abort_handle());
    }

    /// rid of the request currently being sent for a receive (by its args.url)
    pub fn rid_of_pull(&self, token: &str) -> Option<String> {
        let url = format!("/api/devices/pull/{token}");
        let st = self.st.lock().unwrap();
        st.reqs
            .values()
            .find(|r| {
                r.op == "receive" && r.args.get("url").and_then(Value::as_str) == Some(url.as_str())
            })
            .map(|r| r.rid.clone())
    }

    // ───────────────────────── details ─────────────────────────

    /// `details` op (None when offline or unreadable). The caller refines the row with
    /// [`crate::media::details_tags`] and builds the MediaDetails ([`crate::media::details_exif`]).
    pub async fn details(
        &self,
        device_id: &str,
        key: &str,
        rel: &str,
    ) -> Result<Option<DeviceDetails>, HttpError> {
        if !self.is_online(device_id) {
            return Ok(None);
        }
        let (status, _, data) = self
            .fetch_buffer(
                device_id,
                "details",
                json!({ "key": key, "p": rel }),
                1,
                2 * 1024 * 1024,
                None,
            )
            .await?;
        if status != 200 {
            return Ok(None);
        }
        Ok(serde_json::from_slice::<Value>(&data)
            .ok()
            .filter(Value::is_object)
            .and_then(|v| serde_json::from_value(v).ok()))
    }

    /// Stopping the desktop LAN share: close every phone socket that came through it.
    pub fn close_lan(&self) {
        let cids: Vec<u64> = {
            let st = self.st.lock().unwrap();
            st.all
                .values()
                .filter(|c| c.lan && !c.closed)
                .map(|c| c.cid)
                .collect()
        };
        for cid in cids {
            self.detach(cid);
            if let Some(c) = self.st.lock().unwrap().all.get_mut(&cid) {
                c.close(1001, "sharing stopped");
            }
        }
    }

    /// Shutdown: drop every phone (pending requests fail), then wait for listing / thumbnail work.
    pub async fn close(&self) {
        let cids: Vec<u64> = {
            let st = self.st.lock().unwrap();
            if st.closed {
                return;
            }
            st.conns
                .values()
                .chain(st.pending.values())
                .copied()
                .collect()
        };
        for cid in cids {
            self.detach(cid);
            if let Some(c) = self.st.lock().unwrap().all.get_mut(&cid) {
                c.close(1001, "server shutting down");
            }
        }
        {
            let mut st = self.st.lock().unwrap();
            st.closed = true;
            st.pulls.clear();
        }
        if let Some(h) = self.heartbeat.lock().unwrap().take() {
            h.abort();
        }
        self.loops.close();
        self.loops.wait().await;
    }
}

fn pending_info(p: &Conn) -> DeviceInfo {
    DeviceInfo {
        id: p.id.clone(),
        name: p.name.clone(),
        model: p.model.clone(),
        online: true,
        paired: false,
        last_seen: p.last_seen_ms,
        folders: Vec::new(),
        code: p.code.clone(),
        caps: Some(p.caps),
    }
}

fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64).filter(|x| x.is_finite())
}

fn pos_int(v: Option<&Value>) -> Option<f64> {
    num(v).filter(|x| *x > 0.0).map(pa_core::js::round)
}

fn listed_file(o: &Value, dir: String, name: String, kind: MediaKind) -> ListedFile {
    let ext = pa_core::ext_of(&name);
    let playback = match kind {
        MediaKind::Image => {
            if pa_core::BROWSER_IMAGE_EXT.contains(&ext.as_str()) {
                Playback::Direct
            } else {
                Playback::Convert
            }
        }
        MediaKind::Video => {
            if pa_media_playable(&ext) {
                Playback::Direct
            } else {
                Playback::Transcode
            }
        }
        _ => Playback::Direct,
    };
    let taken = num(o.get("t")).filter(|t| *t > 0.0);
    let meta = ExtractedMeta {
        width: pos_int(o.get("w")),
        height: pos_int(o.get("h")),
        duration: num(o.get("d")),
        playback,
        instant: taken.map(|t| t as i64),
        ..Default::default()
    };
    ListedFile {
        dir,
        name,
        ext,
        kind,
        size: num(o.get("s")).unwrap_or(0.0) as i64,
        mtime: num(o.get("m")).unwrap_or(0.0) as i64,
        meta,
        meta_state: 1,
    }
}

/// `isBrowserPlayable(ext, null)`
pub(crate) fn pa_media_playable(ext: &str) -> bool {
    ["mp4", "m4v", "webm", "ogv"].contains(&ext)
}

impl SocketHandle {
    /// a text frame from the phone
    pub fn on_text(&self, text: &str) {
        if self.hub.is_closed() || self.dead.load(Ordering::Relaxed) {
            return;
        }
        let Ok(msg) = serde_json::from_str::<Value>(text) else {
            return;
        };
        if !msg.is_object() {
            return;
        }
        let cid = *self.cid.lock().unwrap();
        match cid {
            None => {
                self.hello_timer.cancel();
                match self.hub.on_hello(self, &msg) {
                    Some(c) => *self.cid.lock().unwrap() = Some(c),
                    None => self.dead.store(true, Ordering::Relaxed),
                }
            }
            Some(c) => {
                self.hub.touch(c);
                self.hub.on_frame(c, &msg);
            }
        }
    }

    /// a binary frame: refused (4400)
    pub fn on_binary(&self) {
        if self.hub.is_closed() {
            return;
        }
        let _ = self.out.send(WsOut::Close(4400, "text frames only".into()));
    }

    /// a WS pong (answer to our ping)
    pub fn on_pong(&self) {
        if let Some(c) = *self.cid.lock().unwrap() {
            self.hub.touch(c);
        }
    }

    /// the socket closed
    pub fn on_close(&self) {
        self.hello_timer.cancel();
        let cid = *self.cid.lock().unwrap();
        if let Some(c) = cid {
            if !self.hub.is_closed() {
                self.hub.detach(c);
            }
            // forget the connection record once nothing refers to it anymore
            let mut st = self.hub.st.lock().unwrap();
            let id = st.all.get(&c).map(|x| x.id.clone());
            if let Some(id) = id {
                if st.conns.get(&id) != Some(&c) && st.pending.get(&id) != Some(&c) {
                    st.all.remove(&c);
                }
            }
        }
    }

    /// the device id once the hello was accepted
    pub fn device_id(&self) -> Option<String> {
        let c = (*self.cid.lock().unwrap())?;
        self.hub
            .st
            .lock()
            .unwrap()
            .all
            .get(&c)
            .map(|x| x.id.clone())
    }
}
