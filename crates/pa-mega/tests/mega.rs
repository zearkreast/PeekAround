//! The real client against the fake MEGA gateway (mirrors the MEGA-protocol parts of
//! packages/server/test/{remote,remote-folders,mega-account}.test.ts + transfer edge cases).

mod common;

use common::{FakeAccount, FakeMega};
use pa_mega::account::{LoginRequest, LoginResult, MemoryStore, SessionStore, StoredSession};
use pa_mega::*;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const PASSWORD: &str = "correct horse battery staple Ünïcode-Pässwörd 42";
const EMAIL: &str = "peek.user@example.com";

fn data(seed: u32, n: usize) -> Vec<u8> {
    (0..n as u32)
        .map(|i| (i.wrapping_mul(7).wrapping_add(seed) & 0xff) as u8)
        .collect()
}

fn jpeg_like(n: usize) -> Vec<u8> {
    let mut v = vec![0xff, 0xd8];
    v.extend(data(3, n));
    v.extend([0xff, 0xd9]);
    v
}

fn opts(mega: &FakeMega, session: Option<Arc<dyn SessionSource>>) -> ApiOptions {
    ApiOptions {
        gateway: mega.url(),
        session,
        retry_unit: Duration::from_millis(5),
        ..Default::default()
    }
}

fn client(mega: &FakeMega) -> MegaClient {
    let mut c = MegaClient::new(opts(mega, None));
    c.read_retry_unit = Duration::from_millis(5);
    c
}

async fn read_all(
    c: &MegaClient,
    link: &MegaLink,
    node: &MegaNode,
    s: u64,
    e: u64,
) -> Result<Vec<u8>, MegaError> {
    collect(c.read(link, node, s, e, CancellationToken::new())?).await
}

#[tokio::test]
async fn lists_a_folder_link_with_file_attributes() {
    let mega = FakeMega::start().await;
    let thumb = jpeg_like(100);
    let url = mega.add_folder_link(
        "Fold1234",
        "Trip",
        &[
            (
                "FiLe0001",
                "a.jpg",
                data(1, 1000),
                vec![(0, thumb.clone()), (1, jpeg_like(500))],
            ),
            ("FiLe0002", "Sub/b.mp4", data(2, 70_000), vec![]),
            ("FiLe0003", "Sub/notes.txt", b"hello mega".to_vec(), vec![]),
        ],
    );
    let link = parse_mega_url(&url).unwrap();
    let c = client(&mega);
    let root = c.load(&link).await.unwrap();
    assert!(root.directory);
    assert_eq!(root.name, "Trip");
    assert_eq!(root.id, "Rold1234");
    assert_eq!(root.timestamp, Some(1_700_000_000_000));
    let a = root.find("FiLe0001").unwrap();
    assert_eq!((a.name.as_str(), a.size), ("a.jpg", 1000));
    assert!(a.fa.as_deref().unwrap().contains(":0*"));
    let sub = root.children.iter().find(|n| n.directory).unwrap();
    assert_eq!(sub.name, "Sub");
    assert_eq!(sub.children.len(), 2);
    // the listing request: "f" with the link id as n, anonymous
    let f: Vec<_> = mega.log().into_iter().filter(|l| l.a == "f").collect();
    assert_eq!(f.len(), 1);
    assert_eq!(
        (f[0].sid.clone(), f[0].n.clone()),
        (None, Some("Fold1234".to_string()))
    );
    assert_eq!(f[0].cmd, json!({ "a": "f", "c": 1, "ca": 1, "r": 1 }));

    let ins = inspect_tree(&root);
    let got: Vec<(&str, bool, Option<&str>)> = ins
        .nodes
        .iter()
        .map(|n| (n.path.as_str(), n.dir, n.kind))
        .collect();
    assert_eq!(
        got,
        vec![
            ("", true, None),
            ("Sub", true, None),
            ("a.jpg", false, Some("image")),
            ("Sub/b.mp4", false, Some("video")),
            ("Sub/notes.txt", false, None)
        ]
    );
    assert_eq!(ins.size, 1000 + 70_000 + 10);

    // file attributes: 120 px thumbnail + preview (ufa → POST <p>/<type>)
    let cancel = CancellationToken::new();
    assert_eq!(c.attr(&link, a, 0, &cancel).await.unwrap().unwrap(), thumb);
    assert_eq!(
        c.attr(&link, a, 1, &cancel).await.unwrap().unwrap(),
        jpeg_like(500)
    );
    assert_eq!(
        c.attr(&link, root.find("FiLe0002").unwrap(), 0, &cancel)
            .await
            .unwrap(),
        None
    );
    let ufa: Vec<_> = mega.log().into_iter().filter(|l| l.a == "ufa").collect();
    assert_eq!(ufa.len(), 2);
    assert!(ufa
        .iter()
        .all(|l| l.n.as_deref() == Some("Fold1234") && l.cmd["ssl"] == 2 && l.cmd["r"] == 1));

    // a sub node of the folder link, and the legacy #F! form with a node
    let sub_link = parse_mega_url(&format!("{url}/file/FiLe0002")).unwrap();
    assert_eq!(c.load(&sub_link).await.unwrap().name, "b.mp4");
    let missing = parse_mega_url(&format!("{url}/folder/Zz9_-xYw")).unwrap();
    let e = c.load(&missing).await.unwrap_err();
    assert_eq!(http_error(&e).status, 404);

    // wrong key / unknown link
    let bad_key = parse_mega_url("https://mega.nz/folder/Fold1234#AAAAAAAAAAAAAAAAAAAAAA").unwrap();
    assert_eq!(
        http_error(&c.load(&bad_key).await.unwrap_err()).message,
        "Invalid MEGA link key"
    );
    let gone = parse_mega_url("https://mega.nz/folder/Gone1234#AAAAAAAAAAAAAAAAAAAAAA").unwrap();
    assert_eq!(
        http_error(&c.load(&gone).await.unwrap_err()).message,
        "MEGA link not found (removed, or the link is wrong)"
    );
}

