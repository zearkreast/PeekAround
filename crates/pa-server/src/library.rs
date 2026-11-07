//! Libraries, folders, temp sessions (mirrors `library.ts`).

use crate::bail;
use crate::ctx::Ctx;
use crate::db::{query_all, FolderRow};
use crate::error::{ApiResult, HttpError};
use crate::events::now_ms;
use crate::p;
use pa_core::{FolderDevice, FolderRef, Library, LibraryOptions, LibraryStats, ServerEvent};
use std::collections::HashMap;
use std::path::Path;

const PALETTE: [&str; 8] = [
    "#8b5cf6", "#ec4899", "#f97316", "#10b981", "#06b6d4", "#3b82f6", "#eab308", "#ef4444",
];
const ICONS: [&str; 8] = [
    "images", "camera", "film", "heart", "mountain", "plane", "palette", "star",
];

/// `device://` pseudo paths of phone folders
pub const DEVICE_SCHEME: &str = "device://";
/// `remote://` pseudo paths of MEGA folders
pub const REMOTE_SCHEME: &str = "remote://";

/// library options column → LibraryOptions (`parseOptions`)
pub fn parse_options(raw: Option<&str>) -> LibraryOptions {
    let v = raw.and_then(|r| serde_json::from_str::<serde_json::Value>(r).ok());
    pa_core::normalize_library_options(v.as_ref())
}

/// folder row by id (404)
pub fn folder_row(ctx: &Ctx, id: i64) -> ApiResult<FolderRow> {
    ctx.db
        .get(
            "SELECT * FROM folders WHERE id = ?",
            p![id],
            FolderRow::from_row,
        )?
        .ok_or_else(|| HttpError::new(404, "Folder not found"))
}

/// `Number(id)` of a path / body id: NaN → no row
pub fn num_id(s: &str) -> Option<i64> {
    let n = pa_core::js::number(s);
    if n.is_finite() && n.fract() == 0.0 {
        Some(n as i64)
    } else {
        None
    }
}

/// folder row by a client id string (404 when not a number)
pub fn folder_row_str(ctx: &Ctx, id: &str) -> ApiResult<FolderRow> {
    match num_id(id) {
        Some(n) => folder_row(ctx, n),
        None => Err(HttpError::new(404, "Folder not found")),
    }
}

/// every folder (sort, id)
pub fn all_folder_rows(ctx: &Ctx) -> ApiResult<Vec<FolderRow>> {
    Ok(ctx.db.all(
        "SELECT * FROM folders ORDER BY sort, id",
        p![],
        FolderRow::from_row,
    )?)
}

fn count_folder(ctx: &Ctx, id: i64) -> i64 {
    ctx.db
        .int("SELECT count(*) c FROM media WHERE folder_id = ? AND kind IN ('image','video','audio')", p![id])
        .ok()
        .flatten()
        .unwrap_or(0)
}

/// media count per folder
pub fn counts(ctx: &Ctx) -> ApiResult<HashMap<i64, i64>> {
    let rows = ctx.db.all(
        "SELECT folder_id, count(*) c FROM media WHERE kind IN ('image','video','audio') GROUP BY folder_id",
        p![],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
    )?;
    Ok(rows.into_iter().collect())
}

/// `folderRef`
pub fn folder_ref(ctx: &Ctx, r: &FolderRow, counts: Option<&HashMap<i64, i64>>) -> FolderRef {
    let media_count = counts
        .map(|c| c.get(&r.id).copied().unwrap_or(0))
        .unwrap_or_else(|| count_folder(ctx, r.id));
    FolderRef {
        id: r.id.to_string(),
        path: r.path.clone(),
        name: r.name.clone(),
        library_id: r.library_id.map(|l| l.to_string()),
        temp: r.temp != 0,
        expires_at: r.expires_at,
        source_name: r.source_name.clone(),
        media_count,
        device: r.device_id.as_ref().filter(|d| !d.is_empty()).map(|d| {
            ctx.devices.lookup(ctx, d).unwrap_or(FolderDevice {
                id: d.clone(),
                name: r.name.clone(),
                online: false,
            })
        }),
        remote: Some(if r.is_remote() {
            ctx.remote.info_with_counts(ctx, r)
        } else {
            None
        }),
    }
}

