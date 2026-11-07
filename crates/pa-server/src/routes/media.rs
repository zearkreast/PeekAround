//! Media routes (mirrors `routes/media.ts`): details, favourites, thumbnails, the original file
//! (Range), display renditions, previews, filmstrips, the live transcode stream and zip downloads.

use super::util::{media_row, media_source, rel_of_row};
use crate::ctx::{Ctx, C};
use crate::db::MediaRow;
use crate::error::{ApiResult, HttpError};
use crate::http::{
    content_disposition, ok_json, redirect, send_file, set_header, BodyScope, Req, SendOpts,
};
use crate::p;
use crate::router::{h, Router};
use crate::security::Limit::{Fs, Global, Heavy};
use crate::thumbs::{Prio, ThumbRow};
use axum::body::Body;
use axum::http::Response;
use pa_core::{CameraInfo, MediaDetails, MediaReason, VideoInfo};
use serde_json::{json, Value};
use std::path::Path;

const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// MediaDetails from the row (+ exif)
pub fn details_from_row(
    ctx: &Ctx,
    r: &MediaRow,
    path: String,
    exif: Option<indexmap::IndexMap<String, Option<pa_core::ExifValue>>>,
) -> MediaDetails {
    let extra: Option<Value> = r
        .extra
        .as_deref()
        .and_then(|e| serde_json::from_str(e).ok());
    let video = if r.kind == "video" {
        extra.as_ref().filter(|e| !e.is_null()).map(|e| VideoInfo {
            codec: e.get("codec").and_then(|v| v.as_str()).map(String::from),
            fps: e.get("fps").and_then(|v| v.as_f64()),
            bitrate: e.get("bitrate").and_then(|v| v.as_f64()),
            audio_codec: e
                .get("audioCodec")
                .and_then(|v| v.as_str())
                .map(String::from),
            rotation: e
                .get("rotation")
                .and_then(|v| v.as_f64())
                .map(|f| f as i32)
                .unwrap_or(0),
        })
    } else {
        None
    };
    MediaDetails {
        item: r.to_item(),
        path,
        exif,
        video,
        gps: match (r.lat, r.lon) {
            (Some(lat), Some(lon)) => Some(pa_core::LatLon { lat, lon }),
            _ => None,
        },
        place: ctx
            .geo
            .place_info(r.city.as_deref(), r.cc.as_deref(), r.admin1.as_deref()),
        camera: if r.make.as_deref().is_some_and(|s| !s.is_empty())
            || r.model.as_deref().is_some_and(|s| !s.is_empty())
            || r.lens.as_deref().is_some_and(|s| !s.is_empty())
        {
            Some(CameraInfo {
                make: r.make.clone(),
                model: r.model.clone(),
                lens: r.lens.clone(),
            })
        } else {
            None
        },
        tz_offset: r.tz_off.map(|t| t as i32),
    }
}

async fn file_reply(
    req: &Req,
    f: &Path,
    preset_cache_control: &'static str,
) -> ApiResult<Response<Body>> {
    match send_file(
        &req.parts.headers,
        req.method(),
        f,
        SendOpts {
            cache_control: false,
            preset_cache_control: Some(preset_cache_control),
        },
    )
    .await
    {
        Some(r) => Ok(r),
        None => Err(HttpError::new(404, "Not found")),
    }
}

/// the engine's view of a media row of a MEGA folder
pub fn remote_media_of(r: &MediaRow) -> pa_remote::RemoteMedia {
    pa_remote::RemoteMedia {
        id: r.id,
        folder_id: r.folder_id,
        dir: r.dir.clone(),
        name: r.name.clone(),
        ext: r.ext.clone(),
        kind: crate::db::kind_of_str(&r.kind),
        size: r.size,
        mtime: r.mtime,
        width: r.width.map(|v| v as f64),
        height: r.height.map(|v| v as f64),
        duration: r.duration,
        playback: crate::db::playback_of_str(&r.playback),
    }
}

