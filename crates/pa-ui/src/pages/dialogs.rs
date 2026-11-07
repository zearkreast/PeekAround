//! Dialogs (ui/dialogs/dialogs.slint): confirm, text prompt, folder pickers (library folders,
//! host directories, phones), library editor, exclusions, MEGA link, pairing, zip / link prompts,
//! keyboard shortcuts; uploads ("Add" → files / folder) with the zip prompt; downloads; toasts
//! with an action.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use pa_core::{BrowseResult, DeviceInfo, DirTarget, FileRef, Library, RemoteInspect};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::api::{self, enc};
use crate::app::AppInner;
use crate::fmt;
use crate::i18n;
use crate::router::Route;
use crate::{
    Chrome, Dlg, KeyRowData, PathRowData, PickRowData, SegOption, SelectOption, Shell, ToastData,
    ToastKind, TreeRowData,
};

type Then = Box<dyn FnOnce(&Rc<AppInner>)>;
type ThenText = Box<dyn FnOnce(&Rc<AppInner>, String)>;
type ThenTarget = Box<dyn FnOnce(&Rc<AppInner>, DirTarget)>;
type ThenChoice = Box<dyn FnOnce(&Rc<AppInner>, Option<String>)>;
type KeyGroups<'a> = Vec<(&'a str, Vec<(&'a str, Vec<String>)>)>;

#[derive(Clone)]
struct Step {
    folder_id: String,
    dir: String,
    label: String,
}

enum PickMode {
    /// library folders → sub folders
    Folders,
    /// phones → their shared folders → sub folders
    Phones,
    /// server directories (server mode "add folder")
    Host,
}

struct Picker {
    mode: PickMode,
    stack: Vec<Step>,
    host_path: Option<String>,
    host_parent: Option<String>,
    on_pick: Option<ThenTarget>,
    on_host: Option<ThenText>,
}

struct LibEdit {
    id: Option<String>,
    paths: Vec<String>,
}

struct Mega {
    url: String,
    intent: String,
    library_id: Option<String>,
    data: Option<RemoteInspect>,
    /// node id → children ids; files below a node
    children: HashMap<String, Vec<String>>,
    files: HashMap<String, Vec<String>>,
    root: String,
    sel: HashSet<String>,
    open: HashSet<String>,
    target: Option<(DirTarget, String)>,
    req: u64,
}

/// dialog state
#[derive(Default)]
pub(crate) struct Dialogs {
    on_top: RefCell<Option<Then>>,
    on_prompt: RefCell<Option<ThenText>>,
    picker: RefCell<Option<Picker>>,
    lib: RefCell<Option<LibEdit>>,
    ex_lib: RefCell<Option<String>>,
    mega: RefCell<Option<Mega>>,
    pairs: RefCell<Vec<DeviceInfo>>,
    on_zip: RefCell<Option<ThenChoice>>,
    on_link: RefCell<Option<ThenChoice>>,
    toast_actions: RefCell<HashMap<i32, Then>>,
}

fn dlg(a: &AppInner) -> Dlg<'_> {
    a.window.global::<Dlg>()
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

// ───────── toasts with an action ─────────

/// a toast with an action button
pub(crate) fn toast_with(
    a: &Rc<AppInner>,
    kind: ToastKind,
    title: &str,
    body: &str,
    action: &str,
    run: impl FnOnce(&Rc<AppInner>) + 'static,
) {
    a.toast(kind, title, body);
    // the toast just pushed is the last one: give it the action
    let n = slint::Model::row_count(&*a.toasts);
    if n == 0 {
        return;
    }
    if let Some(mut t) = slint::Model::row_data(&*a.toasts, n - 1) {
        t.action = action.into();
        let id = t.id;
        slint::Model::set_row_data(&*a.toasts, n - 1, t);
        a.pages
            .dialogs
            .toast_actions
            .borrow_mut()
            .insert(id, Box::new(run));
    }
}

/// a success toast with "Open" → route
pub(crate) fn toast_open(a: &Rc<AppInner>, title: &str, body: &str, route: Route) {
    let label = a.t("common.open");
    toast_with(a, ToastKind::Success, title, body, &label, move |a| {
        a.go(route, true)
    });
}

/// a newer release: toast with the release link
pub(crate) fn update_toast(a: &Rc<AppInner>, u: &crate::host::UpdateInfo) {
    let lang = a.lang();
    let latest = u.latest.clone().unwrap_or_default();
    let url = u.url.clone().unwrap_or_default();
    let title = i18n::tv(lang, "pref.desk.updateAvailable", &[("v", &latest)]);
    let label = a.t("common.open");
    toast_with(a, ToastKind::Info, &title, "", &label, move |a| {
        a.host.open_url(&url)
    });
}

/// a progress toast (kind progress, value) — returns its id
pub(crate) fn toast_progress(a: &Rc<AppInner>, title: &str, value: f32) -> i32 {
    a.toast(ToastKind::Progress, title, "");
    let n = slint::Model::row_count(&*a.toasts);
    let Some(mut t) = (n > 0)
        .then(|| slint::Model::row_data(&*a.toasts, n - 1))
        .flatten()
    else {
        return -1;
    };
    t.progress = value;
    let id = t.id;
    slint::Model::set_row_data(&*a.toasts, n - 1, t);
    id
}

/// replace a toast (by id) in place
pub(crate) fn toast_update(a: &Rc<AppInner>, id: i32, kind: ToastKind, title: &str, body: &str) {
    let n = slint::Model::row_count(&*a.toasts);
    for i in 0..n {
        if let Some(t) = slint::Model::row_data(&*a.toasts, i) {
            if t.id == id {
                slint::Model::set_row_data(
                    &*a.toasts,
                    i,
                    ToastData {
                        kind,
                        title: title.into(),
                        body: body.into(),
                        ..t
                    },
                );
                return;
            }
        }
    }
    a.toast(kind, title, body);
}

// ───────── confirm / prompt ─────────

/// "Are you sure?" over everything
pub(crate) fn confirm(
    a: &Rc<AppInner>,
    title: &str,
    body: &str,
    label: &str,
    danger: bool,
    then: impl FnOnce(&Rc<AppInner>) + 'static,
) {
    let d = dlg(a);
    d.set_c_title(title.into());
    d.set_c_body(body.into());
    d.set_c_label(label.into());
    d.set_c_danger(danger);
    d.set_busy(false);
    *a.pages.dialogs.on_top.borrow_mut() = Some(Box::new(then));
    d.set_top("confirm".into());
}

/// a one-line text dialog
pub(crate) fn prompt(
    a: &Rc<AppInner>,
    title: &str,
    placeholder: &str,
    value: &str,
    label: &str,
    then: impl FnOnce(&Rc<AppInner>, String) + 'static,
) {
    let d = dlg(a);
    d.set_c_title(title.into());
    d.set_prompt_placeholder(placeholder.into());
    d.set_prompt_text(value.into());
    d.set_c_label(label.into());
    d.set_busy(false);
    *a.pages.dialogs.on_prompt.borrow_mut() = Some(Box::new(then));
    d.set_top("prompt".into());
}

fn close_top(a: &Rc<AppInner>) {
    dlg(a).set_top("".into());
    a.pages.dialogs.on_top.borrow_mut().take();
    a.pages.dialogs.on_prompt.borrow_mut().take();
    a.pages.dialogs.picker.borrow_mut().take();
}

/// "New album" (empty; items are added from the grid)
pub(crate) fn new_album(a: &Rc<AppInner>) {
    let (t, ph, l) = (a.t("album.new"), a.t("album.newName"), a.t("common.create"));
    prompt(a, &t, &ph, "", &l, |a, name| {
        let name = if name.trim().is_empty() {
            a.t("album.defaultName")
        } else {
            name.trim().to_string()
        };
        api::call::<pa_core::Album>(
            a,
            "POST",
            "/api/albums",
            Some(serde_json::json!({ "name": name, "ids": [] })),
            |a, r| match r {
                Ok(al) => {
                    super::albums::load_albums(a);
                    a.go(Route::Album(al.id), true);
                }
                Err(e) => a.error_toast(&e.message),
            },
        );
    });
}

/// "New folder" in the open folder
pub(crate) fn new_folder(a: &Rc<AppInner>) {
    let Some(t) = super::library::target(a) else {
        return;
    };
    let (title, ph, l) = (
        a.t("gallery.newFolder"),
        a.t("gallery.newFolderName"),
        a.t("common.create"),
    );
    prompt(a, &title, &ph, "", &l, move |a, name| {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        api::call::<DirTarget>(
            a,
            "POST",
            "/api/fs/mkdir",
            Some(serde_json::json!({ "folderId": t.folder_id, "dir": t.dir, "name": name })),
            |a, r| match r {
                Ok(d) => a.go(Route::Folder(d.folder_id, d.dir), true),
                Err(e) => a.error_toast(&e.message),
            },
        );
    });
}

/// rename a folder (folder card menu)
pub(crate) fn rename_ref(a: &Rc<AppInner>, r: FileRef, name: &str) {
    let (title, l) = (a.t("common.rename"), a.t("common.rename"));
    prompt(a, &title, "", name, &l, move |a, new| {
        let new = new.trim().to_string();
        if new.is_empty() {
            return;
        }
        api::act(
            a,
            "POST",
            "/api/fs/rename",
            Some(serde_json::json!({ "folderId": r.folder_id, "rel": r.rel, "name": new })),
            |a, _| {
                super::library::refresh_meta(a);
            },
        );
    });
}

/// delete folders / files (to the trash where there is one)
pub(crate) fn delete_refs(a: &Rc<AppInner>, refs: Vec<FileRef>, name: &str) {
    let title = i18n::tv(
        a.lang(),
        "sel.deleteTitle",
        &[("n", &refs.len().to_string()), ("name", name)],
    );
    let body = a.t("sel.deleteBody");
    let label = a.t("common.delete");
    confirm(a, &title, &body, &label, true, move |a| {
        api::act(
            a,
            "POST",
            "/api/fs/delete",
            Some(serde_json::json!({ "refs": refs })),
            |a, _| {
                super::library::refresh_meta(a);
                a.grid.0.borrow_mut().reload();
            },
        );
    });
}

/// move folders into another folder
pub(crate) fn move_refs(a: &Rc<AppInner>, refs: Vec<FileRef>) {
    let (title, label) = (a.t("sel.move"), a.t("sel.move"));
    pick_folder(a, &title, &label, move |a, to| {
        api::act(
            a,
            "POST",
            "/api/fs/move",
            Some(serde_json::json!({ "refs": refs, "to": to, "copy": false })),
            |a, v| {
                if let Ok(j) = serde_json::from_value::<pa_core::Job>(v["job"].clone()) {
                    super::jobs::on_job(a, &j);
                }
                super::library::refresh_meta(a);
                a.grid.0.borrow_mut().reload();
            },
        );
    });
}

/// copy phone / MEGA folders to this computer (devices "copyToPc", RemoteRow "copy")
pub(crate) fn copy_to_pc(a: &Rc<AppInner>, refs: Vec<FileRef>, label: &str, remote: bool) {
    let title = i18n::tv(a.lang(), "devices.copyToPcTitle", &[("what", label)]);
    let confirm_l = a.t("devices.copyHere");
    pick_folder(a, &title, &confirm_l, move |a, to| {
        let (path, body) = if remote {
            (
                "/api/remote/copy",
                serde_json::json!({ "refs": refs, "to": to }),
            )
        } else {
            (
                "/api/fs/move",
                serde_json::json!({ "refs": refs, "to": to, "copy": true }),
            )
        };
        api::act(a, "POST", path, Some(body), |a, v| {
            let job = serde_json::from_value::<pa_core::Job>(v.clone())
                .ok()
                .or_else(|| serde_json::from_value::<pa_core::Job>(v["job"].clone()).ok());
            if let Some(j) = job {
                super::jobs::on_job(a, &j);
            }
            a.toast(ToastKind::Info, &a.t("devices.copyStarted"), "");
        });
    });
}

// ───────── downloads ─────────

/// zip download (album / folder): save as → stream to the file
pub(crate) fn download(a: &Rc<AppInner>, path: &str, name: &str) {
    let name = sanitize_file_name(name);
    if a.backend.url(path).is_none() {
        return;
    }
    let fut = a.host.save_file(&a.t("common.download"), &name);
    let path = path.to_string();
    let w = Rc::downgrade(a);
    a.rt.spawn_then(fut, move |dest| {
        let (Some(a), Some(dest)) = (w.upgrade(), dest) else {
            return;
        };
        let id = toast_progress(&a, &a.t("common.download"), 0.3);
        let fut = a.backend.download_to(&path, dest.clone());
        let w = Rc::downgrade(&a);
        a.rt.spawn_then(fut, move |r| {
            let Some(a) = w.upgrade() else { return };
            match r {
                Ok(n) => {
                    toast_update(
                        &a,
                        id,
                        ToastKind::Success,
                        &a.t("common.download"),
                        &format!("{} · {}", dest.display(), fmt::format_bytes(n as i64)),
                    );
                    a.host.show_in_folder(&dest.to_string_lossy());
                }
                Err(e) => toast_update(&a, id, ToastKind::Error, &a.t("common.error"), &e.message),
            }
        });
    });
}

fn sanitize_file_name(n: &str) -> String {
    let s: String = n
        .chars()
        .map(|c| {
            if r#"<>:"/\|?*"#.contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    if s.trim().is_empty() {
        "PeekAround.zip".into()
    } else {
        s
    }
}

// ───────── folder pickers ─────────

/// pick a library folder (+ sub folder) as a destination
pub(crate) fn pick_folder(
    a: &Rc<AppInner>,
    title: &str,
    confirm_label: &str,
    then: impl FnOnce(&Rc<AppInner>, DirTarget) + 'static,
) {
    let d = dlg(a);
    d.set_pick_title(title.into());
    d.set_pick_confirm(
        if confirm_label.is_empty() {
            a.t("common.done")
        } else {
            confirm_label.to_string()
        }
        .into(),
    );
    d.set_pick_host(false);
    d.set_pick_naming(false);
    d.set_pick_empty("".into());
    *a.pages.dialogs.picker.borrow_mut() = Some(Picker {
        mode: PickMode::Folders,
        stack: vec![],
        host_path: None,
        host_parent: None,
        on_pick: Some(Box::new(then)),
        on_host: None,
    });
    publish_picker(a);
    d.set_top("picker".into());
}

/// "send to phone": a phone's shared folder (+ sub folder)
pub(crate) fn pick_phone_folder(
    a: &Rc<AppInner>,
    what: &str,
    then: impl FnOnce(&Rc<AppInner>, DirTarget) + 'static,
) {
    let d = dlg(a);
    d.set_pick_title(i18n::tv(a.lang(), "devices.send.title", &[("what", what)]).into());
    d.set_pick_confirm(a.t("devices.send.confirm").into());
    d.set_pick_host(false);
    d.set_pick_naming(false);
    *a.pages.dialogs.picker.borrow_mut() = Some(Picker {
        mode: PickMode::Phones,
        stack: vec![],
        host_path: None,
        host_parent: None,
        on_pick: Some(Box::new(then)),
        on_host: None,
    });
    publish_picker(a);
    d.set_top("picker".into());
}

fn pick_host(a: &Rc<AppInner>, then: impl FnOnce(&Rc<AppInner>, String) + 'static) {
    let d = dlg(a);
    d.set_pick_title(a.t("lib.hostBrowser").into());
    d.set_pick_confirm(a.t("lib.useThis").into());
    d.set_pick_host(true);
    d.set_pick_naming(false);
    d.set_pick_manual("".into());
    d.set_pick_empty("".into());
    *a.pages.dialogs.picker.borrow_mut() = Some(Picker {
        mode: PickMode::Host,
        stack: vec![],
        host_path: None,
        host_parent: None,
        on_pick: None,
        on_host: Some(Box::new(then)),
    });
    load_host_dirs(a, None);
    d.set_top("picker".into());
}

fn load_host_dirs(a: &Rc<AppInner>, path: Option<String>) {
    dlg(a).set_pick_loading(true);
    let q = match &path {
        Some(p) => api::qs(&[("path", p)]),
        None => String::new(),
    };
    api::call::<serde_json::Value>(a, "GET", &format!("/api/host/dirs{q}"), None, |a, r| {
        let d = dlg(a);
        d.set_pick_loading(false);
        match r {
            Ok(v) => {
                let path = v["path"].as_str().map(str::to_string);
                let parent = v["parent"].as_str().map(str::to_string);
                let rows: Vec<PickRowData> = v["dirs"]
                    .as_array()
                    .map(|l| {
                        l.iter()
                            .map(|x| PickRowData {
                                key: format!("path:{}", x["path"].as_str().unwrap_or("")).into(),
                                label: x["name"].as_str().unwrap_or("").into(),
                                kind: if path.is_some() { "folder" } else { "drive" }.into(),
                                ..Default::default()
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                d.set_pick_rows(model(rows));
                d.set_pick_path(path.clone().unwrap_or_else(|| "/".into()).into());
                d.set_pick_can_back(path.is_some() || parent.is_some());
                d.set_pick_can_new(path.is_some());
                d.set_pick_can_confirm(path.is_some());
                if let Some(p) = a.pages.dialogs.picker.borrow_mut().as_mut() {
                    p.host_path = path;
                    p.host_parent = parent;
                }
            }
            Err(e) => a.error_toast(&e.message),
        }
    });
}

fn publish_picker(a: &Rc<AppInner>) {
    let d = dlg(a);
    let lang = a.lang();
    let (mode_phones, cur) = {
        let p = a.pages.dialogs.picker.borrow();
        let Some(p) = p.as_ref() else { return };
        (matches!(p.mode, PickMode::Phones), p.stack.last().cloned())
    };
    let path = {
        let p = a.pages.dialogs.picker.borrow();
        p.as_ref()
            .map(|p| {
                p.stack
                    .iter()
                    .map(|s| s.label.clone())
                    .collect::<Vec<_>>()
                    .join(" / ")
            })
            .unwrap_or_default()
    };
    d.set_pick_path(
        if path.is_empty() {
            a.t(if mode_phones {
                "devices.send.pick"
            } else {
                "nav.libraries"
            })
        } else {
            path
        }
        .into(),
    );
    d.set_pick_can_back(cur.is_some());
    d.set_pick_can_new(cur.is_some() && !mode_phones);
    d.set_pick_can_confirm(cur.is_some());
    match cur {
        None => {
            let libs = a
                .pages
                .libs
                .borrow()
                .clone()
                .unwrap_or(pa_core::LibrariesResponse {
                    libraries: vec![],
                    temp: vec![],
                    devices: vec![],
                });
            let mut rows = vec![];
            if mode_phones {
                for dv in libs.devices.iter().filter(|d| d.paired) {
                    let usable = dv.online && dv.caps.as_ref().is_none_or(|c| c.receive);
                    rows.push(PickRowData {
                        key: "".into(),
                        label: dv.name.clone().into(),
                        kind: "header".into(),
                        icon: if dv.online { "online" } else { "" }.into(),
                        sub: if !dv.online {
                            a.t("devices.offline")
                        } else if !usable {
                            a.t("devices.noReceive")
                        } else {
                            String::new()
                        }
                        .into(),
                        ..Default::default()
                    });
                    for f in &dv.folders {
                        rows.push(PickRowData {
                            key: format!("root:{}:{} · {}", f.id, dv.name, f.name).into(),
                            label: f.name.clone().into(),
                            kind: "folder".into(),
                            disabled: !usable,
                            ..Default::default()
                        });
                    }
                }
                d.set_pick_empty(
                    if rows.is_empty() {
                        a.t("devices.none")
                    } else {
                        String::new()
                    }
                    .into(),
                );
            } else {
                for l in &libs.libraries {
                    for f in l
                        .folders
                        .iter()
                        .filter(|f| !matches!(f.remote, Some(Some(_))) && f.device.is_none())
                    {
                        let label = if l.folders.len() > 1 {
                            format!("{} · {}", l.name, f.name)
                        } else {
                            l.name.clone()
                        };
                        let step_label = if l.folders.len() > 1 {
                            f.name.clone()
                        } else {
                            l.name.clone()
                        };
                        rows.push(PickRowData {
                            key: format!("root:{}:{}", f.id, step_label).into(),
                            label: label.into(),
                            kind: "lib".into(),
                            color: crate::theme::color(&l.color),
                            icon: l.icon.clone().into(),
                            ..Default::default()
                        });
                    }
                }
                d.set_pick_empty("".into());
            }
            d.set_pick_loading(false);
            d.set_pick_rows(model(rows));
        }
        Some(s) => {
            d.set_pick_loading(true);
            d.set_pick_rows(model(vec![]));
            let q = api::qs(&[("folderId", &s.folder_id), ("dir", &s.dir), ("limit", "1")]);
            let s2 = s.clone();
            api::call::<BrowseResult>(a, "GET", &format!("/api/browse{q}"), None, move |a, r| {
                let still = a
                    .pages
                    .dialogs
                    .picker
                    .borrow()
                    .as_ref()
                    .and_then(|p| p.stack.last().cloned())
                    .is_some_and(|c| c.folder_id == s2.folder_id && c.dir == s2.dir);
                if !still {
                    return;
                }
                let d = dlg(a);
                d.set_pick_loading(false);
                let rows: Vec<PickRowData> = r
                    .map(|b| b.folders)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|f| PickRowData {
                        key: format!("dir:{}:{}", f.dir, f.name).into(),
                        label: f.name.into(),
                        kind: "folder".into(),
                        ..Default::default()
                    })
                    .collect();
                let phones = a
                    .pages
                    .dialogs
                    .picker
                    .borrow()
                    .as_ref()
                    .is_some_and(|p| matches!(p.mode, PickMode::Phones));
                d.set_pick_empty(
                    if rows.is_empty() && phones {
                        a.t("devices.send.here")
                    } else {
                        String::new()
                    }
                    .into(),
                );
                d.set_pick_rows(model(rows));
            });
        }
    }
    let _ = lang;
}

fn picker_open(a: &Rc<AppInner>, key: &str) {
    if let Some(p) = key.strip_prefix("path:") {
        load_host_dirs(a, Some(p.to_string()));
        return;
    }
    {
        let mut pk = a.pages.dialogs.picker.borrow_mut();
        let Some(p) = pk.as_mut() else { return };
        if let Some(rest) = key.strip_prefix("root:") {
            let (fid, label) = rest.split_once(':').unwrap_or((rest, ""));
            p.stack = vec![Step {
                folder_id: fid.into(),
                dir: String::new(),
                label: label.into(),
            }];
        } else if let Some(rest) = key.strip_prefix("dir:") {
            let Some(cur) = p.stack.last().cloned() else {
                return;
            };
            let (dir, name) = rest.rsplit_once(':').unwrap_or((rest, rest));
            p.stack.push(Step {
                folder_id: cur.folder_id,
                dir: dir.into(),
                label: name.into(),
            });
        }
    }
    publish_picker(a);
}

fn picker_confirm(a: &Rc<AppInner>) {
    let p = a.pages.dialogs.picker.borrow_mut().take();
    dlg(a).set_top("".into());
    let Some(mut p) = p else { return };
    match p.mode {
        PickMode::Host => {
            if let (Some(f), Some(path)) = (p.on_host.take(), p.host_path.clone()) {
                f(a, path);
            }
        }
        _ => {
            if let (Some(f), Some(s)) = (p.on_pick.take(), p.stack.last().cloned()) {
                f(
                    a,
                    DirTarget {
                        folder_id: s.folder_id,
                        dir: s.dir,
                    },
                );
            }
        }
    }
}

// ───────── library editor ─────────

fn lib_by_id(a: &AppInner, id: &str) -> Option<Library> {
    a.pages
        .libs
        .borrow()
        .as_ref()?
        .libraries
        .iter()
        .find(|l| l.id == id)
        .cloned()
}

fn publish_lib(a: &Rc<AppInner>) {
    let d = dlg(a);
    let st = a.pages.dialogs.lib.borrow();
    let Some(st) = st.as_ref() else { return };
    let mut rows: Vec<PathRowData> = vec![];
    if let Some(l) = st.id.as_deref().and_then(|id| lib_by_id(a, id)) {
        for f in &l.folders {
            rows.push(PathRowData {
                id: f.id.clone().into(),
                name: f.name.clone().into(),
                path: match &f.remote {
                    Some(Some(r)) => r.label.clone(),
                    _ => f.path.clone(),
                }
                .into(),
                remote: matches!(f.remote, Some(Some(_))),
                pending: false,
            });
        }
    }
    for p in &st.paths {
        rows.push(PathRowData {
            id: "".into(),
            name: "".into(),
            path: p.clone().into(),
            remote: false,
            pending: true,
        });
    }
    d.set_lib_folders(model(rows));
}

/// add / edit a library (None = new)
pub(crate) fn edit_library(a: &Rc<AppInner>, id: Option<String>) {
    let d = dlg(a);
    match id.as_deref().and_then(|i| lib_by_id(a, i)) {
        Some(l) => {
            d.set_lib_new(false);
            d.set_lib_name(l.name.clone().into());
            d.set_lib_color(crate::theme::color(&l.color));
            d.set_lib_icon(l.icon.clone().into());
        }
        None => {
            d.set_lib_new(true);
            d.set_lib_name("".into());
            let n = (fmt::now_ms() as usize / 7) % crate::prefs::ACCENTS.len();
            d.set_lib_color(crate::theme::color(crate::prefs::ACCENTS[n]));
            d.set_lib_icon("images".into());
        }
    }
    d.set_lib_mega(!a.window.global::<Shell>().get_readonly());
    d.set_busy(false);
    *a.pages.dialogs.lib.borrow_mut() = Some(LibEdit {
        id: id.filter(|i| lib_by_id(a, i).is_some()),
        paths: vec![],
    });
    publish_lib(a);
    d.set_open("library".into());
}

fn lib_add_paths(a: &Rc<AppInner>, ps: Vec<String>) {
    let d = dlg(a);
    if d.get_lib_name().is_empty() {
        if let Some(n) = ps
            .first()
            .and_then(|p| p.split(['/', '\\']).rfind(|s| !s.is_empty()))
        {
            d.set_lib_name(n.into());
        }
    }
    if let Some(st) = a.pages.dialogs.lib.borrow_mut().as_mut() {
        for p in ps {
            if !st.paths.contains(&p) {
                st.paths.push(p);
            }
        }
    }
    publish_lib(a);
}

fn lib_pick(a: &Rc<AppInner>) {
    if a.host.is_desktop() {
        let fut = a.host.pick_folders(&a.t("lib.addFolder"));
        let w = Rc::downgrade(a);
        a.rt.spawn_then(fut, move |paths| {
            if let Some(a) = w.upgrade() {
                lib_add_paths(
                    &a,
                    paths
                        .into_iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect(),
                );
            }
        });
    } else {
        pick_host(a, |a, p| lib_add_paths(a, vec![p]));
    }
}

fn hex(c: slint::Color) -> String {
    crate::theme::to_hex((c.red(), c.green(), c.blue()))
}

fn lib_save(a: &Rc<AppInner>) {
    let d = dlg(a);
    let Some((id, paths)) = a
        .pages
        .dialogs
        .lib
        .borrow()
        .as_ref()
        .map(|s| (s.id.clone(), s.paths.clone()))
    else {
        return;
    };
    let name = d.get_lib_name().trim().to_string();
    let color = hex(d.get_lib_color());
    let icon = d.get_lib_icon().to_string();
    d.set_busy(true);
    match id {
        None => {
            let body = serde_json::json!({ "name": if name.is_empty() { "Library".to_string() } else { name }, "color": color, "icon": icon, "folders": paths });
            api::call::<Library>(a, "POST", "/api/libraries", Some(body), |a, r| {
                dlg(a).set_busy(false);
                match r {
                    Ok(l) => {
                        dlg(a).set_open("".into());
                        a.toast(ToastKind::Success, &a.t("lib.created"), "");
                        super::hub::load_libraries(a);
                        a.go(Route::Lib(l.id), true);
                    }
                    Err(e) => a.error_toast(&e.message),
                }
            });
        }
        Some(id) => {
            let body = serde_json::json!({ "name": name, "color": color, "icon": icon });
            let id2 = id.clone();
            api::call::<serde_json::Value>(
                a,
                "PATCH",
                &format!("/api/libraries/{}", enc(&id)),
                Some(body),
                move |a, r| {
                    if let Err(e) = r {
                        dlg(a).set_busy(false);
                        a.error_toast(&e.message);
                        return;
                    }
                    add_folders_seq(a, id2, paths);
                },
            );
        }
    }
}

fn add_folders_seq(a: &Rc<AppInner>, id: String, mut paths: Vec<String>) {
    if paths.is_empty() {
        dlg(a).set_busy(false);
        dlg(a).set_open("".into());
        super::hub::load_libraries(a);
        return;
    }
    let p = paths.remove(0);
    let id2 = id.clone();
    api::call::<serde_json::Value>(
        a,
        "POST",
        &format!("/api/libraries/{}/folders", enc(&id)),
        Some(serde_json::json!({ "path": p })),
        move |a, r| {
            if let Err(e) = r {
                a.error_toast(&e.message);
            }
            add_folders_seq(a, id2, paths);
        },
    );
}

// ───────── exclusions ─────────

const EX_SIZES: [i64; 4] = [0, 10 * 1024, 30 * 1024, 100 * 1024];
const EX_SUGGEST: [&str; 7] = [
    "Thumbnails",
    ".thumbnails",
    "WhatsApp Stickers",
    "Telegram Stickers",
    "cache",
    "*.tmp",
    "@eaDir",
];

fn ex_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn publish_ex_suggest(a: &AppInner) {
    let d = dlg(a);
    let lines = ex_lines(&d.get_ex_text());
    let v: Vec<SharedString> = EX_SUGGEST
        .iter()
        .filter(|s| !lines.iter().any(|l| l == *s))
        .map(|s| (*s).into())
        .collect();
    d.set_ex_suggest(model(v));
}

/// per-library exclusions
pub(crate) fn exclusions(a: &Rc<AppInner>, id: &str) {
    let Some(l) = lib_by_id(a, id) else { return };
    let d = dlg(a);
    let o = l.options.clone().unwrap_or_default();
    d.set_ex_title(i18n::tv(a.lang(), "lib2.ex.title", &[("name", &l.name)]).into());
    d.set_ex_text(o.exclude.join("\n").into());
    let mut sizes: Vec<i64> = EX_SIZES.to_vec();
    if !sizes.contains(&o.min_bytes) {
        sizes.push(o.min_bytes);
        sizes.sort();
    }
    d.set_ex_sizes(model(
        sizes
            .iter()
            .map(|b| SegOption {
                value: b.to_string().into(),
                label: if *b == 0 {
                    a.t("lib2.ex.off")
                } else {
                    fmt::format_bytes(*b)
                }
                .into(),
                ..Default::default()
            })
            .collect(),
    ));
    d.set_ex_min(o.min_bytes.to_string().into());
    d.set_ex_hidden(match o.show_hidden {
        None => -1,
        Some(false) => 0,
        Some(true) => 1,
    });
    d.set_busy(false);
    *a.pages.dialogs.ex_lib.borrow_mut() = Some(l.id.clone());
    publish_ex_suggest(a);
    d.set_open("exclusions".into());
}

fn ex_save(a: &Rc<AppInner>) {
    let d = dlg(a);
    let Some(id) = a.pages.dialogs.ex_lib.borrow().clone() else {
        return;
    };
    let hidden = match d.get_ex_hidden() {
        -1 => serde_json::Value::Null,
        0 => false.into(),
        _ => true.into(),
    };
    let body = serde_json::json!({ "options": {
        "exclude": ex_lines(&d.get_ex_text()),
        "minBytes": d.get_ex_min().parse::<i64>().unwrap_or(0),
        "showHidden": hidden,
    }});
    d.set_busy(true);
    api::call::<serde_json::Value>(
        a,
        "PATCH",
        &format!("/api/libraries/{}", enc(&id)),
        Some(body),
        |a, r| {
            dlg(a).set_busy(false);
            match r {
                Ok(_) => {
                    dlg(a).set_open("".into());
                    a.toast(ToastKind::Success, &a.t("lib2.ex.saved"), "");
                    super::hub::load_libraries(a);
                }
                Err(e) => a.error_toast(&e.message),
            }
        },
    );
}

// ───────── MEGA ─────────

/// first MEGA link in text (mega.ts `findMegaUrl`)
pub(crate) fn find_mega_url(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    for host in ["mega.nz/", "mega.co.nz/"] {
        let Some(i) = lower.find(host) else { continue };
        // back to the start of the URL (scheme / www.)
        let mut start = i;
        let prefix = &lower[..i];
        for p in ["https://www.", "http://www.", "https://", "http://", "www."] {
            if prefix.ends_with(p) {
                start = i - p.len();
                break;
            }
        }
        let rest = &text[i + host.len()..];
        let id_ok = |s: &str, min: usize| {
            s.len() >= min
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        };
        let take = |s: &str| -> usize {
            s.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                .unwrap_or(s.len())
        };
        let end;
        if let Some(r) = rest
            .strip_prefix("file/")
            .or_else(|| rest.strip_prefix("folder/"))
        {
            let n = take(r);
            if !id_ok(&r[..n], 6) || !r[n..].starts_with('#') {
                continue;
            }
            let k = &r[n + 1..];
            let kn = take(k);
            if !id_ok(&k[..kn], 8) {
                continue;
            }
            let kind_len = rest.len() - r.len();
            let mut e = kind_len + n + 1 + kn;
            // optional /file|folder/<id>
            let tail = &rest[e..];
            for sub in ["/file/", "/folder/"] {
                if let Some(t2) = tail.strip_prefix(sub) {
                    let m = take(t2);
                    if id_ok(&t2[..m], 6) {
                        e += sub.len() + m;
                    }
                    break;
                }
            }
            end = i + host.len() + e;
        } else if let Some(r) = rest.strip_prefix("#F!").or_else(|| rest.strip_prefix("#!")) {
            let parts: Vec<&str> = r.splitn(3, '!').collect();
            if parts.len() < 2 {
                continue;
            }
            let a0 = take(parts[0]);
            let a1 = take(parts[1]);
            if !id_ok(&parts[0][..a0], 6) || a0 != parts[0].len() || !id_ok(&parts[1][..a1], 8) {
                continue;
            }
            let mut e = (rest.len() - r.len()) + a0 + 1 + a1;
            if a1 == parts[1].len() {
                if let Some(p2) = parts.get(2) {
                    let a2 = take(p2);
                    if id_ok(&p2[..a2], 6) {
                        e += 1 + a2;
                    }
                }
            }
            end = i + host.len() + e;
        } else {
            continue;
        }
        let raw = &text[start..end];
        let low = raw.to_lowercase();
        return Some(if low.starts_with("https://") {
            raw.to_string()
        } else if low.starts_with("http://") {
            format!("https://{}", &raw[7..])
        } else {
            format!("https://{raw}")
        });
    }
    None
}

fn looks_like_mega(text: &str) -> bool {
    let l = text.to_lowercase();
    l.contains("mega.nz") || l.contains("mega.co.nz")
}

/// open the MEGA dialog (`url` "": ask for one); intent "stream" | "copy" | ""
pub(crate) fn open_mega(a: &Rc<AppInner>, url: &str, intent: &str) {
    open_mega_for(a, url, intent, None)
}

/// [`open_mega`] for a library (editor → "Add MEGA link")
pub(crate) fn open_mega_for(a: &Rc<AppInner>, url: &str, intent: &str, library_id: Option<String>) {
    let d = dlg(a);
    let stream = intent == "stream";
    d.set_mega_title(
        a.t(if stream {
            "mega.addLink"
        } else {
            "remote.title"
        })
        .into(),
    );
    d.set_mega_intro(
        if stream {
            format!("{}. {}", a.t("mega.addLinkHint"), a.t("remote.intro"))
        } else {
            a.t("remote.intro")
        }
        .into(),
    );
    d.set_mega_url(url.into());
    d.set_mega_stage("input".into());
    d.set_readonly(a.window.global::<Shell>().get_readonly());
    d.set_busy(false);
    let here = super::library::target(a).map(|t| {
        let label = a
            .window
            .global::<crate::GridState>()
            .get_title()
            .to_string();
        (t, label)
    });
    *a.pages.dialogs.mega.borrow_mut() = Some(Mega {
        url: url.into(),
        intent: intent.into(),
        library_id,
        data: None,
        children: HashMap::new(),
        files: HashMap::new(),
        root: String::new(),
        sel: HashSet::new(),
        open: HashSet::new(),
        target: here,
        req: 0,
    });
    d.set_open("mega".into());
    if !url.is_empty() {
        mega_inspect(a);
    }
}

fn mega_inspect(a: &Rc<AppInner>) {
    let d = dlg(a);
    let text = d.get_mega_url().to_string();
    let Some(url) = find_mega_url(&text) else {
        d.set_mega_stage("error".into());
        d.set_mega_retry(false);
        d.set_mega_error(
            a.t(if looks_like_mega(&text) {
                "remote.err.badKey"
            } else {
                "remote.err.notMega"
            })
            .into(),
        );
        return;
    };
    d.set_mega_stage("loading".into());
    let req = {
        let mut m = a.pages.dialogs.mega.borrow_mut();
        let Some(m) = m.as_mut() else { return };
        m.req += 1;
        m.url = url.clone();
        m.req
    };
    api::call::<RemoteInspect>(
        a,
        "POST",
        "/api/remote/inspect",
        Some(serde_json::json!({ "url": url })),
        move |a, r| {
            if a.pages.dialogs.mega.borrow().as_ref().map(|m| m.req) != Some(req) {
                return;
            }
            let d = dlg(a);
            match r {
                Ok(data) => {
                    mega_tree(a, data);
                    d.set_mega_stage("tree".into());
                }
                Err(e) => {
                    d.set_mega_stage("error".into());
                    d.set_mega_retry(true);
                    d.set_mega_error(super::library::mega_error(a, e.status, &e.message).into());
                }
            }
        },
    );
}

fn is_media(n: &pa_core::RemoteNode) -> bool {
    matches!(
        n.kind,
        Some(pa_core::MediaKind::Image) | Some(pa_core::MediaKind::Video)
    )
}

fn parent_dir(p: &str) -> &str {
    p.rsplit_once('/').map(|(a, _)| a).unwrap_or("")
}

fn mega_tree(a: &Rc<AppInner>, data: RemoteInspect) {
    let d = dlg(a);
    let lang = a.lang();
    let mut by_path: HashMap<String, String> = HashMap::new();
    let mut root = String::new();
    for n in &data.nodes {
        if n.path.is_empty() {
            root = n.id.clone();
        } else {
            by_path.insert(n.path.clone(), n.id.clone());
        }
    }
    if root.is_empty() {
        root = "__root".into();
    }
    let mut children: HashMap<String, Vec<String>> = HashMap::new();
    let mut files: HashMap<String, Vec<String>> = HashMap::new();
    for n in &data.nodes {
        if !n.dir {
            files.insert(n.id.clone(), vec![n.id.clone()]);
        } else {
            files.entry(n.id.clone()).or_default();
        }
        if n.path.is_empty() {
            continue;
        }
        let parent = by_path
            .get(parent_dir(&n.path))
            .cloned()
            .unwrap_or_else(|| root.clone());
        children.entry(parent).or_default().push(n.id.clone());
    }
    files.entry(root.clone()).or_default();
    for n in data.nodes.iter().rev() {
        if n.path.is_empty() {
            continue;
        }
        let parent = by_path
            .get(parent_dir(&n.path))
            .cloned()
            .unwrap_or_else(|| root.clone());
        let mine = files.get(&n.id).cloned().unwrap_or_default();
        if n.dir {
            // a folder's own list is complete once its children were walked (reverse BFS order)
        }
        files.entry(parent).or_default().extend(mine);
    }
    let media: Vec<&pa_core::RemoteNode> = data
        .nodes
        .iter()
        .filter(|n| !n.dir && (is_media(n) || n.kind == Some(pa_core::MediaKind::Audio)))
        .collect();
    let sel: HashSet<String> = if media.is_empty() {
        data.nodes
            .iter()
            .filter(|n| !n.dir)
            .map(|n| n.id.clone())
            .collect()
    } else {
        media.iter().map(|n| n.id.clone()).collect()
    };
    let dirs: Vec<String> = data
        .nodes
        .iter()
        .filter(|n| n.dir)
        .map(|n| n.id.clone())
        .collect();
    let open: HashSet<String> = if data.nodes.len() <= 60 {
        dirs.into_iter().collect()
    } else {
        [root.clone()].into_iter().collect()
    };
    let nfiles = data.nodes.iter().filter(|n| !n.dir).count();
    let nmedia = data.nodes.iter().filter(|n| !n.dir && is_media(n)).count();
    d.set_mega_name(data.name.clone().into());
    d.set_mega_summary(
        i18n::tv(
            lang,
            "remote.summary",
            &[
                ("n", &nfiles.to_string()),
                ("size", &fmt::format_bytes(data.size)),
            ],
        )
        .into(),
    );
    d.set_mega_media(
        i18n::tv(
            lang,
            "remote.summary",
            &[
                ("n", &nmedia.to_string()),
                ("size", &fmt::format_bytes(data.size)),
            ],
        )
        .into(),
    );
    d.set_mega_truncated(data.truncated);
    d.set_mega_folder(data.kind == pa_core::LinkKind::Folder);
    let can_stream = !a.window.global::<Shell>().get_readonly();
    d.set_mega_can_stream(can_stream);
    let intent = a
        .pages
        .dialogs
        .mega
        .borrow()
        .as_ref()
        .map(|m| m.intent.clone())
        .unwrap_or_default();
    d.set_mega_how(
        if can_stream && intent != "copy" {
            "stream"
        } else {
            "copy"
        }
        .into(),
    );
    let libs = a
        .pages
        .libs
        .borrow()
        .as_ref()
        .map(|l| l.libraries.clone())
        .unwrap_or_default();
    d.set_mega_has_libs(!libs.is_empty());
    d.set_mega_libs(model(
        libs.iter()
            .map(|l| SelectOption {
                value: l.id.clone().into(),
                label: l.name.clone().into(),
            })
            .collect(),
    ));
    let lib_id = a
        .pages
        .dialogs
        .mega
        .borrow()
        .as_ref()
        .and_then(|m| m.library_id.clone());
    d.set_mega_where(if lib_id.is_some() { "existing" } else { "new" }.into());
    d.set_mega_lib(
        lib_id
            .or_else(|| libs.first().map(|l| l.id.clone()))
            .unwrap_or_default()
            .into(),
    );
    d.set_mega_lib_name(data.name.clone().into());
    let has_pc = libs.iter().any(|l| !l.folders.is_empty());
    let target = a
        .pages
        .dialogs
        .mega
        .borrow()
        .as_ref()
        .and_then(|m| m.target.clone());
    d.set_mega_mode(
        if target.is_some() || has_pc {
            "extract"
        } else {
            "temp"
        }
        .into(),
    );
    d.set_mega_target(target.map(|t| t.1).unwrap_or_default().into());
    d.set_mega_zip("extract".into());
    if let Some(m) = a.pages.dialogs.mega.borrow_mut().as_mut() {
        m.children = children;
        m.files = files;
        m.root = root;
        m.sel = sel;
        m.open = open;
        m.data = Some(data);
    }
    publish_mega(a);
}

fn check_state(files: &[String], sel: &HashSet<String>) -> &'static str {
    let n = files.iter().filter(|f| sel.contains(*f)).count();
    if n == 0 {
        "none"
    } else if n == files.len() {
        "all"
    } else {
        "some"
    }
}

fn publish_mega(a: &Rc<AppInner>) {
    let d = dlg(a);
    let lang = a.lang();
    let m = a.pages.dialogs.mega.borrow();
    let Some(m) = m.as_ref() else { return };
    let Some(data) = &m.data else { return };
    let by_id: HashMap<&str, &pa_core::RemoteNode> =
        data.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut rows: Vec<TreeRowData> = vec![];
    const MAX_ROWS: usize = 400;
    fn walk(
        id: &str,
        depth: i32,
        m: &Mega,
        by_id: &HashMap<&str, &pa_core::RemoteNode>,
        rows: &mut Vec<TreeRowData>,
    ) {
        for c in m.children.get(id).map(|v| v.as_slice()).unwrap_or(&[]) {
            if rows.len() >= MAX_ROWS {
                return;
            }
            let Some(n) = by_id.get(c.as_str()) else {
                continue;
            };
            let files = m.files.get(c).cloned().unwrap_or_default();
            rows.push(row_of(n, depth, &files, m));
            if n.dir && m.open.contains(c) {
                walk(c, depth + 1, m, by_id, rows);
            }
        }
    }
    fn row_of(n: &pa_core::RemoteNode, depth: i32, files: &[String], m: &Mega) -> TreeRowData {
        let kind = match n.kind {
            Some(pa_core::MediaKind::Image) => "image",
            Some(pa_core::MediaKind::Video) => "video",
            Some(pa_core::MediaKind::Audio) => "audio",
            Some(pa_core::MediaKind::Archive) => "archive",
            _ => "",
        };
        TreeRowData {
            id: n.id.clone().into(),
            name: n.name.clone().into(),
            size: if n.dir {
                format!("{} · {}", files.len(), fmt::format_bytes(n.size))
            } else {
                fmt::format_bytes(n.size)
            }
            .into(),
            depth,
            dir: n.dir,
            state: check_state(files, &m.sel).into(),
            expanded: m.open.contains(&n.id),
            kind: kind.into(),
            dim: (!n.dir && !is_media(n) && kind != "archive") || (n.dir && files.is_empty()),
        }
    }
    if data.kind == pa_core::LinkKind::File {
        if let Some(n) = data.nodes.iter().find(|n| !n.dir) {
            rows.push(row_of(n, 0, std::slice::from_ref(&n.id), m));
        }
    } else {
        walk(&m.root, 0, m, &by_id, &mut rows);
    }
    d.set_mega_more_rows(rows.len() >= MAX_ROWS);
    d.set_mega_tree(model(rows));
    let all: Vec<String> = data
        .nodes
        .iter()
        .filter(|n| !n.dir)
        .map(|n| n.id.clone())
        .collect();
    d.set_mega_root_state(check_state(&all, &m.sel).into());
    let (mut nfiles, mut bytes, mut zips) = (0usize, 0i64, 0usize);
    for n in data
        .nodes
        .iter()
        .filter(|n| !n.dir && m.sel.contains(&n.id))
    {
        nfiles += 1;
        bytes += n.size;
        if n.name.to_lowercase().ends_with(".zip") {
            zips += 1;
        }
    }
    d.set_mega_selected(
        i18n::tv(
            lang,
            "remote.selected",
            &[
                ("n", &nfiles.to_string()),
                ("size", &fmt::format_bytes(bytes)),
            ],
        )
        .into(),
    );
    d.set_mega_can_import(nfiles > 0);
    let ask = zips > 0 && a.pages.settings.zip_action.borrow().as_str() == "ask";
    d.set_mega_zip_ask(ask);
    d.set_mega_zip_text(i18n::tv(lang, "remote.zipAsk", &[("n", &zips.to_string())]).into());
}

fn mega_toggle(a: &Rc<AppInner>, id: &str) {
    if let Some(m) = a.pages.dialogs.mega.borrow_mut().as_mut() {
        let files = if id == "__all" {
            m.data
                .as_ref()
                .map(|d| {
                    d.nodes
                        .iter()
                        .filter(|n| !n.dir)
                        .map(|n| n.id.clone())
                        .collect()
                })
                .unwrap_or_default()
        } else {
            m.files.get(id).cloned().unwrap_or_default()
        };
        let on = check_state(&files, &m.sel) != "all";
        for f in files {
            if on {
                m.sel.insert(f);
            } else {
                m.sel.remove(&f);
            }
        }
    }
    publish_mega(a);
}

fn mega_import_nodes(m: &Mega) -> Option<Vec<String>> {
    let data = m.data.as_ref()?;
    if data
        .nodes
        .iter()
        .filter(|n| !n.dir)
        .all(|n| m.sel.contains(&n.id))
    {
        return None;
    }
    let dir_of: HashMap<&str, bool> = data.nodes.iter().map(|n| (n.id.as_str(), n.dir)).collect();
    let mut out = vec![];
    fn walk(id: &str, m: &Mega, dir_of: &HashMap<&str, bool>, out: &mut Vec<String>) {
        for c in m.children.get(id).map(|v| v.as_slice()).unwrap_or(&[]) {
            let files = m.files.get(c).cloned().unwrap_or_default();
            if files.is_empty() {
                continue;
            }
            let dir = *dir_of.get(c.as_str()).unwrap_or(&false);
            match check_state(&files, &m.sel) {
                "none" => {}
                "all" if dir => out.push(c.clone()),
                _ if !dir => out.push(c.clone()),
                _ => walk(c, m, dir_of, out),
            }
        }
    }
    walk(&m.root, m, &dir_of, &mut out);
    Some(out)
}

fn mega_add(a: &Rc<AppInner>) {
    let d = dlg(a);
    let Some(url) = a
        .pages
        .dialogs
        .mega
        .borrow()
        .as_ref()
        .map(|m| m.url.clone())
    else {
        return;
    };
    let existing =
        d.get_mega_where() == "existing" && d.get_mega_has_libs() && !d.get_mega_lib().is_empty();
    let body = if existing {
        serde_json::json!({ "url": url, "libraryId": d.get_mega_lib().as_str() })
    } else {
        let n = d.get_mega_lib_name().trim().to_string();
        serde_json::json!({ "url": url, "libraryName": if n.is_empty() { d.get_mega_name().to_string() } else { n } })
    };
    d.set_busy(true);
    api::call::<pa_core::RemoteFolderCreated>(
        a,
        "POST",
        "/api/remote/folders",
        Some(body),
        |a, r| {
            let d = dlg(a);
            d.set_busy(false);
            match r {
                Ok(c) => {
                    d.set_open("".into());
                    a.toast(
                        ToastKind::Success,
                        &a.t("mega.added"),
                        &i18n::tv(a.lang(), "mega.addedBody", &[("name", &c.folder.name)]),
                    );
                    super::hub::load_libraries(a);
                    a.go(Route::Lib(c.library.id), true);
                }
                Err(e) => {
                    let m = super::library::mega_error(a, e.status, &e.message);
                    a.toast(ToastKind::Error, &a.t("common.error"), &m);
                }
            }
        },
    );
}

fn mega_import(a: &Rc<AppInner>) {
    let d = dlg(a);
    let mode = d.get_mega_mode().to_string();
    let target = a
        .pages
        .dialogs
        .mega
        .borrow()
        .as_ref()
        .and_then(|m| m.target.clone());
    if mode == "extract" && target.is_none() {
        mega_pick_target(a);
        return;
    }
    let Some((url, nodes, name)) = a.pages.dialogs.mega.borrow().as_ref().map(|m| {
        (
            m.url.clone(),
            mega_import_nodes(m),
            m.data.as_ref().map(|d| d.name.clone()).unwrap_or_default(),
        )
    }) else {
        return;
    };
    let mut body = serde_json::json!({ "url": url, "mode": mode });
    if let Some(n) = nodes {
        body["nodes"] = n.into();
    }
    if let Some((t, _)) = target.filter(|_| mode == "extract") {
        body["folderId"] = t.folder_id.into();
        body["dir"] = t.dir.into();
    }
    if d.get_mega_zip_ask() {
        body["zip"] = d.get_mega_zip().as_str().into();
    }
    d.set_busy(true);
    api::call::<pa_core::Job>(a, "POST", "/api/remote/import", Some(body), move |a, r| {
        let d = dlg(a);
        d.set_busy(false);
        match r {
            Ok(j) => {
                super::jobs::on_job(a, &j);
                a.toast(ToastKind::Info, &a.t("remote.started"), &name);
                d.set_open("".into());
            }
            Err(e) => {
                let m = super::library::mega_error(a, e.status, &e.message);
                a.toast(ToastKind::Error, &a.t("common.error"), &m);
            }
        }
    });
}

fn mega_pick_target(a: &Rc<AppInner>) {
    let title = a.t("remote.pickFolder");
    pick_folder(a, &title, "", |a, t| {
        let label = target_label(a, &t);
        if let Some(m) = a.pages.dialogs.mega.borrow_mut().as_mut() {
            m.target = Some((t, label.clone()));
        }
        dlg(a).set_mega_target(label.into());
    });
}

/// "Library · folder / sub" of a destination
pub(crate) fn target_label(a: &AppInner, t: &DirTarget) -> String {
    let libs = a.pages.libs.borrow();
    let base = libs
        .as_ref()
        .and_then(|l| {
            l.libraries.iter().find_map(|x| {
                x.folders.iter().find(|f| f.id == t.folder_id).map(|f| {
                    if x.folders.len() > 1 {
                        format!("{} · {}", x.name, f.name)
                    } else {
                        x.name.clone()
                    }
                })
            })
        })
        .unwrap_or_default();
    if t.dir.is_empty() {
        base
    } else {
        format!("{base} / {}", t.dir)
    }
}

// ───────── pairing ─────────

/// a phone asks to pair (server event)
pub(crate) fn on_pair(a: &Rc<AppInner>, d: &DeviceInfo) {
    {
        let mut p = a.pages.dialogs.pairs.borrow_mut();
        if p.iter().any(|x| x.id == d.id) {
            return;
        }
        p.push(d.clone());
    }
    show_pair(a);
    a.host.notify(
        &i18n::tv(a.lang(), "devices.pair.title", &[("name", &d.name)]),
        &a.t("devices.pair.hint"),
    );
}

/// device list changed: drop pairing requests that were answered elsewhere
pub(crate) fn on_devices(a: &Rc<AppInner>) {
    if a.pages.dialogs.pairs.borrow().is_empty() {
        return;
    }
    api::call::<Vec<DeviceInfo>>(a, "GET", "/api/devices", None, |a, r| {
        let Ok(list) = r else { return };
        a.pages.dialogs.pairs.borrow_mut().retain(|p| {
            list.iter()
                .any(|d| d.id == p.id && !d.paired && d.code.is_some())
        });
        show_pair(a);
    });
}

fn show_pair(a: &Rc<AppInner>) {
    let d = dlg(a);
    let first = a.pages.dialogs.pairs.borrow().first().cloned();
    match first {
        Some(p) => {
            d.set_pair_name(p.name.clone().into());
            d.set_pair_model(
                if p.model != p.name {
                    p.model.clone()
                } else {
                    String::new()
                }
                .into(),
            );
            d.set_pair_digits(model(
                p.code
                    .clone()
                    .unwrap_or_default()
                    .chars()
                    .map(|c| SharedString::from(c.to_string()))
                    .collect(),
            ));
            d.set_busy(false);
            if d.get_open().is_empty() || d.get_open() == "pair" {
                d.set_open("pair".into());
            }
        }
        None => {
            if d.get_open() == "pair" {
                d.set_open("".into());
            }
        }
    }
}

fn pair_decide(a: &Rc<AppInner>, approve: bool) {
    let Some(p) = a.pages.dialogs.pairs.borrow().first().cloned() else {
        return;
    };
    dlg(a).set_busy(true);
    let path = format!(
        "/api/devices/{}/{}",
        enc(&p.id),
        if approve { "approve" } else { "reject" }
    );
    api::call::<serde_json::Value>(
        a,
        "POST",
        &path,
        Some(serde_json::json!({})),
        move |a, r| {
            a.pages.dialogs.pairs.borrow_mut().retain(|x| x.id != p.id);
            match r {
                Ok(_) if approve => a.toast(
                    ToastKind::Success,
                    &i18n::tv(a.lang(), "devices.pair.approved", &[("name", &p.name)]),
                    &a.t("devices.pair.approvedBody"),
                ),
                Ok(_) => {}
                Err(e) => a.error_toast(&e.message),
            }
            dlg(a).set_open("".into());
            show_pair(a);
            super::hub::load_libraries(a);
            super::settings::refresh(a);
        },
    );
}

// ───────── uploads ─────────

/// "Add" → upload files / a folder into the open folder
pub(crate) fn upload_pick(a: &Rc<AppInner>, folder: bool) {
    let Some(t) = super::library::target(a) else {
        return;
    };
    let fut = if folder {
        a.host.pick_folders(&a.t("gallery.uploadFolder"))
    } else {
        a.host.pick_files(&a.t("gallery.upload"), vec![])
    };
    let w = Rc::downgrade(a);
    a.rt.spawn_then(fut, move |paths| {
        if let Some(a) = w.upgrade() {
            if !paths.is_empty() {
                add_files(&a, paths, Some(t), folder);
            }
        }
    });
}

/// Files / folders from the OS → the target (upload/actions.ts `addFiles`): folders can be
/// linked as library folders (desktop), zips follow the setting or the zip prompt, local files are
/// copied by the server (import-local), others uploaded with progress.
pub(crate) fn add_files(
    a: &Rc<AppInner>,
    paths: Vec<PathBuf>,
    target: Option<DirTarget>,
    picked_folder: bool,
) {
    if a.window.global::<Shell>().get_readonly() {
        a.toast(ToastKind::Error, &a.t("drop.readonly"), "");
        return;
    }
    let dirs: Vec<PathBuf> = paths.iter().filter(|p| p.is_dir()).cloned().collect();
    let files: Vec<PathBuf> = paths.iter().filter(|p| p.is_file()).cloned().collect();
    if a.host.is_desktop() && !dirs.is_empty() && !picked_folder {
        let names: Vec<String> = dirs
            .iter()
            .filter_map(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();
        dlg(a)
            .set_link_text(i18n::tv(a.lang(), "link.body", &[("name", &names.join(", "))]).into());
        let t2 = target.clone();
        *a.pages.dialogs.on_link.borrow_mut() =
            Some(Box::new(move |a, choice| match choice.as_deref() {
                Some("link") => link_folders(a, dirs, files, t2),
                Some("copy") => match t2 {
                    Some(t) => {
                        import_local(
                            a,
                            dirs.iter()
                                .map(|d| d.to_string_lossy().into_owned())
                                .collect(),
                            &t,
                            None,
                            true,
                        );
                        if !files.is_empty() {
                            add_files(a, files, Some(t), true);
                        }
                    }
                    None => add_files(a, dirs.into_iter().chain(files).collect(), None, true),
                },
                _ => {}
            }));
        dlg(a).set_open("link".into());
        return;
    }
    let Some(t) = target else {
        let title = a.t("drop.pickTitle");
        pick_folder(a, &title, "", move |a, t| {
            add_files(a, paths, Some(t), true)
        });
        return;
    };
    let zips: Vec<String> = paths
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")))
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    let go = move |a: &Rc<AppInner>, mode: Option<String>| {
        let all: Vec<String> = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        if a.host.is_desktop() {
            import_local(a, all, &t, mode, false);
        } else {
            upload_queue(a, paths, t, mode);
        }
    };
    if zips.is_empty() {
        go(a, None);
        return;
    }
    zip_mode(a, zips, move |a, m| {
        if let Some(m) = m {
            go(a, Some(m));
        }
    });
}

fn link_folders(
    a: &Rc<AppInner>,
    dirs: Vec<PathBuf>,
    files: Vec<PathBuf>,
    target: Option<DirTarget>,
) {
    let owner = target.as_ref().and_then(|t| {
        a.pages
            .libs
            .borrow()
            .as_ref()?
            .libraries
            .iter()
            .find(|l| l.folders.iter().any(|f| f.id == t.folder_id))
            .map(|l| l.id.clone())
    });
    for d in &dirs {
        let p = d.to_string_lossy().into_owned();
        match &owner {
            Some(lib) => api::act(
                a,
                "POST",
                &format!("/api/libraries/{}/folders", enc(lib)),
                Some(serde_json::json!({ "path": p })),
                |a, _| super::hub::load_libraries(a),
            ),
            None => {
                let name = d
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                api::call::<Library>(
                    a,
                    "POST",
                    "/api/libraries",
                    Some(serde_json::json!({ "name": name, "folders": [p] })),
                    |a, r| match r {
                        Ok(l) => {
                            super::hub::load_libraries(a);
                            a.go(Route::Lib(l.id), true);
                        }
                        Err(e) => a.error_toast(&e.message),
                    },
                );
            }
        }
    }
    a.toast(ToastKind::Success, &a.t("lib.created"), "");
    if !files.is_empty() {
        add_files(a, files, target, true);
    }
}

/// zip handling: the server setting, else the prompt (None = canceled)
fn zip_mode(
    a: &Rc<AppInner>,
    names: Vec<String>,
    then: impl FnOnce(&Rc<AppInner>, Option<String>) + 'static,
) {
    api::call::<pa_core::Settings>(a, "GET", "/api/settings", None, move |a, r| {
        let setting = r.ok().map(|s| s.zip_action);
        match setting {
            Some(pa_core::ZipAction::Extract) => return then(a, Some("extract".into())),
            Some(pa_core::ZipAction::Temp) => return then(a, Some("temp".into())),
            _ => {}
        }
        let d = dlg(a);
        d.set_zip_text(
            if names.len() == 1 {
                i18n::tv(a.lang(), "zip.body", &[("name", &names[0])])
            } else {
                i18n::tv(a.lang(), "zip.bodyMany", &[("n", &names.len().to_string())])
            }
            .into(),
        );
        d.set_zip_remember(false);
        *a.pages.dialogs.on_zip.borrow_mut() = Some(Box::new(then));
        d.set_open("zip".into());
    });
}

fn after_zip(a: &Rc<AppInner>, v: &serde_json::Value, t: &DirTarget) {
    if let Some(f) = v["tempFolder"]["id"].as_str() {
        let name = v["tempFolder"]["name"].as_str().unwrap_or("").to_string();
        super::hub::load_libraries(a);
        toast_open(
            a,
            &a.t("upload.tempReady"),
            &name,
            Route::Folder(f.into(), String::new()),
        );
        a.go(Route::Folder(f.into(), String::new()), true);
    } else if let Some(d) = v["extractedDir"].as_str() {
        toast_open(
            a,
            &a.t("upload.extracted"),
            d,
            Route::Folder(t.folder_id.clone(), d.into()),
        );
    }
}

/// desktop: the server copies local files / folders (no upload)
fn import_local(
    a: &Rc<AppInner>,
    paths: Vec<String>,
    t: &DirTarget,
    zip: Option<String>,
    folders: bool,
) {
    let n = paths.len();
    let id = toast_progress(a, &a.t("upload.title"), 0.3);
    let mut body = serde_json::json!({ "paths": paths, "folderId": t.folder_id, "dir": t.dir });
    if let Some(z) = zip {
        body["zip"] = z.into();
    }
    let t2 = t.clone();
    api::call::<serde_json::Value>(
        a,
        "POST",
        "/api/import-local",
        Some(body),
        move |a, r| match r {
            Ok(v) => {
                toast_update(
                    a,
                    id,
                    ToastKind::Success,
                    &a.t("upload.done"),
                    &i18n::tv(a.lang(), "upload.doneBody", &[("n", &n.to_string())]),
                );
                if !folders {
                    after_zip(a, &v, &t2);
                }
                super::library::refresh_meta(a);
            }
            Err(e) => toast_update(a, id, ToastKind::Error, &a.t("common.error"), &e.message),
        },
    );
}

/// remote server: upload each file (3 at a time) with the upload panel
fn upload_queue(a: &Rc<AppInner>, paths: Vec<PathBuf>, t: DirTarget, zip: Option<String>) {
    let mut files: Vec<(PathBuf, String)> = vec![];
    for p in &paths {
        collect(
            p,
            p.parent().unwrap_or(std::path::Path::new("")),
            &mut files,
        );
    }
    super::jobs::upload_files(a, files, t, zip);
}

fn collect(p: &std::path::Path, base: &std::path::Path, out: &mut Vec<(PathBuf, String)>) {
    if p.is_dir() {
        if let Ok(rd) = std::fs::read_dir(p) {
            let mut v: Vec<_> = rd.flatten().map(|e| e.path()).collect();
            v.sort();
            for c in v {
                collect(&c, base, out);
            }
        }
    } else if p.is_file() {
        let rel = p
            .strip_prefix(base)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/");
        out.push((p.to_path_buf(), rel));
    }
}

// ───────── shortcuts ─────────

fn shortcuts(a: &Rc<AppInner>) -> Vec<KeyRowData> {
    let m = if a.host.platform() == "darwin" {
        "⌘"
    } else {
        "Ctrl"
    };
    let groups: KeyGroups = vec![
        (
            "pref.kb.general",
            vec![
                ("pref.kb.search", vec!["/".into(), format!("{m} K")]),
                ("pref.kb.help", vec!["?".into()]),
            ],
        ),
        (
            "pref.kb.grid",
            vec![
                ("pref.kb.move", vec!["← ↑ → ↓".into()]),
                ("pref.kb.open", vec!["Enter".into()]),
                ("pref.kb.select", vec!["Space".into()]),
                ("pref.kb.selectAll", vec![format!("{m} A")]),
                ("pref.kb.clearSel", vec!["Esc".into()]),
                ("pref.kb.delete", vec!["Delete".into()]),
                ("pref.kb.density", vec![format!("{m} + 🖱")]),
            ],
        ),
        (
            "pref.kb.viewer",
            vec![
                ("pref.kb.prevNext", vec!["←".into(), "→".into()]),
                ("pref.kb.close", vec!["Esc".into()]),
                ("pref.kb.info", vec!["I".into()]),
                ("pref.kb.fullscreen", vec!["F".into()]),
                ("pref.kb.zoom", vec!["+".into(), "−".into()]),
                ("pref.kb.zoomReset", vec!["0".into()]),
            ],
        ),
        (
            "pref.kb.video",
            vec![
                ("pref.kb.playPause", vec!["Space".into(), "K".into()]),
                ("pref.kb.seek", vec!["J".into(), "L".into()]),
                ("pref.kb.mute", vec!["M".into()]),
                ("pref.kb.fullscreen", vec!["F".into()]),
            ],
        ),
        (
            "pref.kb.slideshow",
            vec![
                ("pref.kb.playPause", vec!["Space".into(), "K".into()]),
                ("pref.kb.prevNext", vec!["←".into(), "→".into()]),
                ("pref.kb.fullscreen", vec!["F".into()]),
                ("pref.kb.captions", vec!["C".into()]),
                ("pref.kb.mute", vec!["M".into()]),
                ("pref.kb.exit", vec!["Esc".into()]),
            ],
        ),
        (
            "pref.kb.photoEditor",
            vec![
                ("pref.kb.save", vec![format!("{m} S")]),
                ("pref.kb.compare", vec!["\\".into()]),
                ("pref.kb.fit", vec!["0".into(), "1".into()]),
                ("pref.kb.deleteLayer", vec!["Delete".into()]),
                ("pref.kb.deselect", vec!["Esc".into()]),
            ],
        ),
        (
            "pref.kb.videoEditor",
            vec![
                ("pref.kb.playPause", vec!["Space".into(), "K".into()]),
                ("pref.kb.trim", vec!["I".into(), "O".into()]),
                ("pref.kb.frame", vec!["←".into(), "→".into()]),
                ("pref.kb.undo", vec![format!("{m} Z"), format!("{m} Y")]),
                ("pref.kb.deleteLayer", vec!["Delete".into()]),
            ],
        ),
    ];
    let mut out = vec![];
    let mut push = |g: &str, items: &[(&str, Vec<String>)]| {
        out.push(KeyRowData {
            group: a.t(g).into(),
            label: "".into(),
            keys: ModelRc::default(),
        });
        for (k, keys) in items {
            out.push(KeyRowData {
                group: "".into(),
                label: a.t(k).into(),
                keys: model(
                    keys.iter()
                        .map(|s| SharedString::from(s.as_str()))
                        .collect(),
                ),
            });
        }
    };
    for (g, items) in &groups {
        push(g, items);
    }
    if a.host.is_desktop() {
        push(
            "pref.kb.desktop",
            &[
                ("pref.kb.fullscreen", vec!["F11".into()]),
                (
                    "pref.kb.winZoom",
                    vec![format!("{m} +"), format!("{m} −"), format!("{m} 0")],
                ),
            ],
        );
    }
    out
}

/// (settings card) the list for the shortcuts card
pub(crate) fn shortcut_rows(a: &Rc<AppInner>) -> Vec<KeyRowData> {
    shortcuts(a)
}

/// the "?" overlay
pub(crate) fn toggle_shortcuts(a: &Rc<AppInner>) {
    let d = dlg(a);
    if d.get_open() == "shortcuts" {
        d.set_open("".into());
        return;
    }
    if !d.get_open().is_empty() || !d.get_top().is_empty() {
        return;
    }
    d.set_shortcuts(model(shortcuts(a)));
    d.set_open("shortcuts".into());
}

/// libraries reloaded: an open editor / picker shows the new state
pub(crate) fn on_libraries(a: &Rc<AppInner>) {
    if dlg(a).get_open() == "library" {
        publish_lib(a);
    }
    if dlg(a).get_top() == "picker"
        && a.pages
            .dialogs
            .picker
            .borrow()
            .as_ref()
            .is_some_and(|p| p.stack.is_empty() && !matches!(p.mode, PickMode::Host))
    {
        publish_picker(a);
    }
}

/// albums reloaded
pub(crate) fn on_albums(_a: &Rc<AppInner>) {}

/// automation: `dialog <name> [arg]`
pub(crate) fn open_by_name(a: &Rc<AppInner>, name: &str, arg: &str) {
    match name {
        "library" => edit_library(a, (!arg.is_empty()).then(|| arg.to_string())),
        "exclusions" => exclusions(a, arg),
        "mega" => open_mega(a, arg, "stream"),
        "mega-copy" => open_mega(a, arg, "copy"),
        "new-album" => new_album(a),
        "new-folder" => new_folder(a),
        "picker" => pick_folder(a, &a.t("drop.pickTitle"), "", |_, _| {}),
        "phone" => pick_phone_folder(a, "IMG_0001.jpg", |_, _| {}),
        "shortcuts" => toggle_shortcuts(a),
        "zip" => zip_mode(a, vec!["photos.zip".into()], |_, _| {}),
        "link" => {
            dlg(a).set_link_text(i18n::tv(a.lang(), "link.body", &[("name", "Vacation")]).into());
            dlg(a).set_open("link".into());
        }
        "confirm" => confirm(
            a,
            &a.t("album.delete"),
            &a.t("album.deleteBody"),
            &a.t("album.delete"),
            true,
            |_| {},
        ),
        "pair" => on_pair(
            a,
            &DeviceInfo {
                id: "demo".into(),
                name: "Pixel 8".into(),
                model: "Google Pixel 8".into(),
                online: true,
                paired: false,
                last_seen: 0,
                folders: vec![],
                code: Some("4821".into()),
                caps: None,
            },
        ),
        "close" => {
            dlg(a).set_open("".into());
            dlg(a).set_top("".into());
        }
        _ => eprintln!("[script] unknown dialog {name}"),
    }
}

pub(crate) fn install(app: &Rc<AppInner>) {
    let d = dlg(app);
    macro_rules! cb {
        ($on:ident, |$a:ident $(, $arg:ident)*| $body:block) => {{
            let w = Rc::downgrade(app);
            d.$on(move |$($arg),*| {
                if let Some($a) = w.upgrade() $body
            });
        }};
    }
    cb!(on_close, |a| {
        let open = dlg(&a).get_open().to_string();
        dlg(&a).set_open("".into());
        match open.as_str() {
            "pair" => {
                // dismissed: the request stays pending on the server; next one (if any)
                let mut p = a.pages.dialogs.pairs.borrow_mut();
                if !p.is_empty() {
                    p.remove(0);
                }
            }
            "mega" => {
                a.pages.dialogs.mega.borrow_mut().take();
            }
            _ => {}
        }
        if open == "pair" {
            show_pair(&a);
        }
    });
    cb!(on_close_top, |a| { close_top(&a) });
    cb!(on_confirm_top, |a| {
        let top = dlg(&a).get_top().to_string();
        match top.as_str() {
            "picker" => picker_confirm(&a),
            "prompt" => {
                let text = dlg(&a).get_prompt_text().to_string();
                let f = a.pages.dialogs.on_prompt.borrow_mut().take();
                dlg(&a).set_top("".into());
                if let Some(f) = f {
                    f(&a, text);
                }
            }
            _ => {
                let f = a.pages.dialogs.on_top.borrow_mut().take();
                dlg(&a).set_top("".into());
                if let Some(f) = f {
                    f(&a);
                }
            }
        }
    });
    cb!(on_confirm, |a| {
        match dlg(&a).get_open().as_str() {
            "library" => lib_save(&a),
            "exclusions" => ex_save(&a),
            _ => {}
        }
    });
    cb!(on_lib_pick, |a| { lib_pick(&a) });
    cb!(on_lib_remove_path, |a, p| {
        if let Some(st) = a.pages.dialogs.lib.borrow_mut().as_mut() {
            st.paths.retain(|x| x.as_str() != p.as_str());
        }
        publish_lib(&a);
    });
    cb!(on_lib_remove_folder, |a, id| {
        api::act(
            &a,
            "DELETE",
            &format!("/api/folders/{}", enc(&id)),
            None,
            |a, _| {
                a.toast(
                    ToastKind::Info,
                    &a.t("lib.removeFolder"),
                    &a.t("lib.removeFolderHint"),
                );
                super::hub::load_libraries(a);
            },
        );
    });
    cb!(on_lib_rescan_folder, |a, id| {
        let remote = a.pages.libs.borrow().as_ref().is_some_and(|l| {
            l.libraries
                .iter()
                .flat_map(|x| x.folders.iter())
                .any(|f| f.id == id.as_str() && matches!(f.remote, Some(Some(_))))
        });
        let path = if remote {
            format!("/api/remote/folders/{}/refresh", enc(&id))
        } else {
            format!("/api/folders/{}/rescan", enc(&id))
        };
        api::act(&a, "POST", &path, Some(serde_json::json!({})), |a, _| {
            a.toast(ToastKind::Info, &a.t("lib2.rescanStarted"), "")
        });
    });
    cb!(on_lib_delete, |a| {
        let Some(id) = a
            .pages
            .dialogs
            .lib
            .borrow()
            .as_ref()
            .and_then(|s| s.id.clone())
        else {
            return;
        };
        let name = lib_by_id(&a, &id).map(|l| l.name).unwrap_or_default();
        let (t, b, l) = (
            a.t("lib.delete"),
            i18n::tv(a.lang(), "lib.deleteConfirm", &[("name", &name)]),
            a.t("common.remove"),
        );
        confirm(&a, &t, &b, &l, true, move |a| {
            api::act(
                a,
                "DELETE",
                &format!("/api/libraries/{}", enc(&id)),
                None,
                |a, _| {
                    dlg(a).set_open("".into());
                    super::hub::load_libraries(a);
                    a.go(Route::Library, false);
                },
            );
        });
    });
    cb!(on_lib_add_mega, |a| {
        let lib = a
            .pages
            .dialogs
            .lib
            .borrow()
            .as_ref()
            .and_then(|s| s.id.clone());
        dlg(&a).set_open("".into());
        open_mega_for(&a, "", "stream", lib);
    });
    cb!(on_ex_add, |a, s| {
        let d = dlg(&a);
        let cur = d.get_ex_text().trim().to_string();
        d.set_ex_text(
            if cur.is_empty() {
                s.to_string()
            } else {
                format!("{cur}\n{s}")
            }
            .into(),
        );
        publish_ex_suggest(&a);
    });
    cb!(on_pick_open, |a, k| { picker_open(&a, &k) });
    cb!(on_pick_back, |a| {
        let host = a
            .pages
            .dialogs
            .picker
            .borrow()
            .as_ref()
            .map(|p| (matches!(p.mode, PickMode::Host), p.host_parent.clone()));
        match host {
            Some((true, parent)) => load_host_dirs(&a, parent),
            Some((false, _)) => {
                if let Some(p) = a.pages.dialogs.picker.borrow_mut().as_mut() {
                    p.stack.pop();
                }
                publish_picker(&a);
            }
            None => {}
        }
    });
    cb!(on_pick_create, |a| {
        let d = dlg(&a);
        let name = d.get_pick_new_name().trim().to_string();
        if name.is_empty() {
            return;
        }
        let (host, cur, hp) = {
            let p = a.pages.dialogs.picker.borrow();
            let Some(p) = p.as_ref() else { return };
            (
                matches!(p.mode, PickMode::Host),
                p.stack.last().cloned(),
                p.host_path.clone(),
            )
        };
        d.set_pick_naming(false);
        if host {
            let Some(hp) = hp else { return };
            api::call::<serde_json::Value>(
                &a,
                "POST",
                "/api/host/mkdir",
                Some(serde_json::json!({ "path": hp, "name": name })),
                |a, r| match r {
                    Ok(v) => load_host_dirs(a, v["path"].as_str().map(str::to_string)),
                    Err(e) => a.error_toast(&e.message),
                },
            );
        } else if let Some(cur) = cur {
            let n2 = name.clone();
            api::call::<DirTarget>(
                &a,
                "POST",
                "/api/fs/mkdir",
                Some(
                    serde_json::json!({ "folderId": cur.folder_id, "dir": cur.dir, "name": name }),
                ),
                move |a, r| match r {
                    Ok(t) => {
                        if let Some(p) = a.pages.dialogs.picker.borrow_mut().as_mut() {
                            p.stack.push(Step {
                                folder_id: t.folder_id,
                                dir: t.dir,
                                label: n2,
                            });
                        }
                        publish_picker(a);
                    }
                    Err(e) => a.error_toast(&e.message),
                },
            );
        }
    });
    cb!(on_pick_manual_add, |a| {
        let p = dlg(&a).get_pick_manual().trim().to_string();
        if p.is_empty() {
            return;
        }
        let f = a
            .pages
            .dialogs
            .picker
            .borrow_mut()
            .take()
            .and_then(|mut pk| pk.on_host.take());
        dlg(&a).set_top("".into());
        if let Some(f) = f {
            f(&a, p);
        }
    });
    cb!(on_mega_inspect, |a| { mega_inspect(&a) });
    cb!(on_mega_other, |a| {
        dlg(&a).set_mega_stage("input".into());
    });
    cb!(on_mega_toggle, |a, id| { mega_toggle(&a, &id) });
    cb!(on_mega_toggle_all, |a| {
        let root = a
            .pages
            .dialogs
            .mega
            .borrow()
            .as_ref()
            .map(|m| m.root.clone())
            .unwrap_or_default();
        let _ = root;
        mega_toggle(&a, "__all");
    });
    cb!(on_mega_expand, |a, id| {
        if let Some(m) = a.pages.dialogs.mega.borrow_mut().as_mut() {
            if !m.open.remove(id.as_str()) {
                m.open.insert(id.to_string());
            }
        }
        publish_mega(&a);
    });
    cb!(on_mega_pick_target, |a| { mega_pick_target(&a) });
    cb!(on_mega_add, |a| { mega_add(&a) });
    cb!(on_mega_import, |a| { mega_import(&a) });
    cb!(on_pair_approve, |a| { pair_decide(&a, true) });
    cb!(on_pair_reject, |a| { pair_decide(&a, false) });
    cb!(on_zip_choose, |a, mode| {
        let d = dlg(&a);
        let remember = d.get_zip_remember();
        d.set_open("".into());
        let f = a.pages.dialogs.on_zip.borrow_mut().take();
        let mode = (!mode.is_empty()).then(|| mode.to_string());
        if remember {
            if let Some(m) = mode.as_deref().filter(|m| *m != "keep") {
                api::act(
                    &a,
                    "PATCH",
                    "/api/settings",
                    Some(serde_json::json!({ "zipAction": m })),
                    |a, _| super::settings::refresh(a),
                );
            }
        }
        if let Some(f) = f {
            f(&a, mode);
        }
    });
    cb!(on_link_choose, |a, c| {
        dlg(&a).set_open("".into());
        let f = a.pages.dialogs.on_link.borrow_mut().take();
        if let Some(f) = f {
            f(&a, (!c.is_empty()).then(|| c.to_string()));
        }
    });

    // toast actions
    let w = Rc::downgrade(app);
    app.window.global::<Shell>().on_toast_action(move |id| {
        let Some(a) = w.upgrade() else { return };
        let f = a.pages.dialogs.toast_actions.borrow_mut().remove(&id);
        if let Some(f) = f {
            f(&a);
        }
        a.dismiss_toast(id);
    });

    // top bar: crumbs + "Add"
    let ch = app.window.global::<Chrome>();
    let w = Rc::downgrade(app);
    ch.on_open_crumb(move |i| {
        if let Some(a) = w.upgrade() {
            a.window.global::<crate::LibState>().invoke_open_crumb(i);
        }
    });
    let w = Rc::downgrade(app);
    ch.on_add(move |what| {
        let Some(a) = w.upgrade() else { return };
        match what.as_str() {
            "upload" => upload_pick(&a, false),
            "upload-folder" => upload_pick(&a, true),
            "new-folder" => new_folder(&a),
            "mega" => open_mega(&a, "", ""),
            _ => {}
        }
    });

    // shortcuts / window keys
    let w = Rc::downgrade(app);
    app.window.global::<Shell>().on_shortcut(move |k| {
        let Some(a) = w.upgrade() else { return };
        match k.as_str() {
            "help" => toggle_shortcuts(&a),
            "fullscreen" => a.host.toggle_fullscreen(),
            "zoom-in" => {
                a.host.zoom(1);
            }
            "zoom-out" => {
                a.host.zoom(-1);
            }
            "zoom-reset" => {
                a.host.zoom(0);
            }
            _ => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mega_links() {
        assert_eq!(
            find_mega_url("look: https://mega.nz/folder/AbCdEf12#0123456789abcdefABCD yes")
                .as_deref(),
            Some("https://mega.nz/folder/AbCdEf12#0123456789abcdefABCD")
        );
        assert_eq!(
            find_mega_url("mega.nz/file/AbCdEf12#0123456789abcdef/file/XyZ12345").as_deref(),
            Some("https://mega.nz/file/AbCdEf12#0123456789abcdef/file/XyZ12345")
        );
        assert_eq!(
            find_mega_url("http://www.mega.co.nz/#F!abcdef!0123456789abcd").as_deref(),
            Some("https://www.mega.co.nz/#F!abcdef!0123456789abcd")
        );
        assert_eq!(find_mega_url("https://mega.nz/folder/AbCdEf12"), None);
        assert!(looks_like_mega("https://mega.nz/folder/AbCdEf12"));
        assert_eq!(find_mega_url("https://example.com"), None);
    }

    #[test]
    fn file_names() {
        assert_eq!(sanitize_file_name("a/b:c.zip"), "a_b_c.zip");
        assert_eq!(sanitize_file_name("  "), "PeekAround.zip");
    }
}
