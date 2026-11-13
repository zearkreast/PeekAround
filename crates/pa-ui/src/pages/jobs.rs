//! Jobs panel (top bar) + uploads panel: server jobs (video edits, slideshow exports, transfers)
//! from `/api/jobs` and the `job` events; file uploads to a remote server with progress.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use pa_core::{DirTarget, Job, JobStatus, JobType};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::api;
use crate::app::AppInner;
use crate::backend::UploadSpec;
use crate::fmt;
use crate::i18n;
use crate::router::Route;
use crate::{JobRowData, JobsState, ToastKind, UploadRowData};

type UpQueue = Rc<RefCell<Vec<(usize, (PathBuf, String))>>>;

#[derive(Clone)]
struct Up {
    name: String,
    size: i64,
    status: String,
    error: String,
}

/// jobs + uploads
#[derive(Default)]
pub(crate) struct JobsCtl {
    jobs: RefCell<HashMap<String, Job>>,
    /// bytes/s of running transfers (from consecutive updates)
    rates: RefCell<HashMap<String, (i64, i64, f64)>>,
    ups: RefCell<Vec<Up>>,
    up_epoch: Cell<u64>,
}

fn status_str(s: JobStatus) -> &'static str {
    match s {
        JobStatus::Queued => "queued",
        JobStatus::Running => "running",
        JobStatus::Done => "done",
        JobStatus::Error => "error",
        JobStatus::Canceled => "canceled",
    }
}

fn transfer_line(a: &AppInner, j: &Job, rate: Option<f64>) -> String {
    let lang = a.lang();
    let mut parts: Vec<String> = vec![];
    if let Some(f) = &j.files {
        if f.total > 1 {
            parts.push(i18n::tv(
                lang,
                "devices.jobFiles",
                &[
                    ("done", &f.done.to_string()),
                    ("total", &f.total.to_string()),
                ],
            ));
        }
    }
    if let Some(b) = &j.bytes {
        if b.total > 0 {
            parts.push(if j.status == JobStatus::Done {
                fmt::format_bytes(b.total)
            } else {
                format!(
                    "{} / {}",
                    fmt::format_bytes(b.done),
                    fmt::format_bytes(b.total)
                )
            });
            if j.status == JobStatus::Running {
                if let Some(r) = rate.filter(|r| *r > 1.0) {
                    parts.push(format!("{}/s", fmt::format_bytes(r as i64)));
                    let eta = (b.total - b.done) as f64 / r;
                    if eta > 2.0 {
                        parts.push(if eta < 60.0 {
                            i18n::tv(
                                lang,
                                "devices.etaSec",
                                &[("n", &(eta.ceil() as i64).to_string())],
                            )
                        } else {
                            i18n::tv(
                                lang,
                                "devices.etaMin",
                                &[("n", &((eta / 60.0).ceil() as i64).to_string())],
                            )
                        });
                    }
                }
            }
        }
    }
    parts.join(" · ")
}

fn publish(a: &Rc<AppInner>) {
    let st = a.window.global::<JobsState>();
    let mut list: Vec<Job> = a.pages.jobs.jobs.borrow().values().cloned().collect();
    list.sort_by_key(|j| std::cmp::Reverse(j.created_at));
    let rates = a.pages.jobs.rates.borrow().clone();
    let rows: Vec<JobRowData> = list
        .iter()
        .map(|j| JobRowData {
            id: j.id.clone().into(),
            kind: match j.kind {
                JobType::VideoEdit => "video-edit",
                JobType::Slideshow => "slideshow",
                JobType::Transfer => "transfer",
            }
            .into(),
            title: j.title.clone().into(),
            status: status_str(j.status).into(),
            progress: j.progress as f32,
            line: if j.kind == JobType::Transfer && j.status != JobStatus::Canceled {
                transfer_line(a, j, rates.get(&j.id).map(|r| r.2))
            } else {
                String::new()
            }
            .into(),
            error: j.error.clone().unwrap_or_default().into(),
            result: j
                .result
                .as_ref()
                .map(|r| r.name.clone())
                .unwrap_or_default()
                .into(),
        })
        .collect();
    let active: Vec<&Job> = list
        .iter()
        .filter(|j| matches!(j.status, JobStatus::Running | JobStatus::Queued))
        .collect();
    st.set_active(active.len() as i32);
    let running = list.iter().find(|j| j.status == JobStatus::Running);
    st.set_running(running.is_some());
    st.set_running_progress(running.map(|j| j.progress as f32).unwrap_or(0.0));
    st.set_any_finished(
        list.iter()
            .any(|j| !matches!(j.status, JobStatus::Running | JobStatus::Queued)),
    );
    st.set_jobs(ModelRc::from(Rc::new(VecModel::from(rows))));
}

/// GET /api/jobs
pub(crate) fn load(a: &Rc<AppInner>) {
    api::call::<pa_core::JobsResponse>(a, "GET", "/api/jobs", None, |a, r| {
        if let Ok(r) = r {
            let mut m = a.pages.jobs.jobs.borrow_mut();
            m.clear();
            for j in r.jobs {
                m.insert(j.id.clone(), j);
            }
            drop(m);
            publish(a);
        }
    });
}