/// `deviceDetails`: the phone's details refine the row (time zone, GPS, camera) → MediaDetails
async fn device_details(
    ctx: &C,
    r: MediaRow,
    folder: &crate::db::FolderRow,
) -> ApiResult<MediaDetails> {
    let (dev, key) = (
        folder.device_id.clone().unwrap_or_default(),
        folder.device_key.clone().unwrap_or_default(),
    );
    let d = ctx
        .devices
        .link
        .details(&dev, &key, &rel_of_row(&r))
        .await
        .ok()
        .flatten();
    let mut row = r.clone();
    if let Some(d) = &d {
        let base = pa_meta::ExtractedMeta {
            width: r.width.map(|v| v as f64),
            height: r.height.map(|v| v as f64),
            duration: r.duration,
            playback: crate::db::playback_of_str(&r.playback),
            instant: (r.time_src == pa_meta::time_src::INSTANT as i64).then_some(r.taken_at),
            ..Default::default()
        };
        let m = pa_link::media::refine_meta(d, base);
        let g = crate::scanner::group_columns(ctx, &r, &m);
        let mut ps: crate::db::Params = pa_meta::group_params(&g)
            .into_iter()
            .map(crate::db::Sql::sql)
            .collect();
        ps.push(crate::db::Sql::sql(r.id));
        ctx.db.run(
            &format!("UPDATE media SET {} WHERE id = ?", pa_meta::GROUP_SET_SQL),
            ps,
        )?;
        row = super::util::media_row_n(ctx, r.id).unwrap_or(r.clone());
        let key_of = |x: &MediaRow| {
            (
                x.taken_local,
                x.lat,
                x.lon,
                x.make.clone(),
                x.model.clone(),
                x.flags,
            )
        };
        if key_of(&r) != key_of(&row) {
            ctx.events
                .media_changed(&r.folder_id.to_string(), &r.dir, MediaReason::Meta);
            ctx.explore.invalidate(ctx, true);
        }
    }
    let name = crate::library::folder_ref(ctx, folder, None)
        .device
        .map(|d| d.name)
        .unwrap_or_default();
    let path = pa_link::media::details_path(&name, &folder.name, &rel_of_row(&row));
    let exif = pa_link::media::details_exif(d.as_ref()).map(|m| {
        m.into_iter()
            .map(|(k, v)| (k, serde_json::from_value::<pa_core::ExifValue>(v).ok()))
            .collect()
    });
    let mut out = details_from_row(ctx, &row, path, exif);
    out.video = None;
    Ok(out)
}

