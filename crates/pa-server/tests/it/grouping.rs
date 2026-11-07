//! Mirrors grouping.integration.test.ts (time zones, GPS, places, cameras, flags, filters, the
//! skeleton ↔ pages property test, explore groups, geo points, display renditions, upload mtime,
//! timeline, media event reasons, background enrichment, home zone changes)

use crate::common::*;
use base64::Engine;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const MIN: i64 = 60_000;
const KADIKOY: (f64, f64) = (40.9903, 29.029);
const ANTALYA: (f64, f64) = (36.8841, 30.7056);

fn exif_date(local: i64) -> String {
    let p = pa_core::js::utc_parts(local);
    format!(
        "{:04}:{:02}:{:02} {:02}:{:02}:{:02}",
        p.year,
        p.month0 + 1,
        p.day,
        p.hour,
        p.minute,
        p.second
    )
}

#[derive(Default, Clone)]
struct Shot {
    rel: String,
    w: u32,
    h: u32,
    local: Option<i64>,
    off: Option<&'static str>,
    gps: Option<(f64, f64)>,
    gps_utc: Option<i64>,
    make: Option<&'static str>,
    model: Option<&'static str>,
    lens: Option<&'static str>,
    png: bool,
    mtime: Option<i64>,
}

fn shot(root: &std::path::Path, s: Shot) {
    let f = root.join(&s.rel);
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    if s.png {
        png(&f, s.w, s.h, "#4a7");
    } else {
        let ex = Exif {
            date: s.local.map(exif_date),
            offset: s.off.map(String::from),
            make: s.make.map(String::from),
            model: s.model.map(String::from),
            lens: s.lens.map(String::from),
            gps: s.gps,
            gps_utc: s.gps_utc,
            xmp: None,
        };
        std::fs::write(&f, jpeg_bytes(s.w, s.h, "#4a7", &ex, 70)).unwrap();
    }
    if let Some(m) = s.mtime {
        set_mtime(&f, m);
    }
}

async fn by_name(srv: &pa_server::Server, name: &str) -> Value {
    items(&get_ok(srv, "/api/browse?recursive=1&folderId=1&limit=5000").await)
        .into_iter()
        .find(|i| i["name"] == name)
        .unwrap_or_else(|| panic!("{name}"))
}

async fn total(srv: &pa_server::Server, qs: &str) -> i64 {
    get_ok(srv, &format!("/api/browse?{qs}&limit=1")).await["total"]
        .as_i64()
        .unwrap()
}

