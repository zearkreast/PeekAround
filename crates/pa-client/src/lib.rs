//! Typed client for the PeekAround HTTP/WS API (the routes of `packages/web/src/backend/http.ts`),
//! over the JSON types of `pa-core`.
//!
//! - [`Client`]: cheap to clone (`Arc` inside); every request is an `async fn`.
//! - URL builders (`thumb_url`, `file_url`, …) for media the UI fetches itself, with the paired
//!   device credentials appended as `pdev` / `ptok` query parameters like the web UI does.
//! - [`Client::events`]: the `/api/events` WebSocket with automatic reconnect (exponential backoff
//!   500 ms · 2ⁿ, at most 15 s) and connection state reports.
//!
//! Transport: `reqwest` (rustls) — it also builds for `wasm32-unknown-unknown`, where it uses
//! `fetch`, so the HTTP half is shared by the native apps and the browser build. Only the events
//! socket has a per-target implementation (`events.rs`: tokio-tungstenite on native; a web-sys
//! `WebSocket` implementation is the seam for the wasm build).

#![warn(missing_docs)]

mod error;
#[cfg(not(target_arch = "wasm32"))]
mod events;
mod query;

use std::sync::{Arc, RwLock};

use pa_core::{
    Album, AlbumsResponse, BrowseQuery, BrowseResult, BrowseSkeleton, DeviceInfo, DirTarget,
    ExploreResult, FileRef, Job, JobsResponse, LibrariesResponse, MediaDetails, MediaItem,
    MediaKind, Playback, ServerInfo, Settings, TimelineResponse, UploadResult, UploadZipMode,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub use error::{Error, Result};
#[cfg(not(target_arch = "wasm32"))]
pub use events::{ClientEvent, ConnectionState, Events};
pub use query::{browse_qs, encode_seg, Qs};

/// Credentials of a paired phone (PeekLink) — sent as headers and as `pdev`/`ptok` on media URLs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceAuth {
    /// device id
    pub device_id: String,
    /// pairing token
    pub token: String,
}

/// Options of [`Client::upload`].
#[derive(Debug, Clone, Default)]
pub struct UploadOptions {
    /// target folder
    pub folder_id: String,
    /// target directory ('' = folder root)
    pub dir: String,
    /// relative path of the file (folder uploads keep their structure)
    pub path: String,
    /// what to do with a zip
    pub zip: Option<UploadZipMode>,
    /// original modification time (ms)
    pub mtime: Option<i64>,
}

/// `POST /api/fs/mkdir`, `/api/zip/extract`, `/api/temp/:id/keep`
pub type DirRef = DirTarget;

/// `POST /api/fs/move`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoveResult {
    /// moved / copied relative paths
    pub moved: Vec<String>,
    /// transfers to / from a linked phone run as a job
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<Job>,
}

/// `POST /api/fs/rename`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameResult {
    /// new relative path
    pub rel: String,
}

/// `POST /api/albums/:id/items` / `…/items/remove`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlbumItemsResult {
    /// added items
    #[serde(default)]
    pub added: Option<i64>,
    /// removed items
    #[serde(default)]
    pub removed: Option<i64>,
    /// the album afterwards
    pub album: Album,
}

/// Thumbnail sizes the server renders.
pub fn thumb_size(size: u32) -> u32 {
    if size <= 320 {
        320
    } else if size <= 640 {
        640
    } else {
        1600
    }
}

struct Inner {
    /// "http://host:port" without a trailing slash
    base: String,
    http: reqwest::Client,
    auth: RwLock<Option<DeviceAuth>>,
}

/// PeekAround API client.
#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("base", &self.inner.base)
            .finish()
    }
}

