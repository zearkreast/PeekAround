//! File routes (mirrors `routes/files.ts`): streaming uploads (original mtime kept), zip extract /
//! temp sessions, desktop imports, mkdir / rename / move / copy / delete (trash), edited copies
//! and the font manager.

use super::util::{
    folder_path, folder_path_v, media_abs, media_row_v, move_file, require_local, require_writable,
};
use crate::ctx::{Ctx, C};
use crate::db::FolderRow;
use crate::error::{ApiResult, HttpError};
use crate::http::{ok_json, BodyScope, JsAccess, Req};
use crate::library;
use crate::p;
use crate::paths::{
    base_of_rel, dir_of_rel, normalize_rel, rel_of, resolve_host_path, resolve_in_root,
    sanitize_name, sanitize_rel, unique_edit_path, unique_path,
};
use crate::router::{h, Router};
use crate::scanner::TRASH_DIR;
use crate::security::Limit::{Fs, Heavy};
use futures_util::StreamExt;
use pa_core::{FileLocation, FolderRef, ServerEvent, UploadResult};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

fn loc(folder: &FolderRow, rel: &str) -> FileLocation {
    FileLocation {
        folder_id: folder.id.to_string(),
        dir: dir_of_rel(rel).to_string(),
        name: base_of_rel(rel).to_string(),
    }
}

/// Turn an extracted / uploaded zip into a temp session.
pub async fn zip_to_temp(ctx: &C, zip_file: &Path, source_name: &str) -> ApiResult<FolderRef> {
    let id = uuid::Uuid::new_v4().to_string();
    let session = ctx.cfg.temp_dir.join(&id);
    let content = session.join("content");
    let (z, c2) = (zip_file.to_path_buf(), content.clone());
    let r = tokio::task::spawn_blocking(move || {
        crate::zip::extract_zip(&z, &c2, crate::zip::MAX_EXTRACT)
    })
    .await
    .map_err(HttpError::internal)?;
    if let Err(e) = r {
        let _ = tokio::fs::remove_dir_all(&session).await;
        return Err(HttpError::new(500, e.to_string()));
    }
    let hours = crate::ctx::setting_f64(&ctx.settings_json(), "tempSessionHours").unwrap_or(24.0);
    let row = library::create_temp_folder(ctx, &content, source_name, hours)?;
    crate::scanner::scan_folder(ctx, row.id, "").await;
    Ok(library::folder_ref(ctx, &row, None))
}

async fn extract(zip: &Path, dest: &Path) -> ApiResult<crate::zip::ExtractResult> {
    let (z, d) = (zip.to_path_buf(), dest.to_path_buf());
    tokio::task::spawn_blocking(move || crate::zip::extract_zip(&z, &d, crate::zip::MAX_EXTRACT))
        .await
        .map_err(HttpError::internal)?
        .map_err(|e| HttpError::new(500, e.to_string()))
}

async fn write_body(req: &mut Req, file: &Path, limit: u64) -> ApiResult<u64> {
    let mut f = tokio::fs::File::create(file).await?;
    let mut s = req.take_raw().into_data_stream();
    let mut n = 0u64;
    while let Some(c) = s.next().await {
        let c = c.map_err(|e| HttpError::new(400, e.to_string()))?;
        n += c.len() as u64;
        if n > limit {
            return Err(HttpError::new(413, "Request body is too large"));
        }
        f.write_all(&c).await?;
    }
    f.flush().await?;
    Ok(n)
}

fn str_of(v: Option<&Value>) -> String {
    v.map(crate::http::js_string).unwrap_or_default()
}

