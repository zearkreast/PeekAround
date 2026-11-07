//! Mirrors packages/server/test/ffmpeg-process.test.ts (runFfmpeg part) + frame / preview /
//! filmstrip smoke tests against the installed ffmpeg (skipped without one).

use pa_media::ffmpeg::Ffmpeg;
use pa_media::jobs::{run_ffmpeg, spawn_fed, JobError, JobRun};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

async fn ff() -> Option<Ffmpeg> {
    let ff = Ffmpeg::new("ffmpeg", Some("ffprobe".into()));
    ff.check().await;
    ff.available().then_some(ff)
}

#[tokio::test]
async fn reports_progress_against_the_expected_duration() {
    let Some(ff) = ff().await else { return };
    let mut seen = Vec::new();
    let mut cb = |p: f64| seen.push(p);
    let cancel = CancellationToken::new();
    run_ffmpeg(
        &ff,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=s=64x64:r=25:d=2",
            "-f",
            "null",
            "-",
        ],
        JobRun {
            cancel: &cancel,
            duration: Some(2.0),
            on_progress: Some(&mut cb),
        },
    )
    .await
    .unwrap();
    assert!(!seen.is_empty());
    let max = seen.iter().cloned().fold(0.0, f64::max);
    assert!(max > 0.9 && max <= 1.0, "{max}");
}

#[tokio::test]
async fn rejects_with_the_ffmpeg_error_message() {
    let Some(ff) = ff().await else { return };
    let cancel = CancellationToken::new();
    let e = run_ffmpeg(
        &ff,
        &["-i", "/definitely/not/here.mp4", "-f", "null", "-"],
        JobRun {
            cancel: &cancel,
            duration: None,
            on_progress: None,
        },
    )
    .await
    .unwrap_err();
    match e {
        JobError::Failed(m) => assert!(
            m.starts_with("ffmpeg failed: ") && m.contains("not/here"),
            "{m}"
        ),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn kills_ffmpeg_on_abort() {
    let Some(ff) = ff().await else { return };
    let cancel = CancellationToken::new();
    let t0 = Instant::now();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        c2.cancel();
    });
    let e = run_ffmpeg(
        &ff,
        &[
            "-re",
            "-f",
            "lavfi",
            "-i",
            "testsrc=s=64x64:r=25:d=60",
            "-f",
            "null",
            "-",
        ],
        JobRun {
            cancel: &cancel,
            duration: None,
            on_progress: None,
        },
    )
    .await
    .unwrap_err();
    assert_eq!(e, JobError::Aborted);
    assert!(t0.elapsed() < Duration::from_secs(10));
}

#[tokio::test]
async fn fed_encoder_and_frame_piping() {
    use tokio::io::AsyncWriteExt;
    let Some(ff) = ff().await else { return };
    let dir = std::env::temp_dir().join(format!("pa-media-fed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("o.mp4");
    let cancel = CancellationToken::new();
    let out_s = out.to_string_lossy().into_owned();
    let mut enc = spawn_fed(
        &ff,
        &[
            "-y", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", "16x16", "-r", "10", "-i", "pipe:0",
            "-c:v", "mpeg4", &out_s,
        ],
        &cancel,
    )
    .unwrap();
    // 5 frames from a source, padded to 8 with a fill frame
    let frame = 16 * 16 * 3;
    let fill = vec![128u8; frame];
    let mut bytes = 0u64;
    let mut on = |n: u64| bytes += n;
    pa_media::jobs::pipe_frames(
        &ff,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=s=16x16:r=10:d=0.5",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "pipe:1",
        ],
        &mut enc,
        8 * frame as u64,
        &fill,
        &cancel,
        &mut on,
    )
    .await
    .unwrap();
    assert_eq!(bytes, 8 * frame as u64);
    enc.stdin.as_mut().unwrap().flush().await.unwrap();
    enc.finish().await.unwrap();
    assert!(enc.exited());
    let probe = ff.probe(&out_s, &Default::default()).await.unwrap();
    assert_eq!((probe.width, probe.height), (Some(16), Some(16)));
    // a killed encoder reports through done()
    let mut enc = spawn_fed(
        &ff,
        &[
            "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", "16x16", "-i", "pipe:0", "-f", "null", "-",
        ],
        &cancel,
    )
    .unwrap();
    cancel.cancel();
    assert_eq!(enc.done().await, Err(JobError::Aborted));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn frame_preview_filmstrip_and_encoders() {
    let Some(ff) = ff().await else { return };
    let dir = std::env::temp_dir().join(format!("pa-media-vid-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("src.mp4");
    ff.run(
        &[
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=s=320x180:r=25:d=10",
            "-pix_fmt",
            "yuv420p",
            src.to_str().unwrap(),
        ],
        Default::default(),
    )
    .await
    .unwrap();
    let png = ff.frame(src.to_str().unwrap(), 1.0, 160).await.unwrap();
    let img = image::load_from_memory(&png).unwrap();
    assert_eq!((img.width(), img.height()), (160, 90));
    // past the end → retried at 0
    assert!(!ff
        .frame(src.to_str().unwrap(), 50.0, 1600)
        .await
        .unwrap()
        .is_empty());
    let base = pa_media::render::render_base_frame(&png, Default::default()).unwrap();
    assert_eq!((base.width, base.height), (160, 90));
    assert!(base.webp1600.is_some());
    let enc = ff.encoders().await;
    if enc.contains("libx264") {
        let out = dir.join("p.mp4");
        pa_media::video::make_preview(&ff, &src, Some(10.0), &out)
            .await
            .unwrap();
        let p = ff
            .probe(out.to_str().unwrap(), &Default::default())
            .await
            .unwrap();
        assert_eq!(p.height, Some(240));
        assert!(!p.has_audio);
    }
    let strip = pa_media::video::filmstrip(&ff, &src, Some(10.0), 4, 48)
        .await
        .unwrap()
        .unwrap();
    let s = image::load_from_memory(&strip).unwrap();
    assert_eq!((s.width(), s.height()), (4 * 86, 48)); // scale=-2:48 of 16:9 → 86
    let _ = std::fs::remove_dir_all(&dir);
}
