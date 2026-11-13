//! Mirrors packages/server/test/devices.integration.test.ts (the hub / tunnel / transfer parts),
//! driving a Rust port of test/helpers/fake-phone.ts against a real axum server, and replaying
//! the golden transcripts of crates/pa-link/tests/fixtures/link.

mod common;

use common::*;
use futures_util::StreamExt;
use pa_core::{FileRef, JobStatus, MediaKind};
use pa_link::access::{self, RequestInfo, RunMode};
use pa_link::hub::sha256_hex;
use pa_link::store::DeviceStore;
use pa_link::transfer::TransferHost;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const FX: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/link"
);

fn fixture(n: &str) -> Vec<Value> {
    serde_json::from_str(&std::fs::read_to_string(Path::new(FX).join(n)).unwrap()).unwrap()
}

fn keys(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

struct Fx {
    tmp: PathBuf,
    lib: PathBuf,
    phone_root: PathBuf,
    vid_bytes: Vec<u8>,
}

fn setup(tag: &str) -> Fx {
    let tmp = std::env::temp_dir().join(format!("pa-link-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let lib = tmp.join("lib");
    let phone_root = tmp.join("phone").join("Camera");
    std::fs::create_dir_all(phone_root.join("Trip")).unwrap();
    std::fs::create_dir_all(phone_root.join("Empty")).unwrap();
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(phone_root.join("IMG_1.jpg"), [0xff, 0xd8, 0xff, 0xd9]).unwrap();
    let vid_bytes = virtual_bytes(0, 4096);
    let vid = phone_root.join("Trip").join("VID_2.mp4");
    std::fs::write(&vid, &vid_bytes).unwrap();
    let photo = lib.join("photo.jpg");
    std::fs::write(&photo, b"\xff\xd8 pc photo \xff\xd9").unwrap();
    let set = |p: &Path, ms: i64| {
        let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
        f.set_modified(std::time::UNIX_EPOCH + Duration::from_millis(ms as u64))
            .unwrap();
    };
    set(&vid, 1700000500000);
    set(&photo, 1690000000000);
    Fx {
        tmp,
        lib,
        phone_root,
        vid_bytes,
    }
}

async fn get(s: &Server, path: &str, headers: &[(&str, &str)]) -> reqwest::Response {
    let mut r = reqwest::Client::new().get(format!("{}{path}", s.base));
    for (k, v) in headers {
        r = r.header(*k, *v);
    }
    r.send().await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn linked_phone_end_to_end() {
    let fx = tokio::task::block_in_place(|| setup("e2e"));
    let s = start(fx.lib.clone()).await;
    let pairing = fixture("pairing.json");
    let hello = pairing[0]["frame"].clone();
    let phone_id = hello["id"].as_str().unwrap().to_string();
    let lt = fixture("list-and-thumb.json");

    // ── pairs a new phone following the golden pairing transcript

    eprintln!("STEP pairs a new phone following the golden p");
    let mut phone = FakePhone::new(
        &s.base,
        &phone_id,
        vec![PhoneFolder {
            key: "k1".into(),
            name: "Kamera".into(),
            root: fx.phone_root.clone(),
        }],
        None,
        "Test Phone",
    );
    phone.st.lock().unwrap().list_body = Some(lt[1]["body"].as_str().unwrap().into());
    let first = phone.connect(Some(hello.clone()), &[]).await;
    assert_eq!(keys(&first), keys(&pairing[1]["frame"]));
    assert_eq!(first["t"], "pending");
    let code = first["code"].as_str().unwrap().to_string();
    assert!(code.len() == 4 && code.bytes().all(|c| c.is_ascii_digit()));
    let pair = pair_events(&s.events);
    assert_eq!(
        (
            pair[0].id.as_str(),
            pair[0].name.as_str(),
            pair[0].paired,
            pair[0].code.as_deref()
        ),
        (phone_id.as_str(), "Test Phone", false, Some(code.as_str()))
    );
    let pending = s.hub.list(None);
    assert_eq!(pending.len(), 1);
    assert!(
        !pending[0].paired
            && pending[0].online
            && pending[0].code.as_deref() == Some(code.as_str())
    );
    // a device can never approve itself
    let me = RequestInfo {
        headers: [("x-peek-device".to_string(), phone_id.clone())].into(),
        ..Default::default()
    };
    assert_eq!(
        access::require_approver(RunMode::Desktop, &me, Some(&phone_id))
            .unwrap_err()
            .status,
        403
    );
    access::require_approver(RunMode::Desktop, &RequestInfo::default(), Some(&phone_id)).unwrap();
    s.hub.approve(&phone_id).unwrap();
    let paired = phone.wait_frame(|f| f["t"] == "paired", 5000).await;
    assert_eq!(keys(&paired), keys(&pairing[2]["frame"]));
    let token = paired["token"].as_str().unwrap().to_string();
    assert!(
        token.len() == 43
            && token
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    );
    let row = s.mem.device(&phone_id);
    let row = row.unwrap();
    assert!(row.paired);
    assert_eq!(row.token_hash.as_deref(), Some(sha256_hex(&token).as_str()));
    // the server lists the shared folder right away (fixture NDJSON body)
    let list_req = phone
        .wait_frame(|f| f["t"] == "req" && f["op"] == "list", 5000)
        .await;
    assert_eq!(list_req["args"], lt[0]["frame"]["args"]);
    let devs = until(
        || {
            Some(s.hub.list(None)).filter(|d| {
                d.first()
                    .and_then(|d| d.folders.first())
                    .is_some_and(|f| f.media_count == 2)
            })
        },
        10_000,
    )
    .await;
    assert!(
        devs[0].paired
            && devs[0].online
            && devs[0].caps
                == Some(pa_core::DeviceCaps {
                    read: true,
                    receive: true
                })
    );
    let f0 = &devs[0].folders[0];
    assert_eq!(
        (
            f0.name.as_str(),
            f0.library_id.clone(),
            f0.temp,
            f0.path.clone()
        ),
        ("Kamera", None, false, format!("device://{phone_id}/k1"))
    );
    assert_eq!(
        f0.device.as_ref().map(|d| (d.name.as_str(), d.online)),
        Some(("Test Phone", true))
    );
    let dev_folder: i64 = f0.id.parse().unwrap();
    assert_eq!(
        s.hub.folder_owner(dev_folder).as_deref(),
        Some(phone_id.as_str())
    );

    // ── rows, sub folders (device_dirs)

    eprintln!("STEP rows, sub folders (device_dirs)");
    let img = s.mem.row(dev_folder, "", "IMG_1.jpg").unwrap();
    assert_eq!(
        (
            img.width,
            img.height,
            img.size,
            img.mtime,
            img.taken,
            img.playback
        ),
        (
            Some(4000.0),
            Some(3000.0),
            2048,
            1700000000000,
            Some(1699999999000),
            pa_core::Playback::Direct
        )
    );
    let vid = s.mem.row(dev_folder, "Trip", "VID_2.mp4").unwrap();
    assert_eq!(
        (vid.kind, vid.duration, vid.width, vid.height),
        (MediaKind::Video, Some(12.5), Some(1920.0), Some(1080.0))
    );
    assert_eq!(s.mem.dirs_of(dev_folder), ["Empty", "Trip"]);

    // ── thumbnails through the tunnel; the background loop fills thumbhashes

    eprintln!("STEP thumbnails through the tunnel; the backg");
    let t = phone
        .wait_frame(
            |f| f["t"] == "req" && f["op"] == "thumb" && f["args"]["p"] == "IMG_1.jpg",
            10_000,
        )
        .await;
    assert!(pa_link::hub::RID_RE.is_match(t["rid"].as_str().unwrap()));
    assert!([320, 640, 1600].contains(&t["args"]["s"].as_i64().unwrap()));
    until(
        || {
            s.mem
                .row(dev_folder, "", "IMG_1.jpg")
                .and_then(|r| r.thumbhash)
        },
        10_000,
    )
    .await;
    // the golden fixture's error frame for a path outside the shared folder → 403 for the consumer
    let e = s
        .hub
        .thumb(&format!("device://{phone_id}/k1/../etc/passwd"), 320, true)
        .await
        .unwrap_err();
    assert_eq!(e.status, 400); // rejected by the server's own path check before reaching the phone
                               // a phone-side refusal maps through consumerStatus (fixture: 403 'outside shared folder')
    phone.set_intercept(Some(Box::new(|f: &Value| {
        f["op"] == "thumb" && f["args"]["p"] == "x/secret.jpg"
    })));
    let p2 = phone.clone();
    let h = tokio::spawn(async move {
        let f = p2
            .wait_frame(
                |f| f["t"] == "req" && f["args"]["p"] == "x/secret.jpg",
                5000,
            )
            .await;
        let fx_err = &fixture("list-and-thumb.json")[5]["frame"];
        let mut err = fx_err.clone();
        err["rid"] = f["rid"].clone();
        p2.send(&err);
    });
    let e = s
        .hub
        .thumb(&format!("device://{phone_id}/k1/x/secret.jpg"), 320, true)
        .await
        .unwrap_err();
    h.await.unwrap();
    assert_eq!(
        (e.status, e.message.as_str()),
        (403, "outside shared folder")
    );
    phone.set_intercept(None);

    // ── byte-exact ranges like the golden file-range transcript

    eprintln!("STEP byte-exact ranges like the golden file-r");
    let fr = fixture("file-range.json");
    let vid_id = vid.id;
    let r = get(
        &s,
        &format!("/api/media/{vid_id}/file"),
        &[("Range", "bytes=100-199")],
    )
    .await;
    assert_eq!(r.status(), 206);
    let req = phone
        .wait_frame(
            |f| f["t"] == "req" && f["op"] == "file" && f["args"]["range"] == "bytes=100-199",
            5000,
        )
        .await;
    assert_eq!(req["args"], fr[0]["frame"]["args"]);
    assert_eq!(
        r.headers()["content-range"],
        fr[1]["headers"]["X-Peek-Content-Range"].as_str().unwrap()
    );
    assert_eq!(r.headers()["content-length"], "100");
    assert_eq!(r.headers()["accept-ranges"], "bytes");
    assert_eq!(r.headers()["content-type"], "video/mp4");
    assert_eq!(sha(&r.bytes().await.unwrap()), sha(&fx.vid_bytes[100..200]));
    assert_eq!(phone.put_result(req["rid"].as_str().unwrap()).await.0, 204);
    let bad = get(
        &s,
        &format!("/api/media/{vid_id}/file"),
        &[("Range", "bytes=5000-")],
    )
    .await;
    assert_eq!(bad.status(), 416);
    assert_eq!(bad.headers()["content-range"], "bytes */4096");
    let suffix = get(
        &s,
        &format!("/api/media/{vid_id}/file"),
        &[("Range", "bytes=-10")],
    )
    .await;
    assert_eq!(suffix.status(), 206);
    assert_eq!(
        suffix.bytes().await.unwrap().as_ref(),
        &fx.vid_bytes[4086..]
    );
    let full = get(&s, &format!("/api/media/{vid_id}/file"), &[]).await;
    assert_eq!(
        (
            full.status().as_u16(),
            full.headers()["content-length"].to_str().unwrap()
        ),
        (200, "4096")
    );
    assert_eq!(
        full.headers()["last-modified"],
        "Tue, 14 Nov 2023 22:21:40 GMT"
    );
    assert_eq!(sha(&full.bytes().await.unwrap()), sha(&fx.vid_bytes));
    // device error frames map to HTTP errors
    {
        let p2 = phone.clone();
        let once = AtomicBool::new(false);
        phone.set_intercept(Some(Box::new(move |f: &Value| {
            if f["op"] != "file" || once.swap(true, Ordering::SeqCst) {
                return false;
            }
            p2.err(f["rid"].as_str().unwrap(), 404, "not found");
            true
        })));
    }
    assert_eq!(
        get(&s, &format!("/api/media/{vid_id}/file"), &[])
            .await
            .status(),
        404
    );
    phone.set_intercept(None);
    let head = reqwest::Client::new()
        .head(format!("{}/api/media/{vid_id}/file", s.base))
        .send()
        .await
        .unwrap();
    assert_eq!(
        (
            head.status().as_u16(),
            head.headers()["content-length"].to_str().unwrap()
        ),
        (200, "4096")
    );

    // ── details come from the phone

    eprintln!("STEP details come from the phone");
    let d = s
        .hub
        .details(&phone_id, "k1", "IMG_1.jpg")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (d.make.as_deref(), d.model.as_deref()),
        (Some("Google"), Some("Pixel 8"))
    );
    let ex = pa_link::media::details_exif(Some(&d)).unwrap();
    assert_eq!(
        (ex["Make"].clone(), ex["FNumber"].clone()),
        (json!("Google"), json!(1.7))
    );
    let m = pa_link::media::refine_meta(
        &d,
        pa_meta::ExtractedMeta {
            width: Some(4000.0),
            height: Some(3000.0),
            ..Default::default()
        },
    );
    assert_eq!(
        (m.lat, m.lon, m.make.as_deref(), m.model.as_deref()),
        (Some(40.99), Some(29.03), Some("Google"), Some("Pixel 8"))
    );
    assert_eq!(
        pa_link::media::details_path("Test Phone", "Kamera", "IMG_1.jpg"),
        "Test Phone/Kamera/IMG_1.jpg"
    );
    // renditions
    assert_eq!(
        s.hub
            .display(&format!("device://{phone_id}/k1/IMG_1.jpg"), 2048)
            .await
            .unwrap()
            .len(),
        4
    );
    let dreq = phone
        .wait_frame(|f| f["t"] == "req" && f["op"] == "display", 5000)
        .await;
    assert_eq!(
        dreq["args"],
        json!({ "key": "k1", "p": "IMG_1.jpg", "max": 2048 })
    );
    s.hub
        .strip(&format!("device://{phone_id}/k1/Trip/VID_2.mp4"), 4, 72)
        .await
        .unwrap();
    let sreq = phone
        .wait_frame(|f| f["t"] == "req" && f["op"] == "strip", 5000)
        .await;
    assert_eq!(
        sreq["args"],
        json!({ "key": "k1", "p": "Trip/VID_2.mp4", "n": 4, "h": 72 })
    );
    // loopback ffmpeg input
    assert_eq!(
        pa_link::media::device_ffmpeg_input(&s.hub, vid_id)
            .unwrap_err()
            .status,
        501
    );
    s.hub.set_self_url(s.base.clone());
    let inp = pa_link::media::device_ffmpeg_input(&s.hub, vid_id).unwrap();
    assert_eq!(
        inp[1],
        format!("X-Peek-Internal: {}\r\n", s.hub.internal_token)
    );

    // ── consumer abort → cancel frame and the reply PUT gets 499

    eprintln!("STEP consumer abort → cancel frame and the re");
    let add_virtual = |rel: &str, size: u64| {
        phone.st.lock().unwrap().virtual_files.insert(
            format!("k1/{rel}"),
            VirtualFile {
                size,
                mtime: 1700001000000,
            },
        );
    };
    add_virtual("Big/abort.mp4", 32 * 1024 * 1024);
    add_virtual("Big/large.mp4", 50 * 1024 * 1024);
    s.hub.relist(dev_folder, "").await.unwrap();
    let abort_id = s.mem.row(dev_folder, "Big", "abort.mp4").unwrap().id;
    let mut r = get(&s, &format!("/api/media/{abort_id}/file"), &[]).await;
    assert!(!r.chunk().await.unwrap().unwrap().is_empty());
    drop(r);
    let freq = phone
        .wait_frame(
            |f| f["t"] == "req" && f["op"] == "file" && f["args"]["p"] == "Big/abort.mp4",
            5000,
        )
        .await;
    let cancel = phone
        .wait_frame(|f| f["t"] == "cancel" && f["rid"] == freq["rid"], 5000)
        .await;
    assert_eq!(keys(&cancel), keys(&fr[4]["frame"]));
    let put = phone.put_result(freq["rid"].as_str().unwrap()).await;
    assert!(put.0 == 499 || put.0 == 0, "{put:?}");
    eprintln!("abort: PUT answered {:?}", put);
    assert!(put.1 < 32 * 1024 * 1024);
    until(|| (s.hub.stats(&phone_id).0 == 0).then_some(()), 10_000).await;

    // ── slow consumer: backpressure keeps the gap bounded (50 MB)

    eprintln!("STEP slow consumer: backpressure keeps the ga");
    let large_id = s.mem.row(dev_folder, "Big", "large.mp4").unwrap().id;
    phone.st.lock().unwrap().written = 0;
    let mut r = get(&s, &format!("/api/media/{large_id}/file"), &[]).await;
    assert_eq!(r.status(), 200);
    let mut read = 0u64;
    let mut max_gap = 0u64;
    let mut hash = <sha2::Sha256 as sha2::Digest>::new();
    while let Some(c) = r.chunk().await.unwrap() {
        sha2::Digest::update(&mut hash, &c);
        read += c.len() as u64;
        let w = phone.st.lock().unwrap().written;
        max_gap = max_gap.max(w.saturating_sub(read));
        if read % (256 * 1024) < c.len() as u64 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    assert_eq!(read, 50 * 1024 * 1024);
    assert_eq!(
        hex::encode(sha2::Digest::finalize(hash)),
        sha(&virtual_bytes(0, 50 * 1024 * 1024))
    );
    assert!(max_gap < 24 * 1024 * 1024, "{max_gap}");
    eprintln!("backpressure: max gap {max_gap} bytes");

    // ── first byte timeout → 504 and a cancel frame; a late reply is refused

    eprintln!("STEP first byte timeout → 504 and a cancel fr");
    s.hub
        .timeouts
        .file_ms
        .store(300, std::sync::atomic::Ordering::Relaxed);
    phone.st.lock().unwrap().ignore.insert("file".into());
    phone.clear_frames();
    let r = get(&s, &format!("/api/media/{vid_id}/file"), &[]).await;
    assert_eq!(r.status(), 504);
    let req = phone
        .wait_frame(
            |f| f["t"] == "req" && f["op"] == "file" && f["args"]["p"] == "Trip/VID_2.mp4",
            5000,
        )
        .await;
    phone
        .wait_frame(|f| f["t"] == "cancel" && f["rid"] == req["rid"], 5000)
        .await;
    let late = phone
        .put(
            req["rid"].as_str().unwrap(),
            vec![],
            reqwest::Body::from("late"),
            vec![
                ("Authorization".into(), format!("Bearer {token}")),
                ("X-Peek-Device".into(), phone_id.clone()),
            ],
        )
        .await;
    assert_eq!(late.0, 410);
    s.hub
        .timeouts
        .file_ms
        .store(30_000, std::sync::atomic::Ordering::Relaxed);
    phone.st.lock().unwrap().ignore.clear();

    // ── device token on phone endpoints

    eprintln!("STEP device token on phone endpoints");
    let rid = "A".repeat(22);
    let auth = vec![
        ("Authorization".to_string(), format!("Bearer {token}")),
        ("X-Peek-Device".to_string(), phone_id.clone()),
    ];
    assert_eq!(
        phone
            .put(&rid, vec![], reqwest::Body::from("x"), vec![])
            .await
            .0,
        401
    );
    assert_eq!(
        phone
            .put(
                &rid,
                vec![],
                reqwest::Body::from("x"),
                vec![
                    ("Authorization".into(), format!("Bearer {}", "x".repeat(43))),
                    ("X-Peek-Device".into(), phone_id.clone())
                ]
            )
            .await
            .0,
        401
    );
    assert_eq!(
        phone
            .put(&rid, vec![], reqwest::Body::from("x"), auth.clone())
            .await
            .0,
        410
    );
    assert_eq!(
        get(&s, "/api/devices/pull/whatever", &[]).await.status(),
        401
    );
    let hdr: Vec<(&str, &str)> = auth.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(
        get(&s, "/api/devices/pull/whatever", &hdr).await.status(),
        404
    );

    // ── copies device → PC as a transfer job (bytes + mtime preserved)

    eprintln!("STEP copies device → PC as a transfer job (by");
    let refs = vec![FileRef {
        folder_id: dev_folder.to_string(),
        rel: "Trip/VID_2.mp4".into(),
    }];
    let lib_folder = s.mem.folder("1000").unwrap();
    let job = s
        .transfers
        .copy_from_device(&refs, &lib_folder, "FromPhone")
        .unwrap();
    assert_eq!(
        (job.files, job.bytes),
        (
            Some(pa_core::Progress { done: 0, total: 1 }),
            Some(pa_core::Progress {
                done: 0,
                total: 4096
            })
        )
    );
    let fin = s.jobs.wait(&job.id).await.unwrap();
    assert_eq!(fin.status, JobStatus::Done, "{:?}", fin.error);
    assert_eq!(
        fin.bytes,
        Some(pa_core::Progress {
            done: 4096,
            total: 4096
        })
    );
    assert_eq!(
        fin.result
            .as_ref()
            .map(|r| (r.folder_id.as_str(), r.dir.as_str(), r.name.as_str())),
        Some(("1000", "FromPhone", "VID_2.mp4"))
    );
    let dest = fx.lib.join("FromPhone").join("VID_2.mp4");
    assert_eq!(sha(&std::fs::read(&dest).unwrap()), sha(&fx.vid_bytes));
    let mt = std::fs::metadata(&dest)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert!((mt - 1700000500000).abs() < 2);
    assert!(std::fs::read_dir(fx.lib.join("FromPhone"))
        .unwrap()
        .all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".part")));
    assert!(s
        .mem
        .upserts
        .lock()
        .unwrap()
        .contains(&(1000, "FromPhone/VID_2.mp4".into())));
    // a whole directory keeps its tree
    let job = s
        .transfers
        .copy_from_device(
            &[FileRef {
                folder_id: dev_folder.to_string(),
                rel: "Trip".into(),
            }],
            &lib_folder,
            "FromPhone",
        )
        .unwrap();
    assert_eq!(s.jobs.wait(&job.id).await.unwrap().status, JobStatus::Done);
    assert_eq!(
        sha(&std::fs::read(fx.lib.join("FromPhone/Trip/VID_2.mp4")).unwrap()),
        sha(&fx.vid_bytes)
    );
    assert!(s
        .mem
        .upserts
        .lock()
        .unwrap()
        .contains(&(1000, "FromPhone/Trip/VID_2.mp4".into())));

    // ── copies PC → device with the receive flow (one-time pull URL), golden receive transcript

    eprintln!("STEP copies PC → device with the receive flow");
    let rcv = fixture("receive.json");
    let dev_row = DeviceStore::device_folder(&s.mem, dev_folder).unwrap();
    phone.clear_frames();
    let job = s
        .transfers
        .copy_to_device(
            &[FileRef {
                folder_id: "1000".into(),
                rel: "photo.jpg".into(),
            }],
            &dev_row,
            "From PC",
        )
        .await
        .unwrap();
    assert_eq!(job.kind, pa_core::JobType::Transfer);
    let fin = s.jobs.wait(&job.id).await.unwrap();
    assert_eq!(fin.status, JobStatus::Done, "{:?}", fin.error);
    assert_eq!(
        fin.result
            .as_ref()
            .map(|r| (r.folder_id.clone(), r.dir.clone(), r.name.clone())),
        Some((dev_folder.to_string(), "From PC".into(), "photo.jpg".into()))
    );
    let recv = phone
        .wait_frame(|f| f["t"] == "req" && f["op"] == "receive", 5000)
        .await;
    assert_eq!(keys(&recv["args"]), keys(&rcv[0]["frame"]["args"]));
    let photo_size = std::fs::metadata(fx.lib.join("photo.jpg")).unwrap().len();
    assert_eq!(
        (
            recv["args"]["key"].clone(),
            recv["args"]["dir"].clone(),
            recv["args"]["name"].clone(),
            recv["args"]["size"].clone(),
            recv["args"]["mtime"].clone()
        ),
        (
            json!("k1"),
            json!("From PC"),
            json!("photo.jpg"),
            json!(photo_size),
            json!(1690000000000i64)
        )
    );
    let pull_headers = phone.st.lock().unwrap().last_pull_headers.clone();
    for (h, _) in rcv[1]["response"]["headers"].as_object().unwrap() {
        assert!(pull_headers.contains_key(&h.to_lowercase()), "{h}");
    }
    assert_eq!(pull_headers["x-peek-mtime"], "1690000000000");
    assert_eq!(
        pull_headers["last-modified"],
        rcv[1]["response"]["headers"]["Last-Modified"]
            .as_str()
            .unwrap()
    );
    let landed = fx.phone_root.join("From PC").join("photo.jpg");
    assert_eq!(
        sha(&std::fs::read(&landed).unwrap()),
        sha(&std::fs::read(fx.lib.join("photo.jpg")).unwrap())
    );
    let url = recv["args"]["url"].as_str().unwrap();
    assert!(regex::Regex::new(r"^/api/devices/pull/[A-Za-z0-9_-]+$")
        .unwrap()
        .is_match(url));
    assert_eq!(get(&s, url, &hdr).await.status(), 404); // one-time
    until(|| s.mem.row(dev_folder, "From PC", "photo.jpg"), 10_000).await;

    // ── rejects a second phone (4403)

    eprintln!("STEP rejects a second phone (4403)");
    let p2 = FakePhone::new(
        &s.base,
        "aaaaaaaa-0000-4000-8000-000000000002",
        vec![PhoneFolder {
            key: "x".into(),
            name: "X".into(),
            root: fx.phone_root.clone(),
        }],
        None,
        "Other",
    );
    assert_eq!(p2.connect(None, &[]).await["t"], "pending");
    s.hub
        .reject("aaaaaaaa-0000-4000-8000-000000000002")
        .unwrap();
    p2.wait_frame(|f| f["t"] == "rejected", 5000).await;
    assert_eq!(p2.closed().await, 4403);
    assert!(s
        .mem
        .device("aaaaaaaa-0000-4000-8000-000000000002")
        .is_none());

    // ── a reconnect with the token gets welcome and replaces the older socket (4409)

    eprintln!("STEP a reconnect with the token gets welcome ");
    let again = FakePhone::new(
        &s.base,
        &phone_id,
        phone.folders.clone(),
        Some(token.clone()),
        "Test Phone",
    );
    again.st.lock().unwrap().virtual_files = phone.st.lock().unwrap().virtual_files.clone();
    let w = again.connect(None, &[]).await;
    assert_eq!(
        (w["t"].clone(), w["paired"].clone(), w["server"].clone()),
        (json!("welcome"), json!(true), json!("test-pc"))
    );
    assert_eq!(phone.closed().await, 4409);
    phone = again;
    // a wrong token never gets in: it is treated like a new phone (pending)
    let impostor = FakePhone::new(
        &s.base,
        &phone_id,
        phone.folders.clone(),
        Some("y".repeat(43)),
        "Test Phone",
    );
    assert_eq!(impostor.connect(None, &[]).await["t"], "pending");
    assert!(s.hub.is_online(&phone_id));
    impostor.close();
    until(
        || s.hub.list(None).iter().all(|d| d.paired).then_some(()),
        10_000,
    )
    .await;

    // ── LAN share: a phone connected through it is dropped when sharing stops

    eprintln!("STEP LAN share: a phone connected through it ");
    let via_lan = FakePhone::new(
        &s.base,
        &phone_id,
        phone.folders.clone(),
        Some(token.clone()),
        "Test Phone",
    );
    assert_eq!(
        via_lan.connect(None, &[("x-test-lan", "1")]).await["t"],
        "welcome"
    );
    assert_eq!(phone.closed().await, 4409);
    s.hub.close_lan();
    assert_eq!(via_lan.closed().await, 1001);
    until(|| (!s.hub.is_online(&phone_id)).then_some(()), 10_000).await;
    let back = FakePhone::new(
        &s.base,
        &phone_id,
        phone.folders.clone(),
        Some(token.clone()),
        "Test Phone",
    );
    assert_eq!(back.connect(None, &[]).await["t"], "welcome");
    phone = back;

    // ── a phone never sees its own folders (list(hide) + deviceAccessHook)

    eprintln!("STEP a phone never sees its own folders");
    {
        let tok = phone.token().unwrap();
        let own = RequestInfo {
            headers: HashMap::from([
                ("x-peek-device".to_string(), phone_id.clone()),
                ("authorization".to_string(), format!("Bearer {tok}")),
            ]),
            lan: true,
            ..Default::default()
        };
        let v = access::viewer_of(&s.hub, RunMode::Desktop, &own);
        assert_eq!(v.self_id.as_deref(), Some(phone_id.as_str()));
        assert!(s
            .hub
            .list(v.self_id.as_deref())
            .iter()
            .all(|d| d.id != phone_id));
        assert!(s.hub.list(None).iter().any(|d| d.id == phone_id));
        let fom = |id: i64| s.mem.row_by_id(id).map(|r| r.folder_id);
        let q = HashMap::from([("folderId".to_string(), dev_folder.to_string())]);
        let refs = access::referenced(&q, "/api/browse", None, None);
        let e = access::check_access(&s.hub, RunMode::Desktop, "/api/browse", &own, &refs, &fom)
            .unwrap_err();
        assert_eq!((e.status, e.message.as_str()), (404, "Folder not found"));
        let refs = access::referenced(
            &HashMap::new(),
            "/api/media/:id/thumb",
            Some(&img.id.to_string()),
            None,
        );
        let e = access::check_access(
            &s.hub,
            RunMode::Desktop,
            "/api/media/1/thumb",
            &own,
            &refs,
            &fom,
        )
        .unwrap_err();
        assert_eq!((e.status, e.message.as_str()), (404, "Media not found"));
        // the PC window (not LAN) sees it; a LAN browser without a device token does not
        let refs = access::referenced(&q, "/api/browse", None, None);
        assert!(access::check_access(
            &s.hub,
            RunMode::Desktop,
            "/api/browse",
            &RequestInfo::default(),
            &refs,
            &fom
        )
        .is_ok());
        let lan = RequestInfo {
            lan: true,
            ..Default::default()
        };
        assert!(
            access::check_access(&s.hub, RunMode::Desktop, "/api/browse", &lan, &refs, &fom)
                .is_err()
        );
        // /api/devices/* is never filtered
        assert!(access::check_access(
            &s.hub,
            RunMode::Desktop,
            "/api/devices/x/approve",
            &own,
            &refs,
            &fom
        )
        .is_ok());
    }

    // ── offline: file requests 503, rows stay

    eprintln!("STEP offline: file requests 503, rows stay");
    phone.close();
    until(|| (!s.hub.list(None)[0].online).then_some(()), 10_000).await;
    let f = get(&s, &format!("/api/media/{vid_id}/file"), &[]).await;
    assert_eq!(f.status(), 503);
    assert_eq!(
        serde_json::from_slice::<Value>(&f.bytes().await.unwrap()).unwrap()["error"],
        "device offline"
    );
    assert!(s.mem.row(dev_folder, "", "IMG_1.jpg").is_some());
    assert!(
        !s.mem.folder_refs(&phone_id)[0]
            .device
            .as_ref()
            .unwrap()
            .online
    );

    // ── forget removes the device, its folders and rows

    eprintln!("STEP forget removes the device, its folders a");
    s.hub.forget(&phone_id).unwrap();
    assert!(s.mem.device_folders(&phone_id).is_empty());
    assert!(s.mem.row_by_id(img.id).is_none());
    assert!(s.hub.list(None).iter().all(|d| d.id != phone_id));
    assert!(s.hub.folder_owner(dev_folder).is_none());
    assert_eq!(s.hub.forget(&phone_id).unwrap_err().status, 404);

    s.hub.close().await;
    let _ = std::fs::remove_dir_all(&fx.tmp);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hello_rules_and_close_codes() {
    let s = start(std::env::temp_dir()).await;
    let raw = |hello: Value| {
        let s = s.base.clone();
        async move {
            let p = FakePhone::new(
                &s,
                "bbbbbbbb-0000-4000-8000-000000000009",
                vec![],
                None,
                "P",
            );
            let url = format!("{}/api/devices/connect?v=1", s.replacen("http", "ws", 1));
            let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
            use futures_util::SinkExt;
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                hello.to_string().into(),
            ))
            .await
            .unwrap();
            let mut code = 0;
            while let Some(Ok(m)) = ws.next().await {
                if let tokio_tungstenite::tungstenite::Message::Close(c) = m {
                    code = c.map(|c| u16::from(c.code)).unwrap_or(0);
                    break;
                }
            }
            drop(p);
            code
        }
    };
    assert_eq!(raw(json!({ "t": "ping" })).await, 4400);
    assert_eq!(
        raw(json!({ "t": "hello", "v": 2, "id": "abcdefgh" })).await,
        4426
    );
    assert_eq!(
        raw(json!({ "t": "hello", "v": 1, "id": "short" })).await,
        4400
    );
    // X-Peek-Device must equal hello.id
    let p = FakePhone::new(
        &s.base,
        "cccccccc-0000-4000-8000-000000000001",
        vec![],
        None,
        "P",
    );
    let url = format!(
        "{}/api/devices/connect?v=1",
        s.base.replacen("http", "ws", 1)
    );
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut req = url.into_client_request().unwrap();
    req.headers_mut().insert(
        "X-Peek-Device",
        "dddddddd-0000-4000-8000-000000000001".parse().unwrap(),
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    use futures_util::SinkExt;
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        p.hello_frame().to_string().into(),
    ))
    .await
    .unwrap();
    let mut code = 0;
    while let Some(Ok(m)) = ws.next().await {
        if let tokio_tungstenite::tungstenite::Message::Close(c) = m {
            code = c.map(|c| u16::from(c.code)).unwrap_or(0);
            break;
        }
    }
    assert_eq!(code, 4400);
    // ping → pong before pairing; too many pending pairings → 4429
    let mut phones = Vec::new();
    for i in 0..16 {
        let ph = FakePhone::new(
            &s.base,
            &format!("eeeeeeee-0000-4000-8000-{i:012}"),
            vec![],
            None,
            "P",
        );
        assert_eq!(ph.connect(None, &[]).await["t"], "pending");
        phones.push(ph);
    }
    phones[0].send(&json!({ "t": "ping" }));
    phones[0].wait_frame(|f| f["t"] == "pong", 5000).await;
    let extra = FakePhone::new(
        &s.base,
        "ffffffff-0000-4000-8000-000000000001",
        vec![],
        None,
        "P",
    );
    let url = format!(
        "{}/api/devices/connect?v=1",
        s.base.replacen("http", "ws", 1)
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        extra.hello_frame().to_string().into(),
    ))
    .await
    .unwrap();
    let mut code = 0;
    while let Some(Ok(m)) = ws.next().await {
        if let tokio_tungstenite::tungstenite::Message::Close(c) = m {
            code = c.map(|c| u16::from(c.code)).unwrap_or(0);
            break;
        }
    }
    assert_eq!(code, 4429);
    // the same id again replaces its pending socket (4409)
    let dup = FakePhone::new(&s.base, &phones[1].id, vec![], None, "P");
    assert_eq!(dup.connect(None, &[]).await["t"], "pending");
    assert_eq!(phones[1].closed().await, 4409);
    // shutdown closes everything with 1001
    s.hub.close().await;
    assert_eq!(phones[2].closed().await, 1001);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn priorities_and_inflight_limits() {
    let fx = tokio::task::block_in_place(|| setup("prio"));
    let s = start(fx.lib.clone()).await;
    let id = "abababab-0000-4000-8000-000000000001";
    let ph = FakePhone::new(
        &s.base,
        id,
        vec![PhoneFolder {
            key: "k1".into(),
            name: "K".into(),
            root: fx.phone_root.clone(),
        }],
        None,
        "P",
    );
    ph.connect(None, &[]).await;
    s.hub.approve(id).unwrap();
    ph.wait_frame(|f| f["t"] == "paired", 5000).await;
    until(
        || {
            s.mem
                .device_folders(id)
                .first()
                .and_then(|f| (!s.mem.rows_of(f.id).is_empty()).then_some(()))
        },
        10_000,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    // stall the phone: every request stays in flight
    ph.st
        .lock()
        .unwrap()
        .ignore
        .extend(["thumb".to_string(), "file".into(), "display".into()]);
    ph.clear_frames();
    let hub = s.hub.clone();
    let mut tasks = Vec::new();
    for i in 0..6 {
        let h = hub.clone();
        tasks.push(tokio::spawn(async move {
            h.thumb(&format!("device://{id}/k1/bg{i}.jpg"), 640, false)
                .await
        }));
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    // background work never takes more than 4 of the 8 slots
    let (inflight, queued) = s.hub.stats(id);
    assert_eq!((inflight, queued), (4, 2));
    for i in 0..5 {
        let h = hub.clone();
        tasks.push(tokio::spawn(async move {
            h.display(&format!("device://{id}/k1/fg{i}.jpg"), 2048)
                .await
        }));
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(s.hub.stats(id), (8, 3));
    // a visible thumbnail (prio 0) queues in front of everything
    let h = hub.clone();
    let vis = tokio::spawn(async move {
        h.thumb(&format!("device://{id}/k1/visible.jpg"), 320, true)
            .await
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    // free one slot: the first answered request lets the visible one go next
    let first = ph
        .wait_frame(|f| f["t"] == "req" && f["op"] == "thumb", 5000)
        .await;
    ph.reply(
        first["rid"].as_str().unwrap(),
        vec![("Content-Type", "image/jpeg".into())],
        reqwest::Body::from(vec![1u8, 2, 3]),
    );
    let next = ph
        .wait_frame(|f| f["t"] == "req" && f["args"]["p"] == "visible.jpg", 5000)
        .await;
    assert_eq!(next["args"]["s"], 320);
    // a consumer that gives up (future dropped) cancels its request
    vis.abort();
    ph.wait_frame(|f| f["t"] == "cancel" && f["rid"] == next["rid"], 5000)
        .await;
    // going offline fails everything with 503
    ph.close();
    for t in tasks {
        if let Ok(Err(e)) = t.await {
            assert!([502, 503, 504].contains(&e.status), "{e:?}");
        }
    }
    s.hub.close().await;
    let _ = std::fs::remove_dir_all(&fx.tmp);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changed_frames_relist_sub_trees() {
    let fx = tokio::task::block_in_place(|| setup("changed"));
    let s = start(fx.lib.clone()).await;
    let id = "cdcdcdcd-0000-4000-8000-000000000001";
    let ph = FakePhone::new(
        &s.base,
        id,
        vec![PhoneFolder {
            key: "k1".into(),
            name: "K".into(),
            root: fx.phone_root.clone(),
        }],
        None,
        "P",
    );
    ph.connect(None, &[]).await;
    s.hub.approve(id).unwrap();
    let fid = until(|| s.mem.device_folders(id).first().map(|f| f.id), 10_000).await;
    until(|| (s.mem.rows_of(fid).len() == 2).then_some(()), 10_000).await;
    std::fs::write(fx.phone_root.join("Trip").join("new.jpg"), b"x").unwrap();
    std::fs::remove_file(fx.phone_root.join("IMG_1.jpg")).unwrap();
    ph.clear_frames();
    ph.send(&json!({ "t": "changed", "key": "k1", "dirs": ["Trip", "Trip/Sub"] }));
    let l = ph
        .wait_frame(|f| f["t"] == "req" && f["op"] == "list", 5000)
        .await;
    assert_eq!(
        l["args"],
        json!({ "key": "k1", "dir": "Trip", "recursive": true })
    );
    until(|| s.mem.row(fid, "Trip", "new.jpg"), 10_000).await;
    // only the sub tree was re-listed: IMG_1.jpg (root) is still there
    assert!(s.mem.row(fid, "", "IMG_1.jpg").is_some());
    // an unknown key is ignored; a vanished dir (404) drops its sub tree
    ph.send(&json!({ "t": "changed", "key": "nope", "dirs": [""] }));
    std::fs::remove_dir_all(fx.phone_root.join("Trip")).unwrap();
    {
        let p2 = ph.clone();
        ph.set_intercept(Some(Box::new(move |f: &Value| {
            if f["op"] == "list" && f["args"]["dir"] == "Trip" {
                p2.err(f["rid"].as_str().unwrap(), 404, "not found");
                return true;
            }
            false
        })));
    }
    ph.send(&json!({ "t": "changed", "key": "k1", "dirs": ["Trip"] }));
    until(
        || s.mem.row(fid, "Trip", "new.jpg").is_none().then_some(()),
        10_000,
    )
    .await;
    assert!(s.mem.row(fid, "Trip", "VID_2.mp4").is_none());
    // folders frame: a new shared folder is listed, a removed one dropped
    std::fs::create_dir_all(fx.tmp.join("phone2")).unwrap();
    std::fs::write(fx.tmp.join("phone2").join("a.jpg"), b"y").unwrap();
    ph.send(&json!({ "t": "folders", "folders": [{ "key": "k2", "name": "Two" }] }));
    let rows = until(
        || {
            s.mem
                .device_folders(id)
                .into_iter()
                .find(|f| f.device_key == "k2")
                .filter(|_| s.mem.device_folders(id).len() == 1)
        },
        10_000,
    )
    .await;
    assert_eq!(rows.name, "Two");
    assert!(s.mem.rows_of(fid).is_empty());
    s.hub.close().await;
    let _ = std::fs::remove_dir_all(&fx.tmp);
}

#[test]
fn access_rules() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let s = start(std::env::temp_dir()).await;
        let hub = &s.hub;
        let h = |pairs: &[(&str, &str)]| RequestInfo { headers: pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), ..Default::default() };
        // server mode: everyone; desktop window: everyone; desktop LAN: only paired phones
        assert!(access::viewer_of(hub, RunMode::Server, &RequestInfo { lan: true, ..Default::default() }).devices);
        assert!(access::viewer_of(hub, RunMode::Desktop, &RequestInfo::default()).devices);
        let lan = RequestInfo { lan: true, ..Default::default() };
        assert!(!access::viewer_of(hub, RunMode::Desktop, &lan).devices);
        assert!(!access::can_see_pairing(RunMode::Desktop, &lan));
        assert!(access::can_see_pairing(RunMode::Server, &lan));
        assert_eq!(access::require_approver(RunMode::Desktop, &lan, None).unwrap_err().message, "Pairing can only be approved on the PC");
        // the internal token sees everything
        let mut internal = h(&[("x-peek-internal", &hub.internal_token)]);
        internal.lan = true;
        assert_eq!(access::viewer_of(hub, RunMode::Desktop, &internal), access::Viewer { devices: true, self_id: None });
        // a paired phone authenticates by headers or ?pdev=&ptok= (headers win)
        let token = "t".repeat(43);
        s.mem.save_paired("phone-123456", "P", "M", &sha256_hex(&token), "{}", 1);
        let mut by_h = h(&[("x-peek-device", "phone-123456"), ("authorization", &format!("Bearer {token}"))]);
        by_h.lan = true;
        assert_eq!(access::viewer_of(hub, RunMode::Desktop, &by_h), access::Viewer { devices: true, self_id: Some("phone-123456".into()) });
        let mut by_q = RequestInfo { lan: true, ..Default::default() };
        by_q.query = HashMap::from([("pdev".into(), "phone-123456".into()), ("ptok".into(), token.clone())]);
        assert!(access::viewer_of(hub, RunMode::Desktop, &by_q).devices);
        let mut bad_q = by_q.clone();
        bad_q.query.insert("ptok".into(), "z".repeat(43));
        assert!(!access::viewer_of(hub, RunMode::Desktop, &bad_q).devices);
        let mut mixed = by_q.clone();
        mixed.headers.insert("x-peek-device".into(), "phone-123456".into());
        assert!(!access::viewer_of(hub, RunMode::Desktop, &mixed).devices);
        assert_eq!(hub.authenticate(Some("phone-123456"), Some(&format!("Bearer {token}"))).unwrap(), "phone-123456");
        assert_eq!(hub.authenticate(None, None).unwrap_err().message, "Device token required");
        s.mem.db.lock().unwrap().devices.get_mut("phone-123456").unwrap().paired = false;
        assert_eq!(hub.authenticate(Some("phone-123456"), Some(&format!("Bearer {token}"))).unwrap_err().status, 403);
        // referenced ids
        let q = HashMap::from([("folderId".to_string(), "5".to_string()), ("ids".into(), "1,2,x".into())]);
        let (f, m) = access::referenced(&q, "/api/media/:id/file", Some("9"), Some(&json!({ "to": { "folderId": "7" }, "refs": [{ "folderId": 8 }], "items": [{ "id": "3" }], "audioId": "" })));
        assert_eq!((f, m), (vec![5, 7, 8], vec![1, 2, 9, 3]));
    });
}
