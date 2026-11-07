//! Mirrors migration.test.ts, upgrade.test.ts, upgrade-11.test.ts and the migration-005 part of
//! albums-library.integration.test.ts; plus opening a database the TypeScript server created.

use crate::common::*;
use pa_server::db::{Db, MIGRATIONS};
use rusqlite::Connection;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;

pub const OLD_SCHEMA_V10: &str = include_str!("old_schema.sql");

fn geo() -> Arc<pa_server::geo::GeoService> {
    Arc::new(pa_server::geo::GeoService::new(None))
}

fn old_db(file: &Path, settings: Option<serde_json::Value>) {
    let d = Connection::open(file).unwrap();
    d.execute_batch(OLD_SCHEMA_V10).unwrap();
    d.execute_batch("INSERT INTO libraries (id, name, color, icon, created_at) VALUES (1, 'Lib', '#fff', 'images', 0)").unwrap();
    d.execute_batch("INSERT INTO folders (id, library_id, path, name) VALUES (1, 1, '/x', 'x')")
        .unwrap();
    let mut ins = d
        .prepare("INSERT INTO media (folder_id, dir, name, ext, kind, size, mtime, taken_at, meta_state, thumb_state, width, height) VALUES (1, ?, ?, ?, ?, 1, ?, ?, ?, 1, 100, 50)")
        .unwrap();
    ins.execute(rusqlite::params![
        "",
        "exif.jpg",
        "jpg",
        "image",
        1_700_000_000_000i64,
        1_690_000_000_000i64,
        1
    ])
    .unwrap();
    ins.execute(rusqlite::params![
        "",
        "clip.mp4",
        "mp4",
        "video",
        1_700_000_000_000i64,
        1_680_000_000_000i64,
        1
    ])
    .unwrap();
    ins.execute(rusqlite::params![
        "a",
        "plain.png",
        "png",
        "image",
        1_650_000_000_000i64,
        1_650_000_000_000i64,
        1
    ])
    .unwrap();
    ins.execute(rusqlite::params![
        "a",
        "pending.jpg",
        "jpg",
        "image",
        1_660_000_000_000i64,
        1_660_000_000_000i64,
        0
    ])
    .unwrap();
    if let Some(s) = settings {
        d.execute(
            "INSERT INTO settings (key, value) VALUES ('settings', ?)",
            [s.to_string()],
        )
        .unwrap();
    }
}

fn names_of(db: &Db, sql: &str) -> Vec<String> {
    db.all(sql, vec![], |r| r.get::<_, String>("name")).unwrap()
}