#[tokio::test]
async fn ranged_reads_downloads_and_url_cache() {
    let mega = FakeMega::start().await;
    let body = data(9, 300_017);
    let url = mega.add_file_link("OneF1234", "single.jpg", &body, &[]);
    let link = parse_mega_url(&url).unwrap();
    let c = client(&mega);
    let node = c.load(&link).await.unwrap();
    assert_eq!(
        (
            node.name.as_str(),
            node.size,
            node.directory,
            node.timestamp
        ),
        ("single.jpg", 300_017, false, None)
    );
    assert_eq!(mega.log()[0].cmd, json!({ "a": "g", "p": "OneF1234" }));
    for (s, e) in [
        (0u64, 300_016u64),
        (3, 20),
        (16, 16),
        (17, 31),
        (15, 16),
        (131_070, 131_090),
        (299_999, 400_000),
        (100_000, 100_000),
    ] {
        let got = read_all(&c, &link, &node, s, e).await.unwrap();
        let e2 = e.min(300_016);
        assert_eq!(got, &body[s as usize..=e2 as usize], "{s}-{e}");
    }
    // the ciphertext requests start at the 16-byte aligned offset
    let dls = mega.with(|i| i.dl.clone());
    assert!(dls.iter().any(|(_, r)| r == "0-20"), "{dls:?}");
    assert!(dls.iter().any(|(_, r)| r == "131056-131090"));
    // one "g" for all of them (download URL cached)
    let g_count = || {
        mega.log()
            .iter()
            .filter(|l| l.a == "g" && l.cmd["g"] == 1)
            .count()
    };
    assert_eq!(g_count(), 1);
    c.clear_download_urls();
    read_all(&c, &link, &node, 0, 10).await.unwrap();
    assert_eq!(g_count(), 2);
    // 416
    let e = c
        .read(&link, &node, 400_000, 500_000, CancellationToken::new())
        .err()
        .unwrap();
    assert_eq!(http_error(&e).status, 416);

    // whole download, MAC verified
    let all = collect(c.download(&link, &node, CancellationToken::new()).unwrap())
        .await
        .unwrap();
    assert_eq!(all, body);
    // corrupted ciphertext → MAC failure
    mega.with(|i| i.faults.corrupt = Some("OneF1234".into()));
    let e = collect(c.download(&link, &node, CancellationToken::new()).unwrap())
        .await
        .unwrap_err();
    assert_eq!(e, MegaError::Mac);
    mega.with(|i| i.faults.corrupt = None);

    // an expired download URL (403) is replaced by a fresh one
    mega.with(|i| i.faults.dl_403 = 1);
    let before = g_count();
    assert_eq!(
        read_all(&c, &link, &node, 5, 50).await.unwrap(),
        &body[5..=50]
    );
    assert_eq!(g_count(), before + 1);

    // a transfer cut in the middle resumes where it stopped (whole download)
    mega.with(|i| i.faults.dl_cut = Some((2, 70_000)));
    let dl_before = mega.with(|i| i.dl.len());
    let all = collect(c.download(&link, &node, CancellationToken::new()).unwrap())
        .await
        .unwrap();
    assert_eq!(all, body);
    // two cut requests + the resumes: the later ones start where the data stopped (aligned)
    let resumed: Vec<String> =
        mega.with(|i| i.dl[dl_before..].iter().map(|d| d.1.clone()).collect());
    assert_eq!(resumed.len(), 3, "{resumed:?}");
    assert_eq!(resumed[0], "0-300016");
    assert_ne!(resumed[1], "0-300016");
    // ... a ranged read reports it
    mega.with(|i| i.faults.dl_cut = Some((1, 1000)));
    let r = read_all(&c, &link, &node, 0, 200_000).await;
    assert!(matches!(r, Err(MegaError::Network(_))), "{r:?}");
    mega.with(|i| i.faults.dl_cut = None);

    // canceling stops the stream
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        collect(c.read(&link, &node, 0, 1000, cancel).unwrap())
            .await
            .unwrap_err(),
        MegaError::Aborted
    );
}

