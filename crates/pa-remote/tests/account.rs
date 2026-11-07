//! Server-level parts of packages/server/test/mega-account.test.ts: the account session (pa-mega's
//! MegaAccount, wired by `RemoteAccount`) on every link request of the remote folders — listing,
//! streaming, import jobs —, expiry fallback to anonymous, logout, and the `changed()` hook
//! (download URLs cleared, quota pause lifted, `megaAccount` event). The login protocol itself is
//! tested in pa-mega.

mod common;

use common::fake::{FakeAccount, FakeMega};
use common::*;
use pa_core::{JobStatus, ServerEvent};
use pa_mega::account::MemoryStore;
use pa_mega::{ApiOptions, LoginRequest, SessionStore};
use pa_remote::{routes, RemoteAccount};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

const EMAIL: &str = "user@example.com";
const PASSWORD: &str = "correct horse battery staple";

fn mega_events(ev: &pa_jobs::MemoryEvents) -> usize {
    ev.snapshot()
        .iter()
        .filter(|e| matches!(e, ServerEvent::MegaAccount))
        .count()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn account_session_on_link_requests() {
    let fake = FakeMega::start().await;
    fake.add_account(FakeAccount {
        email: EMAIL.into(),
        password: PASSWORD.into(),
        mfa: None,
        utype: 1,
        mxfer: Some(2 * 1024u64.pow(4)),
        caxfer: Some(5 * 1024u64.pow(3)),
    });
    let jpg = fixture_photo();
    let folder_link = fake.add_folder_link(
        "Fold1234",
        "Trip",
        &[
            ("FiLe0001", "a.jpg", jpg.clone(), vec![]),
            ("FiLe0002", "notes.txt", b"hello mega".to_vec(), vec![]),
        ],
    );
    let file_link = fake.add_file_link("OneF1234", "single.jpg", &jpg, &[]);
    let sessions = Arc::new(MemoryStore::default());
    let acct = RemoteAccount::new(
        sessions.clone(),
        ApiOptions {
            gateway: fake.url(),
            retry_unit: Duration::from_millis(5),
            ..Default::default()
        },
    );
    let env = start("account", Arc::new(acct.client.clone())).await;
    acct.bind(&env.svc);
    let (lib, svc) = (env.lib.clone(), env.svc.clone());
    let root = env.tmp.join("lib");
    std::fs::create_dir_all(root.join("Local")).unwrap();
    let local = lib.add_local("Lib", &root).to_string();
    let last_f = || fake.log().into_iter().rev().find(|l| l.a == "f").unwrap();

    // ── starts anonymous: link requests carry no sid
    let created = svc.create(&json!({ "url": folder_link })).await.unwrap();
    let fid: i64 = created.folder.id.parse().unwrap();
    assert_eq!(last_f().sid, None);
    // the background passes (thumbnail, metadata) run anonymously first
    until(
        || {
            lib.rows_of(fid)
                .iter()
                .all(|r| r.thumb_state != 0 && r.meta_state != 3)
                .then_some(())
        },
        20_000,
    )
    .await;
    assert!(!acct.account.status().logged_in);

    // ── login (only the sid is stored) lifts the quota pause and emits megaAccount
    svc.set_quota_until(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            + 3_600_000,
    );
    lib.db
        .lock()
        .unwrap()
        .settings
        .insert("remoteQuota".into(), json!({ "until": 1 }));
    let before_ev = mega_events(&env.events);
    let r = acct
        .account
        .login(&LoginRequest {
            email: Some(json!(EMAIL)),
            password: Some(json!(PASSWORD)),
            code: None,
        })
        .await
        .unwrap();
    assert!(serde_json::to_value(&r).unwrap()["ok"].as_bool().unwrap());
    let stored = sessions.load();
    let sid = stored.sid.clone().unwrap();
    assert!(!serde_json::to_string(&stored).unwrap().contains(PASSWORD));
    assert_eq!(svc.quota_until(), 0);
    assert_eq!(lib.db.lock().unwrap().settings["remoteQuota"], json!({}));
    assert!(mega_events(&env.events) > before_ev);

    // ── attaches the sid to listing and download requests (streaming + import job)
    routes::refresh_folder(&svc, &*lib, &fid.to_string())
        .await
        .unwrap();
    let f = last_f();
    assert_eq!(
        (f.sid.as_deref(), f.n.as_deref()),
        (Some(sid.as_str()), Some("Fold1234"))
    );
    // (the background thumbnail pass may have cached the original while anonymous)
    svc.cache.clear().await;
    let at = fake.log().len();
    let row = lib.row(fid, "a.jpg");
    let res = reqwest::get(format!("{}/api/media/{}/file", env.base, row.id))
        .await
        .unwrap();
    let status = res.status();
    let body = res.bytes().await.unwrap().to_vec();
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    assert_eq!(body, jpg);
    let g: Vec<_> = fake.log()[at..]
        .iter()
        .filter(|l| l.a == "g")
        .cloned()
        .collect();
    assert!(!g.is_empty());
    assert!(
        g.iter().all(|l| l.sid.as_deref() == Some(sid.as_str())
            && l.n.as_deref() == Some("Fold1234")
            && l.cmd["g"] == 1
            && l.cmd["n"] == "FiLe0001"),
        "{g:?}"
    );
    // import job: file link → local folder
    let at = fake.log().len();
    let job = routes::import(
        &svc,
        &json!({ "url": file_link, "mode": "extract", "folderId": local, "dir": "Local" }),
    )
    .unwrap();
    let done = env.jobs.wait(&job.id).await.unwrap();
    assert_eq!(done.status, JobStatus::Done, "{:?}", done.error);
    assert_eq!(std::fs::read(root.join("Local/single.jpg")).unwrap(), jpg);
    let imp: Vec<_> = fake.log()[at..]
        .iter()
        .filter(|l| l.a == "g")
        .cloned()
        .collect();
    assert!(imp
        .iter()
        .any(|l| l.cmd["p"] == "OneF1234" && l.cmd.get("g").is_none()));
    assert!(imp
        .iter()
        .any(|l| l.cmd["p"] == "OneF1234" && l.cmd["g"] == 1));
    assert!(imp.iter().all(|l| l.sid.as_deref() == Some(sid.as_str())));

    // ── an expired session: dropped, shown as expired, requests fall back to anonymous
    fake.with(|i| i.sessions.remove(&sid));
    let n_ev = mega_events(&env.events);
    let at = fake.log().len();
    routes::refresh_folder(&svc, &*lib, &fid.to_string())
        .await
        .unwrap();
    let fs: Vec<Option<String>> = fake.log()[at..]
        .iter()
        .filter(|l| l.a == "f")
        .map(|l| l.sid.clone())
        .collect();
    assert_eq!(fs, vec![Some(sid.clone()), None]); // ESID → retried without the session
    assert!(sessions.load().sid.is_none());
    let st = acct.account.refreshed_status(false).await;
    assert!(!st.logged_in && st.expired);
    assert_eq!(st.email.as_deref(), Some(EMAIL));
    assert!(mega_events(&env.events) > n_ev);
    let at = fake.log().len();
    routes::refresh_folder(&svc, &*lib, &fid.to_string())
        .await
        .unwrap();
    let after: Vec<(String, Option<String>)> = fake.log()[at..]
        .iter()
        .map(|l| (l.a.clone(), l.sid.clone()))
        .collect();
    assert_eq!(after, vec![("f".to_string(), None)]);

    // ── logout: "sml" with the session, then the sid is gone and requests are anonymous
    let r = acct
        .account
        .login(&LoginRequest {
            email: Some(json!(EMAIL)),
            password: Some(json!(PASSWORD)),
            code: None,
        })
        .await
        .unwrap();
    assert!(serde_json::to_value(&r).unwrap()["ok"].as_bool().unwrap());
    let sid = sessions.load().sid.unwrap();
    let st = acct.account.logout().await;
    assert!(!st.logged_in && !st.expired && st.email.is_none());
    assert_eq!(
        fake.log()
            .into_iter()
            .rev()
            .find(|l| l.a == "sml")
            .unwrap()
            .sid
            .as_deref(),
        Some(sid.as_str())
    );
    assert!(!fake.with(|i| i.sessions.contains(&sid)));
    assert_eq!(sessions.load(), Default::default());
    let at = fake.log().len();
    routes::refresh_folder(&svc, &*lib, &fid.to_string())
        .await
        .unwrap();
    assert!(fake.log()[at..].iter().all(|l| l.sid.is_none()));

    svc.close().await;
    let _ = std::fs::remove_dir_all(&env.tmp);
}
