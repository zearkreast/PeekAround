//! pa-client against a tiny axum mock of the PeekAround API (no pa-server dependency).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use pa_client::{Client, ClientEvent, ConnectionState, DeviceAuth, Error, UploadOptions};
use pa_core::{BrowseQuery, FileRef, KindFilter, ServerEvent, SortDir, SortKey};
use serde_json::{json, Value};

#[derive(Default)]
struct Mock {
    queries: Mutex<Vec<HashMap<String, String>>>,
    headers: Mutex<Vec<(String, String)>>,
    uploads: Mutex<Vec<(HashMap<String, String>, usize)>>,
    ws_connections: AtomicUsize,
    settings: Mutex<Value>,
}

fn info() -> Value {
    json!({"name":"PeekAround","version":"1.3.2","apiVersion":1,"mode":"server","hostname":"vm","scanning":false,
      "caps":{"upload":true,"fileOps":true,"nativeFolders":false,"browseHost":true,"videoJobs":true,"slideshowExport":true,
              "zipDownload":true,"trash":"folder","readonly":false,"lanShare":false},
      "lan":{"enabled":false,"urls":[]}})
}

fn item(id: u32) -> Value {
    json!({"id":id.to_string(),"folderId":"1","dir":"","name":format!("{id}.jpg"),"ext":"jpg","kind":"image","size":1000,
           "mtime":1700000000000i64,"width":400,"height":300,"duration":null,"takenAt":1700000000000i64,"localAt":1700000000000i64,
           "flags":0,"thumbhash":null,"favorite":false,"playback":"direct"})
}

async fn serve() -> (String, Arc<Mock>) {
    let mock = Arc::new(Mock {
        settings: Mutex::new(json!({
            "zipAction":"ask","homeTimeZone":"UTC","mapTiles":"osm","tileCacheMb":512,"editSuffix":"_edit","showHidden":false,
            "tempSessionHours":24,"hostBrowseRoots":[],"remoteCacheMb":2048,"thumbQuality":"normal","indexIntensity":"normal","hwAccel":false
        })),
        ..Default::default()
    });
    let app = Router::new()
        .route("/api/info", get(|| async { Json(info()) }))
        .route(
            "/api/browse",
            get(|State(m): State<Arc<Mock>>, Query(q): Query<HashMap<String, String>>, h: HeaderMap| async move {
                if let Some(d) = h.get("x-peek-device") {
                    m.headers.lock().unwrap().push(("x-peek-device".into(), d.to_str().unwrap().into()));
                }
                let off: u32 = q.get("offset").and_then(|s| s.parse().ok()).unwrap_or(0);
                let lim: u32 = q.get("limit").and_then(|s| s.parse().ok()).unwrap_or(2);
                m.queries.lock().unwrap().push(q);
                let items: Vec<Value> = (off..(off + lim).min(5)).map(item).collect();
                Json(json!({"crumbs":[],"folders":[],"roots":[],"items":items,"total":5,"archives":[],"target":null,"gen":"g1"}))
            }),
        )
        .route(
            "/api/browse/skeleton",
            get(|State(m): State<Arc<Mock>>, Query(q): Query<HashMap<String, String>>| async move {
                m.queries.lock().unwrap().push(q);
                Json(json!({"gen":"g1","total":5,"kindCounts":{"image":5,"video":0,"audio":0},"aspects":"gICAgIA=",
                            "days":[{"key":"2024-07-05","count":3,"place":"Kadıköy"},{"key":"2024-07-01","count":2,"place":null}]}))
            }),
        )
        .route(
            "/api/settings",
            get(|State(m): State<Arc<Mock>>| async move { Json(m.settings.lock().unwrap().clone()) }).patch(
                |State(m): State<Arc<Mock>>, Json(p): Json<Value>| async move {
                    let mut s = m.settings.lock().unwrap();
                    for (k, v) in p.as_object().unwrap() {
                        s[k] = v.clone();
                    }
                    Json(s.clone())
                },
            ),
        )
        .route("/api/media/{id}/thumb", get(|Path(id): Path<String>| async move { ([("content-type", "image/webp")], format!("THUMB:{id}")) }))
        .route(
            "/api/fs/delete",
            post(|Json(_): Json<Value>| async { (StatusCode::FORBIDDEN, Json(json!({"error":"read-only server"}))) }),
        )
        .route("/api/albums", get(|| async { Json(json!({"albums":[]})) }))
        .route("/api/jobs", get(|| async { Json(json!({"jobs":[]})) }))
        .route(
            "/api/upload",
            put(|State(m): State<Arc<Mock>>, Query(q): Query<HashMap<String, String>>, body: Bytes| async move {
                m.uploads.lock().unwrap().push((q, body.len()));
                Json(json!({"created":[{"folderId":"1","dir":"up","name":"a.jpg"}]}))
            }),
        )
        .route(
            "/api/events",
            get(|State(m): State<Arc<Mock>>, ws: WebSocketUpgrade| async move {
                let n = m.ws_connections.fetch_add(1, Ordering::SeqCst);
                ws.on_upgrade(move |s| events(s, n))
            }),
        )
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), mock)
}

