//! Linked phones (mirrors the state half of `devices/hub.ts` + `devices/access.ts`): paired devices
//! from the database, token auth, folder ownership and the visibility rules. The live link (WS
//! control channel, reply tunnel, listings, thumbnails through the phone) is the pa-link engine's
//! job.

use crate::ctx::{Ctx, C};
use crate::db::{text, FolderRow};
use crate::error::{ApiResult, HttpError};
use crate::http::Req;
use crate::p;
use pa_core::{DeviceInfo, FolderDevice};
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// The hub (`ctx.devices`): a facade over the pa-link engine.
pub struct DeviceHub {
    /// lets the server feed ffmpeg from its own /file endpoint
    pub internal_token: String,
    /// loopback base URL of the main server
    pub self_url: Mutex<String>,
    pub link: Arc<pa_link::DeviceHub>,
}

fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

fn is_id(s: &str) -> bool {
    (8..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

/// `Authorization: Bearer <token>` (16..128 url-safe chars)
pub fn bearer_of(h: Option<&str>) -> Option<String> {
    let h = h?.trim();
    let rest = h.strip_prefix("Bearer")?;
    if !rest.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }
    let t = rest.trim_start();
    ((16..=128).contains(&t.len())
        && t.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    .then(|| t.to_string())
}

/// device credentials of a request: headers win, else `?pdev=&ptok=` (`deviceAuthOf`)
pub fn device_auth_of(req: &Req) -> (Option<String>, Option<String>) {
    let hd = req.header("x-peek-device");
    let ha = req.header("authorization");
    if hd.is_some() || ha.is_some() {
        return (hd.filter(|d| is_id(d)).map(String::from), bearer_of(ha));
    }
    match (req.q("pdev"), req.q("ptok")) {
        (Some(d), Some(t)) if !d.is_empty() && !t.is_empty() => (
            is_id(d).then(|| d.to_string()),
            bearer_of(Some(&format!("Bearer {t}"))),
        ),
        _ => (None, None),
    }
}

/// Who may see linked-phone folders.
#[derive(Debug, Clone)]
pub struct Viewer {
    pub devices: bool,
    pub this_device: Option<String>,
}

impl DeviceHub {
    pub fn new(link: Arc<pa_link::DeviceHub>) -> Self {
        DeviceHub {
            internal_token: link.internal_token.clone(),
            self_url: Mutex::new(String::new()),
            link,
        }
    }

    /// set once listening (loopback ffmpeg inputs)
    pub fn set_self_url(&self, url: String) {
        self.link.set_self_url(url.clone());
        *self.self_url.lock() = url;
    }

    /// connected now
    pub fn is_online(&self, id: &str) -> bool {
        self.link.is_online(id)
    }

    /// FolderRef.device
    pub fn lookup(&self, _ctx: &Ctx, id: &str) -> Option<FolderDevice> {
        let d = self.link.device_lookup(id);
        Some(FolderDevice {
            id: d.id,
            name: d.name,
            online: d.online,
        })
    }

    /// some folder belongs to a phone
    pub fn has_folders(&self, ctx: &Ctx) -> bool {
        ctx.db
            .int(
                "SELECT 1 FROM folders WHERE device_id IS NOT NULL LIMIT 1",
                p![],
            )
            .ok()
            .flatten()
            .is_some()
    }

    /// device id owning a folder
    pub fn folder_owner(&self, ctx: &Ctx, folder_id: i64) -> Option<String> {
        ctx.db
            .get(
                "SELECT device_id FROM folders WHERE id = ?",
                p![folder_id],
                |r| Ok(text(r, 0)),
            )
            .ok()
            .flatten()
            .flatten()
    }

    /// paired (remembered) + pending devices, except `hide`
    pub fn list(&self, _ctx: &Ctx, hide: Option<&str>) -> ApiResult<Vec<DeviceInfo>> {
        Ok(self.link.list(hide))
    }

    /// phone → server HTTP auth (401 / 403); also `?pdev=&ptok=`
    pub fn authenticate(
        &self,
        ctx: &Ctx,
        auth: &(Option<String>, Option<String>),
    ) -> ApiResult<String> {
        let (Some(id), Some(token)) = auth else {
            return Err(HttpError::new(401, "Device token required"));
        };
        let row = ctx.db.get(
            "SELECT token_hash, paired FROM devices WHERE id = ?",
            p![id.as_str()],
            |r| Ok((text(r, 0), crate::db::num_i(r, 1).unwrap_or(0))),
        )?;
        let Some((Some(hash), paired)) = row else {
            return Err(HttpError::new(401, "Unknown device or invalid token"));
        };
        let want = sha256_hex(token);
        if hash.len() != want.len() || !constant_eq(hash.as_bytes(), want.as_bytes()) {
            return Err(HttpError::new(401, "Unknown device or invalid token"));
        }
        if paired == 0 {
            return Err(HttpError::new(403, "Device is not paired"));
        }
        Ok(id.clone())
    }

    /// non-throwing variant
    pub fn paired_device(
        &self,
        ctx: &Ctx,
        auth: &(Option<String>, Option<String>),
    ) -> Option<String> {
        self.authenticate(ctx, auth).ok()
    }

    /// approve a pending pairing
    pub fn approve(&self, id: &str) -> ApiResult<()> {
        Ok(self.link.approve(id)?)
    }

    /// reject a pending pairing
    pub fn reject(&self, id: &str) -> ApiResult<()> {
        Ok(self.link.reject(id)?)
    }

    /// unpair: drop the token, its folders and their media rows
    pub fn forget(&self, _ctx: &C, id: &str) -> ApiResult<()> {
        Ok(self.link.forget(id)?)
    }

    /// (re)list a device folder — the phone does it when online (`scanner.deviceScan`)
    pub async fn scan(&self, _ctx: &C, folder_id: i64, sub_dir: &str) {
        let _ = self.link.relist(folder_id, sub_dir).await;
    }

    /// `{ deviceId, key }` of a device folder; 503 when offline
    pub fn require_online(&self, folder: &FolderRow) -> ApiResult<(String, String)> {
        let id = folder.device_id.clone().unwrap_or_default();
        if !self.is_online(&id) {
            return Err(HttpError::new(503, "device offline"));
        }
        Ok((id, folder.device_key.clone().unwrap_or_default()))
    }
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// `viewerOf(ctx, req)`
pub fn viewer_of(ctx: &Ctx, req: &Req) -> Viewer {
    let auth = device_auth_of(req);
    let this_device = auth.0.clone();
    if req.header("x-peek-internal") == Some(ctx.devices.internal_token.as_str()) {
        return Viewer {
            devices: true,
            this_device: None,
        };
    }
    if !ctx.cfg.desktop() || !req.is_lan() {
        return Viewer {
            devices: true,
            this_device,
        };
    }
    Viewer {
        devices: ctx.devices.paired_device(ctx, &auth).is_some(),
        this_device,
    }
}

/// pairing approval: on the desktop only the local window may decide
pub fn require_approver(ctx: &Ctx, req: &Req, device_id: Option<&str>) -> ApiResult<()> {
    if ctx.cfg.desktop() && req.is_lan() {
        return Err(HttpError::new(
            403,
            "Pairing can only be approved on the PC",
        ));
    }
    if let Some(d) = device_id {
        if device_auth_of(req).0.as_deref() == Some(d) {
            return Err(HttpError::new(403, "A device cannot approve itself"));
        }
    }
    Ok(())
}

/// pairing requests only reach clients that may approve them
pub fn can_see_pairing(ctx: &Ctx, req: &Req) -> bool {
    !ctx.cfg.desktop() || !req.is_lan()
}

/// true when this viewer may not see linked-phone media (`hideDeviceMedia`)
pub fn hide_device_media(ctx: &Ctx, req: &Req) -> bool {
    if !ctx.devices.has_folders(ctx) {
        return false;
    }
    let v = viewer_of(ctx, req);
    !v.devices || v.this_device.is_some()
}

fn num_of(v: &serde_json::Value) -> Option<i64> {
    match v {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) if s.is_empty() => None,
        other => {
            let n = pa_core::js::number_of(other);
            n.is_finite().then_some(n as i64)
        }
    }
}

/// preHandler: hide device folders / media from viewers that may not see them (404).
pub fn device_access(ctx: &Ctx, req: &Req, route: &str) -> ApiResult<()> {
    let url = req.parts.uri.path();
    if !url.starts_with("/api/") || url.starts_with("/api/devices/") {
        return Ok(());
    }
    if !ctx.devices.has_folders(ctx) {
        return Ok(());
    }
    let v = viewer_of(ctx, req);
    if v.devices && v.this_device.is_none() {
        return Ok(());
    }
    let mut folders: Vec<i64> = Vec::new();
    let mut media: Vec<i64> = Vec::new();
    let qn = |k: &str| {
        req.q(k)
            .and_then(|s| num_of(&serde_json::Value::String(s.to_string())))
    };
    if let Some(f) = qn("folderId") {
        folders.push(f);
    }
    if let Some(m) = qn("sourceId") {
        media.push(m);
    }
    if let Some(ids) = req.q("ids") {
        media.extend(
            ids.split(',')
                .filter_map(|i| num_of(&serde_json::Value::String(i.to_string()))),
        );
    }
    if route.starts_with("/api/media/:id") {
        if let Some(m) = num_of(&serde_json::Value::String(req.param("id").to_string())) {
            media.push(m);
        }
    }
    if let Some(b) = req.body.obj() {
        let add = |arr: &mut Vec<i64>, v: Option<&serde_json::Value>| {
            if let Some(n) = v.and_then(num_of) {
                arr.push(n);
            }
        };
        add(&mut folders, b.get("folderId"));
        add(&mut folders, b.get("to").and_then(|t| t.get("folderId")));
        add(
            &mut folders,
            b.get("target").and_then(|t| t.get("folderId")),
        );
        if let Some(serde_json::Value::Array(refs)) = b.get("refs") {
            for r in refs {
                add(&mut folders, r.get("folderId"));
            }
        }
        add(&mut media, b.get("id"));
        add(&mut media, b.get("sourceId"));
        add(&mut media, b.get("audioId"));
        if let Some(serde_json::Value::Array(ids)) = b.get("ids") {
            for i in ids {
                add(&mut media, Some(i));
            }
        }
        if let Some(serde_json::Value::Array(items)) = b.get("items") {
            for it in items {
                add(&mut media, it.get("id"));
            }
        }
    }
    let denied = |owner: Option<String>| {
        owner.is_some_and(|o| !v.devices || Some(&o) == v.this_device.as_ref())
    };
    for f in folders {
        if denied(ctx.devices.folder_owner(ctx, f)) {
            return Err(HttpError::new(404, "Folder not found"));
        }
    }
    for m in media {
        let fid = ctx
            .db
            .int("SELECT folder_id FROM media WHERE id = ?", p![m])?;
        if let Some(fid) = fid {
            if denied(ctx.devices.folder_owner(ctx, fid)) {
                return Err(HttpError::new(404, "Media not found"));
            }
        }
    }
    Ok(())
}