async fn upload(ctx: C, mut req: Req) -> ApiResult<axum::http::Response<axum::body::Body>> {
    require_writable(&ctx)?;
    let (folder, _) = folder_path(&ctx, req.q("folderId").unwrap_or("undefined"), "")?;
    let base_dir = normalize_rel(req.q("dir"))?;
    let rel_path = sanitize_rel(
        req.q("path")
            .filter(|s| !s.is_empty())
            .unwrap_or("upload.bin"),
    )?;
    if rel_path.is_empty() {
        return Err(HttpError::new(400, "Missing path"));
    }
    let name = base_of_rel(&rel_path).to_string();
    let is_zip = pa_core::ext_of(&name) == "zip";
    let mode = if is_zip {
        req.q("zip").unwrap_or("keep").to_string()
    } else {
        "keep".into()
    };
    let limit = ctx.cfg.max_upload_bytes;
    if is_zip && mode != "keep" {
        let scratch_dir = ctx.cfg.temp_dir.join("uploads");
        tokio::fs::create_dir_all(&scratch_dir).await?;
        let scratch = scratch_dir.join(format!("{}.zip", uuid::Uuid::new_v4()));
        let res: ApiResult<axum::http::Response<axum::body::Body>> = async {
            write_body(&mut req, &scratch, limit).await?;
            if mode == "temp" {
                let temp = zip_to_temp(&ctx, &scratch, &name).await?;
                return ok_json(&UploadResult {
                    created: vec![],
                    temp_folder: Some(temp),
                    extracted_dir: None,
                });
            }
            let parent_rel = [base_dir.as_str(), dir_of_rel(&rel_path)]
                .iter()
                .filter(|s| !s.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join("/");
            let parent_abs = resolve_in_root(&folder.path, &parent_rel)?;
            tokio::fs::create_dir_all(&parent_abs).await?;
            let stem = library::strip_zip_ext(&name);
            let dest = unique_path(&parent_abs, if stem.is_empty() { "Archive" } else { &stem });
            let res = extract(&scratch, &dest).await?;
            let dest_rel = rel_of(&folder.path, &dest);
            crate::scanner::scan_folder(&ctx, folder.id, &dest_rel).await;
            ok_json(&UploadResult {
                created: res
                    .files
                    .iter()
                    .map(|f| loc(&folder, &format!("{dest_rel}/{f}")))
                    .collect(),
                temp_folder: None,
                extracted_dir: Some(dest_rel),
            })
        }
        .await;
        let _ = tokio::fs::remove_file(&scratch).await;
        return res;
    }
    let target_rel = [base_dir.as_str(), rel_path.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("/");
    let target_abs = resolve_in_root(&folder.path, &target_rel)?;
    let parent = target_abs
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(&folder.path));
    tokio::fs::create_dir_all(&parent).await?;
    let fname = target_abs
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let final_abs = unique_path(&parent, &fname);
    let part = PathBuf::from(format!(
        "{}.{}.part",
        final_abs.display(),
        hex::encode(rand::random::<[u8; 4]>())
    ));
    let r: ApiResult<()> = async {
        write_body(&mut req, &part, limit).await?;
        let mtime = req.q("mtime").map(pa_core::js::number).unwrap_or(f64::NAN);
        if mtime.is_finite() && mtime > 0.0 && mtime < 4_102_444_800_000.0 {
            let f = std::fs::File::options().write(true).open(&part)?;
            let t = std::time::UNIX_EPOCH + std::time::Duration::from_millis(mtime as u64);
            f.set_times(
                std::fs::FileTimes::new()
                    .set_accessed(std::time::SystemTime::now())
                    .set_modified(t),
            )?;
        }
        tokio::fs::rename(&part, &final_abs).await?;
        Ok(())
    }
    .await;
    if let Err(e) = r {
        let _ = tokio::fs::remove_file(&part).await;
        return Err(e);
    }
    let rel = rel_of(&folder.path, &final_abs);
    crate::scanner::upsert_file(&ctx, folder.id, &rel).await;
    ok_json(&UploadResult {
        created: vec![loc(&folder, &rel)],
        temp_folder: None,
        extracted_dir: None,
    })
}

async fn trash(ctx: &Ctx, folder: &FolderRow, abs: &Path, rel_src: &str) -> ApiResult<()> {
    if folder.temp != 0 {
        if tokio::fs::metadata(abs)
            .await
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            tokio::fs::remove_dir_all(abs).await?;
        } else {
            tokio::fs::remove_file(abs).await?;
        }
        return Ok(());
    }
    if let Some(h) = ctx.cfg.hooks.as_ref().filter(|h| h.has_trash()) {
        if let Some(r) = h.trash(abs) {
            return r.map_err(|e| HttpError::new(500, e.to_string()));
        }
    }
    let stamp = chrono_stamp();
    let dest = rel_src.split('/').fold(
        PathBuf::from(&folder.path).join(TRASH_DIR).join(stamp),
        |p, s| p.join(s),
    );
    move_file(abs, &dest).await?;
    Ok(())
}

