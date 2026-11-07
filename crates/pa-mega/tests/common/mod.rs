//! A fake MEGA API gateway (Rust port of packages/server/test/helpers/fake-mega.ts, extended):
//! served over HTTP on loopback so the REAL client runs against it.
//!  - "cs" commands: us0 / us (v2 login: PBKDF2 → uh, RSA-encrypted csid, AES-wrapped master + private
//!    key), ug, uq, sml, f (folder link listing with `fa`), g (attributes / download URLs), ufa;
//!  - "dl/<handle>/<start>-<end>": ciphertext ranges (MEGA uploader encryption → real MAC, real CTR);
//!  - "fa/<type>": file attribute POSTs;
//!  - failure injection: rejected sessions, EAGAIN, quota, expired URLs, cut transfers, hashcash.
//!
//! Every cs request is logged with its query (`sid`, `n`).
#![allow(dead_code, clippy::type_complexity)]

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use num_bigint::BigUint;
use pa_mega::crypto::{self, b64url, from_b64};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// 2048-bit test key generated once with `openssl genrsa`
const P: &str = "fad25043f9543aa341e11ed7ce01af85d280ef1595388393ff11f12bf6b4a8ff2da25ede545b6d3bcd423aa9332d1bca7a4c3b51383e95adbd72038bce746105011abbc9d0aaab9051594a36e5e05c80b38a864c04b2bc60bfe581ae5ddc345cecddf18ea1ef672b97cea94b5daa0edde2294efc696e1efce17ffdca7478188f";
const Q: &str = "cf082eec76f7134fd761f861c021187b8adad4ef4f2732ac1b270602ce728380302f15c6ba75d7bc4969470498239acdf72dfc048da818809b8d0b07d631ff29bbbe5f9287fffb9609a3cdeb6be5292e486027b1a3d7fa0c94efd82aeacc34ae6ebd5f690cd9d5b8689a21c5db209fcae0df5efcc88531404ce5e98b486875d7";

#[derive(Clone, Debug)]
pub struct FakeAccount {
    pub email: String,
    pub password: String,
    pub mfa: Option<String>,
    pub utype: u32,
    pub mxfer: Option<u64>,
    pub caxfer: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct CsLog {
    pub a: String,
    pub sid: Option<String>,
    pub n: Option<String>,
    pub cmd: Value,
}

#[derive(Clone)]
pub struct FakeNode {
    pub h: String,
    pub name: String,
    pub key: Vec<u8>,
    pub cipher: Vec<u8>,
    pub size: u64,
    pub fa: Option<String>,
    pub dir: bool,
    pub parent: Option<String>,
}

#[derive(Default)]
pub struct Faults {
    /// answer every request that carries a sid with -15
    pub reject_sessions: bool,
    /// the next n cs requests answer -3
    pub eagain: u32,
    /// "g" with g=1 answers { e: -17, tl }
    pub quota_tl: Option<u64>,
    /// dl answers 509 with x-mega-time-left
    pub dl_509: Option<u64>,
    /// the next n dl requests answer 403 (expired URL)
    pub dl_403: u32,
    /// the next n dl requests send only this many bytes, then cut the connection
    pub dl_cut: Option<(u32, usize)>,
    /// flip a byte of the ciphertext of this handle (MAC failure)
    pub corrupt: Option<String>,
    /// the next cs request answers with an X-Hashcash challenge
    pub hashcash: bool,
}

#[derive(Default)]
pub struct Inner {
    pub log: Vec<CsLog>,
    pub dl: Vec<(String, String)>,
    pub fa_posts: Vec<String>,
    pub sessions: HashSet<String>,
    pub accounts: Vec<FakeAccount>,
    pub files: HashMap<String, FakeNode>,
    pub file_links: HashMap<String, String>,
    /// folder link id → (folder key, root handle, name, node handles)
    pub folders: HashMap<String, (Vec<u8>, String, String, Vec<String>)>,
    pub attrs: HashMap<String, Vec<u8>>,
    pub faults: Faults,
    pub hashcash_ok: u32,
    pub uh_cache: HashMap<String, ([u8; 16], String)>,
}

pub struct FakeMega {
    pub inner: Arc<Mutex<Inner>>,
    pub base: String,
    salt: Vec<u8>,
    master: Vec<u8>,
    n: BigUint,
    e: BigUint,
    privk: Vec<u8>,
}

#[derive(Clone)]
struct Srv {
    inner: Arc<Mutex<Inner>>,
    base: String,
    salt: Vec<u8>,
    master: Vec<u8>,
    n: BigUint,
    e: BigUint,
    privk: Vec<u8>,
}

impl FakeMega {
    pub async fn start() -> FakeMega {
        let p = BigUint::parse_bytes(P.as_bytes(), 16).unwrap();
        let q = BigUint::parse_bytes(Q.as_bytes(), 16).unwrap();
        let e = BigUint::from(65537u32);
        let one = BigUint::from(1u8);
        let phi = (&p - &one) * (&q - &one);
        let d = e.modinv(&phi).unwrap();
        let u = p.modinv(&q).unwrap();
        let mut privk = Vec::new();
        for x in [&p, &q, &d, &u] {
            privk.extend(crypto::write_mpi(x));
        }
        privk.resize(privk.len().div_ceil(16) * 16, 0);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}/");
        let inner = Arc::new(Mutex::new(Inner::default()));
        let salt: Vec<u8> = (0..16).map(|_| rand::random()).collect();
        let master: Vec<u8> = (0..16).map(|_| rand::random()).collect();
        let srv = Srv {
            inner: inner.clone(),
            base: base.clone(),
            salt: salt.clone(),
            master: master.clone(),
            n: &p * &q,
            e: e.clone(),
            privk: privk.clone(),
        };
        let app = Router::new()
            .route("/cs", post(cs))
            .route("/dl/{h}/{range}", axum::routing::get(dl))
            .route("/fa/{t}", post(fa))
            .route("/fa", post(fa_root))
            .with_state(srv);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        FakeMega {
            inner,
            base,
            salt,
            master,
            n: &p * &q,
            e,
            privk,
        }
    }