struct LibRow {
    id: i64,
    name: String,
    color: String,
    icon: String,
    sort: i64,
    cover: Option<i64>,
    options: Option<String>,
}

/// `listLibraries`
pub fn list_libraries(ctx: &Ctx) -> ApiResult<Vec<Library>> {
    let counts = counts(ctx)?;
    let libs = ctx.db.with(|c| {
        query_all(c, "SELECT * FROM libraries ORDER BY sort, id", p![], |r| {
            Ok(LibRow {
                id: r.get("id")?,
                name: crate::db::text(r, "name").unwrap_or_default(),
                color: crate::db::text(r, "color").unwrap_or_default(),
                icon: crate::db::text(r, "icon").unwrap_or_default(),
                sort: crate::db::num_i(r, "sort").unwrap_or(0),
                cover: crate::db::num_i(r, "cover_media_id"),
                options: crate::db::text(r, "options"),
            })
        })
    })?;
    let folders = all_folder_rows(ctx)?;
    Ok(libs
        .into_iter()
        .map(|l| {
            let fs: Vec<FolderRef> = folders
                .iter()
                .filter(|f| f.library_id == Some(l.id))
                .map(|f| folder_ref(ctx, f, Some(&counts)))
                .collect();
            Library {
                id: l.id.to_string(),
                media_count: fs.iter().map(|f| f.media_count).sum(),
                name: l.name,
                color: l.color,
                icon: l.icon,
                sort: l.sort,
                folders: fs,
                cover_id: Some(l.cover.map(|c| c.to_string())),
                options: Some(parse_options(l.options.as_deref())),
            }
        })
        .collect())
}

/// scan options of the library a folder belongs to
pub fn options_of_folder(ctx: &Ctx, folder_id: i64) -> LibraryOptions {
    let r = ctx
        .db
        .get("SELECT l.options FROM folders f JOIN libraries l ON l.id = f.library_id WHERE f.id = ?", p![folder_id], |r| Ok(crate::db::text(r, 0)))
        .ok()
        .flatten()
        .flatten();
    parse_options(r.as_deref())
}

/// new library order
pub fn reorder_libraries(ctx: &Ctx, ids: &[String]) -> ApiResult<()> {
    let all: Vec<i64> = ctx
        .db
        .all("SELECT id FROM libraries ORDER BY sort, id", p![], |r| {
            r.get(0)
        })?;
    let want: Vec<i64> = ids
        .iter()
        .filter_map(|s| num_id(s))
        .filter(|id| all.contains(id))
        .collect();
    let rest: Vec<i64> = all
        .iter()
        .copied()
        .filter(|id| !want.contains(id))
        .collect();
    ctx.db.tx(|c| {
        for (i, id) in want.iter().chain(rest.iter()).enumerate() {
            c.prepare_cached("UPDATE libraries SET sort = ? WHERE id = ?")?
                .execute(rusqlite::params![i as i64, id])?;
        }
        Ok(())
    })?;
    ctx.events.emit(ServerEvent::Folders { status: None });
    Ok(())
}

/// header numbers of a library page
pub fn stats(ctx: &Ctx, id: &str, busy: impl Fn(&[i64]) -> bool) -> ApiResult<LibraryStats> {
    let lib = library(ctx, id)?;
    let ids: Vec<i64> = lib
        .folders
        .iter()
        .filter_map(|f| f.id.parse().ok())
        .collect();
    if ids.is_empty() {
        return Ok(LibraryStats {
            items: 0,
            photos: 0,
            videos: 0,
            audio: 0,
            bytes: 0,
            from: None,
            to: None,
            folders: 0,
            scanned_at: None,
            scanning: false,
            pending: 0,
        });
    }
    let in_list = vec!["?"; ids.len()].join(",");
    let params: Vec<rusqlite::types::Value> = ids
        .iter()
        .map(|i| rusqlite::types::Value::Integer(*i))
        .collect();
    let r = ctx.db.get(
        &format!(
            "SELECT count(*) n, sum(kind = 'image') p, sum(kind = 'video') v, sum(kind = 'audio') a, sum(size) b,
              min(taken_local) f, max(taken_local) t, count(DISTINCT folder_id || '/' || dir) d,
              sum(meta_state = 0 OR (thumb_state = 0 AND kind IN ('image','video'))) q
       FROM media WHERE folder_id IN ({in_list}) AND kind IN ('image','video','audio')"
        ),
        params.clone(),
        |r| {
            use crate::db::num_i;
            Ok([num_i(r, 0), num_i(r, 1), num_i(r, 2), num_i(r, 3), num_i(r, 4), num_i(r, 5), num_i(r, 6), num_i(r, 7), num_i(r, 8)])
        },
    )?;
    let scanned = ctx.db.int(
        &format!("SELECT max(scanned_at) s FROM folders WHERE id IN ({in_list})"),
        params,
    )?;
    let r = r.unwrap_or_default();
    let n = r[0].unwrap_or(0);
    Ok(LibraryStats {
        items: n,
        photos: r[1].unwrap_or(0),
        videos: r[2].unwrap_or(0),
        audio: r[3].unwrap_or(0),
        bytes: r[4].unwrap_or(0),
        from: if n != 0 { r[5] } else { None },
        to: if n != 0 { r[6] } else { None },
        folders: r[7].unwrap_or(0),
        scanned_at: scanned,
        scanning: busy(&ids),
        pending: r[8].unwrap_or(0),
    })
}