/// `new Date().toISOString().replace(/[:.]/g, '-')`
fn chrono_stamp() -> String {
    crate::routes::iso_string(crate::events::now_ms()).replace([':', '.'], "-")
}

async fn exif_from(abs: &str) -> Option<crate::exifw::ExifTags> {
    let a = abs.to_string();
    tokio::task::spawn_blocking(move || {
        let f = std::fs::File::open(&a).ok()?;
        let src = pa_meta::ReaderSource::new(f).ok()?;
        let tags = pa_meta::exifr::parse(&src, &pa_meta::exifr::Mode::Group)?;
        let s = |k: &str| match tags.get(k) {
            Some(v) if !v.is_undef() => {
                let t = v.js_string();
                (!t.is_empty()).then_some(t)
            }
            _ => None,
        };
        Some(crate::exifw::ExifTags {
            software: Some("PeekAround".into()),
            make: s("Make"),
            model: s("Model"),
            date_time_original: s("DateTimeOriginal"),
            date_time_digitized: s("CreateDate"),
            lens_model: s("LensModel"),
        })
    })
    .await
    .ok()
    .flatten()
}

fn list_fonts(ctx: &Ctx) -> ApiResult<Vec<Value>> {
    Ok(ctx.db.all(
        "SELECT * FROM fonts ORDER BY family COLLATE NOCASE, weight",
        p![],
        |r| {
            let id = r.get::<_, i64>("id")?;
            Ok(json!({
                "id": id.to_string(),
                "family": crate::db::text(r, "family"),
                "subfamily": crate::db::text(r, "subfamily"),
                "weight": crate::db::num_i(r, "weight"),
                "style": crate::db::text(r, "style"),
                "format": crate::db::text(r, "format"),
                "url": format!("/api/fonts/{id}/file"),
                "bundled": false,
            }))
        },
    )?)
}