/// a job event / a job started here
pub(crate) fn on_job(a: &Rc<AppInner>, j: &Job) {
    crate::video_editor::on_job(a, j);
    let prev = a.pages.jobs.jobs.borrow().get(&j.id).cloned();
    if let Some(b) = &j.bytes {
        let now = fmt::now_ms();
        let mut rates = a.pages.jobs.rates.borrow_mut();
        let r = rates.entry(j.id.clone()).or_insert((b.done, now, 0.0));
        let dt = (now - r.1) as f64 / 1000.0;
        if dt > 0.4 && b.done >= r.0 {
            let inst = (b.done - r.0) as f64 / dt;
            r.2 = if r.2 > 0.0 {
                r.2 * 0.6 + inst * 0.4
            } else {
                inst
            };
            r.0 = b.done;
            r.1 = now;
        }
    }
    a.pages
        .jobs
        .jobs
        .borrow_mut()
        .insert(j.id.clone(), j.clone());
    publish(a);
    // finished while we watched: a toast (the desktop shell also notifies when in the background)
    let was_active = prev
        .as_ref()
        .is_some_and(|p| matches!(p.status, JobStatus::Running | JobStatus::Queued));
    // the video editor toasts its own export
    if was_active && !crate::video_editor::owns_job(&j.id) {
        match j.status {
            JobStatus::Done => {
                let title = a.t("jobs.done");
                match &j.result {
                    Some(r) => super::dialogs::toast_open(
                        a,
                        &title,
                        &j.title,
                        Route::Folder(r.folder_id.clone(), r.dir.clone()),
                    ),
                    None => a.toast(ToastKind::Success, &title, &j.title),
                }
                if j.kind == JobType::Transfer {
                    super::library::refresh_meta(a);
                }
            }
            JobStatus::Error => a.toast(
                ToastKind::Error,
                &a.t("jobs.error"),
                j.error.as_deref().unwrap_or(&j.title),
            ),
            _ => {}
        }
    }
}

/// upload local files to the target (3 in parallel) with the upload panel
pub(crate) fn upload_files(
    a: &Rc<AppInner>,
    files: Vec<(PathBuf, String)>,
    t: DirTarget,
    zip: Option<String>,
) {
    if files.is_empty() {
        return;
    }
    let epoch = a.pages.jobs.up_epoch.get() + 1;
    a.pages.jobs.up_epoch.set(epoch);
    let base = {
        let mut ups = a.pages.jobs.ups.borrow_mut();
        ups.retain(|u| u.status != "done" && u.status != "canceled");
        let base = ups.len();
        for (p, rel) in &files {
            ups.push(Up {
                name: rel.clone(),
                size: crate::files::size(p),
                status: "queued".into(),
                error: String::new(),
            });
        }
        base
    };
    a.window.global::<JobsState>().set_uploads_open(true);
    publish_uploads(a);
    let queue = Rc::new(RefCell::new(
        files.into_iter().enumerate().collect::<Vec<_>>(),
    ));
    queue.borrow_mut().reverse();
    let counts = Rc::new(Cell::new((0usize, 0usize)));
    let running = Rc::new(Cell::new(0usize));
    for _ in 0..3 {
        next_upload(
            a,
            queue.clone(),
            base,
            t.clone(),
            zip.clone(),
            counts.clone(),
            running.clone(),
            epoch,
        );
    }
}

fn next_upload(
    a: &Rc<AppInner>,
    queue: UpQueue,
    base: usize,
    t: DirTarget,
    zip: Option<String>,
    counts: Rc<Cell<(usize, usize)>>,
    running: Rc<Cell<usize>>,
    epoch: u64,
) {
    if a.pages.jobs.up_epoch.get() != epoch {
        return;
    }
    let Some((i, (path, rel))) = queue.borrow_mut().pop() else {
        if running.get() == 0 {
            finish_uploads(a, counts.get());
        }
        return;
    };
    let idx = base + i;
    set_up(a, idx, "uploading", "");
    running.set(running.get() + 1);
    let is_zip = rel.to_lowercase().ends_with(".zip");
    let spec = UploadSpec {
        folder_id: t.folder_id.clone(),
        dir: t.dir.clone(),
        path: rel,
        zip: if is_zip {
            zip.as_deref()
                .and_then(|z| serde_json::from_value(serde_json::Value::String(z.into())).ok())
        } else {
            None
        },
        mtime: crate::files::mtime_ms(&path),
    };
    let fut = a.backend.upload_path(path, spec);
    let w = Rc::downgrade(a);
    a.rt.spawn_then(fut, move |r| {
        let Some(a) = w.upgrade() else { return };
        running.set(running.get().saturating_sub(1));
        let (mut ok, mut failed) = counts.get();
        match r {
            Ok(res) => {
                ok += 1;
                set_up(&a, idx, "done", "");
                if is_zip {
                    let v = serde_json::to_value(&res).unwrap_or_default();
                    if let Some(f) = v["tempFolder"]["id"].as_str() {
                        super::dialogs::toast_open(
                            &a,
                            &a.t("upload.tempReady"),
                            "",
                            Route::Folder(f.into(), String::new()),
                        );
                    }
                }
            }
            Err(e) => {
                failed += 1;
                set_up(&a, idx, "error", &e.message);
            }
        }
        counts.set((ok, failed));
        next_upload(&a, queue, base, t, zip, counts, running, epoch);
    });
}

