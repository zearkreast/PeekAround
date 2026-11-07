//! Core routes (mirrors `routes/core.ts`): info, settings, libraries, folders, temp sessions,
//! host directory browser, the live events socket and the legacy cache routes.

use super::util::{move_file, require_local, require_local_folder, require_writable};
use crate::ctx::{Ctx, C};
use crate::error::{ApiResult, HttpError};
use crate::http::{json_response, ok_json, BodyScope, JsAccess, Req};
use crate::library::{self, LibraryPatch};
use crate::p;
use crate::paths::{is_inside, resolve_host_path, resolve_in_root, sanitize_name, unique_path};
use crate::router::{h, Router};
use crate::security::Limit::{Fs, Global, Heavy};
use pa_core::{Capabilities, HostDirEntry, LanInfo, RunMode, ServerEvent, ServerInfo, TrashMode};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// `serverInfo(ctx)`
pub fn server_info(ctx: &Ctx) -> ServerInfo {
    let desktop = ctx.cfg.desktop();
    let ro = ctx.cfg.readonly;
    let lan = *ctx.lan.lock();
    let server = ctx.cfg.mode == RunMode::Server;
    let has_trash = ctx.cfg.hooks.as_ref().is_some_and(|h| h.has_trash());
    ServerInfo {
        name: pa_core::APP_NAME.into(),
        version: ctx.cfg.version.clone(),
        api_version: pa_core::API_VERSION,
        mode: ctx.cfg.mode,
        hostname: crate::config::hostname(),
        scanning: ctx.scanner.busy(),
        caps: Capabilities {
            upload: !ro,
            file_ops: !ro,
            native_folders: desktop,
            browse_host: !desktop && !ro,
            video_jobs: ctx.ffmpeg.available() && !ro,
            slideshow_export: ctx.ffmpeg.available() && !ro,
            zip_download: true,
            trash: if ro {
                TrashMode::None
            } else if has_trash {
                TrashMode::Os
            } else {
                TrashMode::Folder
            },
            readonly: ro,
            lan_share: desktop,
        },
        lan: LanInfo {
            enabled: server || lan.0,
            urls: if server || lan.0 {
                crate::config::lan_urls(if server { ctx.cfg.port } else { lan.1 })
            } else {
                vec![]
            },
        },
    }
}

fn host_roots(ctx: &Ctx) -> Vec<String> {
    if !ctx.cfg.host_browse_roots.is_empty() {
        return ctx
            .cfg
            .host_browse_roots
            .iter()
            .filter(|r| r.exists())
            .map(|r| r.to_string_lossy().into_owned())
            .collect();
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    if cfg!(windows) {
        let mut out = vec![home];
        for c in b'C'..=b'Z' {
            let d = format!("{}:\\", c as char);
            if Path::new(&d).exists() {
                out.push(d);
            }
        }
        return out;
    }
    vec![home, "/".into()]
}

fn check_host_path(ctx: &Ctx, p: Option<&Value>) -> ApiResult<PathBuf> {
    let s = match p {
        None | Some(Value::Null) => return Err(HttpError::new(400, "Path required")),
        Some(Value::String(s)) if s.is_empty() => return Err(HttpError::new(400, "Path required")),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Bool(false)) => return Err(HttpError::new(400, "Path required")),
        Some(Value::Number(n)) if n.as_f64() == Some(0.0) => {
            return Err(HttpError::new(400, "Path required"))
        }
        Some(_) => return Err(HttpError::new(400, "Invalid path")),
    };
    resolve_host_path(&s, &ctx.cfg.host_browse_roots)
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a.iter().map(crate::http::js_string).collect(),
        _ => vec![],
    }
}

