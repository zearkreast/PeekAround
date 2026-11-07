//! Test harness: an in-memory library (RemoteStore + MediaRowSink + ThumbCache + ImportHost), a
//! MEGA backend wrapper that counts / injects faults around the REAL pa-mega client talking to
//! pa-mega's fake MEGA server, a mock backend for import tests, the loopback `/api/media/:id/file`
//! endpoint (video probes / frames) and a tiny stored-zip writer / reader.
#![allow(dead_code, clippy::type_complexity)]

#[path = "../../../pa-mega/tests/common/mod.rs"]
pub mod fake;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::{Path as AxPath, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use bytes::Bytes;
use pa_core::{FolderRef, Library, MediaKind, Playback};
use pa_jobs::library::{
    BaseThumb, ListScope, ListedFile, MediaRowSink, ThumbCache, ThumbRow, ThumbScope,
};
use pa_jobs::paths::{join_rel, normalize_rel};
use pa_jobs::{HttpError, JobService, MemoryEvents};
use pa_mega::{ByteStream, MegaError, MegaLink, MegaNode};
use pa_meta::ExtractedMeta;
use pa_remote::{
    ImportHost, MegaBackend, NewRemoteFolder, Priority, RemoteDeps, RemoteFolderRow, RemoteMedia,
    RemoteService, RemoteStore,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const META_V: i64 = pa_meta::META_V;

// ───────────────────────── in-memory library ─────────────────────────

#[derive(Debug, Clone)]
pub struct Row {
    pub id: i64,
    pub folder_id: i64,
    pub dir: String,
    pub name: String,
    pub ext: String,
    pub kind: MediaKind,
    pub size: i64,
    pub mtime: i64,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub duration: Option<f64>,
    pub playback: Playback,
    pub extra: Option<String>,
    pub meta_state: i64,
    pub meta_v: i64,
    pub thumb_state: i64,
    pub thumbhash: Option<String>,
    pub seen: bool,
    pub taken_local: i64,
    pub time_src: u8,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub make: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Folder {
    pub id: i64,
    pub name: String,
    pub path: String,
    pub library_id: Option<String>,
    pub remote: Option<String>,
    pub remote_status: Option<String>,
    pub temp: bool,
    pub source_name: Option<String>,
}

#[derive(Default)]
pub struct Db {
    pub folders: BTreeMap<i64, Folder>,
    pub next_folder: i64,
    pub media: BTreeMap<i64, Row>,
    pub next_media: i64,
    pub dirs: HashSet<(i64, String)>,
    pub libraries: Vec<(String, String)>,
    pub settings: HashMap<String, Value>,
    pub thumbs: HashMap<String, (u32, u32)>,
}

pub struct Lib {
    pub db: Mutex<Db>,
    pub svc: OnceLock<Weak<RemoteService>>,
    pub temp_dir: PathBuf,
    pub zip_action: Mutex<String>,
}

fn group_row(r: &mut Row, m: &ExtractedMeta) {
    let g = pa_meta::group_columns(
        &pa_meta::RowInfo {
            name: r.name.clone(),
            dir: r.dir.clone(),
            ext: r.ext.clone(),
            kind: r.kind.as_str().into(),
            mtime: r.mtime,
        },
        m,
        None,
        "UTC",
    );
    r.taken_local = g.taken_local;
    r.time_src = g.time_src;
    r.lat = g.lat;
    r.lon = g.lon;
    r.make = g.make;
}

impl Lib {
    pub fn new(temp_dir: PathBuf) -> Arc<Self> {
        Arc::new(Lib {
            db: Mutex::new(Db {
                next_folder: 1,
                next_media: 1,
                ..Default::default()
            }),
            svc: OnceLock::new(),
            temp_dir,
            zip_action: Mutex::new("keep".into()),
        })
    }
    pub fn svc(&self) -> Arc<RemoteService> {
        self.svc.get().and_then(Weak::upgrade).unwrap()
    }
    /// a local library folder; returns its id
    pub fn add_local(&self, name: &str, root: &Path) -> i64 {
        let mut db = self.db.lock().unwrap();
        let id = db.next_folder;
        db.next_folder += 1;
        db.libraries.push((format!("L{id}"), name.into()));
        db.folders.insert(
            id,
            Folder {
                id,
                name: name.into(),
                path: root.to_string_lossy().into(),
                library_id: Some(format!("L{id}")),
                remote: None,
                remote_status: None,
                temp: false,
                source_name: None,
            },
        );
        id
    }
    pub fn rows_of(&self, folder_id: i64) -> Vec<Row> {
        let mut v: Vec<Row> = self
            .db
            .lock()
            .unwrap()
            .media
            .values()
            .filter(|r| r.folder_id == folder_id)
            .cloned()
            .collect();
        v.sort_by(|a, b| (a.dir.as_str(), a.name.as_str()).cmp(&(b.dir.as_str(), b.name.as_str())));
        v
    }
    pub fn row(&self, folder_id: i64, name: &str) -> Row {
        self.rows_of(folder_id)
            .into_iter()
            .find(|r| r.name == name)
            .unwrap_or_else(|| panic!("no row {name}"))
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
    pub fn folder_row(&self, id: i64) -> RemoteFolderRow {
        self.remote_folder(id).unwrap().unwrap()
    }
    pub fn delete_folder(&self, id: i64) {
        let mut db = self.db.lock().unwrap();
        db.folders.remove(&id);
        db.media.retain(|_, r| r.folder_id != id);
        db.dirs.retain(|d| d.0 != id);
    }
    pub fn temps(&self) -> Vec<Folder> {
        self.db
            .lock()
            .unwrap()
            .folders
            .values()
            .filter(|f| f.temp)
            .cloned()
            .collect()
    }
    fn remote_media(r: &Row) -> RemoteMedia {
        RemoteMedia {
            id: r.id,
            folder_id: r.folder_id,
            dir: r.dir.clone(),
            name: r.name.clone(),
            ext: r.ext.clone(),
            kind: r.kind,
            size: r.size,
            mtime: r.mtime,
            width: r.width,
            height: r.height,
            duration: r.duration,
            playback: r.playback,
        }
    }
    fn insert_local(&self, folder_id: i64, rel: &str, size: i64) {
        let mut db = self.db.lock().unwrap();
        let (dir, name) = match rel.rfind('/') {
            Some(i) => (rel[..i].to_string(), rel[i + 1..].to_string()),
            None => (String::new(), rel.to_string()),
        };
        let Some(kind) = pa_core::kind_of(&name) else {
            return;
        };
        if db
            .media
            .values()
            .any(|r| r.folder_id == folder_id && r.dir == dir && r.name == name)
        {
            return;
        }
        let id = db.next_media;
        db.next_media += 1;
        let ext = pa_core::ext_of(&name);
        db.media.insert(
            id,
            Row {
                id,
                folder_id,
                dir,
                name,
                ext,
                kind,
                size,
                mtime: 0,
                width: None,
                height: None,
                duration: None,
                playback: Playback::Direct,
                extra: None,
                meta_state: 1,
                meta_v: META_V,
                thumb_state: 0,
                thumbhash: None,
                seen: true,
                taken_local: 0,
                time_src: 0,
                lat: None,
                lon: None,
                make: None,
            },
        );
    }
}

impl RemoteStore for Lib {
    fn remote_folders(&self) -> Vec<RemoteFolderRow> {
        let db = self.db.lock().unwrap();
        db.folders
            .values()
            .filter(|f| f.remote.is_some())
            .map(|f| RemoteFolderRow {
                id: f.id,
                name: f.name.clone(),
                path: f.path.clone(),
                remote: f.remote.clone().unwrap(),
                remote_status: f.remote_status.clone(),
            })
            .collect()
    }
    fn remote_folder(&self, id: i64) -> Result<Option<RemoteFolderRow>, HttpError> {
        let db = self.db.lock().unwrap();
        let f = db
            .folders
            .get(&id)
            .ok_or_else(|| HttpError::new(404, "Folder not found"))?;
        Ok(f.remote.clone().map(|remote| RemoteFolderRow {
            id,
            name: f.name.clone(),
            path: f.path.clone(),
            remote,
            remote_status: f.remote_status.clone(),
        }))
    }
    fn set_remote_status(&self, id: i64, status_json: &str) {
        if let Some(f) = self.db.lock().unwrap().folders.get_mut(&id) {
            f.remote_status = Some(status_json.into());
        }
    }
    fn indexing_counts(&self, folder_id: i64) -> (i64, i64) {
        let db = self.db.lock().unwrap();
        let rows: Vec<&Row> = db
            .media
            .values()
            .filter(|r| {
                r.folder_id == folder_id && matches!(r.kind, MediaKind::Image | MediaKind::Video)
            })
            .collect();
        (
            rows.len() as i64,
            rows.iter()
                .filter(|r| r.meta_state == 3 || r.thumb_state == 0)
                .count() as i64,
        )
    }
    fn get_setting(&self, key: &str) -> Option<Value> {
        self.db.lock().unwrap().settings.get(key).cloned()
    }
    fn set_setting(&self, key: &str, value: Value) {
        self.db.lock().unwrap().settings.insert(key.into(), value);
    }
    fn library(&self, id: &str) -> Result<Library, HttpError> {
        let name = self
            .db
            .lock()
            .unwrap()
            .libraries
            .iter()
            .find(|l| l.0 == id)
            .map(|l| l.1.clone())
            .ok_or_else(|| HttpError::new(404, "Library not found"))?;
        Ok(Library {
            id: id.into(),
            name,
            color: "#000".into(),
            icon: "cloud".into(),
            sort: 0,
            folders: vec![],
            media_count: 0,
            cover_id: None,
            options: None,
        })
    }
    fn create_library(&self, name: &str) -> Result<String, HttpError> {
        let mut db = self.db.lock().unwrap();
        let id = format!("R{}", db.libraries.len() + 1);
        db.libraries.push((id.clone(), name.into()));
        Ok(id)
    }
    fn add_remote_folder(&self, library_id: &str, f: &NewRemoteFolder) -> Result<i64, HttpError> {
        let mut db = self.db.lock().unwrap();
        let id = db.next_folder;
        db.next_folder += 1;
        db.folders.insert(
            id,
            Folder {
                id,
                name: f.name.clone(),
                path: f.path.clone(),
                library_id: Some(library_id.into()),
                remote: Some(f.remote.clone()),
                remote_status: Some(f.status.clone()),
                temp: false,
                source_name: None,
            },
        );
        Ok(id)
    }
    fn folder_ref(&self, folder_id: i64) -> Result<FolderRef, HttpError> {
        let f = self
            .db
            .lock()
            .unwrap()
            .folders
            .get(&folder_id)
            .cloned()
            .ok_or_else(|| HttpError::new(404, "Folder not found"))?;
        let remote = self
            .remote_folder(folder_id)?
            .map(|row| self.svc().info(&row).ok());
        let media_count = self.rows_of(folder_id).len() as i64;
        Ok(FolderRef {
            id: f.id.to_string(),
            path: f.path,
            name: f.name,
            library_id: f.library_id,
            temp: f.temp,
            expires_at: None,
            source_name: f.source_name,
            media_count,
            device: None,
            remote,
        })
    }
    fn media_at(&self, folder_path: &str, dir: &str, name: &str) -> Option<RemoteMedia> {
        let db = self.db.lock().unwrap();
        let fid = db.folders.values().find(|f| f.path == folder_path)?.id;
        db.media
            .values()
            .find(|r| r.folder_id == fid && r.dir == dir && r.name == name)
            .map(Self::remote_media)
    }
    fn media(&self, id: i64) -> Option<RemoteMedia> {
        self.db
            .lock()
            .unwrap()
            .media
            .get(&id)
            .map(Self::remote_media)
    }
    fn pending_meta(&self) -> Vec<RemoteMedia> {
        let db = self.db.lock().unwrap();
        let mut v: Vec<&Row> = db
            .media
            .values()
            .filter(|r| {
                db.folders
                    .get(&r.folder_id)
                    .is_some_and(|f| f.remote.is_some())
            })
            .filter(|r| {
                matches!(r.kind, MediaKind::Image | MediaKind::Video)
                    && (r.meta_state == 3 || (r.meta_state == 1 && r.meta_v < META_V))
            })
            .collect();
        v.sort_by_key(|r| std::cmp::Reverse(r.taken_local));
        v.into_iter().take(24).map(Self::remote_media).collect()
    }
    fn apply_meta(&self, row: &RemoteMedia, m: &ExtractedMeta) {
        let mut db = self.db.lock().unwrap();
        let Some(r) = db.media.get_mut(&row.id) else {
            return;
        };
        r.width = m.width.or(r.width);
        r.height = m.height.or(r.height);
        r.duration = m.duration.or(r.duration);
        r.playback = m.playback;
        if let Some(e) = &m.extra {
            r.extra = Some(serde_json::to_string(e).unwrap());
        }
        r.meta_state = 1;
        r.meta_v = META_V;
        group_row(r, m);
    }
    fn meta_failed(&self, id: i64) {
        if let Some(r) = self.db.lock().unwrap().media.get_mut(&id) {
            r.meta_state = 1;
            r.meta_v = META_V;
        }
    }
}

impl MediaRowSink for Lib {
    fn skip_name(&self, _name: &str) -> bool {
        false
    }
    fn skip_file(&self, _folder_id: i64, _rel: &str, _size: i64) -> bool {
        false
    }
    fn skip_dir(&self, _folder_id: i64, _rel: &str) -> bool {
        false
    }
    fn list_begin(&self, scope: &ListScope) -> i64 {
        let mut db = self.db.lock().unwrap();
        for r in db
            .media
            .values_mut()
            .filter(|r| r.folder_id == scope.folder_id)
        {
            r.seen = false;
        }
        db.media.keys().max().copied().unwrap_or(0)
    }
    fn list_upsert(&self, folder_id: i64, files: &[ListedFile]) -> usize {
        let mut db = self.db.lock().unwrap();
        let mut changed = 0;
        for f in files {
            if let Some(r) = db
                .media
                .values_mut()
                .find(|r| r.folder_id == folder_id && r.dir == f.dir && r.name == f.name)
            {
                r.seen = true;
                if r.size == f.size && r.mtime == f.mtime {
                    continue;
                }
                r.size = f.size;
                r.mtime = f.mtime;
                r.width = None;
                r.height = None;
                r.duration = None;
                r.extra = None;
                r.meta_state = f.meta_state;
                r.thumb_state = 0;
                r.thumbhash = None;
                let m = f.meta.clone();
                group_row(r, &m);
                changed += 1;
                continue;
            }
            let id = db.next_media;
            db.next_media += 1;
            let mut r = Row {
                id,
                folder_id,
                dir: f.dir.clone(),
                name: f.name.clone(),
                ext: f.ext.clone(),
                kind: f.kind,
                size: f.size,
                mtime: f.mtime,
                width: None,
                height: None,
                duration: None,
                playback: f.meta.playback,
                extra: None,
                meta_state: f.meta_state,
                meta_v: META_V,
                thumb_state: 0,
                thumbhash: None,
                seen: true,
                taken_local: 0,
                time_src: 0,
                lat: None,
                lon: None,
                make: None,
            };
            group_row(&mut r, &f.meta);
            db.media.insert(id, r);
            changed += 1;
        }
        changed
    }
    fn list_abort(&self, scope: &ListScope) {
        for r in self
            .db
            .lock()
            .unwrap()
            .media
            .values_mut()
            .filter(|r| r.folder_id == scope.folder_id)
        {
            r.seen = true;
        }
    }
    fn list_finish(&self, scope: &ListScope, dirs: &[String], max_before: i64) -> (usize, usize) {
        let mut db = self.db.lock().unwrap();
        let before = db.media.len();
        db.media
            .retain(|_, r| r.folder_id != scope.folder_id || r.seen);
        let removed = before - db.media.len();
        db.dirs.retain(|d| d.0 != scope.folder_id);
        for d in dirs {
            db.dirs.insert((scope.folder_id, d.clone()));
        }
        let added = db
            .media
            .values()
            .filter(|r| r.folder_id == scope.folder_id && r.id > max_before)
            .count();
        (added, removed)
    }
}

#[async_trait]
impl ThumbCache for Lib {
    async fn generate_base(
        &self,
        abs: &str,
        _media_id: i64,
    ) -> Result<Option<BaseThumb>, HttpError> {
        let bytes = self.svc().thumb(abs, 320, Priority::Low).await?;
        let r = tokio::task::spawn_blocking(move || {
            let img = pa_media::decode_bytes(&bytes, pa_media::DecodeOptions::default()).ok()?;
            pa_media::render::render_base(&img, pa_media::ThumbQuality::Normal).ok()
        })
        .await
        .unwrap();
        Ok(r.map(|b| {
            self.db
                .lock()
                .unwrap()
                .thumbs
                .insert(abs.to_string(), (b.width, b.height));
            BaseThumb {
                thumbhash: b.thumbhash,
                width: b.width,
                height: b.height,
            }
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
        if let Some(r) = self.db.lock().unwrap().media.get_mut(&media_id) {
            r.thumbhash = thumbhash.map(str::to_string);
            r.thumb_state = state;
            r.width = r.width.or(width.map(f64::from));
            r.height = r.height.or(height.map(f64::from));
        }
    }
    fn pending_thumbs(&self, scope: ThumbScope<'_>) -> Vec<ThumbRow> {
        assert_eq!(scope, ThumbScope::Remote);
        let db = self.db.lock().unwrap();
        let mut v: Vec<&Row> = db
            .media
            .values()
            .filter(|r| {
                db.folders
                    .get(&r.folder_id)
                    .is_some_and(|f| f.remote.is_some())
                    && r.thumb_state == 0
                    && matches!(r.kind, MediaKind::Image | MediaKind::Video)
            })
            .collect();
        v.sort_by_key(|r| std::cmp::Reverse(r.taken_local));
        v.into_iter()
            .take(16)
            .map(|r| ThumbRow {
                id: r.id,
                folder_id: r.folder_id,
                dir: r.dir.clone(),
                name: r.name.clone(),
                kind: r.kind,
                width: r.width,
                height: r.height,
                folder_path: db.folders[&r.folder_id].path.clone(),
            })
            .collect()
    }
}

#[async_trait]
impl ImportHost for Lib {
    fn require_writable(&self) -> Result<(), HttpError> {
        Ok(())
    }
    fn zip_action(&self) -> String {
        self.zip_action.lock().unwrap().clone()
    }
    fn target(&self, folder_id: &str, dir: &str) -> Result<(i64, PathBuf, PathBuf), HttpError> {
        let id: i64 = folder_id
            .parse()
            .map_err(|_| HttpError::new(404, "Folder not found"))?;
        let f = self
            .db
            .lock()
            .unwrap()
            .folders
            .get(&id)
            .cloned()
            .ok_or_else(|| HttpError::new(404, "Folder not found"))?;
        if f.remote.is_some() {
            return Err(HttpError::new(403, pa_remote::REMOTE_READONLY));
        }
        let rel = normalize_rel(dir)?;
        let root = PathBuf::from(&f.path);
        let abs = if rel.is_empty() {
            root.clone()
        } else {
            root.join(&rel)
        };
        Ok((id, root, abs))
    }
    fn temp_dir(&self) -> PathBuf {
        self.temp_dir.clone()
    }
    async fn create_temp_folder(&self, content: &Path, name: &str) -> Result<i64, HttpError> {
        let mut db = self.db.lock().unwrap();
        let id = db.next_folder;
        db.next_folder += 1;
        db.folders.insert(
            id,
            Folder {
                id,
                name: name.into(),
                path: content.to_string_lossy().into(),
                library_id: None,
                remote: None,
                remote_status: None,
                temp: true,
                source_name: Some(name.into()),
            },
        );
        Ok(id)
    }
    async fn delete_temp(&self, folder_id: i64) {
        let path = self
            .db
            .lock()
            .unwrap()
            .folders
            .get(&folder_id)
            .map(|f| PathBuf::from(&f.path));
        self.delete_folder(folder_id);
        if let Some(p) = path {
            // <temp>/<uuid>/content
            let _ = tokio::fs::remove_dir_all(p.parent().unwrap_or(&p)).await;
        }
    }
    async fn upsert_file(&self, folder_id: i64, rel: &str) {
        let root = self.db.lock().unwrap().folders[&folder_id].path.clone();
        let size = std::fs::metadata(Path::new(&root).join(rel))
            .map(|m| m.len() as i64)
            .unwrap_or(0);
        self.insert_local(folder_id, rel, size);
    }
    async fn scan_folder(&self, folder_id: i64, rel: &str) {
        let root = PathBuf::from(&self.db.lock().unwrap().folders[&folder_id].path);
        let mut stack = vec![root.join(rel)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let r = pa_jobs::paths::rel_posix(&root, &p);
                    self.insert_local(
                        folder_id,
                        &r,
                        e.metadata().map(|m| m.len() as i64).unwrap_or(0),
                    );
                }
            }
        }
    }
    async fn extract_zip(&self, zip: &Path, dest: &Path) -> Result<(), HttpError> {
        let data = tokio::fs::read(zip)
            .await
            .map_err(|e| HttpError::new(500, e.to_string()))?;
        let entries = read_zip(&data).ok_or_else(|| HttpError::new(400, "Not a valid zip file"))?;
        for (name, bytes) in entries {
            let p = dest.join(&name);
            tokio::fs::create_dir_all(p.parent().unwrap())
                .await
                .unwrap();
            tokio::fs::write(&p, bytes).await.unwrap();
        }
        Ok(())
    }
    async fn zip_to_temp(&self, zip: &Path, name: &str) -> Result<String, HttpError> {
        let content = self.temp_dir.join(pa_jobs::random_uuid()).join("content");
        if let Err(e) = self.extract_zip(zip, &content).await {
            let _ = tokio::fs::remove_dir_all(content.parent().unwrap()).await;
            return Err(e);
        }
        let id = self.create_temp_folder(&content, name).await?;
        Ok(id.to_string())
    }
}

/// a stored (uncompressed) zip of (name, data)
pub fn write_zip(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, data) in entries {
        out.extend_from_slice(&0x04034b50u32.to_le_bytes());
        out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(&0u32.to_le_bytes()); // crc (unchecked here)
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
    }
    out.extend_from_slice(&0x06054b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 18]);
    out
}

pub fn read_zip(d: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    let mut i = 0;
    let le32 = |b: &[u8], i: usize| -> Option<u32> {
        Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
    };
    let le16 = |b: &[u8], i: usize| -> Option<usize> {
        Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?) as usize)
    };
    loop {
        match le32(d, i)? {
            0x04034b50 => {
                let size = le32(d, i + 18)? as usize;
                let nlen = le16(d, i + 26)?;
                let xlen = le16(d, i + 28)?;
                let name = String::from_utf8(d.get(i + 30..i + 30 + nlen)?.to_vec()).ok()?;
                let start = i + 30 + nlen + xlen;
                out.push((name, d.get(start..start + size)?.to_vec()));
                i = start + size;
            }
            0x06054b50 => return Some(out),
            _ => return None,
        }
    }
}

// ───────────────────────── MEGA backends ─────────────────────────

#[derive(Default)]
pub struct Stats {
    pub loads: AtomicUsize,
    pub reads: AtomicUsize,
    /// full reads (0..size-1) by node id
    pub full_by: Mutex<HashMap<String, usize>>,
    /// "name:type" of every attribute request
    pub attrs: Mutex<Vec<String>>,
    pub offline: AtomicBool,
    /// read() streams never yield until canceled (MEGA accepted the request, sends nothing)
    pub stall: AtomicBool,
    pub started: AtomicUsize,
    pub aborted: AtomicUsize,
    /// node name → timestamp override (the fake gives every node the same `ts`)
    pub ts: Mutex<HashMap<String, i64>>,
}

/// The real client + counters + fault injection.
pub struct Probe {
    pub inner: Arc<dyn MegaBackend>,
    pub stats: Arc<Stats>,
}

fn fix_ts(n: &mut MegaNode, ts: &HashMap<String, i64>) {
    if let Some(t) = ts.get(&n.name) {
        n.timestamp = Some(*t);
    }
    for c in &mut n.children {
        fix_ts(c, ts);
    }
}

struct Stalled {
    cancel: CancellationToken,
    stats: Arc<Stats>,
    fut: Option<std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>>,
}

impl futures_util::Stream for Stalled {
    type Item = Result<Bytes, MegaError>;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        if self.fut.is_none() {
            let c = self.cancel.clone();
            self.fut = Some(Box::pin(async move { c.cancelled().await }));
        }
        match self.fut.as_mut().unwrap().as_mut().poll(cx) {
            std::task::Poll::Ready(()) => {
                self.stats.aborted.fetch_add(1, Ordering::SeqCst);
                self.fut = Some(Box::pin(std::future::pending()));
                std::task::Poll::Ready(Some(Err(MegaError::Aborted)))
            }
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

#[async_trait]
impl MegaBackend for Probe {
    async fn load(&self, link: &MegaLink) -> Result<MegaNode, MegaError> {
        self.stats.loads.fetch_add(1, Ordering::SeqCst);
        if self.stats.offline.load(Ordering::SeqCst) {
            return Err(MegaError::Network("fetch failed".into()));
        }
        let mut n = self.inner.load(link).await?;
        fix_ts(&mut n, &self.stats.ts.lock().unwrap());
        Ok(n)
    }
    fn read(
        &self,
        link: &MegaLink,
        node: &MegaNode,
        start: u64,
        end: u64,
        cancel: CancellationToken,
    ) -> Result<ByteStream, MegaError> {
        self.stats.reads.fetch_add(1, Ordering::SeqCst);
        if self.stats.stall.load(Ordering::SeqCst) {
            self.stats.started.fetch_add(1, Ordering::SeqCst);
            return Ok(Box::pin(Stalled {
                cancel,
                stats: self.stats.clone(),
                fut: None,
            }));
        }
        if start == 0 && end + 1 == node.size {
            *self
                .stats
                .full_by
                .lock()
                .unwrap()
                .entry(node.id.clone())
                .or_default() += 1;
        }
        self.inner.read(link, node, start, end, cancel)
    }
    fn download(
        &self,
        link: &MegaLink,
        node: &MegaNode,
        cancel: CancellationToken,
    ) -> Result<ByteStream, MegaError> {
        self.inner.download(link, node, cancel)
    }
    async fn attr(
        &self,
        link: &MegaLink,
        node: &MegaNode,
        kind: u32,
        cancel: &CancellationToken,
    ) -> Result<Option<Vec<u8>>, MegaError> {
        self.stats
            .attrs
            .lock()
            .unwrap()
            .push(format!("{}:{kind}", node.name));
        self.inner.attr(link, node, kind, cancel).await
    }
    fn clear_download_urls(&self) {
        self.inner.clear_download_urls()
    }
}

// ───────────────────────── service + loopback ─────────────────────────

pub struct Env {
    pub lib: Arc<Lib>,
    pub svc: Arc<RemoteService>,
    pub events: Arc<MemoryEvents>,
    pub jobs: JobService,
    pub base: String,
    pub tmp: PathBuf,
}

pub fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pa-remote-{tag}-{}", rand::random::<u32>()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub fn ffmpeg() -> Arc<pa_media::Ffmpeg> {
    let f = pa_media::Ffmpeg::new("ffmpeg", Some("ffprobe".into()));
    let ok = std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok();
    f.set_available(ok);
    f.set_probe_available(ok);
    Arc::new(f)
}

pub async fn start(tag: &str, mega: Arc<dyn MegaBackend>) -> Env {
    let tmp = scratch(tag);
    let lib = Lib::new(tmp.join("temp"));
    let events = Arc::new(MemoryEvents::default());
    let jobs = JobService::new(events.clone(), tmp.join("temp"));
    let svc = RemoteService::new(RemoteDeps {
        store: lib.clone(),
        rows: lib.clone(),
        thumbs: lib.clone(),
        events: events.clone(),
        import: lib.clone(),
        jobs: jobs.clone(),
        ffmpeg: ffmpeg(),
        mega,
        cache_dir: tmp.join("cache/remote"),
        cache_bytes: 512 * 1024 * 1024,
    });
    svc.meta_gap_ms.store(0, Ordering::SeqCst);
    let _ = lib.svc.set(Arc::downgrade(&svc));
    // the loopback /api/media/:id/file (what ffmpeg reads for probes and frames)
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let app = Router::new()
        .route("/api/media/{id}/file", axum::routing::get(file).head(file))
        .with_state(lib.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    svc.set_loopback(&base, "internal-token");
    Env {
        lib,
        svc,
        events,
        jobs,
        base,
        tmp,
    }
}

async fn file(
    State(lib): State<Arc<Lib>>,
    AxPath(id): AxPath<i64>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    let Some(row) = lib.media(id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let folder = lib.folder_row(row.folder_id);
    let range = headers.get("range").and_then(|v| v.to_str().ok());
    match lib
        .svc()
        .serve_file(&row, &folder, method == Method::HEAD, range, false)
        .await
    {
        Ok(rep) => {
            let mut b = Response::builder().status(rep.status);
            for (k, v) in &rep.headers {
                b = b.header(k, v);
            }
            let body = match rep.body {
                Some(s) => Body::from_stream(s),
                None => Body::empty(),
            };
            b.body(body).unwrap()
        }
        Err(e) => {
            let mut r = (
                StatusCode::from_u16(e.status).unwrap(),
                axum::Json(json!({ "error": e.message })),
            )
                .into_response();
            if let Some(s) = e.retry_after {
                r.headers_mut()
                    .insert("retry-after", s.to_string().parse().unwrap());
            }
            r
        }
    }
}

pub async fn until<T>(mut f: impl FnMut() -> Option<T>, ms: u64) -> T {
    let t0 = std::time::Instant::now();
    loop {
        if let Some(v) = f() {
            return v;
        }
        if t0.elapsed() > Duration::from_millis(ms) {
            panic!("timeout");
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

pub fn sha(b: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(b))
}

pub fn jpeg(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb(rgb)));
    pa_media::encode::jpeg(&img, pa_media::encode::JpegOptions::new(90)).unwrap()
}

pub fn png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(w, h, image::Rgb(rgb)));
    pa_media::encode::png(&img).unwrap()
}

pub fn fixture_photo() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../pa-meta/tests/files/library/Fotoğraflar/2024 Yaz Tatili/IMG_2400.jpg"
    ))
    .unwrap()
}

/// a short H.264 + AAC mp4 (portrait 180x320, 3 s) with a location tag; None without ffmpeg / x264
pub fn make_video(dir: &Path) -> Option<Vec<u8>> {
    let out = dir.join("clip.mp4");
    let ok = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=180x320:rate=25,noise=alls=30:allf=t",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440",
            "-t",
            "3",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-qp",
            "16",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-metadata",
            "location=+41.0082+028.9784/",
            "-movflags",
            "+faststart+use_metadata_tags",
        ])
        .arg(&out)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    ok.then(|| std::fs::read(&out).ok()).flatten()
}

pub fn rel_join(dir: &str, name: &str) -> String {
    join_rel(dir, name)
}
