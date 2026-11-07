//! Mirrors skeleton.bench.test.ts: a synthetic 100k-row library (rows inserted directly), the
//! list endpoints stay fast. Time limits × `crate::slack()` (PEEK_TEST_SLACK; 4 in debug builds).

use crate::common::*;
use base64::Engine;
use std::time::Instant;

const N: i64 = 100_000;

async fn timed(srv: &pa_server::Server, url: &str) -> (u128, R) {
    get(srv, url).await; // warm statement cache
    srv.ctx.events.bump(); // new generation → no skeleton cache hit
    let t0 = Instant::now();
    let r = get(srv, url).await;
    (t0.elapsed().as_millis(), r)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn skeleton_performance_100k_rows() {
    let slow = crate::slack();
    let tmp = Tmp::new("bench");
    let lib = tmp.join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    let srv = pa_server::Server::start(opts(&tmp, &lib, "Big"))
        .await
        .unwrap();
    pa_server::scanner::scan_all(&srv.ctx).await;
    for _ in 0..400 {
        if !srv.ctx.scanner.busy() {
            break;
        }
        sleep(25).await;
    }
    let mut seed: i64 = 5;
    let mut rnd = || {
        seed = (seed * 16807) % 2147483647;
        seed as f64 / 2147483647.0
    };
    let cities: [(
        Option<&str>,
        Option<&str>,
        Option<&str>,
        Option<f64>,
        Option<f64>,
    ); 4] = [
        (
            Some("TR"),
            Some("34"),
            Some("Kadıköy"),
            Some(40.99),
            Some(29.03),
        ),
        (
            Some("TR"),
            Some("07"),
            Some("Muratpaşa"),
            Some(36.88),
            Some(30.7),
        ),
        (
            Some("DE"),
            Some("16"),
            Some("Mitte"),
            Some(52.52),
            Some(13.4),
        ),
        (None, None, None, None, None),
    ];
    let start = pa_core::js::date_utc(2015, 0, 1, 0, 0, 0, 0);
    let rows: Vec<i64> = (0..N)
        .map(|_| start + (rnd() * 10.0 * 365.0 * 86_400_000.0).floor() as i64)
        .collect();
    let t_ins = Instant::now();
    srv.ctx
        .db
        .tx(|c| {
            let mut st = c.prepare(&format!(
                "INSERT INTO media (folder_id, dir, name, ext, kind, size, mtime, width, height, taken_at, taken_local, meta_state, thumb_state, meta_v, cc, admin1, city, lat, lon, make, model, flags, seen)
                 VALUES (1, ?, ?, 'jpg', ?, 1000, ?, ?, ?, ?, ?, 1, 1, {}, ?, ?, ?, ?, ?, 'Apple', ?, ?, 1)",
                pa_meta::META_V
            ))?;
            for (i, t) in rows.iter().enumerate() {
                let i = i as i64;
                let c = cities[(i % 4) as usize];
                st.execute(rusqlite::params![
                    format!("y{}/m{}", i % 10, i % 12),
                    format!("IMG_{i}.jpg"),
                    if i % 20 != 0 { "image" } else { "video" },
                    t,
                    3000 + (i % 3) * 1000,
                    3000,
                    t,
                    t + 3 * 3_600_000,
                    c.0,
                    c.1,
                    c.2,
                    c.3,
                    c.4,
                    if i % 2 == 1 { "iPhone 13" } else { "iPhone 15" },
                    if i % 50 == 0 { 2 } else { 0 }
                ])?;
            }
            Ok(())
        })
        .unwrap();
    eprintln!("insert 100k: {} ms", t_ins.elapsed().as_millis());

    // the full skeleton
    let (ms, r) = timed(&srv, "/api/browse/skeleton").await;
    let sk = r.json();
    assert_eq!(sk["total"], N);
    assert_eq!(
        base64::engine::general_purpose::STANDARD
            .decode(sk["aspects"].as_str().unwrap())
            .unwrap()
            .len() as i64,
        N
    );
    assert!(sk["days"].as_array().unwrap().len() > 3000);
    eprintln!("skeleton 100k: {ms} ms, {} KB", r.body.len() / 1024);
    assert!(ms < 300 * slow, "skeleton {ms} ms");

    // pages, filters and explore stay fast
    let (pms, p) = timed(&srv, "/api/browse?offset=50000&limit=600").await;
    assert_eq!(items(&p.json()).len(), 600);
    assert!(pms < 150 * slow, "page {pms} ms");
    let (fms, f) = timed(&srv, "/api/browse/skeleton?place=TR-34&sort=name&order=asc").await;
    assert_eq!(f.json()["total"], N / 4);
    assert!(fms < 300 * slow, "filtered skeleton {fms} ms");
    let cached = get(&srv, "/api/browse/skeleton?place=TR-34&sort=name&order=asc").await;
    assert_eq!(cached.body, f.body);
    let (gms, _) = timed(&srv, "/api/geo/points").await;
    assert!(gms < 300 * slow, "geo {gms} ms");
    let t0 = Instant::now();
    let ex = get_ok(&srv, "/api/explore").await;
    let ex_ms = t0.elapsed().as_millis();
    eprintln!("page {pms} ms, filtered skeleton {fms} ms, geo {gms} ms, explore (cold) {ex_ms} ms");
    assert_eq!(ex["places"].as_array().unwrap().len(), 3);
    assert!(ex_ms < 2000 * slow, "explore {ex_ms} ms");
    assert_eq!(
        ex["stats"]["photos"].as_i64().unwrap() + ex["stats"]["videos"].as_i64().unwrap(),
        N
    );
    let mut ccs: Vec<String> = ex["countries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| s(&c["cc"]))
        .collect();
    ccs.sort();
    assert_eq!(ccs, vec!["DE", "TR"]);
    assert_eq!(ex["noLocation"], N / 4);
    assert!(ex["calendarYears"].as_array().unwrap().len() >= 10);

    // explore is computed off the async workers: a ticker keeps running
    srv.ctx.explore.invalidate(&srv.ctx, false);
    let worst = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let w2 = worst.clone();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let s2 = stop.clone();
    let ticker = tokio::spawn(async move {
        let mut last = Instant::now();
        while !s2.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            let n = Instant::now();
            w2.fetch_max(
                n.duration_since(last).as_millis() as u64,
                std::sync::atomic::Ordering::SeqCst,
            );
            last = n;
        }
    });
    let r = get_ok(&srv, "/api/explore").await;
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    ticker.await.unwrap();
    let worst = worst.load(std::sync::atomic::Ordering::SeqCst) as u128;
    eprintln!("explore compute: longest ticker gap {worst} ms");
    assert_eq!(
        r["stats"]["photos"].as_i64().unwrap() + r["stats"]["videos"].as_i64().unwrap(),
        N
    );
    assert!(worst < 500 * slow, "ticker blocked {worst} ms");

    // insight filters stay fast
    let mut out = Vec::new();
    for q in [
        "gps=none",
        "orient=landscape",
        "daytime=night",
        "daytime=golden",
        "color=blue",
        "dup=1",
        "byAdded=1&sort=mtime",
        "place=TR-34&city=Kad%C4%B1k%C3%B6y",
    ] {
        let (ms, r) = timed(&srv, &format!("/api/browse/skeleton?{q}")).await;
        assert_eq!(r.status, 200);
        out.push(format!("{q} {ms} ms"));
        assert!(ms < 400 * slow, "{q} {ms} ms");
    }
    let (cms, _) = timed(&srv, "/api/explore/calendar?year=2020").await;
    assert!(cms < 150 * slow, "calendar {cms} ms");
    eprintln!("{}, calendar {cms} ms", out.join(", "));
    srv.close().await;
}
