//! Folder names outside the BMP (emoji): SQLite's substr counts characters, not UTF-16 units

use crate::common::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn emoji_folders_keep_their_subfolders() {
    let tmp = Tmp::new("emoji");
    let lib = tmp.join("lib");
    jpeg_dated(&lib.join("Tatil 🏖️/a.jpg"), 64, 40, "#123", None);
    jpeg_dated(&lib.join("Tatil 🏖️/Gün 1/b.jpg"), 64, 40, "#456", None);
    let srv = pa_server::Server::start(opts(&tmp, &lib, "Lib")).await.unwrap();
    let names = |v: &serde_json::Value| {
        let mut n: Vec<String> = items(v)
            .iter()
            .map(|i| i["name"].as_str().unwrap_or("").to_string())
            .collect();
        n.sort();
        n
    };
    let mut all = Vec::new();
    for _ in 0..100 {
        all = names(&get_ok(&srv, "/api/browse?folderId=1&recursive=1").await);
        if all.len() == 2 {
            break;
        }
        sleep(50).await;
    }
    assert_eq!(all, ["a.jpg", "b.jpg"]);
    let under = get_ok(&srv, "/api/browse?folderId=1&dir=Tatil%20%F0%9F%8F%96%EF%B8%8F&recursive=1").await;
    assert_eq!(names(&under), ["a.jpg", "b.jpg"]);

    send_ok(
        &srv,
        "POST",
        "/api/fs/rename",
        json!({"folderId": "1", "rel": "Tatil 🏖️", "name": "Yaz 🌊"}),
    )
    .await;
    let moved = get_ok(&srv, "/api/browse?folderId=1&recursive=1").await;
    let dirs: Vec<String> = items(&moved)
        .iter()
        .map(|i| i["dir"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(dirs.contains(&"Yaz 🌊/Gün 1".to_string()), "{dirs:?}");
    assert!(!dirs.iter().any(|d| d.starts_with("Tatil")), "{dirs:?}");
    srv.close().await;
}
