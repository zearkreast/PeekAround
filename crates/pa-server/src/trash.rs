//! The folder trash: deletes without an OS trash land in
//! `<library folder>/.peekaround-trash/<stamp>/<rel>` (`routes/files.rs` `trash`, keep the layout in
//! sync). Listing, restoring, deleting for good and the purge of old entries.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::ctx::Ctx;
use crate::db::FolderRow;
use crate::error::{ApiResult, HttpError};
use crate::p;
use crate::scanner::TRASH_DIR;

/// Entries older than this are deleted for good.
pub const KEEP_DAYS: i64 = 30;

/// One trashed file.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashItem {
    /// `<folderId>:<stamp>/<rel>`
    pub id: String,
    pub folder_id: String,
    pub folder_name: String,
    /// where it was, relative to the library folder
    pub rel: String,
    pub name: String,
    /// "image" | "video" | "audio" | "archive", None for other files
    pub kind: Option<&'static str>,
    pub size: i64,
    /// ms epoch
    pub deleted_at: i64,
    /// ms epoch when the purge removes it
    pub purge_at: i64,
}

/// "2026-11-15T12-00-00-000Z" (an ISO string with `:` and `.` replaced) → ms epoch
pub fn stamp_ms(stamp: &str) -> Option<i64> {
    let b = stamp.as_bytes();
    if b.len() != 24 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[23] != b'Z' {
        return None;
    }
    let n = |a: usize, z: usize| stamp.get(a..z)?.parse::<i64>().ok();
    let (y, mo, d) = (n(0, 4)?, n(5, 7)?, n(8, 10)?);
    let (h, mi, s, ms) = (n(11, 13)?, n(14, 16)?, n(17, 19)?, n(20, 23)?);
    let days = pa_core::js::days_from_civil(y, mo, d);
    Some(days * pa_core::DAY_MS + ((h * 60 + mi) * 60 + s) * 1000 + ms)
}

/// Library folders on this machine (no phones, MEGA links or zip sessions).
fn local_folders(ctx: &Ctx) -> Vec<FolderRow> {
    ctx.db
        .all("SELECT * FROM folders WHERE temp = 0", p![], FolderRow::from_row)
        .unwrap_or_default()
        .into_iter()
        .filter(|f| !f.is_device() && !f.is_remote())
        .collect()
}

fn root_of(f: &FolderRow) -> PathBuf {
    PathBuf::from(&f.path).join(TRASH_DIR)
}

fn walk(dir: &Path, rel: &str, out: &mut Vec<(String, std::fs::Metadata)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let r = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        let Ok(md) = std::fs::symlink_metadata(e.path()) else { continue };
        if md.is_dir() {
            walk(&e.path(), &r, out);
        } else if md.is_file() {
            out.push((r, md));
        }
    }
}

/// Everything in the trash, newest first.
pub fn list(ctx: &Ctx) -> Vec<TrashItem> {
    let mut out = Vec::new();
    for f in local_folders(ctx) {
        let Ok(stamps) = std::fs::read_dir(root_of(&f)) else { continue };
        for s in stamps.flatten() {
            let stamp = s.file_name().to_string_lossy().into_owned();
            let Some(at) = stamp_ms(&stamp) else { continue };
            let mut files = Vec::new();
            walk(&s.path(), "", &mut files);
            for (rel, md) in files {
                let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
                out.push(TrashItem {
                    id: format!("{}:{stamp}/{rel}", f.id),
                    folder_id: f.id.to_string(),
                    folder_name: f.name.clone(),
                    kind: pa_core::kind_of(&name).map(|k| k.as_str()),
                    name,
                    rel,
                    size: md.len() as i64,
                    deleted_at: at,
                    purge_at: at + KEEP_DAYS * pa_core::DAY_MS,
                });
            }
        }
    }
    out.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at).then_with(|| a.rel.cmp(&b.rel)));
    out
}

