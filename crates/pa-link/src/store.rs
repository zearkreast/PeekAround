//! What the phone link needs from the server: the `devices` table, device folder rows, the media
//! rows filled from `list`, the thumbnail service. pa-server implements these over SQLite.

use pa_core::FolderRef;

/// A row of the `devices` table.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceRow {
    pub id: String,
    pub name: String,
    pub model: String,
    /// sha256(token) in hex
    pub token_hash: Option<String>,
    pub paired: bool,
    /// JSON `{"read":bool,"receive":bool}`
    pub caps: Option<String>,
    pub created_at: i64,
    pub last_seen: i64,
}

/// A `folders` row of a device folder (`device_id` / `device_key` set, path `device://<id>/<key>`).
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceFolderRow {
    pub id: i64,
    pub name: String,
    pub sort: i64,
    pub device_id: String,
    pub device_key: String,
}

/// The `devices` table + device folder rows.
pub trait DeviceStore: Send + Sync + 'static {
    fn device(&self, id: &str) -> Option<DeviceRow>;
    /// `SELECT * FROM devices WHERE paired = 1 ORDER BY last_seen DESC`
    fn paired_devices(&self) -> Vec<DeviceRow>;
    /// approve: INSERT … ON CONFLICT(id) DO UPDATE (name, model, token_hash, paired = 1, caps, last_seen)
    fn save_paired(
        &self,
        id: &str,
        name: &str,
        model: &str,
        token_hash: &str,
        caps: &str,
        now: i64,
    );
    /// online: `UPDATE devices SET name, model, caps, last_seen WHERE id`
    fn update_online(&self, id: &str, name: &str, model: &str, caps: &str, now: i64);
    /// offline: `UPDATE devices SET last_seen WHERE id`
    fn set_last_seen(&self, id: &str, now: i64);
    /// forget: `DELETE FROM devices WHERE id` — its folders, media rows and device_dirs go with it
    fn delete_device(&self, id: &str);

    /// `SELECT * FROM folders WHERE device_id = ? ORDER BY sort, id`
    fn device_folders(&self, device_id: &str) -> Vec<DeviceFolderRow>;
    /// every device folder: (folder id, device id)
    fn all_device_folders(&self) -> Vec<(i64, String)>;
    fn device_folder(&self, folder_id: i64) -> Option<DeviceFolderRow>;
    /// `syncFolders` in one transaction: `update` (id, name, sort), `insert` (key, name, sort) with
    /// path `device://<deviceId>/<key>` (`library_id NULL, temp 0`), `delete` ids. Returns the new ids
    /// of the inserted rows (same order).
    fn sync_device_folders(
        &self,
        device_id: &str,
        update: &[(i64, String, i64)],
        insert: &[(String, String, i64)],
        delete: &[i64],
    ) -> Vec<i64>;
    /// `lib.folderRef(row, counts)` for each folder of the device (DeviceInfo.folders)
    fn folder_refs(&self, device_id: &str) -> Vec<FolderRef>;
}
