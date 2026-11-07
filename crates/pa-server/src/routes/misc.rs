//! Tiles, cache, jobs, devices and remote routes (mirror `routes/tiles.ts`, `routes/cache.ts`,
//! `routes/jobs.ts`, `routes/devices.ts`, `routes/remote.ts`).

use super::util::{require_local, require_writable};
use crate::error::HttpError;
use crate::http::{ok_json, set_header, BodyScope};
use crate::router::{h, Router};
use crate::security::Limit::{Fs, Global, Heavy, Tile};
use axum::body::Body;
use axum::http::{Response, StatusCode};
use pa_core::MAP_TILE_STYLES;
use serde_json::{json, Value};

fn tile_error(code: u16, kind: &str, msg: &str, status: Option<u16>) -> Response<Body> {
    let mut body = json!({ "error": msg, "reason": kind });
    if let Some(s) = status.filter(|s| *s != 0) {
        body["status"] = json!(s);
    }
    let mut r = crate::http::json_response(&body);
    *r.status_mut() = StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_GATEWAY);
    set_header(&mut r, "cache-control", "no-store");
    r
}

fn is_digits(s: &str) -> bool {
    (1..=7).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
}

fn strip_ext(y: &str) -> &str {
    let l = y.to_ascii_lowercase();
    for e in [".png", ".jpg", ".jpeg"] {
        if l.ends_with(e) {
            return &y[..y.len() - e.len()];
        }
    }
    y
}

