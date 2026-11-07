//! Mirrors explore-insights.integration.test.ts (XMP / Takeout metadata, explore insights,
//! insight browse filters, recently added, calendar, duplicates, metadata diagnostics + re-read)

use crate::common::*;
use serde_json::{json, Value};

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

#[derive(Default)]
struct J {
    w: Option<u32>,
    h: Option<u32>,
    color: Option<&'static str>,
    local: Option<i64>,
    off: Option<&'static str>,
    gps: Option<(f64, f64)>,
    xmp: Option<String>,
    seed: Option<u64>,
}

fn jpg(file: &std::path::Path, o: J) {
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let (w, h) = (o.w.unwrap_or(120), o.h.unwrap_or(90));
    let ex = Exif {
        make: Some("Apple".into()),
        model: Some("iPhone 13".into()),
        date: o.local.map(exif_date),
        offset: o.off.map(String::from),
        gps: o.gps,
        xmp: o.xmp,
        ..Default::default()
    };
    let color = o.color.unwrap_or("#33bb77");
    let b = match o.seed {
        Some(seed) => seeded_jpeg_bytes(w, h, color, seed, &ex),
        None => jpeg_bytes(w, h, color, &ex, 80),
    };
    std::fs::write(file, b).unwrap();
}

fn xmp_gps(lat: &str, lon: &str) -> String {
    format!(
        r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:exif="http://ns.adobe.com/exif/1.0/" exif:GPSLatitude="{lat}" exif:GPSLongitude="{lon}"/></rdf:RDF></x:xmpmeta><?xpacket end="w"?>"#
    )
}

fn side(dir: &std::path::Path, json_name: &str, lat: f64, lon: f64, ts: i64) {
    std::fs::write(dir.join(json_name), json!({"title": "x", "photoTakenTime": {"timestamp": ts.to_string()}, "geoData": {"latitude": lat, "longitude": lon, "altitude": 0}}).to_string()).unwrap();
}

async fn settle(srv: &pa_server::Server, pred: impl Fn() -> bool) {
    for _ in 0..600 {
        let c = count(srv, "SELECT count(*) c FROM media WHERE meta_state = 0 OR (thumb_state = 0 AND kind IN ('image','video'))");
        if c == 0
            && !srv.ctx.scanner.busy()
            && !srv.ctx.scanner.enriching()
            && !srv.ctx.explore.dups_busy()
            && pred()
        {
            return;
        }
        sleep(100).await;
    }
    panic!("library did not settle");
}

