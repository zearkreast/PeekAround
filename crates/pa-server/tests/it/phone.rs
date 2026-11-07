//! A Rust fake phone (copied from crates/pa-link/tests/common — itself a port of
//! packages/server/test/helpers/fake-phone.ts): speaks the link protocol against the real server.
#![allow(dead_code)]

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use pa_core::{MediaKind, ServerEvent};
use pa_jobs::MemoryEvents;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, Notify};

pub async fn until<T>(mut f: impl FnMut() -> Option<T>, ms: u64) -> T {
    let end = std::time::Instant::now() + Duration::from_millis(ms);
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(std::time::Instant::now() < end, "condition not met in time");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

pub fn pair_events(ev: &MemoryEvents) -> Vec<pa_core::DeviceInfo> {
    ev.snapshot()
        .into_iter()
        .filter_map(|e| {
            if let ServerEvent::Pair { device } = e {
                Some(device)
            } else {
                None
            }
        })
        .collect()
}

// ───────────────────────── fake phone ─────────────────────────

pub fn virtual_byte(i: u64) -> u8 {
    ((i * 31 + (i >> 8)) & 0xff) as u8
}

pub fn virtual_bytes(start: u64, end: u64) -> Vec<u8> {
    (start..end).map(virtual_byte).collect()
}

pub fn sha(b: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(b))
}

#[derive(Clone)]
pub struct PhoneFolder {
    pub key: String,
    pub name: String,
    pub root: PathBuf,
}

#[derive(Clone, Copy)]
pub struct VirtualFile {
    pub size: u64,
    pub mtime: i64,
}

pub type Intercept = Box<dyn FnMut(&Value) -> bool + Send>;

pub struct PhoneState {
    pub frames: Vec<Value>,
    pub token: Option<String>,
    pub close_code: Option<u16>,
    pub list_body: Option<String>,
    pub virtual_files: HashMap<String, VirtualFile>,
    pub ignore: HashSet<String>,
    pub intercept: Option<Intercept>,
    pub intercept_gen: u64,
    pub puts: HashMap<String, tokio::sync::watch::Receiver<Option<(u16, u64)>>>,
    pub last_pull_headers: HashMap<String, String>,
    pub written: u64,
    pub closed: bool,
    pub conn_gen: u64,
}

#[derive(Clone)]
pub struct FakePhone {
    pub base: String,
    pub id: String,
    pub folders: Vec<PhoneFolder>,
    pub name: String,
    pub caps: (bool, bool),
    pub st: Arc<Mutex<PhoneState>>,
    pub notify: Arc<Notify>,
    out: Arc<Mutex<Option<mpsc::UnboundedSender<tokio_tungstenite::tungstenite::Message>>>>,
    pub http: reqwest::Client,
}

impl FakePhone {
    pub fn new(
        base: &str,
        id: &str,
        folders: Vec<PhoneFolder>,
        token: Option<String>,
        name: &str,
    ) -> Self {
        FakePhone {
            base: base.into(),
            id: id.into(),
            folders,
            name: name.into(),
            caps: (true, true),
            st: Arc::new(Mutex::new(PhoneState {
                frames: Vec::new(),
                token,
                close_code: None,
                list_body: None,
                virtual_files: HashMap::new(),
                ignore: HashSet::new(),
                intercept: None,
                intercept_gen: 0,
                puts: HashMap::new(),
                last_pull_headers: HashMap::new(),
                written: 0,
                closed: false,
                conn_gen: 0,
            })),
            notify: Arc::new(Notify::new()),
            out: Arc::default(),
            http: reqwest::Client::new(),
        }
    }

    pub fn token(&self) -> Option<String> {
        self.st.lock().unwrap().token.clone()
    }

    pub fn hello_frame(&self) -> Value {
        json!({
            "t": "hello", "v": 1, "id": self.id, "name": self.name, "model": "Google Pixel 8", "app": "1.1.0",
            "folders": self.folders.iter().map(|f| json!({ "key": f.key, "name": f.name, "path": format!("/storage/emulated/0/{}", f.name) })).collect::<Vec<_>>(),
            "caps": { "read": self.caps.0, "receive": self.caps.1 },
        })
    }

