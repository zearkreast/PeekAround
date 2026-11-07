//! Mirrors packages/server/test/jobs.integration.test.ts (job parts): real ffmpeg on tiny lavfi
//! clips / generated images, an in-memory library (MediaLookup), the real JobService + Exporter.
//! Skipped when ffmpeg / ffprobe / libx264 are missing.

use async_trait::async_trait;
use pa_core::{FileLocation, Job, JobStatus, MediaKind};
use pa_jobs::{Exporter, HttpError, JobMedia, JobService, MediaLookup, MemoryEvents};
use pa_media::ffmpeg::{Ffmpeg, ProbeOptions, ProbeResult};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::OnceCell;

struct Lib {
    root: PathBuf,
    rows: Mutex<HashMap<String, JobMedia>>,
    upserts: Mutex<Vec<(i64, String)>>,
}

#[async_trait]
impl MediaLookup for Lib {
    fn media(&self, id: &str) -> Result<JobMedia, HttpError> {
        self.rows
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| HttpError::new(404, "Media not found"))
    }
    fn target_dir(&self, folder_id: &str, dir: &str) -> Result<(i64, PathBuf), HttpError> {
        if folder_id != "1" {
            return Err(HttpError::new(404, "Folder not found"));
        }
        Ok((1, pa_jobs::paths::resolve_in_root(&self.root, dir)?))
    }
    fn edit_suffix(&self) -> String {
        "_edit".into()
    }
    async fn upsert_file(&self, folder_id: i64, rel: &str) {
        self.upserts
            .lock()
            .unwrap()
            .push((folder_id, rel.to_string()));
    }
}

struct Env {
    lib: Arc<Lib>,
    ex: Exporter,
    ff: Arc<Ffmpeg>,
    temp: PathBuf,
    ids: HashMap<&'static str, String>,
}

static ENV: OnceCell<Option<Env>> = OnceCell::const_new();
/// the media lane is serial anyway; tests touching shared file names run one at a time
static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn ff(ff: &Ffmpeg, args: &[&str]) -> bool {
    let mut a = vec!["-y"];
    a.extend_from_slice(args);
    ff.run(
        &a,
        pa_media::ffmpeg::RunOptions {
            timeout: Some(Duration::from_secs(60)),
            input: None,
        },
    )
    .await
    .is_ok()
}

/// a JPEG with an EXIF APP1 segment carrying `orientation`
fn jpeg_with_orientation(img: &image::RgbImage, orientation: u16) -> Vec<u8> {
    let mut j = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut j, 90)
        .encode_image(img)
        .unwrap();
    let mut tiff = vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 1, 0];
    tiff.extend_from_slice(&0x0112u16.to_le_bytes());
    tiff.extend_from_slice(&3u16.to_le_bytes());
    tiff.extend_from_slice(&1u32.to_le_bytes());
    tiff.extend_from_slice(&(orientation as u32).to_le_bytes());
    tiff.extend_from_slice(&0u32.to_le_bytes());
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend(tiff);
    let mut out = vec![0xff, 0xd8, 0xff, 0xe1];
    out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
    out.extend(app1);
    out.extend_from_slice(&j[2..]);
    out
}

fn photo(color: [u8; 3]) -> image::RgbImage {
    image::RgbImage::from_fn(800, 600, |x, y| {
        let (dx, dy) = (x as f64 - 400.0, y as f64 - 300.0);
        if dx * dx + dy * dy < 200.0 * 200.0 {
            image::Rgb([255, 255, 255])
        } else {
            image::Rgb(color)
        }
    })
}

