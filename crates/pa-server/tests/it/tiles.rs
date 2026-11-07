//! Mirrors tiles.integration.test.ts + settings-runtime.integration.test.ts

use crate::common::*;
use pa_core::MapTileStyle;
use serde_json::json;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Up {
    hits: usize,
    status: u16,
    delay: u64,
    bytes: usize,
    ctype: String,
    ua: String,
}

async fn fake_upstream(st: Arc<Mutex<Up>>) -> String {
    use axum::response::IntoResponse;
    let app = axum::Router::new().fallback(move |req: axum::http::Request<axum::body::Body>| {
        let st = st.clone();
        async move {
            let (status, delay, bytes, ctype) = {
                let mut s = st.lock().unwrap();
                s.hits += 1;
                s.ua = req
                    .headers()
                    .get("user-agent")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                (s.status, s.delay, s.bytes, s.ctype.clone())
            };
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            if status != 200 {
                return (
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    [("content-type", "text/plain".to_string())],
                    b"nope".to_vec(),
                )
                    .into_response();
            }
            let fill = req.uri().path().len() as u8;
            (
                axum::http::StatusCode::OK,
                [("content-type", ctype)],
                vec![fill; bytes],
            )
                .into_response()
        }
    });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    url
}

fn set_upstream(srv: &pa_server::Server, base: Option<String>) {
    *srv.ctx.tiles.upstream.lock() = Arc::new(move |style: MapTileStyle, z, x, y| match &base {
        Some(b) => format!("{b}/{}/{z}/{x}/{y}", style.as_str()),
        None => "http://127.0.0.1:1/x.png".into(),
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn map_tile_proxy() {
    let st = Arc::new(Mutex::new(Up {
        hits: 0,
        status: 200,
        delay: 0,
        bytes: 2000,
        ctype: "image/png".into(),
        ua: String::new(),
    }));
    let reset = |st: &Arc<Mutex<Up>>| {
        *st.lock().unwrap() = Up {
            hits: 0,
            status: 200,
            delay: 0,
            bytes: 2000,
            ctype: "image/png".into(),
            ua: String::new(),
        }
    };
    let up = fake_upstream(st.clone()).await;
    let tmp = Tmp::new("tiles");
    let mut o = pa_server::ServerOptions::new(tmp.join("data"));
    o.port = 0;
    o.host = "127.0.0.1".into();
    o.version = "9.9.9".into();
    o.security = Some(Default::default());
    let srv = pa_server::Server::start(o).await.unwrap();
    set_upstream(&srv, Some(up.clone()));

    // fetches once with an identifying User-Agent, then serves from the disk cache
    let a = get(&srv, "/api/tiles/osm/3/4/2.png").await;
    assert_eq!(a.status, 200);
    assert_eq!(a.h("content-type"), Some("image/png"));
    assert_eq!(a.h("x-peek-tile"), Some("miss"));
    assert_eq!(a.body.len(), 2000);
    assert_eq!(
        st.lock().unwrap().ua,
        "PeekAround/9.9.9 (+https://github.com/zearkreast/PeakAround)"
    );
    let b = get(&srv, "/api/tiles/osm/3/4/2").await;
    assert_eq!(b.status, 200);
    assert_eq!(b.h("x-peek-tile"), Some("hit"));
    assert_eq!(st.lock().unwrap().hits, 1);
    assert!(tmp.join("data/cache/tiles/osm/3/4/2.png").exists());
    assert_eq!(
        get(&srv, "/api/tiles/satellite/2/1/3.png")
            .await
            .h("content-type"),
        Some("image/jpeg")
    );

    // coalesces concurrent requests for one tile into a single upstream fetch
    reset(&st);
    st.lock().unwrap().delay = 150;
    let rs =
        futures_util::future::join_all((0..8).map(|_| get(&srv, "/api/tiles/osm/5/10/12.png")))
            .await;
    assert!(rs.iter().all(|r| r.status == 200));
    assert_eq!(st.lock().unwrap().hits, 1);

    // validates style and z/x/y ranges
    reset(&st);
    for u in [
        "/api/tiles/osm/20/0/0.png",
        "/api/tiles/osm/3/8/0.png",
        "/api/tiles/osm/3/0/8.png",
        "/api/tiles/nope/1/0/0.png",
        "/api/tiles/osm/a/0/0.png",
        "/api/tiles/topo/18/0/0.png",
        "/api/tiles/osm/1/-1/0.png",
    ] {
        assert_eq!(get(&srv, u).await.status, 400, "{u}");
    }
    assert_eq!(st.lock().unwrap().hits, 0);

    // tells "refused" from "offline", and pauses a refusing upstream
    st.lock().unwrap().status = 403;
    let r = get(&srv, "/api/tiles/topo/4/1/1.png").await;
    assert_eq!(r.status, 502);
    assert_eq!(r.h("x-peek-tile-error"), Some("refused:403"));
    assert_eq!(
        (r.json()["reason"].as_str(), r.json()["status"].as_i64()),
        (Some("refused"), Some(403))
    );
    let r2 = get(&srv, "/api/tiles/topo/4/2/1.png").await;
    assert_eq!(r2.h("x-peek-tile-error"), Some("refused:403"));
    assert_eq!(st.lock().unwrap().hits, 1);
    let ts = get_ok(&srv, "/api/tiles/status").await;
    assert_eq!(
        (ts["topo"]["reason"].as_str(), ts["topo"]["status"].as_i64()),
        (Some("refused"), Some(403))
    );
    set_upstream(&srv, None);
    let o = get(&srv, "/api/tiles/osm/6/1/1.png").await;
    assert_eq!(o.status, 504);
    assert_eq!(o.h("x-peek-tile-error"), Some("offline"));
    set_upstream(&srv, Some(up.clone()));

    // serves stale tiles when upstream fails
    reset(&st);
    assert_eq!(get(&srv, "/api/tiles/osm/7/3/3.png").await.status, 200);
    set_mtime(
        &tmp.join("data/cache/tiles/osm/7/3/3.png"),
        now_ms() - 30 * 86_400_000,
    );
    set_upstream(&srv, None);
    let s2 = get(&srv, "/api/tiles/osm/7/3/3.png").await;
    assert_eq!(s2.status, 200);
    assert_eq!(s2.h("x-peek-tile"), Some("stale"));
    assert_eq!(s2.body.len(), 2000);
    set_upstream(&srv, Some(up.clone()));
    assert_eq!(
        get(&srv, "/api/tiles/osm/7/3/3.png").await.h("x-peek-tile"),
        Some("miss")
    );

    // rejects non-image answers
    st.lock().unwrap().ctype = "text/html".into();
    assert_eq!(
        get(&srv, "/api/tiles/satellite/3/3/3.png").await.status,
        502
    );
    reset(&st);

    // reports and clears the cache; settings carry style + cap
    let c = get_ok(&srv, "/api/tiles/cache").await;
    assert!(c["files"].as_i64().unwrap() > 0 && c["bytes"].as_i64().unwrap() > 0);
    assert_eq!(c["limitMb"], 512);
    let s = send_ok(
        &srv,
        "PATCH",
        "/api/settings",
        json!({"mapTiles": "satellite", "tileCacheMb": 5}),
    )
    .await;
    assert_eq!(
        (s["mapTiles"].as_str(), s["tileCacheMb"].as_i64()),
        (Some("satellite"), Some(32))
    );
    assert_eq!(
        send_ok(
            &srv,
            "PATCH",
            "/api/settings",
            json!({"mapTiles": "google"})
        )
        .await["mapTiles"],
        "satellite"
    );
    let c = send_ok(&srv, "DELETE", "/api/tiles/cache", serde_json::Value::Null).await;
    assert_eq!(
        (c["files"].as_i64(), c["bytes"].as_i64()),
        (Some(0), Some(0))
    );
    assert_eq!(
        get(&srv, "/api/tiles/osm/3/4/2.png").await.h("x-peek-tile"),
        Some("miss")
    );

    // tiles are not throttled like file-system routes
    st.lock().unwrap().bytes = 10;
    let rs = futures_util::future::join_all((0..400).map(|i| {
        let u = format!("/api/tiles/osm/9/{i}/5.png");
        let srv = &srv;
        async move { inj(srv, "GET", &u).ip("10.1.2.3").send().await }
    }))
    .await;
    assert_eq!(rs.iter().filter(|r| r.status == 429).count(), 0);
    assert!(rs.iter().all(|r| r.status == 200));
    assert_eq!(rs[0].h("x-ratelimit-limit"), Some("20000"));
    srv.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tile_cache_lru_trims_to_90_percent() {
    let st = Arc::new(Mutex::new(Up {
        hits: 0,
        status: 200,
        delay: 0,
        bytes: 300 * 1024,
        ctype: "image/png".into(),
        ua: String::new(),
    }));
    let up = fake_upstream(st.clone()).await;
    let dir = Tmp::new("tilelru");
    let svc = pa_server::tiles::TileService::new(dir.0.clone(), "test".into(), Arc::new(|| 1));
    let u2 = up.clone();
    *svc.upstream.lock() =
        Arc::new(move |s: MapTileStyle, z, x, y| format!("{u2}/{}/{z}/{x}/{y}", s.as_str()));
    for (x, wait) in [(0, true), (1, true), (2, true), (0, true), (3, false)] {
        svc.get(MapTileStyle::Osm, 4, x, 0).await.unwrap();
        if wait {
            sleep(5).await;
        }
    }
    svc.trim().await;
    let (bytes, files, _) = svc.stats().await;
    assert!(bytes as f64 <= 0.9 * 1024.0 * 1024.0);
    assert!(!dir.join("osm/4/1/0.png").exists());
    assert!(dir.join("osm/4/0/0.png").exists());
    assert!(dir.join("osm/4/3/0.png").exists());
    assert_eq!(st.lock().unwrap().hits, 4);
    let again = pa_server::tiles::TileService::new(dir.0.clone(), "test".into(), Arc::new(|| 1));
    assert_eq!(again.stats().await.1, files);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_settings_and_cache_routes() {
    let tmp = Tmp::new("rt");
    let lib = tmp.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    let mut o = opts(&tmp, &lib, "Lib");
    o.mode = pa_core::RunMode::Desktop;
    let peek = pa_server::Server::start(o).await.unwrap();
    let d = pa_core::Settings::default();
    let s = get_ok(&peek, "/api/settings").await;
    assert_eq!(
        s["thumbQuality"],
        serde_json::to_value(d.thumb_quality).unwrap()
    );
    assert_eq!(s["indexIntensity"], "normal");
    assert_eq!(s["hwAccel"], false);

    let r = send_ok(
        &peek,
        "PATCH",
        "/api/settings",
        json!({"thumbQuality": "high", "indexIntensity": "low", "hwAccel": true}),
    )
    .await;
    assert_eq!(
        (
            r["thumbQuality"].as_str(),
            r["indexIntensity"].as_str(),
            r["hwAccel"].as_bool()
        ),
        (Some("high"), Some("low"), Some(true))
    );
    assert_eq!(peek.ctx.thumbs.quality(), pa_media::ThumbQuality::High);
    assert_eq!(peek.ctx.thumbs.q(80), 88);
    assert_eq!(peek.ctx.thumbs.limiter.low_max(), 1);
    assert_eq!(peek.ctx.scanner.concurrency(), (1, 1));
    assert_eq!(peek.ctx.thumbs.bg.stats().1, 1);
    assert_eq!(peek.ctx.ffmpeg.decode_threads(), 1);
    let r = send_ok(
        &peek,
        "PATCH",
        "/api/settings",
        json!({"thumbQuality": "ultra", "indexIntensity": 9, "hwAccel": "yes"}),
    )
    .await;
    assert_eq!(
        (
            r["thumbQuality"].as_str(),
            r["indexIntensity"].as_str(),
            r["hwAccel"].as_bool()
        ),
        (Some("high"), Some("low"), Some(true))
    );
    let r = send_ok(
        &peek,
        "PATCH",
        "/api/settings",
        json!({"thumbQuality": "low", "indexIntensity": "high", "hwAccel": false}),
    )
    .await;
    assert_eq!(peek.ctx.thumbs.q(80), 66);
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);
    assert_eq!(
        peek.ctx.scanner.concurrency().0,
        pa_server::ctx::intensity_limits(pa_core::IndexIntensity::High, cpus, true).workers
    );
    assert_eq!(r["hwAccel"], false);

    // intensity → concurrency
    use pa_core::IndexIntensity as I;
    let il = |i, n, desk| pa_server::ctx::intensity_limits(i, n, desk);
    let l = il(I::Low, 8, false);
    assert_eq!(
        (
            l.max,
            l.low_max,
            l.workers,
            l.vips_threads,
            l.ffmpeg_threads
        ),
        (4, 1, 1, 1, 1)
    );
    assert!(l.gap_ms > 0);
    let n = il(I::Normal, 8, false);
    assert_eq!(
        (n.max, n.low_max, n.workers, n.gap_ms, n.vips_threads),
        (6, 3, 3, 0, 1)
    );
    assert_eq!(
        (
            il(I::Normal, 4, false).workers,
            il(I::Normal, 4, false).low_max
        ),
        (2, 2)
    );
    assert_eq!(il(I::Normal, 2, false).workers, 1);
    assert_eq!(il(I::Normal, 4, true).workers, 1);
    assert_eq!(il(I::Normal, 8, true).workers, 2);
    assert_eq!(il(I::Normal, 32, true).workers, 2);
    let h = il(I::High, 16, false);
    assert_eq!((h.max, h.low_max, h.workers, h.vips_threads), (12, 8, 8, 0));
    assert!(il(I::High, 1, false).workers >= 1);
    for i in [I::Low, I::Normal, I::High] {
        for n in [1, 2, 4, 8, 32] {
            let l = il(i, n, false);
            assert!(l.low_max <= l.max && l.workers <= n.max(2));
        }
    }
    let on = pa_core::Settings {
        hw_accel: true,
        ..Default::default()
    };
    assert_eq!(pa_server::ctx::hw_accel_args(&on), vec!["-hwaccel", "auto"]);
    let off = pa_core::Settings {
        hw_accel: false,
        ..Default::default()
    };
    assert!(pa_server::ctx::hw_accel_args(&off).is_empty());

    // cache usage per kind, clear one kind
    let thumb_file = peek.ctx.cfg.thumb_dir.join("ab").join("abc-640.webp");
    std::fs::create_dir_all(thumb_file.parent().unwrap()).unwrap();
    std::fs::write(&thumb_file, vec![0u8; 1234]).unwrap();
    let d2 = get_ok(&peek, "/api/cache/details").await;
    assert_eq!(d2["thumbs"]["bytes"], 1234);
    assert_eq!(
        (d2["tiles"]["bytes"].as_i64(), d2["tiles"]["files"].as_i64()),
        (Some(0), Some(0))
    );
    assert!(d2["tiles"]["limitMb"].is_number());
    assert_eq!(d2["remote"]["limitMb"], d.remote_cache_mb);
    assert_eq!(
        send(&peek, "DELETE", "/api/cache/bogus", serde_json::Value::Null)
            .await
            .status,
        400
    );
    assert_eq!(
        send(
            &peek,
            "DELETE",
            "/api/cache/thumbs",
            serde_json::Value::Null
        )
        .await
        .status,
        200
    );
    assert!(!thumb_file.exists());
    for k in ["tiles", "remote"] {
        assert_eq!(
            send(
                &peek,
                "DELETE",
                &format!("/api/cache/{k}"),
                serde_json::Value::Null
            )
            .await
            .status,
            200
        );
    }

    // refuses clearing caches from the network (desktop LAN share)
    let port = std::net::TcpListener::bind("0.0.0.0:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    peek.set_lan(true, Some(port)).await.unwrap();
    let c = reqwest::Client::builder().no_proxy().build().unwrap();
    assert_eq!(
        c.delete(format!("http://127.0.0.1:{port}/api/cache/tiles"))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        c.get(format!("http://127.0.0.1:{port}/api/cache/details"))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    peek.set_lan(false, None).await.unwrap();
    peek.close().await;
}
