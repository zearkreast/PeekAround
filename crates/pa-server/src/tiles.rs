//! Map tile proxy + on-disk LRU cache (mirrors `tiles.ts`): coalesced upstream fetches (4 per
//! style), 14-day freshness with stale-on-error, a one-minute pause after a refusal, an
//! identifying User-Agent and the cap of Settings.tileCacheMb (trimmed to 90 %).

use futures_util::future::{BoxFuture, FutureExt, Shared};
use pa_core::MapTileStyle;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Why a tile is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileErrorKind {
    Offline,
    Refused,
    Invalid,
}

impl TileErrorKind {
    /// 'offline' | 'refused' | 'invalid'
    pub fn as_str(self) -> &'static str {
        match self {
            TileErrorKind::Offline => "offline",
            TileErrorKind::Refused => "refused",
            TileErrorKind::Invalid => "invalid",
        }
    }
}

/// A tile failure.
#[derive(Debug, Clone)]
pub struct TileError {
    pub kind: TileErrorKind,
    pub message: String,
    pub status: u16,
}

/// A tile.
#[derive(Debug, Clone)]
pub struct TileResult {
    pub body: bytes::Bytes,
    pub content_type: &'static str,
    /// 'hit' | 'miss' | 'stale'
    pub cache: &'static str,
}

struct Entry {
    size: u64,
    used: i64,
}

const FRESH_MS: i64 = 14 * 86_400_000;
const REFUSED_PAUSE_MS: i64 = 60_000;
const TIMEOUT: Duration = Duration::from_secs(12);
const MAX_TILE_BYTES: usize = 2 * 1024 * 1024;
const MAX_PARALLEL: usize = 4;

type Upstream = Arc<dyn Fn(MapTileStyle, i64, i64, i64) -> String + Send + Sync>;
type Load = Shared<BoxFuture<'static, Result<TileResult, TileError>>>;

struct Index {
    entries: HashMap<PathBuf, Entry>,
    total: u64,
}

/// The tile service.
pub struct TileService {
    dir: PathBuf,
    user_agent: String,
    cap_mb: Arc<dyn Fn() -> i64 + Send + Sync>,
    index: tokio::sync::OnceCell<Mutex<Index>>,
    inflight: Mutex<HashMap<String, Load>>,
    paused: Mutex<HashMap<MapTileStyle, (i64, u16)>>,
    slots: HashMap<MapTileStyle, Arc<tokio::sync::Semaphore>>,
    trimming: tokio::sync::Mutex<()>,
    /// last upstream problem per style (kind, status, at)
    pub last_error: Mutex<HashMap<MapTileStyle, (TileErrorKind, u16, i64)>>,
    /// tests: replace the upstream URL of a tile
    pub upstream: Mutex<Upstream>,
    client: reqwest::Client,
}

fn now() -> i64 {
    crate::events::now_ms()
}

/// `SSL_CERT_FILE` (PEM bundle) adds trusted roots on top of the bundled webpki roots — for
/// networks behind a TLS-inspecting proxy (and the test sandbox). Unset: the roots stay as they are.
fn extra_roots(b: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    let Some(path) = std::env::var_os("SSL_CERT_FILE") else {
        return b;
    };
    match std::fs::read(&path) {
        Ok(pem) => match reqwest::Certificate::from_pem_bundle(&pem) {
            Ok(certs) => certs.into_iter().fold(b, |b, c| b.add_root_certificate(c)),
            Err(e) => {
                tracing::warn!("SSL_CERT_FILE: {e}");
                b
            }
        },
        Err(e) => {
            tracing::warn!(
                "SSL_CERT_FILE {}: {e}",
                std::path::Path::new(&path).display()
            );
            b
        }
    }
}

