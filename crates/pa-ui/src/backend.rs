//! What the UI talks to. The desktop app (and the Android app in client mode) use [`HttpBackend`]
//! over `pa-client`; Android local mode will implement [`Backend`] over MediaStore; [`DemoBackend`]
//! synthesizes a big library (perf tests, screenshots without a server).
//!
//! Futures are `Send + 'static` so they can run on the tokio runtime the binary provides; the UI
//! thread only receives their results (see `rt.rs`).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use pa_core::{BrowseQuery, BrowseResult, BrowseSkeleton, ServerEvent, ServerInfo};

/// Error shown to the user (+ HTTP status, 0 = network).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendError {
    /// HTTP status (0 = network / unknown)
    pub status: u16,
    /// message
    pub message: String,
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Backend result.
pub type BResult<T> = Result<T, BackendError>;
/// Boxed backend future.
pub type BoxFut<T> = Pin<Box<dyn Future<Output = BResult<T>> + Send + 'static>>;

/// A thumbnail as delivered by the backend.
pub enum ThumbData {
    /// encoded image (WebP / JPEG / PNG)
    Encoded(bytes::Bytes),
    /// raw RGBA8
    Rgba {
        /// width
        width: u32,
        /// height
        height: u32,
        /// pixels
        pixels: Vec<u8>,
    },
}

/// Connection state of the live events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connection {
    /// connecting / reconnecting
    Connecting,
    /// connected
    Online,
    /// reconnects keep failing
    Offline,
}

/// Live event.
#[derive(Debug, Clone, PartialEq)]
// events are rare: no need to box the big `ServerEvent`
#[allow(clippy::large_enum_variant)]
pub enum BackendEvent {
    /// connection state
    Connection(Connection),
    /// server event
    Server(ServerEvent),
}

/// Event sink (called from a background thread).
pub type EventSink = Box<dyn Fn(BackendEvent) + Send + Sync + 'static>;

/// The UI's view of a PeekAround server (or a local library).
pub trait Backend: Send + Sync + 'static {
    /// short label (server URL / "Demo")
    fn label(&self) -> String;
    /// `GET /api/info`
    fn info(&self) -> BoxFut<ServerInfo>;
    /// exact-layout skeleton of a query
    fn skeleton(&self, q: BrowseQuery) -> BoxFut<BrowseSkeleton>;
    /// one page of a query
    fn browse(&self, q: BrowseQuery) -> BoxFut<BrowseResult>;
    /// a thumbnail (`size`: 320 / 640 / 1600)
    fn thumb(&self, id: String, size: u32) -> BoxFut<ThumbData>;
    /// start delivering live events to `sink` (until the backend is dropped)
    fn subscribe(&self, sink: EventSink);

    /// Viewer / selection actions / drag & drop (src/actions_ops.rs); None = browse only.
    fn ops(&self) -> Option<&dyn crate::actions_ops::MediaOps> {
        None
    }

    /// Any JSON route of the HTTP API (`method`, `path` with query, optional body) — the pages
    /// (libraries, albums, settings, devices, MEGA, jobs …) use it through `crate::api`.
    /// Backends without that API answer 501.
    fn request(
        &self,
        _method: &str,
        _path: &str,
        _body: Option<serde_json::Value>,
    ) -> BoxFut<serde_json::Value> {
        Box::pin(async {
            Err(BackendError {
                status: 501,
                message: "not supported".into(),
            })
        })
    }

    /// Absolute URL of a server path (downloads, links); None without an HTTP server.
    fn url(&self, _path: &str) -> Option<String> {
        None
    }

    /// `GET <path>` as raw bytes (map tiles through the server's tile proxy). Errors keep the
    /// HTTP status (504 offline, 502 refused by the tile server).
    fn get_bytes(&self, _path: &str) -> BoxFut<bytes::Bytes> {
        Box::pin(async {
            Err(BackendError {
                status: 501,
                message: "not supported".into(),
            })
        })
    }

    /// Save `GET <path>` (zip download) into `dest`; returns the bytes written.
    fn download_to(&self, _path: &str, _dest: std::path::PathBuf) -> BoxFut<u64> {
        Box::pin(async {
            Err(BackendError {
                status: 501,
                message: "not supported".into(),
            })
        })
    }

    /// Upload a local file (`PUT /api/upload`).
    fn upload_path(
        &self,
        _file: std::path::PathBuf,
        _opts: UploadSpec,
    ) -> BoxFut<pa_core::UploadResult> {
        Box::pin(async {
            Err(BackendError {
                status: 501,
                message: "not supported".into(),
            })
        })
    }
}