/// `PATCH /api/settings` validation
fn settings_patch(b: Option<&Value>) -> ApiResult<Map<String, Value>> {
    let mut patch = Map::new();
    let get = |k: &str| match b {
        Some(Value::Object(m)) => m.get(k),
        _ => None,
    };
    if let Some(Value::String(z)) = get("zipAction") {
        if ["ask", "extract", "temp"].contains(&z.as_str()) {
            patch.insert("zipAction".into(), json!(z));
        }
    }
    if let Some(Value::String(s)) = get("editSuffix") {
        let clean: String = s.chars().filter(|c| !"\\/:*?\"<>|".contains(*c)).collect();
        let clean = pa_core::js::slice16_to(&clean, 32).to_string();
        patch.insert(
            "editSuffix".into(),
            json!(if clean.is_empty() {
                "_edit".to_string()
            } else {
                clean
            }),
        );
    }
    if let Some(Value::Bool(v)) = get("showHidden") {
        patch.insert("showHidden".into(), json!(v));
    }
    if let Some(Value::Number(n)) = get("tempSessionHours") {
        let v = n.as_f64().unwrap_or(0.0).clamp(1.0, 24.0 * 30.0);
        patch.insert("tempSessionHours".into(), crate::routes::js_number(v));
    }
    if let Some(tz) = get("homeTimeZone") {
        match tz {
            Value::String(s) if pa_core::is_valid_time_zone(s) => {
                patch.insert("homeTimeZone".into(), json!(s));
            }
            _ => return Err(HttpError::new(400, "Unknown time zone")),
        }
    }
    if let Some(Value::String(m)) = get("mapTiles") {
        if ["carto", "osm", "topo", "satellite", "off"].contains(&m.as_str()) {
            patch.insert("mapTiles".into(), json!(m));
        }
    }
    if let Some(Value::Number(n)) = get("tileCacheMb") {
        if let Some(f) = n.as_f64().filter(|f| f.is_finite()) {
            patch.insert(
                "tileCacheMb".into(),
                json!(pa_core::js::round(f.clamp(32.0, 16384.0)) as i64),
            );
        }
    }
    if let Some(Value::Number(n)) = get("remoteCacheMb") {
        if let Some(f) = n.as_f64().filter(|f| f.is_finite()) {
            patch.insert(
                "remoteCacheMb".into(),
                json!(pa_core::js::round(f.clamp(0.0, 1024.0 * 1024.0)) as i64),
            );
        }
    }
    if let Some(Value::String(q)) = get("thumbQuality") {
        if ["low", "normal", "high"].contains(&q.as_str()) {
            patch.insert("thumbQuality".into(), json!(q));
        }
    }
    if let Some(Value::String(q)) = get("indexIntensity") {
        if ["low", "normal", "high"].contains(&q.as_str()) {
            patch.insert("indexIntensity".into(), json!(q));
        }
    }
    if let Some(Value::Bool(v)) = get("hwAccel") {
        patch.insert("hwAccel".into(), json!(v));
    }
    Ok(patch)
}

/// rescan folders in the background
pub fn rescan(ctx: &C, ids: impl IntoIterator<Item = i64>) {
    for id in ids {
        let c = ctx.clone();
        ctx.tasks
            .spawn(async move { crate::scanner::scan_folder(&c, id, "").await });
    }
}