async fn all(srv: &pa_server::Server) -> Vec<Value> {
    items(&get_ok(srv, "/api/browse?recursive=1&folderId=1&limit=5000").await)
}
async fn by_name(srv: &pa_server::Server, n: &str) -> Value {
    all(srv)
        .await
        .into_iter()
        .find(|i| i["name"] == n)
        .unwrap_or_else(|| panic!("{n}"))
}
async fn total(srv: &pa_server::Server, qs: &str) -> i64 {
    get_ok(srv, &format!("/api/browse?{qs}&limit=1")).await["total"]
        .as_i64()
        .unwrap()
}
fn close(a: &Value, b: f64, digits: i32) -> bool {
    (a.as_f64().unwrap() - b).abs() < 0.5 * 10f64.powi(-digits)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn explore_insights_metadata_duplicates_diagnostics() {
    let tmp = Tmp::new("insights");
    let lib = tmp.join("lib");
    for k in 0..14u64 {
        jpg(
            &lib.join("2023").join(format!("p{k}.jpg")),
            J {
                seed: Some(k),
                w: Some(if k % 3 == 0 { 90 } else { 120 }),
                h: Some(if k % 3 == 0 { 120 } else { 90 }),
                local: Some(l(&format!(
                    "2023-{:02}-10T{}:15:00",
                    1 + (k % 12),
                    if k % 2 == 1 { "12" } else { "20" }
                ))),
                off: Some("+03:00"),
                gps: if k % 4 == 3 {
                    None
                } else {
                    Some((41.0082 + k as f64 * 0.001, 28.9784))
                },
                color: Some(if k % 2 == 1 { "#2244dd" } else { "#dd2222" }),
                ..Default::default()
            },
        );
    }
    jpg(
        &lib.join("2024/sunset.jpg"),
        J {
            seed: Some(101),
            local: Some(l("2024-06-21T20:30:00")),
            off: Some("+03:00"),
            gps: Some((41.0082, 28.9784)),
            color: Some("#ee8822"),
            ..Default::default()
        },
    );
    jpg(
        &lib.join("2024/square.jpg"),
        J {
            seed: Some(102),
            w: Some(100),
            h: Some(100),
            local: Some(l("2024-06-22T13:00:00")),
            off: Some("+03:00"),
            gps: Some((43.7696, 11.2558)),
            ..Default::default()
        },
    );
    jpg(
        &lib.join("2024/xmp-only.jpg"),
        J {
            seed: Some(103),
            local: Some(l("2024-05-01T10:00:00")),
            off: Some("+03:00"),
            xmp: Some(xmp_gps("41,0.738N", "28,58.476E")),
            ..Default::default()
        },
    );
    jpeg_dated(&lib.join("IMG-20240101-WA0001.jpg"), 80, 100, "#888", None);
    let tk = lib.join("Takeout/Google Photos/Photos from 2019");
    jpeg_dated(&tk.join("IMG_20190704_101010.jpg"), 90, 70, "#191", None);
    side(
        &tk,
        "IMG_20190704_101010.jpg.json",
        37.9715,
        23.7267,
        1562235010,
    );
    jpeg_dated(
        &tk.join("IMG_20190705_121212345_HDR.jpg"),
        90,
        70,
        "#119",
        None,
    );
    side(
        &tk,
        "IMG_20190705_121212345_HDR.jpg.supplemental-me.json",
        37.4467,
        25.3289,
        1562328732,
    );
    jpeg_dated(&tk.join("IMG_1234(1).jpg"), 91, 70, "#911", None);
    side(
        &tk,
        "IMG_1234.jpg.supplemental-metadata(1).json",
        36.3932,
        25.4615,
        1562415132,
    );
    jpeg_dated(&tk.join("IMG_1234-edited.jpg"), 92, 70, "#991", None);
    side(&tk, "IMG_1234.jpg.json", 36.4, 25.47, 1562415100);
    std::fs::create_dir_all(lib.join("Yedek")).unwrap();
    std::fs::copy(lib.join("2023/p1.jpg"), lib.join("Yedek/p1.jpg")).unwrap();
    std::fs::copy(lib.join("2023/p1.jpg"), lib.join("Yedek/p1 kopya.jpg")).unwrap();
    std::fs::copy(lib.join("2023/p2.jpg"), lib.join("Yedek/p2.jpg")).unwrap();
    let mut twin = std::fs::read(lib.join("2023/p4.jpg")).unwrap();
    let n = twin.len();
    twin[n - 3] ^= 0xff;
    std::fs::write(lib.join("Yedek/p4-twin.jpg"), twin).unwrap();

    let srv = start(&tmp, &lib).await;
    send_ok(
        &srv,
        "PATCH",
        "/api/settings",
        json!({"homeTimeZone": "Europe/Istanbul"}),
    )
    .await;
    settle(&srv, || true).await;
    get_ok(&srv, "/api/explore").await;
    let ctx = srv.ctx.clone();
    settle(&srv, || pa_server::explore::pending(&ctx) == 0).await;
    srv.ctx.explore.invalidate(&srv.ctx, false);

    // XMP GPS strings and Google Takeout sidecars
    let x = get_ok(
        &srv,
        &format!(
            "/api/media/{}",
            s(&by_name(&srv, "xmp-only.jpg").await["id"])
        ),
    )
    .await;
    assert!(
        close(&x["gps"]["lat"], 41.0123, 3) && close(&x["gps"]["lon"], 28.9746, 3),
        "{}",
        x["gps"]
    );
    assert_eq!(
        (x["place"]["cc"].as_str(), x["place"]["admin1"].as_str()),
        (Some("TR"), Some("İstanbul"))
    );
    let t1 = by_name(&srv, "IMG_20190704_101010.jpg").await;
    assert_eq!(t1["takenAt"], 1562235010000i64);
    assert_eq!(
        get_ok(&srv, &format!("/api/media/{}", s(&t1["id"]))).await["gps"],
        json!({"lat": 37.9715, "lon": 23.7267})
    );
    let t2 = get_ok(
        &srv,
        &format!(
            "/api/media/{}",
            s(&by_name(&srv, "IMG_20190705_121212345_HDR.jpg").await["id"])
        ),
    )
    .await;
    assert_eq!(t2["gps"], json!({"lat": 37.4467, "lon": 25.3289}));
    assert_eq!(t2["takenAt"], 1562328732000i64);
    let t3 = get_ok(
        &srv,
        &format!(
            "/api/media/{}",
            s(&by_name(&srv, "IMG_1234(1).jpg").await["id"])
        ),
    )
    .await;
    assert_eq!(t3["gps"], json!({"lat": 36.3932, "lon": 25.4615}));
    let t4 = get_ok(
        &srv,
        &format!(
            "/api/media/{}",
            s(&by_name(&srv, "IMG_1234-edited.jpg").await["id"])
        ),
    )
    .await;
    assert_eq!(t4["gps"], json!({"lat": 36.4, "lon": 25.47}));
    assert_eq!(t4["place"]["cc"], "GR");
    assert_eq!(
        count(
            &srv,
            &format!(
                "SELECT count(*) FROM media WHERE meta_v < {}",
                pa_meta::META_V
            )
        ),
        0
    );

    // explore: stats, year in review, places, time of day, colours, shapes, rankings
    srv.ctx.explore.invalidate(&srv.ctx, false);
    let r = get_ok(&srv, "/api/explore").await;
    let everything = all(&srv).await;
    let n = everything
        .iter()
        .filter(|i| i["kind"] == "image" || i["kind"] == "video")
        .count() as i64;
    assert_eq!(
        r["stats"]["photos"].as_i64().unwrap() + r["stats"]["videos"].as_i64().unwrap(),
        n
    );
    assert_eq!(
        r["stats"]["bytes"].as_i64().unwrap(),
        everything
            .iter()
            .map(|i| i["size"].as_i64().unwrap())
            .sum::<i64>()
    );
    assert_eq!(
        (
            r["stats"]["countries"].as_i64(),
            r["stats"]["firstYear"].as_i64()
        ),
        (Some(3), Some(2019))
    );
    let ccs: Vec<String> = r["countries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| s(&c["cc"]))
        .collect();
    for c in ["TR", "IT", "GR"] {
        assert!(ccs.contains(&c.to_string()));
    }
    assert!(r["cities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["cc"] == "TR" && c["placeKey"] == "TR-34"));
    let y23 = r["yearReviews"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == "year-2023")
        .unwrap()
        .clone();
    assert_eq!(
        (y23["kind"].as_str(), y23["count"].as_i64()),
        (Some("year"), Some(18))
    );
    assert_eq!(
        y23["months"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| !m.is_null())
            .count(),
        12
    );
    assert!(r["calendarYears"][0].as_i64().unwrap() >= 2024);
    let g = |arr: &str, v: &str| {
        r[arr]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["value"] == v)
            .cloned()
            .unwrap_or(Value::Null)
    };
    assert!(g("dayTimes", "night")["count"].as_i64().unwrap() > 0);
    let sunset = s(&by_name(&srv, "sunset.jpg").await["id"]);
    assert!(g("dayTimes", "golden")["coverIds"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| s(i) == sunset));
    let colors: Vec<String> = r["colors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| s(&c["value"]))
        .collect();
    assert!(colors.contains(&"red".into()) && colors.contains(&"blue".into()));
    let square = s(&by_name(&srv, "square.jpg").await["id"]);
    assert!(g("orientations", "square")["coverIds"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| s(i) == square));
    assert!(g("orientations", "portrait")["count"].as_i64().unwrap() >= 5);
    assert!(r["largest"][0]["value"].as_f64() >= r["largest"][1]["value"].as_f64());
    let no_loc = count(
        &srv,
        "SELECT count(*) FROM media WHERE lat IS NULL AND kind IN ('image','video')",
    );
    assert_eq!(r["noLocation"], no_loc);
    assert!(no_loc >= 2);
    assert_eq!(
        (
            r["duplicates"]["groups"].as_i64(),
            r["duplicates"]["files"].as_i64(),
            r["duplicates"]["pending"].as_i64()
        ),
        (Some(2), Some(5), Some(0))
    );
    let sz = |p: &str| std::fs::metadata(lib.join(p)).unwrap().len() as i64;
    assert_eq!(
        r["duplicates"]["wasted"],
        2 * sz("2023/p1.jpg") + sz("2023/p2.jpg")
    );

    // browse filters behind the result pages
    assert_eq!(total(&srv, "gps=none").await, no_loc);
    assert_eq!(
        total(&srv, "gps=any").await,
        r["stats"]["withLocation"].as_i64().unwrap()
    );
    assert_eq!(
        total(&srv, "color=red").await,
        g("colors", "red")["count"].as_i64().unwrap()
    );
    assert_eq!(total(&srv, "color=nope").await, total(&srv, "x=1").await);
    for o in r["orientations"].as_array().unwrap() {
        assert_eq!(
            total(&srv, &format!("orient={}", s(&o["value"]))).await,
            o["count"].as_i64().unwrap(),
            "{o}"
        );
    }
    for o in r["dayTimes"].as_array().unwrap() {
        assert_eq!(
            total(&srv, &format!("daytime={}", s(&o["value"]))).await,
            o["count"].as_i64().unwrap(),
            "{o}"
        );
    }
    assert_eq!(total(&srv, "dup=1").await, 5);
    let sizes: Vec<i64> = items(&get_ok(&srv, "/api/browse?dup=1&limit=10").await)
        .iter()
        .map(|i| i["size"].as_i64().unwrap())
        .collect();
    let mut sorted = sizes.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(sizes, sorted);
    let c = r["cities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["placeKey"] == "TR-34")
        .unwrap();
    let cname = percent_encoding::utf8_percent_encode(
        c["name"].as_str().unwrap(),
        percent_encoding::NON_ALPHANUMERIC,
    )
    .to_string();
    assert_eq!(
        total(&srv, &format!("place=TR-34&city={cname}")).await,
        c["count"].as_i64().unwrap()
    );
    assert_eq!(total(&srv, "vlen=short").await, 0);
    assert_eq!(
        get_ok(&srv, "/api/browse/skeleton?gps=none").await["total"],
        no_loc
    );
    use base64::Engine;
    for qs in [
        "gps=none",
        "gps=any",
        "color=red",
        "orient=portrait",
        "orient=landscape",
        "daytime=golden",
        "daytime=night",
        "vlen=short",
        "dup=1",
        "city=x",
        "byAdded=1",
    ] {
        let s2 = get_ok(&srv, &format!("/api/browse/skeleton?{qs}")).await;
        let p2 = get_ok(&srv, &format!("/api/browse?{qs}&limit=5000")).await;
        assert_eq!(s2["total"], p2["total"], "{qs}");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(s2["aspects"].as_str().unwrap())
                .unwrap()
                .len(),
            items(&p2).len(),
            "{qs}"
        );
        if qs == "dup=1" || qs == "byAdded=1" {
            assert_eq!(s2["days"], Value::Null, "{qs}");
        } else {
            let sum: i64 = s2["days"]
                .as_array()
                .map(|d| d.iter().map(|x| x["count"].as_i64().unwrap()).sum())
                .unwrap_or(0);
            assert_eq!(sum, p2["total"].as_i64().unwrap(), "{qs}");
        }
    }

    // recently added: newest additions first
    jpg(
        &lib.join("new/fresh.jpg"),
        J {
            seed: Some(999),
            local: Some(l("2010-01-01T10:00:00")),
            ..Default::default()
        },
    );
    pa_server::scanner::scan_folder(&srv.ctx, 1, "").await;
    settle(&srv, || true).await;
    let page = get_ok(&srv, "/api/browse?byAdded=1&recursive=1&sort=mtime&limit=3").await;
    assert_eq!(items(&page)[0]["name"], "fresh.jpg");
    srv.ctx.explore.invalidate(&srv.ctx, false);
    assert_eq!(
        get_ok(&srv, "/api/explore").await["recent"][0]["id"],
        items(&page)[0]["id"]
    );

    // calendar heatmap counts per day
    let cal = get_ok(&srv, "/api/explore/calendar?year=2023").await;
    assert_eq!(cal["days"].as_array().unwrap().len(), 365);
    assert_eq!(cal["total"], 18);
    let d = ((l("2023-02-10T00:00:00") - l("2023-01-01T00:00:00")) / 86_400_000) as usize;
    assert_eq!(cal["days"][d], 4);
    assert_eq!(cal["max"], 4);
    assert_eq!(
        get(&srv, "/api/explore/calendar?year=abc").await.status,
        400
    );

    // duplicate groups with their items and folders
    let dd = get_ok(&srv, "/api/explore/duplicates").await;
    assert_eq!(dd["total"], 2);
    assert_eq!(dd["groups"][0]["ids"].as_array().unwrap().len(), 3);
    assert_eq!(dd["items"].as_array().unwrap().len(), 5);
    let mut gnames: Vec<String> = dd["groups"][0]["ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| {
            s(&dd["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|i| i["id"] == *id)
                .unwrap()["name"])
        })
        .collect();
    gnames.sort();
    assert_eq!(gnames, vec!["p1 kopya.jpg", "p1.jpg", "p1.jpg"]);
    let paths: Vec<String> = dd["paths"].as_object().unwrap().values().map(s).collect();
    assert!(
        paths.contains(&"lib/2023".into()) && paths.contains(&"lib/Yedek".into()),
        "{paths:?}"
    );
    {
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(lib.join("Yedek/p2.jpg"))
            .unwrap()
            .write_all(&[0])
            .unwrap();
    }
    pa_server::scanner::upsert_file(&srv.ctx, 1, "Yedek/p2.jpg").await;
    settle(&srv, || true).await;
    assert_eq!(
        srv.ctx
            .db
            .get(
                "SELECT chash FROM media WHERE name = 'p2.jpg' AND dir = 'Yedek'",
                vec![],
                |r| r.get::<_, Option<String>>(0)
            )
            .unwrap()
            .unwrap(),
        None
    );
    assert_eq!(get_ok(&srv, "/api/explore/duplicates").await["total"], 1);

    // metadata diagnostics + re-read
    let st = get_ok(&srv, "/api/meta/status").await;
    let l0 = &st["libraries"][0];
    assert_eq!(l0["name"], "Lib");
    let n = |k: &str| l0[k].as_i64().unwrap();
    assert_eq!(
        n("withLocation") + n("withoutLocation") + n("pending") + n("unreadable"),
        n("total")
    );
    assert_eq!((n("messaging"), n("pending")), (1, 0));
    let wa = lib.join("IMG-20240101-WA0001.jpg");
    std::fs::write(format!("{}.json", wa.display()), json!({"photoTakenTime": {"timestamp": "1704100000"}, "geoData": {"latitude": 39.9334, "longitude": 32.8597}}).to_string()).unwrap();
    let wa_id = s(&by_name(&srv, "IMG-20240101-WA0001.jpg").await["id"]);
    assert_eq!(
        get_ok(&srv, &format!("/api/media/{wa_id}")).await["gps"],
        Value::Null
    );
    let rr = send(&srv, "POST", "/api/meta/reread", json!({"libraryId": "1"})).await;
    assert_eq!(rr.status, 200);
    assert!(rr.json()["queued"].as_i64().unwrap() > 10);
    let ctx = srv.ctx.clone();
    settle(&srv, move || {
        ctx.db
            .int(
                &format!(
                    "SELECT count(*) FROM media WHERE meta_v < {}",
                    pa_meta::META_V
                ),
                vec![],
            )
            .ok()
            .flatten()
            == Some(0)
    })
    .await;
    let after = get_ok(&srv, &format!("/api/media/{wa_id}")).await;
    assert_eq!(after["gps"], json!({"lat": 39.9334, "lon": 32.8597}));
    assert_eq!(after["place"]["cc"], "TR");
    assert_eq!(
        get_ok(&srv, "/api/meta/status").await["libraries"][0]["withLocation"],
        n("withLocation") + 1
    );
    assert_eq!(
        send(
            &srv,
            "POST",
            "/api/meta/reread",
            json!({"libraryId": "999"})
        )
        .await
        .status,
        404
    );
    srv.close().await;
}
