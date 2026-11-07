//! Device media in the media routes (mirrors `devices/media.ts`): ranged `/file` through the
//! tunnel, `details` → row refinement, loopback ffmpeg input.

use crate::hub::{DeviceDetails, DeviceHub, ReqOpts, TunnelReply};
use futures_util::StreamExt;
use pa_jobs::library::{content_disposition, http_date, mime_of, single_range, FileReply};
use pa_jobs::HttpError;
use pa_meta::{ExtractedMeta, Tags, Val};
use serde_json::{json, Map, Value};

pub use pa_jobs::stream::internal_input;

/// The media row facts `/file` needs.
#[derive(Debug, Clone)]
pub struct FileRow {
    pub dir: String,
    pub name: String,
    pub ext: String,
    pub size: i64,
}

/// `requireOnline`: 503 'device offline'
pub fn require_online(hub: &DeviceHub, device_id: &str) -> Result<(), HttpError> {
    if hub.is_online(device_id) {
        Ok(())
    } else {
        Err(HttpError::new(503, "device offline"))
    }
}

/// `GET|HEAD /api/media/:id/file` for device media: Range → `file` op with `range` → 206 /
/// Content-Range / Content-Length / Accept-Ranges, streamed straight from the reply PUT.
/// Dropping the reply body (client gone) cancels the transfer on the phone.
pub async fn serve_device_file(
    hub: &DeviceHub,
    device_id: &str,
    key: &str,
    row: &FileRow,
    head: bool,
    range: Option<&str>,
    download: bool,
) -> Result<FileReply, HttpError> {
    require_online(hub, device_id)?;
    let mut headers: Vec<(String, String)> = vec![
        ("Accept-Ranges".into(), "bytes".into()),
        ("Cache-Control".into(), "private, max-age=3600".into()),
    ];
    if download {
        headers.push(("Content-Disposition".into(), content_disposition(&row.name)));
    }
    if head {
        // answered from the index (never drains a tunnelled stream)
        headers.push(("Content-Type".into(), mime_of(&row.ext).into()));
        headers.push(("Content-Length".into(), row.size.to_string()));
        return Ok(FileReply {
            status: 200,
            headers,
            body: None,
        });
    }
    let range = single_range(range);
    let mut args = json!({ "key": key, "p": pa_jobs::paths::join_rel(&row.dir, &row.name) });
    if let Some(r) = &range {
        args["range"] = Value::String(r.clone());
    }
    let tr: TunnelReply = hub
        .request(device_id, "file", args, ReqOpts::prio(1))
        .await?;
    let status = if [200, 206, 416].contains(&tr.status) {
        tr.status
    } else {
        502
    };
    if status == 416 {
        let cr = tr
            .header("x-peek-content-range")
            .map(str::to_string)
            .unwrap_or_else(|| {
                format!(
                    "bytes */{}",
                    tr.header("x-peek-total")
                        .map(str::to_string)
                        .unwrap_or(row.size.to_string())
                )
            });
        headers.push(("Content-Range".into(), cr));
        // drain the (empty) body so the phone's PUT completes
        let mut b = tr.body;
        tokio::spawn(async move { while b.next().await.is_some() {} });
        return Ok(FileReply {
            status,
            headers,
            body: None,
        });
    }
    if status == 502 {
        return Err(HttpError::new(
            502,
            format!("The device answered {}", tr.status),
        ));
    }
    let ct = tr
        .header("content-type")
        .filter(|s| !s.is_empty())
        .unwrap_or(mime_of(&row.ext))
        .to_string();
    headers.push(("Content-Type".into(), ct));
    if let Some(cl) = tr.header("content-length").filter(|s| !s.is_empty()) {
        headers.push(("Content-Length".into(), cl.into()));
    }
    if status == 206 {
        if let Some(cr) = tr.header("x-peek-content-range").filter(|s| !s.is_empty()) {
            headers.push(("Content-Range".into(), cr.into()));
        }
    }
    let mt = tr
        .header("x-peek-mtime")
        .map(pa_core::js::number)
        .unwrap_or(f64::NAN);
    if mt > 0.0 {
        headers.push(("Last-Modified".into(), http_date(mt as i64)));
    }
    Ok(FileReply {
        status,
        headers,
        body: Some(Box::pin(tr.body)),
    })
}

