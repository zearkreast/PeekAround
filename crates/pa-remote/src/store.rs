//! What MEGA folders need from the server (library DB, settings, scanner, import plumbing).

use async_trait::async_trait;
use pa_core::{FolderRef, Library, MediaKind, Playback};
use pa_jobs::HttpError;
use pa_meta::ExtractedMeta;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// A `folders` row with `remote` set (MEGA folder).
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteFolderRow {
    pub id: i64,
    pub name: String,
    /// `remote://mega/<linkId>[-<subId>]`
    pub path: String,
    /// `folders.remote` (RemoteSpec JSON, holds the key)
    pub remote: String,
    /// `folders.remote_status` (RemoteFolderStatus JSON)
    pub remote_status: Option<String>,
}

/// A media row of a MEGA folder (what the service reads).
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteMedia {
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
}

/// The new folder of `POST /api/remote/folders` (`lib.addRemoteFolder`).
#[derive(Debug, Clone)]
pub struct NewRemoteFolder {
    pub path: String,
    pub name: String,
    /// RemoteSpec JSON (`folders.remote`)
    pub remote: String,
    /// RemoteFolderStatus JSON (`folders.remote_status`)
    pub status: String,
}

/// Library DB side of MEGA folders. Never called while the service holds a lock; implementations
/// may call back into [`crate::RemoteService::info`] (e.g. from `folder_ref`).
pub trait RemoteStore: Send + Sync + 'static {
    /// `SELECT * FROM folders WHERE remote IS NOT NULL ORDER BY id`
    fn remote_folders(&self) -> Vec<RemoteFolderRow>;
    /// any folder row by id: Ok(None) = exists but is not a MEGA folder; Err 404 'Folder not found'
    fn remote_folder(&self, id: i64) -> Result<Option<RemoteFolderRow>, HttpError>;
    /// `UPDATE folders SET remote_status = ? WHERE id = ?`
    fn set_remote_status(&self, id: i64, status_json: &str);
    /// `SELECT count(*) total, coalesce(sum(CASE WHEN meta_state = 3 OR thumb_state = 0 THEN 1 ELSE 0 END), 0) todo
    /// FROM media WHERE folder_id = ? AND kind IN ('image','video')` → (total, todo)
    fn indexing_counts(&self, folder_id: i64) -> (i64, i64);
    /// settings table (`remoteQuota`)
    fn get_setting(&self, key: &str) -> Option<Value>;
    fn set_setting(&self, key: &str, value: Value);
    /// `lib.library(id)` (404 unknown)
    fn library(&self, id: &str) -> Result<Library, HttpError>;
    /// `lib.createLibrary({ name, icon: 'cloud' })` → library id
    fn create_library(&self, name: &str) -> Result<String, HttpError>;
    /// `lib.addRemoteFolder(libraryId, …)` → folder id (emits the `folders` event like TS)
    fn add_remote_folder(&self, library_id: &str, f: &NewRemoteFolder) -> Result<i64, HttpError>;
    /// `lib.folderRef(lib.folderRow(id))` (its `remote` is `RemoteService::info`)
    fn folder_ref(&self, folder_id: i64) -> Result<FolderRef, HttpError>;
    /// `SELECT m.* FROM media m JOIN folders f ON f.id = m.folder_id WHERE f.path = ? AND m.dir = ? AND m.name = ? LIMIT 1`
    fn media_at(&self, folder_path: &str, dir: &str, name: &str) -> Option<RemoteMedia>;
    /// `SELECT * FROM media WHERE id = ?`
    fn media(&self, id: i64) -> Option<RemoteMedia>;
    /// rows waiting for metadata: `f.remote IS NOT NULL AND m.kind IN ('image','video') AND
    /// (m.meta_state = 3 OR (m.meta_state = 1 AND m.meta_v < META_V)) ORDER BY m.taken_local DESC LIMIT 24`
    fn pending_meta(&self) -> Vec<RemoteMedia>;
    /// enrichment result: `UPDATE media SET width = COALESCE(?, width), height = COALESCE(?, height),
    /// duration = COALESCE(?, duration), playback = ?, extra = COALESCE(?, extra), meta_state = 1,
    /// meta_v = META_V, <GROUP_SET_SQL of group_columns(row, meta, geo, homeTz)> WHERE id = ?`
    fn apply_meta(&self, row: &RemoteMedia, meta: &ExtractedMeta);
    /// enrichment failed: `UPDATE media SET meta_state = 1, meta_v = META_V WHERE id = ?`
    fn meta_failed(&self, id: i64);
}

/// The server pieces the MEGA import job (`remote/import.ts`) needs.
#[async_trait]
pub trait ImportHost: Send + Sync + 'static {
    /// `requireWritable` (403 in read-only mode)
    fn require_writable(&self) -> Result<(), HttpError>;
    /// `Settings.zipAction` ("extract" | "temp" | anything else = keep)
    fn zip_action(&self) -> String;
    /// `folderPath(folderId, dir)`: a local, writable folder → (folder id, folder root, abs dir)
    /// (404 unknown, 403 phone / MEGA folders, 400 bad dir)
    fn target(&self, folder_id: &str, dir: &str) -> Result<(i64, PathBuf, PathBuf), HttpError>;
    /// `cfg.tempDir`
    fn temp_dir(&self) -> PathBuf;
    /// `lib.createTempFolder(content, name, Settings.tempSessionHours)` → folder id (root = `content`)
    async fn create_temp_folder(&self, content: &Path, name: &str) -> Result<i64, HttpError>;
    /// `lib.deleteTemp(row, tempDir)`
    async fn delete_temp(&self, folder_id: i64);
    /// `scanner.upsertFile(folderId, rel)`
    async fn upsert_file(&self, folder_id: i64, rel: &str);
    /// `scanner.scanFolder(folderId, rel)`
    async fn scan_folder(&self, folder_id: i64, rel: &str);
    /// `extractZip(part, dest)`
    async fn extract_zip(&self, zip: &Path, dest: &Path) -> Result<(), HttpError>;
    /// `zipToTemp(part, name)` → the new temp session's folder id
    async fn zip_to_temp(&self, zip: &Path, name: &str) -> Result<String, HttpError>;
}