    pub fn url(&self) -> String {
        self.base.clone()
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut Inner) -> R) -> R {
        f(&mut self.inner.lock().unwrap())
    }

    pub fn log(&self) -> Vec<CsLog> {
        self.inner.lock().unwrap().log.clone()
    }

    pub fn add_account(&self, a: FakeAccount) {
        self.with(|i| i.accounts.push(a));
    }

    fn make_node(
        h: &str,
        name: &str,
        data: &[u8],
        thumbs: &[(u32, Vec<u8>)],
        inner: &mut Inner,
    ) -> FakeNode {
        let (cipher, key) = crypto::mega_encrypt(data, None);
        let mut fa = Vec::new();
        for (t, plain) in thumbs {
            let handle: [u8; 8] = rand::random();
            inner
                .attrs
                .insert(hex::encode(handle), crypto::encrypt_file_attr(&key, plain));
            fa.push(format!("924:{t}*{}", b64url(&handle)));
        }
        FakeNode {
            h: h.into(),
            name: name.into(),
            key: key.to_vec(),
            cipher,
            size: data.len() as u64,
            fa: (!fa.is_empty()).then(|| fa.join("/")),
            dir: false,
            parent: None,
        }
    }

    /// a file link; returns its URL
    pub fn add_file_link(
        &self,
        id: &str,
        name: &str,
        data: &[u8],
        thumbs: &[(u32, Vec<u8>)],
    ) -> String {
        self.with(|i| {
            let node = Self::make_node(id, name, data, thumbs, i);
            let url = format!("https://mega.nz/file/{id}#{}", b64url(&node.key));
            i.file_links.insert(id.into(), id.into());
            i.files.insert(id.into(), node);
            url
        })
    }

    /// a folder link with files (h, path inside the folder "Sub/x.jpg", data, thumbnails); sub
    /// folders are created from the paths (handle "D" + 7 chars of the name)
    pub fn add_folder_link(
        &self,
        id: &str,
        name: &str,
        files: &[(&str, &str, Vec<u8>, Vec<(u32, Vec<u8>)>)],
    ) -> String {
        self.with(|i| {
            let key: Vec<u8> = (0..16).map(|_| rand::random()).collect();
            let root = format!("R{}", &id[1..]);
            let mut handles = Vec::new();
            for (h, path, data, thumbs) in files {
                let mut parent = root.clone();
                let segs: Vec<&str> = path.split('/').collect();
                for dir in &segs[..segs.len() - 1] {
                    let dh = format!(
                        "D{:0<7}",
                        dir.chars()
                            .filter(|c| c.is_ascii_alphanumeric())
                            .take(7)
                            .collect::<String>()
                    );
                    if !i.files.contains_key(&dh) {
                        let k: Vec<u8> = (0..16).map(|_| rand::random()).collect();
                        i.files.insert(
                            dh.clone(),
                            FakeNode {
                                h: dh.clone(),
                                name: dir.to_string(),
                                key: k,
                                cipher: vec![],
                                size: 0,
                                fa: None,
                                dir: true,
                                parent: Some(parent.clone()),
                            },
                        );
                        handles.push(dh.clone());
                    }
                    parent = dh;
                }
                let mut node = Self::make_node(h, segs[segs.len() - 1], data, thumbs, i);
                node.parent = Some(parent);
                i.files.insert(h.to_string(), node);
                handles.push(h.to_string());
            }
            i.folders
                .insert(id.into(), (key.clone(), root, name.into(), handles));
            format!("https://mega.nz/folder/{id}#{}", b64url(&key))
        })
    }

    pub fn with_sid(&self, sid: &str) -> Vec<CsLog> {
        self.log()
            .into_iter()
            .filter(|l| l.sid.as_deref() == Some(sid))
            .collect()
    }

    pub fn uh(&self, password: &str) -> (Vec<u8>, String) {
        let (k, uh) = crypto::prepare_key_v2(password.as_bytes(), &self.salt);
        (k.to_vec(), uh)
    }

    pub fn rsa_public(&self) -> (BigUint, BigUint) {
        (self.n.clone(), self.e.clone())
    }

    pub fn master(&self) -> Vec<u8> {
        self.master.clone()
    }

    pub fn privk(&self) -> Vec<u8> {
        self.privk.clone()
    }
}

