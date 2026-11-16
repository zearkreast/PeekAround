//! Writes never replace an existing file: uploads, case-only renames, failed zip extractions

use crate::common::*;
use serde_json::json;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn writes_never_replace_a_file() {
    let tmp = Tmp::new("noclobber");
    let lib = tmp.join("lib");
    jpeg_dated(&lib.join("a.jpg"), 64, 40, "#123", None);
    let srv = pa_server::Server::start(opts(&tmp, &lib, "Lib")).await.unwrap();

    // two uploads of one name at the same time: both files survive
    let url = "/api/upload?folderId=1&dir=&path=same.jpg";
    let (r1, r2) = tokio::join!(put(&srv, url, vec![1u8; 300_000]), put(&srv, url, vec![2u8; 300_000]));
    assert_eq!((r1.status, r2.status), (200, 200));
    let mut got: Vec<Vec<u8>> = ["same.jpg", "same (1).jpg"]
        .iter()
        .map(|n| std::fs::read(lib.join(n)).unwrap_or_default())
        .collect();
    got.sort();
    assert_eq!(got, vec![vec![1u8; 300_000], vec![2u8; 300_000]]);

    // a case-only rename onto another file is a conflict (case-sensitive file systems)
    std::fs::write(lib.join("B.jpg"), b"other").unwrap();
    if !lib.join("b.jpg").exists() {
        let r = send(&srv, "POST", "/api/fs/rename", json!({"folderId": "1", "rel": "a.jpg", "name": "B.jpg"})).await;
        assert_eq!(r.status, 409);
        std::fs::write(lib.join("b.jpg"), b"mine").unwrap();
        let r = send(&srv, "POST", "/api/fs/rename", json!({"folderId": "1", "rel": "b.jpg", "name": "B.jpg"})).await;
        assert_eq!(r.status, 409);
        assert_eq!(std::fs::read(lib.join("B.jpg")).unwrap(), b"other");
    }

    // a broken archive leaves no half-extracted folder
    let r = put(&srv, "/api/upload?folderId=1&dir=&path=bad.zip&zip=extract", b"PK\x03\x04not a zip at all".to_vec()).await;
    assert!(r.status >= 400, "{}", r.status);
    let left: Vec<String> = std::fs::read_dir(&lib)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("bad") || n.ends_with(".part"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
    srv.close().await;
}