impl Client {
    /// `base`: e.g. `http://127.0.0.1:47213` (a trailing slash / path is ignored).
    pub fn new(base: &str) -> Result<Client> {
        let mut u = url::Url::parse(base.trim()).map_err(|e| Error::Url(e.to_string()))?;
        if !matches!(u.scheme(), "http" | "https") {
            return Err(Error::Url(format!("unsupported scheme {}", u.scheme())));
        }
        u.set_path("");
        u.set_query(None);
        u.set_fragment(None);
        let base = u.as_str().trim_end_matches('/').to_string();
        #[cfg(not(target_arch = "wasm32"))]
        let http = reqwest::Client::builder()
            .user_agent(concat!("PeekAround/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(10))
            .pool_max_idle_per_host(16)
            .build()
            .map_err(|e| Error::Network(e.to_string()))?;
        #[cfg(target_arch = "wasm32")]
        let http = reqwest::Client::new();
        Ok(Client {
            inner: Arc::new(Inner {
                base,
                http,
                auth: RwLock::new(None),
            }),
        })
    }

    /// Server base URL (no trailing slash).
    pub fn base(&self) -> &str {
        &self.inner.base
    }

    /// Use (or drop) paired-device credentials for every request and media URL.
    pub fn set_device_auth(&self, auth: Option<DeviceAuth>) {
        let auth = auth.filter(|a| !a.device_id.is_empty() && !a.token.is_empty());
        *self.inner.auth.write().unwrap() = auth;
    }

    /// Current paired-device credentials.
    pub fn device_auth(&self) -> Option<DeviceAuth> {
        self.inner.auth.read().unwrap().clone()
    }

    fn url(&self, p: &str) -> String {
        format!("{}{}", self.inner.base, p)
    }

    /// Media URL + `pdev`/`ptok` when paired (`<img>`-style consumers cannot send headers).
    fn murl(&self, p: &str) -> String {
        let u = self.url(p);
        match &*self.inner.auth.read().unwrap() {
            None => u,
            Some(a) => {
                let sep = if u.contains('?') { '&' } else { '?' };
                format!(
                    "{u}{sep}pdev={}&ptok={}",
                    encode_seg(&a.device_id),
                    encode_seg(&a.token)
                )
            }
        }
    }

    fn request(&self, method: reqwest::Method, p: &str) -> reqwest::RequestBuilder {
        let mut rb = self.inner.http.request(method, self.url(p));
        if let Some(a) = &*self.inner.auth.read().unwrap() {
            rb = rb
                .header("Authorization", format!("Bearer {}", a.token))
                .header("X-Peek-Device", a.device_id.clone());
        }
        rb
    }

    async fn send(&self, rb: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let res = rb.send().await.map_err(|e| Error::Network(e.to_string()))?;
        let status = res.status();
        if status.is_success() {
            return Ok(res);
        }
        let text = res.text().await.unwrap_or_default();
        let message = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
            .unwrap_or_else(|| status.canonical_reason().unwrap_or("error").to_string());
        Err(Error::Api {
            status: status.as_u16(),
            message,
        })
    }

    async fn json<T: DeserializeOwned>(&self, rb: reqwest::RequestBuilder) -> Result<T> {
        let res = self.send(rb).await?;
        let bytes = res
            .bytes()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;
        serde_json::from_slice(&bytes).map_err(|e| Error::Decode(e.to_string()))
    }

    async fn get<T: DeserializeOwned>(&self, p: &str) -> Result<T> {
        self.json(self.request(reqwest::Method::GET, p)).await
    }

    async fn with_body<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        m: reqwest::Method,
        p: &str,
        body: &B,
    ) -> Result<T> {
        self.json(self.request(m, p).json(body)).await
    }

    async fn unit<B: Serialize + ?Sized>(
        &self,
        m: reqwest::Method,
        p: &str,
        body: Option<&B>,
    ) -> Result<()> {
        let mut rb = self.request(m, p);
        if let Some(b) = body {
            rb = rb.json(b);
        }
        self.send(rb).await.map(|_| ())
    }

    // ───────── server ─────────

    /// `GET /api/info`
    pub async fn info(&self) -> Result<ServerInfo> {
        self.get("/api/info").await
    }

    /// `GET /api/settings`
    pub async fn settings(&self) -> Result<Settings> {
        self.get("/api/settings").await
    }

    /// `PATCH /api/settings` with a partial settings object (camelCase keys).
    pub async fn update_settings(&self, patch: &serde_json::Value) -> Result<Settings> {
        self.with_body(reqwest::Method::PATCH, "/api/settings", patch)
            .await
    }

    // ───────── libraries / browse ─────────

    /// `GET /api/libraries`
    pub async fn libraries(&self) -> Result<LibrariesResponse> {
        self.get("/api/libraries").await
    }

    /// `GET /api/browse` (one page: `offset` / `limit`)
    pub async fn browse(&self, q: &BrowseQuery) -> Result<BrowseResult> {
        self.get(&format!("/api/browse{}", browse_qs(q))).await
    }

    /// `GET /api/browse/skeleton` (paging fields are ignored)
    pub async fn skeleton(&self, q: &BrowseQuery) -> Result<BrowseSkeleton> {
        let q = BrowseQuery {
            offset: None,
            limit: None,
            ..q.clone()
        };
        self.get(&format!("/api/browse/skeleton{}", browse_qs(&q)))
            .await
    }

    /// `GET /api/timeline`
    pub async fn timeline(&self, library_id: Option<&str>) -> Result<TimelineResponse> {
        let mut qs = Qs::new();
        qs.opt("libraryId", library_id);
        self.get(&format!("/api/timeline{qs}")).await
    }

    /// `GET /api/explore`
    pub async fn explore(&self) -> Result<ExploreResult> {
        self.get("/api/explore").await
    }

    /// `GET /api/media/:id`
    pub async fn media(&self, id: &str) -> Result<MediaDetails> {
        self.get(&format!("/api/media/{}", encode_seg(id))).await
    }

    /// `POST /api/media/favorite`
    pub async fn set_favorite(&self, ids: &[String], favorite: bool) -> Result<()> {
        self.unit(
            reqwest::Method::POST,
            "/api/media/favorite",
            Some(&serde_json::json!({ "ids": ids, "favorite": favorite })),
        )
        .await
    }

    /// `GET /api/devices`
    pub async fn devices(&self) -> Result<Vec<DeviceInfo>> {
        self.get("/api/devices").await
    }

    // ───────── albums ─────────

    /// `GET /api/albums`
    pub async fn albums(&self) -> Result<Vec<Album>> {
        Ok(self.get::<AlbumsResponse>("/api/albums").await?.albums)
    }

    /// `GET /api/albums/:id`
    pub async fn album(&self, id: &str) -> Result<Album> {
        self.get(&format!("/api/albums/{}", encode_seg(id))).await
    }

    /// `POST /api/albums`
    pub async fn create_album(&self, name: &str, ids: &[String]) -> Result<Album> {
        self.with_body(
            reqwest::Method::POST,
            "/api/albums",
            &serde_json::json!({ "name": name, "ids": ids }),
        )
        .await
    }

    /// `PATCH /api/albums/:id` (`name`, `coverId`, `order`)
    pub async fn update_album(&self, id: &str, patch: &serde_json::Value) -> Result<Album> {
        self.with_body(
            reqwest::Method::PATCH,
            &format!("/api/albums/{}", encode_seg(id)),
            patch,
        )
        .await
    }

    /// `DELETE /api/albums/:id`
    pub async fn delete_album(&self, id: &str) -> Result<()> {
        self.unit::<()>(
            reqwest::Method::DELETE,
            &format!("/api/albums/{}", encode_seg(id)),
            None,
        )
        .await
    }

    /// `POST /api/albums/:id/items`
    pub async fn add_to_album(&self, id: &str, ids: &[String]) -> Result<AlbumItemsResult> {
        self.with_body(
            reqwest::Method::POST,
            &format!("/api/albums/{}/items", encode_seg(id)),
            &serde_json::json!({ "ids": ids }),
        )
        .await
    }

    /// `POST /api/albums/:id/items/remove`
    pub async fn remove_from_album(&self, id: &str, ids: &[String]) -> Result<AlbumItemsResult> {
        self.with_body(
            reqwest::Method::POST,
            &format!("/api/albums/{}/items/remove", encode_seg(id)),
            &serde_json::json!({ "ids": ids }),
        )
        .await
    }

    // ───────── file operations ─────────

    /// `POST /api/fs/mkdir`
    pub async fn mkdir(&self, folder_id: &str, dir: &str, name: &str) -> Result<DirRef> {
        self.with_body(
            reqwest::Method::POST,
            "/api/fs/mkdir",
            &serde_json::json!({ "folderId": folder_id, "dir": dir, "name": name }),
        )
        .await
    }

    /// `POST /api/fs/rename`
    pub async fn rename(&self, file: &FileRef, name: &str) -> Result<RenameResult> {
        self.with_body(
            reqwest::Method::POST,
            "/api/fs/rename",
            &serde_json::json!({ "folderId": file.folder_id, "rel": file.rel, "name": name }),
        )
        .await
    }

    /// `POST /api/fs/move` (copies into / out of a linked phone answer with a transfer job)
    pub async fn move_files(
        &self,
        refs: &[FileRef],
        to: &DirTarget,
        copy: bool,
    ) -> Result<MoveResult> {
        self.with_body(
            reqwest::Method::POST,
            "/api/fs/move",
            &serde_json::json!({ "refs": refs, "to": to, "copy": copy }),
        )
        .await
    }

    /// `POST /api/fs/delete`
    pub async fn remove(&self, refs: &[FileRef]) -> Result<()> {
        self.unit(
            reqwest::Method::POST,
            "/api/fs/delete",
            Some(&serde_json::json!({ "refs": refs })),
        )
        .await
    }

    /// `PUT /api/upload` with the raw file body.
    pub async fn upload(
        &self,
        body: impl Into<reqwest::Body>,
        opts: &UploadOptions,
    ) -> Result<UploadResult> {
        let mut qs = Qs::new();
        qs.str("folderId", &opts.folder_id)
            .str("dir", &opts.dir)
            .str("path", &opts.path);
        if let Some(z) = opts.zip {
            qs.str("zip", query::enum_str(&z).as_deref().unwrap_or(""));
        }
        if let Some(m) = opts.mtime {
            qs.num("mtime", m);
        }
        let rb = self
            .request(reqwest::Method::PUT, &format!("/api/upload{qs}"))
            .header("Content-Type", "application/octet-stream")
            .body(body);
        let res = self.send(rb).await?;
        let bytes = res
            .bytes()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;
        // the server answers `{ created: [] }`-like JSON; an empty body counts as "nothing created"
        Ok(serde_json::from_slice(&bytes).unwrap_or(UploadResult {
            created: vec![],
            temp_folder: None,
            extracted_dir: None,
        }))
    }

    /// `PUT /api/upload` streaming a local file (native only).
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn upload_path(
        &self,
        file: &std::path::Path,
        opts: &UploadOptions,
    ) -> Result<UploadResult> {
        let f = tokio::fs::File::open(file)
            .await
            .map_err(|e| Error::Network(format!("{}: {e}", file.display())))?;
        let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(f));
        self.upload(body, opts).await
    }

