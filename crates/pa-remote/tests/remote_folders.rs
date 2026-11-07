//! Mirrors packages/server/test/remote-folders.test.ts (server parts; the crypto / lanes / range
//! unit tests live in pa-mega): the REAL pa-mega client against pa-mega's fake MEGA server, the
//! in-memory library of tests/common, the loopback /file endpoint for ffmpeg.

mod common;

use common::fake::FakeMega;
use common::*;
use futures_util::StreamExt;
use pa_core::{
    JobStatus, MediaKind, MediaReason, Playback, RemoteErrorCode, RemoteFolderState, ServerEvent,
};
use pa_mega::{ApiOptions, MegaClient};
use pa_remote::{routes, MegaBackend, RemoteStore};
use serde_json::json;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn client(fake: &FakeMega) -> Arc<dyn MegaBackend> {
    let mut c = MegaClient::new(ApiOptions {
        gateway: fake.url(),
        retry_unit: Duration::from_millis(5),
        timeout: Duration::from_secs(20),
        ..Default::default()
    });
    c.read_retry_unit = Duration::from_millis(5);
    Arc::new(c)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_streaming_folders() {
    let fake = FakeMega::start().await;
    let photo = fixture_photo();
    // 1 MB "photo": EXIF at the start, the rest is never needed for metadata
    let mut big = photo.clone();
    big.extend((0..1024 * 1024).map(|_| rand::random::<u8>()));
    let preview = jpeg(400, 300, [0xe1, 0x1d, 0x48]);
    let square = jpeg(120, 120, [0x25, 0x63, 0xeb]);
    let tmp0 = scratch("vid");
    let video = make_video(&tmp0);
    let has_video = video.is_some();
    let video = video.unwrap_or_else(|| (0..700_000).map(|i| (i * 7 % 251) as u8).collect());
    let url = fake.add_folder_link(
        "Holiday1",
        "Holiday",
        &[
            (
                "Hbig0001",
                "big.jpg",
                big.clone(),
                vec![(1, preview.clone()), (0, square.clone())],
            ),
            (
                "Hsq00001",
                "sq.jpg",
                photo.clone(),
                vec![(0, square.clone())],
            ),
            ("Hplain01", "plain.jpg", photo.clone(), vec![]),
            ("Hclip001", "Sub/clip.mp4", video.clone(), vec![]),
            ("Hnotes01", "Sub/notes.txt", b"x".to_vec(), vec![]),
            // duplicate names exist on MEGA
            ("Hdup0001", "dup.jpg", photo.clone(), vec![]),
            ("Hdup0002", "dup.jpg", big.clone(), vec![]),
        ],
    );
    // an empty folder below Sub
    fake.with(|i| {
        i.files.insert(
            "DEmpty00".into(),
            common::fake::FakeNode {
                h: "DEmpty00".into(),
                name: "Empty".into(),
                key: vec![9; 16],
                cipher: vec![],
                size: 0,
                fa: None,
                dir: true,
                parent: Some("DSub0000".into()),
            },
        );
        i.folders
            .get_mut("Holiday1")
            .unwrap()
            .3
            .push("DEmpty00".into());
    });
    let key = url.split('#').nth(1).unwrap().to_string();
    let stats = Arc::new(Stats::default());
    stats
        .ts
        .lock()
        .unwrap()
        .insert("plain.jpg".into(), 1_650_000_000_000);
    let env = start(
        "folders",
        Arc::new(Probe {
            inner: client(&fake),
            stats: stats.clone(),
        }),
    )
    .await;
    let (lib, svc) = (env.lib.clone(), env.svc.clone());
    let local_root = env.tmp.join("lib");
    std::fs::create_dir_all(local_root.join("Local")).unwrap();
    let local = lib.add_local("Lib", &local_root);

    // ── adds a MEGA link as a new library: rows without downloading anything
    let created = svc.create(&json!({ "url": url })).await.unwrap();
    assert_eq!(created.library.name, "Holiday");
    assert_eq!(created.folder.path, "remote://mega/Holiday1");
    let info = created.folder.remote.clone().unwrap().unwrap();
    assert_eq!(info.status.state, RemoteFolderState::Ok);
    assert_eq!(info.label, "mega.nz/folder/Holiday1");
    assert_eq!(serde_json::to_value(&info).unwrap()["linkKind"], "folder");
    assert_eq!(serde_json::to_value(&info).unwrap()["provider"], "mega");
    let fid: i64 = created.folder.id.parse().unwrap();
    let shape: Vec<(String, String, MediaKind, i64)> = lib
        .rows_of(fid)
        .iter()
        .map(|r| (r.dir.clone(), r.name.clone(), r.kind, r.size))
        .collect();
    let s = |d: &str, n: &str, k, z: usize| (d.to_string(), n.to_string(), k, z as i64);
    assert_eq!(
        shape,
        vec![
            s("", "big.jpg", MediaKind::Image, big.len()),
            s("", "dup (2).jpg", MediaKind::Image, big.len()),
            s("", "dup.jpg", MediaKind::Image, photo.len()),
            s("", "plain.jpg", MediaKind::Image, photo.len()),
            s("", "sq.jpg", MediaKind::Image, photo.len()),
            s("Sub", "clip.mp4", MediaKind::Video, video.len()),
        ]
    );
    assert_eq!(lib.row(fid, "plain.jpg").mtime, 1_650_000_000_000);
    assert_eq!(lib.row(fid, "big.jpg").mtime, 1_700_000_000_000);
    // sub folders from the index, incl. the empty one
    assert_eq!(
        lib.dirs_of(fid),
        vec!["Sub".to_string(), "Sub/Empty".to_string()]
    );

    // ── never exposes the link key to clients
    let fref = lib.folder_ref(fid).unwrap();
    let txt = serde_json::to_string(&fref).unwrap();
    assert!(!txt.contains(&key), "{txt}");
    assert!(txt.contains("mega.nz/folder/Holiday1"));
    assert!(lib
        .rows_of(fid)
        .iter()
        .all(|r| !r.dir.contains(&key) && !r.name.contains(&key)));
    assert_eq!(
        routes::folder_link(&svc, &*lib, &fid.to_string(), true, false, false, false).unwrap()
            ["url"],
        url.as_str()
    );
    let lan =
        routes::folder_link(&svc, &*lib, &fid.to_string(), true, true, false, false).unwrap_err();
    assert_eq!(
        (lan.status, lan.message.as_str()),
        (403, "The MEGA link can only be shown on this computer")
    );
    // server mode: loopback only, never through a proxy
    assert!(routes::folder_link(&svc, &*lib, &fid.to_string(), false, false, true, false).is_ok());
    assert_eq!(
        routes::folder_link(&svc, &*lib, &fid.to_string(), false, false, true, true)
            .unwrap_err()
            .status,
        403
    );
    assert_eq!(
        routes::folder_link(&svc, &*lib, &local.to_string(), true, false, false, false)
            .unwrap_err()
            .message,
        "Not a remote folder"
    );

    // ── serves byte-exact HTTP ranges (seekable video)
    let http = reqwest::Client::new();
    let clip = lib.row(fid, "clip.mp4").id;
    let furl = |id: i64| format!("{}/api/media/{id}/file", env.base);
    let full = http.get(furl(clip)).send().await.unwrap();
    assert_eq!(full.status(), 200);
    assert_eq!(full.headers()["accept-ranges"], "bytes");
    assert_eq!(full.headers()["content-type"], "video/mp4");
    assert_eq!(sha(&full.bytes().await.unwrap()), sha(&video));
    let n = video.len();
    for (a, b) in [
        (0, 0),
        (1, 15),
        (17, 100_017),
        (n - 33, n - 1),
        (12345, 12345),
    ] {
        let r = http
            .get(furl(clip))
            .header("range", format!("bytes={a}-{b}"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 206);
        assert_eq!(
            r.headers()["content-range"],
            format!("bytes {a}-{b}/{n}").as_str()
        );
        assert_eq!(
            r.headers()["content-length"],
            (b - a + 1).to_string().as_str()
        );
        assert_eq!(
            sha(&r.bytes().await.unwrap()),
            sha(&video[a..=b]),
            "{a}-{b}"
        );
    }
    let open = http
        .get(furl(clip))
        .header("range", "bytes=500000-")
        .send()
        .await
        .unwrap();
    assert_eq!(sha(&open.bytes().await.unwrap()), sha(&video[500_000..]));
    let suffix = http
        .get(furl(clip))
        .header("range", "bytes=-1000")
        .send()
        .await
        .unwrap();
    assert_eq!(sha(&suffix.bytes().await.unwrap()), sha(&video[n - 1000..]));
    let bad = http
        .get(furl(clip))
        .header("range", format!("bytes={n}-"))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 416);
    assert_eq!(
        bad.headers()["content-range"],
        format!("bytes */{n}").as_str()
    );
    let head = http.head(furl(clip)).send().await.unwrap();
    assert_eq!(head.headers()["content-length"], n.to_string().as_str());
    // a viewed image is fetched once into the LRU cache, then served locally
    let dup2 = lib.row(fid, "dup (2).jpg").id;
    assert_eq!(
        sha(&http
            .get(furl(dup2))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap()),
        sha(&big)
    );
    let dup_node = svc
        .node_of(&lib.folder_row(fid), "", "dup (2).jpg")
        .await
        .unwrap()
        .1;
    assert_eq!(dup_node.id, "Hdup0002");
    assert_eq!(
        stats.full_by.lock().unwrap().get("Hdup0002").copied(),
        Some(1)
    );
    assert_eq!(
        sha(&http
            .get(furl(dup2))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap()),
        sha(&big)
    );
    let ranged = http
        .get(furl(dup2))
        .header("range", "bytes=100-199")
        .send()
        .await
        .unwrap();
    assert_eq!(sha(&ranged.bytes().await.unwrap()), sha(&big[100..200]));
    assert_eq!(
        stats.full_by.lock().unwrap().get("Hdup0002").copied(),
        Some(1)
    );
    assert!(svc.cache.size_bytes().await >= big.len() as u64);

    // ── thumbnails: MEGA preview → 120px square → original; thumbhash like local files
    until(
        || {
            lib.rows_of(fid)
                .iter()
                .all(|r| r.thumb_state != 0)
                .then_some(())
        },
        30_000,
    )
    .await;
    let big1 = lib.row(fid, "big.jpg");
    let sq = lib.row(fid, "sq.jpg");
    let plain = lib.row(fid, "plain.jpg");
    assert_eq!(big1.thumb_state, 1);
    assert!(big1.thumbhash.is_some());
    let attrs = stats.attrs.lock().unwrap().clone();
    assert!(attrs.contains(&"big.jpg:1".to_string()), "{attrs:?}");
    assert!(
        !attrs.contains(&"big.jpg:0".to_string()),
        "preview was enough: {attrs:?}"
    );
    assert!(attrs.contains(&"sq.jpg:0".to_string()));
    assert!(sq.thumbhash.is_some());
    assert_eq!(plain.thumb_state, 1); // from the original (≤ 30 MB)
    let abs = |name: &str| format!("remote://mega/Holiday1/{name}");
    assert_eq!(lib.db.lock().unwrap().thumbs[&abs("big.jpg")], (400, 300)); // the preview, never enlarged
    let (sw, sh) = lib.db.lock().unwrap().thumbs[&abs("sq.jpg")];
    assert_eq!(sw, sh);
    assert!(svc.is_square_thumb(&abs("sq.jpg")));
    assert!(!svc.is_square_thumb(&abs("big.jpg")));
    // display rendition of a MEGA image; no filmstrips
    let disp = svc.display(&abs("plain.jpg"), 200).await.unwrap();
    let d = image::load_from_memory(&disp).unwrap();
    assert!(d.width() <= 200 && d.height() <= 200);
    assert_eq!(svc.strip().await.unwrap_err().status, 404);
    assert_eq!(
        svc.display(&abs("Sub/clip.mp4"), 200)
            .await
            .unwrap_err()
            .message,
        "Not an image"
    );
    assert_eq!(
        svc.thumb(
            "remote://mega/Holiday1/nope.jpg",
            320,
            pa_remote::Priority::High
        )
        .await
        .unwrap_err()
        .status,
        404
    );
    assert_eq!(
        svc.thumb("/etc/passwd", 320, pa_remote::Priority::High)
            .await
            .unwrap_err()
            .status,
        400
    );

    // ── metadata: EXIF/GPS from the first 256 KB only; video probe over loopback
    until(
        || {
            lib.rows_of(fid)
                .iter()
                .all(|r| r.meta_state != 3)
                .then_some(())
        },
        60_000,
    )
    .await;
    let b = lib.row(fid, "big.jpg");
    assert!((b.lat.unwrap() - 36.90705).abs() < 1e-4, "{:?}", b.lat);
    assert!((b.lon.unwrap() - 30.69155).abs() < 1e-4);
    assert!(b.make.is_some());
    assert!(b.time_src > 0); // EXIF capture time, not the MEGA timestamp
    let pm = pa_meta::image_meta(&photo, "jpg", false);
    assert_eq!((b.width, b.height), (pm.width, pm.height));
    assert_eq!(
        stats
            .full_by
            .lock()
            .unwrap()
            .get("Hbig0001")
            .copied()
            .unwrap_or(0),
        0
    );
    if has_video && ffmpeg().available() {
        let v = lib.row(fid, "clip.mp4");
        assert!(v.duration.unwrap() > 2.5, "{:?}", v.duration);
        assert_eq!((v.width, v.height), (Some(180.0), Some(320.0)));
        assert_eq!(v.playback, Playback::Direct);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(v.extra.as_deref().unwrap()).unwrap()
                ["codec"],
            "h264"
        );
        if let Some(lat) = v.lat {
            assert!((lat - 41.0082).abs() < 1e-3);
        }
        // live transcode input of MEGA videos goes through the loopback /file endpoint
        let args = svc.ffmpeg_input(v.id).unwrap();
        assert_eq!(args[0], "-headers");
        assert!(args[3].ends_with(&format!("/api/media/{}/file", v.id)));
    }
    let info = svc.info(&lib.folder_row(fid)).unwrap();
    assert_eq!((info.indexing.done, info.indexing.total), (6, 6));

    // ── copies remote items to this computer (MEGA import job, no re-listing)
    let loads = stats.loads.load(Ordering::SeqCst);
    let job = routes::copy(&svc, &*lib, &json!({ "refs": [{ "folderId": fid.to_string(), "rel": "big.jpg" }, { "folderId": fid.to_string(), "rel": "Sub" }], "to": { "folderId": local.to_string(), "dir": "Local" } }))
        .await
        .unwrap();
    let done = env.jobs.wait(&job.id).await.unwrap();
    assert_eq!(done.status, JobStatus::Done, "{:?}", done.error);
    assert_eq!(
        sha(&std::fs::read(local_root.join("Local/big.jpg")).unwrap()),
        sha(&big)
    );
    assert_eq!(
        sha(&std::fs::read(local_root.join("Local/Sub/clip.mp4")).unwrap()),
        sha(&video)
    );
    assert!(local_root.join("Local/Sub/notes.txt").exists());
    assert_eq!(stats.loads.load(Ordering::SeqCst), loads);
    assert_eq!(done.title, "Holiday");
    // by media id through the dedicated route
    let j2 = routes::copy(&svc, &*lib, &json!({ "ids": [lib.row(fid, "plain.jpg").id], "to": { "folderId": local.to_string(), "dir": "" } })).await.unwrap();
    let d2 = env.jobs.wait(&j2.id).await.unwrap();
    assert_eq!(d2.status, JobStatus::Done);
    assert_eq!(d2.result.unwrap().name, "plain.jpg");
    // into a remote folder: refused; bad specs
    let r3 = routes::copy(&svc, &*lib, &json!({ "ids": [lib.row(fid, "plain.jpg").id], "to": { "folderId": fid.to_string(), "dir": "" } })).await.unwrap_err();
    assert_eq!(r3.status, 403);
    assert_eq!(
        routes::copy(&svc, &*lib, &json!({ "ids": [1] }))
            .await
            .unwrap_err()
            .message,
        "to.folderId required"
    );
    assert_eq!(
        routes::copy(
            &svc,
            &*lib,
            &json!({ "to": { "folderId": local.to_string() } })
        )
        .await
        .unwrap_err()
        .message,
        "Nothing selected"
    );
    assert_eq!(routes::copy(&svc, &*lib, &json!({ "refs": [{ "folderId": fid.to_string(), "rel": "nope.jpg" }], "to": { "folderId": local.to_string() } })).await.unwrap_err().message, "Not in the MEGA link: nope.jpg");

    // ── refresh: new nodes are added, removed ones deleted (with events)
    env.events.take();
    let keep = lib.row(fid, "big.jpg");
    fake.add_folder_link(
        "Tmp00001",
        "Tmp",
        &[("Hnew0001", "new.png", png(64, 48, [1, 2, 3]), vec![])],
    );
    fake.with(|i| {
        let f = i.folders.get_mut("Holiday1").unwrap();
        f.3.retain(|h| h != "Hsq00001");
        f.3.push("Hnew0001".into());
        i.files.get_mut("Hnew0001").unwrap().parent = Some("Roliday1".into());
    });
    routes::refresh_folder(&svc, &*lib, &fid.to_string())
        .await
        .unwrap();
    let names: Vec<String> = lib.rows_of(fid).iter().map(|r| r.name.clone()).collect();
    assert!(names.contains(&"new.png".to_string()), "{names:?}");
    assert!(!names.contains(&"sq.jpg".to_string()));
    // unchanged rows keep their id, metadata and thumbnail
    let again = lib.row(fid, "big.jpg");
    assert_eq!(
        (again.id, again.lat, again.thumb_state),
        (keep.id, keep.lat, 1)
    );
    let evs = env.events.snapshot();
    assert!(evs.iter().any(|e| matches!(e, ServerEvent::Media { folder_id, reason: Some(MediaReason::Add), .. } if *folder_id == fid.to_string())), "{evs:?}");
    assert!(
        lib.folder_ref(fid)
            .unwrap()
            .remote
            .unwrap()
            .unwrap()
            .status
            .listed_at
            .unwrap()
            > now_ms() - 10_000
    );
    assert_eq!(
        routes::refresh_folder(&svc, &*lib, &local.to_string())
            .await
            .unwrap_err()
            .message,
        "Not a remote folder"
    );
    assert_eq!(
        routes::refresh_folder(&svc, &*lib, "999")
            .await
            .unwrap_err()
            .status,
        404
    );

    // ── offline: rows stay, status says so; a removed link is reported
    stats.offline.store(true, Ordering::SeqCst);
    let e = routes::refresh_folder(&svc, &*lib, &fid.to_string())
        .await
        .unwrap_err();
    assert_eq!((e.status, e.message.as_str()), (502, "Cannot reach MEGA"));
    let st = lib.folder_ref(fid).unwrap().remote.unwrap().unwrap().status;
    assert_eq!(
        (st.state, st.code),
        (RemoteFolderState::Offline, Some(RemoteErrorCode::Network))
    );
    let retry = st.retry_at.unwrap();
    assert!(
        retry > now_ms() + 50_000 && retry <= now_ms() + 60_000,
        "first retry after 1 min"
    );
    assert!(st.listed_at.is_some());
    assert!(!lib.rows_of(fid).is_empty());
    // the retry schedule grows: 1, 2, 5 … min
    let _ = svc.refresh(fid).await;
    let st2 = lib.folder_ref(fid).unwrap().remote.unwrap().unwrap().status;
    assert!(st2.retry_at.unwrap() > now_ms() + 110_000);
    stats.offline.store(false, Ordering::SeqCst);
    let gone = svc.create(&json!({ "url": format!("https://mega.nz/folder/Gone1234#{key}"), "libraryId": created.library.id })).await.unwrap_err();
    assert_eq!(gone.status, 404, "{}", gone.message);
    assert_eq!(
        svc.create(&json!({ "url": "https://example.com/x" }))
            .await
            .unwrap_err()
            .status,
        400
    );
    assert_eq!(
        svc.create(&json!({ "url": 5 })).await.unwrap_err().message,
        "Not a MEGA link"
    );
    routes::refresh_folder(&svc, &*lib, &fid.to_string())
        .await
        .unwrap();
    assert_eq!(
        lib.folder_ref(fid)
            .unwrap()
            .remote
            .unwrap()
            .unwrap()
            .status
            .state,
        RemoteFolderState::Ok
    );

    // ── transfer quota: status + retry time, then no more requests to MEGA
    fake.with(|i| i.faults.dl_509 = Some(1200));
    let r = http
        .get(furl(clip))
        .header("range", "bytes=0-99")
        .send()
        .await
        .unwrap();
    let st = r.status().as_u16();
    let body = r.bytes().await.unwrap_or_default();
    assert!(st == 429 || (st == 206 && body.len() < 100), "{st}");
    until(
        || (svc.quota_until() as i64 > now_ms()).then_some(()),
        10_000,
    )
    .await;
    let st = lib.folder_ref(fid).unwrap().remote.unwrap().unwrap().status;
    assert_eq!(
        (st.state, st.code),
        (RemoteFolderState::Quota, Some(RemoteErrorCode::Quota))
    );
    assert!(st.retry_at.unwrap() > now_ms() + 1000 * 1000);
    assert!(
        lib.get_setting("remoteQuota").unwrap()["until"]
            .as_f64()
            .unwrap()
            > 0.0
    );
    let reads = stats.reads.load(Ordering::SeqCst);
    let again = http
        .get(furl(clip))
        .header("range", "bytes=0-99")
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 429);
    assert!(again.headers().contains_key("retry-after"));
    assert_eq!(stats.reads.load(Ordering::SeqCst), reads);
    // a zip entry during the quota fails cleanly (no MEGA request)
    let mut z = svc.zip_entry(&lib.folder_row(fid), &lib.media(clip).unwrap());
    assert!(z.next().await.unwrap().is_err());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(stats.reads.load(Ordering::SeqCst), reads);
    fake.with(|i| i.faults.dl_509 = None);
    svc.set_quota_until(0);
    // the 60 s tick: an expired pause is lifted and persisted as {}
    svc.set_quota_until(now_ms() as u64 - 1);
    svc.tick();
    assert_eq!(lib.get_setting("remoteQuota").unwrap(), json!({}));

    // ── zip download streams remote entries
    let collect = |mut s: pa_jobs::library::BodyStream| async move {
        let mut v = Vec::new();
        while let Some(c) = s.next().await {
            v.extend_from_slice(&c.unwrap());
        }
        v
    };
    assert_eq!(
        sha(&collect(svc.zip_entry(
            &lib.folder_row(fid),
            &lib.media(lib.row(fid, "plain.jpg").id).unwrap()
        ))
        .await),
        sha(&photo)
    );
    assert_eq!(
        sha(&collect(svc.zip_entry(&lib.folder_row(fid), &lib.media(clip).unwrap())).await),
        sha(&video)
    );

    // ── an aborted zip download / stream releases its MEGA transfer slots
    let folder = lib.folder_row(fid);
    for _ in 0..3 {
        let mut s = svc.zip_entry(&folder, &lib.media(clip).unwrap());
        assert!(s.next().await.unwrap().is_ok());
        drop(s);
    }
    for _ in 0..5 {
        let mut r = http.get(furl(clip)).send().await.unwrap();
        assert!(r.chunk().await.unwrap().is_some());
        drop(r);
    }
    until(
        || (svc.lane_stats(&folder).unwrap() == (0, 0)).then_some(()),
        5000,
    )
    .await;
    let r = http
        .get(furl(clip))
        .header("range", "bytes=0-99")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 206);

    // ── removing the link drops its rows only
    lib.delete_folder(fid);
    svc.forget(fid);
    assert!(lib.rows_of(fid).is_empty());
    assert!(local_root.join("Local/big.jpg").exists());
    svc.close().await;
    let _ = std::fs::remove_dir_all(&env.tmp);
    let _ = std::fs::remove_dir_all(&tmp0);
}

/// remote-folders.test.ts "close() aborts stalled background transfers instead of waiting for them"
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_aborts_stalled_transfers() {
    let fake = FakeMega::start().await;
    let data = vec![7u8; 500_000];
    let url = fake.add_folder_link(
        "Slow1234",
        "Slow",
        &[
            ("Sa000001", "a.jpg", data.clone(), vec![]),
            ("Sb000001", "b.jpg", data, vec![]),
        ],
    );
    let stats = Arc::new(Stats::default());
    stats.stall.store(true, Ordering::SeqCst);
    let env = start(
        "close",
        Arc::new(Probe {
            inner: client(&fake),
            stats: stats.clone(),
        }),
    )
    .await;
    env.svc.create(&json!({ "url": url })).await.unwrap();
    until(
        || (stats.started.load(Ordering::SeqCst) >= 2).then_some(()),
        5000,
    )
    .await;
    let t0 = std::time::Instant::now();
    env.svc.close().await;
    assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
    assert!(stats.aborted.load(Ordering::SeqCst) >= stats.started.load(Ordering::SeqCst));
    // after close: no listing, no transfers
    assert_eq!(env.svc.refresh(1).await.unwrap_err().status, 503);
    let _ = std::fs::remove_dir_all(&env.tmp);
}