/// A parsed id: the folder, the trashed file and its stamp directory.
pub struct Entry {
    pub folder: FolderRow,
    pub abs: PathBuf,
    pub stamp_dir: PathBuf,
    pub rel: String,
}

/// Resolve an id from a request; only paths inside a trash stamp directory are accepted.
pub fn entry(ctx: &Ctx, id: &str) -> ApiResult<Entry> {
    let bad = || HttpError::new(400, "Invalid trash id");
    let (fid, rest) = id.split_once(':').ok_or_else(bad)?;
    let (stamp, rel) = rest.split_once('/').ok_or_else(bad)?;
    stamp_ms(stamp).ok_or_else(bad)?;
    let rel = crate::paths::normalize_rel(Some(rel))?;
    if rel.is_empty() {
        return Err(bad());
    }
    let folder = local_folders(ctx)
        .into_iter()
        .find(|f| f.id.to_string() == fid)
        .ok_or_else(|| HttpError::new(404, "Folder not found"))?;
    let stamp_dir = root_of(&folder).join(stamp);
    let abs = rel.split('/').fold(stamp_dir.clone(), |p, s| p.join(s));
    if !crate::paths::is_inside(&stamp_dir, &abs) || !abs.is_file() {
        return Err(HttpError::new(404, "Not in the trash"));
    }
    Ok(Entry { folder, abs, stamp_dir, rel })
}

/// Remove empty directories from `from` up to and including the stamp directory, then the trash
/// root when it is empty.
fn prune(from: &Path, stamp_dir: &Path) {
    let mut d = from.to_path_buf();
    while crate::paths::is_inside(stamp_dir, &d) {
        if std::fs::remove_dir(&d).is_err() {
            return;
        }
        match d.parent() {
            Some(p) => d = p.to_path_buf(),
            None => return,
        }
    }
    if let Some(root) = stamp_dir.parent() {
        let _ = std::fs::remove_dir(root);
    }
}

/// Put an entry back where it was ("name (1).ext" when that name is taken). Returns the new rel.
pub fn restore(e: &Entry) -> ApiResult<String> {
    let dest = e.rel.split('/').fold(PathBuf::from(&e.folder.path), |p, s| p.join(s));
    let parent = dest.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(&e.folder.path));
    std::fs::create_dir_all(&parent)?;
    let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let fin = crate::paths::publish_unique(&e.abs, &parent, &name)?;
    if let Some(p) = e.abs.parent() {
        prune(p, &e.stamp_dir);
    }
    Ok(crate::paths::rel_of(&e.folder.path, &fin))
}

/// Delete an entry for good.
pub fn delete(e: &Entry) -> ApiResult<()> {
    std::fs::remove_file(&e.abs)?;
    if let Some(p) = e.abs.parent() {
        prune(p, &e.stamp_dir);
    }
    Ok(())
}

/// Delete every stamp directory older than `cutoff` (ms epoch); `i64::MAX` empties the trash.
/// Returns the number of stamp directories removed.
pub fn purge(ctx: &Ctx, cutoff: i64) -> usize {
    let mut n = 0;
    for f in local_folders(ctx) {
        let root = root_of(&f);
        let Ok(stamps) = std::fs::read_dir(&root) else { continue };
        for s in stamps.flatten() {
            let stamp = s.file_name().to_string_lossy().into_owned();
            if stamp_ms(&stamp).is_some_and(|t| t < cutoff) && std::fs::remove_dir_all(s.path()).is_ok() {
                n += 1;
            }
        }
        let _ = std::fs::remove_dir(&root);
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps() {
        let iso = crate::routes::iso_string(1_763_208_000_123).replace([':', '.'], "-");
        assert_eq!(stamp_ms(&iso), Some(1_763_208_000_123));
        for bad in ["", "2026-11-15", "2026-11-15T12-00-00-000", "x026-11-15T12-00-00-000Z"] {
            assert_eq!(stamp_ms(bad), None, "{bad}");
        }
    }
}
