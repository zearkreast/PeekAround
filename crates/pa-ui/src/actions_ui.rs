//! The interaction hub: grid pointer / keyboard handling (click, Ctrl/Shift click, lasso,
//! day selection, keyboard focus, Ctrl+wheel density), the selection bar, the right-click menu,
//! running registry actions (src/actions.rs) with their dialogs (delete confirm, rename,
//! move / copy folder picker, album picker), tile drag & drop onto `DropTarget`s, OS file drops
//! (winit) → upload, downloads, "show in file manager", copy path, save video frame.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

use pa_core::{Album, DirTarget, FileRef, LibrariesResponse, MediaItem, ServerInfo};
use slint::{ComponentHandle, Image, ModelRc, SharedString, VecModel};

use crate::actions::{self, Ctx, Run, Surface};
use crate::app::AppInner;
use crate::backend::BackendEvent;
use crate::i18n::{self, Lang};
use crate::selection::{self, ClickAction, KeyStep, Selection};
use crate::{
    ActionItem, ActionState, DragState, GridInput, GridState, PickerRow, ToastKind, VIcons,
};

/// A drop handler for `DropTarget` keys with a prefix (e.g. "library:"): `(key, items, copy)`.
pub type DropHandler = Rc<dyn Fn(&str, Vec<MediaItem>, bool)>;

#[derive(Debug, Clone)]
struct Press {
    x: f64,
    y: f64,
    idx: Option<usize>,
    group: Option<usize>,
    on_check: bool,
    ctrl: bool,
    shift: bool,
    moved: bool,
}

#[derive(Debug, Clone)]
struct Lasso {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
    base: Selection,
    /// last pointer position (window y) for edge auto-scroll
    wy: f64,
}

#[derive(Clone)]
#[allow(clippy::large_enum_variant)]
enum DialogKind {
    Delete(Vec<MediaItem>),
    Rename(MediaItem),
    Move { items: Vec<MediaItem>, copy: bool },
    Album(Vec<MediaItem>),
    Upload(Vec<PathBuf>),
}

#[derive(Debug, Clone)]
struct PickNode {
    key: String,
    label: String,
    sub: String,
    depth: i32,
    icon: &'static str,
    target: Option<DirTarget>,
    expanded: bool,
    loaded: bool,
    loading: bool,
    can_expand: bool,
    enabled: bool,
}

struct Hub {
    app: Weak<AppInner>,
    info: RefCell<Option<ServerInfo>>,
    libs: RefCell<Option<LibrariesResponse>>,
    albums: RefCell<Vec<Album>>,
    /// album page open (album actions)
    album: RefCell<Option<String>>,
    /// where OS drops / uploads go on the current page (folder / library page), with a label
    drop_target: RefCell<Option<(DirTarget, String)>>,
    folder_pages: Cell<bool>,
    press: RefCell<Option<Press>>,
    lasso: RefCell<Option<Lasso>>,
    drag: RefCell<Option<(Vec<MediaItem>, bool)>>,
    drop_handlers: RefCell<Vec<(String, DropHandler)>>,
    menu_items: RefCell<Vec<MediaItem>>,
    menu_index: Cell<Option<usize>>,
    dialog: RefCell<Option<DialogKind>>,
    picker: RefCell<Vec<PickNode>>,
    viewer_open: Cell<bool>,
    wheel_acc: Cell<f64>,
    ctrl_down: Cell<bool>,
    cursor: Cell<(f64, f64)>,
    hovered_files: RefCell<Vec<PathBuf>>,
    dropped: RefCell<Vec<PathBuf>>,
    drop_timer: slint::Timer,
    scroll_timer: slint::Timer,
}

thread_local! {
    static HUB: RefCell<Option<Rc<Hub>>> = const { RefCell::new(None) };
}

fn hub() -> Option<Rc<Hub>> {
    HUB.with(|h| h.borrow().clone())
}

// ───────── public API (other screens) ─────────

/// The action context of the current page / server.
pub fn ctx() -> Ctx {
    let Some(h) = hub() else {
        return Ctx::default();
    };
    h.ctx()
}

/// The album page that is open (album actions see it in `Ctx::album`).
pub fn set_current_album(id: Option<String>) {
    if let Some(h) = hub() {
        *h.album.borrow_mut() = id;
        h.publish_selection();
    }
}

/// Where OS file drops / uploads land on the current page (`label`: the folder name shown in
/// the drop overlay); None → the user picks a folder after dropping.
pub fn set_drop_target(t: Option<(DirTarget, String)>) {
    if let Some(h) = hub() {
        *h.drop_target.borrow_mut() = t;
    }
}

/// The router has in-app folder pages ("show in folder" appears).
pub fn set_folder_pages(on: bool) {
    if let Some(h) = hub() {
        h.folder_pages.set(on);
    }
}

/// Handle drops on `DropTarget`s whose key starts with `prefix`.
pub fn register_drop_handler(prefix: &str, f: DropHandler) {
    if let Some(h) = hub() {
        h.drop_handlers.borrow_mut().push((prefix.to_string(), f));
    }
}

/// A library folder's name ("Library / folder" when the library has several), from the last
/// libraries list.
pub(crate) fn folder_name(folder_id: &str) -> Option<String> {
    let h = hub()?;
    let libs = h.libs.borrow();
    libs.as_ref()?.libraries.iter().find_map(|l| {
        let f = l.folders.iter().find(|f| f.id == folder_id)?;
        Some(if l.folders.len() > 1 {
            format!("{} / {}", l.name, f.name)
        } else {
            l.name.clone()
        })
    })
}

/// Open the viewer at grid index `i` (album / folder pages that show the shared grid).
pub fn open_viewer(i: usize) {
    if let Some(v) = crate::viewer::get() {
        v.open_index(i);
    }
}

/// Open the viewer on a single item (no paging).
pub fn open_viewer_item(item: MediaItem) {
    if let Some(v) = crate::viewer::get() {
        v.open_item(item);
    }
}

/// Open the viewer at `items[i]`, paging over `items` (pages that don't use the shared grid).
pub fn open_viewer_list(items: Vec<MediaItem>, i: usize) {
    if let Some(v) = crate::viewer::get() {
        v.open_list(items, i);
    }
}

/// A server / connection event (called by the app's event loop).
pub fn on_event(e: &BackendEvent) {
    let Some(h) = hub() else { return };
    if let BackendEvent::Server(ev) = e {
        use pa_core::ServerEvent as E;
        match ev {
            E::Hello { info } => {
                *h.info.borrow_mut() = Some(info.clone());
                h.load_libraries();
            }
            E::Media { .. } | E::Scan { .. } => {}
            _ => h.load_libraries(),
        }
    }
}

/// VIcons image by name.
pub(crate) fn icon(a: &AppInner, name: &str) -> Image {
    let i = a.window.global::<VIcons>();
    match name {
        "download" => i.get_download(),
        "heart" => i.get_heart(),
        "heart-off" => i.get_heart_off(),
        "presentation" => i.get_presentation(),
        "book-image" => i.get_book_image(),
        "folder-input" => i.get_folder_input(),
        "copy" => i.get_copy(),
        "pencil" => i.get_pencil(),
        "folder-open" => i.get_folder_open(),
        "external-link" => i.get_external_link(),
        "clipboard-copy" => i.get_clipboard_copy(),
        "trash-2" => i.get_trash_2(),
        "share-2" => i.get_share_2(),
        "square-pen" => i.get_square_pen(),
        "layout-grid" => i.get_layout_grid(),
        "maximize-2" => i.get_maximize_2(),
        "upload" => i.get_upload(),
        "image" => i.get_image(),
        "rotate-cw" => i.get_rotate_cw(),
        "library" => i.get_library(),
        "plus" => i.get_plus(),
        "x" => i.get_x(),
        _ => i.get_ellipsis_vertical(),
    }
}

/// Run a registry action on items.
pub(crate) fn run(a: &Rc<AppInner>, id: &str, items: Vec<MediaItem>, surface: Surface) {
    let Some(h) = hub() else { return };
    h.run(a, id, items, surface);
}

/// A dialog / menu of the actions layer is open (the viewer leaves the keys to it).
pub fn dialog_open() -> bool {
    hub().is_some_and(|h| h.dialog.borrow().is_some() || h.menu_open())
}

/// Viewer opened / closed (the selection bar hides under it).
pub(crate) fn viewer_opened(open: bool) {
    if let Some(h) = hub() {
        h.viewer_open.set(open);
        if let Some(a) = h.app.upgrade() {
            a.window.global::<ActionState>().set_sel_hidden(open);
        }
    }
}

