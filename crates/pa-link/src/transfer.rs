//! Transfer jobs between the PC and linked phones (mirrors `devices/transfer.ts`):
//!  - device → PC: `file` op → `<name>.<rand>.part` → unique final name → mtime from X-Peek-Mtime → index;
//!  - PC → device: per file a one-time pull URL + `receive` op; the phone pulls, writes, answers `res`;
//!  - `GET /api/devices/pull/:token`: the PC file, streamed (Content-Length, Last-Modified, X-Peek-Mtime).

use crate::hub::{is_aborted, DeviceHub, PullHooks, ReqOpts};
use crate::store::DeviceFolderRow;
use async_trait::async_trait;
use futures_util::StreamExt;
use pa_core::{FileLocation, FileRef, Job, JobType, Progress};
use pa_jobs::library::{http_date, FileReply};
use pa_jobs::paths::{
    normalize_rel, posix_basename, posix_dirname, rand_hex4, rel_posix, resolve_in_root,
    sanitize_name, set_mtime, unique_path,
};
use pa_jobs::{runner, HttpError, JobFailure, JobService};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

/// A media row of a device folder.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceMedia {
    pub dir: String,
    pub name: String,
    pub size: i64,
    pub mtime: i64,
}

/// A library folder as the copy routes see it (`lib.folderRow`).
#[derive(Debug, Clone, PartialEq)]
pub struct HostFolder {
    pub id: i64,
    pub name: String,
    /// host path (`device://…` for phone folders)
    pub path: PathBuf,
    /// a phone folder (its device id)
    pub device_id: Option<String>,
}

/// What the transfer jobs need from the server.
#[async_trait]
pub trait TransferHost: Send + Sync + 'static {
    /// `lib.folderRow(id)` (404 'Folder not found')
    fn folder(&self, folder_id: &str) -> Result<HostFolder, HttpError>;
    /// `lib.folderRow` + `devices.folderOf` (400 'Not a device folder')
    fn device_folder(&self, folder_id: &str) -> Result<DeviceFolderRow, HttpError>;
    /// `SELECT * FROM media WHERE folder_id = ? AND dir = ? AND name = ?`
    fn find_media(&self, folder_id: i64, dir: &str, name: &str) -> Option<DeviceMedia>;
    /// every row of the folder (`rel = ""`) or below `rel`, `ORDER BY dir, name`
    fn media_under(&self, folder_id: i64, rel: &str) -> Vec<DeviceMedia>;
    /// `scanner.skipName`
    fn skip_name(&self, name: &str) -> bool;
    /// `scanner.upsertFile(folderId, rel)`
    async fn upsert_file(&self, folder_id: i64, rel: &str);
}

/// Shared parts of the transfer jobs.
#[derive(Clone)]
pub struct Transfers {
    pub hub: Arc<DeviceHub>,
    pub jobs: JobService,
    pub host: Arc<dyn TransferHost>,
}

struct FromItem {
    device_id: String,
    key: String,
    row: DeviceMedia,
    /// destination path relative to the target dir ('/' separated)
    dest: String,
}

struct Group {
    top: String,
    file: bool,
    items: Vec<FromItem>,
}

fn prog(done: i64, total: i64) -> Option<Progress> {
    Some(Progress { done, total })
}

