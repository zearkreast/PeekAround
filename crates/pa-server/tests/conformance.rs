//! Conformance harness: the TypeScript server and the Rust server on identical copies of the
//! fixture library, the same requests against both, JSON responses diffed after normalising the
//! volatile fields (absolute paths, ports, generation tokens, wall-clock stamps).
//!
//! Ignored by default. Run with
//! ```text
//! PEEK_TS_DIR=/home/user/peekaround cargo test -p pa-server --test conformance -- --ignored --nocapture
//! ```
//! (`PEEK_TS_DIR` = a checkout of the TS repo with `node_modules` and a built
//! `packages/server/dist`; `PEEK_FIXTURES` overrides `<ts>/fixtures/library`; `PEEK_CONF_OUT`
//! writes the full report there.)

use bytes::Bytes;
use rand::{rngs::StdRng, Rng, SeedableRng};
use serde_json::{json, Map, Value};
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TS_PORT: u16 = 19041;
const RS_PORT: u16 = 19042;

struct Side {
    base: String,
    root: String,
    data: String,
}

#[derive(Debug, Clone)]
struct Resp {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Bytes,
}

#[derive(Clone, Default)]
struct Case {
    method: String,
    path: String,
    body: Option<Value>,
    raw: Option<(Vec<u8>, String)>,
    headers: Vec<(String, String)>,
    /// no x-peek-internal header (rate limit headers are compared)
    external: bool,
    /// body compared as bytes (sha1) instead of JSON
    binary: bool,
    /// only status + content type (thumbnails: encoders differ)
    shallow: bool,
    /// wait for both servers to settle afterwards
    settle: bool,
    /// json keys dropped before comparing (in addition to the global list)
    ignore: Vec<&'static str>,
}

impl Case {
    fn method_post(mut self) -> Case {
        self.method = "POST".into();
        self
    }
}

fn get(path: impl Into<String>) -> Case {
    Case {
        method: "GET".into(),
        path: path.into(),
        ..Default::default()
    }
}
fn req(method: &str, path: impl Into<String>, body: Value) -> Case {
    Case {
        method: method.into(),
        path: path.into(),
        body: Some(body),
        settle: true,
        ..Default::default()
    }
}

struct Harness {
    client: reqwest::Client,
    ts: Side,
    rs: Side,
    report: Vec<String>,
    cases: usize,
    diffs: usize,
}

/// keys compared by presence only (thumbhash: sharp vs fast_image_resize sampling differs by a bit)
const PRESENCE: &[&str] = &["thumbhash"];

/// keys whose values differ by nature between two runs (wall clock / identity)
const VOLATILE: &[&str] = &[
    "scannedAt",
    "gen",
    "uptime",
    "version",
    "created",
    "updated",
    "addedAt",
    "added",
    "lastSeen",
    "startedAt",
    "finishedAt",
    "expiresAt",
    "at",
    "token",
    "internalToken",
];

impl Side {
    fn norm_str(&self, s: &str) -> String {
        static UUID: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
            regex::Regex::new("[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
                .expect("re")
        });
        let mut s = s
            .replace(&self.root, "<ROOT>")
            .replace(&self.data, "<DATA>");
        s = UUID.replace_all(&s, "<UUID>").into_owned();
        s = s.replace(&self.base, "<BASE>");
        let port = self.base.rsplit(':').next().unwrap_or("");
        s.replace(&format!(":{port}"), ":<PORT>")
    }
    fn norm(&self, v: &Value, ignore: &[&str]) -> Value {
        match v {
            Value::String(s) => Value::String(self.norm_str(s)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.norm(x, ignore)).collect()),
            Value::Object(o) => {
                let mut m = Map::new();
                for (k, x) in o {
                    if PRESENCE.contains(&k.as_str()) {
                        m.insert(
                            k.clone(),
                            Value::String(if x.is_null() {
                                "<null>".into()
                            } else {
                                "<present>".into()
                            }),
                        );
                    } else if VOLATILE.contains(&k.as_str()) || ignore.contains(&k.as_str()) {
                        m.insert(
                            k.clone(),
                            Value::String(if x.is_null() {
                                "<null>".into()
                            } else {
                                "<V>".into()
                            }),
                        );
                    } else {
                        m.insert(k.clone(), self.norm(x, ignore));
                    }
                }
                Value::Object(m)
            }
            // wall clock of files written during the run (uploads, edits): ±15 h for localAt
            Value::Number(n)
                if n.as_i64()
                    .is_some_and(|v| v > *START_MS - 600_000 && v < now_ms() + 15 * 3_600_000) =>
            {
                Value::String("<NOW>".into())
            }
            x => x.clone(),
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

static START_MS: std::sync::LazyLock<i64> = std::sync::LazyLock::new(now_ms);

fn diff_json(a: &Value, b: &Value, path: &str, out: &mut Vec<String>) {
    if out.len() > 12 {
        return;
    }
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, v) in x {
                match y.get(k) {
                    Some(w) => diff_json(v, w, &format!("{path}/{k}"), out),
                    None => out.push(format!("{path}/{k}: only in TS ({})", short(v))),
                }
            }
            for (k, w) in y {
                if !x.contains_key(k) {
                    out.push(format!("{path}/{k}: only in RS ({})", short(w)));
                }
            }
            let kx: Vec<&String> = x.keys().filter(|k| y.contains_key(*k)).collect();
            let ky: Vec<&String> = y.keys().filter(|k| x.contains_key(*k)).collect();
            if kx != ky {
                out.push(format!("{path}: key order TS {kx:?} RS {ky:?}"));
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                out.push(format!("{path}: array len TS {} RS {}", x.len(), y.len()));
            }
            for (i, (v, w)) in x.iter().zip(y).enumerate() {
                diff_json(v, w, &format!("{path}/{i}"), out);
            }
        }
        (Value::Number(x), Value::Number(y)) => {
            let (fx, fy) = (x.as_f64().unwrap_or(0.0), y.as_f64().unwrap_or(0.0));
            if x.to_string() != y.to_string() && !(fx == fy) {
                out.push(format!("{path}: TS {x} RS {y}"));
            } else if x.to_string() != y.to_string() {
                out.push(format!("{path}: number text TS {x} RS {y}"));
            }
        }
        _ => {
            if a != b {
                out.push(format!("{path}: TS {} RS {}", short(a), short(b)));
            }
        }
    }
}

fn short(v: &Value) -> String {
    let s = v.to_string();
    if s.len() > 160 {
        let mut e = 160;
        while !s.is_char_boundary(e) {
            e -= 1;
        }
        format!("{}…", &s[..e])
    } else {
        s
    }
}

