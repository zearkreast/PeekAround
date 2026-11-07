//! `POST /api/remote/import` → transfer job (mirrors `remote/import.ts`), also used by the
//! copy-to-computer of MEGA folders.
//!  - mode 'extract': into folderId/dir (local, writable folder); a folder link lands in a new sub
//!    folder named after it (unique), the tree below is preserved;
//!  - mode 'temp': a new temp session named after the link.
//!
//! Only selected nodes (and everything below selected folders) are fetched. Each file streams
//! (decrypting, MAC checked) → `.part` → unique name → index; zips per `spec.zip`.

use crate::backend::MegaBackend;
use crate::store::ImportHost;
use crate::{hx, mega_error};
use futures_util::StreamExt;
use pa_core::{FileLocation, Job, JobType, Progress, ServerEvent, UploadZipMode};
use pa_jobs::paths::{
    normalize_rel, posix_basename, posix_dirname, rel_posix, sanitize_name, unique_path,
};
use pa_jobs::{runner, EventSink, HttpError, JobFailure, JobService};
use pa_mega::tree::{flatten, relative_to_selection, MAX_IMPORT_NODES};
use pa_mega::{parse_mega_url, MegaError, MegaLink, MegaNode};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

/// latest mtime the TS code accepts (2100-01-01)
const MAX_TS: i64 = 4_102_444_800_000;

/// called with every MEGA error of a file
pub type OnError = Arc<dyn Fn(&MegaError) + Send + Sync>;

/// Options of [`Importer::start`] (`RemoteImportOptions`).
#[derive(Clone, Default)]
pub struct ImportOptions {
    /// the already loaded tree (remote folders): no second listing
    pub root: Option<Arc<MegaNode>>,
    /// copy semantics: every selected node lands directly in folderId/dir (a selected folder keeps
    /// its name and tree), instead of a new folder named after the link
    pub relative: bool,
    pub title: Option<String>,
    /// every MEGA error of a file (quota bookkeeping of remote folders)
    pub on_error: Option<OnError>,
}

/// The import job starter (`startRemoteImport`).
#[derive(Clone)]
pub struct Importer {
    pub jobs: JobService,
    pub mega: Arc<dyn MegaBackend>,
    pub host: Arc<dyn ImportHost>,
    pub events: Arc<dyn EventSink>,
}

struct Target {
    folder_id: i64,
    root: PathBuf,
    abs: PathBuf,
    dir: String,
}

struct Got {
    part: PathBuf,
    dir_abs: PathBuf,
    name: String,
}

use pa_core::js::to_string_of as js_string;

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Some(_) => true,
    }
}

/// `String(x ?? '')` for the folder id / dir of a spec
fn str_of(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(v) => js_string(v),
    }
}

/// the link's url as `parseMegaUrl` takes it (non-strings: 400 'Not a MEGA link')
pub(crate) fn parse_url_value(v: Option<&Value>) -> Result<MegaLink, HttpError> {
    match v {
        Some(Value::String(s)) => parse_mega_url(s).map_err(hx),
        _ => Err(HttpError::new(400, "Not a MEGA link")),
    }
}

fn resolve_zip_mode(host: &dyn ImportHost, z: Option<&Value>) -> UploadZipMode {
    match z.and_then(Value::as_str) {
        Some("extract") => return UploadZipMode::Extract,
        Some("temp") => return UploadZipMode::Temp,
        Some("keep") => return UploadZipMode::Keep,
        _ => {}
    }
    match host.zip_action().as_str() {
        "extract" => UploadZipMode::Extract,
        "temp" => UploadZipMode::Temp,
        _ => UploadZipMode::Keep,
    }
}

fn prog(done: i64, total: i64) -> Option<Progress> {
    Some(Progress { done, total })
}

/// Remove empty directories below `dir` (and `dir` itself when `is_self`); returns whether `dir` is gone.
pub fn prune_empty_dirs(dir: &Path, is_self: bool) -> futures_util::future::BoxFuture<'_, bool> {
    Box::pin(async move {
        let Ok(mut rd) = tokio::fs::read_dir(dir).await else {
            return false;
        };
        let mut entries = Vec::new();
        while let Ok(Some(e)) = rd.next_entry().await {
            entries.push(e);
        }
        let mut left = entries.len();
        for e in entries {
            let Ok(ft) = e.file_type().await else {
                continue;
            };
            if ft.is_dir() && !ft.is_symlink() && prune_empty_dirs(&e.path(), true).await {
                left -= 1;
            }
        }
        if is_self && left == 0 {
            let _ = tokio::fs::remove_dir(dir).await;
            return true;
        }
        false
    })
}