fn mtime_ms(md: &std::fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl TileService {
    pub fn new(
        dir: PathBuf,
        user_agent: String,
        cap_mb: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Arc<Self> {
        let slots = pa_core::MAP_TILE_STYLES
            .iter()
            .map(|s| (*s, Arc::new(tokio::sync::Semaphore::new(MAX_PARALLEL))))
            .collect();
        Arc::new(TileService {
            dir,
            user_agent,
            cap_mb,
            index: tokio::sync::OnceCell::new(),
            inflight: Mutex::new(HashMap::new()),
            paused: Mutex::new(HashMap::new()),
            slots,
            trimming: tokio::sync::Mutex::new(()),
            last_error: Mutex::new(HashMap::new()),
            upstream: Mutex::new(Arc::new(|s, z, x, y| {
                pa_core::upstream_tile_url(s, z, x, y)
            })),
            client: extra_roots(
                reqwest::Client::builder().redirect(reqwest::redirect::Policy::limited(10)),
            )
            .build()
            .unwrap_or_default(),
        })
    }

    fn file(&self, style: MapTileStyle, z: i64, x: i64, y: i64) -> PathBuf {
        let ext = if pa_core::tile_style(style).kind == pa_core::TileType::Jpeg {
            "jpg"
        } else {
            "png"
        };
        self.dir
            .join(style.as_str())
            .join(z.to_string())
            .join(x.to_string())
            .join(format!("{y}.{ext}"))
    }

    async fn idx(&self) -> &Mutex<Index> {
        self.index
            .get_or_init(|| async {
                let dir = self.dir.clone();
                let idx = tokio::task::spawn_blocking(move || {
                    let mut idx = Index {
                        entries: HashMap::new(),
                        total: 0,
                    };
                    fn walk(d: &Path, idx: &mut Index) {
                        let Ok(rd) = std::fs::read_dir(d) else { return };
                        for e in rd.flatten() {
                            let p = e.path();
                            let name = e.file_name().to_string_lossy().into_owned();
                            match e.file_type() {
                                Ok(t) if t.is_dir() => walk(&p, idx),
                                Ok(t) if t.is_file() && !name.ends_with(".tmp") => {
                                    if let Ok(md) = e.metadata() {
                                        idx.total += md.len();
                                        idx.entries.insert(
                                            p,
                                            Entry {
                                                size: md.len(),
                                                used: mtime_ms(&md),
                                            },
                                        );
                                    }
                                }
                                _ if name.ends_with(".tmp") => {
                                    let _ = std::fs::remove_file(&p);
                                }
                                _ => {}
                            }
                        }
                    }
                    walk(&dir, &mut idx);
                    idx
                })
                .await
                .unwrap_or(Index {
                    entries: HashMap::new(),
                    total: 0,
                });
                Mutex::new(idx)
            })
            .await
    }

    /// (bytes, files, limitMb)
    pub async fn stats(&self) -> (u64, usize, i64) {
        let i = self.idx().await.lock();
        (i.total, i.entries.len(), (self.cap_mb)())
    }

    /// drop the cache
    pub async fn clear(&self) {
        let idx = self.idx().await;
        let _ = tokio::fs::remove_dir_all(&self.dir).await;
        let mut i = idx.lock();
        i.entries.clear();
        i.total = 0;
    }

    /// one tile: cache → upstream (coalesced) → stale cache
    pub async fn get(
        self: &Arc<Self>,
        style: MapTileStyle,
        z: i64,
        x: i64,
        y: i64,
    ) -> Result<TileResult, TileError> {
        if !pa_core::is_valid_tile(style, z as f64, x as f64, y as f64) {
            return Err(TileError {
                kind: TileErrorKind::Invalid,
                message: "Tile out of range".into(),
                status: 400,
            });
        }
        let key = format!("{}/{z}/{x}/{y}", style.as_str());
        let fut = {
            let mut m = self.inflight.lock();
            if let Some(f) = m.get(&key) {
                f.clone()
            } else {
                let me = self.clone();
                let k2 = key.clone();
                let task = tokio::spawn(async move {
                    let r = me.load(style, z, x, y).await;
                    me.inflight.lock().remove(&k2);
                    r
                });
                let f: Load = async move {
                    task.await.unwrap_or_else(|e| {
                        Err(TileError {
                            kind: TileErrorKind::Offline,
                            message: e.to_string(),
                            status: 0,
                        })
                    })
                }
                .boxed()
                .shared();
                m.insert(key, f.clone());
                f
            }
        };
        fut.await
    }

    async fn load(
        self: &Arc<Self>,
        style: MapTileStyle,
        z: i64,
        x: i64,
        y: i64,
    ) -> Result<TileResult, TileError> {
        let file = self.file(style, z, x, y);
        let ctype = if pa_core::tile_style(style).kind == pa_core::TileType::Jpeg {
            "image/jpeg"
        } else {
            "image/png"
        };
        let mut cached: Option<bytes::Bytes> = None;
        if let Ok(md) = tokio::fs::metadata(&file).await {
            if md.is_file() {
                if let Ok(b) = tokio::fs::read(&file).await {
                    let b = bytes::Bytes::from(b);
                    if now() - mtime_ms(&md) < FRESH_MS {
                        self.touch(&file, b.len() as u64).await;
                        return Ok(TileResult {
                            body: b,
                            content_type: ctype,
                            cache: "hit",
                        });
                    }
                    cached = Some(b);
                }
            }
        }
        match self.fetch_upstream(style, z, x, y).await {
            Ok(body) => {
                self.store(&file, &body).await;
                Ok(TileResult {
                    body,
                    content_type: ctype,
                    cache: "miss",
                })
            }
            Err(e) => match cached {
                Some(b) => {
                    self.touch(&file, b.len() as u64).await;
                    Ok(TileResult {
                        body: b,
                        content_type: ctype,
                        cache: "stale",
                    })
                }
                None => Err(e),
            },
        }
    }

    async fn touch(&self, file: &Path, size: u64) {
        let mut i = self.idx().await.lock();
        match i.entries.get_mut(file) {
            Some(e) => e.used = now(),
            None => {
                i.entries
                    .insert(file.to_path_buf(), Entry { size, used: now() });
                i.total += size;
            }
        }
    }

    async fn fetch_upstream(
        &self,
        style: MapTileStyle,
        z: i64,
        x: i64,
        y: i64,
    ) -> Result<bytes::Bytes, TileError> {
        if let Some((until, status)) = self.paused.lock().get(&style).copied() {
            if until > now() {
                return Err(TileError {
                    kind: TileErrorKind::Refused,
                    message: format!("Tile server refused (HTTP {status})"),
                    status,
                });
            }
        }
        let _slot = self.slots[&style].clone().acquire_owned().await;
        let url = (self.upstream.lock().clone())(style, z, x, y);
        let offline = |me: &Self, msg: String| {
            me.last_error
                .lock()
                .insert(style, (TileErrorKind::Offline, 0, now()));
            TileError {
                kind: TileErrorKind::Offline,
                message: msg,
                status: 0,
            }
        };
        let req = self
            .client
            .get(&url)
            .header("User-Agent", &self.user_agent)
            .header("Accept", "image/png,image/jpeg,image/*;q=0.8")
            .timeout(TIMEOUT);
        let res = match req.send().await {
            Ok(r) => r,
            Err(e) => return Err(offline(self, format!("Tile server unreachable: {e}"))),
        };
        let status = res.status().as_u16();
        if !res.status().is_success() {
            if status != 404 {
                self.paused
                    .lock()
                    .insert(style, (now() + REFUSED_PAUSE_MS, status));
                self.last_error
                    .lock()
                    .insert(style, (TileErrorKind::Refused, status, now()));
            }
            let _ = res.bytes().await;
            return Err(TileError {
                kind: TileErrorKind::Refused,
                message: format!("Tile server answered HTTP {status}"),
                status,
            });
        }
        let ct = res
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let buf = match res.bytes().await {
            Ok(b) => b,
            Err(e) => return Err(offline(self, e.to_string())),
        };
        if !ct.starts_with("image/") || buf.is_empty() || buf.len() > MAX_TILE_BYTES {
            self.last_error
                .lock()
                .insert(style, (TileErrorKind::Refused, status, now()));
            return Err(TileError {
                kind: TileErrorKind::Refused,
                message: "Tile server sent something that is not a tile".into(),
                status: 502,
            });
        }
        self.paused.lock().remove(&style);
        self.last_error.lock().remove(&style);
        Ok(buf)
    }

    fn cap_bytes(&self) -> u64 {
        let mb = (self.cap_mb)();
        let mb = if mb <= 0 { 512 } else { mb.clamp(1, 64 * 1024) };
        mb as u64 * 1024 * 1024
    }

    async fn store(&self, file: &Path, body: &[u8]) {
        if pa_media::render::write_atomic(file, body).await.is_err() {
            return;
        }
        let over = {
            let mut i = self.idx().await.lock();
            if let Some(old) = i.entries.remove(file) {
                i.total -= old.size;
            }
            i.entries.insert(
                file.to_path_buf(),
                Entry {
                    size: body.len() as u64,
                    used: now(),
                },
            );
            i.total += body.len() as u64;
            i.total > self.cap_bytes()
        };
        if over {
            self.trim().await;
        }
    }

    /// drop least recently used tiles until the cache is at 90 % of its cap
    pub async fn trim(&self) {
        let Ok(_g) = self.trimming.try_lock() else {
            return;
        };
        let cap = self.cap_bytes();
        let order: Vec<(PathBuf, u64)> = {
            let i = self.idx().await.lock();
            if i.total <= cap {
                return;
            }
            let mut v: Vec<(PathBuf, u64, i64)> = i
                .entries
                .iter()
                .map(|(p, e)| (p.clone(), e.size, e.used))
                .collect();
            v.sort_by_key(|e| e.2);
            v.into_iter().map(|(p, s, _)| (p, s)).collect()
        };
        let target = (cap as f64 * 0.9) as u64;
        for (file, size) in order {
            {
                let i = self.idx().await.lock();
                if i.total <= target {
                    break;
                }
            }
            let _ = tokio::fs::remove_file(&file).await;
            let mut i = self.idx().await.lock();
            if i.entries.remove(&file).is_some() {
                i.total -= size;
            }
        }
    }
}