/// Give the grid the keyboard focus (after the viewer closed).
pub(crate) fn focus_grid() {
    if let Some(a) = hub().and_then(|h| h.app.upgrade()) {
        let gi = a.window.global::<GridInput>();
        gi.set_focus_request(gi.get_focus_request() + 1);
    }
}

/// Open an URL in the default browser.
pub fn open_url(url: &str) {
    let r = if cfg!(windows) {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    if let Err(e) = r {
        log::warn!("open {url}: {e}");
    }
}

/// Show a file in the OS file manager (selected where the platform can).
pub fn reveal_in_file_manager(path: &Path) -> std::io::Result<()> {
    if cfg!(windows) {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()?;
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open")
            .arg("-R")
            .arg(path)
            .spawn()?;
    } else {
        // org.freedesktop.FileManager1 would select it; xdg-open of the folder works everywhere
        let dir = path.parent().unwrap_or(path);
        let dbus = std::process::Command::new("dbus-send")
            .args([
                "--session",
                "--print-reply",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
            ])
            .args([
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
            ])
            .arg(format!("array:string:file://{}", path.display()))
            .arg("string:")
            .output();
        if !dbus.map(|o| o.status.success()).unwrap_or(false) {
            std::process::Command::new("xdg-open").arg(dir).spawn()?;
        }
    }
    Ok(())
}

/// Save the video frame at `t` as a photo next to the video (VideoPlayer saveFrame).
pub(crate) fn save_frame(a: &Rc<AppInner>, item: MediaItem, input: String, t: f64) {
    let Some(ops) = a.backend.ops() else { return };
    let stem = item
        .name
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(&item.name)
        .to_string();
    let name = format!(
        "{stem}_{}.jpg",
        crate::viewer_fmt::clock(t).replace(':', "-")
    );
    let tmp = std::env::temp_dir().join(format!("pa-frame-{}-{name}", std::process::id()));
    let to = DirTarget {
        folder_id: item.folder_id.clone(),
        dir: item.dir.clone(),
    };
    let up = ops.upload(tmp.clone(), to, name.clone());
    let w = Rc::downgrade(a);
    a.rt.spawn_then(
        async move {
            let t2 = tmp.clone();
            let grabbed =
                tokio::task::spawn_blocking(move || crate::video::grab_frame(&input, t, &t2))
                    .await
                    .map_err(|e| e.to_string())?;
            grabbed?;
            let r = up.await.map_err(|e| e.message);
            let _ = std::fs::remove_file(&tmp);
            r
        },
        move |r: Result<(), String>| {
            let Some(a) = w.upgrade() else { return };
            match r {
                Ok(()) => a.toast(ToastKind::Success, &a.t("viewer.frameSaved"), &name),
                Err(e) => a.toast(ToastKind::Error, &a.t("common.error"), &e),
            }
        },
    );
}

// ───────── the hub ─────────

impl Hub {
    fn app(&self) -> Option<Rc<AppInner>> {
        self.app.upgrade()
    }

    fn lang(&self) -> Lang {
        self.app().map(|a| a.lang()).unwrap_or(Lang::En)
    }

    fn t(&self, k: &str) -> String {
        i18n::t(self.lang(), k)
    }

    fn tn(&self, k: &str, n: usize) -> String {
        let l = self.lang();
        i18n::tv(l, k, &[("n", &i18n::format_count(l, n as i64))])
    }

    fn ctx(&self) -> Ctx {
        let info = self.info.borrow();
        let libs = self.libs.borrow();
        let mut c = Ctx {
            writable: info.as_ref().is_some_and(|i| !i.caps.readonly),
            desktop: info
                .as_ref()
                .is_some_and(|i| i.mode == pa_core::RunMode::Desktop),
            zip_download: info.as_ref().is_some_and(|i| i.caps.zip_download),
            trash: info.as_ref().map(|i| i.caps.trash),
            albums: self.app().is_some_and(|a| a.backend.ops().is_some()) && info.is_some(),
            album: self.album.borrow().clone(),
            folder_pages: self.folder_pages.get(),
            ..Default::default()
        };
        if let Some(l) = libs.as_ref() {
            c.temp = l.temp.iter().map(|f| f.id.clone()).collect();
            c.device = l
                .devices
                .iter()
                .flat_map(|d| d.folders.iter().map(|f| f.id.clone()))
                .chain(
                    l.libraries
                        .iter()
                        .flat_map(|lib| lib.folders.iter())
                        .filter(|f| f.device.is_some())
                        .map(|f| f.id.clone()),
                )
                .collect();
            c.remote = l
                .libraries
                .iter()
                .flat_map(|lib| lib.folders.iter())
                .filter(|f| matches!(f.remote, Some(Some(_))))
                .map(|f| f.id.clone())
                .collect();
        }
        c
    }

    fn load_info(self: &Rc<Self>) {
        let Some(a) = self.app() else { return };
        let me = Rc::downgrade(self);
        a.rt.spawn_then(a.backend.info(), move |r| {
            if let (Some(h), Ok(i)) = (me.upgrade(), r) {
                *h.info.borrow_mut() = Some(i);
                h.publish_selection();
            }
        });
        self.load_libraries();
    }

    fn load_libraries(self: &Rc<Self>) {
        let Some(a) = self.app() else { return };
        let Some(ops) = a.backend.ops() else { return };
        let me = Rc::downgrade(self);
        a.rt.spawn_then(ops.libraries(), move |r| {
            if let (Some(h), Ok(l)) = (me.upgrade(), r) {
                *h.libs.borrow_mut() = Some(l);
                h.publish_selection();
            }
        });
    }

    // ───────── selection bar ─────────

    fn publish_selection(&self) {
        let Some(a) = self.app() else { return };
        let items = a.grid.0.borrow().ix.selection.items();
        let st = a.window.global::<ActionState>();
        let n = items.len();
        st.set_sel_count(n as i32);
        st.set_sel_label(self.tn("common.selected", n).into());
        if n == 0 {
            st.set_sel_actions(ModelRc::default());
            return;
        }
        let ctx = self.ctx();
        let list = actions::for_items(&items, Surface::Selection, &ctx);
        let lang = self.lang();
        let mut prev = None;
        let v: Vec<ActionItem> = list
            .iter()
            .map(|act| {
                let d = ActionItem {
                    id: act.id.clone().into(),
                    icon: icon(&a, act.icon_for(&items)),
                    label: act.label_for(&items, &ctx, lang).into(),
                    active: act.is_active(&items),
                    danger: act.destructive,
                    sep: prev.is_some_and(|g| g != act.group),
                };
                prev = Some(act.group);
                d
            })
            .collect();
        st.set_sel_actions(ModelRc::new(VecModel::from(v)));
    }

    fn selection_changed(&self) {
        if let Some(a) = self.app() {
            a.grid.0.borrow_mut().selection_changed();
        }
        self.publish_selection();
    }

    fn select_all(&self) {
        let Some(a) = self.app() else { return };
        let total = a.grid.0.borrow().total();
        if total == 0 {
            return;
        }
        let w = Rc::downgrade(&a);
        a.grid
            .0
            .borrow_mut()
            .items_in_range(0, total - 1, move |list| {
                if let Some(a) = w.upgrade() {
                    a.grid.0.borrow_mut().ix.selection.set(list);
                    if let Some(h) = hub() {
                        h.selection_changed();
                    }
                }
            });
    }

    fn clear_selection(&self) {
        let Some(a) = self.app() else { return };
        a.grid.0.borrow_mut().ix.selection.clear();
        self.selection_changed();
    }

    // ───────── context menu ─────────

    fn menu_open(&self) -> bool {
        self.app()
            .is_some_and(|a| a.window.global::<ActionState>().get_menu_open())
    }

    fn open_menu(&self, items: Vec<MediaItem>, index: Option<usize>, wx: f64, wy: f64) {
        let Some(a) = self.app() else { return };
        if items.is_empty() {
            return;
        }
        let ctx = self.ctx();
        let lang = self.lang();
        let list = actions::for_items(&items, Surface::Menu, &ctx);
        let mut v: Vec<ActionItem> = Vec::new();
        if items.len() == 1 && index.is_some() {
            v.push(ActionItem {
                id: "open".into(),
                icon: icon(&a, "maximize-2"),
                label: self.t("common.open").into(),
                ..Default::default()
            });
        }
        let mut group = None;
        for act in list {
            v.push(ActionItem {
                id: act.id.clone().into(),
                icon: icon(&a, act.icon_for(&items)),
                label: act.label_for(&items, &ctx, lang).into(),
                active: act.is_active(&items),
                danger: act.destructive,
                sep: !v.is_empty() && group != Some(act.group),
            });
            group = Some(act.group);
        }
        *self.menu_items.borrow_mut() = items;
        self.menu_index.set(index);
        let st = a.window.global::<ActionState>();
        st.set_menu_items(ModelRc::new(VecModel::from(v)));
        st.set_menu_x(wx as f32);
        st.set_menu_y(wy as f32);
        st.set_menu_open(true);
    }

    fn close_menu(&self) {
        if let Some(a) = self.app() {
            a.window.global::<ActionState>().set_menu_open(false);
        }
        if !self.viewer_open.get() {
            focus_grid();
        }
    }

    fn menu_action(self: &Rc<Self>, id: &str) {
        let Some(a) = self.app() else { return };
        self.close_menu();
        let items = std::mem::take(&mut *self.menu_items.borrow_mut());
        if id == "open" {
            if let Some(i) = self.menu_index.get() {
                open_viewer(i);
            }
            return;
        }
        self.run(&a, id, items, Surface::Menu);
    }

    // ───────── running actions ─────────

    fn run(self: &Rc<Self>, a: &Rc<AppInner>, id: &str, items: Vec<MediaItem>, surface: Surface) {
        if items.is_empty() {
            return;
        }
        let Some(act) = actions::get(id) else { return };
        if let Run::Custom(f) = &act.run {
            f(items);
            if surface == Surface::Selection && act.clears_selection {
                self.clear_selection();
            }
            return;
        }
        let ops_ok = a.backend.ops().is_some();
        if !ops_ok {
            return;
        }
        match id {
            "favorite" => self.favorite(a, items),
            "edit" => crate::editor::open(a, items[0].clone()),
            "download" => self.download(a, items),
            "slideshow" => {
                if let Some(v) = crate::viewer::get() {
                    v.close_now();
                }
                crate::slideshow_ui::open_with(a, items);
            }
            "add-to-album" => self.open_album_picker(items),
            "move" | "copy" => self.open_move(items, id == "copy"),
            "rename" => {
                let it = items[0].clone();
                let st = a.window.global::<ActionState>();
                st.set_rename_text(it.name.clone().into());
                st.set_busy(false);
                *self.dialog.borrow_mut() = Some(DialogKind::Rename(it));
                st.set_dialog("rename".into());
            }
            "show-in-folder" => {
                if let Some(v) = crate::viewer::get() {
                    v.close_now();
                }
                let it = &items[0];
                a.go(
                    crate::router::Route::Folder(it.folder_id.clone(), it.dir.clone()),
                    true,
                );
            }
            "reveal" => self.reveal(a, items[0].clone()),
            "copy-path" => self.copy_path(a, items[0].clone()),
            "delete" => self.delete(a, items),
            _ => log::warn!("action {id}: no built-in handler"),
        }
    }

    fn favorite(&self, a: &Rc<AppInner>, items: Vec<MediaItem>) {
        let Some(ops) = a.backend.ops() else { return };
        let fav = !actions::all_favorite(&items);
        let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
        // optimistic: patch every loaded copy in place (no relayout)
        a.grid
            .0
            .borrow_mut()
            .patch_items(&ids, |m| m.favorite = fav);
        if let Some(v) = crate::viewer::get() {
            v.patch_current(&|m| {
                if ids.contains(&m.id) {
                    m.favorite = fav
                }
            });
        }
        self.publish_selection();
        let w = Rc::downgrade(a);
        let ids2 = ids.clone();
        a.rt.spawn_then(ops.set_favorite(ids, fav), move |r| {
            let Some(a) = w.upgrade() else { return };
            if let Err(e) = r {
                a.grid
                    .0
                    .borrow_mut()
                    .patch_items(&ids2, |m| m.favorite = !fav);
                if let Some(v) = crate::viewer::get() {
                    v.patch_current(&|m| {
                        if ids2.contains(&m.id) {
                            m.favorite = !fav
                        }
                    });
                }
                a.toast(ToastKind::Error, &a.t("common.error"), &e.message);
            }
        });
    }

    fn download(&self, a: &Rc<AppInner>, items: Vec<MediaItem>) {
        let Some(ops) = a.backend.ops() else { return };
        let dir = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(std::env::temp_dir);
        let name = if items.len() == 1 {
            items[0].name.clone()
        } else {
            format!("PeekAround-{}.zip", items.len())
        };
        let dest = crate::actions_ops::unique_path(&dir, &name);
        let ids: Vec<String> = items.iter().map(|i| i.id.clone()).collect();
        a.toast(ToastKind::Info, &a.t("common.download"), &name);
        let w = Rc::downgrade(a);
        a.rt.spawn_then(ops.download(ids, dest), move |r| {
            let Some(a) = w.upgrade() else { return };
            match r {
                Ok(p) => {
                    let l = a.lang();
                    a.toast(
                        ToastKind::Success,
                        &i18n::tv(
                            l,
                            "native.savedTo",
                            &[(
                                "name",
                                &p.file_name()
                                    .map(|f| f.to_string_lossy().into_owned())
                                    .unwrap_or_default(),
                            )],
                        ),
                        &p.parent()
                            .map(|d| d.display().to_string())
                            .unwrap_or_default(),
                    );
                }
                Err(e) => a.toast(ToastKind::Error, &a.t("common.error"), &e.message),
            }
        });
    }

    fn with_path(
        &self,
        a: &Rc<AppInner>,
        item: MediaItem,
        then: impl FnOnce(&Rc<AppInner>, String) + 'static,
    ) {
        let Some(ops) = a.backend.ops() else { return };
        let w = Rc::downgrade(a);
        a.rt.spawn_then(ops.media(&item.id), move |r| {
            let Some(a) = w.upgrade() else { return };
            match r {
                Ok(d) => then(&a, d.path),
                Err(e) => a.toast(ToastKind::Error, &a.t("common.error"), &e.message),
            }
        });
    }

    fn reveal(&self, a: &Rc<AppInner>, item: MediaItem) {
        self.with_path(a, item, |a, path| {
            let p = PathBuf::from(&path);
            if !p.is_absolute() || !p.exists() {
                a.toast(ToastKind::Error, &a.t("common.error"), &path);
                return;
            }
            if let Err(e) = reveal_in_file_manager(&p) {
                a.toast(ToastKind::Error, &a.t("common.error"), &e.to_string());
            }
        });
    }

    fn copy_path(&self, a: &Rc<AppInner>, item: MediaItem) {
        self.with_path(a, item, |a, path| match set_clipboard(&path) {
            Ok(()) => a.toast(ToastKind::Success, &a.t("native.pathCopied"), &path),
            Err(e) => a.toast(ToastKind::Error, &a.t("common.error"), &e),
        });
    }

    fn delete(&self, a: &Rc<AppInner>, items: Vec<MediaItem>) {
        let ctx = self.ctx();
        let temp = ctx.any_temp(&items);
        let trash = ctx.trash;
        let confirm = a.prefs.borrow().confirm_delete;
        let has_trash = matches!(
            trash,
            Some(pa_core::TrashMode::Os) | Some(pa_core::TrashMode::Folder)
        );
        if !confirm && !temp && has_trash {
            self.do_delete(a, items);
            return;
        }
        let st = a.window.global::<ActionState>();
        st.set_dialog_title(self.tn("sel.deleteConfirm", items.len()).into());
        st.set_dialog_body(
            self.t(if temp {
                "sel.deleteBodyTemp"
            } else if trash == Some(pa_core::TrashMode::Os) {
                "sel.deleteBodyTrash"
            } else {
                "sel.deleteBodyFolder"
            })
            .into(),
        );
        st.set_busy(false);
        *self.dialog.borrow_mut() = Some(DialogKind::Delete(items));
        st.set_dialog("delete".into());
    }

    fn do_delete(&self, a: &Rc<AppInner>, items: Vec<MediaItem>) {
        let Some(ops) = a.backend.ops() else { return };
        let refs: Vec<FileRef> = items.iter().map(ref_of).collect();
        let n = refs.len();
        let w = Rc::downgrade(a);
        a.window.global::<ActionState>().set_busy(true);
        a.rt.spawn_then(ops.remove(refs), move |r| {
            let Some(a) = w.upgrade() else { return };
            let Some(h) = hub() else { return };
            a.window.global::<ActionState>().set_busy(false);
            h.close_dialog();
            match r {
                Ok(()) => {
                    h.clear_selection();
                    a.grid.0.borrow_mut().on_media_event(None);
                    a.toast(ToastKind::Success, &h.tn("sel.deleted", n), "");
                }
                Err(e) => a.toast(ToastKind::Error, &a.t("common.error"), &e.message),
            }
        });
    }

    // ───────── dialogs ─────────

    fn close_dialog(&self) {
        if !self.viewer_open.get() {
            focus_grid();
        }
        *self.dialog.borrow_mut() = None;
        if let Some(a) = self.app() {
            let st = a.window.global::<ActionState>();
            st.set_dialog("".into());
            st.set_busy(false);
        }
    }

    fn confirm_dialog(self: &Rc<Self>) {
        let Some(a) = self.app() else { return };
        let Some(d) = self.dialog.borrow().clone() else {
            return;
        };
        let st = a.window.global::<ActionState>();
        if st.get_busy() {
            return;
        }
        let Some(ops) = a.backend.ops() else { return };
        match d {
            DialogKind::Delete(items) => self.do_delete(&a, items),
            DialogKind::Rename(item) => {
                let name = st.get_rename_text().trim().to_string();
                if name.is_empty() || name == item.name {
                    self.close_dialog();
                    return;
                }
                st.set_busy(true);
                let w = Rc::downgrade(&a);
                a.rt.spawn_then(ops.rename(ref_of(&item), name), move |r| {
                    let Some(a) = w.upgrade() else { return };
                    let Some(h) = hub() else { return };
                    match r {
                        Ok(()) => {
                            h.close_dialog();
                            a.grid.0.borrow_mut().on_media_event(None);
                        }
                        Err(e) => {
                            a.window.global::<ActionState>().set_busy(false);
                            a.toast(ToastKind::Error, &a.t("common.error"), &e.message);
                        }
                    }
                });
            }
            DialogKind::Move { items, copy } => {
                let Some(target) = self.picked_target() else {
                    return;
                };
                let refs: Vec<FileRef> = items.iter().map(ref_of).collect();
                let n = refs.len();
                st.set_busy(true);
                let w = Rc::downgrade(&a);
                a.rt.spawn_then(ops.move_files(refs, target, copy), move |r| {
                    let Some(a) = w.upgrade() else { return };
                    let Some(h) = hub() else { return };
                    h.close_dialog();
                    match r {
                        Ok(()) => {
                            h.clear_selection();
                            a.grid.0.borrow_mut().on_media_event(None);
                            a.toast(
                                ToastKind::Success,
                                &h.tn(if copy { "sel.copied" } else { "sel.moved" }, n),
                                "",
                            );
                        }
                        Err(e) => a.toast(ToastKind::Error, &a.t("common.error"), &e.message),
                    }
                });
            }
            DialogKind::Album(items) => {
                let sel = st.get_picker_selected().to_string();
                let Some(id) = sel.strip_prefix("album:").map(str::to_string) else {
                    return;
                };
                self.close_dialog();
                self.add_to_album(&a, id, items);
            }
            DialogKind::Upload(paths) => {
                let Some(target) = self.picked_target() else {
                    return;
                };
                self.close_dialog();
                self.upload(&a, paths, target);
            }
        }
    }

    fn picked_target(&self) -> Option<DirTarget> {
        let a = self.app()?;
        let sel = a
            .window
            .global::<ActionState>()
            .get_picker_selected()
            .to_string();
        self.picker
            .borrow()
            .iter()
            .find(|n| n.key == sel)
            .and_then(|n| n.target.clone())
    }

    fn add_to_album(&self, a: &Rc<AppInner>, id: String, items: Vec<MediaItem>) {
        let Some(ops) = a.backend.ops() else { return };
        let ids: Vec<String> = items
            .iter()
            .filter(|m| m.kind != pa_core::MediaKind::Audio)
            .map(|m| m.id.clone())
            .collect();
        let n = ids.len();
        let name = self
            .albums
            .borrow()
            .iter()
            .find(|al| al.id == id)
            .map(|al| al.name.clone())
            .unwrap_or_default();
        let w = Rc::downgrade(a);
        a.rt.spawn_then(ops.add_to_album(id, ids), move |r| {
            let Some(a) = w.upgrade() else { return };
            match r {
                Ok(()) => {
                    let l = a.lang();
                    a.toast(
                        ToastKind::Success,
                        &i18n::tv(
                            l,
                            "album.added",
                            &[("n", &i18n::format_count(l, n as i64)), ("name", &name)],
                        ),
                        "",
                    );
                }
                Err(e) => a.toast(ToastKind::Error, &a.t("common.error"), &e.message),
            }
        });
    }

    fn open_album_picker(self: &Rc<Self>, items: Vec<MediaItem>) {
        let Some(a) = self.app() else { return };
        let Some(ops) = a.backend.ops() else { return };
        let st = a.window.global::<ActionState>();
        st.set_dialog_title(self.tn("album.pickTitle", items.len()).into());
        st.set_dialog_body("".into());
        st.set_confirm_label(self.t("album.addTo").into());
        st.set_new_album_name("".into());
        st.set_picker_selected("".into());
        st.set_picker_loading(true);
        st.set_picker_rows(ModelRc::default());
        st.set_confirm_enabled(false);
        *self.dialog.borrow_mut() = Some(DialogKind::Album(items));
        st.set_dialog("album".into());
        let me = Rc::downgrade(self);
        a.rt.spawn_then(ops.albums(), move |r| {
            let Some(h) = me.upgrade() else { return };
            let Some(a) = h.app() else { return };
            let st = a.window.global::<ActionState>();
            st.set_picker_loading(false);
            let albums = r.unwrap_or_default();
            let lang = a.lang();
            *h.picker.borrow_mut() = albums
                .iter()
                .map(|al| PickNode {
                    key: format!("album:{}", al.id),
                    label: al.name.clone(),
                    sub: i18n::format_count(lang, al.count),
                    depth: 0,
                    icon: "album",
                    target: None,
                    expanded: false,
                    loaded: true,
                    loading: false,
                    can_expand: false,
                    enabled: true,
                })
                .collect();
            *h.albums.borrow_mut() = albums;
            h.publish_picker();
        });
    }

    fn create_album(&self) {
        let Some(a) = self.app() else { return };
        let Some(ops) = a.backend.ops() else { return };
        let st = a.window.global::<ActionState>();
        let name = st.get_new_album_name().trim().to_string();
        let Some(DialogKind::Album(items)) = self.dialog.borrow().clone() else {
            return;
        };
        if name.is_empty() {
            return;
        }
        self.close_dialog();
        let ids: Vec<String> = items.iter().map(|m| m.id.clone()).collect();
        let n = ids.len();
        let w = Rc::downgrade(&a);
        a.rt.spawn_then(ops.create_album(name.clone(), ids), move |r| {
            let Some(a) = w.upgrade() else { return };
            match r {
                Ok(_) => {
                    let l = a.lang();
                    a.toast(
                        ToastKind::Success,
                        &i18n::tv(
                            l,
                            "album.added",
                            &[("n", &i18n::format_count(l, n as i64)), ("name", &name)],
                        ),
                        "",
                    );
                }
                Err(e) => a.toast(ToastKind::Error, &a.t("common.error"), &e.message),
            }
        });
    }

    fn open_move(self: &Rc<Self>, items: Vec<MediaItem>, copy: bool) {
        let Some(a) = self.app() else { return };
        let st = a.window.global::<ActionState>();
        st.set_dialog_title(self.t(if copy { "sel.copy" } else { "sel.move" }).into());
        st.set_dialog_body(self.t("sel.pickTarget").into());
        st.set_confirm_label(
            self.t(if copy { "sel.copy" } else { "sel.move" })
                .trim_end_matches('…')
                .into(),
        );
        *self.dialog.borrow_mut() = Some(DialogKind::Move { items, copy });
        self.open_folder_picker();
        st.set_dialog("move".into());
    }

    fn open_folder_picker(self: &Rc<Self>) {
        let Some(a) = self.app() else { return };
        let st = a.window.global::<ActionState>();
        st.set_picker_selected("".into());
        st.set_confirm_enabled(false);
        let ctx = self.ctx();
        let libs = self.libs.borrow().clone();
        let mut nodes = Vec::new();
        if let Some(l) = libs {
            for lib in &l.libraries {
                nodes.push(PickNode {
                    key: format!("lib:{}", lib.id),
                    label: lib.name.clone(),
                    sub: String::new(),
                    depth: 0,
                    icon: "library",
                    target: None,
                    expanded: true,
                    loaded: true,
                    loading: false,
                    can_expand: false,
                    enabled: false,
                });
                for f in &lib.folders {
                    let ro = ctx.device.contains(&f.id) || ctx.remote.contains(&f.id);
                    nodes.push(PickNode {
                        key: format!("dir:{}:", f.id),
                        label: f.name.clone(),
                        sub: String::new(),
                        depth: 1,
                        icon: "folder",
                        target: Some(DirTarget {
                            folder_id: f.id.clone(),
                            dir: String::new(),
                        }),
                        expanded: false,
                        loaded: false,
                        loading: false,
                        can_expand: true,
                        enabled: !ro,
                    });
                }
            }
        }
        *self.picker.borrow_mut() = nodes;
        st.set_picker_loading(false);
        self.publish_picker();
    }

    fn publish_picker(&self) {
        let Some(a) = self.app() else { return };
        let st = a.window.global::<ActionState>();
        let nodes = self.picker.borrow();
        // hide the children of collapsed nodes
        let mut rows = Vec::new();
        let mut hide_below: Option<i32> = None;
        for n in nodes.iter() {
            if let Some(d) = hide_below {
                if n.depth > d {
                    continue;
                }
                hide_below = None;
            }
            rows.push(PickerRow {
                key: n.key.clone().into(),
                label: n.label.clone().into(),
                sub: n.sub.clone().into(),
                depth: n.depth,
                icon: n.icon.into(),
                enabled: n.enabled || n.icon == "library",
                expanded: n.expanded,
                has_children: n.can_expand,
                loading: n.loading,
            });
            if n.can_expand && !n.expanded {
                hide_below = Some(n.depth);
            }
        }
        st.set_picker_rows(ModelRc::new(VecModel::from(rows)));
    }

    fn picker_click(self: &Rc<Self>, key: &str) {
        let Some(a) = self.app() else { return };
        let st = a.window.global::<ActionState>();
        let node = self.picker.borrow().iter().find(|n| n.key == key).cloned();
        let Some(n) = node else { return };
        if n.icon == "library" {
            return;
        }
        if !n.enabled {
            return;
        }
        st.set_picker_selected(key.into());
        st.set_confirm_enabled(true);
    }

    fn picker_toggle(self: &Rc<Self>, key: &str) {
        let Some(a) = self.app() else { return };
        let (needs_load, target, depth) = {
            let mut nodes = self.picker.borrow_mut();
            let Some(n) = nodes.iter_mut().find(|n| n.key == key) else {
                return;
            };
            if !n.can_expand {
                return;
            }
            n.expanded = !n.expanded;
            let need = n.expanded && !n.loaded && !n.loading;
            if need {
                n.loading = true;
            }
            (need, n.target.clone(), n.depth)
        };
        self.publish_picker();
        let (true, Some(t)) = (needs_load, target) else {
            return;
        };
        let q = pa_core::BrowseQuery {
            folder_id: Some(t.folder_id.clone()),
            dir: Some(t.dir.clone()),
            limit: Some(0),
            ..Default::default()
        };
        let me = Rc::downgrade(self);
        let key = key.to_string();
        a.rt.spawn_then(a.backend.browse(q), move |r| {
            let Some(h) = me.upgrade() else { return };
            {
                let mut nodes = h.picker.borrow_mut();
                let Some(pos) = nodes.iter().position(|n| n.key == key) else {
                    return;
                };
                nodes[pos].loading = false;
                nodes[pos].loaded = true;
                let enabled = nodes[pos].enabled;
                let subs = r.map(|b| b.folders).unwrap_or_default();
                if subs.is_empty() {
                    nodes[pos].can_expand = false;
                }
                let new: Vec<PickNode> = subs
                    .into_iter()
                    .map(|sf| PickNode {
                        key: format!("dir:{}:{}", t.folder_id, sf.dir),
                        label: sf.name,
                        sub: String::new(),
                        depth: depth + 1,
                        icon: "folder",
                        target: Some(DirTarget {
                            folder_id: t.folder_id.clone(),
                            dir: sf.dir,
                        }),
                        expanded: false,
                        loaded: false,
                        loading: false,
                        can_expand: true,
                        enabled,
                    })
                    .collect();
                for (k, n) in new.into_iter().enumerate() {
                    nodes.insert(pos + 1 + k, n);
                }
            }
            h.publish_picker();
        });
    }

    // ───────── uploads (OS drops) ─────────

    fn files_dropped(self: &Rc<Self>, paths: Vec<PathBuf>) {
        let Some(a) = self.app() else { return };
        if crate::pages::fonts::wants_drop(&a) {
            crate::pages::fonts::upload(&a, paths);
            return;
        }
        let ctx = self.ctx();
        if !ctx.writable {
            a.toast(ToastKind::Error, &a.t("drop.readonly"), "");
            return;
        }
        let target = self.drop_target.borrow().clone();
        match target {
            Some((t, _)) => self.upload(&a, paths, t),
            None => {
                let st = a.window.global::<ActionState>();
                st.set_dialog_title(self.t("drop.pickTitle").into());
                st.set_dialog_body(
                    paths
                        .iter()
                        .take(3)
                        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                        .collect::<Vec<_>>()
                        .join(", ")
                        .into(),
                );
                st.set_confirm_label(self.t("native.upload").into());
                *self.dialog.borrow_mut() = Some(DialogKind::Upload(paths));
                self.open_folder_picker();
                st.set_dialog("move".into());
            }
        }
    }

    fn upload(&self, a: &Rc<AppInner>, paths: Vec<PathBuf>, to: DirTarget) {
        let Some(ops) = a.backend.ops() else { return };
        // expand folders (keeping their structure under the target)
        let mut files: Vec<(PathBuf, String)> = Vec::new();
        for p in &paths {
            collect_files(p, p.parent().unwrap_or(Path::new("")), &mut files);
        }
        let n = files.len();
        if n == 0 {
            return;
        }
        let l = a.lang();
        a.toast(
            ToastKind::Info,
            &a.t("upload.title"),
            &i18n::tv(
                l,
                "upload.progress",
                &[("done", "0"), ("total", &n.to_string())],
            ),
        );
        let futs: Vec<_> = files
            .into_iter()
            .map(|(f, rel)| ops.upload(f, to.clone(), rel))
            .collect();
        let w = Rc::downgrade(a);
        a.rt.spawn_then(
            async move {
                let mut ok = 0usize;
                let mut failed = 0usize;
                let mut last_err = String::new();
                for f in futs {
                    match f.await {
                        Ok(()) => ok += 1,
                        Err(e) => {
                            failed += 1;
                            last_err = e.message;
                        }
                    }
                }
                (ok, failed, last_err)
            },
            move |(ok, failed, err)| {
                let Some(a) = w.upgrade() else { return };
                let l = a.lang();
                if ok > 0 {
                    a.toast(
                        ToastKind::Success,
                        &a.t("upload.done"),
                        &i18n::tv(
                            l,
                            "upload.doneBody",
                            &[("n", &i18n::format_count(l, ok as i64))],
                        ),
                    );
                    a.grid.0.borrow_mut().on_media_event(None);
                }
                if failed > 0 {
                    a.toast(
                        ToastKind::Error,
                        &i18n::tv(
                            l,
                            "upload.failed",
                            &[("n", &i18n::format_count(l, failed as i64))],
                        ),
                        &err,
                    );
                }
            },
        );
    }

    // ───────── grid pointer ─────────

    fn grid_pointer(
        self: &Rc<Self>,
        kind: i32,
        button: i32,
        x: f64,
        y: f64,
        ox: f64,
        oy: f64,
        ctrl: bool,
        shift: bool,
    ) {
        let Some(a) = self.app() else { return };
        {
            let mut g = a.grid.0.borrow_mut();
            let scroll = g.viewport().0;
            g.ix.origin = (ox, oy);
            g.ix.origin_scroll = scroll;
        }
        let (wx, wy) = (ox + x, oy + y);
        match (kind, button) {
            // ── press ──
            (0, 0) => {
                let (idx, group, on_check) = self.hit(&a, x, y);
                if idx.is_none() && group.is_none() {
                    // lasso on empty space
                    let base = {
                        let mut g = a.grid.0.borrow_mut();
                        if !(ctrl || shift) {
                            g.ix.selection.clear();
                        }
                        g.ix.selection.clone()
                    };
                    self.selection_changed();
                    *self.lasso.borrow_mut() = Some(Lasso {
                        x0: x,
                        y0: y,
                        x1: x,
                        y1: y,
                        base,
                        wy,
                    });
                    return;
                }
                *self.press.borrow_mut() = Some(Press {
                    x,
                    y,
                    idx,
                    group,
                    on_check,
                    ctrl,
                    shift,
                    moved: false,
                });
            }
            (0, 1) => {
                // right click: the media menu for the selection (if the tile is in it) or the tile
                let (idx, _, _) = self.hit(&a, x, y);
                let Some(i) = idx else { return };
                let (item, list) = {
                    let g = a.grid.0.borrow();
                    let Some(item) = g.get_at(i) else { return };
                    let list = if g.ix.selection.contains(&item.id) {
                        g.ix.selection.items()
                    } else {
                        vec![item.clone()]
                    };
                    (item, list)
                };
                let _ = item;
                let single = (list.len() == 1).then_some(i);
                self.open_menu(list, single, wx, wy);
            }
            // ── move ──
            (2, b) => {
                if b != 0 {
                    // hover
                    let (idx, group, _) = self.hit(&a, x, y);
                    let mut g = a.grid.0.borrow_mut();
                    g.set_hover(idx);
                    drop(g);
                    let gi = a.window.global::<GridInput>();
                    gi.set_hover_group(group.map(|g| g as i32).unwrap_or(-1));
                    return;
                }
                if self.lasso.borrow().is_some() {
                    self.lasso_move(&a, x, y, wy);
                    return;
                }
                let start_drag = {
                    let mut p = self.press.borrow_mut();
                    let Some(p) = p.as_mut() else { return };
                    if !p.moved && (x - p.x).hypot(y - p.y) > 6.0 {
                        p.moved = true;
                        p.idx
                    } else {
                        None
                    }
                };
                if let Some(i) = start_drag {
                    self.start_tile_drag(&a, i);
                }
                if self.drag.borrow().is_some() {
                    let ds = a.window.global::<DragState>();
                    ds.set_x(wx as f32);
                    ds.set_y(wy as f32);
                    let copy = ctrl;
                    if let Some(d) = self.drag.borrow_mut().as_mut() {
                        d.1 = copy;
                    }
                }
            }
            // ── release ──
            (1, 0) => {
                if self.lasso.borrow_mut().take().is_some() {
                    self.scroll_timer.stop();
                    a.window.global::<GridInput>().set_lasso(false);
                    return;
                }
                if self.drag.borrow().is_some() {
                    self.end_tile_drag(&a, true);
                    self.press.borrow_mut().take();
                    return;
                }
                let Some(p) = self.press.borrow_mut().take() else {
                    return;
                };
                if p.moved {
                    return;
                }
                self.click(&a, p);
            }
            (3, _) => {
                self.lasso.borrow_mut().take();
                self.press.borrow_mut().take();
                self.scroll_timer.stop();
                a.window.global::<GridInput>().set_lasso(false);
                if self.drag.borrow().is_some() {
                    self.end_tile_drag(&a, false);
                }
            }
            (4, _) => {
                a.grid.0.borrow_mut().set_hover(None);
                a.window.global::<GridInput>().set_hover_group(-1);
            }
            _ => {}
        }
    }

    /// (tile, header select circle, on the tile's check circle)
    fn hit(&self, a: &AppInner, x: f64, y: f64) -> (Option<usize>, Option<usize>, bool) {
        let g = a.grid.0.borrow();
        let l = g.layout();
        if let Some(r) = l.row_at(y) {
            if l.row_header[r] && y >= l.row_top[r] && x >= l.width - 40.0 {
                return (None, Some(l.row_group[r] as usize), false);
            }
        }
        let idx = l.item_at(x, y).filter(|&i| g.get_at(i).is_some());
        let on_check = idx
            .and_then(|i| g.tile_rect(i))
            .is_some_and(|(tx, ty, _, _)| {
                let (cx, cy) = (x - tx, y - ty);
                (0.0..=40.0).contains(&cx) && (0.0..=40.0).contains(&cy)
            });
        (idx, None, on_check)
    }

    fn click(self: &Rc<Self>, a: &Rc<AppInner>, p: Press) {
        if let Some(gr) = p.group {
            let (start, count) = {
                let g = a.grid.0.borrow();
                match g.layout().groups.get(gr) {
                    Some(grp) => (grp.start, grp.count),
                    None => return,
                }
            };
            let w = Rc::downgrade(a);
            a.grid.0.borrow_mut().items_in_range(
                start,
                start + count.saturating_sub(1),
                move |list| {
                    let Some(a) = w.upgrade() else { return };
                    {
                        let mut g = a.grid.0.borrow_mut();
                        let all =
                            !list.is_empty() && list.iter().all(|m| g.ix.selection.contains(&m.id));
                        if all {
                            let ids: Vec<String> = list.iter().map(|m| m.id.clone()).collect();
                            g.ix.selection.remove(&ids);
                        } else {
                            g.ix.selection.add(list);
                        }
                    }
                    if let Some(h) = hub() {
                        h.selection_changed();
                    }
                },
            );
            return;
        }
        let Some(i) = p.idx else { return };
        let (action, item) = {
            let g = a.grid.0.borrow();
            let Some(item) = g.get_at(i) else { return };
            (
                g.ix.selection.click_action(i, p.ctrl, p.shift, p.on_check),
                item,
            )
        };
        {
            let mut g = a.grid.0.borrow_mut();
            g.ix.ring = false;
            g.set_focus(Some(i));
        }
        match action {
            ClickAction::Open => open_viewer(i),
            ClickAction::Toggle => {
                a.grid.0.borrow_mut().ix.selection.toggle(&item, i);
                self.selection_changed();
            }
            ClickAction::Range(s, e) => {
                a.grid.0.borrow_mut().ix.selection.anchor = Some(i);
                let w = Rc::downgrade(a);
                a.grid.0.borrow_mut().items_in_range(s, e, move |list| {
                    if let Some(a) = w.upgrade() {
                        a.grid.0.borrow_mut().ix.selection.add(list);
                        if let Some(h) = hub() {
                            h.selection_changed();
                        }
                    }
                });
            }
        }
    }

    fn lasso_move(self: &Rc<Self>, a: &Rc<AppInner>, x: f64, y: f64, wy: f64) {
        let rect = {
            let mut l = self.lasso.borrow_mut();
            let Some(l) = l.as_mut() else { return };
            l.x1 = x;
            l.y1 = y;
            l.wy = wy;
            selection::rect(l.x0, l.y0, l.x1, l.y1)
        };
        let gi = a.window.global::<GridInput>();
        gi.set_lasso(true);
        gi.set_lasso_x(rect.0 as f32);
        gi.set_lasso_y(rect.1 as f32);
        gi.set_lasso_w((rect.2 - rect.0) as f32);
        gi.set_lasso_h((rect.3 - rect.1) as f32);
        {
            let mut g = a.grid.0.borrow_mut();
            let hits = selection::lasso_hits(g.layout(), rect);
            let items: Vec<MediaItem> = hits.into_iter().filter_map(|i| g.get_at(i)).collect();
            let base = self
                .lasso
                .borrow()
                .as_ref()
                .map(|l| l.base.clone())
                .unwrap_or_default();
            let mut sel = base;
            sel.add(items);
            g.ix.selection = sel;
        }
        self.selection_changed();
        // auto-scroll near the viewport edges
        self.edge_scroll(a, wy);
    }

    fn edge_scroll(self: &Rc<Self>, a: &Rc<AppInner>, wy: f64) {
        let (top, bottom) = self.viewport_window_y(a);
        let speed = if wy > bottom - 40.0 {
            18.0
        } else if wy < top + 40.0 {
            -18.0
        } else {
            0.0
        };
        if speed == 0.0 {
            self.scroll_timer.stop();
            return;
        }
        if self.scroll_timer.running() {
            return;
        }
        let me = Rc::downgrade(self);
        self.scroll_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(16),
            move || {
                let Some(h) = me.upgrade() else { return };
                let Some(a) = h.app() else { return };
                let Some((wy, x1, y1)) = h.lasso.borrow().as_ref().map(|l| (l.wy, l.x1, l.y1))
                else {
                    h.scroll_timer.stop();
                    return;
                };
                let (top, bottom) = h.viewport_window_y(&a);
                let sp = if wy > bottom - 40.0 {
                    18.0
                } else if wy < top + 40.0 {
                    -18.0
                } else {
                    0.0
                };
                if sp == 0.0 {
                    h.scroll_timer.stop();
                    return;
                }
                let gs = a.window.global::<GridState>();
                let max = (a.grid.0.borrow().height() + 400.0) as f32;
                let before = gs.get_scroll_y();
                let ny = (before - sp as f32).clamp(-max, 0.0);
                gs.set_scroll_y(ny);
                let moved = (before - ny) as f64;
                // the pointer stays put on screen: its grid y moves with the scroll
                h.lasso_move(&a, x1, y1 + moved, wy);
            },
        );
    }

    fn viewport_window_y(&self, a: &AppInner) -> (f64, f64) {
        let g = a.grid.0.borrow();
        let (scroll, vh) = g.viewport();
        let oy = g.ix.origin.1 + g.ix.origin_scroll - scroll;
        let top = oy + scroll;
        (top, top + vh)
    }

    fn start_tile_drag(&self, a: &Rc<AppInner>, i: usize) {
        let (list, ghost) = {
            let mut g = a.grid.0.borrow_mut();
            let Some(item) = g.get_at(i) else { return };
            let list = if g.ix.selection.contains(&item.id) {
                g.ix.selection.items()
            } else {
                vec![item.clone()]
            };
            let ghost = g
                .cached_thumb(&item.id, 320)
                .or_else(|| g.placeholder_of(i));
            (list, ghost)
        };
        let ds = a.window.global::<DragState>();
        ds.set_count(list.len() as i32);
        ds.set_ghost(ghost.unwrap_or_default());
        ds.set_over("".into());
        ds.set_active(true);
        *self.drag.borrow_mut() = Some((list, false));
    }

    fn end_tile_drag(&self, a: &Rc<AppInner>, drop: bool) {
        let ds = a.window.global::<DragState>();
        let over = ds.get_over().to_string();
        ds.set_active(false);
        ds.set_over("".into());
        let Some((items, copy)) = self.drag.borrow_mut().take() else {
            return;
        };
        if !drop || over.is_empty() {
            return;
        }
        self.drop_on(a, &over, items, copy);
    }

    fn drop_on(&self, a: &Rc<AppInner>, key: &str, items: Vec<MediaItem>, copy: bool) {
        let handlers = self.drop_handlers.borrow().clone();
        if let Some((_, f)) = handlers.iter().find(|(p, _)| key.starts_with(p.as_str())) {
            f(key, items, copy);
            return;
        }
        if let Some(id) = key.strip_prefix("album:") {
            self.add_to_album(a, id.to_string(), items);
        } else if let Some(rest) = key.strip_prefix("folder:") {
            let (fid, dir) = rest.split_once(':').unwrap_or((rest, ""));
            let Some(ops) = a.backend.ops() else { return };
            let ctx = self.ctx();
            if !ctx.writable {
                return;
            }
            let copy = copy || ctx.on_device(&items);
            let refs: Vec<FileRef> = items.iter().map(ref_of).collect();
            let n = refs.len();
            let w = Rc::downgrade(a);
            let to = DirTarget {
                folder_id: fid.to_string(),
                dir: dir.to_string(),
            };
            a.rt.spawn_then(ops.move_files(refs, to, copy), move |r| {
                let Some(a) = w.upgrade() else { return };
                let Some(h) = hub() else { return };
                match r {
                    Ok(()) => {
                        a.grid.0.borrow_mut().on_media_event(None);
                        a.toast(
                            ToastKind::Success,
                            &h.tn(if copy { "sel.copied" } else { "sel.moved" }, n),
                            "",
                        );
                    }
                    Err(e) => a.toast(ToastKind::Error, &a.t("common.error"), &e.message),
                }
            });
        } else if key == "new-album" {
            self.open_album_picker_rc(items);
        } else {
            log::warn!("drop on {key}: no handler");
        }
    }

    fn open_album_picker_rc(&self, items: Vec<MediaItem>) {
        if let Some(h) = hub() {
            h.open_album_picker(items);
        }
    }

    // ───────── grid keyboard ─────────

    fn grid_key(self: &Rc<Self>, text: &str, ctrl: bool, shift: bool) -> bool {
        use slint::platform::Key;
        let Some(a) = self.app() else { return false };
        let t = SharedString::from(text);
        let k = |key: Key| -> SharedString {
            let c: char = key.into();
            c.to_string().into()
        };
        let step = if t == k(Key::RightArrow) {
            Some(KeyStep::Right)
        } else if t == k(Key::LeftArrow) {
            Some(KeyStep::Left)
        } else if t == k(Key::DownArrow) {
            Some(KeyStep::Down)
        } else if t == k(Key::UpArrow) {
            Some(KeyStep::Up)
        } else if t == k(Key::Home) && ctrl {
            Some(KeyStep::Home)
        } else if t == k(Key::End) && ctrl {
            Some(KeyStep::End)
        } else {
            None
        };
        if let Some(st) = step {
            let next = {
                let g = a.grid.0.borrow();
                let l = g.layout();
                if l.total == 0 {
                    return false;
                }
                match g.ix.focus {
                    Some(i) => selection::key_step(l, i, st),
                    None => {
                        let (scroll, _) = g.viewport();
                        l.nearest_item(0.0, scroll.max(0.0) + 10.0).unwrap_or(0)
                    }
                }
            };
            let mut g = a.grid.0.borrow_mut();
            g.ix.ring = true;
            g.set_focus(Some(next));
            g.scroll_to_index(next, false);
            if shift {
                if let Some(it) = g.get_at(next) {
                    g.ix.selection.add([it]);
                    drop(g);
                    self.selection_changed();
                }
            }
            return true;
        }
        let focus = a.grid.0.borrow().ix.focus;
        let focused_item = focus.and_then(|i| a.grid.0.borrow().get_at(i));
        if t == k(Key::Return) {
            if let Some(i) = focus {
                open_viewer(i);
                return true;
            }
            return false;
        }
        if text == " " {
            if let (Some(i), Some(it)) = (focus, focused_item) {
                a.grid.0.borrow_mut().ix.selection.toggle(&it, i);
                self.selection_changed();
                return true;
            }
            return false;
        }
        if t == k(Key::Escape) {
            if !a.grid.0.borrow().ix.selection.is_empty() {
                self.clear_selection();
                return true;
            }
            return false;
        }
        if ctrl && text.eq_ignore_ascii_case("a") {
            self.select_all();
            return true;
        }
        let selected = a.grid.0.borrow().ix.selection.items();
        if t == k(Key::F2) {
            let target = if selected.len() == 1 {
                selected
            } else if selected.is_empty() {
                focused_item.into_iter().collect()
            } else {
                vec![]
            };
            if !target.is_empty()
                && actions::for_items(&target, Surface::Menu, &self.ctx())
                    .iter()
                    .any(|x| x.id == "rename")
            {
                self.run(&a, "rename", target, Surface::Menu);
                return true;
            }
            return false;
        }
        if t == k(Key::Delete) || t == k(Key::Backspace) {
            let list = if !selected.is_empty() {
                selected
            } else if t == k(Key::Delete) {
                focused_item.into_iter().collect()
            } else {
                vec![]
            };
            if !list.is_empty()
                && actions::for_items(&list, Surface::Menu, &self.ctx())
                    .iter()
                    .any(|x| x.id == "delete")
            {
                self.run(&a, "delete", list, Surface::Selection);
                return true;
            }
            return false;
        }
        false
    }

    // ───────── density (Ctrl + wheel) ─────────

    /// `dy` > 0: wheel up (bigger tiles). Returns true when handled.
    fn density_wheel(&self, dy: f64, wx: Option<f64>, wy: Option<f64>) -> bool {
        let Some(a) = self.app() else { return false };
        if self.viewer_open.get() {
            return false;
        }
        if !matches!(
            a.window.global::<crate::Shell>().get_page(),
            crate::Page::Photos | crate::Page::Favorites | crate::Page::Search
        ) && a.grid.0.borrow().total() == 0
        {
            return false;
        }
        let acc = self.wheel_acc.get() + dy;
        if acc.abs() < 70.0 {
            self.wheel_acc.set(acc);
            return true;
        }
        self.wheel_acc.set(0.0);
        let cur = a.prefs.borrow().grid_zoom_desktop;
        let next = (cur + if acc < 0.0 { 1 } else { -1 }).clamp(0, 3);
        if next == cur {
            return true;
        }
        // anchor: the item under the pointer keeps its screen position
        let (anchor, offset) = {
            let g = a.grid.0.borrow();
            let (scroll, vh) = g.viewport();
            let (ox, oy0) = g.ix.origin;
            let oy = oy0 + g.ix.origin_scroll - scroll;
            let (gx, gy) = match (wx, wy) {
                (Some(x), Some(y)) => (x - ox, y - oy),
                _ => (g.layout().width / 2.0, scroll + vh / 2.0),
            };
            let i = g.layout().nearest_item(gx.max(0.0), gy.max(0.0));
            (i, gy - scroll)
        };
        a.set_pref("density", &next.to_string());
        if let Some(i) = anchor {
            let g = a.grid.0.borrow();
            if let Some((_, y, _, h)) = g.tile_rect(i) {
                let target = (y + h / 2.0 - offset).max(0.0);
                let gs = a.window.global::<GridState>();
                let header = -(gs.get_scroll_y() as f64) - g.viewport().0;
                gs.set_scroll_y(-((target + header).max(0.0) as f32));
            }
        }
        true
    }
}