pub fn routes(r: &mut Router) {
    let m = BodyScope::Main;
    r.add(
        "GET",
        "/api/info",
        Global,
        m,
        h(|ctx, _req| async move { ok_json(&server_info(&ctx)) }),
    );

    r.add(
        "GET",
        "/api/settings",
        Fs,
        m,
        h(|ctx, _req| async move { ok_json(&Value::Object(ctx.settings_json())) }),
    );
    r.add(
        "PATCH",
        "/api/settings",
        Fs,
        m,
        h(|ctx, req| async move {
            let b = req.body.value().filter(|v| !v.is_null());
            let patch = settings_patch(b)?;
            let show = patch.contains_key("showHidden");
            let tiles = patch.contains_key("tileCacheMb");
            let remote_mb = patch.get("remoteCacheMb").and_then(Value::as_u64);
            let s = ctx.save_settings(patch);
            if let Some(mb) = remote_mb {
                ctx.remote.svc.cache.set_cap(mb * 1024 * 1024).await;
            }
            if show {
                let c = ctx.clone();
                ctx.tasks
                    .spawn(async move { crate::scanner::scan_all(&c).await });
            }
            if tiles {
                let c = ctx.clone();
                tokio::spawn(async move { c.tiles.trim().await });
            }
            ok_json(&Value::Object(s))
        }),
    );

    r.add(
        "GET",
        "/api/libraries",
        Fs,
        m,
        h(|ctx, req| async move {
            let v = crate::devices::viewer_of(&ctx, &req);
            let devices = if v.devices {
                ctx.devices.list(&ctx, v.this_device.as_deref())?
            } else {
                vec![]
            };
            ok_json(&pa_core::LibrariesResponse {
                libraries: library::list_libraries(&ctx)?,
                temp: library::list_temp(&ctx)?,
                devices,
            })
        }),
    );
    r.add(
        "POST",
        "/api/libraries",
        Fs,
        m,
        h(|ctx, req| async move {
            require_writable(&ctx)?;
            let b = req.body.value();
            let folders = b.prop("folders")?;
            if folders
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty())
            {
                require_local(&req)?;
            }
            let list = str_list(folders);
            for f in &list {
                check_host_path(&ctx, Some(&Value::String(f.clone())))?;
            }
            let name = match b.prop("name")? {
                Some(Value::String(s)) => s.clone(),
                None => {
                    return Err(HttpError::new(
                        500,
                        "Cannot read properties of undefined (reading 'trim')",
                    ))
                }
                Some(Value::Null) => {
                    return Err(HttpError::new(
                        500,
                        "Cannot read properties of null (reading 'trim')",
                    ))
                }
                Some(_) => return Err(HttpError::new(500, "input.name.trim is not a function")),
            };
            let color = b.prop("color")?.and_then(|v| v.as_str()).map(String::from);
            let icon = b.prop("icon")?.and_then(|v| v.as_str()).map(String::from);
            let lib = library::create_library(&ctx, &name, color, icon, &list)?;
            crate::watcher::sync(&ctx);
            rescan(&ctx, lib.folders.iter().filter_map(|f| f.id.parse().ok()));
            ok_json(&lib)
        }),
    );
    r.add(
        "PATCH",
        "/api/libraries/:id",
        Fs,
        m,
        h(|ctx, req| async move {
            require_writable(&ctx)?;
            let b = req.body.value().filter(|v| !v.is_null());
            let get = |k: &str| b.and_then(|v| v.get(k));
            let patch = LibraryPatch {
                name: get("name").and_then(|v| v.as_str()).map(String::from),
                color: get("color").and_then(|v| v.as_str()).map(String::from),
                icon: get("icon").and_then(|v| v.as_str()).map(String::from),
                sort: get("sort").and_then(|v| v.as_f64()).map(|f| f as i64),
                cover_id: get("coverId").map(|v| match v {
                    Value::Null => None,
                    other => Some(crate::http::js_string(other)),
                }),
                options: get("options").cloned(),
            };
            let (lib, rescan_needed) = library::update_library(&ctx, req.param("id"), patch)?;
            if rescan_needed {
                ctx.scanner.invalidate_rules();
                rescan(&ctx, lib.folders.iter().filter_map(|f| f.id.parse().ok()));
            }
            ok_json(&lib)
        }),
    );
    r.add(
        "POST",
        "/api/libraries/order",
        Fs,
        m,
        h(|ctx, req| async move {
            require_writable(&ctx)?;
            let ids = str_list(req.body.opt("ids"));
            library::reorder_libraries(&ctx, &ids)?;
            ok_json(&json!({ "libraries": library::list_libraries(&ctx)? }))
        }),
    );
    r.add(
        "GET",
        "/api/libraries/:id/stats",
        Fs,
        m,
        h(|ctx, req| async move {
            let c = ctx.clone();
            ok_json(&library::stats(&ctx, req.param("id"), |ids| {
                c.scanner.busy_for(ids)
            })?)
        }),
    );
    r.add(
        "POST",
        "/api/libraries/:id/rescan",
        Heavy,
        m,
        h(|ctx, req| async move {
            let lib = library::library(&ctx, req.param("id"))?;
            rescan(&ctx, lib.folders.iter().filter_map(|f| f.id.parse().ok()));
            ok_json(&json!({ "ok": true }))
        }),
    );
    r.add(
        "DELETE",
        "/api/libraries/:id",
        Fs,
        m,
        h(|ctx, req| async move {
            require_writable(&ctx)?;
            library::delete_library(&ctx, req.param("id"))?;
            crate::watcher::sync(&ctx);
            ok_json(&json!({ "ok": true }))
        }),
    );
    r.add(
        "POST",
        "/api/libraries/:id/folders",
        Fs,
        m,
        h(|ctx, req| async move {
            require_writable(&ctx)?;
            require_local(&req)?;
            let path = req.body.prop("path")?;
            let pth = check_host_path(&ctx, path)?;
            let name = req
                .body
                .prop("name")?
                .and_then(|v| v.as_str())
                .map(String::from);
            let lid = library::num_id(req.param("id")).unwrap_or(-1);
            let row = library::add_folder(&ctx, lid, &pth.to_string_lossy(), name.as_deref())
                .map_err(|e| {
                    if e.message.starts_with("FOREIGN KEY") {
                        HttpError::new(500, "FOREIGN KEY constraint failed")
                    } else {
                        e
                    }
                })?;
            crate::watcher::sync(&ctx);
            rescan(&ctx, [row.id]);
            ok_json(&library::folder_ref(&ctx, &row, None))
        }),
    );
    r.add(
        "PATCH",
        "/api/folders/:id",
        Fs,
        m,
        h(|ctx, req| async move {
            require_writable(&ctx)?;
            let f = library::folder_row_str(&ctx, req.param("id"))?;
            if !f.is_remote() {
                require_local_folder(&f, "Device folders are named on the phone")?;
            }
            let name = match req.body.prop("name")? {
                Some(Value::String(s)) => s.clone(),
                None => {
                    return Err(HttpError::new(
                        500,
                        "Cannot read properties of undefined (reading 'trim')",
                    ))
                }
                Some(other) => crate::http::js_string(other),
            };
            library::rename_folder(&ctx, f.id, &name)?;
            ok_json(&json!({ "ok": true }))
        }),
    );
    r.add(
        "DELETE",
        "/api/folders/:id",
        Fs,
        m,
        h(|ctx, req| async move {
            require_writable(&ctx)?;
            let row = library::folder_row_str(&ctx, req.param("id"))?;
            if !row.is_remote() {
                require_local_folder(
                    &row,
                    "Stop sharing the folder on the phone, or forget the device",
                )?;
            }
            if row.temp != 0 {
                library::delete_temp(&ctx, &row).await?;
            } else {
                library::remove_folder(&ctx, row.id)?;
            }
            if row.is_remote() {
                ctx.remote.forget(row.id);
            }
            ctx.events.emit(ServerEvent::Folders { status: None });
            crate::watcher::sync(&ctx);
            ok_json(&json!({ "ok": true }))
        }),
    );
    r.add(
        "POST",
        "/api/folders/:id/rescan",
        Heavy,
        m,
        h(|ctx, req| async move {
            let id = library::num_id(req.param("id"));
            if let Some(id) = id {
                rescan(&ctx, [id]);
            }
            ok_json(&json!({ "ok": true }))
        }),
    );

    // ── temp sessions ──
    r.add(
        "POST",
        "/api/temp/:id/keep",
        Heavy,
        m,
        h(|ctx, req| async move {
            require_writable(&ctx)?;
            let temp = library::folder_row_str(&ctx, req.param("id"))?;
            if temp.temp == 0 {
                return Err(HttpError::new(400, "Not a temporary session"));
            }
            let fid = req
                .body
                .prop("folderId")?
                .map(crate::http::js_string)
                .unwrap_or_else(|| "undefined".into());
            let target = library::folder_row_str(&ctx, &fid)?;
            require_local_folder(&target, super::util::DEVICE_READONLY)?;
            let dir = req
                .body
                .opt("dir")
                .map(crate::http::js_string)
                .unwrap_or_default();
            let dest_parent = resolve_in_root(&target.path, &dir)?;
            let dest = unique_path(
                &dest_parent,
                if temp.name.is_empty() {
                    "Archive"
                } else {
                    &temp.name
                },
            );
            move_file(Path::new(&temp.path), &dest).await?;
            library::delete_temp(&ctx, &temp).await?;
            let rel_dir = crate::paths::rel_of(&target.path, &dest);
            crate::scanner::scan_folder(&ctx, target.id, "").await;
            ok_json(&json!({ "folderId": target.id.to_string(), "dir": rel_dir }))
        }),
    );
    r.add(
        "POST",
        "/api/temp/:id/extend",
        Fs,
        m,
        h(|ctx, req| async move {
            let temp = library::folder_row_str(&ctx, req.param("id"))?;
            if temp.temp == 0 {
                return Err(HttpError::new(400, "Not a temporary session"));
            }
            let hrs = req
                .body
                .opt("hours")
                .map(pa_core::js::number_of)
                .filter(|n| !n.is_nan() && *n != 0.0)
                .unwrap_or(0.0);
            let hours = hrs.clamp(1.0, 24.0 * 30.0);
            ctx.db.run(
                "UPDATE folders SET expires_at = ? WHERE id = ?",
                p![
                    crate::events::now_ms() + (hours * 3_600_000.0) as i64,
                    temp.id
                ],
            )?;
            ctx.events.emit(ServerEvent::Folders { status: None });
            ok_json(&json!({ "ok": true }))
        }),
    );

    // ── host directory browser ──
    r.add(
        "GET",
        "/api/host/dirs",
        Fs,
        m,
        h(|ctx, req| async move {
            require_local(&req)?;
            let roots = host_roots(&ctx);
            let Some(qp) = req.q("path").filter(|s| !s.is_empty()) else {
                let dirs: Vec<HostDirEntry> = roots.into_iter().map(|r| HostDirEntry { name: r.clone(), path: r }).collect();
                return ok_json(&json!({ "path": null, "parent": null, "dirs": dirs }));
            };
            let p = check_host_path(&ctx, Some(&Value::String(qp.to_string())))?;
            let p2 = p.clone();
            let entries = tokio::task::spawn_blocking(move || std::fs::read_dir(&p2).map(|rd| rd.flatten().collect::<Vec<_>>()))
                .await
                .map_err(HttpError::internal)?
                .map_err(|_| HttpError::new(400, "Cannot read directory"))?;
            let mut dirs: Vec<HostDirEntry> = entries
                .into_iter()
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| !n.starts_with('.'))
                .map(|n| HostDirEntry { path: p.join(&n).to_string_lossy().into_owned(), name: n })
                .collect();
            dirs.sort_by(|a, b| pa_core::locale_compare(&a.name, &b.name));
            let parent = p.parent().map(Path::to_path_buf).unwrap_or_else(|| p.clone());
            let parent_allowed = parent != p && (ctx.cfg.host_browse_roots.is_empty() || ctx.cfg.host_browse_roots.iter().any(|r| is_inside(r, &parent)));
            ok_json(&json!({ "path": p.to_string_lossy(), "parent": if parent_allowed { Some(parent.to_string_lossy().into_owned()) } else { None }, "dirs": dirs }))
        }),
    );
    r.add(
        "POST",
        "/api/host/mkdir",
        Fs,
        m,
        h(|ctx, req| async move {
            require_writable(&ctx)?;
            require_local(&req)?;
            let base = check_host_path(&ctx, req.body.prop("path")?)?;
            let name = req
                .body
                .prop("name")?
                .map(crate::http::js_string)
                .unwrap_or_else(|| "undefined".into());
            let p = base.join(sanitize_name(&name));
            if !is_inside(&base, &p) {
                return Err(HttpError::new(400, "Invalid name"));
            }
            tokio::fs::create_dir_all(&p).await?;
            ok_json(&json!({ "path": p.to_string_lossy() }))
        }),
    );

    // ── realtime ──
    r.add("GET", "/api/events", Global, m, h(events_socket));

    // ── cache (legacy: thumbnails + MEGA together) ──
    r.add(
        "GET",
        "/api/cache",
        Fs,
        m,
        h(|ctx, _req| async move {
            let (a, b) = (ctx.thumbs.size_bytes().await, ctx.remote.cache_size().await);
            ok_json(&json!({ "bytes": a, "remoteBytes": b }))
        }),
    );
    r.add(
        "DELETE",
        "/api/cache",
        Fs,
        m,
        h(|ctx, req| async move {
            require_local(&req)?;
            ctx.thumbs.clear().await;
            ctx.remote.cache_clear().await;
            ctx.db.run("UPDATE media SET thumb_state = 0", p![])?;
            crate::scanner::kick(&ctx);
            ctx.remote.kick();
            ok_json(&json!({ "ok": true }))
        }),
    );
}

