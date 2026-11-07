//! Export jobs through the server (pa-jobs wiring): PUT /api/jobs/assets, POST /api/jobs/video-edit,
//! POST /api/jobs/slideshow, GET/DELETE /api/jobs — a subset of jobs.integration.test.ts (the
//! argument builders are golden-tested in pa-jobs)

use crate::common::*;
use serde_json::{json, Value};

async fn wait_job(srv: &pa_server::Server, id: &str) -> Value {
    serde_json::to_value(
        tokio::time::timeout(std::time::Duration::from_secs(120), srv.ctx.jobs.wait(id))
            .await
            .expect("job ends")
            .expect("job"),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn video_edit_and_slideshow_exports() {
    let tmp = Tmp::new("jobs");
    let lib = tmp.join("lib");
    std::fs::create_dir_all(lib.join("out")).unwrap();
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=640x360:rate=25",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=44100",
        "-t",
        "5",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-shortest",
        lib.join("clip.mp4").to_str().unwrap(),
    ]);
    for (i, c) in ["#e74c3c", "#3498db", "#2ecc71"].iter().enumerate() {
        jpeg_dated(&lib.join(format!("photo{}.jpg", i + 1)), 800, 600, c, None);
    }
    let srv = start(&tmp, &lib).await;
    let mut ids = std::collections::HashMap::new();
    for f in ["clip.mp4", "photo1.jpg", "photo2.jpg", "photo3.jpg"] {
        ids.insert(
            f,
            pa_server::scanner::upsert_file(&srv.ctx, 1, f)
                .await
                .unwrap()
                .to_string(),
        );
    }
    // overlay asset
    let r = put(
        &srv,
        "/api/jobs/assets?ext=png",
        png_bytes(320, 240, "#ff0000"),
    )
    .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let asset = s(&r.json()["assetId"]);

    let job = send_ok(
        &srv,
        "POST",
        "/api/jobs/video-edit",
        json!({
            "sourceId": ids["clip.mp4"], "trim": {"start": 1, "end": 3}, "crop": {"x": 100, "y": 60, "w": 320, "h": 240},
            "speed": 2, "volume": 0.8, "overlays": [{"assetId": asset, "start": 0, "end": 0.5}],
            "output": {"format": "mp4", "height": null, "quality": "medium"}
        }),
    )
    .await;
    assert_eq!(job["type"], "video-edit");
    assert_eq!(job["title"], "clip.mp4");
    let fin = wait_job(&srv, &s(&job["id"])).await;
    assert_eq!(fin["status"], "done", "{fin}");
    assert_eq!(
        fin["result"],
        json!({"folderId": "1", "dir": "", "name": "clip_edit.mp4"})
    );
    let out = lib.join("clip_edit.mp4");
    let p = srv
        .ctx
        .ffmpeg
        .probe(&out.to_string_lossy(), &Default::default())
        .await
        .expect("probe");
    let d = p.duration.unwrap_or(0.0);
    assert!(d > 0.7 && d < 1.3, "duration {d}");
    assert_eq!((p.width, p.height), (Some(320), Some(240)));
    assert!(
        count(
            &srv,
            "SELECT count(*) FROM media WHERE name = 'clip_edit.mp4'"
        ) == 1
    );

    // validation errors keep their TS messages
    let bad = send(
        &srv,
        "POST",
        "/api/jobs/video-edit",
        json!({"sourceId": "99999"}),
    )
    .await;
    assert_eq!(bad.status, 404);

    let job = send_ok(
        &srv,
        "POST",
        "/api/jobs/slideshow",
        json!({
            "items": [{"id": ids["photo1.jpg"]}, {"id": ids["photo2.jpg"]}, {"id": ids["photo3.jpg"]}],
            "duration": 1, "transition": "fade", "transitionDuration": 0.3, "kenBurns": true,
            "width": 320, "height": 180, "fps": 20, "background": "#000000",
            "target": {"folderId": "1", "dir": "out"}, "name": "Tatil"
        }),
    )
    .await;
    assert_eq!(job["title"], "Tatil.mp4");
    let fin = wait_job(&srv, &s(&job["id"])).await;
    assert_eq!(fin["status"], "done", "{fin}");
    assert_eq!(
        fin["result"],
        json!({"folderId": "1", "dir": "out", "name": "Tatil.mp4"})
    );
    let p = srv
        .ctx
        .ffmpeg
        .probe(
            &lib.join("out/Tatil.mp4").to_string_lossy(),
            &Default::default(),
        )
        .await
        .expect("probe");
    assert!(
        (p.duration.unwrap_or(0.0) - 2.4).abs() < 0.3,
        "{:?}",
        p.duration
    );
    assert_eq!((p.width, p.height), (Some(320), Some(180)));

    // the list, clear finished
    let jobs = get_ok(&srv, "/api/jobs").await;
    assert_eq!(jobs["jobs"].as_array().unwrap().len(), 2);
    assert_eq!(
        send_ok(&srv, "DELETE", "/api/jobs", Value::Null).await["jobs"],
        json!([])
    );
    srv.close().await;
}
