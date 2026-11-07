//! Route helpers (mirrors `routes/util.ts`).

use crate::ctx::Ctx;
use crate::db::{FolderRow, MediaRow};
use crate::error::{ApiResult, HttpError};
use crate::http::Req;
use crate::library;
use crate::p;
use std::path::{Path, PathBuf};

pub const DEVICE_READONLY: &str = "Files on a linked device can only be viewed and copied";
pub const REMOTE_READONLY: &str =
    "Files in a MEGA link can only be viewed and copied to this computer";

/// media row by client id (404)
pub fn media_row(ctx: &Ctx, id: &str) -> ApiResult<MediaRow> {
    let Some(n) = library::num_id(id) else {
        return Err(HttpError::new(404, "Media not found"));
    };
    media_row_n(ctx, n)
}

/// media row by id (404)
pub fn media_row_n(ctx: &Ctx, id: i64) -> ApiResult<MediaRow> {
    ctx.db
        .get(
            "SELECT * FROM media WHERE id = ?",
            p![id],
            MediaRow::from_row,
        )?
        .ok_or_else(|| HttpError::new(404, "Media not found"))
}

/// media row by a JSON id value (`Number(v)`)
pub fn media_row_v(ctx: &Ctx, v: Option<&serde_json::Value>) -> ApiResult<MediaRow> {
    let n = v.map(pa_core::js::number_of).unwrap_or(f64::NAN);
    if !n.is_finite() || n.fract() != 0.0 {
        return Err(HttpError::new(404, "Media not found"));
    }
    media_row_n(ctx, n as i64)
}

/// 403 for device and remote folders
pub fn require_local_folder(folder: &FolderRow, message: &str) -> ApiResult<()> {
    if folder.is_device() {
        return Err(HttpError::new(403, message));
    }
    if folder.is_remote() {
        return Err(HttpError::new(403, REMOTE_READONLY));
    }
    Ok(())
}

/// local file of a media row (403 for device / remote media)
pub fn media_abs(ctx: &Ctx, r: &MediaRow) -> ApiResult<(FolderRow, String)> {
    let folder = library::folder_row(ctx, r.folder_id)?;
    require_local_folder(&folder, DEVICE_READONLY)?;
    let a = crate::scanner::abs(&folder, &r.dir, &r.name);
    Ok((folder, a))
}

/// Source of a media row (also device / remote pseudo paths).
pub struct Source {
    pub folder: FolderRow,
    pub abs: String,
    pub device: bool,
    pub remote: bool,
}

/// `mediaSource`
pub fn media_source(ctx: &Ctx, r: &MediaRow) -> ApiResult<Source> {
    let folder = library::folder_row(ctx, r.folder_id)?;
    let abs = crate::scanner::abs(&folder, &r.dir, &r.name);
    Ok(Source {
        device: folder.is_device(),
        remote: folder.is_remote(),
        folder,
        abs,
    })
}

/// `folderPath(ctx, folderId, rel)`
pub fn folder_path(ctx: &Ctx, folder_id: &str, rel: &str) -> ApiResult<(FolderRow, PathBuf)> {
    let folder = library::folder_row_str(ctx, folder_id)?;
    require_local_folder(&folder, DEVICE_READONLY)?;
    let abs = crate::paths::resolve_in_root(&folder.path, rel)?;
    Ok((folder, abs))
}

/// `folderPath` with a JSON folder id
pub fn folder_path_v(
    ctx: &Ctx,
    folder_id: Option<&serde_json::Value>,
    rel: &str,
) -> ApiResult<(FolderRow, PathBuf)> {
    folder_path(
        ctx,
        &folder_id
            .map(crate::http::js_string)
            .unwrap_or_else(|| "undefined".into()),
        rel,
    )
}

/// rename, or copy + delete across devices
pub async fn move_file(src: &Path, dest: &Path) -> std::io::Result<()> {
    if let Some(p) = dest.parent() {
        tokio::fs::create_dir_all(p).await?;
    }
    match tokio::fs::rename(src, dest).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            let (s, d) = (src.to_path_buf(), dest.to_path_buf());
            tokio::task::spawn_blocking(move || copy_tree(&s, &d, false))
                .await
                .map_err(std::io::Error::other)??;
            if tokio::fs::metadata(src)
                .await
                .map(|m| m.is_dir())
                .unwrap_or(false)
            {
                tokio::fs::remove_dir_all(src).await
            } else {
                tokio::fs::remove_file(src).await
            }
        }
        Err(e) => Err(e),
    }
}

/// `fs.cp(src, dest, { recursive, errorOnExist, force: false, preserveTimestamps })`
pub fn copy_tree(src: &Path, dest: &Path, preserve_times: bool) -> std::io::Result<()> {
    let md = std::fs::metadata(src)?;
    if md.is_dir() {
        std::fs::create_dir_all(dest)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            copy_tree(&e.path(), &dest.join(e.file_name()), preserve_times)?;
        }
    } else {
        if dest.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("EEXIST: file already exists, cp '{}'", dest.display()),
            ));
        }
        std::fs::copy(src, dest)?;
        if preserve_times {
            if let (Ok(a), Ok(m)) = (md.accessed(), md.modified()) {
                let f = std::fs::File::options().write(true).open(dest)?;
                let _ = f.set_times(std::fs::FileTimes::new().set_accessed(a).set_modified(m));
            }
        }
    }
    Ok(())
}

/// 403 in read-only mode
pub fn require_writable(ctx: &Ctx) -> ApiResult<()> {
    if ctx.cfg.readonly {
        return Err(HttpError::new(
            403,
            "PeekAround is running in read-only mode",
        ));
    }
    Ok(())
}

/// desktop "share on LAN": remote clients never reach the host file system by path
pub fn require_local(req: &Req) -> ApiResult<()> {
    if req.is_lan() {
        return Err(HttpError::new(403, "Not allowed from the network"));
    }
    Ok(())
}

/// `dir ? dir/name : name`
pub fn rel(dir: &str, name: &str) -> String {
    pa_core::join_rel(dir, name)
}

/// `relOfRow`
pub fn rel_of_row(r: &MediaRow) -> String {
    pa_core::join_rel(&r.dir, &r.name)
}

/// Playback as stored
pub fn playback_str(p: pa_core::Playback) -> &'static str {
    match p {
        pa_core::Playback::Direct => "direct",
        pa_core::Playback::Convert => "convert",
        pa_core::Playback::Transcode => "transcode",
    }
}