fn hashcash_valid(challenge_token: &str, answer: &str, threshold: u64) -> bool {
    use sha2::{Digest, Sha256};
    let parts: Vec<&str> = answer.split(':').collect();
    if parts.len() != 3 || parts[0] != "1" || parts[1] != challenge_token {
        return false;
    }
    let prefix = from_b64(parts[2]);
    let token = from_b64(challenge_token);
    let mut h = Sha256::new();
    h.update(&prefix);
    for _ in 0..262_144 {
        h.update(&token);
    }
    let d = h.finalize();
    (u32::from_be_bytes(d[..4].try_into().unwrap()) as u64) <= threshold
}

async fn cs(
    State(s): State<Srv>,
    Query(q): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let token = b64url(&[7u8; 48]);
    {
        let mut i = s.inner.lock().unwrap();
        if let Some(ans) = headers.get("x-hashcash").and_then(|v| v.to_str().ok()) {
            // easiness 255: threshold 127 << 24 (about every second attempt matches)
            if hashcash_valid(&token, ans, 127u64 << 24) {
                i.hashcash_ok += 1;
            } else {
                return (
                    StatusCode::OK,
                    [("X-Hashcash", format!("1:255:1700000000:{token}"))],
                    "-3",
                )
                    .into_response();
            }
        } else if i.faults.hashcash {
            i.faults.hashcash = false;
            return (
                StatusCode::OK,
                [("X-Hashcash", format!("1:255:1700000000:{token}"))],
                "-3",
            )
                .into_response();
        }
        if i.faults.eagain > 0 {
            i.faults.eagain -= 1;
            return (StatusCode::OK, "-3").into_response();
        }
    }
    let cmds: Vec<Value> = serde_json::from_slice(&body).unwrap_or_default();
    let sid = q.get("sid").cloned();
    let n = q.get("n").cloned();
    let out: Vec<Value> = cmds
        .iter()
        .map(|cmd| {
            let mut i = s.inner.lock().unwrap();
            i.log.push(CsLog {
                a: cmd["a"].as_str().unwrap_or("").into(),
                sid: sid.clone(),
                n: n.clone(),
                cmd: cmd.clone(),
            });
            if let Some(sid) = &sid {
                if i.faults.reject_sessions || !i.sessions.contains(sid) {
                    return json!(-15);
                }
            }
            command(&s, &mut i, cmd, sid.as_deref(), n.as_deref())
        })
        .collect();
    axum::Json(Value::Array(out)).into_response()
}