fn close(v: &Value, want: f64, digits: i32) -> bool {
    v.as_f64()
        .is_some_and(|x| (x - want).abs() < 0.5 * 10f64.powi(-digits))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn grouping_skeleton_and_explore() {
    let tmp = Tmp::new("group");
    let lib = tmp.join("lib");
    shot(
        &lib,
        Shot {
            rel: "Tatil/selfie.jpg".into(),
            w: 600,
            h: 800,
            local: Some(l("2024-07-01T23:30:00")),
            off: Some("+03:00"),
            gps: Some(KADIKOY),
            make: Some("samsung"),
            model: Some("SM-G991B"),
            lens: Some("Samsung Galaxy S21 front camera 2.2mm f/2.2"),
            ..Default::default()
        },
    );
    shot(
        &lib,
        Shot {
            rel: "Tatil/nikon.jpg".into(),
            w: 900,
            h: 600,
            local: Some(l("2024-07-03T10:00:00")),
            gps: Some(ANTALYA),
            gps_utc: Some(l("2024-07-03T07:00:04")),
            make: Some("NIKON CORPORATION"),
            model: Some("NIKON D750"),
            ..Default::default()
        },
    );
    shot(
        &lib,
        Shot {
            rel: "Tatil/pano.jpg".into(),
            w: 3000,
            h: 1000,
            local: Some(l("2024-07-04T18:00:00")),
            off: Some("+03:00"),
            gps: Some(ANTALYA),
            make: Some("samsung"),
            model: Some("SM-G991B"),
            ..Default::default()
        },
    );
    shot(
        &lib,
        Shot {
            rel: "Screenshot_20240812-101500.png".into(),
            w: 540,
            h: 1170,
            png: true,
            mtime: Some(l("2024-08-12T07:15:00")),
            ..Default::default()
        },
    );
    shot(
        &lib,
        Shot {
            rel: "plain.jpg".into(),
            w: 400,
            h: 400,
            mtime: Some(l("2023-03-03T12:00:00")),
            ..Default::default()
        },
    );
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=160x120:rate=5",
        "-t",
        "1",
        lib.join("anim.gif").to_str().unwrap(),
    ]);
    let mut seed: i64 = 3;
    let mut rnd = || {
        seed = (seed * 16807) % 2147483647;
        seed as f64 / 2147483647.0
    };
    for k in 0..36i64 {
        let a = (rnd() * 1000.0).floor() as i64 * 86_400_000;
        let b = if k % 4 == 0 {
            0
        } else {
            (rnd() * 86_400_000.0).floor() as i64
        };
        let local = l("2022-01-01T00:00:00") + a + b;
        let w = 300 + (rnd() * 200.0).floor() as u32;
        let h = 300 + (rnd() * 200.0).floor() as u32;
        shot(
            &lib,
            Shot {
                rel: format!("Rastgele/{}/r{k}.jpg", if k % 3 != 0 { "a" } else { "b" }),
                w,
                h,
                local: Some(local),
                off: Some("+03:00"),
                gps: if k % 2 == 1 {
                    Some(KADIKOY)
                } else if k % 5 != 0 {
                    Some(ANTALYA)
                } else {
                    None
                },
                make: Some(if k % 3 != 0 { "Apple" } else { "samsung" }),
                model: Some(if k % 3 != 0 { "iPhone 13" } else { "SM-G991B" }),
                ..Default::default()
            },
        );
    }
    std::fs::create_dir_all(lib.join("Videolar")).unwrap();
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=320x240:rate=10",
        "-t",
        "1",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-metadata",
        "creation_time=2025-12-31T21:30:00Z",
        "-metadata",
        "location=+41.0422+029.0067/",
        "-metadata",
        "com.android.manufacturer=Google",
        "-metadata",
        "com.android.model=Pixel 7",
        "-movflags",
        "use_metadata_tags",
        lib.join("Videolar/yilbasi.mp4").to_str().unwrap(),
    ]);
    let srv = start(&tmp, &lib).await;
    let events: Arc<Mutex<Vec<pa_core::ServerEvent>>> = Arc::default();
    let ev2 = events.clone();
    let _sub = srv
        .ctx
        .events
        .on(move |e| ev2.lock().unwrap().push(e.clone()));
    assert_eq!(
        send(
            &srv,
            "PATCH",
            "/api/settings",
            json!({"homeTimeZone": "Europe/Istanbul"})
        )
        .await
        .status,
        200
    );
    let total_files = 36 + 7;
    let mut quiet = 0;
    for _ in 0..1200 {
        let c = count(&srv, "SELECT count(*) c FROM media WHERE meta_state != 0 AND (thumb_state != 0 OR kind NOT IN ('image','video'))");
        quiet = if c >= total_files
            && srv.ctx.scanner.idle()
            && !srv.ctx.explore.dups_busy()
            && !srv.ctx.events.gen_pending()
        {
            quiet + 1
        } else {
            0
        };
        if quiet >= 3 {
            break;
        }
        sleep(100).await;
    }

    // time zone, GPS, place and camera
    let s0 = by_name(&srv, "selfie.jpg").await;
    assert_eq!(s0["localAt"], l("2024-07-01T23:30:00"));
    assert_eq!(s0["takenAt"], l("2024-07-01T20:30:00"));
    assert_eq!(s0["flags"], 2);
    let d = get_ok(&srv, &format!("/api/media/{}", s(&s0["id"]))).await;
    assert_eq!(d["tzOffset"], 180);
    assert!(
        close(&d["gps"]["lat"], KADIKOY.0, 4) && close(&d["gps"]["lon"], KADIKOY.1, 4),
        "{}",
        d["gps"]
    );
    assert_eq!(
        d["place"],
        json!({"city": "Kadıköy", "admin1": "İstanbul", "cc": "TR", "key": "TR-34"})
    );
    assert_eq!(
        d["camera"],
        json!({"make": "Samsung", "model": "SM-G991B", "lens": "Samsung Galaxy S21 front camera 2.2mm f/2.2"})
    );
    let n = get_ok(
        &srv,
        &format!("/api/media/{}", s(&by_name(&srv, "nikon.jpg").await["id"])),
    )
    .await;
    assert_eq!(n["tzOffset"], 180);
    assert_eq!(n["takenAt"], l("2024-07-03T07:00:00"));
    assert_eq!(
        (n["camera"]["make"].as_str(), n["camera"]["model"].as_str()),
        (Some("Nikon"), Some("D750"))
    );
    assert_eq!(
        (n["place"]["admin1"].as_str(), n["place"]["cc"].as_str()),
        (Some("Antalya"), Some("TR"))
    );

    // screenshots, panoramas, animations; videos read location + maker tags
    assert_eq!(
        by_name(&srv, "Screenshot_20240812-101500.png").await["flags"],
        1
    );
    assert_eq!(by_name(&srv, "pano.jpg").await["flags"], 4);
    assert_eq!(by_name(&srv, "anim.gif").await["flags"], 8);
    let v = by_name(&srv, "yilbasi.mp4").await;
    assert_eq!(v["takenAt"], l("2025-12-31T21:30:00"));
    assert_eq!(v["localAt"], l("2026-01-01T00:30:00"));
    let vd = get_ok(&srv, &format!("/api/media/{}", s(&v["id"]))).await;
    assert_eq!(
        (vd["place"]["cc"].as_str(), vd["place"]["admin1"].as_str()),
        (Some("TR"), Some("İstanbul"))
    );
    assert_eq!(
        (
            vd["camera"]["make"].as_str(),
            vd["camera"]["model"].as_str()
        ),
        (Some("Google"), Some("Pixel 7"))
    );
    assert_eq!(vd["tzOffset"], Value::Null);
    let p = by_name(&srv, "plain.jpg").await;
    assert_eq!(p["takenAt"], l("2023-03-03T12:00:00"));
    assert_eq!(p["localAt"], l("2023-03-03T15:00:00"));

    // filters by place, camera, flag, date and free text
    let ist = total(&srv, "place=TR-34").await;
    assert_eq!(
        ist,
        count(
            &srv,
            "SELECT count(*) c FROM media WHERE cc = 'TR' AND admin1 = '34'"
        )
    );
    assert!(ist > 5);
    assert_eq!(
        total(&srv, "place=TR").await,
        count(&srv, "SELECT count(*) c FROM media WHERE cc = 'TR'")
    );
    assert_eq!(total(&srv, "camera=nikon%7Cd750").await, 1);
    assert_eq!(total(&srv, "camera=nope%7Cx").await, 0);
    for (f, want) in [
        ("selfie", 1),
        ("screenshot", 1),
        ("panorama", 1),
        ("animated", 1),
        ("video", 1),
    ] {
        assert_eq!(total(&srv, &format!("flag={f}")).await, want, "{f}");
    }
    assert_eq!(
        total(
            &srv,
            &format!(
                "from={}&to={}",
                l("2024-07-01T00:00:00"),
                l("2024-07-05T00:00:00")
            )
        )
        .await,
        3
    );
    assert_eq!(
        total(
            &srv,
            &format!(
                "from={}&to={}",
                l("2024-07-02T00:00:00"),
                l("2024-07-03T00:00:00")
            )
        )
        .await,
        0
    );
    let tr = total(&srv, "place=TR").await;
    for q in ["ISTANBUL", "istanbul", "İstanbul", "Türkiye"] {
        let e = percent_encoding::utf8_percent_encode(q, percent_encoding::NON_ALPHANUMERIC)
            .to_string();
        assert_eq!(
            total(&srv, &format!("q={e}")).await,
            ist + if q == "Türkiye" { tr - ist } else { 0 },
            "{q}"
        );
    }
    assert_eq!(total(&srv, "q=kad%C4%B1k%C3%B6y%20selfie").await, 1);
    let ant24 = srv.ctx.db.int("SELECT count(*) c FROM media WHERE admin1 = '07' AND name LIKE 'r%' AND taken_local >= ? AND taken_local < ?", vec![l("2024-01-01T00:00:00").into(), l("2025-01-01T00:00:00").into()]).unwrap().unwrap();
    assert_eq!(total(&srv, "q=antalya%202024").await, 2 + ant24);
    assert_eq!(total(&srv, "q=nikon").await, 1);
    assert_eq!(
        total(&srv, "q=ekran%20g%C3%B6r%C3%BCnt%C3%BCs%C3%BC").await,
        1
    );
    assert!(total(&srv, "q=temmuz").await >= 3);
    assert!(
        get_ok(&srv, "/api/search?q=Kad%C4%B1k%C3%B6y").await["total"]
            .as_i64()
            .unwrap()
            > 1
    );

    // skeleton equals the concatenation of browse pages (random filters / sorts)
    let db = &srv.ctx.db;
    db.run("INSERT INTO devices (id, name, token_hash, paired, created_at) VALUES ('skeleton-dev-1', 'P', 'x', 1, 0)", vec![]).unwrap();
    let dev_folder = db.insert("INSERT INTO folders (library_id, path, name, device_id, device_key) VALUES (NULL, 'device://skeleton-dev-1/k', 'Cam', 'skeleton-dev-1', 'k')", vec![]).unwrap();
    let temp_folder = db.insert("INSERT INTO folders (library_id, path, name, temp, expires_at) VALUES (NULL, ?, 'T', 1, ?)", vec![tmp.join("nowhere").to_string_lossy().into_owned().into(), (now_ms() + 3_600_000).into()]).unwrap();
    for (fid, nm) in [
        (dev_folder, "r1.jpg"),
        (dev_folder, "r10.jpg"),
        (temp_folder, "r1.jpg"),
    ] {
        db.run(
            "INSERT INTO media (folder_id, dir, name, ext, kind, size, mtime, taken_at, taken_local, meta_state, meta_v, thumb_state, width, height, cc, admin1) VALUES (?, '', ?, 'jpg', 'image', 1, 1, ?, ?, 1, 99, 1, 10, 10, 'TR', '34')",
            vec![fid.into(), nm.to_string().into(), l("2023-05-05T10:00:00").into(), l("2023-05-05T10:00:00").into()],
        )
        .unwrap();
    }
    srv.ctx.events.bump();
    let mut seed: i64 = 11;
    let mut rnd = move || {
        seed = (seed * 16807) % 2147483647;
        seed as f64 / 2147483647.0
    };
    fn pick<'a>(r: f64, a: &[&'a str]) -> &'a str {
        a[(r * a.len() as f64).floor() as usize]
    }
    for _run in 0..25 {
        let mut params: Vec<(String, String)> = Vec::new();
        let mut set = |k: &str, v: &str| {
            if let Some(e) = params.iter_mut().find(|(a, _)| a == k) {
                e.1 = v.into();
            } else {
                params.push((k.into(), v.into()));
            }
        };
        let sort = pick(rnd(), &["takenAt", "name", "mtime", "size", "kind"]);
        set("sort", sort);
        set("order", pick(rnd(), &["asc", "desc"]));
        let loc = pick(rnd(), &["all", "folder", "dir", "lib"]);
        if loc == "folder" {
            set("folderId", "1");
            set("recursive", "1");
        }
        if loc == "dir" {
            set("folderId", "1");
            set("dir", pick(rnd(), &["Rastgele", "Rastgele/a", "Tatil"]));
            set("recursive", pick(rnd(), &["1", "0"]));
        }
        if loc == "lib" {
            set("libraryId", "1");
        }
        match pick(
            rnd(),
            &["none", "place", "camera", "flag", "date", "q", "kind"],
        ) {
            "place" => set("place", pick(rnd(), &["TR-34", "TR-07", "TR", "DE-16"])),
            "camera" => set(
                "camera",
                pick(rnd(), &["apple|iphone 13", "samsung|sm-g991b"]),
            ),
            "flag" => set("flag", pick(rnd(), &["video", "selfie", "panorama"])),
            "date" => {
                set("from", &l("2022-06-01T00:00:00").to_string());
                set("to", &l("2024-01-01T00:00:00").to_string());
            }
            "q" => set("q", pick(rnd(), &["istanbul", "r1", "2023", "iphone"])),
            "kind" => set("kind", pick(rnd(), &["image", "video", "favorites"])),
            _ => {}
        }
        let qs = params
            .iter()
            .map(|(k, v)| {
                format!(
                    "{k}={}",
                    percent_encoding::utf8_percent_encode(v, percent_encoding::NON_ALPHANUMERIC)
                )
            })
            .collect::<Vec<_>>()
            .join("&");
        let sk = get_ok(&srv, &format!("/api/browse/skeleton?{qs}")).await;
        let aspects = base64::engine::general_purpose::STANDARD
            .decode(sk["aspects"].as_str().unwrap())
            .unwrap();
        assert_eq!(aspects.len() as i64, sk["total"].as_i64().unwrap(), "{qs}");
        let limit = 1 + (rnd() * 7.0).floor() as i64;
        let mut all: Vec<Value> = Vec::new();
        let mut gen;
        let mut off = 0;
        loop {
            let p = get_ok(
                &srv,
                &format!("/api/browse?{qs}&offset={off}&limit={limit}"),
            )
            .await;
            assert_eq!(p["total"], sk["total"], "{qs}");
            gen = p["gen"].clone();
            all.extend(items(&p));
            if off + limit >= p["total"].as_i64().unwrap() {
                break;
            }
            off += limit;
        }
        assert_eq!(gen, sk["gen"], "{qs}");
        assert_eq!(all.len() as i64, sk["total"].as_i64().unwrap(), "{qs}");
        if loc != "dir" && loc != "folder" {
            assert!(all.iter().all(|it| it["folderId"] == "1"), "{qs}");
        }
        for (i, it) in all.iter().enumerate() {
            let a = match (it["width"].as_f64(), it["height"].as_f64()) {
                (Some(w), Some(h)) if w > 0.0 && h > 0.0 => w / h,
                _ => 1.0,
            };
            assert!(
                (pa_core::byte_to_aspect(aspects[i]) / a).log2().abs() < 0.02,
                "{qs} #{i}"
            );
        }
        let mut kinds = json!({"image": 0, "video": 0, "audio": 0});
        for it in &all {
            let k = s(&it["kind"]);
            if let Some(n) = kinds.get_mut(&k) {
                *n = json!(n.as_i64().unwrap() + 1);
            }
        }
        assert_eq!(sk["kindCounts"], kinds, "{qs}");
        if sort == "takenAt" {
            let days = sk["days"].as_array().unwrap();
            assert_eq!(
                days.iter()
                    .map(|d| d["count"].as_i64().unwrap())
                    .sum::<i64>(),
                sk["total"].as_i64().unwrap()
            );
            let flat: Vec<String> = days
                .iter()
                .flat_map(|d| {
                    std::iter::repeat_n(s(&d["key"]), d["count"].as_u64().unwrap() as usize)
                })
                .collect();
            let want: Vec<String> = all
                .iter()
                .map(|it| pa_core::local_day_key(it["localAt"].as_i64().unwrap()))
                .collect();
            assert_eq!(flat, want, "{qs}");
        } else {
            assert_eq!(sk["days"], Value::Null, "{qs}");
        }
    }
    let lib_r1 = count(&srv, "SELECT count(*) c FROM media WHERE folder_id = 1 AND kind IN ('image','video','audio') AND instr(peek_search(name, city, cc, admin1, make, model), 'r1') > 0");
    assert_eq!(
        get_ok(&srv, "/api/browse/skeleton?q=r1").await["total"],
        lib_r1
    );
    assert_eq!(get_ok(&srv, "/api/search?q=r1").await["total"], lib_r1);
    assert_eq!(
        get_ok(
            &srv,
            &format!("/api/browse/skeleton?folderId={dev_folder}&q=r1")
        )
        .await["total"],
        2
    );
    db.run(
        "DELETE FROM folders WHERE id IN (?, ?)",
        vec![dev_folder.into(), temp_folder.into()],
    )
    .unwrap();
    db.run("DELETE FROM devices WHERE id = 'skeleton-dev-1'", vec![])
        .unwrap();
    srv.ctx.events.bump();

    // skeleton days carry the dominant city
    let sk = get_ok(&srv, "/api/browse/skeleton?folderId=1&dir=Tatil").await;
    assert_eq!(
        sk["days"],
        json!([{"key": "2024-07-04", "count": 1, "place": "Muratpaşa"}, {"key": "2024-07-03", "count": 1, "place": "Muratpaşa"}, {"key": "2024-07-01", "count": 1, "place": "Kadıköy"}])
    );

    // explore groups places, types, cameras, years
    let ex = get_ok(&srv, "/api/explore").await;
    let places = ex["places"].as_array().unwrap();
    let istp = places.iter().find(|p| p["key"] == "TR-34").unwrap();
    assert_eq!(
        (istp["cc"].as_str(), istp["name"].as_str()),
        (Some("TR"), Some("İstanbul"))
    );
    assert!((istp["lat"].as_f64().unwrap() - 41.0).abs() < 0.5);
    assert_eq!(
        places.iter().find(|p| p["key"] == "TR-07").unwrap()["name"],
        "Antalya"
    );
    let mut types: Vec<String> = ex["types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| s(&t["flag"]))
        .collect();
    types.sort();
    assert_eq!(
        types,
        vec!["animated", "panorama", "screenshot", "selfie", "video"]
    );
    let cams: Vec<String> = ex["cameras"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| s(&c["key"]))
        .collect();
    for k in [
        "nikon|d750",
        "samsung|sm-g991b",
        "apple|iphone 13",
        "google|pixel 7",
    ] {
        assert!(cams.contains(&k.to_string()), "{k}");
    }
    let nik = ex["cameras"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["key"] == "nikon|d750")
        .unwrap();
    assert_eq!(
        (
            nik["make"].as_str(),
            nik["model"].as_str(),
            nik["count"].as_i64()
        ),
        (Some("Nikon"), Some("D750"), Some(1))
    );
    let years: Vec<i64> = ex["years"]
        .as_array()
        .unwrap()
        .iter()
        .map(|y| y["year"].as_i64().unwrap())
        .collect();
    let mut sorted = years.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(years, sorted);
    assert_eq!(
        ex["years"]
            .as_array()
            .unwrap()
            .iter()
            .map(|y| y["count"].as_i64().unwrap())
            .sum::<i64>(),
        count(
            &srv,
            "SELECT count(*) c FROM media WHERE kind IN ('image','video')"
        )
    );
    assert_eq!(ex["indexing"]["done"], ex["indexing"]["total"]);
    for p in places {
        assert_eq!(
            total(&srv, &format!("place={}", s(&p["key"]))).await,
            p["count"].as_i64().unwrap()
        );
    }
    for c in ex["cameras"].as_array().unwrap() {
        let e = percent_encoding::utf8_percent_encode(
            c["key"].as_str().unwrap(),
            percent_encoding::NON_ALPHANUMERIC,
        )
        .to_string();
        assert_eq!(
            total(&srv, &format!("camera={e}")).await,
            c["count"].as_i64().unwrap()
        );
    }
    for t in ex["types"].as_array().unwrap() {
        assert_eq!(
            total(&srv, &format!("flag={}&kind=image-video", s(&t["flag"]))).await,
            t["count"].as_i64().unwrap()
        );
    }

    // geo points with the browse filters
    let pts = get_ok(&srv, "/api/geo/points?place=TR-07").await;
    let pts = pts["points"].as_array().unwrap();
    assert_eq!(pts.len() as i64, total(&srv, "place=TR-07").await);
    for p in pts {
        assert!(p[0].is_string());
        assert!(
            (p[1].as_f64().unwrap() - ANTALYA.0).abs() < 0.05
                && (p[2].as_f64().unwrap() - ANTALYA.1).abs() < 0.05
        );
    }

    // bounded display renditions for zoom
    let pano = by_name(&srv, "pano.jpg").await;
    let r = get(
        &srv,
        &format!("/api/media/{}/display?max=2048", s(&pano["id"])),
    )
    .await;
    assert_eq!(r.status, 200);
    assert!(r.h("content-type").unwrap().contains("image/jpeg"));
    let (w, h) = dims(&r.body);
    assert_eq!(w.max(h), 2048);
    assert_eq!(
        get(
            &srv,
            &format!(
                "/api/media/{}/display?max=4096",
                s(&by_name(&srv, "plain.jpg").await["id"])
            )
        )
        .await
        .status,
        302
    );
    assert_eq!(
        get(
            &srv,
            &format!(
                "/api/media/{}/display",
                s(&by_name(&srv, "Screenshot_20240812-101500.png").await["id"])
            )
        )
        .await
        .status,
        302
    );

    // keeps upload mtime and changes gen
    let before = get_ok(&srv, "/api/browse?folderId=1&dir=Yukle&limit=1").await;
    let mtime = l("2021-05-06T07:08:09");
    let r = put(
        &srv,
        &format!("/api/upload?folderId=1&dir=Yukle&path=up.jpg&mtime={mtime}"),
        std::fs::read(lib.join("plain.jpg")).unwrap(),
    )
    .await;
    assert_eq!(r.status, 200);
    let mt = std::fs::metadata(lib.join("Yukle/up.jpg"))
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert_eq!(mt, mtime);
    let after = get_ok(&srv, "/api/browse?folderId=1&dir=Yukle&limit=1").await;
    assert_eq!(
        after["total"].as_i64().unwrap(),
        before["total"].as_i64().unwrap() + 1
    );
    assert_ne!(after["gen"], before["gen"]);
    assert_eq!(items(&after)[0]["takenAt"], mtime);

    // timeline buckets use the local wall clock
    let t = get_ok(&srv, "/api/timeline").await;
    assert_eq!(
        t["buckets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["key"] == "2026-01")
            .unwrap()["count"],
        1
    );

    // tags media events with a reason
    {
        let evs = events.lock().unwrap();
        let media: Vec<_> = evs
            .iter()
            .filter_map(|e| {
                if let pa_core::ServerEvent::Media { reason, .. } = e {
                    Some(*reason)
                } else {
                    None
                }
            })
            .collect();
        assert!(!media.is_empty());
        assert!(media.iter().all(|r| r.is_some()));
        assert!(media.contains(&Some(pa_core::MediaReason::Add)));
    }

    // enriches old rows in the background without per-row media events
    let snap = |srv: &pa_server::Server| {
        srv.ctx.db.all("SELECT id, cc, admin1, city, make, model, flags, taken_local, tz_off FROM media ORDER BY id", vec![], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, Option<String>>(5)?, r.get::<_, i64>(6)?, r.get::<_, Option<i64>>(7)?, r.get::<_, Option<i64>>(8)?))
        })
        .unwrap()
    };
    let snapshot = snap(&srv);
    db.run("UPDATE media SET meta_v = 0, cc = NULL, admin1 = NULL, city = NULL, make = NULL, model = NULL, flags = 0, lat = NULL, lon = NULL WHERE kind IN ('image','video')", vec![]).unwrap();
    let ex0 = get_ok(&srv, "/api/explore").await;
    assert!(ex0["indexing"]["done"].as_i64() < ex0["indexing"]["total"].as_i64());
    for _ in 0..100 {
        if !srv.ctx.scanner.busy() {
            break;
        }
        sleep(50).await;
    }
    srv.ctx.events.close();
    let mark = events.lock().unwrap().len();
    pa_server::scanner::kick_enrich(&srv.ctx);
    for _ in 0..400 {
        if !srv.ctx.scanner.enriching()
            && count(
                &srv,
                &format!(
                    "SELECT count(*) c FROM media WHERE meta_v < {}",
                    pa_meta::META_V
                ),
            ) == 0
        {
            break;
        }
        sleep(50).await;
    }
    assert_eq!(snap(&srv), snapshot);
    let ex1 = get_ok(&srv, "/api/explore").await;
    assert_eq!(
        ex1["indexing"],
        json!({"done": ex0["indexing"]["total"], "total": ex0["indexing"]["total"]})
    );
    assert!(!ex1["places"].as_array().unwrap().is_empty());
    assert_eq!(
        events.lock().unwrap()[mark..]
            .iter()
            .filter(|e| matches!(e, pa_core::ServerEvent::Media { .. }))
            .count(),
        0
    );
    for _ in 0..150 {
        if events.lock().unwrap()[mark..]
            .iter()
            .any(|e| matches!(e, pa_core::ServerEvent::Explore))
        {
            break;
        }
        sleep(100).await;
    }
    assert_eq!(
        events.lock().unwrap()[mark..]
            .iter()
            .filter(|e| matches!(e, pa_core::ServerEvent::Explore))
            .count(),
        1
    );

    // re-derives local times when the home zone changes
    let v0 = by_name(&srv, "yilbasi.mp4").await;
    let s0 = by_name(&srv, "selfie.jpg").await;
    let r = send_ok(
        &srv,
        "PATCH",
        "/api/settings",
        json!({"homeTimeZone": "America/New_York"}),
    )
    .await;
    assert_eq!(r["homeTimeZone"], "America/New_York");
    let v = by_name(&srv, "yilbasi.mp4").await;
    assert_eq!(v["takenAt"], v0["takenAt"]);
    assert_eq!(
        v["localAt"],
        pa_core::instant_to_local(v0["takenAt"].as_i64().unwrap(), "America/New_York")
    );
    assert_eq!(
        v["localAt"].as_i64().unwrap(),
        v0["localAt"].as_i64().unwrap() - 8 * 60 * MIN
    );
    let s1 = by_name(&srv, "selfie.jpg").await;
    assert_eq!(
        (s1["localAt"].clone(), s1["takenAt"].clone()),
        (s0["localAt"].clone(), s0["takenAt"].clone())
    );
    assert_eq!(
        send(
            &srv,
            "PATCH",
            "/api/settings",
            json!({"homeTimeZone": "Mars/Olympus"})
        )
        .await
        .status,
        400
    );
    srv.close().await;
}