fn to_val(v: &Value) -> Val {
    match v {
        Value::Null => Val::Undef,
        Value::Bool(b) => Val::Bool(*b),
        Value::Number(n) => Val::Num(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => Val::Str(s.clone()),
        Value::Array(a) => Val::Arr(a.iter().map(to_val).collect()),
        Value::Object(o) => Val::Obj(o.iter().map(|(k, v)| (k.clone(), to_val(v))).collect()),
    }
}

/// The exifr-like tag object `deviceDetails` feeds into `applyExif`: the phone's `exif` plus
/// `latitude` / `longitude` (finite gps), `Make`, `Model`, `LensModel`.
pub fn details_tags(d: &DeviceDetails) -> Tags {
    let mut t: Tags = d
        .exif
        .iter()
        .flatten()
        .map(|(k, v)| (k.clone(), if v.is_null() { Val::Undef } else { to_val(v) }))
        .collect();
    // JSON null stays a real null in JS (it overrides); keep it as Undef-like only for absent keys
    for (k, v) in d.exif.iter().flatten() {
        if v.is_null() {
            t.insert(k.clone(), Val::Undef);
        }
    }
    if let Some(g) = &d.gps {
        let (lat, lon) = (
            g.get("lat").and_then(Value::as_f64),
            g.get("lon").and_then(Value::as_f64),
        );
        if let (Some(lat), Some(lon)) =
            (lat.filter(|x| x.is_finite()), lon.filter(|x| x.is_finite()))
        {
            t.insert("latitude".into(), Val::Num(lat));
            t.insert("longitude".into(), Val::Num(lon));
        }
    }
    for (k, v) in [
        ("Make", &d.make),
        ("Model", &d.model),
        ("LensModel", &d.lens),
    ] {
        if let Some(s) = v.as_ref().filter(|s| !s.is_empty()) {
            t.insert(k.into(), Val::Str(s.clone()));
        }
    }
    t
}

/// Refine a row's metadata with the phone's details (`applyExif`, dims kept from the row).
/// `base` = width/height/duration/playback of the row and `instant = taken_at` when its
/// `time_src` is the instant one.
pub fn refine_meta(d: &DeviceDetails, base: ExtractedMeta) -> ExtractedMeta {
    let (w, h) = (base.width, base.height);
    let mut m = base;
    pa_meta::apply_exif(&mut m, &details_tags(d));
    if m.width.is_none() {
        m.width = w;
        m.height = h;
    }
    m
}

/// `MediaDetails.exif` of device media: numbers / nulls as is, strings ≤ 120 UTF-16 units; None when empty.
pub fn details_exif(d: Option<&DeviceDetails>) -> Option<Map<String, Value>> {
    let mut out = Map::new();
    for (k, v) in d.and_then(|d| d.exif.as_ref()).into_iter().flatten() {
        match v {
            Value::Null | Value::Number(_) => {
                out.insert(k.clone(), v.clone());
            }
            Value::String(s) => {
                out.insert(
                    k.clone(),
                    Value::String(pa_core::js::slice16_to(s, 120).to_string()),
                );
            }
            _ => {}
        }
    }
    (!out.is_empty()).then_some(out)
}

/// `MediaDetails.path` of device media: `<device name>/<folder name>/<rel>` (empty parts dropped)
pub fn details_path(device_name: &str, folder_name: &str, rel: &str) -> String {
    [device_name, folder_name, rel]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join("/")
}

/// `deviceFfmpegInput`: 501 until the server knows its loopback URL
pub fn device_ffmpeg_input(hub: &DeviceHub, media_id: i64) -> Result<Vec<String>, HttpError> {
    let url = hub.self_url();
    if url.is_empty() {
        return Err(HttpError::new(
            501,
            "Transcoding device videos is not available",
        ));
    }
    Ok(internal_input(&url, &hub.internal_token, media_id))
}

/// `lazyDeviceStream` (zip entries): the `file` request only starts when the body is first polled.
pub fn lazy_device_stream(
    hub: std::sync::Arc<DeviceHub>,
    device_id: String,
    key: String,
    rel: String,
) -> pa_jobs::library::BodyStream {
    let s = futures_util::stream::once(async move {
        let tr = hub
            .request(
                &device_id,
                "file",
                json!({ "key": key, "p": rel }),
                ReqOpts::prio(1),
            )
            .await
            .map_err(|e| std::io::Error::other(e.message))?;
        if tr.status != 200 {
            return Err(std::io::Error::other(format!(
                "device answered {}",
                tr.status
            )));
        }
        Ok(tr.body)
    })
    .map(|r| match r {
        Ok(b) => b.left_stream(),
        Err(e) => futures_util::stream::once(async move { Err(e) }).right_stream(),
    })
    .flatten();
    Box::pin(s)
}
