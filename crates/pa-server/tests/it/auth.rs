//! PEEK_PASSWORD: the gate, login / logout, header, query and cookie sessions

use crate::common::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn password_gates_the_api() {
    let tmp = Tmp::new("auth");
    let lib = tmp.join("lib");
    jpeg_dated(&lib.join("a.jpg"), 64, 40, "#123", None);
    let mut o = opts(&tmp, &lib, "Lib");
    o.security = Some(pa_server::config::SecurityOptions {
        password: Some("çok gizli".into()),
        ..Default::default()
    });
    let srv = pa_server::Server::start(o).await.unwrap();

    assert_eq!(get(&srv, "/api/info").await.status, 401);
    assert_eq!(get(&srv, "/api/browse?folderId=1").await.status, 401);
    let a = get_ok(&srv, "/api/auth").await;
    assert_eq!((a["required"].as_bool(), a["ok"].as_bool()), (Some(true), Some(false)));

    let bad = send(&srv, "POST", "/api/auth/login", json!({"password": "yanlış"})).await;
    assert_eq!(bad.status, 401);
    let ok = send(&srv, "POST", "/api/auth/login", json!({"password": "çok gizli", "remember": true})).await;
    assert_eq!(ok.status, 200);
    let token = ok.json()["token"].as_str().unwrap().to_string();
    let cookie = ok.headers.get("set-cookie").cloned().unwrap_or_default();
    assert!(cookie.starts_with("peek_session=") && cookie.contains("HttpOnly"), "{cookie}");

    let with_bearer = inj(&srv, "GET", "/api/info").header("authorization", &format!("Bearer {token}")).send().await;
    assert_eq!(with_bearer.status, 200);
    let with_cookie = inj(&srv, "GET", "/api/info").header("cookie", &format!("peek_session={token}")).send().await;
    assert_eq!(with_cookie.status, 200);
    assert_eq!(get(&srv, &format!("/api/info?psess={token}")).await.status, 200);
    let forged = inj(&srv, "GET", "/api/info").header("authorization", "Bearer 00ff").send().await;
    assert_eq!(forged.status, 401);

    inj(&srv, "POST", "/api/auth/logout")
        .header("authorization", &format!("Bearer {token}"))
        .json(json!({}))
        .send()
        .await;
    let after = inj(&srv, "GET", "/api/info").header("authorization", &format!("Bearer {token}")).send().await;
    assert_eq!(after.status, 401);
    srv.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_password_no_gate() {
    let tmp = Tmp::new("noauth");
    let lib = tmp.join("lib");
    jpeg_dated(&lib.join("a.jpg"), 64, 40, "#123", None);
    let mut o = opts(&tmp, &lib, "Lib");
    o.security = Some(pa_server::config::SecurityOptions::default());
    let srv = pa_server::Server::start(o).await.unwrap();
    assert_eq!(get(&srv, "/api/info").await.status, 200);
    assert_eq!(get_ok(&srv, "/api/auth").await["required"], false);
    srv.close().await;
}