fn command(s: &Srv, i: &mut Inner, cmd: &Value, sid: Option<&str>, n: Option<&str>) -> Value {
    match cmd["a"].as_str().unwrap_or("") {
        "us0" => json!({ "v": 2, "s": base64_std(&s.salt) }),
        "us" => {
            let Some(acc) = i
                .accounts
                .iter()
                .find(|a| Some(a.email.as_str()) == cmd["user"].as_str())
                .cloned()
            else {
                return json!(-9);
            };
            // PBKDF2 (100 000 rounds) is slow in debug builds: once per password
            let (aes, uh) = i
                .uh_cache
                .entry(acc.password.clone())
                .or_insert_with(|| crypto::prepare_key_v2(acc.password.as_bytes(), &s.salt))
                .clone();
            if cmd["uh"].as_str() != Some(uh.as_str()) {
                return json!(-9);
            }
            if let Some(m) = &acc.mfa {
                if cmd["mfa"].as_str() != Some(m.as_str()) {
                    return json!(-26);
                }
            }
            // session id: 43 random bytes (first one non-zero) + filler → RSA with the public key
            let mut raw: Vec<u8> = (0..43).map(|_| rand::random()).collect();
            raw[0] |= 1;
            let sid = b64url(&raw);
            i.sessions.insert(sid);
            raw.extend((0..212).map(|_| rand::random::<u8>()));
            let m = BigUint::from_bytes_be(&raw);
            let c = m.modpow(&s.e, &s.n);
            json!({
                "k": b64url(&crypto::ecb_encrypt(&aes, &s.master)),
                "csid": b64url(&crypto::write_mpi(&c)),
                "privk": b64url(&crypto::ecb_encrypt(&s.master, &s.privk)),
                "u": "UsErHnd1",
            })
        }
        "ug" => {
            if sid.is_none() {
                return json!(-15);
            }
            json!({ "u": "UsErHnd1", "email": i.accounts.first().map(|a| a.email.clone()), "name": "Test" })
        }
        "uq" => {
            if sid.is_none() {
                return json!(-15);
            }
            let a = i.accounts.first().cloned();
            let mut v = json!({ "utype": a.as_ref().map(|a| a.utype).unwrap_or(0) });
            if let Some(m) = a.as_ref().and_then(|a| a.mxfer) {
                v["mxfer"] = json!(m);
            }
            if let Some(c) = a.as_ref().and_then(|a| a.caxfer) {
                v["caxfer"] = json!(c);
            }
            v
        }
        "sml" => {
            if let Some(sid) = sid {
                i.sessions.remove(sid);
            }
            json!(0)
        }
        "f" => {
            let Some((key, root, name, handles)) = n.and_then(|n| i.folders.get(n)).cloned() else {
                return json!(-9);
            };
            let ts = 1_700_000_000u64;
            let mut f = vec![
                json!({ "h": root, "p": "PaReNt00", "t": 1, "ts": ts, "a": crypto::encrypt_attributes(&key, &json!({ "n": name })), "k": format!("{root}:{}", b64url(&crypto::ecb_encrypt(&key, &key))) }),
            ];
            for h in handles {
                let k = &i.files[&h];
                let mut v = json!({ "h": k.h, "p": k.parent, "t": k.dir as u8, "ts": ts, "a": crypto::encrypt_attributes(&k.key, &json!({ "n": k.name })), "k": format!("{}:{}", k.h, b64url(&crypto::ecb_encrypt(&key, &k.key))) });
                if !k.dir {
                    v["s"] = json!(k.size);
                }
                if let Some(fa) = &k.fa {
                    v["fa"] = json!(fa);
                }
                f.push(v);
            }
            json!({ "f": f, "ok": [] })
        }
        "g" => {
            let node = if let Some(p) = cmd["p"].as_str() {
                i.file_links.get(p).and_then(|h| i.files.get(h)).cloned()
            } else {
                n.and_then(|n| i.folders.get(n))
                    .filter(|f| f.3.iter().any(|h| Some(h.as_str()) == cmd["n"].as_str()))
                    .and_then(|_| i.files.get(cmd["n"].as_str()?))
                    .cloned()
            };
            let Some(node) = node else { return json!(-9) };
            if cmd["g"].as_i64() == Some(1) {
                if let Some(tl) = i.faults.quota_tl {
                    return json!({ "e": -17, "tl": tl });
                }
                return json!({ "g": format!("{}dl/{}", s.base, node.h), "s": node.size });
            }
            let mut v = json!({ "s": node.size, "at": crypto::encrypt_attributes(&node.key, &json!({ "n": node.name })) });
            if let Some(fa) = &node.fa {
                v["fa"] = json!(fa);
            }
            v
        }
        "ufa" => json!({ "p": format!("{}fa", s.base) }),
        _ => json!(-2),
    }
}

