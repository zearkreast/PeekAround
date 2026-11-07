//! Mirrors shutdown.test.ts (close() with background work in flight; device thumbnails that fill
//! in image dimensions start a new layout generation)

use crate::common::*;
use crate::phone::{FakePhone, PhoneFolder};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

async fn desktop(tmp: &Tmp) -> pa_server::Server {
    let mut o = pa_server::ServerOptions::new(tmp.join("data"));
    o.port = 0;
    o.host = "127.0.0.1".into();
    o.mode = pa_core::RunMode::Desktop;
    o.security = Some(Default::default());
    pa_server::Server::start(o).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_waits_for_device_thumbnails_and_running_jobs() {
    let tmp = Tmp::new("close");
    let root = tmp.join("phone");
    for i in 0..4 {
        jpeg_dated(&root.join(format!("IMG_{i}.jpg")), 64, 48, "#123456", None);
    }
    let srv = desktop(&tmp).await;
    let phone = FakePhone::new(
        &srv.url,
        "shutdown-phone-1",
        vec![PhoneFolder {
            key: "cam".into(),
            name: "Camera".into(),
            root: root.clone(),
        }],
        None,
        "Pixel",
    );
    // thumbnails that never settle before close(): the phone holds every thumb request
    let held = Arc::new(Mutex::new(0usize));
    let h2 = held.clone();
    phone.set_intercept(Some(Box::new(move |f: &serde_json::Value| {
        if f["op"] == "thumb" {
            *h2.lock().unwrap() += 1;
            return true;
        }
        false
    })));
    assert_eq!(phone.connect(None, &[]).await["t"], "pending");
    srv.ctx.devices.approve(&phone.id).unwrap();
    phone.wait_frame(|f| f["t"] == "paired", 10_000).await;
    for _ in 0..200 {
        if *held.lock().unwrap() > 0 {
            break;
        }
        sleep(20).await;
    }
    assert!(*held.lock().unwrap() > 0);

    // a job that ends (with a DB read) only after its signal fired
    let saw = Arc::new(AtomicBool::new(false));
    let s2 = saw.clone();
    let ctx = srv.ctx.clone();
    srv.ctx.jobs.enqueue(
        pa_core::JobType::Transfer,
        "slow",
        pa_jobs::runner(move |jc: pa_jobs::JobContext| {
            let s2 = s2.clone();
            let ctx = ctx.clone();
            async move {
                jc.cancel.cancelled().await;
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                s2.store(true, Ordering::SeqCst);
                let _ = ctx.db.int("SELECT 1", vec![]);
                Ok(None)
            }
        }),
    );
    sleep(20).await;
    tokio::time::timeout(std::time::Duration::from_secs(40), srv.close())
        .await
        .expect("close() returns");
    assert!(saw.load(Ordering::SeqCst), "close() waited for the job");
    assert!(srv.ctx.db.is_closed());
    sleep(300).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn device_thumbnails_that_fill_in_dimensions_start_a_new_generation() {
    let tmp = Tmp::new("dims");
    let root = tmp.join("phone");
    jpeg_dated(&root.join("wide.jpg"), 800, 400, "#345678", None);
    let srv = desktop(&tmp).await;
    let phone = FakePhone::new(
        &srv.url,
        "dims-phone-1",
        vec![PhoneFolder {
            key: "cam".into(),
            name: "Camera".into(),
            root,
        }],
        None,
        "Pixel",
    );
    let held: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
    let h2 = held.clone();
    phone.set_intercept(Some(Box::new(move |f: &serde_json::Value| {
        if f["op"] == "thumb" {
            h2.lock().unwrap().push(f.clone());
            return true;
        }
        false
    })));
    phone.connect(None, &[]).await;
    srv.ctx.devices.approve(&phone.id).unwrap();
    phone.wait_frame(|f| f["t"] == "paired", 10_000).await;
    for _ in 0..200 {
        if !held.lock().unwrap().is_empty() {
            break;
        }
        sleep(20).await;
    }
    let folder_id = srv
        .ctx
        .db
        .int(
            "SELECT id FROM folders WHERE device_id = ?",
            vec![phone.id.clone().into()],
        )
        .unwrap()
        .unwrap();
    let sk1 = get_ok(&srv, &format!("/api/browse/skeleton?folderId={folder_id}")).await;
    assert_eq!(sk1["total"], 1);
    // release: answer the held requests
    phone.set_intercept(None);
    let reqs: Vec<serde_json::Value> = held.lock().unwrap().drain(..).collect();
    for f in reqs {
        phone.handle(f).await;
    }
    for _ in 0..200 {
        if srv
            .ctx
            .db
            .int(
                "SELECT width FROM media WHERE folder_id = ?",
                vec![folder_id.into()],
            )
            .ok()
            .flatten()
            .is_some()
        {
            break;
        }
        sleep(20).await;
    }
    sleep(100).await;
    let sk2 = get_ok(&srv, &format!("/api/browse/skeleton?folderId={folder_id}")).await;
    let page = get_ok(&srv, &format!("/api/browse?folderId={folder_id}")).await;
    let it = &items(&page)[0];
    assert_eq!(
        it["width"].as_f64().unwrap() / it["height"].as_f64().unwrap(),
        2.0
    );
    assert_eq!(sk2["gen"], page["gen"]);
    assert_ne!(sk2["gen"], sk1["gen"]);
    use base64::Engine;
    let a1 = base64::engine::general_purpose::STANDARD
        .decode(sk1["aspects"].as_str().unwrap())
        .unwrap();
    let a2 = base64::engine::general_purpose::STANDARD
        .decode(sk2["aspects"].as_str().unwrap())
        .unwrap();
    assert_ne!(a1[0], a2[0]);
    phone.close();
    srv.close().await;
}