    /// open the socket, send hello; resolves with the server's first answer (welcome / pending / rejected)
    pub async fn connect(&self, hello: Option<Value>, extra: &[(&str, &str)]) -> Value {
        let url = format!(
            "{}/api/devices/connect?v=1",
            self.base.replacen("http", "ws", 1)
        );
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut req = url.into_client_request().unwrap();
        req.headers_mut()
            .insert("X-Peek-Device", self.id.parse().unwrap());
        if let Some(t) = self.token() {
            req.headers_mut()
                .insert("Authorization", format!("Bearer {t}").parse().unwrap());
        }
        for (k, v) in extra {
            let name = axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap();
            req.headers_mut().insert(name, v.parse().unwrap());
        }
        let gen = {
            let mut s = self.st.lock().unwrap();
            s.close_code = None;
            s.closed = false;
            s.conn_gen += 1;
            s.conn_gen
        };
        let (ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
        let (mut tx, mut rx) = ws.split();
        let (otx, mut orx) = mpsc::unbounded_channel();
        *self.out.lock().unwrap() = Some(otx.clone());
        tokio::spawn(async move {
            while let Some(m) = orx.recv().await {
                if tx.send(m).await.is_err() {
                    break;
                }
            }
            let _ = tx.close().await;
        });
        let me = self.clone();
        tokio::spawn(async move {
            use tokio_tungstenite::tungstenite::Message;
            let mut code = 1006u16;
            while let Some(m) = rx.next().await {
                match m {
                    Ok(Message::Text(t)) => {
                        let f: Value = serde_json::from_str(t.as_str()).unwrap();
                        me.on_frame(f);
                    }
                    Ok(Message::Close(c)) => {
                        code = c.map(|c| u16::from(c.code)).unwrap_or(1005);
                        break;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
            // only the current socket's close counts
            let mut s = me.st.lock().unwrap();
            if s.conn_gen == gen {
                s.close_code = Some(code);
                s.closed = true;
            }
            drop(s);
            me.notify.notify_waiters();
        });
        self.send(&hello.unwrap_or_else(|| self.hello_frame()));
        self.wait_frame(
            |f| matches!(f["t"].as_str(), Some("welcome" | "pending" | "rejected")),
            10_000,
        )
        .await
    }

    pub fn send(&self, f: &Value) {
        if let Some(o) = self.out.lock().unwrap().as_ref() {
            let _ = o.send(tokio_tungstenite::tungstenite::Message::Text(
                f.to_string().into(),
            ));
        }
    }

    pub fn close(&self) {
        // dropping the sender ends the writer task, which closes the socket
        *self.out.lock().unwrap() = None;
    }

    pub async fn closed(&self) -> u16 {
        loop {
            let n = self.notify.notified();
            if let Some(c) = self.st.lock().unwrap().close_code {
                return c;
            }
            tokio::time::timeout(Duration::from_secs(10), n)
                .await
                .expect("socket close");
        }
    }

    pub async fn wait_frame(&self, pred: impl Fn(&Value) -> bool, ms: u64) -> Value {
        let end = tokio::time::Instant::now() + Duration::from_millis(ms);
        loop {
            let n = self.notify.notified();
            {
                let mut s = self.st.lock().unwrap();
                if let Some(i) = s.frames.iter().position(&pred) {
                    return s.frames.remove(i);
                }
            }
            if tokio::time::timeout_at(end, n).await.is_err() {
                panic!(
                    "timeout waiting for frame; have {:?}",
                    self.st.lock().unwrap().frames
                );
            }
        }
    }

    pub fn set_intercept(&self, i: Option<Intercept>) {
        let mut s = self.st.lock().unwrap();
        s.intercept = i;
        s.intercept_gen += 1;
    }

    pub fn clear_frames(&self) {
        self.st.lock().unwrap().frames.clear();
    }

    fn on_frame(&self, f: Value) {
        if f["t"] == "paired" {
            self.st.lock().unwrap().token = f["token"].as_str().map(str::to_string);
        }
        self.st.lock().unwrap().frames.push(f.clone());
        self.notify.notify_waiters();
        if f["t"] == "req" {
            let me = self.clone();
            tokio::spawn(async move { me.handle(f).await });
        }
    }

    fn auth(&self) -> Vec<(String, String)> {
        vec![
            (
                "Authorization".into(),
                format!("Bearer {}", self.token().unwrap_or_default()),
            ),
            ("X-Peek-Device".into(), self.id.clone()),
        ]
    }

    fn folder(&self, key: &str) -> &PhoneFolder {
        self.folders
            .iter()
            .find(|f| f.key == key)
            .expect("unknown key")
    }

    /// phone-side path guard (relative, no .., stays inside the root)
    fn resolve(&self, key: &str, rel: &str) -> Option<PathBuf> {
        if rel.is_empty() || rel.starts_with('/') || rel.split('/').any(|s| s == "..") {
            return None;
        }
        let mut p = self.folder(key).root.clone();
        for s in rel.split('/') {
            p.push(s);
        }
        Some(p)
    }

    pub fn err(&self, rid: &str, status: u16, message: &str) {
        self.send(&json!({ "t": "err", "rid": rid, "status": status, "message": message }));
    }

    pub async fn handle(&self, f: Value) {
        let taken = {
            let mut s = self.st.lock().unwrap();
            s.intercept.take().map(|i| (i, s.intercept_gen))
        };
        if let Some((mut i, gen)) = taken {
            let took = i(&f);
            let mut s = self.st.lock().unwrap();
            if s.intercept_gen == gen && s.intercept.is_none() {
                s.intercept = Some(i);
            }
            if took {
                return;
            }
        }
        if self
            .st
            .lock()
            .unwrap()
            .ignore
            .contains(f["op"].as_str().unwrap_or(""))
        {
            return;
        }
        let rid = f["rid"].as_str().unwrap().to_string();
        let a = &f["args"];
        let key = a["key"].as_str().unwrap_or("");
        if !self.folders.iter().any(|x| x.key == key) {
            return self.err(&rid, 404, "unknown folder");
        }
        match f["op"].as_str().unwrap() {
            "list" => {
                let body = self.st.lock().unwrap().list_body.take();
                let body = match body {
                    Some(b) => b,
                    None => self.walk(key, a["dir"].as_str().unwrap_or("")),
                };
                self.reply(
                    &rid,
                    vec![("Content-Type", "application/x-ndjson".into())],
                    Bytes::from(body).into(),
                );
            }
            "thumb" | "display" => {
                let Some(abs) = self.resolve(key, a["p"].as_str().unwrap_or("")) else {
                    return self.err(&rid, 403, "outside shared folder");
                };
                // a real JPEG of the file, long edge ≤ s / max (like the TS fake phone's sharp)
                let size = a["s"].as_u64().or(a["max"].as_u64()).unwrap_or(640) as u32;
                let Ok(img) = image::open(&abs) else {
                    return self.err(&rid, 415, "not an image");
                };
                let img = img.thumbnail(size, size).to_rgb8();
                let mut out = Vec::new();
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80)
                    .encode_image(&img)
                    .unwrap();
                let len = out.len();
                self.reply(
                    &rid,
                    vec![
                        ("Content-Type", "image/jpeg".into()),
                        ("Content-Length", len.to_string()),
                    ],
                    Bytes::from(out).into(),
                );
            }
            "strip" => {
                let jpg = Bytes::from_static(&[0xff, 0xd8, 0xff, 0xd9]);
                self.reply(
                    &rid,
                    vec![
                        ("Content-Type", "image/jpeg".into()),
                        ("Content-Length", "4".into()),
                    ],
                    jpg.into(),
                );
            }
            "details" => {
                let body = json!({ "exif": { "Make": "Google", "Model": "Pixel 8", "FNumber": 1.7 }, "gps": { "lat": 40.99, "lon": 29.03 }, "make": "Google", "model": "Pixel 8", "lens": null }).to_string();
                let len = body.len().to_string();
                self.reply(
                    &rid,
                    vec![
                        ("Content-Type", "application/json".into()),
                        ("Content-Length", len),
                    ],
                    Bytes::from(body).into(),
                );
            }
            "file" => self.file(&rid, a).await,
            "receive" => self.receive(&rid, a).await,
            _ => self.err(&rid, 400, "unknown op"),
        }
    }

    fn walk(&self, key: &str, dir: &str) -> String {
        let f = self.folder(key).clone();
        let mut lines = Vec::new();
        fn go(abs: &std::path::Path, rel: &str, lines: &mut Vec<String>) {
            let mut entries: Vec<_> = std::fs::read_dir(abs)
                .map(|r| r.filter_map(|e| e.ok()).collect())
                .unwrap_or_default();
            entries.sort_by_key(|e| e.file_name());
            if entries.is_empty() && !rel.is_empty() {
                lines.push(json!({ "dir": rel }).to_string());
            }
            for e in entries {
                let name = e.file_name().to_string_lossy().into_owned();
                let p = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}/{name}")
                };
                if e.file_type().unwrap().is_dir() {
                    go(&e.path(), &p, lines);
                } else if !name.ends_with(".part") {
                    let st = e.metadata().unwrap();
                    let m = st
                        .modified()
                        .unwrap()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as i64;
                    lines.push(json!({ "p": p, "k": pa_core::kind_of(&name).map(|k| k.as_str()), "s": st.len(), "m": m, "w": null, "h": null, "d": null, "t": null, "id": null }).to_string());
                }
            }
        }
        let start = if dir.is_empty() {
            f.root.clone()
        } else {
            f.root.join(dir)
        };
        go(&start, dir, &mut lines);
        for (k, v) in self.st.lock().unwrap().virtual_files.iter() {
            let (vk, p) = k.split_once('/').unwrap();
            if vk == key && (dir.is_empty() || p.starts_with(&format!("{dir}/"))) {
                lines.push(json!({ "p": p, "k": pa_core::kind_of(p).map(|k| k.as_str()), "s": v.size, "m": v.mtime, "w": 1920, "h": 1080, "d": 60, "t": v.mtime, "id": null }).to_string());
            }
        }
        lines.join("\n") + "\n"
    }

    async fn file(&self, rid: &str, a: &Value) {
        let key = a["key"].as_str().unwrap();
        let p = a["p"].as_str().unwrap();
        let v = self
            .st
            .lock()
            .unwrap()
            .virtual_files
            .get(&format!("{key}/{p}"))
            .copied();
        let (size, mtime, abs) = if let Some(v) = v {
            (v.size, v.mtime, None)
        } else {
            let Some(abs) = self.resolve(key, p) else {
                return self.err(rid, 403, "outside shared folder");
            };
            let Ok(st) = std::fs::metadata(&abs) else {
                return self.err(rid, 404, "not found");
            };
            let m = st
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            (st.len(), m, Some(abs))
        };
        let (mut start, mut end, mut status) = (0u64, size.saturating_sub(1), 200);
        if let Some(r) = a["range"].as_str() {
            let (x, y) = r.strip_prefix("bytes=").unwrap().split_once('-').unwrap();
            if x.is_empty() {
                start = size.saturating_sub(y.parse().unwrap());
            } else {
                start = x.parse().unwrap();
                if !y.is_empty() {
                    end = (size - 1).min(y.parse().unwrap());
                }
            }
            if start >= size || start > end {
                self.reply(
                    rid,
                    vec![
                        ("X-Peek-Status", "416".into()),
                        ("X-Peek-Content-Range", format!("bytes */{size}")),
                        ("X-Peek-Total", size.to_string()),
                        ("Content-Length", "0".into()),
                    ],
                    Bytes::new().into(),
                );
                return;
            }
            status = 206;
        }
        let len = end - start + 1;
        let mut h = vec![
            ("X-Peek-Status", status.to_string()),
            (
                "Content-Type",
                if pa_core::kind_of(p) == Some(MediaKind::Video) {
                    "video/mp4".into()
                } else {
                    "application/octet-stream".into()
                },
            ),
            ("Content-Length", len.to_string()),
            ("X-Peek-Total", size.to_string()),
            ("X-Peek-Mtime", mtime.to_string()),
        ];
        if status == 206 {
            h.push((
                "X-Peek-Content-Range",
                format!("bytes {start}-{end}/{size}"),
            ));
        }
        let body = match abs {
            Some(abs) => {
                let data = std::fs::read(abs).unwrap();
                Bytes::from(data[start as usize..=end as usize].to_vec()).into()
            }
            None => {
                let st = self.st.clone();
                let stream = futures_util::stream::unfold(start, move |pos| {
                    let st = st.clone();
                    async move {
                        if pos > end {
                            return None;
                        }
                        let n = (64 * 1024).min(end + 1 - pos);
                        let b = virtual_bytes(pos, pos + n);
                        st.lock().unwrap().written += n;
                        Some((Ok::<_, std::io::Error>(Bytes::from(b)), pos + n))
                    }
                });
                reqwest::Body::wrap_stream(stream)
            }
        };
        self.reply(rid, h, body);
    }

    async fn receive(&self, rid: &str, a: &Value) {
        let key = a["key"].as_str().unwrap();
        let dir = a["dir"].as_str().unwrap_or("");
        let dir_abs = if dir.is_empty() {
            Some(self.folder(key).root.clone())
        } else {
            self.resolve(key, dir)
        };
        let Some(dir_abs) = dir_abs else {
            return self.err(rid, 403, "outside shared folder");
        };
        std::fs::create_dir_all(&dir_abs).unwrap();
        let mut req = self
            .http
            .get(format!("{}{}", self.base, a["url"].as_str().unwrap()));
        for (k, v) in self.auth() {
            req = req.header(k, v);
        }
        let res = req.send().await.unwrap();
        if res.status() != 200 {
            return self.err(rid, 502, &format!("pull failed {}", res.status().as_u16()));
        }
        let headers: HashMap<String, String> = res
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap().to_string()))
            .collect();
        let body = res.bytes().await.unwrap();
        self.st.lock().unwrap().last_pull_headers = headers.clone();
        let name0 = a["name"].as_str().unwrap();
        let ext = pa_jobs::paths::extname(name0);
        let mut name = name0.to_string();
        let mut i = 1;
        while dir_abs.join(&name).exists() {
            name = format!("{} ({i}){ext}", &name0[..name0.len() - ext.len()]);
            i += 1;
        }
        std::fs::write(dir_abs.join(&name), &body).unwrap();
        let mt = headers
            .get("x-peek-mtime")
            .and_then(|m| m.parse::<i64>().ok())
            .unwrap_or(a["mtime"].as_i64().unwrap());
        pa_jobs::paths::set_mtime(&dir_abs.join(&name), mt).await;
        let p = if dir.is_empty() {
            name
        } else {
            format!("{dir}/{name}")
        };
        self.send(&json!({ "t": "res", "rid": rid, "ok": true, "data": { "p": p } }));
        self.send(&json!({ "t": "changed", "key": key, "dirs": [dir] }));
    }

    /// stream a reply PUT; the result (status, bytes sent) lands in `puts[rid]`
    pub fn reply(&self, rid: &str, headers: Vec<(&str, String)>, body: reqwest::Body) {
        let (tx, rx) = tokio::sync::watch::channel(None);
        self.st.lock().unwrap().puts.insert(rid.to_string(), rx);
        let me = self.clone();
        let rid = rid.to_string();
        let headers: Vec<(String, String)> = headers
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        tokio::spawn(async move {
            let auth = me.auth();
            let r = me.put(&rid, headers, body, auth).await;
            let _ = tx.send(Some(r));
        });
    }

    pub async fn put(
        &self,
        rid: &str,
        headers: Vec<(String, String)>,
        body: reqwest::Body,
        auth: Vec<(String, String)>,
    ) -> (u16, u64) {
        let mut req = self
            .http
            .put(format!("{}/api/devices/reply/{rid}", self.base));
        for (k, v) in auth.into_iter().chain(headers) {
            req = req.header(k, v);
        }
        let before = self.st.lock().unwrap().written;
        match req.body(body).send().await {
            Ok(r) => (
                r.status().as_u16(),
                self.st.lock().unwrap().written - before,
            ),
            Err(_) => (0, self.st.lock().unwrap().written - before),
        }
    }

    pub async fn put_result(&self, rid: &str) -> (u16, u64) {
        let mut rx = until(|| self.st.lock().unwrap().puts.get(rid).cloned(), 10_000).await;
        let v = rx.wait_for(|v| v.is_some()).await.unwrap();
        v.unwrap()
    }
}