/// temp sessions (newest first)
pub fn list_temp(ctx: &Ctx) -> ApiResult<Vec<FolderRef>> {
    let counts = counts(ctx)?;
    let rows = ctx.db.all(
        "SELECT * FROM folders WHERE temp = 1 ORDER BY id DESC",
        p![],
        FolderRow::from_row,
    )?;
    Ok(rows
        .iter()
        .map(|f| folder_ref(ctx, f, Some(&counts)))
        .collect())
}

/// one library (404)
pub fn library(ctx: &Ctx, id: &str) -> ApiResult<Library> {
    list_libraries(ctx)?
        .into_iter()
        .find(|l| l.id == id)
        .ok_or_else(|| HttpError::new(404, "Library not found"))
}

/// `createLibrary`
pub fn create_library(
    ctx: &Ctx,
    name: &str,
    color: Option<String>,
    icon: Option<String>,
    folders: &[String],
) -> ApiResult<Library> {
    let n = ctx
        .db
        .int("SELECT count(*) c FROM libraries", p![])?
        .unwrap_or(0);
    let trimmed = pa_core::js::trim(name);
    let id = ctx.db.insert(
        "INSERT INTO libraries (name, color, icon, sort, created_at) VALUES (?, ?, ?, ?, ?)",
        p![
            if trimmed.is_empty() {
                "Library".to_string()
            } else {
                trimmed.to_string()
            },
            color.unwrap_or_else(|| PALETTE[(n as usize) % PALETTE.len()].to_string()),
            icon.unwrap_or_else(|| ICONS[(n as usize) % ICONS.len()].to_string()),
            n,
            now_ms()
        ],
    )?;
    for p in folders {
        add_folder(ctx, id, p, None)?;
    }
    ctx.events.emit(ServerEvent::Folders { status: None });
    library(ctx, &id.to_string())
}

/// patch of `updateLibrary`
#[derive(Default)]
pub struct LibraryPatch {
    pub name: Option<String>,
    pub color: Option<String>,
    pub icon: Option<String>,
    pub sort: Option<i64>,
    /// None = untouched, Some(None) = automatic, Some(Some(id))
    pub cover_id: Option<Option<String>>,
    /// raw options patch (merged over the current ones, then normalised)
    pub options: Option<serde_json::Value>,
}

