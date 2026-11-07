//! JobService semantics (jobs/index.ts): lanes, lifecycle, progress clamp + event throttling,
//! cancel of queued / running jobs, errors, close(), assets; and the transcode stream.

use pa_core::{FileLocation, JobStatus, JobType, Progress, ServerEvent};
use pa_jobs::{runner, JobFailure, JobService, MemoryEvents};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;

fn svc() -> (JobService, Arc<MemoryEvents>) {
    let ev = Arc::new(MemoryEvents::default());
    let dir = std::env::temp_dir().join(format!(
        "pa-jobs-q-{}-{}",
        std::process::id(),
        rand::random::<u32>()
    ));
    (JobService::new(ev.clone(), dir), ev)
}

fn statuses(ev: &MemoryEvents, id: &str) -> Vec<JobStatus> {
    let mut out: Vec<JobStatus> = Vec::new();
    for e in ev.snapshot() {
        if let ServerEvent::Job { job } = e {
            if job.id == id && out.last() != Some(&job.status) {
                out.push(job.status);
            }
        }
    }
    out
}

#[tokio::test]
async fn lanes_lifecycle_and_results() {
    let (jobs, ev) = svc();
    let (tx, rx) = oneshot::channel::<()>();
    let a = jobs.enqueue(
        JobType::VideoEdit,
        "a.mp4",
        runner(move |jc| async move {
            jc.progress(2.0);
            let _ = rx.await;
            Ok(Some(FileLocation {
                folder_id: "1".into(),
                dir: "".into(),
                name: "a_edit.mp4".into(),
            }))
        }),
    );
    assert_eq!(a.status, JobStatus::Running);
    assert_eq!(
        (a.bytes, a.files, a.error.clone(), a.result.clone()),
        (None, None, None, None)
    );
    // same lane → queued behind it
    let b = jobs.enqueue(
        JobType::Slideshow,
        "b.mp4",
        runner(|_| async { Err(JobFailure::new("x".repeat(600))) }),
    );
    assert_eq!(b.status, JobStatus::Queued);
    // the transfer lane never waits for an export
    let t = jobs.enqueue(
        JobType::Transfer,
        "copy",
        runner(|jc| async move {
            jc.transfer(
                Some(Progress {
                    done: 50,
                    total: 200,
                }),
                Some(Progress { done: 0, total: 2 }),
            );
            assert!((jc.job().progress - 0.25).abs() < 1e-9);
            jc.transfer(
                Some(Progress { done: 0, total: 0 }),
                Some(Progress { done: 1, total: 2 }),
            );
            assert!((jc.job().progress - 0.5).abs() < 1e-9);
            Ok(None)
        }),
    );
    assert_eq!(
        (t.bytes, t.files),
        (Some(Progress::default()), Some(Progress::default()))
    );
    let t = jobs.wait(&t.id).await.unwrap();
    assert_eq!((t.status, t.progress), (JobStatus::Done, 1.0));
    assert_eq!(jobs.get(&b.id).unwrap().status, JobStatus::Queued);
    assert_eq!(jobs.get(&a.id).unwrap().progress, 1.0); // clamped
    tx.send(()).unwrap();
    let a = jobs.wait(&a.id).await.unwrap();
    assert_eq!(a.result.unwrap().name, "a_edit.mp4");
    let b = jobs.wait(&b.id).await.unwrap();
    assert_eq!(b.status, JobStatus::Error);
    assert_eq!(b.error.unwrap().len(), 500);
    assert_eq!(
        statuses(&ev, &a.id),
        [JobStatus::Queued, JobStatus::Running, JobStatus::Done]
    );
    assert_eq!(
        statuses(&ev, &b.id),
        [JobStatus::Queued, JobStatus::Running, JobStatus::Error]
    );
    // newest first; clear removes finished ones
    assert_eq!(jobs.list().len(), 3);
    assert!(jobs
        .list()
        .windows(2)
        .all(|w| w[0].created_at >= w[1].created_at));
    jobs.clear_finished();
    assert!(jobs.list().is_empty());
}

#[tokio::test]
async fn cancel_queued_and_running() {
    let (jobs, ev) = svc();
    let run = jobs.enqueue(
        JobType::VideoEdit,
        "long",
        runner(|jc| async move {
            jc.cancel.cancelled().await;
            Err("Canceled".into())
        }),
    );
    let queued = jobs.enqueue(JobType::VideoEdit, "next", runner(|_| async { Ok(None) }));
    jobs.cancel(&queued.id);
    assert_eq!(jobs.get(&queued.id).unwrap().status, JobStatus::Canceled);
    jobs.cancel("unknown");
    jobs.cancel(&run.id);
    let r = jobs.wait(&run.id).await.unwrap();
    assert_eq!((r.status, r.error), (JobStatus::Canceled, None));
    assert_eq!(
        statuses(&ev, &queued.id),
        [JobStatus::Queued, JobStatus::Canceled]
    );
    // a canceled queued job never runs
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(jobs.get(&queued.id).unwrap().status, JobStatus::Canceled);
}

