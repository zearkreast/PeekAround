//! Mirrors api.integration.test.ts

use crate::common::*;
use serde_json::json;
use sha2::Digest;

fn sha(f: &std::path::Path) -> String {
    hex::encode(sha2::Sha256::digest(std::fs::read(f).expect("read")))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn api_integration() {
    let tmp = Tmp::new("api");
    let lib = tmp.join("lib");
    let photo = lib.join("Tatil").join("deniz.jpg");
    jpeg_dated(&photo, 640, 400, "#3b82f6", Some("2024:07:01 10:00:00"));
    png(&lib.join("portre.png"), 300, 500, "#f97316");
    let srv = start(&tmp, &lib).await;
    for _ in 0..50 {
        let r = get(&srv, "/api/browse?folderId=1&recursive=1").await.json();
        let it = items(&r);
        if it.len() == 2
            && it
                .iter()
                .all(|x| x["width"].as_i64().is_some_and(|w| w > 0))
        {
            break;
        }
        sleep(100).await;
    }

    // browses folders with metadata and EXIF date
    let r = get_ok(&srv, "/api/browse?folderId=1").await;
    let folders: Vec<String> = r["folders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| s(&f["name"]))
        .collect();
    assert_eq!(folders, vec!["Tatil"]);
    assert_eq!(items(&r).len(), 1);
    let all = get_ok(&srv, "/api/browse?folderId=1&recursive=1").await;
    let deniz = items(&all)
        .into_iter()
        .find(|i| i["name"] == "deniz.jpg")
        .unwrap();
    assert_eq!(
        (deniz["width"].as_i64(), deniz["height"].as_i64()),
        (Some(640), Some(400))
    );
    assert_eq!(
        pa_core::js::utc_year(deniz["takenAt"].as_i64().unwrap()),
        2024
    );

    // serves thumbnails and ranges
    let id = s(&items(&get_ok(&srv, "/api/browse?folderId=1&dir=Tatil").await)[0]["id"]);
    let t = get(&srv, &format!("/api/media/{id}/thumb?s=320")).await;
    assert_eq!(t.status, 200);
    assert!(t.h("content-type").unwrap().contains("image/webp"));
    let r = inj(&srv, "GET", &format!("/api/media/{id}/file"))
        .header("range", "bytes=0-9")
        .send()
        .await;
    assert_eq!(r.status, 206);
    assert_eq!(r.body.len(), 10);

    // uploads with folder structure and unique names
    let body = std::fs::read(&photo).unwrap();
    for _ in 0..2 {
        let r = put(
            &srv,
            "/api/upload?folderId=1&dir=&path=Yeni%2Fa.jpg",
            body.clone(),
        )
        .await;
        assert_eq!(r.status, 200, "{}", r.text());
    }
    let mut ls: Vec<String> = std::fs::read_dir(lib.join("Yeni"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    ls.sort();
    assert_eq!(ls, vec!["a (1).jpg", "a.jpg"]);

    // extracts zips permanently or as temporary sessions
    let zip = zip_bytes(&[("Albüm/1.jpg", &body)]);
    let ex = put(
        &srv,
        "/api/upload?folderId=1&path=arsiv.zip&zip=extract",
        zip.clone(),
    )
    .await
    .json();
    assert_eq!(ex["extractedDir"], "arsiv");
    assert!(lib.join("arsiv").join("Albüm").join("1.jpg").exists());
    let tp = put(
        &srv,
        "/api/upload?folderId=1&path=arsiv.zip&zip=temp",
        zip.clone(),
    )
    .await
    .json();
    let temp = &tp["tempFolder"];
    assert_eq!(temp["temp"], true);
    assert!(!lib.join("arsiv.zip").exists());
    let libs = get_ok(&srv, "/api/libraries").await;
    assert!(libs["temp"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["id"] == temp["id"]));

    // saves edits as a copy and never touches the original
    let before = sha(&photo);
    let id = s(&items(&get_ok(&srv, "/api/browse?folderId=1&dir=Tatil").await)[0]["id"]);
    let png = png_bytes(64, 40, "#000");
    for expected in ["deniz_edit.jpg", "deniz_edit-2.jpg"] {
        let r = inj(
            &srv,
            "POST",
            &format!("/api/edits/save?sourceId={id}&format=jpg"),
        )
        .bytes(png.clone())
        .send()
        .await;
        assert_eq!(r.json()["item"]["name"], expected, "{}", r.text());
    }
    assert_eq!(sha(&photo), before);
    let edited = std::fs::read(lib.join("Tatil").join("deniz_edit.jpg")).unwrap();
    assert!(
        edited.windows(6).any(|w| w == b"Exif\0\0"),
        "capture date carried over (EXIF)"
    );

    // renames, moves and deletes into the trash folder
    let rn = send(
        &srv,
        "POST",
        "/api/fs/rename",
        json!({"folderId": "1", "rel": "portre.png", "name": "portre2.png"}),
    )
    .await;
    assert_eq!(rn.json()["rel"], "portre2.png");
    send_ok(&srv, "POST", "/api/fs/move", json!({"refs": [{"folderId": "1", "rel": "portre2.png"}], "to": {"folderId": "1", "dir": "Tatil"}})).await;
    assert!(lib.join("Tatil").join("portre2.png").exists());
    send_ok(
        &srv,
        "POST",
        "/api/fs/delete",
        json!({"refs": [{"folderId": "1", "rel": "Tatil/portre2.png"}]}),
    )
    .await;
    assert!(!lib.join("Tatil").join("portre2.png").exists());
    assert_eq!(
        std::fs::read_dir(lib.join(".peekaround-trash"))
            .unwrap()
            .count(),
        1
    );

    // accepts custom fonts and reads the family name
    if let Some(font) = [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Medium.ttf",
    ]
    .iter()
    .find(|f| std::path::Path::new(f).exists())
    {
        let name = std::path::Path::new(font)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let r = put(
            &srv,
            &format!("/api/fonts?name={name}"),
            std::fs::read(font).unwrap(),
        )
        .await
        .json();
        let fonts = r["fonts"].as_array().unwrap();
        assert_eq!(fonts.len(), 1);
        assert!(fonts[0]["family"].as_str().unwrap().chars().count() > 2);
        assert_eq!(
            get(&srv, fonts[0]["url"].as_str().unwrap()).await.status,
            200
        );
    }

    // rejects path traversal
    let r = put(&srv, "/api/upload?folderId=1&dir=../../etc&path=x.jpg", "x").await;
    assert_eq!(r.status, 400);
    srv.close().await;
}