/// hello + one media event, then the socket closes (first connection only) → the client reconnects
async fn events(mut s: WebSocket, n: usize) {
    let _ = s
        .send(Message::Text(
            json!({"type":"hello","info":info()}).to_string().into(),
        ))
        .await;
    let _ = s
        .send(Message::Text(r#"{"type":"unknownFutureEvent"}"#.into()))
        .await;
    let _ = s
        .send(Message::Text(
            json!({"type":"media","folderId":"1","dirs":[""],"reason":"thumb"})
                .to_string()
                .into(),
        ))
        .await;
    if n == 0 {
        let _ = s.send(Message::Close(None)).await;
        return;
    }
    tokio::time::sleep(Duration::from_secs(30)).await;
}

#[tokio::test]
async fn http_routes() {
    let (base, mock) = serve().await;
    let c = Client::new(&base).unwrap();
    let info = c.info().await.unwrap();
    assert_eq!(info.api_version, 1);

    let q = BrowseQuery {
        recursive: Some(true),
        sort: Some(SortKey::TakenAt),
        order: Some(SortDir::Desc),
        kind: Some(KindFilter::All),
        offset: Some(2),
        limit: Some(300),
        ..Default::default()
    };
    let sk = c.skeleton(&q).await.unwrap();
    assert_eq!(sk.total, 5);
    assert_eq!(
        sk.days.as_ref().unwrap()[0].place.as_deref(),
        Some("Kadıköy")
    );
    let page = c.browse(&q).await.unwrap();
    assert_eq!(page.items.len(), 3);
    assert_eq!(page.items[0].id, "2");
    {
        let qs = mock.queries.lock().unwrap();
        // skeleton: paging dropped
        assert_eq!(qs[0].get("recursive").map(String::as_str), Some("1"));
        assert!(!qs[0].contains_key("offset") && !qs[0].contains_key("limit"));
        assert_eq!(qs[1].get("offset").map(String::as_str), Some("2"));
        assert_eq!(qs[1].get("sort").map(String::as_str), Some("takenAt"));
    }

    let s = c
        .update_settings(&json!({"showHidden": true}))
        .await
        .unwrap();
    assert!(s.show_hidden);
    assert!(c.settings().await.unwrap().show_hidden);

    let bytes = c.get_bytes(&c.thumb_url("42", 200)).await.unwrap();
    assert_eq!(&bytes[..], b"THUMB:42");

    let err = c
        .remove(&[FileRef {
            folder_id: "1".into(),
            rel: "a.jpg".into(),
        }])
        .await
        .unwrap_err();
    assert_eq!(
        err,
        Error::Api {
            status: 403,
            message: "read-only server".into()
        }
    );
    assert_eq!(err.status(), 403);
    assert!(matches!(
        c.media("nope").await.unwrap_err(),
        Error::Api { status: 404, .. }
    ));

    assert!(c.albums().await.unwrap().is_empty());
    assert!(c.jobs().await.unwrap().is_empty());

    let up = c
        .upload(
            vec![1u8; 1234],
            &UploadOptions {
                folder_id: "1".into(),
                dir: "up".into(),
                path: "a.jpg".into(),
                mtime: Some(5),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(up.created[0].name, "a.jpg");
    {
        let ups = mock.uploads.lock().unwrap();
        assert_eq!(ups[0].1, 1234);
        assert_eq!(ups[0].0.get("dir").map(String::as_str), Some("up"));
        assert_eq!(ups[0].0.get("mtime").map(String::as_str), Some("5"));
    }

    // paired device: headers on requests
    c.set_device_auth(Some(DeviceAuth {
        device_id: "phone-1".into(),
        token: "tok".into(),
    }));
    c.browse(&BrowseQuery::default()).await.unwrap();
    assert_eq!(
        mock.headers.lock().unwrap()[0],
        ("x-peek-device".into(), "phone-1".into())
    );

    // network error
    let dead = Client::new("http://127.0.0.1:9").unwrap();
    assert_eq!(dead.info().await.unwrap_err().status(), 0);
}

#[tokio::test]
async fn events_reconnect() {
    let (base, mock) = serve().await;
    let c = Client::new(&base).unwrap();
    let mut ev = c.events();
    let mut got = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    // connecting → online → hello, media → (closed) connecting → online → hello, media
    while got
        .iter()
        .filter(|e| matches!(e, ClientEvent::Server(ServerEvent::Hello { .. })))
        .count()
        < 2
    {
        let e = tokio::time::timeout_at(deadline, ev.recv())
            .await
            .expect("timed out")
            .unwrap();
        got.push(e);
    }
    assert_eq!(got[0], ClientEvent::Connection(ConnectionState::Connecting));
    assert_eq!(got[1], ClientEvent::Connection(ConnectionState::Online));
    assert!(matches!(
        got[2],
        ClientEvent::Server(ServerEvent::Hello { .. })
    ));
    assert!(
        matches!(&got[3], ClientEvent::Server(ServerEvent::Media { folder_id, .. }) if folder_id == "1")
    );
    assert_eq!(got[4], ClientEvent::Connection(ConnectionState::Connecting));
    assert_eq!(got[5], ClientEvent::Connection(ConnectionState::Online));
    assert_eq!(mock.ws_connections.load(Ordering::SeqCst), 2);
    drop(ev);
}