/// `updateLibrary` → (library, rescan needed)
pub fn update_library(ctx: &Ctx, id: &str, patch: LibraryPatch) -> ApiResult<(Library, bool)> {
    let lid = num_id(id).ok_or_else(|| HttpError::new(404, "Library not found"))?;
    let lib = ctx
        .db
        .get("SELECT * FROM libraries WHERE id = ?", p![lid], |r| {
            Ok(LibRow {
                id: r.get("id")?,
                name: crate::db::text(r, "name").unwrap_or_default(),
                color: crate::db::text(r, "color").unwrap_or_default(),
                icon: crate::db::text(r, "icon").unwrap_or_default(),
                sort: crate::db::num_i(r, "sort").unwrap_or(0),
                cover: crate::db::num_i(r, "cover_media_id"),
                options: crate::db::text(r, "options"),
            })
        })?
        .ok_or_else(|| HttpError::new(404, "Library not found"))?;
    let mut cover = lib.cover;
    if let Some(c) = patch.cover_id {
        match c.filter(|s| !s.is_empty()) {
            None => cover = None,
            Some(cid) => {
                let m = ctx.db.int(
                    "SELECT m.id FROM media m JOIN folders f ON f.id = m.folder_id WHERE m.id = ? AND f.library_id = ?",
                    p![num_id(&cid).map(|n| n as f64).unwrap_or(f64::NAN), lid],
                )?;
                match m {
                    Some(m) => cover = Some(m),
                    None => bail!(400, "The cover must be an item of this library"),
                }
            }
        }
    }
    let before = parse_options(lib.options.as_deref());
    let options = match &patch.options {
        Some(o) => {
            let mut merged = serde_json::to_value(&before).unwrap_or_default();
            if let (Some(m), Some(o)) = (merged.as_object_mut(), o.as_object()) {
                for (k, v) in o {
                    m.insert(k.clone(), v.clone());
                }
            }
            pa_core::normalize_library_options(Some(&merged))
        }
        None => before.clone(),
    };
    let name = patch
        .name
        .as_deref()
        .map(pa_core::js::trim)
        .filter(|n| !n.is_empty())
        .map(String::from)
        .unwrap_or(lib.name);
    ctx.db.run(
        "UPDATE libraries SET name = ?, color = ?, icon = ?, sort = ?, cover_media_id = ?, options = ? WHERE id = ?",
        p![name, patch.color.unwrap_or(lib.color), patch.icon.unwrap_or(lib.icon), patch.sort.unwrap_or(lib.sort), cover, serde_json::to_string(&options).unwrap_or_default(), lid],
    )?;
    ctx.events.emit(ServerEvent::Folders { status: None });
    let rescan = serde_json::to_string(&before).ok() != serde_json::to_string(&options).ok();
    Ok((library(ctx, &lid.to_string())?, rescan))
}

/// delete a library (folders + media cascade)
pub fn delete_library(ctx: &Ctx, id: &str) -> ApiResult<()> {
    ctx.db.run(
        "DELETE FROM libraries WHERE id = ?",
        p![num_id(id).map(|n| n as f64).unwrap_or(f64::NAN)],
    )?;
    ctx.events.emit(ServerEvent::Folders { status: None });
    Ok(())
}

