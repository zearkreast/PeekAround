//! Public-link client (what `megaClient()` / `wrapMega` / `megaRead` / `megaFileAttr` do on top
//! of megajs): listing a file or folder link, ranged decrypted reads from any offset, whole-file
//! downloads with MAC verification, file attributes (thumbnail / preview), download-URL caching.

use crate::api::{Api, ApiOptions};
use crate::crypto::{self, b64url, from_b64, CtrRange, MacState};
use crate::error::{HttpError, MegaError, EACCESS, EBLOCKED, EKEY, ENOENT, EOVERQUOTA};
use crate::link::MegaLink;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// A node of a loaded link (plain data; transfers go through [`MegaClient`]).
#[derive(Debug, Clone, Serialize)]
pub struct MegaNode {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub directory: bool,
    /// ms since the epoch (MEGA `ts`)
    pub timestamp: Option<i64>,
    pub children: Vec<MegaNode>,
    /// raw MEGA file attribute list ("<cluster>:<type>*<handle>/…")
    pub fa: Option<String>,
    /// decrypted node key (32 bytes for files, 16 for folders); never serialized
    #[serde(skip)]
    pub key: Option<Vec<u8>>,
}

impl MegaNode {
    /// depth-first search by id
    pub fn find(&self, id: &str) -> Option<&MegaNode> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(id))
    }
}

/// decrypted bytes, chunk by chunk; drop the stream to cancel the transfer
pub type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes, MegaError>> + Send>>;

#[derive(Clone)]
pub struct MegaClient {
    api: Api,
    urls: Arc<Mutex<HashMap<String, (String, Instant)>>>,
    /// back-off before retry `n` of a ranged read: `read_retry_unit × 2^n` (500 ms in production)
    pub read_retry_unit: Duration,
}