impl Importer {
    /// `startRemoteImport(ctx, spec, opts)` — validation errors before queueing (400 / 403 / 404).
    pub fn start(&self, spec: &Value, opts: ImportOptions) -> Result<Job, HttpError> {
        self.host.require_writable()?;
        let link = parse_url_value(spec.get("url"))?;
        let mode = spec.get("mode").and_then(Value::as_str);
        if mode != Some("extract") && mode != Some("temp") {
            return Err(HttpError::new(400, "mode must be 'extract' or 'temp'"));
        }
        let zip_mode = resolve_zip_mode(&*self.host, spec.get("zip"));
        let mut target = None;
        if mode == Some("extract") {
            if !truthy(spec.get("folderId")) {
                return Err(HttpError::new(400, "folderId required"));
            }
            let dir = str_of(spec.get("dir"));
            let (folder_id, root, abs) = self.host.target(&str_of(spec.get("folderId")), &dir)?;
            target = Some(Target {
                folder_id,
                root,
                abs,
                dir: normalize_rel(&dir)?,
            });
        }
        let selected: Option<HashSet<String>> = match spec.get("nodes") {
            Some(Value::Array(a)) => Some(a.iter().map(js_string).collect()),
            _ => None,
        };
        if selected.as_ref().is_some_and(|s| s.is_empty()) {
            return Err(HttpError::new(400, "Nothing selected"));
        }
        let me = self.clone();
        let title = opts.title.clone().unwrap_or_else(|| "MEGA".into());
        Ok(self.jobs.enqueue(
            JobType::Transfer,
            title,
            runner(
                move |jc| async move { me.run(jc, link, opts, selected, target, zip_mode).await },
            ),
        ))
    }