fn finish_uploads(a: &Rc<AppInner>, (ok, failed): (usize, usize)) {
    a.grid.0.borrow_mut().reload();
    super::library::refresh_meta(a);
    if failed > 0 {
        a.toast(
            ToastKind::Error,
            &i18n::tv(a.lang(), "upload.failed", &[("n", &failed.to_string())]),
            "",
        );
    } else if ok > 0 {
        a.toast(
            ToastKind::Success,
            &a.t("upload.done"),
            &i18n::tv(a.lang(), "upload.doneBody", &[("n", &ok.to_string())]),
        );
    }
    let w = Rc::downgrade(a);
    slint::Timer::single_shot(std::time::Duration::from_millis(2500), move || {
        let Some(a) = w.upgrade() else { return };
        let all_done = a
            .pages
            .jobs
            .ups
            .borrow()
            .iter()
            .all(|u| u.status == "done" || u.status == "canceled");
        if all_done {
            a.window.global::<JobsState>().set_uploads_open(false);
            a.pages.jobs.ups.borrow_mut().clear();
            publish_uploads(&a);
        }
    });
}

fn set_up(a: &Rc<AppInner>, idx: usize, status: &str, err: &str) {
    if let Some(u) = a.pages.jobs.ups.borrow_mut().get_mut(idx) {
        u.status = status.into();
        u.error = err.into();
    }
    publish_uploads(a);
}

fn publish_uploads(a: &Rc<AppInner>) {
    let st = a.window.global::<JobsState>();
    let ups = a.pages.jobs.ups.borrow().clone();
    let total: i64 = ups.iter().map(|u| u.size).sum();
    let loaded: i64 = ups
        .iter()
        .filter(|u| u.status == "done")
        .map(|u| u.size)
        .sum();
    let done = ups.iter().filter(|u| u.status == "done").count();
    st.set_uploads_progress(if total > 0 {
        loaded as f32 / total as f32
    } else {
        0.0
    });
    st.set_uploads_active(
        ups.iter()
            .any(|u| u.status == "uploading" || u.status == "queued" || u.status == "processing"),
    );
    st.set_uploads_sub(
        format!(
            "{} · {} / {}",
            i18n::tv(
                a.lang(),
                "upload.progress",
                &[
                    ("done", &done.to_string()),
                    ("total", &ups.len().to_string())
                ]
            ),
            fmt::format_bytes(loaded),
            fmt::format_bytes(total)
        )
        .into(),
    );
    st.set_uploads(ModelRc::from(Rc::new(VecModel::from(
        ups.iter()
            .map(|u| UploadRowData {
                name: u.name.clone().into(),
                status: u.status.clone().into(),
                progress: if u.status == "done" {
                    1.0
                } else if u.status == "uploading" {
                    0.5
                } else {
                    0.0
                },
                size: fmt::format_bytes(u.size).into(),
                error: u.error.clone().into(),
            })
            .collect::<Vec<_>>(),
    ))));
}

pub(crate) fn install(app: &Rc<AppInner>) {
    let st = app.window.global::<JobsState>();
    let w = Rc::downgrade(app);
    st.on_cancel(move |id| {
        if let Some(a) = w.upgrade() {
            api::act(
                &a,
                "DELETE",
                &format!("/api/jobs/{}", api::enc(&id)),
                None,
                |a, _| load(a),
            );
        }
    });
    let w = Rc::downgrade(app);
    st.on_clear(move || {
        if let Some(a) = w.upgrade() {
            api::act(&a, "DELETE", "/api/jobs", None, |a, _| load(a));
        }
    });
    let w = Rc::downgrade(app);
    st.on_open_result(move |id| {
        let Some(a) = w.upgrade() else { return };
        let r = a
            .pages
            .jobs
            .jobs
            .borrow()
            .get(id.as_str())
            .and_then(|j| j.result.clone());
        if let Some(r) = r {
            a.go(Route::Folder(r.folder_id, r.dir), true);
        }
    });
    let w = Rc::downgrade(app);
    st.on_cancel_uploads(move || {
        let Some(a) = w.upgrade() else { return };
        // stop starting new ones; running requests finish
        a.pages.jobs.up_epoch.set(a.pages.jobs.up_epoch.get() + 1);
        for u in a.pages.jobs.ups.borrow_mut().iter_mut() {
            if u.status == "queued" {
                u.status = "canceled".into();
            }
        }
        publish_uploads(&a);
    });
    let w = Rc::downgrade(app);
    st.on_refresh(move || {
        if let Some(a) = w.upgrade() {
            load(&a);
        }
    });
}
