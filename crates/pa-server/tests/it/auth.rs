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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn long_and_odd_passwords() {
    let tmp = Tmp::new("auth-long");
    let lib = tmp.join("lib");
    jpeg_dated(&lib.join("a.jpg"), 64, 40, "#123", None);
    // ~10 000 characters, Turkish letters, emoji, quotes and a backslash
    let long: String = "Çok-uzun parola ğüşıöç 🔐 \"tırnak\" \\ ".repeat(300);
    assert!(long.chars().count() > 9000);
    let mut o = opts(&tmp, &lib, "Lib");
    o.security = Some(pa_server::config::SecurityOptions {
        password: Some(long.clone()),
        ..Default::default()
    });
    let srv = pa_server::Server::start(o).await.unwrap();

    let ok = send(&srv, "POST", "/api/auth/login", json!({ "password": long })).await;
    assert_eq!(ok.status, 200);
    assert!(ok.json()["token"].as_str().is_some_and(|t| !t.is_empty()));

    // one character off, a prefix, empty, missing, not a string: refused, never a crash
    let mut off = long.clone();
    off.pop();
    for body in [
        json!({ "password": off }),
        json!({ "password": long.chars().take(100).collect::<String>() }),
        json!({ "password": "" }),
        json!({}),
        json!({ "password": 12345 }),
        json!({ "password": null }),
        json!({ "password": ["x"] }),
    ] {
        let r = send(&srv, "POST", "/api/auth/login", body.clone()).await;
        assert_eq!(r.status, 401, "{body}");
    }
    // a body far over the limit is refused before it is read in full
    let huge = json!({ "password": "x".repeat(200_000) });
    let r = send(&srv, "POST", "/api/auth/login", huge).await;
    assert_eq!(r.status, 413);
    // not JSON at all
    let r = inj(&srv, "POST", "/api/auth/login").bytes(vec![0xff, 0xfe, 0x00, 0x7b]).send().await;
    assert!(r.status >= 400 && r.status < 500, "{}", r.status);
    // the server still answers
    let again = send(&srv, "POST", "/api/auth/login", json!({ "password": long })).await;
    assert_eq!(again.status, 200);
    srv.close().await;
}
