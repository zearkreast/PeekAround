//! The folder trash: list, thumbnail, restore, delete for good, empty

use crate::common::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn trash_lists_restores_and_empties() {
    let tmp = Tmp::new("trash-api");
    let lib = tmp.join("lib");
    jpeg_dated(&lib.join("Yaz/a.jpg"), 64, 40, "#123", None);
    jpeg_dated(&lib.join("b.jpg"), 64, 40, "#456", None);
    let srv = pa_server::Server::start(opts(&tmp, &lib, "Lib")).await.unwrap();
    for _ in 0..100 {
        if items(&get_ok(&srv, "/api/browse?folderId=1&recursive=1").await).len() == 2 {
            break;
        }
        sleep(50).await;
    }
    send_ok(
        &srv,
        "POST",
        "/api/fs/delete",
        json!({"refs": [{"folderId": "1", "rel": "Yaz/a.jpg"}, {"folderId": "1", "rel": "b.jpg"}]}),
    )
    .await;
    assert!(!lib.join("Yaz/a.jpg").exists());

    let t = get_ok(&srv, "/api/trash").await;
    let list = t["items"].as_array().unwrap().clone();
    let mut rels: Vec<&str> = list.iter().map(|i| i["rel"].as_str().unwrap()).collect();
    rels.sort();
    assert_eq!(rels, ["Yaz/a.jpg", "b.jpg"]);
    assert_eq!(t["keepDays"], 30);
    assert!(list.iter().all(|i| i["kind"] == "image" && i["size"].as_i64().unwrap() > 0));
    let id_of = |rel: &str| list.iter().find(|i| i["rel"] == rel).unwrap()["id"].as_str().unwrap().to_string();

    let th = get(&srv, &format!("/api/trash/thumb?id={}", urlencode(&id_of("b.jpg")))).await;
    assert_eq!(th.status, 200);

    // ids never leave the trash
    let r = send(&srv, "POST", "/api/trash/delete", json!({"ids": ["1:2026-11-15T12-00-00-000Z/../../b.jpg"]})).await;
    assert!(r.status >= 400);

    // restore puts it back where it was, folders included
    send_ok(&srv, "POST", "/api/trash/restore", json!({"ids": [id_of("Yaz/a.jpg")]})).await;
    assert!(lib.join("Yaz/a.jpg").exists());
    // a restored name that is taken again gets "name (1)"
    std::fs::write(lib.join("b.jpg"), b"new").unwrap();
    send_ok(&srv, "POST", "/api/trash/restore", json!({"ids": [id_of("b.jpg")]})).await;
    assert_eq!(std::fs::read(lib.join("b.jpg")).unwrap(), b"new");
    assert!(lib.join("b (1).jpg").exists());
    assert!(!lib.join(".peekaround-trash").exists(), "empty stamp folders are pruned");

    // delete for good, then empty
    send_ok(&srv, "POST", "/api/fs/delete", json!({"refs": [{"folderId": "1", "rel": "b (1).jpg"}, {"folderId": "1", "rel": "Yaz/a.jpg"}]})).await;
    let list = get_ok(&srv, "/api/trash").await["items"].as_array().unwrap().clone();
    let one = list[0]["id"].as_str().unwrap().to_string();
    send_ok(&srv, "POST", "/api/trash/delete", json!({"ids": [one]})).await;
    assert_eq!(get_ok(&srv, "/api/trash").await["items"].as_array().unwrap().len(), 1);
    send_ok(&srv, "POST", "/api/trash/empty", json!({})).await;
    assert_eq!(get_ok(&srv, "/api/trash").await["items"].as_array().unwrap().len(), 0);
    srv.close().await;
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}