const CMP_HEADERS: &[&str] = &[
    "content-type",
    "cache-control",
    "content-range",
    "accept-ranges",
    "content-disposition",
    "vary",
    "access-control-allow-origin",
    "access-control-allow-methods",
    "access-control-allow-headers",
    "x-ratelimit-limit",
    "x-ratelimit-remaining",
    "retry-after",
    "location",
    "content-encoding",
];

impl Harness {
    async fn send(&self, side: &Side, c: &Case) -> Resp {
        let url = format!("{}{}", side.base, side.norm_back(&c.path));
        let mut rb = self.client.request(c.method.parse().expect("method"), &url);
        if !c.external {
            rb = rb.header("x-peek-internal", "1");
        }
        for (k, v) in &c.headers {
            rb = rb.header(k.as_str(), side.norm_back(v));
        }
        if let Some(b) = &c.body {
            let s = side.norm_back(&b.to_string());
            rb = rb.header("content-type", "application/json").body(s);
        }
        if let Some((raw, ct)) = &c.raw {
            rb = rb.header("content-type", ct.as_str()).body(raw.clone());
        }
        let r = match rb.send().await {
            Ok(r) => r,
            Err(e) => {
                return Resp {
                    status: 0,
                    headers: BTreeMap::new(),
                    body: Bytes::from(format!("transport error: {e}")),
                }
            }
        };
        let status = r.status().as_u16();
        let mut headers = BTreeMap::new();
        for (k, v) in r.headers() {
            headers.insert(k.as_str().to_string(), v.to_str().unwrap_or("").to_string());
        }
        let body = r.bytes().await.unwrap_or_default();
        Resp {
            status,
            headers,
            body,
        }
    }

    async fn run(&mut self, c: Case) -> (Resp, Resp) {
        let a = self.send(&self.ts, &c).await;
        let b = self.send(&self.rs, &c).await;
        self.cases += 1;
        let mut d = Vec::new();
        if a.status != b.status {
            d.push(format!("status TS {} RS {}", a.status, b.status));
        }
        for h in CMP_HEADERS {
            let (x, y) = (a.headers.get(*h), b.headers.get(*h));
            let norm = |v: Option<&String>, s: &Side| v.map(|v| s.norm_str(v));
            let (x, y) = (norm(x, &self.ts), norm(y, &self.rs));
            if *h == "content-type"
                && c.shallow
                && x.as_deref().unwrap_or("").starts_with("image/")
                && y.as_deref().unwrap_or("").starts_with("image/")
            {
                continue;
            }
            if x != y {
                d.push(format!("header {h}: TS {x:?} RS {y:?}"));
            }
        }
        if !c.shallow {
            let ct = a.headers.get("content-type").cloned().unwrap_or_default();
            if !c.binary && ct.contains("json") {
                match (
                    serde_json::from_slice::<Value>(&a.body),
                    serde_json::from_slice::<Value>(&b.body),
                ) {
                    (Ok(x), Ok(y)) => {
                        let (x, y) = (self.ts.norm(&x, &c.ignore), self.rs.norm(&y, &c.ignore));
                        diff_json(&x, &y, "", &mut d);
                    }
                    (x, y) => {
                        if a.body != b.body {
                            d.push(format!(
                                "body (non-json) TS {:?} RS {:?}",
                                x.err().map(|e| e.to_string()),
                                y.err().map(|e| e.to_string())
                            ));
                        }
                    }
                }
            } else if ct.starts_with("text/") && !c.binary {
                let (x, y) = (
                    self.ts.norm_str(&String::from_utf8_lossy(&a.body)),
                    self.rs.norm_str(&String::from_utf8_lossy(&b.body)),
                );
                if x != y {
                    d.push(format!(
                        "text body TS {:?} RS {:?}",
                        short(&Value::String(x)),
                        short(&Value::String(y))
                    ));
                }
            } else {
                let (ha, hb) = (
                    hex::encode(Sha1::digest(&a.body)),
                    hex::encode(Sha1::digest(&b.body)),
                );
                if ha != hb {
                    d.push(format!(
                        "bytes TS {} ({ha:.10}) RS {} ({hb:.10})",
                        a.body.len(),
                        b.body.len()
                    ));
                }
            }
        }
        if !d.is_empty() {
            self.diffs += 1;
            let hs: Vec<String> = c.headers.iter().map(|(k, v)| format!("{k}: {v}")).collect();
            let label = if hs.is_empty() {
                String::new()
            } else {
                format!(" [{}]", hs.join("; "))
            };
            self.report.push(format!(
                "✗ {} {}{label}\n    {}",
                c.method,
                c.path,
                d.join("\n    ")
            ));
        }
        if c.settle {
            self.settle().await;
        }
        (a, b)
    }

    async fn json(&self, side: &Side, path: &str) -> Value {
        let r = self.send(side, &get(path)).await;
        serde_json::from_slice(&r.body).unwrap_or(Value::Null)
    }

    /// both servers idle: indexing done, no enrichment, stable status twice in a row
    async fn settle(&self) {
        let t0 = Instant::now();
        let mut last = (Value::Null, Value::Null);
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let a = self.json(&self.ts, "/api/meta/status").await;
            let b = self.json(&self.rs, "/api/meta/status").await;
            let idle = |v: &Value| {
                v["enriching"] == json!(false) && v["indexing"]["done"] == v["indexing"]["total"]
            };
            if idle(&a) && idle(&b) && (a.clone(), b.clone()) == last {
                return;
            }
            last = (a, b);
            if t0.elapsed() > Duration::from_secs(120) {
                eprintln!("settle timeout: {last:?}");
                return;
            }
        }
    }
}

impl Side {
    /// placeholders in a request → this side's values
    fn norm_back(&self, s: &str) -> String {
        s.replace("<ROOT>", &self.root)
            .replace("<DATA>", &self.data)
    }
}

fn copy_tree(from: &Path, to: &Path) {
    let st = Command::new("cp")
        .arg("-a")
        .arg(from)
        .arg(to)
        .status()
        .expect("cp");
    assert!(st.success());
}

