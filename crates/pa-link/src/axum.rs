//! axum glue (feature `axum`): run a phone socket against the hub, answer reply PUTs / pull GETs,
//! turn [`FileReply`] / [`HttpError`] into responses. Optional — the server may use its own adapters.

use crate::hub::{bearer_of, ConnectMeta, DeviceHub, WsOut, RID_RE};
use crate::transfer::open_pull;
use axum::body::Body;
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use pa_jobs::library::FileReply;
use pa_jobs::HttpError;
use std::sync::Arc;
use std::time::Duration;

/// `{ "error": message }` with the status (+ `Retry-After`)
pub fn error_response(e: &HttpError) -> Response {
    let mut r = (
        StatusCode::from_u16(e.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        axum::Json(serde_json::json!({ "error": e.message })),
    )
        .into_response();
    if let Some(s) = e.retry_after {
        r.headers_mut().insert("Retry-After", HeaderValue::from(s));
    }
    r
}

/// A [`FileReply`] as a streamed response.
pub fn file_response(f: FileReply) -> Response {
    let mut b = Response::builder().status(f.status);
    for (k, v) in &f.headers {
        b = b.header(k.as_str(), v.as_str());
    }
    let body = match f.body {
        Some(s) => Body::from_stream(s),
        None => Body::empty(),
    };
    b.body(body)
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn header<'a>(h: &'a HeaderMap, k: &str) -> Option<&'a str> {
    h.get(k).and_then(|v| v.to_str().ok())
}

/// [`ConnectMeta`] of an upgrade request
pub fn connect_meta(h: &HeaderMap, lan: bool) -> ConnectMeta {
    ConnectMeta {
        lan,
        header_id: header(h, "x-peek-device").map(str::to_string),
        token: bearer_of(header(h, "authorization")),
    }
}

/// `GET /api/devices/connect` after the upgrade: pump frames both ways until the socket closes.
pub async fn serve_socket(hub: Arc<DeviceHub>, socket: WebSocket, meta: ConnectMeta) {
    let (handle, mut out) = hub.attach(meta);
    let (mut tx, mut rx) = socket.split();
    let mut closing: Option<tokio::time::Instant> = None;
    loop {
        let deadline = closing.map(|t| t + Duration::from_secs(5));
        tokio::select! {
            m = rx.next() => match m {
                Some(Ok(Message::Text(t))) => handle.on_text(t.as_str()),
                Some(Ok(Message::Binary(_))) => handle.on_binary(),
                Some(Ok(Message::Pong(_))) => handle.on_pong(),
                Some(Ok(Message::Ping(_))) => {}
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
            },
            o = out.recv() => match o {
                Some(WsOut::Text(s)) => { if tx.send(Message::Text(s.into())).await.is_err() { break } }
                Some(WsOut::Ping) => { let _ = tx.send(Message::Ping(Default::default())).await; }
                Some(WsOut::Close(code, reason)) => {
                    if closing.is_none() {
                        let _ = tx.send(Message::Close(Some(CloseFrame { code, reason: reason.into() }))).await;
                        closing = Some(tokio::time::Instant::now());
                    }
                }
                Some(WsOut::Terminate) | None => break,
            },
            _ = async { match deadline { Some(d) => tokio::time::sleep_until(d).await, None => std::future::pending().await } } => break,
        }
    }
    handle.on_close();
}

/// `PUT /api/devices/reply/:rid` — authenticates, pipes the body, answers 204 / 410 / 499.
pub async fn reply_put(hub: &DeviceHub, rid: &str, headers: &HeaderMap, body: Body) -> Response {
    let device_id = match hub.authenticate(
        header(headers, "x-peek-device"),
        header(headers, "authorization"),
    ) {
        Ok(d) => d,
        Err(e) => return error_response(&e),
    };
    if !RID_RE.is_match(rid) {
        return error_response(&HttpError::new(410, "Unknown request"));
    }
    let hv: Vec<(String, String)> = headers
        .iter()
        .filter_map(|(k, v)| {
            v.to_str()
                .ok()
                .map(|v| (k.as_str().to_string(), v.to_string()))
        })
        .collect();
    let code = hub
        .accept_reply(rid, &device_id, hv, body.into_data_stream())
        .await;
    match code {
        410 => error_response(&HttpError::new(410, "Unknown or expired request")),
        // the consumer went away: tell the phone and drop the connection so it stops sending
        499 => Response::builder()
            .status(499)
            .header("Connection", "close")
            .body(Body::empty())
            .unwrap(),
        c => StatusCode::from_u16(c)
            .unwrap_or(StatusCode::NO_CONTENT)
            .into_response(),
    }
}

/// `GET /api/devices/pull/:token`
pub async fn pull_get(hub: &DeviceHub, token: &str, headers: &HeaderMap) -> Response {
    let r = async {
        let device_id = hub.authenticate(
            header(headers, "x-peek-device"),
            header(headers, "authorization"),
        )?;
        open_pull(hub, token, &device_id).await
    }
    .await;
    match r {
        Ok(f) => file_response(f),
        Err(e) => error_response(&e),
    }
}