pub fn routes(r: &mut Router) {
    let m = BodyScope::Main;
    r.add(
        "GET",
        "/api/media/:id",
        Fs,
        m,
        h(|ctx, req| async move {
            let r = media_row(&ctx, req.param("id"))?;
            let src = media_source(&ctx, &r)?;
            if src.device {
                return ok_json(&device_details(&ctx, r, &src.folder).await?);
            }
            if src.remote {
                return ok_json(&ctx.remote.details(&ctx, &r, &src.folder));
            }
            let path = if ctx.cfg.desktop() {
                src.abs.clone()
            } else {
                let joined = [src.folder.name.as_str(), r.dir.as_str(), r.name.as_str()]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("/");
                crate::paths::normalize(Path::new(&joined))
                    .to_string_lossy()
                    .into_owned()
            };
            let exif = if r.kind == "image" {
                let a = src.abs.clone();
                let tz = ctx.local_tz.clone();
                tokio::task::spawn_blocking(move || pa_meta::read_exif(Path::new(&a), &tz))
                    .await
                    .ok()
                    .flatten()
                    .map(|m| m.into_iter().map(|(k, v)| (k, Some(v))).collect())
            } else {
                None
            };
            let mut d = details_from_row(&ctx, &r, path, exif);
            if r.kind == "image" && d.exif.is_none() {
                d.exif = None;
            }
            ok_json(&d)
        }),
    );
    r.add(
        "PATCH",
        "/api/media/:id",
        Fs,
        m,
        h(|ctx, req| async move {
            let r = media_row(&ctx, req.param("id"))?;
            if let Some(Value::Bool(f)) = req.body.opt("favorite") {
                ctx.db.run(
                    "UPDATE media SET favorite = ? WHERE id = ?",
                    p![*f as i64, r.id],
                )?;
                ctx.events
                    .media_changed(&r.folder_id.to_string(), &r.dir, MediaReason::Change);
            }
            ok_json(&super::util::media_row_n(&ctx, r.id)?.to_item())
        }),
    );
    r.add(
        "POST",
        "/api/media/favorite",
        Fs,
        m,
        h(|ctx, req| async move {
            let ids = req.body.prop("ids")?.cloned();
            let fav = req
                .body
                .prop("favorite")?
                .map(super::truthy)
                .unwrap_or(false);
            if let Some(Value::Array(ids)) = ids {
                for id in &ids {
                    let r = super::util::media_row_v(&ctx, Some(id))?;
                    ctx.db.run(
                        "UPDATE media SET favorite = ? WHERE id = ?",
                        p![fav as i64, r.id],
                    )?;
                    ctx.events
                        .media_changed(&r.folder_id.to_string(), &r.dir, MediaReason::Change);
                }
            }
            ok_json(&json!({ "ok": true }))
        }),
    );
    r.add(
        "GET",
        "/api/media/:id/thumb",
        Global,
        m,
        h(|ctx, req| async move {
            let r = media_row(&ctx, req.param("id"))?;
            let src = media_source(&ctx, &r)?;
            let s = crate::scope::num_or(req.q("s"), 320.0);
            let f = ctx
                .thumbs
                .thumb(
                    &src.abs,
                    &ThumbRow::from(&r),
                    pa_media::render::snap_size(s),
                    Prio::High,
                )
                .await
                .ok()
                .flatten();
            let Some(f) = f else {
                return Err(HttpError::new(404, "No thumbnail"));
            };
            file_reply(&req, &f, IMMUTABLE).await
        }),
    );
    r.add(
        "GET",
        "/api/media/:id/file",
        Global,
        m,
        h(|ctx, req| async move {
            let r = media_row(&ctx, req.param("id"))?;
            let src = media_source(&ctx, &r)?;
            let head = req.method() == axum::http::Method::HEAD;
            let range = req.header("range").map(String::from);
            let download = req.q("download").is_some_and(|d| !d.is_empty());
            if src.device {
                let (dev, key) = (
                    src.folder.device_id.clone().unwrap_or_default(),
                    src.folder.device_key.clone().unwrap_or_default(),
                );
                let row = pa_link::media::FileRow {
                    dir: r.dir.clone(),
                    name: r.name.clone(),
                    ext: r.ext.clone(),
                    size: r.size,
                };
                let f = pa_link::media::serve_device_file(
                    &ctx.devices.link,
                    &dev,
                    &key,
                    &row,
                    head,
                    range.as_deref(),
                    download,
                )
                .await?;
                return Ok(pa_link::axum::file_response(f));
            }
            if src.remote {
                let folder = crate::remote::row_of(&src.folder)
                    .ok_or_else(|| HttpError::new(400, "Not a remote folder"))?;
                let rm = remote_media_of(&r);
                let f = ctx
                    .remote
                    .svc
                    .serve_file(&rm, &folder, head, range.as_deref(), download)
                    .await?;
                return Ok(pa_link::axum::file_response(f));
            }
            if tokio::fs::metadata(&src.abs).await.is_err() {
                return Err(HttpError::new(404, "File missing"));
            }
            let mut resp = file_reply(&req, Path::new(&src.abs), "private, max-age=3600").await?;
            if req.q("download").is_some_and(|d| !d.is_empty()) {
                set_header(
                    &mut resp,
                    "content-disposition",
                    &content_disposition(&r.name),
                );
            }
            Ok(resp)
        }),
    );
    r.add(
        "GET",
        "/api/media/:id/display",
        Global,
        m,
        h(|ctx, req| async move {
            let r = media_row(&ctx, req.param("id"))?;
            if r.kind != "image" {
                return Err(HttpError::new(400, "Not an image"));
            }
            let src = media_source(&ctx, &r)?;
            let max_req = crate::scope::num_or(req.q("max"), 0.0);
            let max = if max_req > 0.0 {
                Some(if max_req <= 2048.0 { 2048 } else { 4096 })
            } else {
                None
            };
            let animated = r.ext == "gif" || (r.flags as u32 & pa_core::media_flag::ANIMATED) != 0;
            let fits = match (max, r.width, r.height) {
                (Some(m), Some(w), Some(hh)) if w != 0 && hh != 0 => w.max(hh) <= m as i64,
                _ => false,
            };
            if r.playback == "direct" && (max.is_none() || fits || animated || r.ext == "svg") {
                return Ok(redirect(&format!("/api/media/{}/file", r.id)));
            }
            let f = ctx
                .thumbs
                .display(&src.abs, &ThumbRow::from(&r), max)
                .await
                .map_err(|e| {
                    if is_offline(&e) {
                        HttpError::new(503, "device offline")
                    } else {
                        HttpError::new(500, e)
                    }
                })?;
            let Some(f) = f else {
                return Err(HttpError::new(415, "Cannot convert"));
            };
            file_reply(&req, &f, IMMUTABLE).await
        }),
    );
    r.add(
        "GET",
        "/api/media/:id/preview",
        Global,
        m,
        h(|ctx, req| async move {
            let r = media_row(&ctx, req.param("id"))?;
            let src = media_source(&ctx, &r)?;
            if src.device {
                if r.kind != "video" {
                    return Err(HttpError::new(404, "No preview"));
                }
                return Ok(redirect(&format!("/api/media/{}/file", r.id)));
            }
            let f = ctx
                .thumbs
                .preview(&src.abs, &ThumbRow::from(&r))
                .await
                .ok()
                .flatten();
            let Some(f) = f else {
                return Err(HttpError::new(404, "No preview"));
            };
            file_reply(&req, &f, IMMUTABLE).await
        }),
    );
    r.add(
        "GET",
        "/api/media/:id/filmstrip",
        Global,
        m,
        h(|ctx, req| async move {
            let r = media_row(&ctx, req.param("id"))?;
            let src = media_source(&ctx, &r)?;
            let n = crate::scope::num_or(req.q("n"), 12.0);
            let hh = crate::scope::num_or(req.q("h"), 72.0);
            let f = ctx
                .thumbs
                .filmstrip(&src.abs, &ThumbRow::from(&r), n, hh)
                .await
                .ok()
                .flatten();
            let Some(f) = f else {
                return Err(HttpError::new(404, "No filmstrip"));
            };
            file_reply(&req, &f, IMMUTABLE).await
        }),
    );
    r.add("GET", "/api/media/:id/stream", Global, m, h(stream));
    r.add("GET", "/api/download", Heavy, m, h(download));
}

