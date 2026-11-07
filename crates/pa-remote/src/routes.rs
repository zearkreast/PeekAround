//! The logic of `routes/remote.ts` (rate limits, `requireLocal` and the HTTP glue stay in the
//! server). Bodies are raw JSON: the TS handlers read them without a schema.

use crate::service::{link_is_local, RemoteService};
use crate::store::ImportHost;
use crate::truthy;
use pa_core::{FileRef, Job, RemoteFolderCreated};
use pa_jobs::HttpError;
use serde_json::{json, Value};

fn folder_id(id: &str) -> Result<i64, HttpError> {
    let n = pa_core::js::number(id);
    if n.is_finite() && n.fract() == 0.0 {
        Ok(n as i64)
    } else {
        Err(HttpError::new(404, "Folder not found"))
    }
}

/// `POST /api/remote/inspect {url}` → RemoteInspect (≤ 20k nodes, `truncated`)
pub async fn inspect(
    svc: &RemoteService,
    body: &Value,
) -> Result<pa_mega::RemoteInspect, HttpError> {
    svc.inspect(body).await
}

/// `POST /api/remote/import RemoteImportSpec` → Job
pub fn import(svc: &RemoteService, body: &Value) -> Result<Job, HttpError> {
    svc.start_import(body)
}

/// `POST /api/remote/folders RemoteFolderCreate` → RemoteFolderCreated (`requireWritable` first)
pub async fn create_folder(
    svc: &RemoteService,
    host: &dyn ImportHost,
    body: &Value,
) -> Result<RemoteFolderCreated, HttpError> {
    host.require_writable()?;
    svc.create(body).await
}

/// `POST /api/remote/folders/:id/refresh` → `{ ok: true }`
pub async fn refresh_folder(
    svc: &RemoteService,
    store: &dyn crate::RemoteStore,
    id: &str,
) -> Result<Value, HttpError> {
    let id = folder_id(id)?;
    if store.remote_folder(id)?.is_none() {
        return Err(HttpError::new(400, "Not a remote folder"));
    }
    svc.refresh(id).await?;
    Ok(json!({ "ok": true }))
}

/// `GET /api/remote/folders/:id/link` → `{ url }` — the key is a credential: see [`link_is_local`]
pub fn folder_link(
    svc: &RemoteService,
    store: &dyn crate::RemoteStore,
    id: &str,
    desktop: bool,
    lan: bool,
    loopback_ip: bool,
    forwarded_headers: bool,
) -> Result<Value, HttpError> {
    let row = store
        .remote_folder(folder_id(id)?)?
        .ok_or_else(|| HttpError::new(400, "Not a remote folder"))?;
    if !link_is_local(desktop, lan, loopback_ip, forwarded_headers) {
        return Err(HttpError::new(
            403,
            "The MEGA link can only be shown on this computer",
        ));
    }
    Ok(json!({ "url": svc.link_of(&row)? }))
}

/// `POST /api/remote/copy RemoteCopySpec` → Job (also `/api/fs/move {copy:true}` from a MEGA folder)
pub async fn copy(
    svc: &RemoteService,
    host: &dyn ImportHost,
    body: &Value,
) -> Result<Job, HttpError> {
    host.require_writable()?;
    let to = body.get("to");
    let to_folder = to.and_then(|t| t.get("folderId"));
    if !to_folder.is_some_and(truthy) {
        return Err(HttpError::new(400, "to.folderId required"));
    }
    let to_folder = pa_core::js::to_string_of(to_folder.unwrap());
    let to_dir = match to.and_then(|t| t.get("dir")) {
        None | Some(Value::Null) => String::new(),
        Some(v) => pa_core::js::to_string_of(v),
    };
    host.target(&to_folder, &to_dir)?; // a local, writable target
    let refs: Vec<FileRef> = match body.get("refs") {
        Some(Value::Array(a)) => a
            .iter()
            .map(|r| FileRef {
                folder_id: r
                    .get("folderId")
                    .map(pa_core::js::to_string_of)
                    .unwrap_or_default(),
                rel: r
                    .get("rel")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            })
            .collect(),
        _ => Vec::new(),
    };
    let ids: Vec<String> = match body.get("ids") {
        Some(Value::Array(a)) => a.iter().map(pa_core::js::to_string_of).collect(),
        _ => Vec::new(),
    };
    svc.copy(&refs, &ids, &to_folder, &to_dir).await
}
