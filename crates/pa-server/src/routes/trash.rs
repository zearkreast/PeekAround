//! Trash routes: list, thumbnail, restore, delete for good, empty (see `crate::trash`).

use super::util::require_writable;
use crate::ctx::C;
use crate::error::{ApiResult, HttpError};
use crate::http::{ok_json, set_header, BodyScope, Req};
use crate::router::{h, Router};
use crate::security::Limit::{Fs, Global};
use crate::trash;
use axum::body::Body;
use axum::http::Response;
use serde_json::{json, Value};

fn ids(req: &Req) -> Vec<String> {
    match req.body.opt("ids") {
        Some(Value::Array(a)) => a.iter().map(crate::http::js_string).collect(),
        _ => vec![],
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> ApiResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(HttpError::internal)
}

fn changed(ctx: &C, folder_id: i64, rel: &str) {
    let dir = rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    ctx.events
        .media_changed(&folder_id.to_string(), dir, pa_core::MediaReason::Add);
}

pub fn routes(r: &mut Router) {
    let m = BodyScope::Main;
    r.add(
        "GET",
        "/api/trash",
        Global,
        m,
        h(|ctx, _req| async move {
            let c = ctx.clone();
            let items = blocking(move || trash::list(&c)).await?;
            let bytes: i64 = items.iter().map(|i| i.size).sum();
            ok_json(&json!({ "items": items, "bytes": bytes, "keepDays": trash::KEEP_DAYS }))
        }),
    );
    r.add(
        "GET",
        "/api/trash/thumb",
        Global,
        m,
        h(|ctx, req| async move {
            let e = trash::entry(&ctx, req.q("id").unwrap_or(""))?;
            let ext = pa_core::ext_of(&e.rel);
            if pa_core::kind_of(&e.rel) != Some(pa_core::MediaKind::Image) {
                return Err(HttpError::new(415, "No preview"));
            }
            let data = blocking(move || -> Result<Vec<u8>, String> {
                let opts = pa_media::DecodeOptions::default().ext(&ext).fit(480, 480);
                let img = pa_media::decode_path(&e.abs, opts).map_err(|x| x.to_string())?;
                let img = pa_media::resize::into_inside(img, 480, 480, true).map_err(|x| x.to_string())?;
                pa_media::encode::webp(&img, 80.0).map_err(|x| x.to_string())
            })
            .await?
            .map_err(|x| HttpError::new(415, x))?;
            let mut resp = Response::new(Body::from(data));
            set_header(&mut resp, "content-type", "image/webp");
            set_header(&mut resp, "cache-control", "private, max-age=600");
            Ok(resp)
        }),
    );
    r.add(
        "POST",
        "/api/trash/restore",
        Fs,
        m,
        h(|ctx, req| async move {
            require_writable(&ctx)?;
            let mut restored = Vec::new();
            for id in ids(&req) {
                let e = trash::entry(&ctx, &id)?;
                let fid = e.folder.id;
                let rel = blocking(move || trash::restore(&e)).await??;
                crate::scanner::upsert_file(&ctx, fid, &rel).await;
                changed(&ctx, fid, &rel);
                restored.push(json!({ "folderId": fid.to_string(), "rel": rel }));
            }
            ok_json(&json!({ "restored": restored }))
        }),
    );
    r.add(
        "POST",
        "/api/trash/delete",
        Fs,
        m,
        h(|ctx, req| async move {
            require_writable(&ctx)?;
            let mut n = 0;
            for id in ids(&req) {
                let e = trash::entry(&ctx, &id)?;
                blocking(move || trash::delete(&e)).await??;
                n += 1;
            }
            ok_json(&json!({ "deleted": n }))
        }),
    );
    r.add(
        "POST",
        "/api/trash/empty",
        Fs,
        m,
        h(|ctx, _req| async move {
            require_writable(&ctx)?;
            let c = ctx.clone();
            let n = blocking(move || trash::purge(&c, i64::MAX)).await?;
            ok_json(&json!({ "emptied": n }))
        }),
    );
}