fn ref_of(m: &MediaItem) -> FileRef {
    FileRef {
        folder_id: m.folder_id.clone(),
        rel: pa_core::join_rel(&m.dir, &m.name),
    }
}

fn collect_files(p: &Path, base: &Path, out: &mut Vec<(PathBuf, String)>) {
    if p.is_dir() {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for e in entries {
            if e.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            collect_files(&e, base, out);
        }
    } else if p.is_file() {
        let rel = p
            .strip_prefix(base)
            .unwrap_or(p)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        out.push((p.to_path_buf(), rel));
    }
}

/// Put text on the clipboard.
pub(crate) fn set_clipboard(text: &str) -> Result<(), String> {
    #[cfg(feature = "clipboard")]
    {
        let mut c = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        c.set_text(text.to_string()).map_err(|e| e.to_string())
    }
    #[cfg(not(feature = "clipboard"))]
    {
        let _ = text;
        Err("no clipboard".into())
    }
}

// ───────── winit: OS file drops, Ctrl+wheel ─────────

#[cfg(feature = "desktop")]
fn install_winit(app: &Rc<AppInner>) {
    use slint::winit_030::winit::event::{MouseScrollDelta, WindowEvent};
    use slint::winit_030::{EventResult, WinitWindowAccessor};
    app.window.window().on_winit_window_event(move |w, ev| {
        // observers of the desktop shell (focus, scale factor): pa-desktop can't install its own filter
        crate::pages::notify_window_event(ev);
        let Some(h) = hub() else {
            return EventResult::Propagate;
        };
        let sc = w.scale_factor() as f64;
        match ev {
            WindowEvent::ModifiersChanged(m) => {
                let s = m.state();
                h.ctrl_down.set(s.control_key() || s.super_key());
            }
            WindowEvent::CursorMoved { position, .. } => {
                h.cursor.set((position.x / sc, position.y / sc));
            }
            WindowEvent::MouseWheel { delta, .. } if h.ctrl_down.get() => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y as f64 * 100.0,
                    MouseScrollDelta::PixelDelta(p) => p.y / sc,
                };
                let (cx, cy) = h.cursor.get();
                if h.density_wheel(dy, Some(cx), Some(cy)) {
                    return EventResult::PreventDefault;
                }
            }
            WindowEvent::HoveredFile(p) => {
                h.hovered_files.borrow_mut().push(p.clone());
                if let Some(a) = h.app() {
                    let ds = a.window.global::<DragState>();
                    let ctx = h.ctx();
                    ds.set_files_readonly(!ctx.writable);
                    ds.set_files_target(
                        h.drop_target
                            .borrow()
                            .as_ref()
                            .map(|t| t.1.clone())
                            .unwrap_or_default()
                            .into(),
                    );
                    ds.set_files_over(true);
                }
            }
            WindowEvent::HoveredFileCancelled => {
                h.hovered_files.borrow_mut().clear();
                if let Some(a) = h.app() {
                    a.window.global::<DragState>().set_files_over(false);
                }
            }
            WindowEvent::DroppedFile(p) => {
                h.hovered_files.borrow_mut().clear();
                h.dropped.borrow_mut().push(p.clone());
                if let Some(a) = h.app() {
                    a.window.global::<DragState>().set_files_over(false);
                }
                // one event per file: collect them, then upload as one batch
                let me = Rc::downgrade(&h);
                h.drop_timer.start(
                    slint::TimerMode::SingleShot,
                    Duration::from_millis(80),
                    move || {
                        if let Some(h) = me.upgrade() {
                            let paths = std::mem::take(&mut *h.dropped.borrow_mut());
                            h.files_dropped(paths);
                        }
                    },
                );
            }
            _ => {}
        }
        EventResult::Propagate
    });
}

