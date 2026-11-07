//! Read-only mode and the desktop trash hook (ServerOptions.readonly / hooks)

use crate::common::*;
use serde_json::json;
use std::sync::{Arc, Mutex};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn readonly_mode_refuses_every_write() {
    let tmp = Tmp::new("ro");
    let lib = tmp.join("lib");
    jpeg_dated(&lib.join("a.jpg"), 64, 40, "#123", None);
    let mut o = opts(&tmp, &lib, "Lib");
    o.readonly = true;
    let srv = pa_server::Server::start(o).await.unwrap();
    let caps = get_ok(&srv, "/api/info").await["caps"].clone();
    assert_eq!(
        (
            caps["readonly"].as_bool(),
            caps["upload"].as_bool(),
            caps["fileOps"].as_bool(),
            caps["trash"].as_str()
        ),
        (Some(true), Some(false), Some(false), Some("none"))
    );
    let ro = "PeekAround is running in read-only mode";
    for (m, u, b) in [
        (
            "POST",
            "/api/fs/mkdir",
            json!({"folderId": "1", "dir": "", "name": "x"}),
        ),
        (
            "POST",
            "/api/fs/rename",
            json!({"folderId": "1", "rel": "a.jpg", "name": "b.jpg"}),
        ),
        (
            "POST",
            "/api/fs/delete",
            json!({"refs": [{"folderId": "1", "rel": "a.jpg"}]}),
        ),
        (
            "POST",
            "/api/fs/move",
            json!({"refs": [{"folderId": "1", "rel": "a.jpg"}], "to": {"folderId": "1", "dir": "x"}}),
        ),
        ("POST", "/api/jobs/video-edit", json!({})),
        ("POST", "/api/remote/folders", json!({"url": "x"})),
    ] {
        let r = send(&srv, m, u, b).await;
        assert_eq!(
            (r.status, r.json()["error"].as_str().map(String::from)),
            (403, Some(ro.to_string())),
            "{u}"
        );
    }
    let r = put(&srv, "/api/upload?folderId=1&dir=&path=x.jpg", "x").await;
    assert_eq!(r.status, 403);
    assert!(lib.join("a.jpg").exists() && !lib.join("x.jpg").exists());
    // reads still work
    assert_eq!(get(&srv, "/api/browse?folderId=1").await.status, 200);
    srv.close().await;
}

struct Trash(Arc<Mutex<Vec<std::path::PathBuf>>>);

impl pa_server::DesktopHooks for Trash {
    fn trash(&self, abs: &std::path::Path) -> Option<std::io::Result<()>> {
        self.0.lock().unwrap().push(abs.to_path_buf());
        Some(std::fs::remove_file(abs))
    }
    fn has_trash(&self) -> bool {
        true
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deletes_go_to_the_os_trash_hook_when_the_desktop_has_one() {
    let tmp = Tmp::new("trash");
    let lib = tmp.join("lib");
    jpeg_dated(&lib.join("a.jpg"), 64, 40, "#123", None);
    let got: Arc<Mutex<Vec<std::path::PathBuf>>> = Arc::default();
    let mut o = opts(&tmp, &lib, "Lib");
    o.mode = pa_core::RunMode::Desktop;
    o.hooks = Some(Arc::new(Trash(got.clone())));
    let srv = pa_server::Server::start(o).await.unwrap();
    assert_eq!(get_ok(&srv, "/api/info").await["caps"]["trash"], "os");
    for _ in 0..50 {
        if !items(&get(&srv, "/api/browse?folderId=1").await.json()).is_empty() {
            break;
        }
        sleep(50).await;
    }
    send_ok(
        &srv,
        "POST",
        "/api/fs/delete",
        json!({"refs": [{"folderId": "1", "rel": "a.jpg"}]}),
    )
    .await;
    assert_eq!(got.lock().unwrap().len(), 1);
    assert!(!lib.join("a.jpg").exists());
    assert!(!lib.join(".peekaround-trash").exists());
    assert!(items(&get_ok(&srv, "/api/browse?folderId=1").await).is_empty());
    srv.close().await;
}

/// The desktop LAN share serves the API (native phone app) but no web UI; the main listener and
/// the Docker / server mode keep serving the web app.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn desktop_lan_listener_serves_no_web_ui() {
    let tmp = Tmp::new("lanweb");
    let lib = tmp.join("lib");
    jpeg_dated(&lib.join("a.jpg"), 64, 40, "#123", None);
    let web = tmp.join("web");
    std::fs::create_dir_all(web.join("assets")).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html><title>PA</title>").unwrap();
    std::fs::write(web.join("assets").join("app.js"), "console.log(1)").unwrap();
    for (mode, lan, web_ok) in [
        (pa_core::RunMode::Desktop, true, false),
        (pa_core::RunMode::Desktop, false, true),
        (pa_core::RunMode::Server, false, true),
    ] {
        let mut o = opts(&tmp, &lib, "Lib");
        o.mode = mode;
        o.web_dir = Some(web.clone());
        let srv = pa_server::Server::start(o).await.unwrap();
        let req = |u: &'static str| {
            let i = inj(&srv, "GET", u);
            if lan {
                i.lan().ip("192.168.1.50")
            } else {
                i
            }
        };
        let want = if web_ok { 200 } else { 404 };
        for u in ["/", "/index.html", "/assets/app.js", "/library/1"] {
            assert_eq!(req(u).send().await.status, want, "{mode:?} lan={lan} {u}");
        }
        // the API stays reachable through every listener
        let r = req("/api/info").send().await;
        assert_eq!(r.status, 200, "{mode:?} lan={lan} /api/info");
        assert_eq!(r.json()["name"], "PeekAround");
        srv.close().await;
    }
}