fn is_offline(e: &str) -> bool {
    e.contains("unavailable") || e.contains("offline")
}

async fn stream(ctx: C, req: Req) -> ApiResult<Response<Body>> {
    let r = media_row(&ctx, req.param("id"))?;
    if r.kind != "video" {
        return Err(HttpError::new(400, "Not a video"));
    }
    if !ctx.ffmpeg.available() {
        return Err(HttpError::new(501, "ffmpeg not available"));
    }
    let src = media_source(&ctx, &r)?;
    if src.device {
        ctx.devices.require_online(&src.folder)?;
    }
    let input: Vec<String> = if src.device {
        pa_link::media::device_ffmpeg_input(&ctx.devices.link, r.id)?
    } else if src.remote {
        ctx.remote.svc.ffmpeg_input(r.id)?
    } else {
        vec!["-i".into(), src.abs.clone()]
    };
    let t = crate::scope::num_or(req.q("t"), 0.0).max(0.0);
    let mut args: Vec<String> = vec!["-loglevel".into(), "error".into()];
    if t != 0.0 {
        args.extend(["-ss".into(), format!("{t:.2}")]);
    }
    args.extend(crate::ctx::hw_accel_args(&ctx.settings()));
    args.extend(input);
    for a in [
        "-map",
        "0:v:0",
        "-map",
        "0:a:0?",
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-crf",
        "23",
        "-pix_fmt",
        "yuv420p",
        "-vf",
        "scale='min(1920,iw)':-2",
        "-c:a",
        "aac",
        "-b:a",
        "160k",
        "-ac",
        "2",
        "-movflags",
        "frag_keyframe+empty_moov+default_base_moof",
        "-f",
        "mp4",
        "pipe:1",
    ] {
        args.push(a.into());
    }
    let mut child = ctx.ffmpeg.spawn(&args).map_err(HttpError::internal)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| HttpError::internal("no stdout"))?;
    drop(child.stdin.take());
    if let Some(mut e) = child.stderr.take() {
        tokio::spawn(async move {
            let mut sink = tokio::io::sink();
            let _ = tokio::io::copy(&mut e, &mut sink).await;
        });
    }
    // the child is killed when the response body (and with it this guard) is dropped
    struct Kill(tokio::process::Child);
    impl Drop for Kill {
        fn drop(&mut self) {
            let _ = self.0.start_kill();
        }
    }
    let guard = Kill(child);
    let s = tokio_util::io::ReaderStream::new(stdout);
    let s = futures_util::StreamExt::map(s, move |c| {
        let _ = &guard;
        c
    });
    let mut resp = Response::new(Body::from_stream(s));
    set_header(&mut resp, "content-type", "video/mp4");
    set_header(&mut resp, "cache-control", "no-store");
    Ok(resp)
}