/// add a host folder to a library
pub fn add_folder(ctx: &Ctx, library_id: i64, p: &str, name: Option<&str>) -> ApiResult<FolderRow> {
    let abs = crate::paths::resolve(p);
    let md = std::fs::metadata(&abs)
        .map_err(|_| HttpError::new(400, format!("Folder does not exist: {}", abs.display())))?;
    if !md.is_dir() {
        bail!(400, "Not a directory");
    }
    let abs_s = abs.to_string_lossy().into_owned();
    if let Some(existing) = ctx.db.get(
        "SELECT * FROM folders WHERE library_id = ? AND path = ?",
        p![library_id, abs_s.clone()],
        FolderRow::from_row,
    )? {
        return Ok(existing);
    }
    let n = ctx
        .db
        .int(
            "SELECT count(*) c FROM folders WHERE library_id = ?",
            p![library_id],
        )?
        .unwrap_or(0);
    let base = abs
        .file_name()
        .map(|b| b.to_string_lossy().into_owned())
        .unwrap_or_default();
    let nm = name
        .map(pa_core::js::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or(if base.is_empty() { abs_s.clone() } else { base });
    let id = ctx.db.insert(
        "INSERT INTO folders (library_id, path, name, temp, sort) VALUES (?, ?, ?, 0, ?)",
        p![library_id, abs_s, nm, n],
    )?;
    ctx.events.emit(ServerEvent::Folders { status: None });
    folder_row(ctx, id)
}

/// remove a folder row (media cascade)
pub fn remove_folder(ctx: &Ctx, id: i64) -> ApiResult<()> {
    ctx.db.run("DELETE FROM folders WHERE id = ?", p![id])?;
    ctx.events.emit(ServerEvent::Folders { status: None });
    Ok(())
}

/// rename a folder (label only)
pub fn rename_folder(ctx: &Ctx, id: i64, name: &str) -> ApiResult<()> {
    ctx.db.run(
        "UPDATE folders SET name = ? WHERE id = ?",
        p![pa_core::js::trim(name), id],
    )?;
    ctx.events.emit(ServerEvent::Folders { status: None });
    Ok(())
}

/// temp session row for an extracted zip
pub fn create_temp_folder(
    ctx: &Ctx,
    abs: &Path,
    source_name: &str,
    hours: f64,
) -> ApiResult<FolderRow> {
    let name = strip_zip_ext(source_name);
    let id = ctx.db.insert(
        "INSERT INTO folders (library_id, path, name, temp, expires_at, source_name) VALUES (NULL, ?, ?, 1, ?, ?)",
        p![abs.to_string_lossy().into_owned(), name, now_ms() + (hours * 3_600_000.0) as i64, source_name],
    )?;
    ctx.events.emit(ServerEvent::Folders { status: None });
    folder_row(ctx, id)
}

/// `name.replace(/\.zip$/i, '')`
pub fn strip_zip_ext(name: &str) -> String {
    if name.len() >= 4 && name.as_bytes()[name.len() - 4..].eq_ignore_ascii_case(b".zip") {
        name[..name.len() - 4].to_string()
    } else {
        name.to_string()
    }
}

/// Remove expired temp sessions (and their files).
pub async fn cleanup_temp(ctx: &Ctx, all: bool) -> ApiResult<()> {
    let rows = if all {
        ctx.db.all(
            "SELECT * FROM folders WHERE temp = 1",
            p![],
            FolderRow::from_row,
        )?
    } else {
        ctx.db.all(
            "SELECT * FROM folders WHERE temp = 1 AND expires_at < ?",
            p![now_ms()],
            FolderRow::from_row,
        )?
    };
    for r in &rows {
        delete_temp(ctx, r).await?;
    }
    if !rows.is_empty() {
        ctx.events.emit(ServerEvent::Folders { status: None });
    }
    Ok(())
}

/// drop a temp session row + its directory (only inside our temp dir)
pub async fn delete_temp(ctx: &Ctx, r: &FolderRow) -> ApiResult<()> {
    ctx.db.run("DELETE FROM folders WHERE id = ?", p![r.id])?;
    let session = crate::paths::resolve(Path::new(&r.path).parent().unwrap_or(Path::new(&r.path)));
    let temp = crate::paths::resolve(&ctx.cfg.temp_dir);
    if crate::paths::is_inside(&temp, &session) && session != temp {
        let _ = tokio::fs::remove_dir_all(&session).await;
    }
    Ok(())
}

/// On first start create one library per configured root.
pub fn ensure_roots(ctx: &Ctx) -> ApiResult<()> {
    if ctx.cfg.roots.is_empty() {
        return Ok(());
    }
    if ctx
        .db
        .int("SELECT count(*) c FROM libraries", p![])?
        .unwrap_or(0)
        > 0
    {
        return Ok(());
    }
    for r in &ctx.cfg.roots {
        let abs = crate::paths::resolve(&r.path);
        if !abs.exists() {
            continue;
        }
        if r.split {
            let mut children: Vec<String> = std::fs::read_dir(&abs)
                .map(|rd| {
                    rd.flatten()
                        .filter(|d| {
                            d.file_type().map(|t| t.is_dir()).unwrap_or(false) || d.path().is_dir()
                        })
                        .map(|d| d.file_name().to_string_lossy().into_owned())
                        .filter(|n| !n.starts_with('.'))
                        .collect()
                })
                .unwrap_or_default();
            children.sort_by(|a, b| pa_core::locale_compare(a, b));
            if !children.is_empty() {
                for c in children {
                    let p = abs.join(&c).to_string_lossy().into_owned();
                    create_library(ctx, &c, None, None, &[p])?;
                }
                continue;
            }
        }
        let base = abs
            .file_name()
            .map(|b| b.to_string_lossy().into_owned())
            .unwrap_or_default();
        let name = r
            .name
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or(if base.is_empty() {
                abs.to_string_lossy().into_owned()
            } else {
                base
            });
        create_library(
            ctx,
            &name,
            None,
            None,
            &[abs.to_string_lossy().into_owned()],
        )?;
    }
    Ok(())
}