async fn env() -> Option<&'static Env> {
    ENV.get_or_init(|| async {
        let f = Ffmpeg::new("ffmpeg", Some("ffprobe".into()));
        f.check().await;
        if !f.available() || !f.probe_available() || !f.encoders().await.contains("libx264") {
            eprintln!("ffmpeg / ffprobe / libx264 missing: skipped");
            return None;
        }
        let tmp = std::env::temp_dir().join(format!("pa-jobs-it-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let lib = tmp.join("lib");
        std::fs::create_dir_all(lib.join("out")).unwrap();
        let p = |n: &str| lib.join(n).to_string_lossy().into_owned();
        assert!(
            ff(
                &f,
                &[
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=s=640x480:r=25:d=5",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=f=440:d=5",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast",
                    "-pix_fmt",
                    "yuv420p",
                    "-c:a",
                    "aac",
                    "-shortest",
                    &p("clip.mp4")
                ]
            )
            .await
        );
        assert!(
            ff(
                &f,
                &[
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc=s=320x240:r=25:d=3",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast",
                    "-pix_fmt",
                    "yuv420p",
                    &p("silent.mp4")
                ]
            )
            .await
        );
        let rotated = ff(
            &f,
            &[
                "-display_rotation",
                "-90",
                "-i",
                &p("silent.mp4"),
                "-c",
                "copy",
                &p("rotated.mp4"),
            ],
        )
        .await;
        assert!(
            ff(
                &f,
                &[
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=f=330:d=3",
                    "-c:a",
                    "aac",
                    &p("music.m4a")
                ]
            )
            .await
        );
        for (i, c) in [[0xe7, 0x4c, 0x3c], [0x2e, 0xcc, 0x71], [0x34, 0x98, 0xdb]]
            .iter()
            .enumerate()
        {
            photo(*c)
                .save(lib.join(format!("photo{}.jpg", i + 1)))
                .unwrap();
        }
        let yellow = image::RgbImage::from_pixel(600, 400, image::Rgb([0xf1, 0xc4, 0x0f]));
        std::fs::write(lib.join("portrait.jpg"), jpeg_with_orientation(&yellow, 6)).unwrap();

        let ff = Arc::new(f);
        let mut rows = HashMap::new();
        let mut ids = HashMap::new();
        let files: [(&'static str, MediaKind); 8] = [
            ("clip.mp4", MediaKind::Video),
            ("silent.mp4", MediaKind::Video),
            ("rotated.mp4", MediaKind::Video),
            ("music.m4a", MediaKind::Audio),
            ("photo1.jpg", MediaKind::Image),
            ("photo2.jpg", MediaKind::Image),
            ("photo3.jpg", MediaKind::Image),
            ("portrait.jpg", MediaKind::Image),
        ];
        for (i, (name, kind)) in files.into_iter().enumerate() {
            if name == "rotated.mp4" && !rotated {
                continue;
            }
            let id = (i + 1).to_string();
            rows.insert(
                id.clone(),
                JobMedia {
                    id: i as i64 + 1,
                    folder_id: 1,
                    dir: String::new(),
                    name: name.into(),
                    kind,
                    width: None,
                    height: None,
                    duration: None,
                    extra: None,
                    abs: Ok(lib.join(name)),
                },
            );
            ids.insert(name, id);
        }
        let lib_h = Arc::new(Lib {
            root: lib.clone(),
            rows: Mutex::new(rows),
            upserts: Mutex::new(Vec::new()),
        });
        let temp = tmp.join("temp");
        let jobs = JobService::new(Arc::new(MemoryEvents::default()), &temp);
        let ex = Exporter {
            jobs,
            ffmpeg: ff.clone(),
            host: lib_h.clone(),
            temp_dir: temp.clone(),
        };
        Some(Env {
            lib: lib_h,
            ex,
            ff,
            temp,
            ids,
        })
    })
    .await
    .as_ref()
}

async fn probe(e: &Env, file: &Path) -> ProbeResult {
    e.ff.probe(&file.to_string_lossy(), &ProbeOptions::default())
        .await
        .expect("probe")
}

async fn run_job(e: &Env, video: bool, spec: Value) -> (Job, PathBuf) {
    let r = if video {
        e.ex.start_video_edit(&spec).await
    } else {
        e.ex.start_slideshow(&spec).await
    };
    let job = r.unwrap_or_else(|err| panic!("{} {}", err.status, err.message));
    let job = tokio::time::timeout(Duration::from_secs(150), e.ex.jobs.wait(&job.id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.error, None);
    assert_eq!(job.status, JobStatus::Done);
    let res = job.result.clone().unwrap();
    let mut file = e.lib.root.clone();
    for s in res.dir.split('/').filter(|s| !s.is_empty()) {
        file.push(s);
    }
    file.push(&res.name);
    assert!(file.exists());
    (job, file)
}

async fn pixel(e: &Env, file: &Path, at: f64, x: u32, y: u32) -> [u8; 3] {
    let png = e.ff.frame(&file.to_string_lossy(), at, 1600).await.unwrap();
    let img = pa_media::decode_bytes(&png, Default::default())
        .unwrap()
        .to_rgb8();
    img.get_pixel(x, y).0
}

fn parts(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".part"))
        .collect()
}

fn slideshow_dirs(temp: &Path) -> Vec<String> {
    std::fs::read_dir(temp)
        .map(|r| {
            r.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("slideshow-"))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn edits_a_video_trim_crop_speed_overlay() {
    let Some(e) = env().await else { return };
    let _g = LOCK.lock().await;
    // overlay at the OUTPUT resolution (crop 320x240): red box top-left
    let ov = image::RgbaImage::from_fn(320, 240, |x, y| {
        if x < 120 && y < 60 {
            image::Rgba([255, 0, 0, 255])
        } else {
            image::Rgba([0, 0, 0, 0])
        }
    });
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(ov)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let asset = e.ex.jobs.save_asset(&png, "png").await.unwrap();
    let (job, file) = run_job(e, true, json!({
        "sourceId": e.ids["clip.mp4"], "trim": { "start": 1, "end": 3 }, "crop": { "x": 100, "y": 60, "w": 320, "h": 240 },
        "speed": 2, "volume": 0.8, "adjust": { "brightness": 0.1, "saturation": 0.2, "warmth": 0.3, "vignette": 0.3 },
        "overlays": [{ "assetId": asset, "start": 0, "end": 0.5 }], "output": { "format": "mp4", "height": null, "quality": "medium" }
    }))
    .await;
    assert_eq!(job.title, "clip.mp4");
    assert_eq!(
        job.result,
        Some(FileLocation {
            folder_id: "1".into(),
            dir: "".into(),
            name: "clip_edit.mp4".into()
        })
    );
    let p = probe(e, &file).await;
    let d = p.duration.unwrap();
    assert!(d > 0.7 && d < 1.3, "{d}");
    assert_eq!((p.width, p.height), (Some(320), Some(240)));
    assert_eq!(
        (p.codec.as_deref(), p.audio_codec.as_deref()),
        (Some("h264"), Some("aac"))
    );
    let [r1, g1, b1] = pixel(e, &file, 0.2, 30, 20).await;
    assert!(r1 > 200 && (g1 as u32 + b1 as u32) < 120, "{r1} {g1} {b1}");
    let [r2, g2, b2] = pixel(e, &file, 0.8, 30, 20).await;
    assert!(!(r2 > 200 && (g2 as u32 + b2 as u32) < 120));
    assert!(e
        .lib
        .upserts
        .lock()
        .unwrap()
        .contains(&(1, "clip_edit.mp4".into())));
    assert!(
        (probe(e, &e.lib.root.join("clip.mp4"))
            .await
            .duration
            .unwrap()
            - 5.0)
            .abs()
            < 0.5
    );
    assert!(parts(&e.lib.root).is_empty());

    // second edit gets a new name; webm without audio
    let (_, file) = run_job(e, true, json!({ "sourceId": e.ids["clip.mp4"], "trim": { "start": 0, "end": 1.5 }, "mute": true, "rotate": 90, "flipH": true,
        "output": { "format": "webm", "height": 240, "quality": "low" } }))
    .await;
    assert_eq!(file.file_name().unwrap(), "clip_edit.webm");
    let p = probe(e, &file).await;
    assert_eq!((p.width, p.height), (Some(180), Some(240)));
    assert!((p.duration.unwrap() - 1.5).abs() < 0.5);
    assert!(!p.has_audio);
    assert!(["vp9", "vp8"].contains(&p.codec.as_deref().unwrap()));
    let (_, again) = run_job(e, true, json!({ "sourceId": e.ids["clip.mp4"], "trim": { "start": 0, "end": 1 }, "output": { "format": "mp4", "height": 120, "quality": "low" } })).await;
    assert_eq!(again.file_name().unwrap(), "clip_edit-2.mp4");
}

#[tokio::test(flavor = "multi_thread")]
async fn exports_gif_and_rotated_crop() {
    let Some(e) = env().await else { return };
    let _g = LOCK.lock().await;
    let (_, file) = run_job(
        e,
        true,
        json!({ "sourceId": e.ids["silent.mp4"], "trim": { "start": 0.5, "end": 1.5 }, "speed": 0.5,
        "output": { "format": "gif", "height": 120, "quality": "high" } }),
    )
    .await;
    assert_eq!(file.file_name().unwrap(), "silent_edit.gif");
    let p = probe(e, &file).await;
    assert_eq!(p.codec.as_deref(), Some("gif"));
    assert_eq!((p.width, p.height), (Some(160), Some(120)));
    assert!((p.duration.unwrap() - 2.0).abs() < 0.3, "{:?}", p.duration);

    let Some(rid) = e.ids.get("rotated.mp4") else {
        return;
    };
    let src = probe(e, &e.lib.root.join("rotated.mp4")).await;
    if src.rotation == 0 {
        return;
    }
    assert_eq!((src.width, src.height), (Some(240), Some(320)));
    let (_, file) = run_job(e, true, json!({ "sourceId": rid, "crop": { "x": 0, "y": 100, "w": 240, "h": 200 }, "output": { "format": "mp4", "height": null, "quality": "low" } })).await;
    let p = probe(e, &file).await;
    assert_eq!((p.width, p.height, p.rotation), (Some(240), Some(200), 0));
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_bad_video_edits_before_queueing() {
    let Some(e) = env().await else { return };
    let _g = LOCK.lock().await;
    let before = e.ex.jobs.list().len();
    let o = json!({ "format": "mp4", "height": null, "quality": "high" });
    for (spec, code) in [
        (json!({ "sourceId": e.ids["photo1.jpg"], "output": o }), 400),
        (
            json!({ "sourceId": e.ids["clip.mp4"], "crop": { "x": 700, "y": 0, "w": 10, "h": 10 }, "output": o }),
            400,
        ),
        (
            json!({ "sourceId": e.ids["clip.mp4"], "trim": { "start": 9, "end": 12 }, "output": o }),
            400,
        ),
        (
            json!({ "sourceId": e.ids["clip.mp4"], "speed": 10, "output": o }),
            400,
        ),
        (
            json!({ "sourceId": e.ids["clip.mp4"], "overlays": [{ "assetId": "00000000-0000-0000-0000-000000000000.png", "start": 0, "end": 1 }], "output": o }),
            400,
        ),
        (json!({ "sourceId": "999999", "output": o }), 404),
    ] {
        assert_eq!(
            e.ex.start_video_edit(&spec).await.unwrap_err().status,
            code,
            "{spec}"
        );
    }
    let base = json!({ "duration": 2, "transition": "fade", "transitionDuration": 0.5, "kenBurns": false, "width": 640, "height": 360, "fps": 25, "background": "#000" });
    let with = |extra: Value| {
        let mut b = base.clone();
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        b
    };
    let p1 = json!([{ "id": e.ids["photo1.jpg"] }]);
    for spec in [
        with(json!({ "items": [] })),
        with(json!({ "items": [{ "id": e.ids["music.m4a"] }] })),
        with(json!({ "items": p1, "audioId": e.ids["photo2.jpg"] })),
        with(json!({ "items": p1, "transition": "spin" })),
        with(json!({ "items": p1, "target": { "folderId": "1", "dir": "missing" } })),
        with(json!({ "items": p1, "target": { "folderId": "1", "dir": "../.." } })),
    ] {
        assert_eq!(
            e.ex.start_slideshow(&spec).await.unwrap_err().status,
            400,
            "{spec}"
        );
    }
    assert_eq!(e.ex.jobs.list().len(), before);
}

#[tokio::test(flavor = "multi_thread")]
async fn slideshows() {
    let Some(e) = env().await else { return };
    let _g = LOCK.lock().await;
    let ids = &e.ids;
    let (job, file) = run_job(e, false, json!({
        "items": [{ "id": ids["photo1.jpg"] }, { "id": ids["photo2.jpg"] }, { "id": ids["photo3.jpg"] }], "duration": 2, "transition": "circle",
        "transitionDuration": 0.5, "kenBurns": true, "audioId": ids["music.m4a"], "width": 640, "height": 360, "fps": 25, "background": "#000000",
        "target": { "folderId": "1", "dir": "out" }, "name": "Tatil"
    }))
    .await;
    assert_eq!(job.title, "Tatil.mp4");
    assert_eq!(
        job.result,
        Some(FileLocation {
            folder_id: "1".into(),
            dir: "out".into(),
            name: "Tatil.mp4".into()
        })
    );
    let p = probe(e, &file).await;
    assert!((p.duration.unwrap() - 5.0).abs() < 0.3, "{:?}", p.duration);
    assert_eq!(
        (p.width, p.height, p.codec.as_deref()),
        (Some(640), Some(360), Some("h264"))
    );
    assert!(p.has_audio && p.audio_codec.as_deref() == Some("aac"));
    assert!(e
        .lib
        .upserts
        .lock()
        .unwrap()
        .contains(&(1, "out/Tatil.mp4".into())));
    assert!(parts(&e.lib.root.join("out")).is_empty());
    assert!(slideshow_dirs(&e.temp).is_empty());

    // a video item, an EXIF-rotated photo, hard cuts, default name / location
    let (job, file) = run_job(e, false, json!({
        "items": [{ "id": ids["photo1.jpg"] }, { "id": ids["portrait.jpg"], "duration": 1.5 }, { "id": ids["clip.mp4"], "trim": { "start": 1, "end": 2.5 } }],
        "duration": 1, "transition": "none", "transitionDuration": 0.5, "kenBurns": false, "width": 320, "height": 180, "fps": 20, "background": "#202020"
    }))
    .await;
    assert_eq!(
        job.result,
        Some(FileLocation {
            folder_id: "1".into(),
            dir: "".into(),
            name: "Slayt.mp4".into()
        })
    );
    let p = probe(e, &file).await;
    assert!((p.duration.unwrap() - 4.0).abs() < 0.3);
    assert_eq!(
        (p.width, p.height, p.has_audio),
        (Some(320), Some(180), false)
    );
    // portrait photo (1.0–2.5 s): pillarboxed → background at the left edge, yellow in the middle
    let edge = pixel(e, &file, 1.7, 5, 90).await;
    let mid = pixel(e, &file, 1.7, 160, 90).await;
    assert!(*edge.iter().max().unwrap() < 60, "{edge:?}");
    assert!(mid[0] > 180 && mid[1] > 150 && mid[2] < 90, "{mid:?}");

    // random transitions with a video slide
    let (_, file) = run_job(e, false, json!({
        "items": [{ "id": ids["silent.mp4"] }, { "id": ids["photo2.jpg"] }, { "id": ids["photo3.jpg"] }], "duration": 1.5, "transition": "random",
        "transitionDuration": 0.4, "kenBurns": false, "width": 320, "height": 240, "fps": 25, "background": "#fff", "name": "Karışık"
    }))
    .await;
    assert!((probe(e, &file).await.duration.unwrap() - 5.2).abs() < 0.3);
    assert_eq!(file.file_name().unwrap(), "Karışık.mp4");

    // cancel a running job: partial output removed
    let job = e
        .ex
        .start_slideshow(&json!({
            "items": [{ "id": ids["photo1.jpg"] }, { "id": ids["photo2.jpg"] }, { "id": ids["photo3.jpg"] }], "duration": 30, "transition": "fade",
            "transitionDuration": 1, "kenBurns": true, "width": 1280, "height": 720, "fps": 30, "background": "#000", "name": "Canceled"
        }))
        .await
        .unwrap();
    let t0 = Instant::now();
    loop {
        let j = e.ex.jobs.get(&job.id).unwrap();
        if j.status == JobStatus::Running && j.progress > 0.0 {
            break;
        }
        assert!(
            t0.elapsed() < Duration::from_secs(60)
                && !matches!(j.status, JobStatus::Done | JobStatus::Error),
            "{:?}",
            j.status
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    e.ex.jobs.cancel(&job.id);
    let j = e.ex.jobs.wait(&job.id).await.unwrap();
    assert_eq!((j.status, j.error), (JobStatus::Canceled, None));
    assert!(!e.lib.root.join("Canceled.mp4").exists());
    assert!(parts(&e.lib.root).is_empty());
    assert!(slideshow_dirs(&e.temp).is_empty());
}