pub fn routes(r: &mut Router) {
    let f = BodyScope::Files;
    r.add("PUT", "/api/upload", Fs, f, h(upload)).body_limit = Some(u64::MAX);
    r.add(
        "POST",
        "/api/zip/peek",
        Heavy,
        f,
        h(|ctx, req| async move {
            let r = media_row_v(&ctx, req.body.prop("id")?)?;
            if r.kind != "archive" {
                return Err(HttpError::new(400, "Not an archive"));
            }
            let (_, a) = media_abs(&ctx, &r)?;
            ok_json(&json!({ "tempFolder": zip_to_temp(&ctx, Path::new(&a), &r.name).await? }))
        }),
    );
    r.add(
        "POST",
        "/api/zip/extract",
        Heavy,
        f,
        h(|ctx, req| async move {
            require_writable(&ctx)?;
            let r = media_row_v(&ctx, req.body.prop("id")?)?;
            if r.kind != "archive" {
                return Err(HttpError::new(400, "Not an archive"));
            }
            let (folder, a) = media_abs(&ctx, &r)?;
            let a = PathBuf::from(a);
            let stem = library::strip_zip_ext(&r.name);
            let dest = unique_path(
                a.parent().unwrap_or(Path::new("/")),
                if stem.is_empty() { "Archive" } else { &stem },
            );
            extract(&a, &dest).await?;
            let dest_rel = rel_of(&folder.path, &dest);
            crate::scanner::scan_folder(&ctx, folder.id, &dest_rel).await;
            ok_json(&json!({ "folderId": folder.id.to_string(), "dir": dest_rel }))
        }),
    );
    r.add(
        "POST",
        "/api/import-local",
        Heavy,
        f,
        h(|ctx, req| async move {
            require_writable(&ctx)?;
            require_local(&req)?;
            if !ctx.cfg.desktop() {
                return Err(HttpError::new(403, "Only available in the desktop app"));
            }
            let fid = req.body.prop("folderId")?;
            let dir = str_of(req.body.opt("dir"));
            let (folder, dest_dir) = folder_path_v(&ctx, fid, &dir)?;
            tokio::fs::create_dir_all(&dest_dir).await?;
            let mut result = UploadResult {
                created: vec![],
                temp_folder: None,
                extracted_dir: None,
            };
            let zip_mode = req
                .body
                .opt("zip")
                .and_then(|v| v.as_str())
                .map(String::from);
            let paths = match req.body.opt("paths") {
                Some(Value::Array(a)) => a.iter().map(crate::http::js_string).collect(),
                _ => vec![],
            };
            for raw in paths {
                let src = resolve_host_path(&raw, &ctx.cfg.host_browse_roots)?;
                let Ok(st) = tokio::fs::metadata(&src).await else {
                    continue;
                };
                let base = sanitize_name(
                    &src.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                );
                if st.is_file()
                    && pa_core::ext_of(&base) == "zip"
                    && zip_mode.as_deref().is_some_and(|z| z != "keep")
                {
                    if zip_mode.as_deref() == Some("temp") {
                        result.temp_folder = Some(zip_to_temp(&ctx, &src, &base).await?);
                    } else {
                        let dest = unique_path(&dest_dir, &library::strip_zip_ext(&base));
                        extract(&src, &dest).await?;
                        let rel = rel_of(&folder.path, &dest);
                        crate::scanner::scan_folder(&ctx, folder.id, &rel).await;
                        result.extracted_dir = Some(rel);
                    }
                    continue;
                }
                let dest = unique_path(&dest_dir, &base);
                let (s2, d2) = (src.clone(), dest.clone());
                tokio::task::spawn_blocking(move || super::util::copy_tree(&s2, &d2, true))
                    .await
                    .map_err(HttpError::internal)??;
                let rel = rel_of(&folder.path, &dest);
                if st.is_dir() {
                    crate::scanner::scan_folder(&ctx, folder.id, &rel).await;
                } else {
                    crate::scanner::upsert_file(&ctx, folder.id, &rel).await;
                }
                result.created.push(loc(&folder, &rel));
            }
            ok_json(&result)
        }),
    );

    // ── file operations ──
    r.add(
        "POST",
        "/api/fs/mkdir",
        Fs,
        f,
        h(|ctx, req| async move {
            require_writable(&ctx)?;
            let fid = req.body.prop("folderId")?;
            let dir = str_of(req.body.opt("dir"));
            let (folder, abs) = folder_path_v(&ctx, fid, &dir)?;
            let name = req
                .body
                .opt("name")
                .map(crate::http::js_string)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "New folder".into());
            let dest = unique_path(&abs, &sanitize_name(&name));
            tokio::fs::create_dir_all(&dest).await?;
            ctx.events.media_changed(
                &folder.id.to_string(),
                &normalize_rel(Some(&dir))?,
                pa_core::MediaReason::Change,
            );
            ok_json(
                &json!({ "folderId": folder.id.to_string(), "dir": rel_of(&folder.path, &dest) }),
            )
        }),
    );
    r.add(
        "POST",
        "/api/fs/rename",
        Fs,
        f,
        h(|ctx, req| async move {
            require_writable(&ctx)?;
            let fid = req.body.prop("folderId")?;
            let relv = str_of(req.body.opt("rel"));
            let (folder, abs) = folder_path_v(&ctx, fid, &relv)?;
            let rel_src = normalize_rel(Some(&relv))?;
            if rel_src.is_empty() {
                return Err(HttpError::new(400, "Cannot rename the folder root"));
            }
            let Ok(st) = tokio::fs::metadata(&abs).await else {
                return Err(HttpError::new(404, "Not found"));
            };
            let new_name = sanitize_name(&str_of(req.body.opt("name")));
            let dest = abs.parent().unwrap_or(Path::new("/")).join(&new_name);
            if dest == abs {
                return ok_json(&json!({ "rel": rel_src }));
            }
            if dest.exists()
                && dest.to_string_lossy().to_lowercase() != abs.to_string_lossy().to_lowercase()
            {
                return Err(HttpError::new(409, "A file with that name already exists"));
            }
            let fav = if st.is_file() {
                ctx.db
                    .int(
                        "SELECT favorite FROM media WHERE folder_id = ? AND dir = ? AND name = ?",
                        p![folder.id, dir_of_rel(&rel_src), base_of_rel(&rel_src)],
                    )?
                    .unwrap_or(0)
            } else {
                0
            };
            tokio::fs::rename(&abs, &dest).await?;
            let rel_dest = rel_of(&folder.path, &dest);
            if st.is_dir() {
                crate::scanner::relocate_dir(&ctx, folder.id, &rel_src, folder.id, &rel_dest);
                crate::scanner::remove_dir(&ctx, folder.id, &rel_src);
                crate::scanner::scan_folder(&ctx, folder.id, &rel_dest).await;
            } else if !crate::scanner::relocate_file(
                &ctx, folder.id, &rel_src, folder.id, &rel_dest,
            ) {
                crate::scanner::remove_file(&ctx, folder.id, &rel_src);
                let id = crate::scanner::upsert_file(&ctx, folder.id, &rel_dest).await;
                if let (Some(id), true) = (id, fav != 0) {
                    ctx.db
                        .run("UPDATE media SET favorite = 1 WHERE id = ?", p![id])?;
                }
            }
            ok_json(&json!({ "rel": rel_dest }))
        }),
    );
    r.add("POST", "/api/fs/move", Fs, f, h(fs_move));
    r.add(
        "POST",
        "/api/fs/delete",
        Fs,
        f,
        h(|ctx, req| async move {
            require_writable(&ctx)?;
            let mut n = 0;
            let refs = match req.body.prop("refs")? {
                Some(Value::Array(a)) => a.clone(),
                _ => vec![],
            };
            for rf in refs {
                let (folder, abs) = folder_path_v(
                    &ctx,
                    Some(&rf).prop("folderId")?,
                    &str_of(Some(&rf).prop("rel")?),
                )?;
                let rel_src = normalize_rel(Some(&str_of(rf.get("rel"))))?;
                if rel_src.is_empty() {
                    continue;
                }
                let Ok(st) = tokio::fs::metadata(&abs).await else {
                    continue;
                };
                trash(&ctx, &folder, &abs, &rel_src).await?;
                if st.is_dir() {
                    crate::scanner::remove_dir(&ctx, folder.id, &rel_src);
                } else {
                    crate::scanner::remove_file(&ctx, folder.id, &rel_src);
                }
                n += 1;
            }
            ok_json(&json!({ "deleted": n }))
        }),
    );
    r.add("POST", "/api/edits/save", Fs, f, h(save_edit))
        .body_limit = Some(1024 * 1024 * 1024);

    // ── fonts ──
    r.add(
        "GET",
        "/api/fonts",
        crate::security::Limit::Global,
        f,
        h(|ctx, _req| async move { ok_json(&json!({ "fonts": list_fonts(&ctx)? })) }),
    );
    r.add(
        "PUT",
        "/api/fonts",
        Fs,
        f,
        h(|ctx, mut req| async move {
            let name = sanitize_name(req.q("name").filter(|s| !s.is_empty()).unwrap_or("font.ttf"));
            let ext = pa_core::ext_of(&name);
            if !["ttf", "otf", "woff", "woff2"].contains(&ext.as_str()) {
                return Err(HttpError::new(400, "Unsupported font format"));
            }
            req.body_limit = 64 * 1024 * 1024;
            let buf = req.read_all().await?;
            let facts = crate::fonts::parse_font(&buf).ok_or_else(|| HttpError::new(400, "Invalid or unsupported font file"))?;
            let stem = match name.rfind('.') {
                Some(i) => &name[..i],
                None => name.as_str(),
            };
            let fallback = regex::Regex::new("[-_]+").map(|re| re.replace_all(stem, " ").into_owned()).unwrap_or_else(|_| stem.to_string());
            let sub = facts.subfamily.clone().unwrap_or_else(|| "Regular".into());
            let lower = sub.to_lowercase();
            let family: String = facts.family.clone().unwrap_or(fallback).chars().take(80).collect();
            let weight = facts.weight.filter(|w| *w != 0).map(i64::from).unwrap_or(if lower.contains("bold") { 700 } else { 400 });
            let style = if lower.contains("italic") || lower.contains("oblique") || facts.italic_angle != 0.0 { "italic" } else { "normal" };
            tokio::fs::create_dir_all(&ctx.cfg.font_dir).await?;
            let file = format!("{}.{ext}", uuid::Uuid::new_v4());
            tokio::fs::write(ctx.cfg.font_dir.join(&file), &buf).await?;
            let format = match ext.as_str() {
                "ttf" => "truetype",
                "otf" => "opentype",
                e => e,
            };
            ctx.db.run(
                "INSERT INTO fonts (family, subfamily, weight, style, format, file, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
                p![family, sub, weight, style, format, file, crate::events::now_ms()],
            )?;
            ctx.events.emit(ServerEvent::Fonts);
            ok_json(&json!({ "fonts": list_fonts(&ctx)? }))
        }),
    )
    .body_limit = Some(64 * 1024 * 1024);
    r.add(
        "DELETE",
        "/api/fonts/:id",
        Fs,
        f,
        h(|ctx, req| async move {
            let id = pa_core::js::number(req.param("id"));
            let file = ctx
                .db
                .get("SELECT file FROM fonts WHERE id = ?", p![id], |r| {
                    Ok(crate::db::text(r, 0))
                })?
                .flatten();
            if let Some(file) = file {
                ctx.db.run("DELETE FROM fonts WHERE id = ?", p![id])?;
                let _ = tokio::fs::remove_file(ctx.cfg.font_dir.join(file)).await;
                ctx.events.emit(ServerEvent::Fonts);
            }
            ok_json(&json!({ "fonts": list_fonts(&ctx)? }))
        }),
    );
    r.add(
        "GET",
        "/api/fonts/:id/file",
        Fs,
        f,
        h(|ctx, req| async move {
            let id = pa_core::js::number(req.param("id"));
            let file = ctx
                .db
                .get("SELECT file FROM fonts WHERE id = ?", p![id], |r| {
                    Ok(crate::db::text(r, 0))
                })?
                .flatten();
            let Some(file) = file else {
                return Err(HttpError::new(404, "Font not found"));
            };
            let mut resp = crate::http::send_file(
                &req.parts.headers,
                req.method(),
                &ctx.cfg.font_dir.join(file),
                crate::http::SendOpts {
                    cache_control: false,
                    preset_cache_control: Some("public, max-age=31536000, immutable"),
                },
            )
            .await
            .ok_or_else(|| HttpError::new(404, "Not found"))?;
            crate::http::set_header(&mut resp, "access-control-allow-origin", "*");
            Ok(resp)
        }),
    );
}