    // ───────── jobs ─────────

    /// `GET /api/jobs`
    pub async fn jobs(&self) -> Result<Vec<Job>> {
        Ok(self.get::<JobsResponse>("/api/jobs").await?.jobs)
    }

    /// `DELETE /api/jobs/:id`
    pub async fn cancel_job(&self, id: &str) -> Result<()> {
        self.unit::<()>(
            reqwest::Method::DELETE,
            &format!("/api/jobs/{}", encode_seg(id)),
            None,
        )
        .await
    }

    /// `DELETE /api/jobs` (finished jobs)
    pub async fn clear_jobs(&self) -> Result<()> {
        self.unit::<()>(reqwest::Method::DELETE, "/api/jobs", None)
            .await
    }

    // ───────── generic ─────────

    /// Any JSON route: `method` (GET / POST / PUT / PATCH / DELETE), `path` with its query string,
    /// optional JSON body. An empty answer (204) is `null`.
    pub async fn call_json(
        &self,
        method: &str,
        path: &str,
        body: Option<&serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let m = reqwest::Method::from_bytes(method.as_bytes())
            .map_err(|e| Error::Network(e.to_string()))?;
        let mut rb = self.request(m, path);
        if let Some(b) = body {
            rb = rb.json(b);
        }
        let res = self.send(rb).await?;
        let bytes = res
            .bytes()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;
        if bytes.iter().all(|b| b.is_ascii_whitespace()) {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_slice(&bytes).map_err(|e| Error::Decode(e.to_string()))
    }

    /// `PUT <path>` with a raw body (font uploads); the JSON answer, `null` when empty.
    pub async fn put_bytes(&self, path: &str, body: Vec<u8>) -> Result<serde_json::Value> {
        let rb = self
            .request(reqwest::Method::PUT, path)
            .header("Content-Type", "application/octet-stream")
            .body(body);
        let bytes = self
            .send(rb)
            .await?
            .bytes()
            .await
            .map_err(|e| Error::Network(e.to_string()))?;
        if bytes.iter().all(|b| b.is_ascii_whitespace()) {
            return Ok(serde_json::Value::Null);
        }
        serde_json::from_slice(&bytes).map_err(|e| Error::Decode(e.to_string()))
    }

    /// `POST /api/edits/save`: an edited photo (any decodable image) saved as a copy next to
    /// `source_id` (`name_edit.jpg`, `name_edit-2.jpg` …) in `format` (jpg / png / webp).
    pub async fn save_edit(
        &self,
        body: Vec<u8>,
        source_id: &str,
        format: &str,
        quality: u32,
    ) -> Result<serde_json::Value> {
        let mut qs = Qs::new();
        qs.str("sourceId", source_id)
            .str("format", format)
            .num("quality", quality as i64);
        let rb = self
            .request(reqwest::Method::POST, &format!("/api/edits/save{qs}"))
            .header("Content-Type", "application/octet-stream")
            .body(body);
        self.send(rb)
            .await?
            .json()
            .await
            .map_err(|e| Error::Decode(e.to_string()))
    }

    /// `POST /api/edits/save` without a source: a new image (a collage) saved as `name.<format>`
    /// in a library directory (a free name: `name (1).jpg` …).
    pub async fn save_image_to(
        &self,
        body: Vec<u8>,
        folder_id: &str,
        dir: &str,
        name: &str,
        format: &str,
        quality: u32,
    ) -> Result<serde_json::Value> {
        let mut qs = Qs::new();
        qs.str("folderId", folder_id)
            .str("dir", dir)
            .str("name", name)
            .str("format", format)
            .num("quality", quality as i64);
        let rb = self
            .request(reqwest::Method::POST, &format!("/api/edits/save{qs}"))
            .header("Content-Type", "application/octet-stream")
            .body(body);
        self.send(rb)
            .await?
            .json()
            .await
            .map_err(|e| Error::Decode(e.to_string()))
    }

    /// Stream `GET <path>` into a file (zip downloads); returns the bytes written.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn download_to(&self, path: &str, dest: &std::path::Path) -> Result<u64> {
        use futures_util::StreamExt;
        use tokio::io::AsyncWriteExt;
        let res = self.send(self.request(reqwest::Method::GET, path)).await?;
        let tmp = dest.with_extension("part");
        let mut f = tokio::fs::File::create(&tmp)
            .await
            .map_err(|e| Error::Network(format!("{}: {e}", tmp.display())))?;
        let mut n = 0u64;
        let mut st = res.bytes_stream();
        while let Some(chunk) = st.next().await {
            let c = chunk.map_err(|e| Error::Network(e.to_string()))?;
            f.write_all(&c)
                .await
                .map_err(|e| Error::Network(e.to_string()))?;
            n += c.len() as u64;
        }
        f.flush().await.map_err(|e| Error::Network(e.to_string()))?;
        drop(f);
        tokio::fs::rename(&tmp, dest)
            .await
            .map_err(|e| Error::Network(e.to_string()))?;
        Ok(n)
    }

