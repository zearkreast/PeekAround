//! Who may see linked-phone folders (mirrors `devices/access.ts`, docs/link-protocol.md):
//!  - server mode (Docker): every client;
//!  - desktop: the local window (requests that did not come through the LAN listener) and paired
//!    phones authenticating with `Authorization: Bearer` + `X-Peek-Device` (or `?pdev=&ptok=`);
//!    other LAN clients see no device folders at all;
//!  - a phone never sees its own folders (`X-Peek-Device` = that device).
//!
//! Requests carrying the hub's internal token (ffmpeg reading /file over loopback) see everything.

use crate::hub::{device_header, DeviceHub};
use pa_jobs::HttpError;
use serde_json::Value;
use std::collections::HashMap;

/// Desktop app (local window + optional LAN share) or headless server (Docker).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    Desktop,
    Server,
}

/// The request facts the access rules look at.
#[derive(Debug, Clone, Default)]
pub struct RequestInfo {
    /// header values by lower-case name (`authorization`, `x-peek-device`, `x-peek-internal`)
    pub headers: HashMap<String, String>,
    /// query parameters (`pdev`, `ptok`, `folderId`, `ids`, `sourceId`)
    pub query: HashMap<String, String>,
    /// arrived through the desktop's LAN listener (`req.raw.peekLan`)
    pub lan: bool,
}

impl RequestInfo {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

/// (`X-Peek-Device`, `Authorization`) of a request: headers always win; only when neither header
/// is present, `?pdev=<id>&ptok=<token>` is used (media URLs of `<img>` / `<video>` on a phone).
pub fn device_auth_of(req: &RequestInfo) -> (Option<String>, Option<String>) {
    let (dev, auth) = (req.header("x-peek-device"), req.header("authorization"));
    if dev.is_some() || auth.is_some() {
        return (dev.map(str::to_string), auth.map(str::to_string));
    }
    match (req.query.get("pdev"), req.query.get("ptok")) {
        (Some(d), Some(t)) if !d.is_empty() && !t.is_empty() => {
            (Some(d.clone()), Some(format!("Bearer {t}")))
        }
        _ => (None, None),
    }
}

/// `Viewer`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Viewer {
    /// may see device folders at all
    pub devices: bool,
    /// the asking phone (never sees its own folders)
    pub self_id: Option<String>,
}

pub fn viewer_of(hub: &DeviceHub, mode: RunMode, req: &RequestInfo) -> Viewer {
    let (dev, auth) = device_auth_of(req);
    let self_id = device_header(dev.as_deref());
    if req.header("x-peek-internal") == Some(hub.internal_token.as_str()) {
        return Viewer {
            devices: true,
            self_id: None,
        };
    }
    if mode != RunMode::Desktop || !req.lan {
        return Viewer {
            devices: true,
            self_id,
        };
    }
    Viewer {
        devices: hub.paired_device(dev.as_deref(), auth.as_deref()).is_some(),
        self_id,
    }
}

/// approve / reject / forget: on the desktop only the local window; a device never approves itself
pub fn require_approver(
    mode: RunMode,
    req: &RequestInfo,
    device_id: Option<&str>,
) -> Result<(), HttpError> {
    if mode == RunMode::Desktop && req.lan {
        return Err(HttpError::new(
            403,
            "Pairing can only be approved on the PC",
        ));
    }
    if let Some(id) = device_id {
        let (dev, _) = device_auth_of(req);
        if device_header(dev.as_deref()).as_deref() == Some(id) {
            return Err(HttpError::new(403, "A device cannot approve itself"));
        }
    }
    Ok(())
}

/// pending pairings (and their codes) only for viewers that may approve them; also decides who
/// receives `pair` events
pub fn can_see_pairing(mode: RunMode, req: &RequestInfo) -> bool {
    mode != RunMode::Desktop || !req.lan
}

/// folder ids / media ids a request refers to (query, route params, JSON body)
pub fn referenced(
    query: &HashMap<String, String>,
    route: &str,
    id_param: Option<&str>,
    body: Option<&Value>,
) -> (Vec<i64>, Vec<i64>) {
    let mut folders = Vec::new();
    let mut media = Vec::new();
    let add = |arr: &mut Vec<i64>, v: Option<&Value>| {
        let Some(v) = v else { return };
        if v.is_null() || v.as_str() == Some("") {
            return;
        }
        let n = pa_core::js::number_of(v);
        if n.is_finite() {
            arr.push(n as i64);
        }
    };
    let q = |k: &str| query.get(k).map(|s| Value::String(s.clone()));
    add(&mut folders, q("folderId").as_ref());
    add(&mut media, q("sourceId").as_ref());
    if let Some(ids) = query.get("ids") {
        for id in ids.split(',') {
            add(&mut media, Some(&Value::String(id.to_string())));
        }
    }
    if route.starts_with("/api/media/:id") {
        add(
            &mut media,
            id_param.map(|s| Value::String(s.to_string())).as_ref(),
        );
    }
    if let Some(b) = body.filter(|b| b.is_object()) {
        add(&mut folders, b.get("folderId"));
        add(&mut folders, b.get("to").and_then(|t| t.get("folderId")));
        add(
            &mut folders,
            b.get("target").and_then(|t| t.get("folderId")),
        );
        if let Some(Value::Array(refs)) = b.get("refs") {
            for r in refs {
                add(&mut folders, r.get("folderId"));
            }
        }
        add(&mut media, b.get("id"));
        add(&mut media, b.get("sourceId"));
        add(&mut media, b.get("audioId"));
        if let Some(Value::Array(ids)) = b.get("ids") {
            for id in ids {
                add(&mut media, Some(id));
            }
        }
        if let Some(Value::Array(items)) = b.get("items") {
            for it in items {
                add(&mut media, it.get("id"));
            }
        }
    }
    (folders, media)
}

/// `deviceAccessHook` (preHandler of every `/api/` route except `/api/devices/*`): hide device
/// folders / media from viewers that may not see them (404, as if absent). `folder_of_media`
/// maps a media id to its folder id (`SELECT folder_id FROM media WHERE id = ?`).
pub fn check_access(
    hub: &DeviceHub,
    mode: RunMode,
    url: &str,
    req: &RequestInfo,
    refs: &(Vec<i64>, Vec<i64>),
    folder_of_media: &dyn Fn(i64) -> Option<i64>,
) -> Result<(), HttpError> {
    if !hub.has_folders() {
        return Ok(());
    }
    if !url.starts_with("/api/") || url.starts_with("/api/devices/") {
        return Ok(());
    }
    let v = viewer_of(hub, mode, req);
    if v.devices && v.self_id.is_none() {
        return Ok(());
    }
    let denied =
        |owner: Option<String>| owner.is_some_and(|o| !v.devices || Some(&o) == v.self_id.as_ref());
    for f in &refs.0 {
        if denied(hub.folder_owner(*f)) {
            return Err(HttpError::new(404, "Folder not found"));
        }
    }
    for m in &refs.1 {
        if let Some(fid) = folder_of_media(*m) {
            if denied(hub.folder_owner(fid)) {
                return Err(HttpError::new(404, "Media not found"));
            }
        }
    }
    Ok(())
}