async fn fs_move(ctx: C, req: Req) -> ApiResult<axum::http::Response<axum::body::Body>> {
    require_writable(&ctx)?;
    let refs: Vec<Value> = match req.body.prop("refs")? {
        Some(Value::Array(a)) => a.clone(),
        _ => vec![],
    };
    let to = req.body.prop("to")?;
    let to_fid = to.prop("folderId")?;
    let to_dir = str_of(to.and_then(|t| t.get("dir")));
    let target = library::folder_row_str(
        &ctx,
        &to_fid
            .map(crate::http::js_string)
            .unwrap_or_else(|| "undefined".into()),
    )?;
    let copy = req.body.opt("copy").is_some_and(super::truthy);
    let mut remote_refs = 0;
    let mut device_refs = 0;
    for rf in &refs {
        let fr = library::folder_row_str(&ctx, &str_of(Some(rf).prop("folderId")?))?;
        if fr.is_remote() {
            remote_refs += 1;
        }
        if fr.is_device() {
            device_refs += 1;
        }
    }
    if remote_refs > 0 {
        if !copy {
            return Err(HttpError::new(
                403,
                "Files in a MEGA link can only be copied",
            ));
        }
        if remote_refs != refs.len() {
            return Err(HttpError::new(
                400,
                "Copy MEGA files separately from local files",
            ));
        }
        folder_path_v(&ctx, to_fid, &to_dir)?;
        let fid = to_fid.map(crate::http::js_string).unwrap_or_default();
        let job = ctx
            .remote
            .svc
            .copy(&file_refs(&refs), &[], &fid, &to_dir)
            .await?;
        return ok_json(&json!({ "moved": [], "job": job }));
    }
    let mut from_device: Vec<pa_core::FileRef> = Vec::new();
    if target.is_device() || device_refs > 0 {
        if !copy {
            return Err(HttpError::new(
                403,
                "Files on a linked device can only be copied",
            ));
        }
        if target.is_device() {
            if device_refs > 0 {
                return Err(HttpError::new(
                    403,
                    "Copying between devices is not supported",
                ));
            }
            let df = pa_link::DeviceFolderRow {
                id: target.id,
                name: target.name.clone(),
                sort: target.sort,
                device_id: target.device_id.clone().unwrap_or_default(),
                device_key: target.device_key.clone().unwrap_or_default(),
            };
            let job = transfers(&ctx)
                .copy_to_device(&file_refs(&refs), &df, &to_dir)
                .await?;
            return ok_json(&json!({ "moved": [], "job": job }));
        }
        for rf in &refs {
            let fr = library::folder_row_str(&ctx, &str_of(rf.get("folderId")))?;
            if fr.is_device() {
                from_device.push(pa_core::FileRef {
                    folder_id: fr.id.to_string(),
                    rel: str_of(rf.get("rel")),
                });
            }
        }
    }
    let (to_folder, to_abs) = folder_path_v(&ctx, to_fid, &to_dir)?;
    tokio::fs::create_dir_all(&to_abs).await?;
    let mut out: Vec<String> = Vec::new();
    let job = if from_device.is_empty() {
        None
    } else {
        let hf = pa_link::HostFolder {
            id: to_folder.id,
            name: to_folder.name.clone(),
            path: PathBuf::from(&to_folder.path),
            device_id: None,
        };
        Some(transfers(&ctx).copy_from_device(&from_device, &hf, &to_dir)?)
    };
    for rf in refs {
        if library::folder_row_str(&ctx, &str_of(rf.get("folderId"))).is_ok_and(|f| f.is_device()) {
            continue;
        }
        let relv = str_of(rf.get("rel"));
        let (folder, abs) = folder_path_v(&ctx, rf.get("folderId"), &relv)?;
        let rel_src = normalize_rel(Some(&relv))?;
        if rel_src.is_empty() {
            continue;
        }
        if to_abs == abs || to_abs.starts_with(&abs) {
            return Err(HttpError::new(400, "Cannot move a folder into itself"));
        }
        let Ok(st) = tokio::fs::metadata(&abs).await else {
            continue;
        };
        if !copy && abs.parent() == Some(to_abs.as_path()) {
            continue;
        }
        let fname = abs
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let dest = unique_path(&to_abs, &fname);
        if copy {
            let (s2, d2) = (abs.clone(), dest.clone());
            tokio::task::spawn_blocking(move || super::util::copy_tree(&s2, &d2, true))
                .await
                .map_err(HttpError::internal)??;
        } else {
            move_file(&abs, &dest).await?;
        }
        let rel_dest = rel_of(&to_folder.path, &dest);
        if !copy {
            if st.is_dir() {
                crate::scanner::relocate_dir(&ctx, folder.id, &rel_src, to_folder.id, &rel_dest);
                crate::scanner::remove_dir(&ctx, folder.id, &rel_src);
            } else if !crate::scanner::relocate_file(
                &ctx,
                folder.id,
                &rel_src,
                to_folder.id,
                &rel_dest,
            ) {
                crate::scanner::remove_file(&ctx, folder.id, &rel_src);
            }
        }
        if st.is_dir() {
            crate::scanner::scan_folder(&ctx, to_folder.id, &rel_dest).await;
        } else {
            crate::scanner::upsert_file(&ctx, to_folder.id, &rel_dest).await;
        }
        out.push(rel_dest);
    }
    match job {
        Some(j) => ok_json(&json!({ "moved": out, "job": j })),
        None => ok_json(&json!({ "moved": out })),
    }
}