struct Kill(Child);
impl Drop for Kill {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn wait_up(client: &reqwest::Client, base: &str) {
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(60) {
        if client.get(format!("{base}/api/info")).send().await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("server at {base} did not come up");
}

fn zip_bytes(files: &[(&str, &[u8])]) -> Vec<u8> {
    // stored (no compression) zip with UTF-8 names
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let off = out.len() as u32;
        let crc = crc32fast::hash(data);
        let n = name.as_bytes();
        let mut h = Vec::new();
        h.extend_from_slice(&0x04034b50u32.to_le_bytes());
        h.extend_from_slice(&20u16.to_le_bytes());
        h.extend_from_slice(&0x0800u16.to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(&0u32.to_le_bytes());
        h.extend_from_slice(&crc.to_le_bytes());
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&(n.len() as u16).to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(n);
        out.extend_from_slice(&h);
        out.extend_from_slice(data);
        let mut c = Vec::new();
        c.extend_from_slice(&0x02014b50u32.to_le_bytes());
        c.extend_from_slice(&20u16.to_le_bytes());
        c.extend_from_slice(&20u16.to_le_bytes());
        c.extend_from_slice(&0x0800u16.to_le_bytes());
        c.extend_from_slice(&0u16.to_le_bytes());
        c.extend_from_slice(&0u32.to_le_bytes());
        c.extend_from_slice(&crc.to_le_bytes());
        c.extend_from_slice(&(data.len() as u32).to_le_bytes());
        c.extend_from_slice(&(data.len() as u32).to_le_bytes());
        c.extend_from_slice(&(n.len() as u16).to_le_bytes());
        c.extend_from_slice(&[0u8; 12]);
        c.extend_from_slice(&off.to_le_bytes());
        c.extend_from_slice(n);
        central.extend_from_slice(&c);
    }
    let cd_off = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x06054b50u32.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_off.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn ids_of(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|i| i["id"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the TypeScript server (PEEK_TS_DIR)"]
async fn conformance() {
    let _ = *START_MS;
    let ts_dir = PathBuf::from(
        std::env::var("PEEK_TS_DIR").unwrap_or_else(|_| "/home/user/peekaround".into()),
    );
    let fixtures = std::env::var("PEEK_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|_| ts_dir.join("fixtures/library"));
    let work = std::env::var("PEEK_CONF_WORK")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join(format!("peek-conf-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(work.join("ts")).expect("work dir");
    std::fs::create_dir_all(work.join("rs")).expect("work dir");
    copy_tree(&fixtures, &work.join("ts/lib"));
    copy_tree(&fixtures, &work.join("rs/lib"));
    let ts_root = work.join("ts/lib").to_string_lossy().into_owned();
    let rs_root = work.join("rs/lib").to_string_lossy().into_owned();
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());

    let ts_child = Command::new("node")
        .arg(ts_dir.join("packages/server/dist/cli.js"))
        .args(["-r", &format!("Test={ts_root}"), "-d"])
        .arg(work.join("ts/data"))
        .args([
            "-p",
            &TS_PORT.to_string(),
            "--host",
            "127.0.0.1",
            "--no-mdns",
        ])
        .current_dir(&ts_dir)
        .env("npm_package_version", "1.3.2")
        .env("PEEK_WEB_DIR", ts_dir.join("packages/web/dist"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("node");
    let _ts_guard = Kill(ts_child);

    let mut o = pa_server::ServerOptions::new(work.join("rs/data"));
    o.port = RS_PORT;
    o.host = "127.0.0.1".into();
    o.roots = vec![pa_server::RootSpec {
        name: Some("Test".into()),
        path: rs_root.clone(),
        split: false,
    }];
    o.host_browse_roots = vec![rs_root.clone(), home.clone()];
    o.version = "1.3.2".into();
    o.web_dir = Some(ts_dir.join("packages/web/dist"));
    let srv = pa_server::Server::start(o).await.expect("rust server");

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .expect("client");
    let ts = Side {
        base: format!("http://127.0.0.1:{TS_PORT}"),
        root: ts_root,
        data: work.join("ts/data").to_string_lossy().into_owned(),
    };
    let rs = Side {
        base: format!("http://127.0.0.1:{RS_PORT}"),
        root: rs_root,
        data: work.join("rs/data").to_string_lossy().into_owned(),
    };
    wait_up(&client, &ts.base).await;
    wait_up(&client, &rs.base).await;
    let mut h = Harness {
        client,
        ts,
        rs,
        report: Vec::new(),
        cases: 0,
        diffs: 0,
    };
    h.settle().await;

    // added_at is the wall clock of the insert (trigger): make it deterministic on both sides,
    // then invalidate the explore caches with a favourite toggle
    for d in [&h.ts.data, &h.rs.data] {
        let c = rusqlite::Connection::open(Path::new(d).join("peekaround.db")).expect("open db");
        c.busy_timeout(Duration::from_secs(10))
            .expect("busy timeout");
        c.execute("UPDATE media SET added_at = mtime", [])
            .expect("added_at");
    }
    let first = h
        .json(&h.ts, "/api/browse?libraryId=1&recursive=1&limit=1")
        .await;
    if let Some(id) = ids_of(&first).first() {
        for fav in [true, false] {
            let c = req(
                "PATCH",
                format!("/api/media/{id}"),
                json!({ "favorite": fav }),
            );
            let _ = h.send(&h.ts, &c).await;
            let _ = h.send(&h.rs, &c).await;
        }
    }
    h.settle().await;

    // ── id sanity: same scan order on both sides ──
    let all = "/api/browse?libraryId=1&recursive=1&limit=1000&sort=name&order=asc";
    let a = h.json(&h.ts, all).await;
    let b = h.json(&h.rs, all).await;
    let ids = ids_of(&a);
    assert!(!ids.is_empty(), "no media in the TS listing");
    if ids != ids_of(&b) {
        h.report.push(format!(
            "!! media ids differ between the servers: TS {:?} RS {:?}",
            &ids[..ids.len().min(10)],
            &ids_of(&b)[..ids_of(&b).len().min(10)]
        ));
    }
    let kinds: Vec<(String, String, String)> = a["items"]
        .as_array()
        .map(|v| {
            v.iter()
                .map(|i| {
                    (
                        i["id"].as_str().unwrap_or("").to_string(),
                        i["kind"].as_str().unwrap_or("").to_string(),
                        i["dir"].as_str().unwrap_or("").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();

    // ── read-only GETs ──
    for p in [
        "/api/info",
        "/api/settings",
        "/api/libraries",
        "/api/libraries/1/stats",
        "/api/libraries/99/stats",
        "/api/meta/status",
        "/api/albums",
        "/api/albums/1",
        "/api/devices",
        "/api/fonts",
        "/api/jobs",
        "/api/remote/account",
        "/api/tiles/cache",
        "/api/tiles/status",
        "/api/geo/points",
        "/api/geo/points?libraryId=1",
        "/api/geo/points?libraryId=1&kind=video",
        "/api/explore",
        "/api/explore?libraryId=1",
        "/api/explore?libraryId=1&kind=image",
        "/api/explore/calendar?year=2024",
        "/api/explore/calendar?year=2025&libraryId=1",
        "/api/explore/calendar",
        "/api/explore/duplicates",
        "/api/explore/duplicates?offset=1&limit=2",
        "/api/host/dirs",
        "/api/host/dirs?path=<ROOT>",
        "/api/host/dirs?path=/etc",
        "/api/host/dirs?path=<ROOT>/nope",
        "/api/timeline?libraryId=1",
        "/api/timeline?libraryId=1&kind=image",
        "/api/timeline?libraryId=1&recursive=1",
        "/api/search?q=berlin",
        "/api/search?q=2024",
        "/api/search?q=",
        "/api/search",
        "/api/media/0",
        "/api/media/abc",
        "/api/media/99999",
        "/api/media/99999/file",
        "/api/media/99999/thumb",
        "/api/nope",
        "/api/browse/nope",
        "/nope",
        "/api/download",
        "/api/download?ids=",
        "/api/zip/peek",
        "/api/devices/pull/xyz",
        "/api/remote/folders/1/link",
        "/api/fonts/1/file",
    ] {
        h.run(get(p)).await;
    }

    // thumbnail bytes: sharp/libvips vs pa-media encoders
    h.run(Case {
        ignore: vec!["bytes"],
        ..get("/api/cache")
    })
    .await;
    h.run(Case {
        ignore: vec!["bytes"],
        ..get("/api/cache/details")
    })
    .await;

    // ── browse: folders, skeleton, random filter combos ──
    let mut rng = StdRng::seed_from_u64(1313);
    let dirs: Vec<String> = {
        let mut d: Vec<String> = kinds.iter().map(|k| k.2.clone()).collect();
        d.sort();
        d.dedup();
        d
    };
    for d in &dirs {
        let e = percent_encoding::utf8_percent_encode(d, percent_encoding::NON_ALPHANUMERIC)
            .to_string();
        h.run(get(format!("/api/browse?folderId=1&dir={e}"))).await;
        h.run(get(format!("/api/browse/skeleton?folderId=1&dir={e}")))
            .await;
        h.run(get(format!(
            "/api/browse?folderId=1&dir={e}&recursive=1&sort=name&order=asc&limit=5&offset=2"
        )))
        .await;
    }
    for p in [
        "/api/browse",
        "/api/browse?libraryId=1",
        "/api/browse?libraryId=99",
        "/api/browse?folderId=99",
        "/api/browse?folderId=1&dir=../etc",
        "/api/browse?folderId=1&dir=nope",
        "/api/browse/skeleton",
        "/api/browse/skeleton?libraryId=1",
        "/api/browse/skeleton?libraryId=1&recursive=1",
    ] {
        h.run(get(p)).await;
    }
    let sorts = ["takenAt", "name", "mtime", "size", "kind", "bogus"];
    let kinds_f = ["all", "image", "video", "favorites", "audio", ""];
    let filters = [
        "",
        "q=ankara",
        "q=berlin 2025",
        "q=video",
        "q=type:video",
        "q=-aile",
        "q=İstanbul",
        "q=istanbul",
        "flag=screenshot",
        "flag=panorama",
        "flag=video",
        "flag=selfie",
        "flag=gif",
        "flag=bogus",
        "from=1700000000000",
        "to=1720000000000",
        "from=1690000000000&to=1730000000000",
        "place=TR",
        "place=Berlin",
        "camera=Google",
        "gps=none",
        "gps=any",
        "color=blue",
        "color=green",
        "orient=portrait",
        "orient=landscape",
        "orient=square",
        "orient=pano",
        "daytime=night",
        "daytime=morning",
        "vlen=short",
        "vlen=long",
        "dup=1",
        "city=Ankara",
        "byAdded=1",
        "byPosition=1",
    ];
    for i in 0..220 {
        let sort = sorts[rng.gen_range(0..sorts.len())];
        let order = if rng.gen_bool(0.5) { "asc" } else { "desc" };
        let kind = kinds_f[rng.gen_range(0..kinds_f.len())];
        let f1 = filters[rng.gen_range(0..filters.len())];
        let f2 = if rng.gen_bool(0.3) {
            filters[rng.gen_range(0..filters.len())]
        } else {
            ""
        };
        // repeated keys become arrays in Fastify's query parser (documented difference): skip
        let key = |f: &str| f.split('=').next().unwrap_or("").to_string();
        let f2 = if key(f2) == key(f1) { "" } else { f2 };
        let scope = match rng.gen_range(0..4) {
            0 => "libraryId=1&recursive=1".to_string(),
            1 => "folderId=1&recursive=1".to_string(),
            2 => format!(
                "folderId=1&dir={}",
                percent_encoding::utf8_percent_encode(
                    &dirs[rng.gen_range(0..dirs.len())],
                    percent_encoding::NON_ALPHANUMERIC
                )
            ),
            _ => "libraryId=1".to_string(),
        };
        let mut q = format!("{scope}&sort={sort}&order={order}");
        if !kind.is_empty() {
            q.push_str(&format!("&kind={kind}"));
        }
        for f in [f1, f2] {
            if !f.is_empty() {
                let f = f.replace(' ', "%20").replace('İ', "%C4%B0");
                q.push('&');
                q.push_str(&f);
            }
        }
        if rng.gen_bool(0.3) {
            q.push_str(&format!(
                "&offset={}&limit={}",
                rng.gen_range(0..30),
                rng.gen_range(1..40)
            ));
        }
        let ep = match i % 4 {
            0 | 1 => "/api/browse",
            2 => "/api/browse/skeleton",
            _ => "/api/timeline",
        };
        h.run(get(format!("{ep}?{q}"))).await;
        if i % 11 == 0 {
            h.run(get(format!("/api/geo/points?{q}"))).await;
            h.run(get(format!("/api/explore/calendar?year=2024&{q}")))
                .await;
        }
    }

    // explore section filters → browse
    let ex = h.json(&h.ts, "/api/explore?libraryId=1").await;
    let mut ex_queries = Vec::new();
    fn walk_filters(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                if let Some(Value::Object(q)) = o.get("query").or_else(|| o.get("filter")) {
                    let s: Vec<String> = q
                        .iter()
                        .filter_map(|(k, v)| {
                            let s = match v {
                                Value::String(s) => s.clone(),
                                Value::Number(n) => n.to_string(),
                                Value::Bool(b) if *b => "1".into(),
                                _ => return None,
                            };
                            Some(format!(
                                "{k}={}",
                                percent_encoding::utf8_percent_encode(
                                    &s,
                                    percent_encoding::NON_ALPHANUMERIC
                                )
                            ))
                        })
                        .collect();
                    out.push(s.join("&"));
                }
                for x in o.values() {
                    walk_filters(x, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| walk_filters(x, out)),
            _ => {}
        }
    }
    walk_filters(&ex, &mut ex_queries);
    ex_queries.sort();
    ex_queries.dedup();
    for q in ex_queries.iter().take(80) {
        h.run(get(format!("/api/browse?libraryId=1&recursive=1&{q}")))
            .await;
        h.run(get(format!(
            "/api/browse/skeleton?libraryId=1&recursive=1&{q}"
        )))
        .await;
    }

    // ── every media item: details, file (+ Range), thumbs ──
    for (id, kind, _) in &kinds {
        h.run(get(format!("/api/media/{id}"))).await;
        h.run(Case {
            binary: true,
            ..get(format!("/api/media/{id}/file"))
        })
        .await;
        h.run(Case {
            binary: true,
            headers: vec![("range".into(), "bytes=0-99".into())],
            ..get(format!("/api/media/{id}/file"))
        })
        .await;
        h.run(Case {
            shallow: true,
            ..get(format!("/api/media/{id}/thumb"))
        })
        .await;
        h.run(Case {
            shallow: true,
            ..get(format!("/api/media/{id}/thumb?s=640"))
        })
        .await;
        if kind == "video" {
            h.run(Case {
                shallow: true,
                ..get(format!("/api/media/{id}/preview"))
            })
            .await;
            h.run(Case {
                shallow: true,
                ..get(format!("/api/media/{id}/filmstrip?n=4&h=60"))
            })
            .await;
        } else {
            h.run(Case {
                shallow: true,
                ..get(format!("/api/media/{id}/display"))
            })
            .await;
            h.run(Case {
                shallow: true,
                ..get(format!("/api/media/{id}/display?max=800"))
            })
            .await;
        }
    }
    if let Some((id, _, _)) = kinds.first() {
        for r in [
            "bytes=100-",
            "bytes=-50",
            "bytes=5-4",
            "bytes=99999999-",
            "bytes=0-0,5-9",
            "items=0-1",
            "bytes=abc",
        ] {
            h.run(Case {
                binary: true,
                headers: vec![("range".into(), r.into())],
                ..get(format!("/api/media/{id}/file"))
            })
            .await;
        }
        h.run(Case {
            binary: true,
            method: "HEAD".into(),
            ..get(format!("/api/media/{id}/file"))
        })
        .await;
        h.run(Case {
            binary: true,
            ..get(format!("/api/media/{id}/file?download=1"))
        })
        .await;
        h.run(Case {
            binary: true,
            headers: vec![(
                "if-modified-since".into(),
                "Wed, 01 Jan 2099 00:00:00 GMT".into(),
            )],
            ..get(format!("/api/media/{id}/file"))
        })
        .await;
        // zip download of a few ids (stored entries → same bytes)
        let few = ids.iter().take(3).cloned().collect::<Vec<_>>().join(",");
        h.run(Case {
            binary: true,
            ..get(format!("/api/download?ids={few}&name=Seçim"))
        })
        .await;
        h.run(Case {
            binary: true,
            ..get("/api/download?folderId=1&dir=Fotoğraflar/Komik")
        })
        .await;
    }

    // ── security ──
    h.run(Case {
        headers: vec![("host".into(), "evil.example.com".into())],
        ..get("/api/info")
    })
    .await;
    h.run(Case {
        headers: vec![("origin".into(), "http://evil.example.com".into())],
        ..req("POST", "/api/albums", json!({"name": "x"}))
    })
    .await;
    h.run(Case {
        headers: vec![("origin".into(), "http://evil.example.com".into())],
        ..get("/api/info")
    })
    .await;
    h.run(Case {
        headers: vec![("sec-fetch-site".into(), "cross-site".into())],
        ..req("POST", "/api/albums", json!({"name": "x"}))
    })
    .await;
    h.run(Case {
        headers: vec![("origin".into(), "http://localhost:5173".into())],
        ..get("/api/info")
    })
    .await;
    h.run(Case {
        method: "OPTIONS".into(),
        headers: vec![
            ("origin".into(), "http://localhost:5173".into()),
            ("access-control-request-method".into(), "POST".into()),
            (
                "access-control-request-headers".into(),
                "content-type".into(),
            ),
        ],
        ..get("/api/albums")
    })
    .await;
    h.run(Case {
        method: "OPTIONS".into(),
        headers: vec![("origin".into(), "http://localhost:5173".into())],
        ..get("/api/albums")
    })
    .await;
    h.run(Case {
        method: "OPTIONS".into(),
        ..get("/api/albums")
    })
    .await;
    h.run(Case {
        raw: Some((b"{bad json".to_vec(), "application/json".into())),
        ..req("POST", "/api/albums", Value::Null)
    })
    .await;
    h.run(Case {
        raw: Some((b"name=x".to_vec(), "text/plain".into())),
        ..req("POST", "/api/albums", Value::Null)
    })
    .await;

    // ── albums CRUD ──
    h.run(req(
        "POST",
        "/api/albums",
        json!({"name": "  Tatil   2024 "}),
    ))
    .await;
    h.run(req("POST", "/api/albums", json!({"name": ""}))).await;
    h.run(req("POST", "/api/albums", json!({}))).await;
    h.run(req(
        "POST",
        "/api/albums",
        json!({"name": "İkinci", "ids": ids.iter().take(5).collect::<Vec<_>>() }),
    ))
    .await;
    h.run(req(
        "POST",
        "/api/albums/1/items",
        json!({"ids": ids.iter().skip(3).take(6).collect::<Vec<_>>() }),
    ))
    .await;
    h.run(req("POST", "/api/albums/1/items", json!({"ids": "nope"})))
        .await;
    h.run(req("POST", "/api/albums/99/items", json!({"ids": ["1"]})))
        .await;
    h.run(get("/api/albums")).await;
    h.run(get("/api/albums/1")).await;
    h.run(get("/api/albums/2")).await;
    h.run(get("/api/browse?albumId=1")).await;
    h.run(get("/api/browse?albumId=1&byPosition=1")).await;
    h.run(get("/api/browse/skeleton?albumId=2")).await;
    h.run(req(
        "POST",
        "/api/albums/1/reorder",
        json!({"ids": ids.iter().skip(3).take(6).rev().collect::<Vec<_>>() }),
    ))
    .await;
    h.run(get("/api/browse?albumId=1&byPosition=1")).await;
    h.run(req(
        "PATCH",
        "/api/albums/1",
        json!({"name": "Yeni ad", "order": "asc", "coverId": ids[4]}),
    ))
    .await;
    h.run(req("PATCH", "/api/albums/1", json!({"order": "bogus"})))
        .await;
    h.run(req("PATCH", "/api/albums/1", json!({"coverId": null})))
        .await;
    h.run(req(
        "POST",
        "/api/albums/membership",
        json!({"ids": ids.iter().take(8).collect::<Vec<_>>() }),
    ))
    .await;
    h.run(req("POST", "/api/albums/order", json!({"ids": ["2", "1"]})))
        .await;
    h.run(get("/api/albums")).await;
    h.run(req(
        "POST",
        "/api/albums/1/items/remove",
        json!({"ids": [ids[3].clone()]}),
    ))
    .await;
    h.run(get("/api/download?albumId=1")).await;
    h.run(req("DELETE", "/api/albums/2", Value::Null)).await;
    h.run(req("DELETE", "/api/albums/2", Value::Null)).await;
    h.run(get("/api/albums")).await;

    // ── favorites ──
    h.run(req(
        "PATCH",
        format!("/api/media/{}", ids[0]),
        json!({"favorite": true}),
    ))
    .await;
    h.run(req(
        "POST",
        "/api/media/favorite",
        json!({"ids": ids.iter().skip(1).take(3).collect::<Vec<_>>(), "favorite": true}),
    ))
    .await;
    h.run(req(
        "POST",
        "/api/media/favorite",
        json!({"ids": [], "favorite": true}),
    ))
    .await;
    h.run(get("/api/browse?libraryId=1&recursive=1&kind=favorites"))
        .await;
    h.run(get(format!("/api/media/{}", ids[0]))).await;

    // ── settings ──
    h.run(req(
        "PATCH",
        "/api/settings",
        json!({"editSuffix": "_düzen", "indexIntensity": "low"}),
    ))
    .await;
    h.run(req(
        "PATCH",
        "/api/settings",
        json!({"homeTimeZone": "Bogus/Zone"}),
    ))
    .await;
    h.run(req(
        "PATCH",
        "/api/settings",
        json!({"homeTimeZone": "Europe/Berlin"}),
    ))
    .await;
    h.run(req(
        "PATCH",
        "/api/settings",
        json!({"tileCacheMb": -5, "showHidden": "yes"}),
    ))
    .await;
    h.run(req(
        "PATCH",
        "/api/settings",
        json!({"language": "tr", "theme": "dark"}),
    ))
    .await;
    h.run(get("/api/settings")).await;
    h.run(get("/api/browse?libraryId=1&recursive=1&limit=10"))
        .await;
    h.run(req(
        "PATCH",
        "/api/settings",
        json!({"homeTimeZone": "Europe/Istanbul"}),
    ))
    .await;

    // ── libraries / folders ──
    h.run(req(
        "POST",
        "/api/libraries",
        json!({"name": "Videolar", "color": "#ff0000", "folders": ["<ROOT>/Videolar"]}),
    ))
    .await;
    h.run(req("POST", "/api/libraries", json!({"name": ""})))
        .await;
    h.run(req(
        "POST",
        "/api/libraries",
        json!({"name": "Bad", "folders": ["/definitely/not/here"]}),
    ))
    .await;
    h.run(get("/api/libraries")).await;
    h.run(req("PATCH", "/api/libraries/2", json!({"name": "Klipler", "icon": "film", "options": {"exclude": ["*.mkv"], "minBytes": 10}}))).await;
    h.run(get("/api/libraries")).await;
    h.run(get("/api/browse?libraryId=2&recursive=1")).await;
    h.run(req(
        "POST",
        "/api/libraries/order",
        json!({"ids": ["2", "1"]}),
    ))
    .await;
    h.run(req(
        "POST",
        "/api/libraries/2/folders",
        json!({"path": "<ROOT>/Müzik", "name": "Müzik"}),
    ))
    .await;
    h.run(req(
        "POST",
        "/api/libraries/2/folders",
        json!({"path": "<ROOT>/Müzik"}),
    ))
    .await;
    h.run(get("/api/libraries")).await;
    h.run(req("PATCH", "/api/folders/3", json!({"name": "Sesler"})))
        .await;
    h.run(req("POST", "/api/folders/3/rescan", json!({}))).await;
    h.run(req("DELETE", "/api/folders/3", Value::Null)).await;
    h.run(req("POST", "/api/libraries/2/rescan", json!({})))
        .await;
    h.run(get("/api/libraries/2/stats")).await;
    h.run(req("DELETE", "/api/libraries/2", Value::Null)).await;
    h.run(req("DELETE", "/api/libraries/2", Value::Null)).await;
    h.run(get("/api/libraries")).await;

    // ── file operations ──
    let photo = std::fs::read(fixtures.join("Fotoğraflar/Aile/aile_1.jpg")).unwrap_or_default();
    let put = |path: &str, body: Vec<u8>| Case {
        method: "PUT".into(),
        path: path.into(),
        raw: Some((body, "application/octet-stream".into())),
        settle: true,
        ..Default::default()
    };
    h.run(req(
        "POST",
        "/api/fs/mkdir",
        json!({"folderId": "1", "dir": "", "name": "Yeni Klasör"}),
    ))
    .await;
    h.run(req(
        "POST",
        "/api/fs/mkdir",
        json!({"folderId": "1", "dir": "", "name": "Yeni Klasör"}),
    ))
    .await;
    h.run(req(
        "POST",
        "/api/fs/mkdir",
        json!({"folderId": "1", "dir": "", "name": "../x"}),
    ))
    .await;
    h.run(req(
        "POST",
        "/api/fs/mkdir",
        json!({"folderId": "1", "dir": "../..", "name": "x"}),
    ))
    .await;
    h.run(req(
        "POST",
        "/api/fs/mkdir",
        json!({"folderId": "1", "dir": ""}),
    ))
    .await;
    h.run(req(
        "POST",
        "/api/fs/mkdir",
        json!({"folderId": "99", "dir": "", "name": "x"}),
    ))
    .await;
    for _ in 0..2 {
        h.run(put(
            "/api/upload?folderId=1&dir=Yeni%20Klas%C3%B6r&path=y%C3%BCkle.jpg",
            photo.clone(),
        ))
        .await;
    }
    h.run(put(
        "/api/upload?folderId=1&dir=&path=Alt%2FAlt2%2Fderin.jpg&mtime=1700000000000",
        photo.clone(),
    ))
    .await;
    h.run(put("/api/upload?folderId=1&dir=&path=", photo.clone()))
        .await;
    h.run(put(
        "/api/upload?folderId=1&dir=&path=..%2Fevil.jpg",
        photo.clone(),
    ))
    .await;
    h.run(put(
        "/api/upload?folderId=1&dir=../../etc&path=x.jpg",
        photo.clone(),
    ))
    .await;
    h.run(put(
        "/api/upload?folderId=99&dir=&path=x.jpg",
        photo.clone(),
    ))
    .await;
    h.run(put("/api/upload?dir=&path=x.jpg", photo.clone()))
        .await;
    h.run(get("/api/browse?folderId=1&dir=Yeni%20Klas%C3%B6r"))
        .await;
    h.run(get("/api/browse?folderId=1&dir=Alt%2FAlt2")).await;
    h.run(req(
        "POST",
        "/api/fs/rename",
        json!({"folderId": "1", "rel": "Yeni Klasör/yükle.jpg", "name": "adı değişti.jpg"}),
    ))
    .await;
    h.run(req(
        "POST",
        "/api/fs/rename",
        json!({"folderId": "1", "rel": "Yeni Klasör/adı değişti.jpg", "name": "a/b.jpg"}),
    ))
    .await;
    h.run(req(
        "POST",
        "/api/fs/rename",
        json!({"folderId": "1", "rel": "Yeni Klasör/yükle (1).jpg", "name": "adı değişti.jpg"}),
    ))
    .await;
    h.run(req(
        "POST",
        "/api/fs/rename",
        json!({"folderId": "1", "rel": "", "name": "x"}),
    ))
    .await;
    h.run(req(
        "POST",
        "/api/fs/rename",
        json!({"folderId": "1", "rel": "yok.jpg", "name": "x.jpg"}),
    ))
    .await;
    h.run(req(
        "POST",
        "/api/fs/rename",
        json!({"folderId": "1", "rel": "Yeni Klasör", "name": "Klasör 2"}),
    ))
    .await;
    h.run(get("/api/browse?folderId=1&dir=Klas%C3%B6r%202"))
        .await;
    h.run(req("POST", "/api/fs/move", json!({"refs": [{"folderId": "1", "rel": "Klasör 2/adı değişti.jpg"}], "to": {"folderId": "1", "dir": "Fotoğraflar"}}))).await;
    h.run(req("POST", "/api/fs/move", json!({"refs": [{"folderId": "1", "rel": "Alt/Alt2/derin.jpg"}], "to": {"folderId": "1", "dir": "Fotoğraflar"}, "copy": true}))).await;
    h.run(req("POST", "/api/fs/move", json!({"refs": [{"folderId": "1", "rel": "Alt"}], "to": {"folderId": "1", "dir": "Klasör 2"}}))).await;
    h.run(req("POST", "/api/fs/move", json!({"refs": [{"folderId": "1", "rel": "Klasör 2"}], "to": {"folderId": "1", "dir": "Klasör 2/Alt"}}))).await;
    h.run(req(
        "POST",
        "/api/fs/move",
        json!({"refs": [], "to": {"folderId": "1", "dir": ""}}),
    ))
    .await;
    h.run(req(
        "POST",
        "/api/fs/move",
        json!({"refs": [{"folderId": "1", "rel": "yok.jpg"}], "to": {"folderId": "1", "dir": ""}}),
    ))
    .await;
    h.run(get("/api/browse?folderId=1&dir=Foto%C4%9Fraflar&sort=name"))
        .await;
    h.run(get(
        "/api/browse?folderId=1&dir=Klas%C3%B6r%202%2FAlt%2FAlt2",
    ))
    .await;
    h.run(req(
        "POST",
        "/api/fs/delete",
        json!({"refs": [{"folderId": "1", "rel": "Fotoğraflar/derin.jpg"}]}),
    ))
    .await;
    h.run(req(
        "POST",
        "/api/fs/delete",
        json!({"refs": [{"folderId": "1", "rel": "Klasör 2"}]}),
    ))
    .await;
    h.run(req(
        "POST",
        "/api/fs/delete",
        json!({"refs": [{"folderId": "1", "rel": ""}]}),
    ))
    .await;
    h.run(req(
        "POST",
        "/api/fs/delete",
        json!({"refs": [{"folderId": "1", "rel": "yok.jpg"}]}),
    ))
    .await;
    h.run(get("/api/browse?folderId=1&dir=&sort=name")).await;
    h.run(get("/api/libraries")).await;

    // edits: saved as a copy next to the source
    let png =
        std::fs::read(fixtures.join("Fotoğraflar/Doğa/İstanbul Boğazı 1.png")).unwrap_or_default();
    let src_id = ids.first().cloned().unwrap_or_default();
    for f in ["jpg", "jpg", "webp", "png", "bogus"] {
        h.run(
            put(
                &format!("/api/edits/save?sourceId={src_id}&format={f}"),
                png.clone(),
            )
            .method_post(),
        )
        .await;
    }
    h.run(
        put(
            "/api/edits/save?folderId=1&dir=&name=kolaj.jpg&format=jpg",
            png.clone(),
        )
        .method_post(),
    )
    .await;
    h.run(put("/api/edits/save?sourceId=99999&format=jpg", png.clone()).method_post())
        .await;

    // fonts
    if let Ok(font) = std::fs::read("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf") {
        h.run(put("/api/fonts?name=DejaVuSans.ttf", font.clone()))
            .await;
        h.run(put("/api/fonts?name=x.exe", font)).await;
        h.run(get("/api/fonts")).await;
        h.run(Case {
            binary: true,
            ..get("/api/fonts/1/file")
        })
        .await;
        h.run(req("DELETE", "/api/fonts/1", Value::Null)).await;
        h.run(get("/api/fonts")).await;
    }

    // ── zip ──
    let foto = h
        .json(&h.ts, "/api/browse?folderId=1&dir=Foto%C4%9Fraflar")
        .await;
    let arch = foto["archives"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|a| a["id"].as_str())
        .unwrap_or("0")
        .to_string();
    h.run(req("POST", "/api/zip/peek", json!({"id": arch})))
        .await;
    h.run(req("POST", "/api/zip/peek", json!({"id": src_id})))
        .await;
    h.run(req("POST", "/api/zip/peek", json!({"id": "99999"})))
        .await;
    h.run(get("/api/libraries")).await;
    let libs = h.json(&h.ts, "/api/libraries").await;
    if let Some(t) = libs["temp"].as_array().and_then(|a| a.first()) {
        let tid = t["id"].as_str().unwrap_or("0").to_string();
        h.run(get(format!("/api/browse?folderId={tid}&recursive=1")))
            .await;
        h.run(get(format!(
            "/api/browse/skeleton?folderId={tid}&recursive=1"
        )))
        .await;
        h.run(req(
            "POST",
            format!("/api/temp/{tid}/extend"),
            json!({"hours": 2}),
        ))
        .await;
        h.run(req(
            "POST",
            format!("/api/temp/{tid}/extend"),
            json!({"hours": -1}),
        ))
        .await;
        h.run(req(
            "POST",
            format!("/api/temp/{tid}/keep"),
            json!({"folderId": "1", "dir": "Fotoğraflar"}),
        ))
        .await;
        h.run(get("/api/libraries")).await;
    }
    h.run(req("POST", "/api/zip/extract", json!({"id": arch})))
        .await;
    h.run(get("/api/libraries")).await;
    h.run(get("/api/browse?folderId=1&dir=Foto%C4%9Fraflar&sort=name"))
        .await;
    let z = zip_bytes(&[
        ("a/ç.txt", b"hello"),
        ("b.jpg", &photo),
        ("__MACOSX/._b.jpg", b"x"),
        ("../evil.jpg", b"x"),
    ]);
    h.run(put(
        "/api/upload?folderId=1&dir=&path=test.zip&zip=extract",
        z.clone(),
    ))
    .await;
    h.run(put(
        "/api/upload?folderId=1&dir=&path=test.zip&zip=temp",
        z.clone(),
    ))
    .await;
    h.run(put("/api/upload?folderId=1&dir=&path=test.zip", z.clone()))
        .await;
    h.run(put(
        "/api/upload?folderId=1&dir=&path=bozuk.zip&zip=extract",
        b"not a zip".to_vec(),
    ))
    .await;
    h.run(get("/api/browse?folderId=1&dir=&sort=name")).await;
    h.run(get("/api/browse?folderId=1&dir=test&recursive=1&sort=name"))
        .await;
    h.run(get("/api/libraries")).await;

    // ── caches ──
    h.run(req("DELETE", "/api/cache/bogus", Value::Null)).await;
    h.run(req("DELETE", "/api/tiles/cache", Value::Null)).await;
    h.run(get("/api/tiles/cache")).await;
    h.run(get("/api/tiles/bogus/1/0/0")).await;
    h.run(get("/api/tiles/osm/99/0/0")).await;
    h.run(get("/api/tiles/osm/1/5/0")).await;

    // ── jobs / devices / remote (validation paths) ──
    h.run(req("DELETE", "/api/jobs/nope", Value::Null)).await;
    h.run(req("DELETE", "/api/jobs", Value::Null)).await;
    h.run(req("POST", "/api/devices/nope/approve", json!({})))
        .await;
    h.run(req("DELETE", "/api/devices/nope", Value::Null)).await;
    h.run(req("POST", "/api/remote/folders", json!({"url": "bogus"})))
        .await;
    h.run(req("POST", "/api/remote/inspect", json!({"url": "bogus"})))
        .await;
    // MEGA link validation (unknown links reach MEGA: needs network, PEEK_CONF_OFFLINE skips them)
    let mut megas = vec![
        "https://mega.nz/folder/AbCdEfGh",
        "mega.nz/#F!AbCdEfGh",
        "https://example.com/x",
        "https://mega.nz/file/AbCdEfGh#short",
    ];
    if std::env::var("PEEK_CONF_OFFLINE").is_err() {
        megas.extend([
            "https://mega.nz/folder/AbCdEfGh#AbCdEfGhIjKlMnOpQrStUv",
            "https://mega.nz/file/AbCdEfGh#AbCdEfGhIjKlMnOpQrStUvWxYz0123456789abcdefg",
        ]);
    }
    for u in megas {
        h.run(req("POST", "/api/remote/inspect", json!({ "url": u })))
            .await;
        h.run(req(
            "POST",
            "/api/remote/folders",
            json!({ "url": u, "libraryId": "1" }),
        ))
        .await;
        h.run(req(
            "POST",
            "/api/remote/import",
            json!({ "url": u, "to": {"folderId": "1", "dir": ""} }),
        ))
        .await;
    }
    h.run(req(
        "POST",
        "/api/remote/copy",
        json!({"ids": ["1"], "to": {"folderId": "1", "dir": ""}}),
    ))
    .await;
    h.run(req("POST", "/api/remote/copy", json!({"ids": ["1"]})))
        .await;
    h.run(req("POST", "/api/remote/folders/1/refresh", json!({})))
        .await;
    h.run(req(
        "POST",
        "/api/remote/account/login",
        json!({"email": "x"}),
    ))
    .await;
    h.run(req("POST", "/api/remote/account/logout", json!({})))
        .await;
    h.run(req("POST", "/api/meta/reread", json!({"libraryId": "1"})))
        .await;
    h.run(req(
        "POST",
        "/api/temp/99/keep",
        json!({"folderId": "1", "dir": ""}),
    ))
    .await;
    h.run(req("POST", "/api/temp/99/extend", json!({"hours": 2})))
        .await;
    h.run(req(
        "POST",
        "/api/host/mkdir",
        json!({"path": "<ROOT>", "name": "host-dir"}),
    ))
    .await;
    h.run(req(
        "POST",
        "/api/host/mkdir",
        json!({"path": "/", "name": "x"}),
    ))
    .await;
    h.run(req("POST", "/api/import-local", json!({"paths": []})))
        .await;

    // ── rate limit (HEAVY: 60/min) ──
    for i in 0..62 {
        let c = Case {
            external: true,
            ..get("/api/host/dirs?path=<ROOT>")
        };
        if i >= 58 {
            h.run(c).await;
        } else {
            let _ = h.send(&h.ts, &c).await;
            let _ = h.send(&h.rs, &c).await;
        }
    }

    let summary = format!(
        "conformance: {} cases, {} with differences",
        h.cases, h.diffs
    );
    let full = format!("{summary}\n\n{}", h.report.join("\n"));
    if let Ok(out) = std::env::var("PEEK_CONF_OUT") {
        let _ = std::fs::write(out, &full);
    }
    println!("{full}");
    srv.close().await;
    drop(_ts_guard);
    let _ = std::fs::remove_dir_all(&work);
    if std::env::var("PEEK_CONF_STRICT").is_ok() {
        assert_eq!(h.diffs, 0, "{summary}");
    }
}
