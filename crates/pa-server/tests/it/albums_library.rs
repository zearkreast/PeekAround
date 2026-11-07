//! Mirrors albums-library.integration.test.ts (the exclusion matcher / options unit tests live in
//! pa-core; migration 005 in migration.rs)

use crate::common::*;
use serde_json::{json, Value};

async fn all(srv: &pa_server::Server) -> Vec<Value> {
    items(&get_ok(srv, "/api/browse?libraryId=1&recursive=1&limit=500").await)
}

async fn by_name(srv: &pa_server::Server, n: &str) -> Value {
    all(srv)
        .await
        .into_iter()
        .find(|i| i["name"] == n)
        .unwrap_or_else(|| panic!("{n} not listed"))
}

async fn sorted_names(srv: &pa_server::Server) -> Vec<String> {
    let mut v: Vec<String> = all(srv).await.iter().map(|i| s(&i["name"])).collect();
    v.sort();
    v
}

async fn wait_count(srv: &pa_server::Server, n: usize) {
    for _ in 0..50 {
        if all(srv).await.len() == n {
            return;
        }
        sleep(100).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn albums_and_library_features() {
    let tmp = Tmp::new("albums");
    let lib = tmp.join("lib");
    jpeg_dated(
        &lib.join("a.jpg"),
        400,
        300,
        "#3b82f6",
        Some("2024:01:01 10:00:00"),
    );
    jpeg_dated(
        &lib.join("Tatil").join("b.jpg"),
        300,
        400,
        "#f97316",
        Some("2024:06:01 10:00:00"),
    );
    jpeg_dated(
        &lib.join("Tatil").join("Gün 2").join("c.jpg"),
        500,
        500,
        "#10b981",
        Some("2023:03:01 10:00:00"),
    );
    jpeg_dated(
        &lib.join("Thumbnails").join("thumb.jpg"),
        400,
        300,
        "#000000",
        None,
    );
    jpeg_dated(&lib.join("icon.jpg"), 8, 8, "#ff0000", None);
    noise_jpeg(&lib.join(".hidden").join("h.jpg"), 200, 150);
    let srv = start(&tmp, &lib).await;
    srv.ctx.events.set_intervals(pa_server::events::Intervals {
        add: 0,
        remove: 0,
        change: 0,
        meta: 0,
        thumb: 0,
    });
    for _ in 0..80 {
        let it = all(&srv).await;
        if it.len() == 5 && it.iter().all(|x| x["width"].as_i64().is_some()) {
            break;
        }
        sleep(100).await;
    }

    // library stats
    let st = get_ok(&srv, "/api/libraries/1/stats").await;
    assert_eq!(
        (
            st["items"].as_i64(),
            st["photos"].as_i64(),
            st["videos"].as_i64(),
            st["folders"].as_i64()
        ),
        (Some(5), Some(5), Some(0), Some(4))
    );
    assert!(st["bytes"].as_i64().unwrap() > 0);
    assert!(st["from"].as_i64().unwrap() < st["to"].as_i64().unwrap());
    assert!(st["scannedAt"].as_i64().unwrap() > 0);

    // exclusions: names, globs, minimum size and hidden folders
    let l1 = send_ok(
        &srv,
        "PATCH",
        "/api/libraries/1",
        json!({"options": {"exclude": ["thumbnails"], "minBytes": 1000}}),
    )
    .await;
    assert_eq!(
        l1["options"],
        json!({"exclude": ["thumbnails"], "minBytes": 1000, "showHidden": null})
    );
    wait_count(&srv, 3).await;
    assert_eq!(sorted_names(&srv).await, vec!["a.jpg", "b.jpg", "c.jpg"]);
    let root = get_ok(&srv, "/api/browse?folderId=1").await;
    assert_eq!(
        root["folders"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| s(&f["name"]))
            .collect::<Vec<_>>(),
        vec!["Tatil"]
    );
    send_ok(
        &srv,
        "PATCH",
        "/api/libraries/1",
        json!({"options": {"showHidden": true}}),
    )
    .await;
    wait_count(&srv, 4).await;
    assert_eq!(
        sorted_names(&srv).await,
        vec!["a.jpg", "b.jpg", "c.jpg", "h.jpg"]
    );
    send_ok(
        &srv,
        "PATCH",
        "/api/libraries/1",
        json!({"options": {"showHidden": null}}),
    )
    .await;
    wait_count(&srv, 3).await;
    assert_eq!(all(&srv).await.len(), 3);

    // watcher / uploads honour exclusions
    let body = std::fs::read(lib.join("a.jpg")).unwrap();
    assert_eq!(
        put(
            &srv,
            "/api/upload?folderId=1&dir=&path=Thumbnails%2Fnew.jpg",
            body
        )
        .await
        .status,
        200
    );
    sleep(150).await;
    assert!(!all(&srv).await.iter().any(|i| i["name"] == "new.jpg"));
    pa_server::scanner::scan_folder(&srv.ctx, 1, ".hidden").await;
    pa_server::scanner::scan_folder(&srv.ctx, 1, "Thumbnails").await;
    assert_eq!(sorted_names(&srv).await, vec!["a.jpg", "b.jpg", "c.jpg"]);

    // library cover + order
    let b = by_name(&srv, "b.jpg").await;
    let l = send_ok(
        &srv,
        "PATCH",
        "/api/libraries/1",
        json!({"coverId": b["id"]}),
    )
    .await;
    assert_eq!(l["coverId"], b["id"]);
    assert_eq!(
        send(
            &srv,
            "PATCH",
            "/api/libraries/1",
            json!({"coverId": "999999"})
        )
        .await
        .status,
        400
    );
    send_ok(&srv, "POST", "/api/libraries", json!({"name": "Second"})).await;
    let ordered = send_ok(
        &srv,
        "POST",
        "/api/libraries/order",
        json!({"ids": ["2", "1"]}),
    )
    .await;
    assert_eq!(
        ordered["libraries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| s(&x["name"]))
            .collect::<Vec<_>>(),
        vec!["Second", "Lib"]
    );

    // album CRUD + items + browse / skeleton filter
    let (a, b, c) = (
        by_name(&srv, "a.jpg").await,
        by_name(&srv, "b.jpg").await,
        by_name(&srv, "c.jpg").await,
    );
    let album = send_ok(
        &srv,
        "POST",
        "/api/albums",
        json!({"name": "  Yaz   2024 ", "ids": [b["id"], a["id"]]}),
    )
    .await;
    let aid = s(&album["id"]);
    assert_eq!(album["name"], "Yaz 2024");
    assert_eq!(album["count"], 2);
    assert_eq!(album["coverId"], b["id"]);
    assert_eq!(album["coverPicked"], false);
    assert_eq!(
        send(&srv, "POST", "/api/albums", json!({"name": " "}))
            .await
            .status,
        400
    );
    let add = send_ok(
        &srv,
        "POST",
        &format!("/api/albums/{aid}/items"),
        json!({"ids": [c["id"], a["id"], "x", 999999]}),
    )
    .await;
    assert_eq!(add["added"], 1);
    assert_eq!(add["album"]["count"], 3);
    let page = get_ok(&srv, &format!("/api/browse?albumId={aid}")).await;
    assert_eq!(page["total"], 3);
    assert_eq!(names(&page), vec!["b.jpg", "a.jpg", "c.jpg"]);
    let sk = get_ok(&srv, &format!("/api/browse/skeleton?albumId={aid}")).await;
    assert_eq!(sk["total"], 3);
    assert_eq!(
        sk["days"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["count"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        vec![1, 1, 1]
    );
    assert_eq!(
        get_ok(
            &srv,
            &format!(
                "/api/browse?albumId={aid}&from={}",
                pa_core::js::date_utc(2024, 0, 1, 0, 0, 0, 0)
            )
        )
        .await["total"],
        2
    );
    assert_eq!(
        names(&get_ok(&srv, &format!("/api/browse?albumId={aid}&byPosition=1")).await),
        vec!["b.jpg", "a.jpg", "c.jpg"]
    );
    send_ok(
        &srv,
        "POST",
        &format!("/api/albums/{aid}/reorder"),
        json!({"ids": [c["id"]], "before": b["id"]}),
    )
    .await;
    assert_eq!(
        names(&get_ok(&srv, &format!("/api/browse?albumId={aid}&byPosition=1")).await),
        vec!["c.jpg", "b.jpg", "a.jpg"]
    );
    let sk2 = get_ok(
        &srv,
        &format!("/api/browse/skeleton?albumId={aid}&byPosition=1"),
    )
    .await;
    assert_eq!(sk2["total"], 3);
    assert_eq!(sk2["days"], Value::Null);
    assert_eq!(
        get_ok(&srv, &format!("/api/albums/{aid}")).await["order"],
        "custom"
    );
    let mem = send_ok(
        &srv,
        "POST",
        "/api/albums/membership",
        json!({"ids": [a["id"], c["id"]]}),
    )
    .await;
    assert_eq!(mem["counts"][&aid], 2);
    let upd = send_ok(
        &srv,
        "PATCH",
        &format!("/api/albums/{aid}"),
        json!({"name": "Yaz", "coverId": c["id"], "order": "asc"}),
    )
    .await;
    assert_eq!(
        (
            upd["name"].clone(),
            upd["coverId"].clone(),
            upd["coverPicked"].clone(),
            upd["order"].clone()
        ),
        (json!("Yaz"), c["id"].clone(), json!(true), json!("asc"))
    );
    let rm = send_ok(
        &srv,
        "POST",
        &format!("/api/albums/{aid}/items/remove"),
        json!({"ids": [c["id"]]}),
    )
    .await;
    assert_eq!(rm["removed"], 1);
    assert_eq!(rm["album"]["count"], 2);
    assert_eq!(rm["album"]["coverPicked"], false);

    // album items survive renames and moves (same id), leave on delete
    let a = by_name(&srv, "a.jpg").await;
    let r = send_ok(
        &srv,
        "POST",
        "/api/fs/rename",
        json!({"folderId": a["folderId"], "rel": "a.jpg", "name": "renamed.jpg"}),
    )
    .await;
    assert_eq!(r["rel"], "renamed.jpg");
    assert_eq!(by_name(&srv, "renamed.jpg").await["id"], a["id"]);
    send_ok(&srv, "POST", "/api/fs/move", json!({"refs": [{"folderId": a["folderId"], "rel": "renamed.jpg"}], "to": {"folderId": a["folderId"], "dir": "Tatil"}})).await;
    let moved = by_name(&srv, "renamed.jpg").await;
    assert_eq!(
        (moved["id"].clone(), moved["dir"].clone()),
        (a["id"].clone(), json!("Tatil"))
    );
    let b = by_name(&srv, "b.jpg").await;
    send_ok(
        &srv,
        "POST",
        "/api/fs/rename",
        json!({"folderId": b["folderId"], "rel": "Tatil", "name": "Yaz"}),
    )
    .await;
    let b2 = by_name(&srv, "b.jpg").await;
    assert_eq!(
        (b2["id"].clone(), b2["dir"].clone()),
        (b["id"].clone(), json!("Yaz"))
    );
    let mut in_album: Vec<String> =
        items(&get_ok(&srv, &format!("/api/browse?albumId={aid}")).await)
            .iter()
            .map(|i| s(&i["id"]))
            .collect();
    in_album.sort();
    let mut want = vec![s(&a["id"]), s(&b["id"])];
    want.sort();
    assert_eq!(in_album, want);
    send_ok(
        &srv,
        "POST",
        "/api/fs/delete",
        json!({"refs": [{"folderId": a["folderId"], "rel": "Yaz/renamed.jpg"}]}),
    )
    .await;
    assert_eq!(
        get_ok(&srv, &format!("/api/albums/{aid}")).await["count"],
        1
    );
    assert_eq!(
        get_ok(&srv, &format!("/api/browse?albumId={aid}")).await["total"],
        1
    );

    // album zip, list order and delete
    let z = get(&srv, &format!("/api/download?albumId={aid}&name=Yaz")).await;
    assert_eq!(z.status, 200);
    assert_eq!(z.h("content-type"), Some("application/zip"));
    let second = send_ok(&srv, "POST", "/api/albums", json!({"name": "İkinci"})).await;
    let al = |v: Value| {
        v["albums"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| s(&x["name"]))
            .collect::<Vec<_>>()
    };
    assert_eq!(al(get_ok(&srv, "/api/albums").await), vec!["İkinci", "Yaz"]);
    send_ok(&srv, "POST", "/api/albums/order", json!({"ids": [aid]})).await;
    assert_eq!(al(get_ok(&srv, "/api/albums").await), vec!["Yaz", "İkinci"]);
    send_ok(
        &srv,
        "DELETE",
        &format!("/api/albums/{}", s(&second["id"])),
        Value::Null,
    )
    .await;
    send_ok(&srv, "DELETE", &format!("/api/albums/{aid}"), Value::Null).await;
    assert_eq!(get_ok(&srv, "/api/albums").await["albums"], json!([]));
    assert_eq!(get(&srv, &format!("/api/albums/{aid}")).await.status, 404);
    assert!(!all(&srv).await.is_empty());

    // reordering a 20k item album stays fast
    let db = &srv.ctx.db;
    db.run("INSERT INTO folders (library_id, path, name, temp) VALUES (NULL, '/nonexistent-big', 'big', 1)", vec![]).unwrap();
    let fid = db
        .int(
            "SELECT id FROM folders WHERE path = '/nonexistent-big'",
            vec![],
        )
        .unwrap()
        .unwrap();
    db.tx(|c| {
        let mut st = c.prepare(
            "INSERT INTO media (folder_id, dir, name, ext, kind) VALUES (?, '', ?, 'jpg', 'image')",
        )?;
        for i in 0..20_000 {
            st.execute(rusqlite::params![fid, format!("x{i}.jpg")])?;
        }
        Ok(())
    })
    .unwrap();
    let ids: Vec<String> = db
        .all(
            "SELECT id FROM media WHERE folder_id = ? ORDER BY id",
            vec![fid.into()],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
        .iter()
        .map(|i| i.to_string())
        .collect();
    let big = send_ok(
        &srv,
        "POST",
        "/api/albums",
        json!({"name": "Big", "ids": ids}),
    )
    .await;
    assert_eq!(big["count"], 20_000);
    let t0 = std::time::Instant::now();
    let rev: Vec<String> = ids.iter().rev().cloned().collect();
    send_ok(
        &srv,
        "POST",
        &format!("/api/albums/{}/reorder", s(&big["id"])),
        json!({"ids": rev, "before": null}),
    )
    .await;
    assert!(
        t0.elapsed().as_millis() < 3000 * crate::slack(),
        "{:?}",
        t0.elapsed()
    );
    let first = db
        .int(
            "SELECT media_id FROM album_items WHERE album_id = ? ORDER BY position LIMIT 1",
            vec![pa_core::js::number(&s(&big["id"])).into()],
        )
        .unwrap()
        .unwrap();
    assert_eq!(first.to_string(), *ids.last().unwrap());
    send_ok(
        &srv,
        "DELETE",
        &format!("/api/albums/{}", s(&big["id"])),
        Value::Null,
    )
    .await;
    db.run("DELETE FROM folders WHERE id = ?", vec![fid.into()])
        .unwrap();

    // sub folders carry their newest date
    let r = get_ok(&srv, "/api/browse?folderId=1").await;
    assert!(
        r["folders"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == "Yaz")
            .unwrap()["latest"]
            .as_i64()
            .unwrap()
            > 0
    );
    srv.close().await;
}