    async fn run(
        &self,
        jc: pa_jobs::JobContext,
        link: MegaLink,
        opts: ImportOptions,
        selected: Option<HashSet<String>>,
        target: Option<Target>,
        zip_mode: UploadZipMode,
    ) -> Result<Option<FileLocation>, JobFailure> {
        let root: Arc<MegaNode> = match &opts.root {
            Some(r) => r.clone(),
            None => Arc::new(self.mega.load(&link).await.map_err(|e| mega_error(&e))?),
        };
        jc.set_title(opts.title.clone().unwrap_or_else(|| root.name.clone()));
        let (nodes, _) = flatten(&root, MAX_IMPORT_NODES);
        // (node, path) of every file to fetch
        let files: Vec<(&MegaNode, String)> = nodes
            .iter()
            .filter(|n| {
                !n.node.directory
                    && selected
                        .as_ref()
                        .is_none_or(|s| n.chain.iter().any(|id| s.contains(id)))
            })
            .map(|n| {
                let path = match (&selected, opts.relative) {
                    (Some(s), true) => relative_to_selection(&n.path, &n.chain, s),
                    _ => n.path.clone(),
                };
                (n.node, path)
            })
            .collect();
        if files.is_empty() {
            return Err("Nothing to import".into());
        }
        let total_bytes: i64 = files.iter().map(|f| f.0.size as i64).sum();
        let n_files = files.len() as i64;
        jc.transfer(prog(0, total_bytes), prog(0, n_files));

        let (folder_id, base_abs, base_rel, folder_root, temp_row) = match &target {
            Some(t) => {
                let base_abs = if root.directory && !opts.relative {
                    unique_path(&t.abs, &sanitize_name(&root.name)).await
                } else {
                    t.abs.clone()
                };
                tokio::fs::create_dir_all(&base_abs)
                    .await
                    .map_err(|e| JobFailure::new(e.to_string()))?;
                let rel = rel_posix(&t.root, &base_abs);
                (t.folder_id, base_abs, rel, t.root.clone(), None)
            }
            None => {
                let content = self
                    .host
                    .temp_dir()
                    .join(pa_jobs::random_uuid())
                    .join("content");
                tokio::fs::create_dir_all(&content)
                    .await
                    .map_err(|e| JobFailure::new(e.to_string()))?;
                let id = self.host.create_temp_folder(&content, &root.name).await?;
                (id, content.clone(), String::new(), content, Some(id))
            }
        };

        // Phase 1 downloads every file (media lands right away); zips wait as `.part` files and are
        // handled in phase 2 per spec.zip. A failing file/zip never stops the others (only a cancel
        // or a MEGA quota error does); failures end the job with "n of m", no empty folder stays.
        let bytes = Arc::new(Mutex::new(0i64));
        let mut done = 0i64;
        let mut landed = 0usize;
        let mut zip_sessions = 0usize;
        let mut last_session: Option<String> = None;
        let mut failures: Vec<String> = Vec::new();
        let mut first_failure: Option<HttpError> = None;
        let mut zips: Vec<Got> = Vec::new();
        let total = files.len();
        let res: Result<(), JobFailure> = async {
            let mut i = 0;
            while i < files.len() {
                let (node, path) = (files[i].0, files[i].1.clone());
                if jc.is_cancelled() {
                    return Err("canceled".into());
                }
                let before = *bytes.lock().unwrap();
                let b2 = bytes.clone();
                let jc2 = jc.clone();
                let mut on_bytes = move |n: u64| {
                    let mut b = b2.lock().unwrap();
                    *b += n as i64;
                    jc2.transfer(prog(*b, total_bytes), prog(done, n_files));
                };
                match self
                    .download_one(
                        &link,
                        node,
                        &path,
                        &base_abs,
                        jc.cancel.clone(),
                        &mut on_bytes,
                    )
                    .await
                {
                    Ok(got) => {
                        if zip_mode != UploadZipMode::Keep && pa_core::ext_of(&got.name) == "zip" {
                            zips.push(got);
                        } else {
                            self.land_file(folder_id, &folder_root, &got)
                                .await
                                .map_err(|e| JobFailure::new(e.to_string()))?;
                            landed += 1;
                        }
                    }
                    Err(e) => {
                        if jc.is_cancelled() {
                            return Err(JobFailure::new(e.to_string()));
                        }
                        if let Some(f) = &opts.on_error {
                            f(&e);
                        }
                        let err = mega_error(&e);
                        failures.push(err.message.clone());
                        first_failure.get_or_insert(err.clone());
                        // quota: every further request fails the same way → count the rest as failed
                        if err.status == 429 {
                            for _ in i + 1..files.len() {
                                failures.push(err.message.clone());
                            }
                            done = n_files;
                            break;
                        }
                    }
                }
                *bytes.lock().unwrap() = before + node.size as i64;
                done += 1;
                jc.transfer(
                    prog(*bytes.lock().unwrap(), total_bytes),
                    prog(done, n_files),
                );
                i += 1;
            }
            for z in &zips {
                if jc.is_cancelled() {
                    return Err("canceled".into());
                }
                let r: Result<(), HttpError> = async {
                    if zip_mode == UploadZipMode::Temp {
                        last_session = Some(self.host.zip_to_temp(&z.part, &z.name).await?);
                        zip_sessions += 1;
                    } else {
                        self.extract_into(folder_id, &folder_root, z).await?;
                        landed += 1;
                    }
                    Ok(())
                }
                .await;
                if let Err(e) = r {
                    failures.push(e.message.clone());
                    first_failure.get_or_insert(e);
                }
                let _ = tokio::fs::remove_file(&z.part).await;
            }
            Ok(())
        }
        .await;
        let cleanup = || async {
            if temp_row.is_some() && landed == 0 {
                self.host.delete_temp(folder_id).await;
                self.events.emit(ServerEvent::Folders { status: None });
                return;
            }
            if temp_row.is_some() {
                self.events.emit(ServerEvent::Folders { status: None });
            }
            if temp_row.is_some() || (root.directory && !opts.relative) {
                prune_empty_dirs(&base_abs, temp_row.is_none()).await;
            }
        };
        if let Err(e) = res {
            for z in &zips {
                let _ = tokio::fs::remove_file(&z.part).await;
            }
            cleanup().await;
            return Err(e);
        }
        cleanup().await;

        let target_dir = target.as_ref().map(|t| t.dir.clone()).unwrap_or_default();
        let result = if temp_row.is_some() {
            if landed > 0 {
                Some(FileLocation {
                    folder_id: folder_id.to_string(),
                    dir: String::new(),
                    name: root.name.clone(),
                })
            } else {
                last_session.map(|id| FileLocation {
                    folder_id: id,
                    dir: String::new(),
                    name: String::new(),
                })
            }
        } else if root.directory && !opts.relative {
            let dir = posix_dirname(&base_rel).to_string();
            if landed > 0 {
                Some(FileLocation {
                    folder_id: folder_id.to_string(),
                    dir,
                    name: posix_basename(&base_rel).to_string(),
                })
            } else {
                Some(FileLocation {
                    folder_id: folder_id.to_string(),
                    dir: target_dir,
                    name: String::new(),
                })
            }
        } else {
            Some(FileLocation {
                folder_id: folder_id.to_string(),
                dir: target_dir,
                name: if total == 1 && landed > 0 {
                    sanitize_name(&files[0].0.name)
                } else {
                    String::new()
                },
            })
        };
        if !failures.is_empty() {
            jc.set_result(if landed > 0 || zip_sessions > 0 {
                result
            } else {
                None
            });
            if failures.len() == total && failures.len() == 1 {
                return Err(first_failure
                    .map(JobFailure::from)
                    .unwrap_or_else(|| JobFailure::new(failures[0].clone())));
            }
            let first = failures[0].clone();
            return Err(JobFailure::new(format!(
                "{} of {} files could not be imported: {}",
                failures.len(),
                total,
                first
            )));
        }
        Ok(result)
    }