fn file_refs(refs: &[Value]) -> Vec<pa_core::FileRef> {
    refs.iter()
        .map(|r| pa_core::FileRef {
            folder_id: r
                .get("folderId")
                .map(crate::http::js_string)
                .unwrap_or_default(),
            rel: str_of(r.get("rel")),
        })
        .collect()
}

/// the phone transfer jobs (pa-link)
fn transfers(ctx: &C) -> pa_link::Transfers {
    pa_link::Transfers {
        hub: ctx.devices.link.clone(),
        jobs: ctx.jobs.clone(),
        host: std::sync::Arc::new(super::misc::store_of(ctx)),
    }
}

async fn save_edit(ctx: C, mut req: Req) -> ApiResult<axum::http::Response<axum::body::Body>> {
    require_writable(&ctx)?;
    let fmt = match req.q("format") {
        Some(f @ ("jpg" | "png" | "webp")) => f.to_string(),
        _ => "jpg".into(),
    };
    let quality = crate::scope::num_or(req.q("quality"), 92.0).clamp(40.0, 100.0);
    req.body_limit = 1024 * 1024 * 1024;
    let input = req.read_all().await?;
    if input.is_empty() {
        return Err(HttpError::new(400, "Empty body"));
    }
    let suffix = ctx.settings().edit_suffix;
    let (folder, dest, exif) = if let Some(sid) = req.q("sourceId").filter(|s| !s.is_empty()) {
        let src = super::util::media_row(&ctx, sid)?;
        let (folder, a) = media_abs(&ctx, &src)?;
        let base = if src.ext.is_empty() {
            src.name.clone()
        } else {
            src.name[..src.name.len().saturating_sub(src.ext.len() + 1)].to_string()
        };
        let dir = Path::new(&a)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let dest = unique_edit_path(&dir, &base, &suffix, &fmt);
        let exif = exif_from(&a).await;
        (folder, dest, exif)
    } else {
        let (folder, abs) = folder_path(&ctx, req.q("folderId").unwrap_or(""), req.qs("dir"))?;
        tokio::fs::create_dir_all(&abs).await?;
        let dest = unique_path(
            &abs,
            &format!(
                "{}.{fmt}",
                sanitize_name(
                    req.q("name")
                        .filter(|s| !s.is_empty())
                        .unwrap_or("PeekAround")
                )
            ),
        );
        (folder, dest, None)
    };
    let q = quality as u8;
    let fmt2 = fmt.clone();
    let data = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, String> {
        let img = pa_media::decode_bytes(&input, pa_media::DecodeOptions::default())
            .map_err(|e| e.to_string())?;
        Ok(match fmt2.as_str() {
            "jpg" => {
                let mut o = pa_media::encode::JpegOptions::new(q);
                o.chroma444 = q >= 95;
                let j = pa_media::encode::jpeg(&img, o).map_err(|e| e.to_string())?;
                match &exif {
                    Some(t) => crate::exifw::jpeg_with_exif(&j, t),
                    None => j,
                }
            }
            "webp" => {
                let w = pa_media::encode::webp(&img, q as f32).map_err(|e| e.to_string())?;
                match &exif {
                    Some(t) => crate::exifw::webp_with_exif(
                        &w,
                        t,
                        img.width(),
                        img.height(),
                        img.color().has_alpha(),
                    ),
                    None => w,
                }
            }
            _ => pa_media::encode::png(&img).map_err(|e| e.to_string())?,
        })
    })
    .await
    .map_err(HttpError::internal)?
    .map_err(|e| HttpError::new(500, e))?;
    let tmp = PathBuf::from(format!("{}.part", dest.display()));
    tokio::fs::write(&tmp, &data).await?;
    tokio::fs::rename(&tmp, &dest).await?;
    let rel = rel_of(&folder.path, &dest);
    let id = crate::scanner::upsert_file(&ctx, folder.id, &rel).await;
    ok_json(&json!({ "item": loc(&folder, &rel), "id": id.map(|i| i.to_string()) }))
}