const DL_URL_TTL: Duration = Duration::from_secs(600);

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl MegaClient {
    pub fn new(opts: ApiOptions) -> Self {
        Self {
            api: Api::new(opts),
            urls: Arc::new(Mutex::new(HashMap::new())),
            read_retry_unit: Duration::from_millis(250),
        }
    }

    pub fn api(&self) -> &Api {
        &self.api
    }

    /// forget cached download URLs (login / logout: new URLs are accounted to the new session)
    pub fn clear_download_urls(&self) {
        self.urls.lock().unwrap().clear();
    }

    fn folder_query(link: &MegaLink) -> Vec<(&'static str, String)> {
        if link.is_folder() {
            vec![("n", link.id.clone())]
        } else {
            Vec::new()
        }
    }

    async fn request(&self, link: &MegaLink, cmd: Value) -> Result<Value, MegaError> {
        let q = Self::folder_query(link);
        let q: Vec<(&str, &str)> = q.iter().map(|(k, v)| (*k, v.as_str())).collect();
        self.api.request(&cmd, &q).await
    }

    /// Load the shared node of a link: a file link's node, a folder link's whole tree, or the
    /// `sub` node of a folder link.
    pub async fn load(&self, link: &MegaLink) -> Result<MegaNode, MegaError> {
        let key = from_b64(&link.key);
        if link.is_folder() {
            if key.len() != 16 {
                return Err(MegaError::Key("wrong key length".into()));
            }
            let resp = self
                .request(link, json!({ "a": "f", "c": 1, "ca": 1, "r": 1 }))
                .await?;
            let nodes = resp
                .get("f")
                .and_then(Value::as_array)
                .ok_or_else(|| MegaError::Invalid("MEGA returned no folder listing".into()))?;
            build_folder(nodes, &key, link)
        } else {
            if key.len() != 32 {
                return Err(MegaError::Key("wrong key length".into()));
            }
            let resp = self
                .request(link, json!({ "a": "g", "p": link.id }))
                .await?;
            let at = resp.get("at").and_then(Value::as_str).unwrap_or("");
            let attrs = crypto::decrypt_attributes(&key, at).ok_or_else(|| {
                MegaError::Key("Attributes could not be decrypted with provided key.".into())
            })?;
            Ok(MegaNode {
                id: link.id.clone(),
                name: attrs
                    .get("n")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("untitled")
                    .to_string(),
                size: resp.get("s").and_then(Value::as_u64).unwrap_or(0),
                directory: false,
                timestamp: None,
                children: Vec::new(),
                fa: resp.get("fa").and_then(Value::as_str).map(str::to_string),
                key: Some(key),
            })
        }
    }

    /// Temporary download URL of a node ("g"), cached 10 min per link/node.
    pub async fn download_url(
        &self,
        link: &MegaLink,
        node_id: &str,
        fresh: bool,
    ) -> Result<String, MegaError> {
        let k = format!("{}/{}", link.id, node_id);
        if !fresh {
            if let Some((u, at)) = self.urls.lock().unwrap().get(&k) {
                if at.elapsed() < DL_URL_TTL {
                    return Ok(u.clone());
                }
            }
        }
        let cmd = if link.is_folder() {
            json!({ "a": "g", "g": 1, "ssl": 2, "n": node_id })
        } else {
            json!({ "a": "g", "g": 1, "ssl": 2, "p": link.id })
        };
        let resp = self.request(link, cmd).await?;
        let g = resp
            .get("g")
            .and_then(Value::as_str)
            .filter(|g| g.starts_with("http:") || g.starts_with("https:"));
        let Some(g) = g else {
            let e = resp.get("e").and_then(Value::as_i64).unwrap_or(0);
            let tl = resp.get("tl").and_then(Value::as_u64).unwrap_or(0);
            if e == EOVERQUOTA as i64 || tl > 0 {
                return Err(MegaError::Quota {
                    seconds: (tl > 0).then_some(tl),
                });
            }
            return Err(MegaError::Invalid(if e != 0 {
                format!("MEGA error {e}")
            } else {
                "MEGA servers returned an invalid response, maybe caused by rate limit".into()
            }));
        };
        let mut m = self.urls.lock().unwrap();
        if m.len() > 5000 {
            m.clear();
        }
        m.insert(k, (g.to_string(), Instant::now()));
        Ok(g.to_string())
    }

    fn quota_error(res: &reqwest::Response) -> MegaError {
        let left = res
            .headers()
            .get("x-mega-time-left")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .filter(|s| *s > 0);
        MegaError::Quota { seconds: left }
    }

    /// Open the ciphertext of `[aligned, end]` (one request, retried with a fresh URL before the
    /// first byte: expired URL → 403/404, congestion → 5xx).
    async fn open_range(
        &self,
        link: &MegaLink,
        node_id: &str,
        aligned: u64,
        end: u64,
        cancel: &CancellationToken,
    ) -> Result<reqwest::Response, MegaError> {
        let mut last: Option<MegaError> = None;
        for attempt in 0..4u32 {
            if cancel.is_cancelled() {
                return Err(MegaError::Aborted);
            }
            if attempt > 0 {
                tokio::select! {
                    _ = cancel.cancelled() => return Err(MegaError::Aborted),
                    _ = tokio::time::sleep(self.read_retry_unit * 2u32.pow(attempt)) => {}
                }
            }
            let url = match self.download_url(link, node_id, attempt > 0).await {
                Ok(u) => u,
                Err(e) => {
                    let fatal = matches!(e, MegaError::Quota { .. } | MegaError::Aborted)
                        || matches!(
                            e.code(),
                            Some(ENOENT)
                                | Some(EBLOCKED)
                                | Some(EACCESS)
                                | Some(EKEY)
                                | Some(EOVERQUOTA)
                        );
                    if fatal {
                        return Err(e);
                    }
                    last = Some(e);
                    continue;
                }
            };
            let res = tokio::select! {
                _ = cancel.cancelled() => return Err(MegaError::Aborted),
                r = self.api.http.get(format!("{url}/{aligned}-{end}")).send() => r,
            };
            let res = match res {
                Ok(r) => r,
                Err(e) => {
                    last = Some(e.into());
                    continue;
                }
            };
            if res.status().as_u16() == 509 {
                return Err(Self::quota_error(&res));
            }
            if !res.status().is_success() {
                last = Some(MegaError::Http(res.status().as_u16()));
                continue;
            }
            return Ok(res);
        }
        Err(last.unwrap_or_else(|| MegaError::Invalid("MEGA download failed".into())))
    }

    /// Decrypted bytes `[start, end]` (inclusive, clamped to the file) of a file node — random
    /// access for streaming. One ciphertext request from the 16-byte aligned offset.
    pub fn read(
        &self,
        link: &MegaLink,
        node: &MegaNode,
        start: u64,
        end: u64,
        cancel: CancellationToken,
    ) -> Result<ByteStream, MegaError> {
        if node.directory {
            return Err(HttpError::new(400, "Not a file").into());
        }
        let key = node
            .key
            .clone()
            .filter(|k| k.len() >= 32)
            .ok_or_else(|| MegaError::Key("key isn't defined".into()))?;
        let end = end.min(node.size.saturating_sub(1));
        if node.size == 0 || start > end {
            return Err(HttpError::new(416, "Range not satisfiable").into());
        }
        let (tx, rx) = mpsc::channel::<Result<Bytes, MegaError>>(8);
        let (this, link, id) = (self.clone(), link.clone(), node.id.clone());
        tokio::spawn(async move {
            let r = this
                .pump_range(&link, &id, &key, start, end, &cancel, &tx, None)
                .await;
            if let Err(e) = r {
                let _ = tx.send(Err(e)).await;
            }
        });
        Ok(channel_stream(rx))
    }

    /// one range into `tx`; returns the bytes delivered (for resumes)
    #[allow(clippy::too_many_arguments)]
    async fn pump_range(
        &self,
        link: &MegaLink,
        id: &str,
        key: &[u8],
        start: u64,
        end: u64,
        cancel: &CancellationToken,
        tx: &mpsc::Sender<Result<Bytes, MegaError>>,
        mut mac: Option<&mut MacState>,
    ) -> Result<(), MegaError> {
        let aligned = start - start % 16;
        let res = self.open_range(link, id, aligned, end, cancel).await?;
        let mut dec = CtrRange::new(key, aligned, start, end).map_err(MegaError::Key)?;
        let mut body = res.bytes_stream();
        let mut carry: Vec<u8> = Vec::new();
        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => return Err(MegaError::Aborted),
                _ = tx.closed() => return Err(MegaError::Aborted),
                c = body.next() => c,
            };
            let Some(chunk) = chunk else { break };
            let chunk = chunk?;
            // keep CTR on 16-byte boundaries across chunks: decrypt whole blocks only
            carry.extend_from_slice(&chunk);
            let whole = carry.len() / 16 * 16;
            if whole == 0 {
                continue;
            }
            let mut blocks: Vec<u8> = carry.drain(..whole).collect();
            let out = dec.update(&mut blocks);
            if !out.is_empty() {
                if let Some(m) = mac.as_deref_mut() {
                    m.update(out);
                }
                if tx.send(Ok(Bytes::copy_from_slice(out))).await.is_err() {
                    return Err(MegaError::Aborted);
                }
            }
            if dec.remaining() == 0 {
                return Ok(());
            }
        }
        if !carry.is_empty() {
            let out = dec.update(&mut carry);
            if !out.is_empty() {
                if let Some(m) = mac {
                    m.update(out);
                }
                if tx.send(Ok(Bytes::copy_from_slice(out))).await.is_err() {
                    return Err(MegaError::Aborted);
                }
            }
        }
        if dec.remaining() > 0 {
            return Err(MegaError::Network(format!(
                "MEGA closed the transfer {} bytes early",
                dec.remaining()
            )));
        }
        Ok(())
    }

    /// The whole decrypted file, MAC-verified at the end (an `Err(Mac)` item instead of the end).
    /// Interrupted transfers resume from where they stopped (up to 8 times, 1 s × 2^n back-off).
    pub fn download(
        &self,
        link: &MegaLink,
        node: &MegaNode,
        cancel: CancellationToken,
    ) -> Result<ByteStream, MegaError> {
        if node.directory {
            return Err(HttpError::new(400, "Not a file").into());
        }
        let key = node
            .key
            .clone()
            .filter(|k| k.len() >= 32)
            .ok_or_else(|| MegaError::Key("key isn't defined".into()))?;
        let (tx, rx) = mpsc::channel::<Result<Bytes, MegaError>>(8);
        let (this, link, id, size) = (self.clone(), link.clone(), node.id.clone(), node.size);
        tokio::spawn(async move {
            if size == 0 {
                return;
            }
            let mut mac = MacState::new(&key).expect("file key");
            let mut pos = 0u64;
            let mut tries = 0u32;
            while pos < size {
                // count what this attempt delivers through a forwarding channel
                let (itx, mut irx) = mpsc::channel::<Result<Bytes, MegaError>>(8);
                let fwd = async {
                    let mut n = 0u64;
                    while let Some(item) = irx.recv().await {
                        match item {
                            Ok(b) => {
                                n += b.len() as u64;
                                if tx.send(Ok(b)).await.is_err() {
                                    return (n, false);
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    (n, true)
                };
                let pump = async {
                    let r = this
                        .pump_range(
                            &link,
                            &id,
                            &key,
                            pos,
                            size - 1,
                            &cancel,
                            &itx,
                            Some(&mut mac),
                        )
                        .await;
                    drop(itx);
                    r
                };
                let (r, (n, open)) = tokio::join!(pump, fwd);
                pos += n;
                if !open {
                    return; // consumer went away
                }
                match r {
                    Ok(()) => {}
                    Err(e)
                        if e.is_aborted()
                            || matches!(e, MegaError::Quota { .. } | MegaError::Key(_))
                            || e.code().is_some()
                            || tries >= 8 =>
                    {
                        let _ = tx.send(Err(e)).await;
                        return;
                    }
                    Err(_) => {
                        tries += 1;
                        tokio::select! {
                            _ = cancel.cancelled() => { let _ = tx.send(Err(MegaError::Aborted)).await; return; }
                            _ = tokio::time::sleep(this.read_retry_unit * 4 * 2u32.pow(tries)) => {}
                        }
                    }
                }
            }
            if !mac.verify(&key[24..32]) {
                let _ = tx.send(Err(MegaError::Mac)).await;
            }
        });
        Ok(channel_stream(rx))
    }

    /// Decrypted file attribute: 0 = 120 px square thumbnail, 1 = preview. None when the node has none.
    pub async fn attr(
        &self,
        link: &MegaLink,
        node: &MegaNode,
        kind: u32,
        cancel: &CancellationToken,
    ) -> Result<Option<Vec<u8>>, MegaError> {
        let Some(key) = node.key.as_ref() else {
            return Ok(None);
        };
        let Some(r) = crypto::parse_fa(node.fa.as_deref())
            .into_iter()
            .find(|r| r.kind == kind)
        else {
            return Ok(None);
        };
        let resp = self
            .request(
                link,
                json!({ "a": "ufa", "fah": b64url(&r.handle), "ssl": 2, "r": 1 }),
            )
            .await?;
        let base = resp
            .get("p")
            .and_then(Value::as_str)
            .ok_or_else(|| MegaError::Invalid("MEGA returned no file attribute host".into()))?
            .to_string();
        // the web client posts the handles to "<p>/<type>"; older hosts answer on "<p>" too
        for url in [format!("{base}/{kind}"), base.clone()] {
            let res = tokio::select! {
                _ = cancel.cancelled() => return Err(MegaError::Aborted),
                r = self.api.http.post(&url).body(r.handle.to_vec()).send() => r?,
            };
            if res.status().as_u16() == 509 {
                return Err(Self::quota_error(&res));
            }
            if !res.status().is_success() {
                continue;
            }
            let body = tokio::select! {
                _ = cancel.cancelled() => return Err(MegaError::Aborted),
                b = res.bytes() => b?,
            };
            if let Some((_, part)) = crypto::parse_file_attr_response(&body)
                .into_iter()
                .find(|(h, _)| *h == r.handle)
            {
                if !part.is_empty() {
                    return crypto::decrypt_file_attr(key, &part)
                        .map(Some)
                        .map_err(MegaError::Key);
                }
            }
        }
        Ok(None)
    }
}

fn channel_stream(mut rx: mpsc::Receiver<Result<Bytes, MegaError>>) -> ByteStream {
    Box::pin(futures_util::stream::poll_fn(move |cx| rx.poll_recv(cx)))
}

/// Collect a stream into memory (small reads: headers, thumbnails).
pub async fn collect(mut s: ByteStream) -> Result<Vec<u8>, MegaError> {
    let mut out = Vec::new();
    while let Some(c) = s.next().await {
        out.extend_from_slice(&c?);
    }
    Ok(out)
}

/// Node metadata of a listing entry: (key, name) — tries every `id:key` alternative.
fn node_meta(n: &Value, folder_key: &[u8]) -> (Option<Vec<u8>>, Option<String>) {
    let Some(k) = n.get("k").and_then(Value::as_str) else {
        return (None, None);
    };
    let mut key = None;
    for part in k.split('/') {
        let enc = from_b64(part.rsplit(':').next().unwrap_or(""));
        if enc.is_empty() || enc.len() > 32 || enc.len() % 16 != 0 {
            key = None; // RSA-wrapped keys need the account's private key: not for links
            continue;
        }
        let dec = crypto::ecb_decrypt(folder_key, &enc);
        if let Some(a) = n.get("a").and_then(Value::as_str) {
            if let Some(attrs) = crypto::decrypt_attributes(&dec, a) {
                return (
                    Some(dec),
                    Some(
                        attrs
                            .get("n")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    ),
                );
            }
        }
        key = Some(dec);
    }
    (key, None)
}

fn build_folder(
    nodes: &[Value],
    folder_key: &[u8],
    link: &MegaLink,
) -> Result<MegaNode, MegaError> {
    let h = |n: &Value| n.get("h").and_then(Value::as_str).unwrap_or("").to_string();
    // the root folder is the one whose "h" equals the first part of one of its "k" entries
    let root = nodes
        .iter()
        .find(|n| {
            let hh = h(n);
            n.get("k").and_then(Value::as_str).is_some_and(|k| {
                k.split('/')
                    .any(|p| p.split(':').next() == Some(hh.as_str()))
            })
        })
        .ok_or_else(|| MegaError::Invalid("MEGA returned no root folder".into()))?;
    let root_id = h(root);
    // flat nodes first, then children by parent (listing order)
    let mut flat: Vec<(String, String, MegaNode)> = Vec::with_capacity(nodes.len());
    for n in nodes {
        let (key, name) = node_meta(n, folder_key);
        let id = h(n);
        if id == root_id && name.is_none() {
            return Err(MegaError::Key(
                "Attributes could not be decrypted with provided key.".into(),
            ));
        }
        let directory = n.get("t").and_then(Value::as_i64).unwrap_or(0) != 0;
        let ts = n
            .get("ts")
            .and_then(Value::as_i64)
            .filter(|t| *t != 0)
            .map(|t| t * 1000);
        flat.push((
            id.clone(),
            n.get("p").and_then(Value::as_str).unwrap_or("").to_string(),
            MegaNode {
                id,
                name: name
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "untitled".into()),
                size: n.get("s").and_then(Value::as_u64).unwrap_or(0),
                directory,
                timestamp: ts,
                children: Vec::new(),
                fa: n.get("fa").and_then(Value::as_str).map(str::to_string),
                key,
            },
        ));
    }
    let mut kids: HashMap<String, Vec<usize>> = HashMap::new();
    let ids: HashMap<String, usize> = flat
        .iter()
        .enumerate()
        .map(|(i, f)| (f.0.clone(), i))
        .collect();
    for (i, (id, parent, _)) in flat.iter().enumerate() {
        if *id != root_id && ids.contains_key(parent) {
            kids.entry(parent.clone()).or_default().push(i);
        }
    }
    let mut slots: Vec<Option<MegaNode>> = flat.into_iter().map(|f| Some(f.2)).collect();
    fn assemble(
        i: usize,
        slots: &mut Vec<Option<MegaNode>>,
        kids: &HashMap<String, Vec<usize>>,
        depth: usize,
    ) -> Option<MegaNode> {
        let mut n = slots[i].take()?;
        if depth < 512 {
            if let Some(ch) = kids.get(&n.id) {
                for &c in ch {
                    if let Some(k) = assemble(c, slots, kids, depth + 1) {
                        n.children.push(k);
                    }
                }
            }
        }
        Some(n)
    }
    let root = assemble(ids[&root_id], &mut slots, &kids, 0).expect("root");
    let _ = now_ms;
    match &link.sub {
        None => Ok(root),
        Some(sub) => root.find(&sub.id).cloned().ok_or(MegaError::NodeNotFound),
    }
}