#[tokio::test]
async fn progress_events_are_throttled() {
    let (jobs, ev) = svc();
    let j = jobs.enqueue(
        JobType::VideoEdit,
        "p",
        runner(|jc| async move {
            for i in 0..1000 {
                jc.progress(i as f64 / 1000.0);
            }
            Ok(None)
        }),
    );
    jobs.wait(&j.id).await;
    let n = ev
        .snapshot()
        .iter()
        .filter(|e| matches!(e, ServerEvent::Job { job } if job.id == j.id))
        .count();
    // queued + running + first tick + done
    assert!(n <= 5, "{n}");
}

#[tokio::test]
async fn close_cancels_and_waits() {
    let (jobs, _) = svc();
    let run = jobs.enqueue(
        JobType::Transfer,
        "t",
        runner(|jc| async move {
            jc.cancel.cancelled().await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            Err("canceled".into())
        }),
    );
    let q = jobs.enqueue(JobType::Transfer, "q", runner(|_| async { Ok(None) }));
    jobs.close().await;
    assert_eq!(jobs.get(&run.id).unwrap().status, JobStatus::Canceled);
    assert_eq!(jobs.get(&q.id).unwrap().status, JobStatus::Canceled);
    let late = jobs.enqueue(JobType::VideoEdit, "late", runner(|_| async { Ok(None) }));
    assert_eq!(late.status, JobStatus::Canceled);
}

#[tokio::test]
async fn assets() {
    let (jobs, _) = svc();
    let id = jobs.save_asset(b"png", "PNG").await.unwrap();
    assert!(id.ends_with(".png") && id.len() == 40, "{id}");
    assert_eq!(
        tokio::fs::read(jobs.asset_path(&id).unwrap())
            .await
            .unwrap(),
        b"png"
    );
    assert!(jobs
        .save_asset(b"x", "exe")
        .await
        .unwrap()
        .ends_with(".bin"));
    let s = futures_util::stream::iter(vec![
        Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"a")),
        Ok(bytes::Bytes::from_static(b"b")),
    ]);
    let id2 = jobs.save_asset_stream(s, "mp3").await.unwrap();
    assert_eq!(
        tokio::fs::read(jobs.asset_path(&id2).unwrap())
            .await
            .unwrap(),
        b"ab"
    );
    assert_eq!(
        jobs.asset_path("../x.png").unwrap_err().message,
        "Invalid asset id"
    );
    assert!(jobs
        .asset_path("00000000-0000-0000-0000-000000000000.toolong")
        .is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn transcode_stream() {
    use futures_util::StreamExt;
    use pa_jobs::stream::*;
    assert_eq!(hw_accel_args(true), ["-hwaccel", "auto"]);
    assert_eq!(
        internal_input("http://127.0.0.1:5", "tok", 7),
        [
            "-headers",
            "X-Peek-Internal: tok\r\n",
            "-i",
            "http://127.0.0.1:5/api/media/7/file"
        ]
    );
    let a = transcode_args(&file_input("/x.mkv"), 12.345, true);
    assert_eq!(
        &a[..8],
        [
            "-loglevel",
            "error",
            "-ss",
            "12.35",
            "-hwaccel",
            "auto",
            "-i",
            "/x.mkv"
        ]
    );
    assert_eq!(a.last().unwrap(), "pipe:1");
    assert!(a
        .join(" ")
        .contains("-movflags frag_keyframe+empty_moov+default_base_moof -f mp4"));
    assert_eq!(
        &transcode_args(&file_input("/x"), 0.0, false)[..4],
        ["-loglevel", "error", "-i", "/x"]
    );
    assert_eq!(parse_t(Some("abc")), 0.0);
    assert_eq!(parse_t(Some("-3")), 0.0);
    assert_eq!(parse_t(Some("2.5")), 2.5);

    let ff = pa_media::Ffmpeg::new("ffmpeg", None);
    ff.check().await;
    if !ff.available() || !ff.encoders().await.contains("libx264") {
        return;
    }
    let dir = std::env::temp_dir().join(format!("pa-jobs-st-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("src.mkv");
    ff.run(
        &[
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=160x120:rate=10",
            "-t",
            "2",
            "-c:v",
            "mpeg4",
            &src.to_string_lossy(),
        ],
        Default::default(),
    )
    .await
    .unwrap();
    let mut s = spawn_transcode(
        &ff,
        &transcode_args(&file_input(&src.to_string_lossy()), 0.0, false),
    )
    .unwrap();
    let mut body = Vec::new();
    while let Some(c) = s.next().await {
        body.extend_from_slice(&c.unwrap());
    }
    assert!(body.len() > 1000);
    assert_eq!(&body[4..8], b"ftyp");
    // dropping a running stream kills ffmpeg
    let mut s = spawn_transcode(
        &ff,
        &transcode_args(
            &[
                "-re".into(),
                "-f".into(),
                "lavfi".into(),
                "-i".into(),
                "testsrc=size=160x120:rate=10:d=60".into(),
            ],
            0.0,
            false,
        ),
    )
    .unwrap();
    let pid = s.pid().unwrap();
    let _ = s.next().await;
    drop(s);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let alive = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|st| !st.contains(") Z "))
        .unwrap_or(false);
    assert!(!alive, "ffmpeg {pid} still running");
    let _ = std::fs::remove_dir_all(&dir);
}