pub fn routes(r: &mut Router) {
    let m = BodyScope::Main;
    // ── tiles ──
    r.add(
        "GET",
        "/api/tiles/:style/:z/:x/:y",
        Tile,
        m,
        h(|ctx, req| async move {
            let style = pa_core::parse_tile_style(req.param("style"));
            let y = strip_ext(req.param("y")).to_string();
            let nums = [req.param("z"), req.param("x"), y.as_str()];
            let expose = "X-Peek-Tile, X-Peek-Tile-Error";
            let Some(style) = style.filter(|_| nums.iter().all(|v| is_digits(v))) else {
                let mut r = crate::http::json_response(&json!({ "error": "Invalid tile" }));
                *r.status_mut() = StatusCode::BAD_REQUEST;
                set_header(&mut r, "x-peek-tile-error", "invalid");
                return Ok(r);
            };
            let (z, x, yy) = (
                nums[0].parse().unwrap_or(0),
                nums[1].parse().unwrap_or(0),
                y.parse().unwrap_or(0),
            );
            match ctx.tiles.get(style, z, x, yy).await {
                Ok(t) => {
                    let mut r = Response::new(Body::from(t.body));
                    set_header(&mut r, "content-type", t.content_type);
                    set_header(
                        &mut r,
                        "cache-control",
                        if t.cache == "stale" {
                            "no-cache"
                        } else {
                            "public, max-age=86400"
                        },
                    );
                    set_header(&mut r, "x-peek-tile", t.cache);
                    set_header(&mut r, "access-control-expose-headers", expose);
                    Ok(r)
                }
                Err(e) => {
                    use crate::tiles::TileErrorKind as K;
                    let code = match e.kind {
                        K::Invalid => 400,
                        K::Offline => 504,
                        K::Refused => 502,
                    };
                    let mut r = tile_error(code, e.kind.as_str(), &e.message, Some(e.status));
                    let hdr = if e.kind == K::Refused {
                        format!("refused:{}", e.status)
                    } else {
                        e.kind.as_str().to_string()
                    };
                    set_header(&mut r, "x-peek-tile-error", &hdr);
                    set_header(&mut r, "access-control-expose-headers", expose);
                    Ok(r)
                }
            }
        }),
    );
    r.add(
        "GET",
        "/api/tiles/cache",
        Global,
        m,
        h(|ctx, _req| async move {
            let (b, f, l) = ctx.tiles.stats().await;
            ok_json(&json!({ "bytes": b, "files": f, "limitMb": l }))
        }),
    );
    r.add(
        "DELETE",
        "/api/tiles/cache",
        Global,
        m,
        h(|ctx, req| async move {
            require_local(&req)?;
            ctx.tiles.clear().await;
            let (b, f, l) = ctx.tiles.stats().await;
            ok_json(&json!({ "bytes": b, "files": f, "limitMb": l }))
        }),
    );
    r.add(
        "GET",
        "/api/tiles/status",
        Global,
        m,
        h(|ctx, _req| async move {
            let errs = ctx.tiles.last_error.lock().clone();
            let mut out = serde_json::Map::new();
            for s in MAP_TILE_STYLES {
                out.insert(
                    s.as_str().into(),
                    errs.get(&s)
                        .map(|(k, st, at)| json!({ "reason": k.as_str(), "status": st, "at": at }))
                        .unwrap_or(Value::Null),
                );
            }
            ok_json(&Value::Object(out))
        }),
    );

    // ── cache details ──
    r.add(
        "GET",
        "/api/cache/details",
        Fs,
        m,
        h(|ctx, _req| async move {
            let s = ctx.settings();
            let thumbs = ctx.thumbs.size_bytes().await;
            let (tb, tf, tl) = ctx.tiles.stats().await;
            let remote = ctx.remote.cache_size().await;
            ok_json(&json!({
                "thumbs": { "bytes": thumbs },
                "tiles": { "bytes": tb, "files": tf, "limitMb": tl },
                "remote": { "bytes": remote, "limitMb": s.remote_cache_mb },
            }))
        }),
    );
    r.add(
        "DELETE",
        "/api/cache/:kind",
        Heavy,
        m,
        h(|ctx, req| async move {
            require_local(&req)?;
            let kind = req.param("kind").to_string();
            match kind.as_str() {
                "thumbs" => {
                    ctx.thumbs.clear().await;
                    ctx.db
                        .run("UPDATE media SET thumb_state = 0", crate::p![])?;
                    crate::scanner::kick(&ctx);
                    ctx.remote.kick();
                }
                "tiles" => ctx.tiles.clear().await,
                "remote" => ctx.remote.cache_clear().await,
                _ => return Err(HttpError::new(400, "Unknown cache")),
            }
            ok_json(&json!({ "ok": true, "kind": kind }))
        }),
    );

    // ── jobs ──
    r.add(
        "GET",
        "/api/jobs",
        Global,
        m,
        h(|ctx, _req| async move { ok_json(&json!({ "jobs": ctx.jobs.list() })) }),
    );
    r.add(
        "DELETE",
        "/api/jobs/:id",
        Global,
        m,
        h(|ctx, req| async move {
            ctx.jobs.cancel(req.param("id"));
            ok_json(&json!({ "ok": true }))
        }),
    );
    r.add(
        "DELETE",
        "/api/jobs",
        Global,
        m,
        h(|ctx, _req| async move {
            ctx.jobs.clear_finished();
            ok_json(&json!({ "jobs": ctx.jobs.list() }))
        }),
    );
    r.add(
        "PUT",
        "/api/jobs/assets",
        Fs,
        m,
        h(|ctx, mut req| async move {
            require_writable(&ctx)?;
            req.body_limit = 256 * 1024 * 1024;
            let data = req.read_all().await?;
            let ext = req
                .q("ext")
                .filter(|e| !e.is_empty())
                .unwrap_or("png")
                .to_string();
            let id = ctx
                .jobs
                .save_asset(&data, &ext)
                .await
                .map_err(HttpError::internal)?;
            ok_json(&json!({ "assetId": id }))
        }),
    )
    .body_limit = Some(256 * 1024 * 1024);
    for path in ["/api/jobs/video-edit", "/api/jobs/slideshow"] {
        r.add(
            "POST",
            path,
            Heavy,
            m,
            h(|ctx, req| async move {
                require_writable(&ctx)?;
                let ex = ctx
                    .exporter
                    .get()
                    .cloned()
                    .ok_or_else(|| HttpError::new(503, "Server is starting"))?;
                let body = req.body.value().cloned().unwrap_or(Value::Null);
                let slideshow = req.parts.uri.path().ends_with("/slideshow");
                // Exporter::start_slideshow's future is not provably Send (higher-ranked closure
                // lifetimes inside pa-jobs): drive it on a blocking thread of this runtime
                let rt = tokio::runtime::Handle::current();
                let job = tokio::task::spawn_blocking(move || {
                    rt.block_on(async move {
                        if slideshow {
                            ex.start_slideshow(&body).await
                        } else {
                            ex.start_video_edit(&body).await
                        }
                    })
                })
                .await
                .map_err(HttpError::internal)??;
                ok_json(&job)
            }),
        );
    }

    // ── devices ──
    let d = BodyScope::Devices;
    r.add(
        "GET",
        "/api/devices",
        Global,
        d,
        h(|ctx, req| async move {
            let v = crate::devices::viewer_of(&ctx, &req);
            if !v.devices {
                return ok_json(&json!([]));
            }
            let all = ctx.devices.list(&ctx, v.this_device.as_deref())?;
            let all: Vec<_> = if crate::devices::can_see_pairing(&ctx, &req) {
                all
            } else {
                all.into_iter().filter(|d| d.paired).collect()
            };
            ok_json(&all)
        }),
    );
    r.add(
        "POST",
        "/api/devices/:id/approve",
        Fs,
        d,
        h(|ctx, req| async move {
            crate::devices::require_approver(&ctx, &req, Some(req.param("id")))?;
            ctx.devices.approve(req.param("id"))?;
            ok_json(&json!({ "ok": true }))
        }),
    );
    r.add(
        "POST",
        "/api/devices/:id/reject",
        Fs,
        d,
        h(|ctx, req| async move {
            crate::devices::require_approver(&ctx, &req, Some(req.param("id")))?;
            ctx.devices.reject(req.param("id"))?;
            ok_json(&json!({ "ok": true }))
        }),
    );
    r.add(
        "DELETE",
        "/api/devices/:id",
        Fs,
        d,
        h(|ctx, req| async move {
            crate::devices::require_approver(&ctx, &req, None)?;
            ctx.devices.forget(&ctx, req.param("id"))?;
            ok_json(&json!({ "ok": true }))
        }),
    );
    r.add(
        "GET",
        "/api/devices/connect",
        Global,
        d,
        h(|ctx, req| async move {
            use axum::extract::ws::WebSocketUpgrade;
            use axum::extract::FromRequestParts;
            use axum::response::IntoResponse;
            let lan = req.is_lan();
            let mut parts = req.parts;
            let Ok(ws) = WebSocketUpgrade::from_request_parts(&mut parts, &()).await else {
                return Err(HttpError::new(404, "Not found"));
            };
            let meta = pa_link::axum::connect_meta(&parts.headers, lan);
            let hub = ctx.devices.link.clone();
            Ok(ws
                .max_message_size(1024 * 1024)
                .on_upgrade(move |s| pa_link::axum::serve_socket(hub, s, meta))
                .into_response())
        }),
    );
    r.add(
        "GET",
        "/api/devices/pull/:token",
        Global,
        d,
        h(|ctx, req| async move {
            let auth = crate::devices::device_auth_of(&req);
            let device_id = ctx.devices.authenticate(&ctx, &auth)?;
            let f = pa_link::transfer::open_pull(&ctx.devices.link, req.param("token"), &device_id)
                .await?;
            Ok(pa_link::axum::file_response(f))
        }),
    );
    r.add(
        "PUT",
        "/api/devices/reply/:rid",
        Global,
        BodyScope::Any,
        h(|ctx, mut req| async move {
            let auth = crate::devices::device_auth_of(&req);
            let device_id = ctx.devices.authenticate(&ctx, &auth)?;
            let rid = req.param("rid").to_string();
            if rid.len() != 22
                || !rid
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            {
                return Err(HttpError::new(410, "Unknown request"));
            }
            let headers: Vec<(String, String)> = req
                .parts
                .headers
                .iter()
                .filter_map(|(k, v)| {
                    v.to_str()
                        .ok()
                        .map(|v| (k.as_str().to_string(), v.to_string()))
                })
                .collect();
            let body = req.take_raw().into_data_stream();
            let code = ctx
                .devices
                .link
                .accept_reply(&rid, &device_id, headers, body)
                .await;
            match code {
                410 => Err(HttpError::new(410, "Unknown or expired request")),
                499 => {
                    let mut r = crate::http::empty(499);
                    set_header(&mut r, "connection", "close");
                    Ok(r)
                }
                c => Ok(crate::http::empty(c)),
            }
        }),
    );

    // ── remote (MEGA) ──
    r.add(
        "POST",
        "/api/remote/inspect",
        Heavy,
        m,
        h(|ctx, req| async move {
            let body = req.body.value().cloned().unwrap_or(Value::Null);
            ok_json(&pa_remote::routes::inspect(&ctx.remote.svc, &body).await?)
        }),
    );
    r.add(
        "POST",
        "/api/remote/import",
        Heavy,
        m,
        h(|ctx, req| async move {
            let body = req.body.value().cloned().unwrap_or(Value::Null);
            ok_json(&pa_remote::routes::import(&ctx.remote.svc, &body)?)
        }),
    );
    r.add(
        "POST",
        "/api/remote/folders",
        Heavy,
        m,
        h(|ctx, req| async move {
            let body = req.body.value().cloned().unwrap_or(Value::Null);
            let host = store_of(&ctx);
            ok_json(&pa_remote::routes::create_folder(&ctx.remote.svc, &host, &body).await?)
        }),
    );
    r.add(
        "POST",
        "/api/remote/folders/:id/refresh",
        Heavy,
        m,
        h(|ctx, req| async move {
            let store = store_of(&ctx);
            ok_json(
                &pa_remote::routes::refresh_folder(&ctx.remote.svc, &store, req.param("id"))
                    .await?,
            )
        }),
    );
    r.add(
        "GET",
        "/api/remote/folders/:id/link",
        Fs,
        m,
        h(|ctx, req| async move {
            let store = store_of(&ctx);
            let proxied =
                req.header("x-forwarded-for").is_some() || req.header("forwarded").is_some();
            ok_json(&pa_remote::routes::folder_link(
                &ctx.remote.svc,
                &store,
                req.param("id"),
                ctx.cfg.desktop(),
                req.is_lan(),
                crate::security::is_loopback_ip(&req.conn.ip),
                proxied,
            )?)
        }),
    );
    r.add(
        "POST",
        "/api/remote/copy",
        Heavy,
        m,
        h(|ctx, req| async move {
            let body = req.body.value().cloned().unwrap_or(Value::Null);
            let host = store_of(&ctx);
            ok_json(&pa_remote::routes::copy(&ctx.remote.svc, &host, &body).await?)
        }),
    );
    r.add(
        "GET",
        "/api/remote/account",
        Fs,
        m,
        h(|ctx, req| async move {
            require_local(&req)?;
            ok_json(&ctx.remote.account.account.refreshed_status(false).await)
        }),
    );
    r.add(
        "POST",
        "/api/remote/account/login",
        Heavy,
        m,
        h(|ctx, req| async move {
            require_local(&req)?;
            let body: pa_mega::LoginRequest = req
                .body
                .value()
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default();
            ok_json(&ctx.remote.account.account.login(&body).await?)
        }),
    );
    r.add(
        "POST",
        "/api/remote/account/logout",
        Heavy,
        m,
        h(|ctx, req| async move {
            require_local(&req)?;
            ok_json(&ctx.remote.account.account.logout().await)
        }),
    );
}

/// the engines' DB adapter for a route
pub fn store_of(ctx: &crate::ctx::C) -> crate::engines::Store {
    crate::engines::Store {
        ctx: std::sync::Arc::downgrade(ctx),
        db: ctx.db.clone(),
    }
}