impl Transfers {
    /// `POST /api/fs/move {copy: true}` from device folders into a local folder (`to` must be a
    /// local, writable folder — the route checks that). Returns the queued transfer job.
    pub fn copy_from_device(
        &self,
        refs: &[FileRef],
        to: &HostFolder,
        to_dir: &str,
    ) -> Result<Job, HttpError> {
        let dir = normalize_rel(to_dir)?;
        let to_abs = resolve_in_root(&to.path, &dir)?;
        let mut groups = Vec::new();
        for r in refs {
            let folder = self.host.device_folder(&r.folder_id)?;
            let rel = normalize_rel(&r.rel)?;
            let name = posix_basename(&rel).to_string();
            let parent = posix_dirname(&rel).to_string();
            let file = if rel.is_empty() {
                None
            } else {
                self.host.find_media(folder.id, &parent, &name)
            };
            if let Some(f) = file {
                groups.push(Group {
                    top: name.clone(),
                    file: true,
                    items: vec![FromItem {
                        device_id: folder.device_id.clone(),
                        key: folder.device_key.clone(),
                        row: f,
                        dest: name,
                    }],
                });
                continue;
            }
            let rows = self.host.media_under(folder.id, &rel);
            let top = if rel.is_empty() {
                folder.name.clone()
            } else {
                name
            };
            let cut = if rel.is_empty() { 0 } else { rel.len() + 1 };
            groups.push(Group {
                top: sanitize_name(&top),
                file: false,
                items: rows
                    .into_iter()
                    .map(|row| {
                        let full = pa_jobs::paths::join_rel(&row.dir, &row.name);
                        FromItem {
                            device_id: folder.device_id.clone(),
                            key: folder.device_key.clone(),
                            dest: full.get(cut..).unwrap_or("").to_string(),
                            row,
                        }
                    })
                    .collect(),
            });
        }
        let count: usize = groups.iter().map(|g| g.items.len()).sum();
        if count == 0 {
            return Err(HttpError::bad("Nothing to copy"));
        }
        for g in &groups {
            for it in &g.items {
                if !self.hub.is_online(&it.device_id) {
                    return Err(HttpError::new(503, "device offline"));
                }
            }
        }
        let total_bytes: i64 = groups
            .iter()
            .flat_map(|g| &g.items)
            .map(|i| i.row.size.max(0))
            .sum();
        let title = if count == 1 {
            groups
                .iter()
                .flat_map(|g| &g.items)
                .next()
                .unwrap()
                .row
                .name
                .clone()
        } else {
            format!("{count} files")
        };
        let me = self.clone();
        let to = to.clone();
        let total = count as i64;
        Ok(self.jobs.enqueue_with(
            JobType::Transfer,
            title,
            |j| {
                j.bytes = prog(0, total_bytes);
                j.files = prog(0, total);
            },
            runner(move |jc| async move {
                let mut done = 0i64;
                let bytes = Arc::new(Mutex::new(0i64));
                let mut failed: Vec<String> = Vec::new();
                let mut first: Option<String> = None;
                jc.transfer(prog(0, total_bytes), prog(0, total));
                tokio::fs::create_dir_all(&to_abs).await?;
                for g in groups {
                    // a copied directory lands under a unique name next to existing ones
                    let base = if g.file {
                        to_abs.clone()
                    } else {
                        unique_path(&to_abs, &g.top).await
                    };
                    for it in g.items {
                        if jc.is_cancelled() {
                            return Err("canceled".into());
                        }
                        let mut target_dir = base.clone();
                        for s in posix_dirname(&it.dest)
                            .split('/')
                            .filter(|s| !s.is_empty() && *s != ".")
                        {
                            target_dir.push(s);
                        }
                        let before = *bytes.lock().unwrap();
                        let jc2 = jc.clone();
                        let b2 = bytes.clone();
                        let d = done;
                        let on_bytes = move |n: u64| {
                            let mut b = b2.lock().unwrap();
                            *b += n as i64;
                            jc2.transfer(prog(*b, total_bytes), prog(d, total));
                        };
                        match fetch_device_file(&me.hub, &it, &target_dir, &jc.cancel, on_bytes)
                            .await
                        {
                            Ok(fin) => {
                                me.host.upsert_file(to.id, &rel_posix(&to.path, &fin)).await;
                                if first.is_none() {
                                    first = rel_posix(&to_abs, &fin)
                                        .split('/')
                                        .next()
                                        .map(str::to_string);
                                }
                            }
                            Err(e) => {
                                if jc.is_cancelled() {
                                    return Err(e.into());
                                }
                                if e.status == 503 {
                                    return Err(e.into());
                                }
                                failed.push(it.row.name.clone());
                                *bytes.lock().unwrap() = before + it.row.size.max(0);
                            }
                        }
                        done += 1;
                        let b = *bytes.lock().unwrap();
                        jc.transfer(prog(b, total_bytes), prog(done, total));
                    }
                }
                if !failed.is_empty() {
                    let list = failed
                        .iter()
                        .take(5)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(JobFailure::new(format!(
                        "Copied {} of {done} files; failed: {list}{}",
                        done - failed.len() as i64,
                        if failed.len() > 5 { "…" } else { "" }
                    )));
                }
                Ok(Some(FileLocation {
                    folder_id: to.id.to_string(),
                    dir,
                    name: first.unwrap_or_default(),
                }))
            }),
        ))
    }