    /// Absolute URL of a server path (downloads, links opened in the browser).
    pub fn absolute(&self, path: &str) -> String {
        self.murl(path)
    }

    // ───────── raw bytes (thumbnails, display images) ─────────

    /// GET any URL of this server (e.g. one of the media URLs below) as bytes.
    pub async fn get_bytes(&self, url: &str) -> Result<bytes::Bytes> {
        let mut rb = self.inner.http.get(url);
        if let Some(a) = &*self.inner.auth.read().unwrap() {
            rb = rb
                .header("Authorization", format!("Bearer {}", a.token))
                .header("X-Peek-Device", a.device_id.clone());
        }
        self.send(rb)
            .await?
            .bytes()
            .await
            .map_err(|e| Error::Network(e.to_string()))
    }

    // ───────── media URLs ─────────

    /// `/api/media/:id/thumb?s=320|640|1600` (WebP)
    pub fn thumb_url(&self, id: &str, size: u32) -> String {
        self.murl(&format!(
            "/api/media/{}/thumb?s={}",
            encode_seg(id),
            thumb_size(size)
        ))
    }

    /// `/api/media/:id/file` (the original)
    pub fn file_url(&self, id: &str, download: bool) -> String {
        self.murl(&format!(
            "/api/media/{}/file{}",
            encode_seg(id),
            if download { "?download=1" } else { "" }
        ))
    }