#[test]
fn migrations_on_a_1_0_database() {
    let dir = Tmp::new("mig");
    let file = dir.join("peekaround.db");
    old_db(
        &file,
        Some(json!({"zipAction": "ask", "homeTimeZone": "Europe/Istanbul"})),
    );
    let db = Db::open(&file, geo()).unwrap();
    assert_eq!(
        db.int("PRAGMA user_version", vec![]).unwrap(),
        Some(MIGRATIONS.len() as i64)
    );
    let cols = names_of(&db, "PRAGMA table_info(media)");
    for c in [
        "lat",
        "lon",
        "geo_id",
        "cc",
        "admin1",
        "city",
        "make",
        "model",
        "lens",
        "flags",
        "meta_v",
        "taken_local",
        "tz_off",
        "time_src",
    ] {
        assert!(cols.contains(&c.to_string()), "{c}");
    }
    let idx = names_of(
        &db,
        "SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'media'",
    );
    for i in [
        "media_local",
        "media_place",
        "media_model",
        "media_flags",
        "media_metav",
    ] {
        assert!(idx.contains(&i.to_string()), "{i}");
    }
    let rows = db
        .all(
            "SELECT name, taken_at, taken_local, time_src, flags, meta_v FROM media ORDER BY id",
            vec![],
            |r| {
                Ok((
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            },
        )
        .unwrap();
    for r in &rows {
        assert_eq!(r.1, pa_core::instant_to_local(r.0, "Europe/Istanbul"));
        assert_eq!(r.3, 0);
        assert_eq!(r.4, 0);
    }
    assert_eq!(
        rows.iter().map(|r| r.2).collect::<Vec<_>>(),
        vec![1, 3, 0, 0]
    );
    assert_eq!(
        db.int("SELECT count(*) c FROM media", vec![]).unwrap(),
        Some(4)
    );
    db.close();
    assert!(Path::new(&format!("{}.bak-v0", file.display())).exists());
    let again = Db::open(&file, geo()).unwrap();
    assert_eq!(
        again
            .int(
                "SELECT count(*) c FROM media WHERE taken_local IS NULL",
                vec![]
            )
            .unwrap(),
        Some(0)
    );
    again.close();
}

#[test]
fn falls_back_to_the_server_time_zone_without_stored_settings() {
    let dir = Tmp::new("mig");
    let file = dir.join("peekaround.db");
    old_db(&file, None);
    let db = Db::open(&file, geo()).unwrap();
    let (at, local) = db
        .get(
            "SELECT taken_at, taken_local FROM media WHERE name = 'exif.jpg'",
            vec![],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .unwrap()
        .unwrap();
    let tz = pa_server::db::system_time_zone();
    assert_eq!(local, pa_core::instant_to_local(at, &tz));
    assert_eq!(db.home_time_zone(), tz);
    db.close();
}

#[test]
fn creates_the_new_schema_from_scratch() {
    let dir = Tmp::new("mig");
    let db = Db::open(&dir.join("new.db"), geo()).unwrap();
    assert!(names_of(&db, "PRAGMA table_info(media)").contains(&"taken_local".to_string()));
    assert_eq!(
        db.get("SELECT peek_fold('İSTANBUL Kadıköy') v", vec![], |r| {
            r.get::<_, String>(0)
        })
        .unwrap()
        .unwrap(),
        "istanbul kadikoy"
    );
    db.close();
}

#[test]
fn migration_005_creates_album_tables_and_columns() {
    let db = Db::open(Path::new(":memory:"), geo()).unwrap();
    assert_eq!(
        db.int("PRAGMA user_version", vec![]).unwrap(),
        Some(MIGRATIONS.len() as i64)
    );
    let tables = names_of(&db, "SELECT name FROM sqlite_master WHERE type = 'table'");
    assert!(tables.contains(&"albums".to_string()) && tables.contains(&"album_items".to_string()));
    let lc = names_of(&db, "PRAGMA table_info(libraries)");
    assert!(lc.contains(&"cover_media_id".to_string()) && lc.contains(&"options".to_string()));
    assert!(names_of(&db, "PRAGMA table_info(folders)").contains(&"scanned_at".to_string()));
    db.close();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upgrading_a_1_0_data_dir() {
    let tmp = Tmp::new("up");
    let lib = tmp.join("lib");
    let data = tmp.join("data");
    std::fs::create_dir_all(&data).unwrap();
    jpeg_dated(&lib.join("one.jpg"), 40, 20, "#123", None);
    jpeg_dated(&lib.join("a").join("two.jpg"), 20, 40, "#456", None);
    set_mtime(&lib.join("one.jpg"), 1_600_000_000_000);
    set_mtime(&lib.join("a").join("two.jpg"), 1_650_000_000_000);
    let db_file = data.join("peekaround.db");
    {
        let d = Connection::open(&db_file).unwrap();
        d.execute_batch(OLD_SCHEMA_V10).unwrap();
        d.execute_batch("INSERT INTO libraries (id, name, color, icon, created_at) VALUES (1, 'Lib', '#fff', 'images', 0)").unwrap();
        d.execute(
            "INSERT INTO folders (id, library_id, path, name) VALUES (1, 1, ?, 'lib')",
            [lib.to_string_lossy()],
        )
        .unwrap();
        let size = |p: &Path| std::fs::metadata(p).unwrap().len() as i64;
        let sql = "INSERT INTO media (folder_id, dir, name, ext, kind, size, mtime, taken_at, meta_state, thumb_state, width, height, favorite) VALUES (1, ?, ?, 'jpg', 'image', ?, ?, ?, 1, 0, ?, ?, ?)";
        d.execute(
            sql,
            rusqlite::params![
                "",
                "one.jpg",
                size(&lib.join("one.jpg")),
                1_600_000_000_000i64,
                1_600_000_000_000i64,
                40,
                20,
                1
            ],
        )
        .unwrap();
        d.execute(
            sql,
            rusqlite::params![
                "a",
                "two.jpg",
                size(&lib.join("a").join("two.jpg")),
                1_650_000_000_000i64,
                1_650_000_000_000i64,
                20,
                40,
                0
            ],
        )
        .unwrap();
        d.execute(
            "INSERT INTO settings (key, value) VALUES ('settings', ?)",
            [json!({"zipAction": "ask", "editSuffix": "_x"}).to_string()],
        )
        .unwrap();
    }
    let srv = start(&tmp, &lib).await;
    assert!(Path::new(&format!("{}.bak-v0", db_file.display())).exists());
    assert_eq!(
        srv.ctx.db.int("PRAGMA user_version", vec![]).unwrap(),
        Some(MIGRATIONS.len() as i64)
    );
    let st = get_ok(&srv, "/api/settings").await;
    assert_eq!(
        (st["zipAction"].as_str(), st["editSuffix"].as_str()),
        (Some("ask"), Some("_x"))
    );
    assert!(st["homeTimeZone"].is_string());
    let libs = get_ok(&srv, "/api/libraries").await;
    assert_eq!(libs["libraries"].as_array().unwrap().len(), 1);
    assert_eq!(libs["devices"], json!([]));
    let b = get_ok(&srv, "/api/browse?folderId=1&recursive=1").await;
    assert_eq!(names(&b), vec!["two.jpg", "one.jpg"]);
    assert_eq!(
        items(&b).iter().find(|i| i["name"] == "one.jpg").unwrap()["favorite"],
        true
    );
    assert_eq!(
        get_ok(&srv, "/api/browse/skeleton?folderId=1&recursive=1").await["total"],
        2
    );
    assert_eq!(get(&srv, "/api/explore").await.status, 200);
    assert_eq!(get_ok(&srv, "/api/devices").await, json!([]));
    assert_eq!(get_ok(&srv, "/api/search?q=two").await["total"], 1);
    srv.close().await;
    // second start: no new backup, ids stable
    let bak = |d: &Path| {
        let mut v: Vec<String> = std::fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".bak-"))
            .collect();
        v.sort();
        v
    };
    let before = bak(&data);
    let again = start(&tmp, &lib).await;
    assert_eq!(bak(&data), before);
    let b = get_ok(&again, "/api/browse?folderId=1&recursive=1").await;
    let ids: Vec<(String, String)> = items(&b)
        .iter()
        .map(|i| (s(&i["id"]), s(&i["name"])))
        .collect();
    assert_eq!(
        ids,
        vec![
            ("2".into(), "two.jpg".into()),
            ("1".into(), "one.jpg".into())
        ]
    );
    again.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn upgrading_a_1_1_data_dir() {
    let tmp = Tmp::new("up11");
    let lib = tmp.join("lib");
    let data = tmp.join("data");
    std::fs::create_dir_all(&data).unwrap();
    jpeg_dated(&lib.join("one.jpg"), 40, 20, "#123", None);
    jpeg_dated(&lib.join("two.jpg"), 20, 40, "#456", None);
    let db_file = data.join("peekaround.db");
    {
        let old = pa_server::db::open_with_migrations(&db_file, 2).unwrap();
        assert_eq!(
            old.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        old.execute_batch("INSERT INTO libraries (id, name, color, icon, created_at) VALUES (1, 'Lib', '#fff', 'images', 0)").unwrap();
        old.execute(
            "INSERT INTO folders (id, library_id, path, name) VALUES (1, 1, ?, 'lib')",
            [lib.to_string_lossy()],
        )
        .unwrap();
        old.execute_batch(
            "INSERT INTO devices (id, name, created_at, paired) VALUES ('ph1', 'Phone', 0, 1)",
        )
        .unwrap();
        old.execute_batch("INSERT INTO folders (id, library_id, path, name, device_id, device_key) VALUES (2, NULL, 'device://ph1/cam', 'Camera', 'ph1', 'cam')").unwrap();
        let ins = |folder: i64, name: &str, size: i64, mtime: i64, lat: Option<f64>, fav: i64| {
            old.execute(
                "INSERT INTO media (folder_id, dir, name, ext, kind, size, mtime, taken_at, taken_local, time_src, meta_state, meta_v, thumb_state, width, height, favorite, lat, lon, thumbhash)
                 VALUES (?, '', ?, 'jpg', 'image', ?, ?, ?, ?, 1, 1, 1, 1, 40, 20, ?, ?, ?, 'YJqGPQw7sFlslqhFafSE+Q6oJ1h2iHB2Rw')",
                rusqlite::params![folder, name, size, mtime, mtime, mtime, fav, lat, lat.map(|_| 29.0)],
            )
            .unwrap();
        };
        let st = |n: &str| std::fs::metadata(lib.join(n)).unwrap();
        let ms = |m: std::fs::Metadata| {
            m.modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64
        };
        ins(
            1,
            "one.jpg",
            st("one.jpg").len() as i64,
            ms(st("one.jpg")),
            Some(41.0),
            1,
        );
        ins(
            1,
            "two.jpg",
            st("two.jpg").len() as i64,
            ms(st("two.jpg")),
            None,
            0,
        );
        ins(2, "IMG_1.jpg", 1234, 1_650_000_000_000, None, 0);
    }
    let srv = start(&tmp, &lib).await;
    let db = &srv.ctx.db;
    assert!(Path::new(&format!("{}.bak-v2", db_file.display())).exists());
    assert_eq!(
        db.int("PRAGMA user_version", vec![]).unwrap(),
        Some(MIGRATIONS.len() as i64)
    );
    let rows = db
        .all(
            "SELECT id, added_at, mtime, chash, favorite FROM media ORDER BY id",
            vec![],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, i64>(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(rows.len(), 3);
    for r in &rows {
        assert_eq!(r.1, Some(r.2));
        assert_eq!(r.3, None);
    }
    assert_eq!(rows[0].4, 1);
    let a = send_ok(
        &srv,
        "POST",
        "/api/albums",
        json!({"name": "A", "ids": [rows[0].0.to_string(), rows[1].0.to_string()]}),
    )
    .await;
    assert_eq!(a["count"], 2);
    let p = send(
        &srv,
        "PATCH",
        "/api/libraries/1",
        json!({"coverId": rows[1].0.to_string()}),
    )
    .await;
    assert_eq!(p.status, 200, "{}", p.text());
    assert_eq!(get(&srv, "/api/libraries/1/stats").await.status, 200);
    assert_eq!(get(&srv, "/api/explore").await.status, 200);
    assert_eq!(
        get(&srv, "/api/browse/skeleton?color=red").await.status,
        200
    );
    assert_eq!(get(&srv, "/api/browse?daytime=golden").await.status, 200);
    // triggers: a changed thumbhash / location resets the derived columns
    db.run(
        "UPDATE media SET color = 3, sun = 10 WHERE id = ?",
        vec![rows[1].0.into()],
    )
    .unwrap();
    db.run(
        "UPDATE media SET thumbhash = 'x', lat = 1 WHERE id = ?",
        vec![rows[1].0.into()],
    )
    .unwrap();
    let cs = db
        .get(
            "SELECT color, sun FROM media WHERE id = ?",
            vec![rows[1].0.into()],
            |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<f64>>(1)?)),
        )
        .unwrap()
        .unwrap();
    assert_eq!(cs, (None, None));
    db.run("INSERT INTO media (folder_id, dir, name, ext, kind) VALUES (1, '', 'new.jpg', 'jpg', 'image')", vec![]).unwrap();
    assert!(
        db.int("SELECT added_at FROM media WHERE name = 'new.jpg'", vec![])
            .unwrap()
            .unwrap()
            > now_ms() - 60_000
    );
    db.run("DELETE FROM media WHERE id = ?", vec![rows[1].0.into()])
        .unwrap();
    assert_eq!(
        db.int("SELECT count(*) c FROM album_items", vec![])
            .unwrap(),
        Some(1)
    );
    assert_eq!(
        db.int(
            "SELECT cover_media_id c FROM libraries WHERE id = 1",
            vec![]
        )
        .unwrap(),
        None
    );
    db.run("DELETE FROM devices WHERE id = 'ph1'", vec![])
        .unwrap();
    assert_eq!(
        db.int("SELECT count(*) c FROM media WHERE folder_id = 2", vec![])
            .unwrap(),
        Some(0)
    );
    srv.close().await;
    let again = start(&tmp, &lib).await;
    let mut baks: Vec<String> = std::fs::read_dir(&data)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".bak-v") && !n.ends_with(".bak-v0"))
        .collect();
    baks.sort();
    assert_eq!(baks, vec!["peekaround.db.bak-v2"]);
    again.close().await;
}

/// A database created by the TypeScript server (`node dist/cli.js`), opened by the Rust server:
/// same rows, ids, settings; no new migration / backup. Skipped without PEEK_TS_DIR's node build.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn opens_a_database_created_by_the_ts_server() {
    let ts = std::path::PathBuf::from(
        std::env::var("PEEK_TS_DIR").unwrap_or_else(|_| "/home/user/peekaround".into()),
    );
    let cli = ts.join("packages/server/dist/cli.js");
    if !cli.exists()
        || std::process::Command::new("node")
            .arg("--version")
            .output()
            .is_err()
    {
        eprintln!("skipped: no TS server build at {}", cli.display());
        return;
    }
    let tmp = Tmp::new("tsdb");
    let lib = tmp.join("lib");
    jpeg_dated(
        &lib.join("Tatil").join("deniz.jpg"),
        64,
        40,
        "#3b82f6",
        Some("2024:07:01 10:00:00"),
    );
    png(&lib.join("portre.png"), 30, 50, "#f97316");
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut child = std::process::Command::new("node")
        .arg(&cli)
        .args(["-r", &format!("Lib={}", lib.display()), "-d"])
        .arg(tmp.join("data"))
        .args(["-p", &port.to_string(), "--host", "127.0.0.1", "--no-mdns"])
        .current_dir(&ts)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let c = reqwest::Client::builder().no_proxy().build().unwrap();
    let base = format!("http://127.0.0.1:{port}");
    let mut ts_items = serde_json::Value::Null;
    for _ in 0..150 {
        if let Ok(r) = c
            .get(format!(
                "{base}/api/browse?folderId=1&recursive=1&sort=name&order=asc"
            ))
            .send()
            .await
        {
            let v: serde_json::Value = r.json().await.unwrap_or_default();
            if items(&v).len() == 2 && items(&v).iter().all(|i| i["thumbhash"].is_string()) {
                ts_items = v;
                break;
            }
        }
        sleep(100).await;
    }
    let _ = c
        .patch(format!("{base}/api/media/1"))
        .header("content-type", "application/json")
        .body(r#"{"favorite":true}"#)
        .send()
        .await;
    let _ = c
        .post(format!("{base}/api/albums"))
        .header("content-type", "application/json")
        .body(r#"{"name":"TS","ids":["1"]}"#)
        .send()
        .await;
    let ts_settings: serde_json::Value = c
        .get(format!("{base}/api/settings"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(items(&ts_items).len(), 2, "TS server indexed the library");
    let baks = || {
        let mut v: Vec<String> = std::fs::read_dir(tmp.join("data"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".bak-"))
            .collect();
        v.sort();
        v
    };
    let before = baks();
    let srv = start(&tmp, &lib).await;
    assert_eq!(
        srv.ctx.db.int("PRAGMA user_version", vec![]).unwrap(),
        Some(MIGRATIONS.len() as i64)
    );
    assert_eq!(
        baks(),
        before,
        "no migration / backup for an up-to-date TS database"
    );
    let rs = get_ok(
        &srv,
        "/api/browse?folderId=1&recursive=1&sort=name&order=asc",
    )
    .await;
    let key = |v: &serde_json::Value| {
        items(v)
            .iter()
            .map(|i| {
                (
                    s(&i["id"]),
                    s(&i["name"]),
                    i["width"].clone(),
                    i["takenAt"].clone(),
                    i["thumbhash"].clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(key(&rs), key(&ts_items));
    assert_eq!(get_ok(&srv, "/api/settings").await, ts_settings);
    let albums = get_ok(&srv, "/api/albums").await;
    assert_eq!(albums["albums"][0]["name"], "TS");
    assert_eq!(albums["albums"][0]["count"], 1);
    assert_eq!(get_ok(&srv, "/api/media/1").await["favorite"], true);
    srv.close().await;
}