    /// `POST /api/fs/move {copy: true}` from local folders into a device folder: per file a
    /// one-time pull URL + `receive` op; afterwards the touched directories are re-listed.
    pub async fn copy_to_device(
        &self,
        refs: &[FileRef],
        device_folder: &DeviceFolderRow,
        to_dir: &str,
    ) -> Result<Job, HttpError> {
        let device_id = device_folder.device_id.clone();
        let key = device_folder.device_key.clone();
        let caps = self
            .hub
            .caps(&device_id)
            .ok_or_else(|| HttpError::new(503, "device offline"))?;
        if !caps.receive {
            return Err(HttpError::new(403, "This device does not accept files"));
        }
        let dir = normalize_rel(to_dir)?;
        let mut items: Vec<ToItem> = Vec::new();
        for r in refs {
            let folder = self.host.folder(&r.folder_id)?;
            if folder.device_id.is_some() {
                return Err(HttpError::new(
                    403,
                    "Copying between devices is not supported",
                ));
            }
            let abs = resolve_in_root(&folder.path, &r.rel)?;
            let Ok(st) = tokio::fs::metadata(&abs).await else {
                continue;
            };
            let base = abs
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            if st.is_dir() {
                self.walk(&abs, &pa_jobs::paths::join_rel(&dir, &base), &mut items)
                    .await;
            } else if st.is_file() {
                items.push(ToItem {
                    abs,
                    dir: dir.clone(),
                    name: base,
                    size: st.len(),
                    mtime: mtime_ms(&st),
                });
            }
        }
        if items.is_empty() {
            return Err(HttpError::bad("Nothing to copy"));
        }
        let total_bytes: i64 = items.iter().map(|i| i.size as i64).sum();
        let total = items.len() as i64;
        let title = if items.len() == 1 {
            items[0].name.clone()
        } else {
            format!("{} files", items.len())
        };
        let me = self.clone();
        let folder_id = device_folder.id;
        Ok(self.jobs.enqueue_with(
            JobType::Transfer,
            title,
            |j| {
                j.bytes = prog(0, total_bytes);
                j.files = prog(0, total);
            },
            runner(move |jc| async move {
                let mut done = 0i64;
                let bytes = Arc::new(Mutex::new(0i64));
                let mut first: Option<String> = None;
                let mut touched: Vec<String> = Vec::new();
                jc.transfer(prog(0, total_bytes), prog(0, total));
                let res: Result<(), JobFailure> = async {
                    for it in &items {
                        if jc.is_cancelled() {
                            return Err("canceled".into());
                        }
                        let before = *bytes.lock().unwrap();
                        let token_cell: Arc<Mutex<String>> = Arc::default();
                        // cancel also stops a pull the phone is still reading
                        let (hub1, hub2) = (me.hub.clone(), me.hub.clone());
                        let (tc1, tc2) = (token_cell.clone(), token_cell.clone());
                        let job_cancel = jc.cancel.clone();
                        let (jc2, b2, d) = (jc.clone(), bytes.clone(), done);
                        let hooks = PullHooks {
                            on_start: Some(Arc::new(move |stream_cancel: CancellationToken| {
                                let jcan = job_cancel.clone();
                                tokio::spawn(async move {
                                    tokio::select! {
                                        _ = jcan.cancelled() => stream_cancel.cancel(),
                                        _ = stream_cancel.cancelled() => {}
                                    }
                                });
                                if let Some(rid) = hub1.rid_of_pull(&tc1.lock().unwrap()) {
                                    hub1.hold_receive(&rid);
                                }
                            })),
                            on_bytes: Some(Arc::new(move |n: u64| {
                                let mut b = b2.lock().unwrap();
                                *b += n as i64;
                                jc2.transfer(prog(*b, total_bytes), prog(d, total));
                            })),
                            on_end: Some(Arc::new(move || {
                                if let Some(rid) = hub2.rid_of_pull(&tc2.lock().unwrap()) {
                                    hub2.await_receive_result(&rid);
                                }
                            })),
                        };
                        let token = me.hub.create_pull(&device_id, it.abs.clone(), it.size, it.mtime, hooks);
                        *token_cell.lock().unwrap() = token.clone();
                        let r = me
                            .hub
                            .receive(
                                &device_id,
                                json!({ "key": key, "dir": it.dir, "name": it.name, "size": it.size, "mtime": it.mtime, "url": format!("/api/devices/pull/{token}") }),
                                ReqOpts::prio(1).signal(jc.cancel.clone()),
                            )
                            .await;
                        me.hub.revoke_pull(&token);
                        let res = match r {
                            Ok(v) => v,
                            Err(e) if is_aborted(&e) => return Err("canceled".into()),
                            Err(e) => return Err(e.into()),
                        };
                        if first.is_none() {
                            first = Some(match res.get("p") {
                                Some(Value::String(p)) => p.clone(),
                                _ => pa_jobs::paths::join_rel(&it.dir, &it.name),
                            });
                        }
                        *bytes.lock().unwrap() = before + it.size as i64;
                        if !touched.contains(&it.dir) {
                            touched.push(it.dir.clone());
                        }
                        done += 1;
                        let b = *bytes.lock().unwrap();
                        jc.transfer(prog(b, total_bytes), prog(done, total));
                    }
                    Ok(())
                }
                .await;
                // refresh what landed (the phone also reports `changed`; relist is de-duplicated)
                for d in &touched {
                    drop(me.hub.relist(folder_id, d));
                }
                res?;
                let p = first.unwrap_or_default();
                Ok(Some(FileLocation {
                    folder_id: folder_id.to_string(),
                    dir: posix_dirname(&p).to_string(),
                    name: posix_basename(&p).to_string(),
                }))
            }),
        ))
    }