async fn download(ctx: C, req: Req) -> ApiResult<Response<Body>> {
    use crate::zip::{ZipEntry, ZipSource};
    let mut ids_q = req.q("ids").map(String::from).filter(|s| !s.is_empty());
    if let (Some(aid), None) = (req.q("albumId").filter(|s| !s.is_empty()), &ids_q) {
        let no_devices = crate::devices::hide_device_media(&ctx, &req);
        let rows: Vec<i64> = ctx.db.all(
            &format!(
                "SELECT media_id FROM album_items WHERE album_id = ?{} ORDER BY position",
                if no_devices { " AND media_id IN (SELECT m.id FROM media m JOIN folders f ON f.id = m.folder_id WHERE f.device_id IS NULL)" } else { "" }
            ),
            p![pa_core::js::number(aid)],
            |r| r.get(0),
        )?;
        ids_q = Some(
            rows.iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    let mut entries: Vec<ZipEntry> = Vec::new();
    if let Some(ids) = ids_q.filter(|s| !s.is_empty()) {
        let mut used = std::collections::HashSet::new();
        for id in ids.split(',').filter(|s| !s.is_empty()) {
            let r = media_row(&ctx, id)?;
            let src = media_source(&ctx, &r)?;
            let mut n = r.name.clone();
            let mut i = 1;
            while used.contains(&n) {
                let ext = match r.name.rfind('.') {
                    Some(d) => &r.name[d..],
                    None => "",
                };
                n = format!("{} ({i}){ext}", &r.name[..r.name.len() - ext.len()]);
                i += 1;
            }
            used.insert(n.clone());
            if src.device {
                let s = pa_link::media::lazy_device_stream(
                    ctx.devices.link.clone(),
                    src.folder.device_id.clone().unwrap_or_default(),
                    src.folder.device_key.clone().unwrap_or_default(),
                    rel_of_row(&r),
                );
                entries.push(ZipEntry {
                    name: n,
                    source: ZipSource::Stream(s, Some(r.size.max(0) as u64)),
                });
            } else if src.remote {
                let folder = crate::remote::row_of(&src.folder)
                    .ok_or_else(|| HttpError::new(400, "Not a remote folder"))?;
                let s = ctx.remote.svc.zip_entry(&folder, &remote_media_of(&r));
                entries.push(ZipEntry {
                    name: n,
                    source: ZipSource::Stream(s, Some(r.size.max(0) as u64)),
                });
            } else {
                entries.push(ZipEntry {
                    name: n,
                    source: ZipSource::File(src.abs.into()),
                });
            }
        }
    } else if let Some(fid) = req.q("folderId").filter(|s| !s.is_empty()) {
        let folder = crate::library::folder_row_str(&ctx, fid)?;
        if folder.is_device() || folder.is_remote() {
            // phone / MEGA folder: entries from the index, each streamed when the zip reaches it
            let dir = crate::paths::trim_chars(req.qs("dir"), "/").to_string();
            let (sql, ps) = if dir.is_empty() {
                (
                    "SELECT * FROM media WHERE folder_id = ? ORDER BY dir, name".to_string(),
                    p![folder.id],
                )
            } else {
                ("SELECT * FROM media WHERE folder_id = ? AND (dir = ? OR substr(dir, 1, ?) = ?) ORDER BY dir, name".to_string(), p![folder.id, dir.clone(), pa_core::js::len16(&dir) as i64 + 1, format!("{dir}/")])
            };
            let rows = ctx.db.all(&sql, ps, MediaRow::from_row)?;
            let rf = crate::remote::row_of(&folder);
            for r in rows {
                let rel = rel_of_row(&r);
                let s = match &rf {
                    Some(f) => ctx.remote.svc.zip_entry(f, &remote_media_of(&r)),
                    None => pa_link::media::lazy_device_stream(
                        ctx.devices.link.clone(),
                        folder.device_id.clone().unwrap_or_default(),
                        folder.device_key.clone().unwrap_or_default(),
                        rel.clone(),
                    ),
                };
                let name = if dir.is_empty() {
                    rel
                } else {
                    pa_core::js::slice16_from(&rel, pa_core::js::len16(&dir) + 1).to_string()
                };
                entries.push(ZipEntry {
                    name,
                    source: ZipSource::Stream(s, Some(r.size.max(0) as u64)),
                });
            }
        } else {
            let base = crate::paths::resolve_in_root(&folder.path, req.qs("dir"))?;
            let c = ctx.clone();
            let found = tokio::task::spawn_blocking(move || {
                let mut out = Vec::new();
                fn walk(
                    c: &Ctx,
                    d: &Path,
                    rel: &str,
                    out: &mut Vec<(String, std::path::PathBuf)>,
                ) -> std::io::Result<()> {
                    let mut ents: Vec<_> = std::fs::read_dir(d)?.flatten().collect();
                    ents.sort_by(|a, b| {
                        a.file_name()
                            .as_encoded_bytes()
                            .cmp(b.file_name().as_encoded_bytes())
                    });
                    for e in ents {
                        let name = e.file_name().to_string_lossy().into_owned();
                        if c.scanner.skip_name(&name) {
                            continue;
                        }
                        let rp = if rel.is_empty() {
                            name.clone()
                        } else {
                            format!("{rel}/{name}")
                        };
                        let t = e.file_type()?;
                        if t.is_dir() {
                            walk(c, &e.path(), &rp, out)?;
                        } else if t.is_file() {
                            out.push((rp, e.path()));
                        }
                    }
                    Ok(())
                }
                walk(&c, &base, "", &mut out).map(|_| out)
            })
            .await
            .map_err(HttpError::internal)?
            .map_err(|e| HttpError::new(500, e.to_string()))?;
            for (rel, p) in found {
                entries.push(ZipEntry {
                    name: rel,
                    source: ZipSource::File(p),
                });
            }
        }
    }
    if entries.is_empty() {
        return Err(HttpError::new(400, "Nothing to download"));
    }
    let name = format!(
        "{}.zip",
        req.q("name")
            .filter(|s| !s.is_empty())
            .unwrap_or("PeekAround")
    );
    let mut resp = Response::new(Body::from_stream(crate::zip::zip_stream(entries)));
    set_header(&mut resp, "content-type", "application/zip");
    set_header(
        &mut resp,
        "content-disposition",
        &content_disposition(&name),
    );
    Ok(resp)
}

#[allow(dead_code)]
fn _immutable() -> &'static str {
    IMMUTABLE
}