    async fn download_one(
        &self,
        link: &MegaLink,
        node: &MegaNode,
        path: &str,
        base_abs: &Path,
        cancel: CancellationToken,
        on_bytes: &mut (dyn FnMut(u64) + Send),
    ) -> Result<Got, MegaError> {
        // the root node of a file link has path ''; inside folders the path is relative to the link root
        let p = if path.is_empty() {
            node.name.as_str()
        } else {
            path
        };
        let mut segs: Vec<String> = p.split('/').map(sanitize_name).collect();
        let name = segs.pop().unwrap_or_default();
        let mut dir_abs = base_abs.to_path_buf();
        for s in &segs {
            dir_abs.push(s);
        }
        tokio::fs::create_dir_all(&dir_abs).await?;
        let part = dir_abs.join(format!("{name}.{:08x}.part", rand::random::<u32>()));
        let r = async {
            let mut f = tokio::fs::File::create(&part).await?;
            let mut s = self.mega.download(link, node, cancel.clone())?;
            loop {
                let c = tokio::select! {
                    c = s.next() => c,
                    _ = cancel.cancelled() => return Err(MegaError::Aborted),
                };
                let Some(c) = c else { break };
                let c = c?;
                f.write_all(&c).await?;
                on_bytes(c.len() as u64);
            }
            f.flush().await?;
            drop(f);
            if let Some(ts) = node.timestamp.filter(|t| *t > 0 && *t < MAX_TS) {
                pa_jobs::paths::set_mtime(&part, ts).await;
            }
            Ok::<_, MegaError>(())
        }
        .await;
        match r {
            Ok(()) => Ok(Got {
                part,
                dir_abs,
                name,
            }),
            Err(e) => {
                let _ = tokio::fs::remove_file(&part).await;
                Err(e)
            }
        }
    }

    async fn land_file(&self, folder_id: i64, root: &Path, g: &Got) -> std::io::Result<()> {
        let fin = unique_path(&g.dir_abs, &g.name).await;
        tokio::fs::rename(&g.part, &fin).await?;
        self.host
            .upsert_file(folder_id, &rel_posix(root, &fin))
            .await;
        Ok(())
    }

    /// Extract a downloaded zip next to where it would have landed; a failed extraction leaves nothing behind.
    async fn extract_into(&self, folder_id: i64, root: &Path, z: &Got) -> Result<(), HttpError> {
        let stem = strip_zip(&z.name);
        let dest = unique_path(&z.dir_abs, if stem.is_empty() { "Archive" } else { stem }).await;
        if let Err(e) = self.host.extract_zip(&z.part, &dest).await {
            let _ = tokio::fs::remove_dir_all(&dest).await;
            return Err(e);
        }
        self.host
            .scan_folder(folder_id, &rel_posix(root, &dest))
            .await;
        Ok(())
    }
}

/// `name.replace(/\.zip$/i, '')`
fn strip_zip(name: &str) -> &str {
    let n = name.len();
    if n >= 4 && name.is_char_boundary(n - 4) && name[n - 4..].eq_ignore_ascii_case(".zip") {
        &name[..n - 4]
    } else {
        name
    }
}