#[tokio::test]
async fn quota_rate_limit_and_hashcash() {
    let mega = FakeMega::start().await;
    let body = data(4, 5000);
    let url = mega.add_file_link("Quota123", "q.mp4", &body, &[]);
    let link = parse_mega_url(&url).unwrap();
    let c = client(&mega);
    // EAGAIN twice → retried
    mega.with(|i| i.faults.eagain = 2);
    let node = c.load(&link).await.unwrap();
    assert_eq!(mega.log().len(), 1);
    // EAGAIN 5 times → gives up after 4 retries
    mega.with(|i| i.faults.eagain = 5);
    let e = c.load(&link).await.unwrap_err();
    assert_eq!(e, MegaError::Api(-3));
    assert_eq!(http_error(&e).status, 503);
    mega.with(|i| i.faults.eagain = 0);

    // "g" over quota
    mega.with(|i| i.faults.quota_tl = Some(600));
    let e = collect(
        c.read(&link, &node, 0, 10, CancellationToken::new())
            .unwrap(),
    )
    .await
    .unwrap_err();
    assert_eq!(e, MegaError::Quota { seconds: Some(600) });
    let h = http_error(&e);
    assert_eq!(
        (h.status, h.message.as_str(), h.retry_after),
        (
            429,
            "MEGA bandwidth quota exceeded, try again later (retry in about 10 min)",
            Some(600)
        )
    );
    mega.with(|i| i.faults.quota_tl = None);
    // 509 on the transfer
    mega.with(|i| i.faults.dl_509 = Some(1800));
    let e = collect(
        c.read(&link, &node, 0, 10, CancellationToken::new())
            .unwrap(),
    )
    .await
    .unwrap_err();
    assert_eq!(e.time_limit(), Some(1800));
    let gate = TransferGate::default();
    assert!(gate.note_error(&e).is_some());
    assert_eq!(gate.check_quota().unwrap_err().status, 429);
    mega.with(|i| i.faults.dl_509 = None);

    // hashcash challenge answered
    mega.with(|i| i.faults.hashcash = true);
    c.load(&link).await.unwrap();
    assert_eq!(mega.with(|i| i.hashcash_ok), 1);
}