    /// What to show for an item in the viewer: the converted display image for formats a client
    /// cannot decode (HEIC, TIFF…), the original otherwise.
    pub fn display_url(&self, item: &MediaItem) -> String {
        if item.kind == MediaKind::Image && item.playback == Playback::Convert {
            self.murl(&format!("/api/media/{}/display", encode_seg(&item.id)))
        } else {
            self.file_url(&item.id, false)
        }
    }

    /// `/api/media/:id/display?max=4096` (zoomed-in viewer)
    pub fn zoom_url(&self, id: &str) -> String {
        self.murl(&format!("/api/media/{}/display?max=4096", encode_seg(id)))
    }

    /// `/api/media/:id/display`: the full-size original, converted when it isn't web-decodable
    pub fn full_url(&self, id: &str) -> String {
        self.murl(&format!("/api/media/{}/display", encode_seg(id)))
    }

    /// `/api/media/:id/stream` (transcoded video, optionally from `start_at` seconds)
    pub fn stream_url(&self, id: &str, start_at: f64) -> String {
        let t = if start_at > 0.0 {
            format!("?t={start_at:.2}")
        } else {
            String::new()
        };
        self.murl(&format!("/api/media/{}/stream{t}", encode_seg(id)))
    }

    /// `/api/media/:id/preview` (short hover preview clip)
    pub fn preview_url(&self, id: &str) -> String {
        self.murl(&format!("/api/media/{}/preview", encode_seg(id)))
    }