fn base64_std(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(b)
}

async fn dl(State(s): State<Srv>, Path((h, range)): Path<(String, String)>) -> Response {
    let mut i = s.inner.lock().unwrap();
    let Some(f) = i.files.get(&h).cloned() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some((a, b)) = range
        .split_once('-')
        .and_then(|(a, b)| Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()?)))
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    i.dl.push((h.clone(), range.clone()));
    if let Some(t) = i.faults.dl_509 {
        return (
            StatusCode::from_u16(509).unwrap(),
            [("x-mega-time-left", t.to_string())],
            "",
        )
            .into_response();
    }
    if i.faults.dl_403 > 0 {
        i.faults.dl_403 -= 1;
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut data = f.cipher[a.min(f.cipher.len())..(b + 1).min(f.cipher.len())].to_vec();
    if i.faults.corrupt.as_deref() == Some(h.as_str()) && !data.is_empty() {
        let m = data.len() / 2;
        data[m] ^= 0x55;
    }
    if let Some((left, bytes)) = i.faults.dl_cut {
        if left > 0 {
            i.faults.dl_cut = if left > 1 {
                Some((left - 1, bytes))
            } else {
                None
            };
            // announce the full length, send a part, then end the body with an error
            let len = data.len();
            let part = Bytes::from(data[..bytes.min(len)].to_vec());
            // (flush the part first: a connection closed before any byte is retried by hyper)
            let stream = futures_util::stream::unfold(0u8, move |st| {
                let part = part.clone();
                async move {
                    match st {
                        0 => Some((Ok::<Bytes, std::io::Error>(part), 1)),
                        1 => {
                            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                            Some((Err(std::io::Error::other("cut")), 2))
                        }
                        _ => None,
                    }
                }
            });
            return Response::builder()
                .status(200)
                .header("content-length", len.to_string())
                .body(axum::body::Body::from_stream(stream))
                .unwrap();
        }
    }
    (
        StatusCode::OK,
        [("content-type", "application/octet-stream")],
        data,
    )
        .into_response()
}

fn fa_answer(s: &Srv, body: &[u8]) -> Response {
    let mut i = s.inner.lock().unwrap();
    let mut parts = Vec::new();
    for h in body.chunks_exact(8) {
        let key = hex::encode(h);
        i.fa_posts.push(key.clone());
        if let Some(d) = i.attrs.get(&key) {
            parts.push((<[u8; 8]>::try_from(h).unwrap(), d.clone()));
        }
    }
    (StatusCode::OK, crypto::build_file_attr_response(&parts)).into_response()
}

async fn fa(State(s): State<Srv>, Path(_t): Path<String>, body: Bytes) -> Response {
    fa_answer(&s, &body)
}

async fn fa_root(State(s): State<Srv>, body: Bytes) -> Response {
    fa_answer(&s, &body)
}