    fn walk<'a>(
        &'a self,
        abs: &'a Path,
        target_dir: &'a str,
        items: &'a mut Vec<ToItem>,
    ) -> futures_util::future::BoxFuture<'a, ()> {
        Box::pin(async move {
            let Ok(mut rd) = tokio::fs::read_dir(abs).await else {
                return;
            };
            let mut entries = Vec::new();
            while let Ok(Some(e)) = rd.next_entry().await {
                entries.push(e);
            }
            for e in entries {
                let name = e.file_name().to_string_lossy().into_owned();
                if self.host.skip_name(&name) {
                    continue;
                }
                let Ok(ft) = e.file_type().await else {
                    continue;
                };
                let p = e.path();
                if ft.is_dir() {
                    self.walk(&p, &pa_jobs::paths::join_rel(target_dir, &name), items)
                        .await;
                } else if ft.is_file() && pa_core::kind_of(&name).is_some() {
                    if let Ok(st) = tokio::fs::metadata(&p).await {
                        items.push(ToItem {
                            abs: p,
                            dir: target_dir.to_string(),
                            name,
                            size: st.len(),
                            mtime: mtime_ms(&st),
                        });
                    }
                }
            }
        })
    }
}

struct ToItem {
    abs: PathBuf,
    /// target dir inside the device folder
    dir: String,
    name: String,
    size: u64,
    mtime: i64,
}

fn mtime_ms(st: &std::fs::Metadata) -> i64 {
    st.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| pa_core::js::round(d.as_secs_f64() * 1000.0) as i64)
        .unwrap_or(0)
}