/// Simulate an OS file drop (automation / tests).
pub(crate) fn simulate_drop(paths: Vec<PathBuf>) {
    if let Some(h) = hub() {
        h.files_dropped(paths);
    }
}

/// Simulate Ctrl+wheel at a window point (automation).
pub(crate) fn simulate_density_wheel(dy: f64, x: f64, y: f64) {
    if let Some(h) = hub() {
        h.wheel_acc.set(0.0);
        h.density_wheel(dy, Some(x), Some(y));
    }
}

/// Wire everything (call once after the app is built).
pub(crate) fn install(app: &Rc<AppInner>) {
    let h = Rc::new(Hub {
        app: Rc::downgrade(app),
        info: RefCell::new(None),
        libs: RefCell::new(None),
        albums: RefCell::new(vec![]),
        album: RefCell::new(None),
        drop_target: RefCell::new(None),
        folder_pages: Cell::new(false),
        press: RefCell::new(None),
        lasso: RefCell::new(None),
        drag: RefCell::new(None),
        drop_handlers: RefCell::new(vec![]),
        menu_items: RefCell::new(vec![]),
        menu_index: Cell::new(None),
        dialog: RefCell::new(None),
        picker: RefCell::new(vec![]),
        viewer_open: Cell::new(false),
        wheel_acc: Cell::new(0.0),
        ctrl_down: Cell::new(false),
        cursor: Cell::new((0.0, 0.0)),
        hovered_files: RefCell::new(vec![]),
        dropped: RefCell::new(vec![]),
        drop_timer: slint::Timer::default(),
        scroll_timer: slint::Timer::default(),
    });
    HUB.with(|c| *c.borrow_mut() = Some(h.clone()));
    crate::viewer::install(app);

    let gi = app.window.global::<GridInput>();
    {
        let w = Rc::downgrade(&h);
        gi.on_pointer(move |k, b, x, y, ox, oy, c, s| {
            if let Some(h) = w.upgrade() {
                h.grid_pointer(k, b, x as f64, y as f64, ox as f64, oy as f64, c, s);
            }
        });
    }
    {
        let w = Rc::downgrade(&h);
        gi.on_key(move |t, c, s, _alt| w.upgrade().is_some_and(|h| h.grid_key(&t, c, s)));
    }
    {
        let w = Rc::downgrade(&h);
        gi.on_wheel(move |_x, _y, dy, ctrl| {
            ctrl && w
                .upgrade()
                .is_some_and(|h| h.density_wheel(dy as f64, None, None))
        });
    }
    {
        let w = Rc::downgrade(app);
        gi.on_label_at(move |y| {
            w.upgrade()
                .map(|a| a.grid.0.borrow().label_at(y as f64))
                .unwrap_or_default()
                .into()
        });
    }
    let st = app.window.global::<ActionState>();
    macro_rules! cb {
        ($on:ident, |$h:ident $(, $arg:ident)*| $body:expr) => {{
            let w = Rc::downgrade(&h);
            st.$on(move |$($arg),*| {
                if let Some($h) = w.upgrade() {
                    $body
                }
            });
        }};
    }
    cb!(on_sel_action, |h, id| {
        if let Some(a) = h.app() {
            let items = a.grid.0.borrow().ix.selection.items();
            h.run(&a, &id, items, Surface::Selection);
            if actions::get(&id)
                .is_some_and(|x| x.clears_selection && matches!(x.run, Run::Custom(_)))
            {
                h.clear_selection();
            }
        }
    });
    cb!(on_sel_all, |h| h.select_all());
    cb!(on_sel_clear, |h| h.clear_selection());
    cb!(on_menu_action, |h, id| h.menu_action(&id));
    cb!(on_menu_close, |h| h.close_menu());
    cb!(on_dialog_close, |h| h.close_dialog());
    cb!(on_dialog_confirm, |h| h.confirm_dialog());
    cb!(on_picker_click, |h, k| h.picker_click(&k));
    cb!(on_picker_toggle, |h, k| h.picker_toggle(&k));
    cb!(on_create_album, |h| h.create_album());
    let ds = app.window.global::<DragState>();
    ds.on_enter(|k| {
        if let Some(a) = hub().and_then(|h| h.app()) {
            a.window.global::<DragState>().set_over(k);
        }
    });
    ds.on_leave(|k| {
        if let Some(a) = hub().and_then(|h| h.app()) {
            let ds = a.window.global::<DragState>();
            if ds.get_over() == k {
                ds.set_over("".into());
            }
        }
    });
    // the selection bar follows the grid's selection (also cleared by queries)
    let w = Rc::downgrade(&h);
    app.grid.0.borrow_mut().ix.listeners.push(Rc::new(move || {
        if let Some(h) = w.upgrade() {
            let n = h
                .app()
                .map(|a| a.grid.0.borrow().ix.selection.len())
                .unwrap_or(0);
            if let Some(a) = h.app() {
                if a.window.global::<ActionState>().get_sel_count() as usize != n {
                    h.publish_selection();
                }
            }
        }
    }));
    #[cfg(feature = "desktop")]
    install_winit(app);
    h.load_info();
}
