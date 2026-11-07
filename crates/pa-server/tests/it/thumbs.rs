//! Mirrors events-thumbs.test.ts (ThumbService base dedup; the EventHub / Limiter parts are unit
//! tests in src/events.rs and src/thumbs.rs) and thumbs-atomic.test.ts

use crate::common::*;
use pa_server::thumbs::{Prio, ThumbRow, ThumbService};
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn decodes_a_video_frame_once_for_every_size_and_the_background_loop() {
    let tmp = Tmp::new("th");
    let vid = tmp.join("v.mp4");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=320x240:rate=10",
        "-t",
        "2",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        vid.to_str().unwrap(),
    ]);
    let ff = Arc::new(pa_media::Ffmpeg::new("ffmpeg", Some("ffprobe".into())));
    ff.check().await;
    let th = ThumbService::new(tmp.join("thumbs"), ff);
    let row = ThumbRow {
        kind: "video".into(),
        ext: "mp4".into(),
        mtime: 1,
        size: std::fs::metadata(&vid).unwrap().len() as i64,
        duration: Some(2.0),
    };
    let abs = vid.to_string_lossy().into_owned();
    let (a, b, c, d) = tokio::join!(
        th.generate_base(&abs, &row, Prio::Low),
        th.thumb(&abs, &row, 320, Prio::High),
        th.thumb(&abs, &row, 640, Prio::High),
        th.thumb(&abs, &row, 1600, Prio::High)
    );
    assert!(a.unwrap().unwrap().thumbhash.is_some());
    for f in [b, c, d] {
        assert!(f.unwrap().unwrap().exists());
    }
    assert_eq!(th.frames.load(Ordering::SeqCst), 1);
    th.thumb(&abs, &row, 320, Prio::High).await.unwrap();
    assert_eq!(th.frames.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cache_files_appear_atomically() {
    let dir = Tmp::new("atomic");
    let mut partial = 0;
    for k in 0..4 {
        let img = dir.join(format!("i{k}.jpg"));
        noise_jpeg(&img, 2400, 1600);
        let st = std::fs::metadata(&img).unwrap();
        let ff = Arc::new(pa_media::Ffmpeg::new("/nonexistent/ffmpeg", None));
        let ts = ThumbService::new(dir.join(format!("cache{k}")), ff);
        let mtime = st
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let row = ThumbRow {
            kind: "image".into(),
            ext: "jpg".into(),
            mtime,
            size: st.len() as i64,
            duration: None,
        };
        let abs = img.to_string_lossy().into_owned();
        let key = ts.key(&abs, &row);
        let files = [ts.file(&key, "640.webp"), ts.file(&key, "320.webp")];
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (s2, f2) = (stop.clone(), files.clone());
        let poll = tokio::spawn(async move {
            let mut seen = [-1i64, -1];
            while !s2.load(Ordering::SeqCst) && seen.iter().any(|s| *s < 0) {
                for i in 0..2 {
                    if seen[i] < 0 {
                        if let Ok(m) = tokio::fs::metadata(&f2[i]).await {
                            seen[i] = m.len() as i64;
                        }
                    }
                }
                tokio::task::yield_now().await;
            }
            seen
        });
        ts.generate_base(&abs, &row, Prio::High).await.unwrap();
        stop.store(true, Ordering::SeqCst);
        let seen = poll.await.unwrap();
        for i in 0..2 {
            if seen[i] >= 0 && seen[i] != std::fs::metadata(&files[i]).unwrap().len() as i64 {
                partial += 1;
            }
        }
        let tmps: Vec<_> = std::fs::read_dir(files[0].parent().unwrap())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .collect();
        assert!(tmps.is_empty());
    }
    assert_eq!(partial, 0);
}