async fn fetch_device_file(
    hub: &DeviceHub,
    it: &FromItem,
    target_dir: &Path,
    cancel: &CancellationToken,
    mut on_bytes: impl FnMut(u64),
) -> Result<PathBuf, HttpError> {
    let io = |e: std::io::Error| HttpError::new(500, e.to_string());
    tokio::fs::create_dir_all(target_dir).await.map_err(io)?;
    let name = sanitize_name(&it.row.name);
    let part = target_dir.join(format!("{name}.{}.part", rand_hex4()));
    let tr = hub
        .request(
            &it.device_id,
            "file",
            json!({ "key": it.key, "p": pa_jobs::paths::join_rel(&it.row.dir, &it.row.name) }),
            ReqOpts::prio(1).signal(cancel.clone()),
        )
        .await?;
    if tr.status != 200 {
        return Err(HttpError::new(
            502,
            format!("The device answered {}", tr.status),
        ));
    }
    let mt = tr
        .header("x-peek-mtime")
        .map(pa_core::js::number)
        .filter(|n| *n != 0.0 && !n.is_nan());
    let r = async {
        let mut f = tokio::fs::File::create(&part).await.map_err(io)?;
        let mut body = tr.body;
        loop {
            let c = tokio::select! {
                c = body.next() => c,
                _ = cancel.cancelled() => return Err(crate::hub::aborted()),
            };
            let Some(c) = c else { break };
            let c = c.map_err(|e| HttpError::new(502, e.to_string()))?;
            f.write_all(&c).await.map_err(io)?;
            on_bytes(c.len() as u64);
        }
        f.flush().await.map_err(io)?;
        drop(f);
        let mt = mt.map(|m| m as i64).unwrap_or(it.row.mtime);
        if mt > 0 && mt < 4_102_444_800_000 {
            set_mtime(&part, mt).await;
        }
        let fin = unique_path(target_dir, &name).await;
        tokio::fs::rename(&part, &fin).await.map_err(io)?;
        Ok(fin)
    }
    .await;
    if r.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
    }
    r
}

/// `GET /api/devices/pull/:token` (after `authenticate`): one-time PC file download for `receive`.
/// The body counts bytes into the job and fires the hooks; 404 unknown / expired / gone file,
/// 403 another device.
pub async fn open_pull(
    hub: &DeviceHub,
    token: &str,
    device_id: &str,
) -> Result<FileReply, HttpError> {
    let p = hub.take_pull(token, device_id)?;
    let st = tokio::fs::metadata(&p.abs)
        .await
        .ok()
        .filter(|m| m.is_file());
    let Some(st) = st else {
        return Err(HttpError::new(404, "File no longer exists"));
    };
    let file = tokio::fs::File::open(&p.abs)
        .await
        .map_err(|_| HttpError::new(404, "File no longer exists"))?;
    let mtime = if p.mtime != 0 { p.mtime } else { mtime_ms(&st) };
    let size = st.len();
    let cancel = CancellationToken::new();
    if let Some(f) = &p.hooks.on_start {
        f(cancel.clone());
    }
    let hooks = p.hooks.clone();
    let reader = tokio_util::io::ReaderStream::with_capacity(file, 64 * 1024);
    let body = futures_util::stream::unfold(
        (reader, hooks, cancel, false),
        |(mut reader, hooks, cancel, ended)| async move {
            if ended {
                return None;
            }
            let next = tokio::select! {
                c = reader.next() => c,
                _ = cancel.cancelled() => Some(Err(std::io::Error::other("canceled"))),
            };
            match next {
                Some(Ok(c)) => {
                    if let Some(f) = &hooks.on_bytes {
                        f(c.len() as u64);
                    }
                    Some((Ok(c), (reader, hooks, cancel, false)))
                }
                Some(Err(e)) => Some((Err(e), (reader, hooks, cancel, true))),
                None => {
                    if let Some(f) = &hooks.on_end {
                        f();
                    }
                    None
                }
            }
        },
    );
    Ok(FileReply {
        status: 200,
        headers: vec![
            ("Content-Type".into(), "application/octet-stream".into()),
            ("Content-Length".into(), size.to_string()),
            ("Last-Modified".into(), http_date(mtime)),
            ("X-Peek-Mtime".into(), mtime.to_string()),
            ("Cache-Control".into(), "no-store".into()),
        ],
        body: Some(Box::pin(body)),
    })
}
