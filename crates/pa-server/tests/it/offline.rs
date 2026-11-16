//! A library folder that goes away (unmounted drive, offline share): rows, favorites and the
//! offline flag; rescanned when it is back

use crate::common::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_offline_folder_keeps_its_rows_and_comes_back() {
    let tmp = Tmp::new("offline");
    let lib = tmp.join("lib");
    jpeg_dated(&lib.join("a.jpg"), 64, 40, "#123", None);
    jpeg_dated(&lib.join("Yaz/b.jpg"), 64, 40, "#456", None);
    let srv = pa_server::Server::start(opts(&tmp, &lib, "Lib")).await.unwrap();
    let all = || async { items(&get_ok(&srv, "/api/browse?folderId=1&recursive=1").await) };
    for _ in 0..100 {
        if all().await.len() == 2 {
            break;
        }
        sleep(50).await;
    }
    let id = all().await.iter().find(|i| i["name"] == "a.jpg").unwrap()["id"].as_str().unwrap().to_string();
    send_ok(&srv, "POST", "/api/media/favorite", json!({"ids": [id], "favorite": true})).await;
    let offline = |v: &serde_json::Value| v["libraries"][0]["folders"][0]["offline"].as_bool().unwrap_or(false);
    assert!(!offline(&get_ok(&srv, "/api/libraries").await));

    let away = tmp.join("away");
    std::fs::rename(&lib, &away).unwrap();
    pa_server::library::check_offline(&srv.ctx).await;
    assert!(offline(&get_ok(&srv, "/api/libraries").await));
    send_ok(&srv, "POST", "/api/libraries/1/rescan", json!({})).await;
    sleep(300).await;
    assert_eq!(all().await.len(), 2, "rows survive a scan of a missing folder");

    std::fs::rename(&away, &lib).unwrap();
    jpeg_dated(&lib.join("c.jpg"), 64, 40, "#789", None);
    pa_server::library::check_offline(&srv.ctx).await;
    assert!(!offline(&get_ok(&srv, "/api/libraries").await));
    let now = all().await;
    assert_eq!(now.len(), 3, "rescanned when back");
    assert!(now.iter().any(|i| i["id"] == id.as_str() && i["favorite"] == true), "same id, still a favorite");
    srv.close().await;
}