struct CountingStore {
    inner: MemoryStore,
    saves: AtomicUsize,
}

impl SessionStore for CountingStore {
    fn load(&self) -> StoredSession {
        self.inner.load()
    }
    fn save(&self, s: &StoredSession) {
        self.saves.fetch_add(1, Ordering::SeqCst);
        self.inner.save(s)
    }
}

fn login_req(email: &str, password: &str, code: Option<&str>) -> LoginRequest {
    LoginRequest {
        email: Some(json!(email)),
        password: Some(json!(password)),
        code: code.map(|c| json!(c)),
    }
}

#[tokio::test]
async fn account_login_session_expiry_and_logout() {
    let mega = FakeMega::start().await;
    mega.add_account(FakeAccount {
        email: EMAIL.into(),
        password: PASSWORD.into(),
        mfa: None,
        utype: 1,
        mxfer: Some(2 * 1024u64.pow(4)),
        caxfer: Some(5 * 1024u64.pow(3)),
    });
    mega.add_account(FakeAccount {
        email: "twofa@example.com".into(),
        password: PASSWORD.into(),
        mfa: Some("123456".into()),
        utype: 0,
        mxfer: None,
        caxfer: None,
    });
    let folder_url = mega.add_folder_link(
        "Fold1234",
        "Trip",
        &[("FiLe0001", "a.jpg", data(1, 4000), vec![])],
    );
    let store = Arc::new(CountingStore {
        inner: MemoryStore::default(),
        saves: AtomicUsize::new(0),
    });
    let changes = Arc::new(AtomicUsize::new(0));
    let ch = changes.clone();
    let account = MegaAccount::new(
        store.clone(),
        MegaAuth::new(opts(&mega, None)),
        Box::new(move || {
            ch.fetch_add(1, Ordering::SeqCst);
        }),
    );
    let session: Arc<dyn SessionSource> = account.clone();
    let mut c = MegaClient::new(opts(&mega, Some(session)));
    c.read_retry_unit = Duration::from_millis(5);
    let link = parse_mega_url(&folder_url).unwrap();

    // anonymous: link requests carry no sid
    assert!(!account.status().logged_in);
    let root = c.load(&link).await.unwrap();
    assert!(mega
        .log()
        .iter()
        .filter(|l| l.a == "f")
        .all(|l| l.sid.is_none() && l.n.as_deref() == Some("Fold1234")));

    // refusals
    let r = |x: Result<LoginResult, HttpError>| serde_json::to_value(x.unwrap()).unwrap();
    assert_eq!(
        r(account.login(&login_req(EMAIL, "nope", None)).await),
        json!({ "ok": false, "reason": "credentials" })
    );
    assert_eq!(
        r(account
            .login(&login_req("twofa@example.com", PASSWORD, None))
            .await),
        json!({ "ok": false, "reason": "mfa-required" })
    );
    assert_eq!(
        r(account
            .login(&login_req("twofa@example.com", PASSWORD, Some("000000")))
            .await),
        json!({ "ok": false, "reason": "mfa-invalid" })
    );
    assert_eq!(
        account
            .login(&login_req("not-an-email", PASSWORD, None))
            .await
            .unwrap_err()
            .status,
        400
    );
    assert_eq!(
        account
            .login(&login_req(EMAIL, PASSWORD, Some("12")))
            .await
            .unwrap_err()
            .status,
        400
    );
    assert_eq!(store.load().sid, None);
    let us: Vec<_> = mega.log().into_iter().filter(|l| l.a == "us").collect();
    assert!(us.len() >= 3);
    assert!(us.iter().any(|l| l.cmd["mfa"] == "000000"));
    assert!(!format!("{:?}", mega.log()).contains(PASSWORD));

    // 2FA login, then the main account (upper-case e-mail): the 2FA session is ended
    let tf = account
        .login(&login_req("twofa@example.com", PASSWORD, Some("123 456")))
        .await
        .unwrap();
    assert!(matches!(tf, LoginResult::Ok { .. }));
    let tf_sid = store.load().sid.unwrap();
    let res = account
        .login(&login_req(&EMAIL.to_uppercase(), PASSWORD, None))
        .await
        .unwrap();
    let LoginResult::Ok { account: st, .. } = res else {
        panic!("{res:?}")
    };
    assert_eq!(
        serde_json::to_value(&st).unwrap()["transfer"],
        json!({ "used": 5.0 * 1024f64.powi(3), "total": 2.0 * 1024f64.powi(4) })
    );
    assert!(
        st.logged_in
            && st.email.as_deref() == Some(EMAIL)
            && st.kind == Some("pro")
            && st.plan == Some("Pro I")
            && !st.expired
    );
    let s = store.load();
    assert_eq!(
        (
            s.sid.is_some(),
            s.email.as_deref(),
            s.since.is_some(),
            s.expired
        ),
        (true, Some(EMAIL), true, None)
    );
    let sid = s.sid.unwrap();
    assert!(mega.with(|i| i.sessions.contains(&sid)));
    for _ in 0..100 {
        if mega
            .log()
            .iter()
            .any(|l| l.a == "sml" && l.sid.as_deref() == Some(tf_sid.as_str()))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(mega
        .log()
        .iter()
        .any(|l| l.a == "sml" && l.sid.as_deref() == Some(tf_sid.as_str())));
    let with: Vec<String> = mega.with_sid(&sid).into_iter().map(|l| l.a).collect();
    assert!(with.contains(&"ug".to_string()) && with.contains(&"uq".to_string()));
    assert!(changes.load(Ordering::SeqCst) >= 2);

    // the sid rides on listing and download requests
    c.load(&link).await.unwrap();
    assert_eq!(
        mega.log()
            .iter()
            .rev()
            .find(|l| l.a == "f")
            .unwrap()
            .sid
            .as_deref(),
        Some(sid.as_str())
    );
    let a = root.find("FiLe0001").unwrap();
    let at = mega.log().len();
    assert_eq!(
        read_all(&c, &link, a, 0, 3999).await.unwrap(),
        data(1, 4000)
    );
    let g: Vec<_> = mega.log()[at..]
        .iter()
        .filter(|l| l.a == "g")
        .cloned()
        .collect();
    assert!(!g.is_empty());
    assert!(g.iter().all(|l| l.sid.as_deref() == Some(sid.as_str())
        && l.n.as_deref() == Some("Fold1234")
        && l.cmd["g"] == 1
        && l.cmd["n"] == "FiLe0001"));

    // expired session: dropped, shown as expired, the request is retried anonymously
    mega.with(|i| i.sessions.remove(&sid));
    let at = mega.log().len();
    c.load(&link).await.unwrap();
    let f: Vec<Option<String>> = mega.log()[at..]
        .iter()
        .filter(|l| l.a == "f")
        .map(|l| l.sid.clone())
        .collect();
    assert_eq!(f, vec![Some(sid.clone()), None]);
    assert_eq!(store.load().sid, None);
    let st = account.status();
    assert!(!st.logged_in && st.expired && st.email.as_deref() == Some(EMAIL));
    let at = mega.log().len();
    c.load(&link).await.unwrap();
    assert!(mega.log()[at..].iter().all(|l| l.sid.is_none()));

    // a session MEGA rejects on the status check is dropped too
    assert!(matches!(
        account
            .login(&login_req(EMAIL, PASSWORD, None))
            .await
            .unwrap(),
        LoginResult::Ok { .. }
    ));
    mega.with(|i| i.faults.reject_sessions = true);
    let st = account.refreshed_status(true).await;
    assert!(!st.logged_in && st.expired);
    mega.with(|i| i.faults.reject_sessions = false);

    // logout: "sml" with the session, then requests are anonymous
    assert!(matches!(
        account
            .login(&login_req(EMAIL, PASSWORD, None))
            .await
            .unwrap(),
        LoginResult::Ok { .. }
    ));
    let sid = store.load().sid.unwrap();
    let st = account.logout().await;
    assert!(!st.logged_in && st.email.is_none() && !st.expired);
    assert_eq!(
        mega.log()
            .iter()
            .rev()
            .find(|l| l.a == "sml")
            .unwrap()
            .sid
            .as_deref(),
        Some(sid.as_str())
    );
    assert!(!mega.with(|i| i.sessions.contains(&sid)));
    assert_eq!(store.load(), StoredSession::default());
    let at = mega.log().len();
    c.load(&link).await.unwrap();
    assert!(mega.log()[at..].iter().all(|l| l.sid.is_none()));
    // nothing ever sent the password
    assert!(!format!("{:?}", mega.log()).contains(PASSWORD));
}

#[tokio::test]
async fn lanes_hold_transfers_per_link() {
    let mega = FakeMega::start().await;
    let url = mega.add_file_link("Lane1234", "l.bin", &data(5, 1000), &[]);
    let link = parse_mega_url(&url).unwrap();
    let c = client(&mega);
    let node = c.load(&link).await.unwrap();
    let gate = TransferGate::default();
    let cancel = CancellationToken::new();
    let mut held = Vec::new();
    for _ in 0..4 {
        held.push(gate.slot(&link.id, Priority::High, &cancel).await.unwrap());
    }
    assert_eq!(gate.lanes(&link.id).stats(), (4, 0));
    let g2 = gate.clone();
    let id = link.id.clone();
    let waiter = tokio::spawn(async move {
        g2.slot(&id, Priority::Low, &CancellationToken::new())
            .await
            .map(|_| ())
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(gate.lanes(&link.id).stats(), (4, 1));
    drop(held.pop());
    waiter.await.unwrap().unwrap();
    // another link has its own lanes
    let _other = gate
        .slot("Other123", Priority::High, &cancel)
        .await
        .unwrap();
    // a read inside a slot
    let _s = gate.slot(&link.id, Priority::High, &cancel).await.unwrap();
    assert_eq!(
        read_all(&c, &link, &node, 0, 999).await.unwrap(),
        data(5, 1000)
    );
}

#[tokio::test]
async fn import_download_to_part_file() {
    let mega = FakeMega::start().await;
    let body = data(6, 150_000);
    let url = mega.add_folder_link(
        "Imp12345",
        "Imp",
        &[("FiLe0009", "x.jpg", body.clone(), vec![])],
    );
    let link = parse_mega_url(&url).unwrap();
    let c = client(&mega);
    let root = c.load(&link).await.unwrap();
    let node = root.find("FiLe0009").unwrap();
    let dir = std::env::temp_dir().join(format!("pa-mega-imp-{}", std::process::id()));
    let mut n = 0u64;
    let mut on = |b: u64| n += b;
    let part = pa_mega::import::download_to_part(
        &c,
        &link,
        node,
        &dir,
        "x.jpg",
        CancellationToken::new(),
        &mut on,
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(&part).unwrap(), body);
    assert_eq!(n, 150_000);
    let mtime = std::fs::metadata(&part)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    assert_eq!(mtime, 1_700_000_000_000);
    // a failing download leaves no .part behind
    mega.with(|i| i.faults.corrupt = Some("FiLe0009".into()));
    let mut on = |_b: u64| {};
    let e = pa_mega::import::download_to_part(
        &c,
        &link,
        node,
        &dir,
        "y.jpg",
        CancellationToken::new(),
        &mut on,
    )
    .await
    .unwrap_err();
    assert_eq!(e, MegaError::Mac);
    let left: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with("y.jpg"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