    /// `/api/media/:id/filmstrip?n=&h=`
    pub fn filmstrip_url(&self, id: &str, n: u32, h: u32) -> String {
        self.murl(&format!(
            "/api/media/{}/filmstrip?n={n}&h={h}",
            encode_seg(id)
        ))
    }

    /// `/api/download?ids=…` (zip of several items)
    pub fn download_url(&self, ids: &[String], name: Option<&str>) -> String {
        let mut qs = Qs::new();
        qs.str("ids", &ids.join(",")).opt("name", name);
        self.murl(&format!("/api/download{qs}"))
    }

    /// `ws://…/api/events` (or `wss://`)
    pub fn events_url(&self) -> String {
        let b = &self.inner.base;
        if let Some(rest) = b.strip_prefix("https") {
            format!("wss{rest}/api/events")
        } else {
            format!("ws{}/api/events", b.strip_prefix("http").unwrap_or(b))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let c = Client::new("http://127.0.0.1:47213/some/path?x=1").unwrap();
        assert_eq!(c.base(), "http://127.0.0.1:47213");
        assert_eq!(
            c.thumb_url("a b/1", 300),
            "http://127.0.0.1:47213/api/media/a%20b%2F1/thumb?s=320"
        );
        assert_eq!(
            c.thumb_url("7", 500),
            "http://127.0.0.1:47213/api/media/7/thumb?s=640"
        );
        assert_eq!(
            c.stream_url("7", 1.5),
            "http://127.0.0.1:47213/api/media/7/stream?t=1.50"
        );
        assert_eq!(c.events_url(), "ws://127.0.0.1:47213/api/events");
        c.set_device_auth(Some(DeviceAuth {
            device_id: "d1".into(),
            token: "t&k".into(),
        }));
        assert_eq!(
            c.file_url("7", true),
            "http://127.0.0.1:47213/api/media/7/file?download=1&pdev=d1&ptok=t%26k"
        );
        assert_eq!(
            c.zoom_url("7"),
            "http://127.0.0.1:47213/api/media/7/display?max=4096&pdev=d1&ptok=t%26k"
        );
        let s = Client::new("https://peek.example.org").unwrap();
        assert_eq!(s.events_url(), "wss://peek.example.org/api/events");
        assert!(Client::new("ftp://x").is_err());
    }
}
