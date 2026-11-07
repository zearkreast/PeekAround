//! Mirrors lan-android.integration.test.ts (the desktop LAN share seen from the Android app)

use crate::common::*;
use futures_util::StreamExt;
use serde_json::Value;

const APP_ORIGIN: &str = "http://localhost";

fn lan_address() -> String {
    pa_server::config::interfaces()
        .into_iter()
        .find(|i| !i.internal)
        .map(|i| i.addr.to_string())
        .unwrap_or_else(|| "127.0.0.1".into())
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("0.0.0.0:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn ws_open(url: &str, origin: Option<&str>) -> (bool, Option<u16>, Option<Value>) {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut req = url.into_client_request().unwrap();
    if let Some(o) = origin {
        req.headers_mut().insert("origin", o.parse().unwrap());
    }
    match tokio_tungstenite::connect_async(req).await {
        Ok((mut ws, _)) => {
            let first = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
                .await
                .ok()
                .flatten()
                .and_then(|m| m.ok())
                .and_then(|m| m.into_text().ok())
                .and_then(|t| serde_json::from_str(&t).ok());
            let _ = ws.close(None).await;
            (true, None, first)
        }
        Err(tokio_tungstenite::tungstenite::Error::Http(r)) => {
            (false, Some(r.status().as_u16()), None)
        }
        Err(_) => (false, None, None),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn desktop_lan_share_from_the_android_app() {
    let tmp = Tmp::new("lan");
    let lib = tmp.join("Fotoğraflar");
    jpeg_dated(
        &lib.join("Tatil").join("deniz.jpg"),
        640,
        400,
        "#3b82f6",
        None,
    );
    let mut o = opts(&tmp, &lib, "Fotoğraflar");
    o.mode = pa_core::RunMode::Desktop;
    let srv = pa_server::Server::start(o).await.unwrap();
    let mut img_id = String::new();
    for _ in 0..50 {
        let it = items(&get(&srv, "/api/browse?folderId=1&recursive=1").await.json());
        if it.len() == 1 && it[0]["width"].as_i64().is_some() {
            img_id = s(&it[0]["id"]);
            break;
        }
        sleep(100).await;
    }
    let port = free_port();
    let lan = srv.set_lan(true, Some(port)).await.unwrap();
    assert!(lan.enabled);
    let base = format!("http://{}:{port}", lan_address());
    let c = reqwest::Client::builder().no_proxy().build().unwrap();
    let get_o = |p: &str| c.get(format!("{base}{p}")).header("origin", APP_ORIGIN);

    // the connect screen check
    let r = get_o("/api/info").send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.headers().get("access-control-allow-origin").unwrap(),
        APP_ORIGIN
    );
    let info: Value = r.json().await.unwrap();
    assert_eq!(info["name"], "PeekAround");
    assert_eq!(info["mode"], "desktop");
    assert_eq!(info["lan"]["enabled"], true);
    assert!(info["lan"]["urls"]
        .as_array()
        .unwrap()
        .iter()
        .any(|u| u.as_str().unwrap().ends_with(&format!(":{port}"))));

    // libraries, browse and thumbnails cross-origin
    let libs = get_o("/api/libraries").send().await.unwrap();
    assert_eq!(
        libs.headers().get("access-control-allow-origin").unwrap(),
        APP_ORIGIN
    );
    assert_eq!(
        libs.json::<Value>().await.unwrap()["libraries"][0]["name"],
        "Fotoğraflar"
    );
    let browse: Value = get_o("/api/browse?folderId=1&recursive=1")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(names(&browse), vec!["deniz.jpg"]);
    assert_eq!(
        get_o("/api/browse/skeleton?folderId=1&recursive=1")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let t = get_o(&format!("/api/media/{img_id}/thumb?s=640"))
        .send()
        .await
        .unwrap();
    assert_eq!(t.status(), 200);
    assert!(t
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("image/"));
    let f = get_o(&format!("/api/media/{img_id}/file"))
        .header("range", "bytes=0-9")
        .send()
        .await
        .unwrap();
    assert_eq!(f.status(), 206);

    // the CORS preflight of a linked phone
    let r = c
        .request(reqwest::Method::OPTIONS, format!("{base}/api/settings"))
        .header("origin", APP_ORIGIN)
        .header("access-control-request-method", "PATCH")
        .header(
            "access-control-request-headers",
            "authorization,content-type,x-peek-device",
        )
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    assert_eq!(
        r.headers().get("access-control-allow-origin").unwrap(),
        APP_ORIGIN
    );
    assert!(r
        .headers()
        .get("access-control-allow-methods")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("PATCH"));
    assert!(r
        .headers()
        .get("access-control-allow-headers")
        .unwrap()
        .to_str()
        .unwrap()
        .to_lowercase()
        .contains("x-peek-device"));
    let p = c
        .patch(format!("{base}/api/settings"))
        .header("origin", APP_ORIGIN)
        .header("content-type", "application/json")
        .body(r#"{"showHidden":false}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(p.status(), 200);

    // LAN-only refusals stay readable by the app (403 with CORS)
    let r = get_o("/api/remote/account").send().await.unwrap();
    assert_eq!(r.status(), 403);
    assert_eq!(
        r.headers().get("access-control-allow-origin").unwrap(),
        APP_ORIGIN
    );
    assert_eq!(get(&srv, "/api/remote/account").await.status, 200);

    // the live events socket from the app origin; foreign origins are refused
    let ws = format!("{}/api/events", base.replacen("http", "ws", 1));
    let (ok, _, first) = ws_open(&ws, Some(APP_ORIGIN)).await;
    assert!(ok);
    let first = first.unwrap();
    assert_eq!(first["type"], "hello");
    assert_eq!(first["info"]["name"], "PeekAround");
    assert!(ws_open(&ws, None).await.0);
    let evil = ws_open(&ws, Some("https://evil.example")).await;
    assert_eq!((evil.0, evil.1), (false, Some(403)));

    // reachable after sharing is switched off and on again
    srv.set_lan(false, None).await.unwrap();
    assert!(c.get(format!("{base}/api/info")).send().await.is_err());
    srv.set_lan(true, Some(port)).await.unwrap();
    assert_eq!(get_o("/api/info").send().await.unwrap().status(), 200);
    srv.close().await;
}

#[test]
fn discovery_names_and_lan_order() {
    let s = pa_server::config::mdns_service("DESKTOP-AB12CD", 8080, "1.3.0");
    assert_eq!(s.name, "PeekAround DESKTOP-AB12CD");
    assert_eq!(s.host, "peekaround-desktop-ab12cd.local");
    assert_eq!(
        pa_server::config::mdns_service("Melikşah’s PC", 1, "x").host,
        "peekaround-melik-ah-s-pc.local"
    );
    assert_eq!(
        pa_server::config::mdns_service("---", 1, "x").host,
        "peekaround-pc.local"
    );
}