async fn events_socket(ctx: C, req: Req) -> ApiResult<axum::http::Response<axum::body::Body>> {
    use axum::extract::ws::{Message, WebSocketUpgrade};
    use axum::extract::FromRequestParts;
    use axum::response::IntoResponse;
    let pairing = crate::devices::can_see_pairing(&ctx, &req);
    let closed = req.conn.closed.clone();
    let mut parts = req.parts;
    let ws = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(ws) => ws,
        Err(_) => return Err(HttpError::new(404, "Not found")),
    };
    let ws = ws.max_message_size(1024 * 1024);
    Ok(ws
        .on_upgrade(move |mut socket| async move {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let hello = serde_json::to_string(&ServerEvent::Hello { info: server_info(&ctx) }).unwrap_or_default();
            if socket.send(Message::Text(hello.into())).await.is_err() {
                return;
            }
            let sub = ctx.events.on(move |e| {
                if matches!(e, ServerEvent::Pair { .. }) && !pairing {
                    return;
                }
                if let Ok(s) = serde_json::to_string(e) {
                    let _ = tx.send(s);
                }
            });
            let mut ping = tokio::time::interval(std::time::Duration::from_secs(25));
            ping.tick().await;
            loop {
                tokio::select! {
                    m = rx.recv() => match m {
                        Some(s) => if socket.send(Message::Text(s.into())).await.is_err() { break },
                        None => break,
                    },
                    _ = ping.tick() => if socket.send(Message::Ping(Vec::new().into())).await.is_err() { break },
                    r = socket.recv() => match r {
                        Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                        _ => {}
                    },
                    _ = closed.cancelled() => break,
                    _ = ctx.shutdown.cancelled() => break,
                }
            }
            drop(sub);
            let _ = socket.send(Message::Close(None)).await;
        })
        .into_response())
}

#[allow(dead_code)]
fn unused(_: &Map<String, Value>) -> axum::http::Response<axum::body::Body> {
    json_response(&json!({}))
}
