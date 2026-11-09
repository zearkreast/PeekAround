//! The phone side against the real server: pairing, listing, thumbnails, ranged files, a PC →
//! phone copy (receive) and stop.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pa_phone::{LinkClient, LinkFolder, LinkPrefs, LinkStatus, NoPlatform};
use serde_json::{json, Value};

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn jpeg(p: &Path, w: u32, h: u32) {
    let img = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 120])
    }));
    let b = pa_media::encode::jpeg(&img, pa_media::encode::JpegOptions::new(85)).unwrap();
    std::fs::write(p, b).unwrap();
}

async fn until<T>(what: &str, mut f: impl AsyncFnMut() -> Option<T>) -> T {
    for _ in 0..200 {
        if let Some(v) = f().await {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out: {what}");
}

async fn get_json(c: &reqwest::Client, url: &str) -> Value {
    let b = c.get(url).send().await.unwrap().bytes().await.unwrap();
    serde_json::from_slice(&b).unwrap_or(Value::Null)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pairs_lists_serves_and_receives() {
    let tmp = Tmp(std::env::temp_dir().join(format!("pa-phone-it-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&tmp.0);
    let lib = tmp.0.join("lib");
    let cam = tmp.0.join("phone").join("Camera");
    let files = tmp.0.join("files");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::create_dir_all(cam.join("Trip")).unwrap();
    std::fs::create_dir_all(cam.join("Empty")).unwrap();
    jpeg(&cam.join("IMG_1.jpg"), 800, 600);
    let vid: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(cam.join("Trip").join("VID_2.mp4"), &vid).unwrap();
    std::fs::write(cam.join(".hidden.jpg"), b"x").unwrap();
    jpeg(&lib.join("photo.jpg"), 320, 200);

    let mut o = pa_server::ServerOptions::new(tmp.0.join("data"));
    o.port = 0;
    o.host = "127.0.0.1".into();
    o.mdns = false;
    o.mode = pa_core::RunMode::Desktop;
    o.roots = vec![pa_server::RootSpec {
        path: lib.to_string_lossy().into_owned(),
        name: Some("Lib".into()),
        split: false,
    }];
    let srv = pa_server::Server::start(o).await.unwrap();
    let base = srv.url.clone();
    let api = |p: &str| format!("{base}{p}");
    let c = reqwest::Client::builder().no_proxy().build().unwrap();

    let mut prefs = LinkPrefs {
        enabled: true,
        url: Some(base.clone()),
        device_name: Some("Test Phone".into()),
        folders: vec![LinkFolder {
            key: "k1".into(),
            name: "Kamera".into(),
            path: cam.to_string_lossy().into_owned(),
        }],
        ..Default::default()
    };
    let phone_id = prefs.ensure_device_id();
    prefs.save(&files).unwrap();
    let link = LinkClient::start(&files, Arc::new(NoPlatform), "2.0.0-test", "Test Model");

    // pairing: pending with a code on both sides, approved from the PC
    let code = until("pending", async || {
        let s = LinkStatus::load(&files);
        (s.state == "pending").then_some(s.code.unwrap_or_default())
    })
    .await;
    assert_eq!(code.len(), 4);
    let devs = get_json(&c, &api("/api/devices")).await;
    assert_eq!(devs[0]["id"], phone_id.as_str());
    assert_eq!(devs[0]["code"], code.as_str());
    assert_eq!(devs[0]["name"], "Test Phone");
    let r = c
        .post(api(&format!("/api/devices/{phone_id}/approve")))
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    until("online", async || (link.status().state == "online").then_some(())).await;
    let token = LinkPrefs::load(&files).token.expect("token saved");
    assert_eq!(token.len(), 43);

    // the listing becomes media rows of a device folder
    let devs = until("listed", async || {
        let d = get_json(&c, &api("/api/devices")).await;
        (d[0]["folders"][0]["mediaCount"] == 2).then_some(d)
    })
    .await;
    let dev_folder = devs[0]["folders"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(devs[0]["caps"], json!({"read": true, "receive": true}));
    let root = get_json(&c, &api(&format!("/api/browse?folderId={dev_folder}"))).await;
    let img = &root["items"][0];
    assert_eq!(img["name"], "IMG_1.jpg");
    assert_eq!((img["width"].as_i64(), img["height"].as_i64()), (Some(800), Some(600)));
    let subs: Vec<&str> = root["folders"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["name"].as_str())
        .collect();
    assert_eq!(subs, ["Empty", "Trip"]);
    let img_id = img["id"].as_str().unwrap().to_string();

    // thumbnail through the tunnel
    let t = c
        .get(api(&format!("/api/media/{img_id}/thumb?s=320")))
        .send()
        .await
        .unwrap();
    assert_eq!(t.status(), 200);
    let tb = t.bytes().await.unwrap();
    let ti = image::load_from_memory(&tb).expect("thumbnail image");
    assert!(ti.width() <= 320 && ti.width() > 0);

    // ranged file
    let trip = get_json(&c, &api(&format!("/api/browse?folderId={dev_folder}&dir=Trip"))).await;
    let vid_id = trip["items"][0]["id"].as_str().unwrap().to_string();
    let r = c
        .get(api(&format!("/api/media/{vid_id}/file")))
        .header("range", "bytes=100-199")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 206);
    assert_eq!(
        r.headers().get("content-range").unwrap().to_str().unwrap(),
        "bytes 100-199/4096"
    );
    assert_eq!(&r.bytes().await.unwrap()[..], &vid[100..200]);
    let r = c.get(api(&format!("/api/media/{vid_id}/file"))).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(&r.bytes().await.unwrap()[..], &vid[..]);

    // PC → phone copy: the receive op pulls the file into the shared folder
    let libs = get_json(&c, &api("/api/libraries")).await;
    let lib_folder = libs["libraries"][0]["folders"][0]["id"].as_str().unwrap().to_string();
    let r: Value = serde_json::from_slice(
        &c.post(api("/api/fs/move"))
            .header("content-type", "application/json")
            .body(
                json!({"refs": [{"folderId": lib_folder, "rel": "photo.jpg"}],
                       "to": {"folderId": dev_folder, "dir": "From PC"}, "copy": true})
                .to_string(),
            )
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap(),
    )
    .unwrap();
    let job = r["job"]["id"].as_str().expect("transfer job").to_string();
    let fin = serde_json::to_value(srv.ctx.jobs.wait(&job).await.unwrap()).unwrap();
    assert_eq!(fin["status"], "done", "{fin}");
    let landed = cam.join("From PC").join("photo.jpg");
    assert_eq!(
        std::fs::read(&landed).unwrap(),
        std::fs::read(lib.join("photo.jpg")).unwrap()
    );
    until("received file listed", async || {
        let b = get_json(&c, &api(&format!("/api/browse?folderId={dev_folder}&dir=From%20PC"))).await;
        b["items"].as_array().is_some_and(|a| a.len() == 1).then_some(())
    })
    .await;

    // stop: the PC sees the phone offline, still paired
    link.stop();
    until("off", async || (LinkStatus::load(&files).state == "off").then_some(())).await;
    let devs = until("offline", async || {
        let d = get_json(&c, &api("/api/devices")).await;
        (d[0]["online"] == false).then_some(d)
    })
    .await;
    assert_eq!(devs[0]["paired"], true);
    let _ = srv.close().await;
}