/// Where an upload goes.
#[derive(Debug, Clone, Default)]
pub struct UploadSpec {
    /// target folder
    pub folder_id: String,
    /// target directory ('' = folder root)
    pub dir: String,
    /// relative path of the file (folder uploads keep their structure)
    pub path: String,
    /// zip handling: None = server setting, "extract" / "temp" / "keep"
    pub zip: Option<pa_core::UploadZipMode>,
    /// original modification time (ms)
    pub mtime: Option<i64>,
}

// ───────── HTTP (pa-client) ─────────

/// [`Backend`] over the HTTP/WS API.
#[cfg(feature = "http")]
pub struct HttpBackend {
    client: pa_client::Client,
    rt: tokio::runtime::Handle,
    events: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

#[cfg(feature = "http")]
impl HttpBackend {
    /// `rt`: the runtime the event socket runs on.
    pub fn new(client: pa_client::Client, rt: tokio::runtime::Handle) -> Arc<HttpBackend> {
        Arc::new(HttpBackend {
            client,
            rt,
            events: Default::default(),
        })
    }

    /// the client
    pub fn client(&self) -> &pa_client::Client {
        &self.client
    }
}

#[cfg(feature = "http")]
impl Drop for HttpBackend {
    fn drop(&mut self) {
        for t in self.events.lock().unwrap().drain(..) {
            t.abort();
        }
    }
}

#[cfg(feature = "http")]
fn berr(e: pa_client::Error) -> BackendError {
    BackendError {
        status: e.status(),
        message: e.to_string(),
    }
}

#[cfg(feature = "http")]
impl Backend for HttpBackend {
    fn ops(&self) -> Option<&dyn crate::actions_ops::MediaOps> {
        Some(self)
    }
    fn label(&self) -> String {
        self.client.base().to_string()
    }
    fn info(&self) -> BoxFut<ServerInfo> {
        let c = self.client.clone();
        Box::pin(async move { c.info().await.map_err(berr) })
    }
    fn skeleton(&self, q: BrowseQuery) -> BoxFut<BrowseSkeleton> {
        let c = self.client.clone();
        Box::pin(async move { c.skeleton(&q).await.map_err(berr) })
    }
    fn browse(&self, q: BrowseQuery) -> BoxFut<BrowseResult> {
        let c = self.client.clone();
        Box::pin(async move { c.browse(&q).await.map_err(berr) })
    }
    fn thumb(&self, id: String, size: u32) -> BoxFut<ThumbData> {
        let c = self.client.clone();
        Box::pin(async move {
            let url = c.thumb_url(&id, size);
            c.get_bytes(&url)
                .await
                .map(ThumbData::Encoded)
                .map_err(berr)
        })
    }
    fn subscribe(&self, sink: EventSink) {
        let mut ev = self.client.events_on(&self.rt);
        let task = self.rt.spawn(async move {
            while let Some(e) = ev.recv().await {
                sink(match e {
                    pa_client::ClientEvent::Connection(s) => BackendEvent::Connection(match s {
                        pa_client::ConnectionState::Connecting => Connection::Connecting,
                        pa_client::ConnectionState::Online => Connection::Online,
                        pa_client::ConnectionState::Offline => Connection::Offline,
                    }),
                    pa_client::ClientEvent::Server(e) => BackendEvent::Server(e),
                });
            }
        });
        self.events.lock().unwrap().push(task);
    }
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> BoxFut<serde_json::Value> {
        let c = self.client.clone();
        let (m, p) = (method.to_string(), path.to_string());
        Box::pin(async move { c.call_json(&m, &p, body.as_ref()).await.map_err(berr) })
    }
    fn url(&self, path: &str) -> Option<String> {
        Some(self.client.absolute(path))
    }
    fn get_bytes(&self, path: &str) -> BoxFut<bytes::Bytes> {
        let c = self.client.clone();
        let url = format!("{}{path}", c.base());
        Box::pin(async move { c.get_bytes(&url).await.map_err(berr) })
    }
    fn download_to(&self, path: &str, dest: std::path::PathBuf) -> BoxFut<u64> {
        let c = self.client.clone();
        let p = path.to_string();
        Box::pin(async move { c.download_to(&p, &dest).await.map_err(berr) })
    }
    fn upload_path(
        &self,
        file: std::path::PathBuf,
        o: UploadSpec,
    ) -> BoxFut<pa_core::UploadResult> {
        let c = self.client.clone();
        Box::pin(async move {
            let opts = pa_client::UploadOptions {
                folder_id: o.folder_id,
                dir: o.dir,
                path: o.path,
                zip: o.zip,
                mtime: o.mtime,
            };
            c.upload_path(&file, &opts).await.map_err(berr)
        })
    }
}

// ───────── demo ─────────

/// A synthetic library of `n` items (random aspects, ~1–60 items per day, procedural thumbnails,
/// 15–60 ms latency): perf tests and screenshots without a server.
pub struct DemoBackend {
    n: usize,
    aspects: Vec<u8>,
    days: Vec<pa_core::SkeletonDay>,
}

/// xorshift
fn rnd(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

impl DemoBackend {
    /// `n` items
    pub fn new(n: usize) -> Arc<DemoBackend> {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        const ASPECTS: [f64; 7] = [1.5, 0.6667, 1.3333, 0.75, 1.7778, 1.0, 0.5625];
        let aspects = (0..n)
            .map(|_| pa_core::aspect_to_byte(ASPECTS[(rnd(&mut seed) % 7) as usize]))
            .collect();
        let places = [
            "Kadıköy",
            "Beşiktaş",
            "Bodrum",
            "Kapadokya",
            "Berlin",
            "Antalya",
        ];
        let mut days = Vec::new();
        let (mut left, mut day) = (n, 0i64);
        // newest first, starting today-ish
        let start = 20_400i64; // days since epoch ≈ 2025-11
        while left > 0 {
            let c = (1 + rnd(&mut seed) % 60) as usize;
            let c = c.min(left);
            let (y, m, d) = crate::i18n::civil_from_days(start - day);
            let place = rnd(&mut seed)
                .is_multiple_of(3)
                .then(|| places[(rnd(&mut seed) % 6) as usize].to_string());
            days.push(pa_core::SkeletonDay {
                key: format!("{y:04}-{m:02}-{d:02}"),
                count: c as i64,
                place,
            });
            left -= c;
            day += 1 + (rnd(&mut seed) % 4) as i64;
        }
        Arc::new(DemoBackend { n, aspects, days })
    }

    /// the synthetic item with id `d<i>` (viewer details)
    pub fn demo_item(&self, id: &str) -> Option<pa_core::MediaItem> {
        let i: usize = id.strip_prefix('d')?.parse().ok()?;
        (i < self.n).then(|| self.item(i))
    }

    fn item(&self, i: usize) -> pa_core::MediaItem {
        let a = pa_core::byte_to_aspect(self.aspects[i]);
        let video = i % 23 == 7;
        pa_core::MediaItem {
            id: format!("d{i}"),
            folder_id: "1".into(),
            dir: String::new(),
            name: format!("IMG_{i:05}.jpg"),
            ext: "jpg".into(),
            kind: if video {
                pa_core::MediaKind::Video
            } else {
                pa_core::MediaKind::Image
            },
            size: 1_000_000,
            mtime: 0,
            width: Some((1000.0 * a).round() as i64),
            height: Some(1000),
            duration: video.then_some(12.0 + (i % 90) as f64),
            taken_at: 0,
            local_at: 0,
            flags: 0,
            thumbhash: None,
            favorite: i % 31 == 3,
            playback: pa_core::Playback::Direct,
        }
    }
}

/// simulated latency (created inside the future: it runs on the tokio runtime)
async fn sleep_ms(ms: u64) {
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await
}

impl Backend for DemoBackend {
    fn ops(&self) -> Option<&dyn crate::actions_ops::MediaOps> {
        Some(self)
    }
    fn label(&self) -> String {
        format!("Demo ({} items)", self.n)
    }
    fn info(&self) -> BoxFut<ServerInfo> {
        Box::pin(async {
            serde_json::from_value(serde_json::json!({"name":"PeekAround","version":"demo","apiVersion":1,"mode":"desktop","hostname":"demo","scanning":false,
              "caps":{"upload":false,"fileOps":false,"nativeFolders":false,"browseHost":false,"videoJobs":false,"slideshowExport":false,
                      "zipDownload":false,"trash":"none","readonly":true,"lanShare":false},"lan":{"enabled":false,"urls":[]}}))
            .map_err(|e| BackendError { status: 0, message: e.to_string() })
        })
    }
    fn skeleton(&self, q: BrowseQuery) -> BoxFut<BrowseSkeleton> {
        use base64::Engine;
        let fav = q.kind == Some(pa_core::KindFilter::Favorites);
        let (aspects, days) = if fav {
            let idx: Vec<usize> = (0..self.n).filter(|i| i % 31 == 3).collect();
            (
                idx.iter().map(|&i| self.aspects[i]).collect::<Vec<u8>>(),
                None,
            )
        } else {
            (self.aspects.clone(), Some(self.days.clone()))
        };
        let total = aspects.len() as i64;
        let wait = sleep_ms(80);
        Box::pin(async move {
            wait.await;
            Ok(BrowseSkeleton {
                gen: "demo".into(),
                total,
                kind_counts: pa_core::KindCounts {
                    image: total - total / 23,
                    video: total / 23,
                    audio: 0,
                },
                aspects: base64::engine::general_purpose::STANDARD.encode(aspects),
                days,
            })
        })
    }
    fn browse(&self, q: BrowseQuery) -> BoxFut<BrowseResult> {
        let fav = q.kind == Some(pa_core::KindFilter::Favorites);
        let all: Vec<usize> = if fav {
            (0..self.n).filter(|i| i % 31 == 3).collect()
        } else {
            (0..self.n).collect()
        };
        let off = q.offset.unwrap_or(0).max(0) as usize;
        let lim = q.limit.unwrap_or(300).max(0) as usize;
        let items = all
            .iter()
            .skip(off)
            .take(lim)
            .map(|&i| self.item(i))
            .collect();
        let total = all.len() as i64;
        let wait = sleep_ms(40);
        Box::pin(async move {
            wait.await;
            Ok(BrowseResult {
                crumbs: vec![],
                folders: vec![],
                roots: vec![],
                items,
                total,
                archives: vec![],
                target: None,
                gen: Some("demo".into()),
            })
        })
    }
    fn thumb(&self, id: String, size: u32) -> BoxFut<ThumbData> {
        let i: usize = id.trim_start_matches('d').parse().unwrap_or(0);
        let a = pa_core::byte_to_aspect(*self.aspects.get(i).unwrap_or(&128));
        let wait = sleep_ms(15 + (i as u64 * 7919) % 45);
        Box::pin(async move {
            wait.await;
            let (w, h) = if a >= 1.0 {
                (size, (size as f64 / a) as u32)
            } else {
                ((size as f64 * a) as u32, size)
            };
            let (w, h) = (w.max(1), h.max(1));
            // a soft two-colour gradient per item
            let mut s = (i as u64).wrapping_mul(0x2545_f491_4f6c_dd1d) | 1;
            let c1 = [
                (rnd(&mut s) % 200) as f32 + 30.0,
                (rnd(&mut s) % 200) as f32 + 30.0,
                (rnd(&mut s) % 200) as f32 + 30.0,
            ];
            let c2 = [
                (rnd(&mut s) % 200) as f32 + 30.0,
                (rnd(&mut s) % 200) as f32 + 30.0,
                (rnd(&mut s) % 200) as f32 + 30.0,
            ];
            let mut px = Vec::with_capacity((w * h * 4) as usize);
            for y in 0..h {
                for x in 0..w {
                    let t = (x as f32 / w as f32 * 0.6 + y as f32 / h as f32 * 0.4).clamp(0.0, 1.0);
                    for k in 0..3 {
                        px.push((c1[k] * (1.0 - t) + c2[k] * t) as u8);
                    }
                    px.push(255);
                }
            }
            Ok(ThumbData::Rgba {
                width: w,
                height: h,
                pixels: px,
            })
        })
    }
    fn subscribe(&self, sink: EventSink) {
        sink(BackendEvent::Connection(Connection::Online));
    }
}
