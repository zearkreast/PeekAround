//! Mirrors devices.integration.test.ts (linked phones through the real server: pairing, folder
//! rows, thumbnails / ranges / details through the tunnel, phone auth, transfers both ways, write
//! protection, own folders hidden, offline, forget) — the pa-link wiring end to end.

use crate::common::*;
use crate::phone::{sha, virtual_bytes, FakePhone, PhoneFolder};
use serde_json::{json, Value};

const FX: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/shared/fixtures/link"
);

fn fixture(n: &str) -> Vec<Value> {
    serde_json::from_str(&std::fs::read_to_string(format!("{FX}/{n}")).expect("fixture"))
        .expect("json")
}

async fn until_json(c: &reqwest::Client, url: &str, f: impl Fn(&Value) -> bool) -> Value {
    for _ in 0..250 {
        if let Ok(r) = c.get(url).send().await {
            if let Ok(v) = r.json::<Value>().await {
                if f(&v) {
                    return v;
                }
            }
        }
        sleep(40).await;
    }
    panic!("condition not met in time: {url}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn linked_phones_device_tunnel() {
    if !std::path::Path::new(FX).exists() {
        eprintln!("skipped: no link fixtures at {FX}");
        return;
    }
    let tmp = Tmp::new("dev");
    let lib = tmp.join("lib");
    let phone_root = tmp.join("phone").join("Camera");
    let photo = lib.join("photo.jpg");
    let vid = phone_root.join("Trip").join("VID_2.mp4");
    let vid_bytes = virtual_bytes(0, 4096);
    std::fs::create_dir_all(phone_root.join("Trip")).unwrap();
    std::fs::create_dir_all(phone_root.join("Empty")).unwrap();
    jpeg_dated(&phone_root.join("IMG_1.jpg"), 800, 600, "#3b82f6", None);
    std::fs::write(&vid, &vid_bytes).unwrap();
    set_mtime(&vid, 1_700_000_500_000);
    jpeg_dated(&photo, 320, 200, "#f97316", None);
    set_mtime(&photo, 1_690_000_000_000);
    let mut o = opts(&tmp, &lib, "Lib");
    o.mode = pa_core::RunMode::Desktop;
    let srv = pa_server::Server::start(o).await.unwrap();
    srv.ctx.events.set_intervals(pa_server::events::Intervals {
        add: 10,
        remove: 10,
        change: 10,
        meta: 10,
        thumb: 10,
    });
    let base = srv.url.clone();
    let c = reqwest::Client::builder().no_proxy().build().unwrap();
    let api = |p: &str| format!("{base}{p}");
    let lib_folder = s(&c
        .get(api("/api/libraries"))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()["libraries"][0]["folders"][0]["id"]);

    // pairs a new phone following the golden pairing transcript
    let pairing = fixture("pairing.json");
    let hello = pairing[0]["frame"].clone();
    let phone_id = s(&hello["id"]);
    let mut phone = FakePhone::new(
        &base,
        &phone_id,
        vec![PhoneFolder {
            key: "k1".into(),
            name: "Kamera".into(),
            root: phone_root.clone(),
        }],
        None,
        "Test Phone",
    );
    phone.st.lock().unwrap().list_body = fixture("list-and-thumb.json")[1]["body"]
        .as_str()
        .map(String::from);
    let first = phone.connect(Some(hello.clone()), &[]).await;
    let mut k1: Vec<String> = first.as_object().unwrap().keys().cloned().collect();
    let mut k2: Vec<String> = pairing[1]["frame"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    k1.sort();
    k2.sort();
    assert_eq!(k1, k2);
    assert_eq!(first["t"], "pending");
    let code = s(&first["code"]);
    assert!(code.len() == 4 && code.bytes().all(|b| b.is_ascii_digit()));
    let pending: Value = c
        .get(api("/api/devices"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(pending[0]["id"], phone_id);
    assert_eq!(pending[0]["paired"], false);
    assert_eq!(pending[0]["online"], true);
    assert_eq!(pending[0]["code"], code);
    // a device can never approve itself
    let r = c
        .post(api(&format!("/api/devices/{phone_id}/approve")))
        .header("content-type", "application/json")
        .header("x-peek-device", &phone_id)
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let r = c
        .post(api(&format!("/api/devices/{phone_id}/approve")))
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let paired = phone.wait_frame(|f| f["t"] == "paired", 10_000).await;
    let token = s(&paired["token"]);
    assert_eq!(token.len(), 43);
    let (hash, p) = srv
        .ctx
        .db
        .get(
            "SELECT token_hash, paired FROM devices WHERE id = ?",
            vec![phone_id.clone().into()],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
        )
        .unwrap()
        .unwrap();
    assert_eq!(p, 1);
    assert_eq!(
        hash,
        hex::encode(<sha2::Sha256 as sha2::Digest>::digest(token.as_bytes()))
    );
    let devs = until_json(&c, &api("/api/devices"), |d| {
        d[0]["folders"][0]["mediaCount"] == 2
    })
    .await;
    assert_eq!(devs[0]["paired"], true);
    assert_eq!(devs[0]["caps"], json!({"read": true, "receive": true}));
    let dev_folder = s(&devs[0]["folders"][0]["id"]);
    let f0 = &devs[0]["folders"][0];
    assert_eq!(f0["name"], "Kamera");
    assert_eq!(f0["libraryId"], Value::Null);
    assert_eq!(f0["path"], format!("device://{phone_id}/k1"));
    assert_eq!(
        f0["device"],
        json!({"id": phone_id, "name": "Test Phone", "online": true})
    );

    // the phone folder as a regular folder
    let libs: Value = c
        .get(api("/api/libraries"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!libs["libraries"]
        .to_string()
        .contains(&format!("\"id\":\"{dev_folder}\",\"path\":\"device")));
    assert_eq!(libs["devices"][0]["folders"][0]["id"], dev_folder.as_str());
    let root: Value = c
        .get(api(&format!("/api/browse?folderId={dev_folder}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(names(&root), vec!["IMG_1.jpg"]);
    let sub: Vec<String> = root["folders"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| s(&f["name"]))
        .collect();
    assert_eq!(sub, vec!["Empty", "Trip"]);
    assert_eq!(root["target"], Value::Null);
    let img = &items(&root)[0];
    let img_id = s(&img["id"]);
    assert_eq!(
        (
            img["width"].as_i64(),
            img["height"].as_i64(),
            img["size"].as_i64(),
            img["mtime"].as_i64(),
            img["takenAt"].as_i64()
        ),
        (
            Some(4000),
            Some(3000),
            Some(2048),
            Some(1_700_000_000_000),
            Some(1_699_999_999_000)
        )
    );
    let trip: Value = c
        .get(api(&format!("/api/browse?folderId={dev_folder}&dir=Trip")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let v = &items(&trip)[0];
    assert_eq!(
        (
            v["name"].as_str(),
            v["kind"].as_str(),
            v["duration"].as_f64()
        ),
        (Some("VID_2.mp4"), Some("video"), Some(12.5))
    );
    let vid_id = s(&v["id"]);
    let home: Value = c
        .get(api("/api/browse?recursive=1"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!items(&home).iter().any(|i| s(&i["id"]) == img_id));

    // thumbnails through the tunnel
    let t = c
        .get(api(&format!("/api/media/{img_id}/thumb?s=320")))
        .send()
        .await
        .unwrap();
    assert_eq!(t.status(), 200);
    assert!(t.headers()["content-type"]
        .to_str()
        .unwrap()
        .contains("image/webp"));
    let req = phone
        .wait_frame(
            |f| f["t"] == "req" && f["op"] == "thumb" && f["args"]["p"] == "IMG_1.jpg",
            10_000,
        )
        .await;
    assert_eq!(s(&req["rid"]).len(), 22);
    until_json(
        &c,
        &api(&format!("/api/browse?folderId={dev_folder}")),
        |b| b["items"][0]["thumbhash"].is_string(),
    )
    .await;

    // byte-exact ranges (golden file-range transcript)
    let fx = fixture("file-range.json");
    let r = c
        .get(api(&format!("/api/media/{vid_id}/file")))
        .header("range", "bytes=100-199")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 206);
    let req = phone
        .wait_frame(
            |f| f["t"] == "req" && f["op"] == "file" && f["args"]["range"] == "bytes=100-199",
            10_000,
        )
        .await;
    assert_eq!(req["args"], fx[0]["frame"]["args"]);
    assert_eq!(
        r.headers()["content-range"].to_str().unwrap(),
        fx[1]["headers"]["X-Peek-Content-Range"].as_str().unwrap()
    );
    assert_eq!(r.headers()["content-length"], "100");
    assert_eq!(r.headers()["accept-ranges"], "bytes");
    assert_eq!(r.headers()["content-type"], "video/mp4");
    assert_eq!(sha(&r.bytes().await.unwrap()), sha(&vid_bytes[100..200]));
    assert_eq!(phone.put_result(&s(&req["rid"])).await.0, 204);
    let bad = c
        .get(api(&format!("/api/media/{vid_id}/file")))
        .header("range", "bytes=5000-")
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 416);
    assert_eq!(bad.headers()["content-range"], "bytes */4096");
    let suffix = c
        .get(api(&format!("/api/media/{vid_id}/file")))
        .header("range", "bytes=-10")
        .send()
        .await
        .unwrap();
    assert_eq!(suffix.status(), 206);
    assert_eq!(&suffix.bytes().await.unwrap()[..], &vid_bytes[4086..]);
    let full = c
        .get(api(&format!("/api/media/{vid_id}/file")))
        .send()
        .await
        .unwrap();
    assert_eq!(full.status(), 200);
    assert_eq!(full.headers()["content-length"], "4096");
    assert_eq!(sha(&full.bytes().await.unwrap()), sha(&vid_bytes));
    let p2 = phone.clone();
    phone.set_intercept(Some(Box::new(move |f: &Value| {
        if f["op"] != "file" {
            return false;
        }
        p2.send(&json!({"t": "err", "rid": f["rid"], "status": 404, "message": "not found"}));
        true
    })));
    assert_eq!(
        c.get(api(&format!("/api/media/{vid_id}/file")))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    phone.set_intercept(None);
    let head = c
        .head(api(&format!("/api/media/{vid_id}/file")))
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), 200);
    assert_eq!(head.headers()["content-length"], "4096");

    // details come from the phone and enrich the row
    let d: Value = c
        .get(api(&format!("/api/media/{img_id}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(d["camera"]["make"], "Google");
    assert_eq!(d["camera"]["model"], "Pixel 8");
    assert_eq!(d["gps"], json!({"lat": 40.99, "lon": 29.03}));
    assert_eq!(d["exif"]["Make"], "Google");
    assert_eq!(d["exif"]["FNumber"], 1.7);
    assert_eq!(d["path"], "Test Phone/Kamera/IMG_1.jpg");

    // requires the device token on phone endpoints
    let rid = "A".repeat(22);
    assert_eq!(phone.put(&rid, vec![], "x".into(), vec![]).await.0, 401);
    assert_eq!(
        phone
            .put(
                &rid,
                vec![],
                "x".into(),
                vec![
                    ("Authorization".into(), format!("Bearer {}", "x".repeat(43))),
                    ("X-Peek-Device".into(), phone_id.clone())
                ]
            )
            .await
            .0,
        401
    );
    let auth = vec![
        ("Authorization".to_string(), format!("Bearer {token}")),
        ("X-Peek-Device".to_string(), phone_id.clone()),
    ];
    assert_eq!(
        phone.put(&rid, vec![], "x".into(), auth.clone()).await.0,
        410
    );
    assert_eq!(
        c.get(api("/api/devices/pull/whatever"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        c.get(api("/api/devices/pull/whatever"))
            .header("authorization", format!("Bearer {token}"))
            .header("x-peek-device", &phone_id)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );

    // copies device → PC as a transfer job (bytes + mtime preserved)
    let mv = |b: Value| {
        c.post(api("/api/fs/move"))
            .header("content-type", "application/json")
            .body(b.to_string())
            .send()
    };
    let r = mv(json!({"refs": [{"folderId": dev_folder, "rel": "Trip/VID_2.mp4"}], "to": {"folderId": lib_folder, "dir": "FromPhone"}})).await.unwrap();
    assert_eq!(r.status(), 403);
    let r: Value = mv(json!({"refs": [{"folderId": dev_folder, "rel": "Trip/VID_2.mp4"}], "to": {"folderId": lib_folder, "dir": "FromPhone"}, "copy": true})).await.unwrap().json().await.unwrap();
    assert_eq!(r["moved"], json!([]));
    assert_eq!(r["job"]["type"], "transfer");
    let fin = srv.ctx.jobs.wait(&s(&r["job"]["id"])).await.unwrap();
    let fin = serde_json::to_value(&fin).unwrap();
    assert_eq!(fin["status"], "done", "{fin}");
    assert_eq!(fin["bytes"], json!({"done": 4096, "total": 4096}));
    assert_eq!(
        fin["result"],
        json!({"folderId": lib_folder, "dir": "FromPhone", "name": "VID_2.mp4"})
    );
    let dest = lib.join("FromPhone").join("VID_2.mp4");
    assert_eq!(sha(&std::fs::read(&dest).unwrap()), sha(&vid_bytes));
    let mt = std::fs::metadata(&dest)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert!((mt - 1_700_000_500_000).abs() < 2);

    // copies PC → device with the receive flow (one-time pull URL)
    let r: Value = mv(json!({"refs": [{"folderId": lib_folder, "rel": "photo.jpg"}], "to": {"folderId": dev_folder, "dir": "From PC"}, "copy": true})).await.unwrap().json().await.unwrap();
    assert_eq!(r["job"]["type"], "transfer", "{r}");
    let fin = serde_json::to_value(srv.ctx.jobs.wait(&s(&r["job"]["id"])).await.unwrap()).unwrap();
    assert_eq!(fin["status"], "done", "{fin}");
    assert_eq!(
        fin["result"],
        json!({"folderId": dev_folder, "dir": "From PC", "name": "photo.jpg"})
    );
    let recv = phone
        .wait_frame(|f| f["t"] == "req" && f["op"] == "receive", 10_000)
        .await;
    assert_eq!(recv["args"]["mtime"], 1_690_000_000_000i64);
    assert_eq!(
        phone
            .st
            .lock()
            .unwrap()
            .last_pull_headers
            .get("x-peek-mtime")
            .map(String::as_str),
        Some("1690000000000")
    );
    let landed = phone_root.join("From PC").join("photo.jpg");
    assert_eq!(
        sha(&std::fs::read(&landed).unwrap()),
        sha(&std::fs::read(&photo).unwrap())
    );
    let pull_url = s(&recv["args"]["url"]);
    assert_eq!(
        c.get(api(&pull_url))
            .header("authorization", format!("Bearer {token}"))
            .header("x-peek-device", &phone_id)
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    until_json(
        &c,
        &api(&format!("/api/browse?folderId={dev_folder}&dir=From%20PC")),
        |b| b["items"].as_array().is_some_and(|a| a.len() == 1),
    )
    .await;

    // protects device folders from writes
    let post = |p: &str, b: Value| {
        c.post(api(p))
            .header("content-type", "application/json")
            .body(b.to_string())
            .send()
    };
    assert_eq!(
        post(
            "/api/fs/rename",
            json!({"folderId": dev_folder, "rel": "IMG_1.jpg", "name": "x.jpg"})
        )
        .await
        .unwrap()
        .status(),
        403
    );
    assert_eq!(
        post(
            "/api/fs/delete",
            json!({"refs": [{"folderId": dev_folder, "rel": "IMG_1.jpg"}]})
        )
        .await
        .unwrap()
        .status(),
        403
    );
    assert_eq!(
        post(
            "/api/fs/mkdir",
            json!({"folderId": dev_folder, "dir": "", "name": "New"})
        )
        .await
        .unwrap()
        .status(),
        403
    );
    assert_eq!(post("/api/fs/move", json!({"refs": [{"folderId": lib_folder, "rel": "photo.jpg"}], "to": {"folderId": dev_folder, "dir": ""}})).await.unwrap().status(), 403);
    assert_eq!(
        c.put(api(&format!(
            "/api/upload?folderId={dev_folder}&path=a.jpg"
        )))
        .header("content-type", "application/octet-stream")
        .body("x")
        .send()
        .await
        .unwrap()
        .status(),
        403
    );
    assert_eq!(
        c.patch(api(&format!("/api/folders/{dev_folder}")))
            .header("content-type", "application/json")
            .body(r#"{"name":"x"}"#)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        c.delete(api(&format!("/api/folders/{dev_folder}")))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        c.post(api(&format!("/api/edits/save?sourceId={img_id}")))
            .header("content-type", "application/octet-stream")
            .body(std::fs::read(&photo).unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let ve = post(
        "/api/jobs/video-edit",
        json!({"sourceId": vid_id, "output": {"format": "mp4", "height": null, "quality": "low"}}),
    )
    .await
    .unwrap()
    .status()
    .as_u16();
    assert!([403, 501].contains(&ve), "{ve}");

    // a phone never sees its own folders
    let own = |p: &str| c.get(api(p)).header("x-peek-device", &phone_id).send();
    let libs: Value = own("/api/libraries").await.unwrap().json().await.unwrap();
    assert!(!libs["devices"]
        .as_array()
        .unwrap()
        .iter()
        .any(|d| d["id"] == phone_id));
    assert_eq!(
        own(&format!("/api/browse?folderId={dev_folder}"))
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        own(&format!("/api/media/{img_id}/thumb?s=320"))
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        c.get(api(&format!("/api/browse?folderId={dev_folder}")))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );

    // rejects a second phone (4403)
    let other = FakePhone::new(
        &base,
        "aaaaaaaa-0000-4000-8000-000000000002",
        vec![PhoneFolder {
            key: "x".into(),
            name: "X".into(),
            root: phone_root.clone(),
        }],
        None,
        "Other",
    );
    assert_eq!(other.connect(None, &[]).await["t"], "pending");
    assert_eq!(
        post(
            "/api/devices/aaaaaaaa-0000-4000-8000-000000000002/reject",
            json!({})
        )
        .await
        .unwrap()
        .status(),
        200
    );
    other.wait_frame(|f| f["t"] == "rejected", 10_000).await;
    assert_eq!(other.closed().await, 4403);

    // a reconnect with the token gets welcome and replaces the older socket (4409)
    let again = FakePhone::new(
        &base,
        &phone_id,
        phone.folders.clone(),
        Some(token.clone()),
        "Test Phone",
    );
    // the same listing as before (rows — and with them the thumbnail cache keys — stay put)
    again.st.lock().unwrap().list_body = fixture("list-and-thumb.json")[1]["body"]
        .as_str()
        .map(String::from);
    assert_eq!(again.connect(None, &[]).await["t"], "welcome");
    assert_eq!(phone.closed().await, 4409);
    phone = again;
    let impostor = FakePhone::new(
        &base,
        &phone_id,
        phone.folders.clone(),
        Some("y".repeat(43)),
        "Test Phone",
    );
    assert_eq!(impostor.connect(None, &[]).await["t"], "pending");
    assert!(srv.ctx.devices.is_online(&phone_id));
    impostor.close();
    until_json(&c, &api("/api/devices"), |d| {
        d.as_array()
            .is_some_and(|a| a.iter().all(|x| x["paired"] == true))
    })
    .await;

    // offline: cached thumbnails still served, file requests 503
    phone.close();
    until_json(&c, &api("/api/devices"), |d| d[0]["online"] == false).await;
    assert_eq!(
        c.get(api(&format!("/api/media/{img_id}/thumb?s=320")))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let f = c
        .get(api(&format!("/api/media/{vid_id}/file")))
        .send()
        .await
        .unwrap();
    assert_eq!(f.status(), 503);
    assert_eq!(f.json::<Value>().await.unwrap()["error"], "device offline");
    assert_eq!(
        c.get(api(&format!("/api/media/{img_id}/thumb?s=1600")))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );

    // forget removes the device, its folders and rows
    assert_eq!(
        c.delete(api(&format!("/api/devices/{phone_id}")))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        count(
            &srv,
            &format!("SELECT count(*) FROM folders WHERE device_id = '{phone_id}'")
        ),
        0
    );
    assert_eq!(
        count(
            &srv,
            &format!("SELECT count(*) FROM media WHERE id = {img_id}")
        ),
        0
    );
    srv.close().await;
}
