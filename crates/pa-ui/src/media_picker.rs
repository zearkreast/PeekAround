//! Photo / video picker shared by the slideshow ("Add media", MediaPicker.tsx) and the collage
//! (PhotoPicker.tsx): libraries → folders → their media; several with a confirm button, or one
//! photo at once. The selection survives folder changes.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use pa_core::{BrowseQuery, DirTarget, KindFilter, LibrariesResponse, Library, MediaItem, MediaKind};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::app::AppInner;
use crate::i18n;
use crate::{MediaDirRow, MediaPickerState, MediaTileRow};

/// What the picker is for.
pub(crate) struct Opts {
    pub title: String,
    /// photos only (else photos and videos)
    pub images_only: bool,
    /// one tap picks
    pub single: bool,
    pub select_all: bool,
    /// open at this folder
    pub start: Option<DirTarget>,
    /// footer texts with `{n}`: "3 selected", "Add 3"; the empty folder note
    pub selected_key: &'static str,
    pub add_key: &'static str,
    pub empty_key: &'static str,
}

type OnPick = Box<dyn FnOnce(&Rc<AppInner>, Vec<MediaItem>)>;

#[derive(Clone)]
struct Loc {
    folder_id: String,
    dir: String,
    label: String,
}

#[derive(Default)]
struct Pick {
    opts: Option<Opts>,
    on_pick: Option<OnPick>,
    libs: Vec<Library>,
    stack: Vec<Loc>,
    /// where each row of `dirs` leads
    dirs: Vec<Loc>,
    /// the open folder's media
    items: Vec<MediaItem>,
    selected: Vec<MediaItem>,
    tiles: Option<Rc<VecModel<MediaTileRow>>>,
    /// tiles whose thumbnail was requested
    asked: HashSet<usize>,
    /// bumps on every navigation (late browse results are dropped)
    epoch: u64,
}

thread_local! {
    static PICK: RefCell<Pick> = RefCell::new(Pick::default());
}

fn with<R>(f: impl FnOnce(&mut Pick) -> R) -> R {
    PICK.with(|p| f(&mut p.borrow_mut()))
}

fn lib_label(l: &Library, f: &pa_core::FolderRef) -> String {
    if l.folders.len() > 1 {
        format!("{} · {}", l.name, f.name)
    } else {
        l.name.clone()
    }
}

/// Open the picker; `on_pick` gets the chosen media (in the order they were picked).
pub(crate) fn open(a: &Rc<AppInner>, opts: Opts, on_pick: impl FnOnce(&Rc<AppInner>, Vec<MediaItem>) + 'static) {
    let s = a.window.global::<MediaPickerState>();
    s.set_title(opts.title.clone().into());
    s.set_single(opts.single);
    s.set_can_select_all(opts.select_all);
    s.set_empty_text(a.t(opts.empty_key).into());
    s.set_open(true);
    s.set_loading(true);
    with(|p| {
        *p = Pick::default();
        p.opts = Some(opts);
        p.on_pick = Some(Box::new(on_pick));
    });
    publish(a);
    crate::api::call::<LibrariesResponse>(a, "GET", "/api/libraries", None, |a, r| {
        let start = with(|p| {
            p.libs = r.map(|r| r.libraries).unwrap_or_default();
            let start = p.opts.as_ref().and_then(|o| o.start.clone())?;
            let (l, f) = p.libs.iter().find_map(|l| l.folders.iter().find(|f| f.id == start.folder_id).map(|f| (l, f)))?;
            let mut stack = vec![Loc { folder_id: f.id.clone(), dir: String::new(), label: lib_label(l, f) }];
            let parts: Vec<&str> = start.dir.split('/').filter(|s| !s.is_empty()).collect();
            for i in 0..parts.len() {
                stack.push(Loc { folder_id: f.id.clone(), dir: parts[..=i].join("/"), label: parts[i].to_string() });
            }
            p.stack = stack;
            Some(())
        });
        a.window.global::<MediaPickerState>().set_loading(false);
        if start.is_some() {
            load(a);
        } else {
            publish(a);
        }
    });
}

pub(crate) fn close(a: &Rc<AppInner>) {
    with(|p| *p = Pick::default());
    let s = a.window.global::<MediaPickerState>();
    s.set_open(false);
    s.set_dirs(ModelRc::default());
    s.set_items(ModelRc::default());
}

/// The breadcrumb, the selection count and, at the top, the library folders.
fn publish(a: &Rc<AppInner>) {
    let s = a.window.global::<MediaPickerState>();
    let lang = a.lang();
    let rows = with(|p| {
        let path = p.stack.iter().map(|s| s.label.as_str()).collect::<Vec<_>>().join(" / ");
        s.set_path(if path.is_empty() { a.t("nav.libraries") } else { path }.into());
        s.set_can_back(!p.stack.is_empty());
        let n = p.selected.len();
        s.set_selected(n as i32);
        if let Some(o) = &p.opts {
            let n = n.to_string();
            s.set_selected_text(i18n::tv(lang, o.selected_key, &[("n", &n)]).into());
            s.set_add_text(i18n::tv(lang, o.add_key, &[("n", &n)]).into());
        }
        if !p.stack.is_empty() {
            return None;
        }
        let mut rows = vec![];
        let mut dirs = vec![];
        for l in &p.libs {
            for f in &l.folders {
                rows.push(MediaDirRow { label: lib_label(l, f).into(), lib: true, color: crate::theme::color(&l.color), icon: l.icon.clone().into() });
                dirs.push(Loc { folder_id: f.id.clone(), dir: String::new(), label: lib_label(l, f) });
            }
        }
        p.dirs = dirs;
        p.items.clear();
        p.tiles = None;
        Some(rows)
    });
    if let Some(rows) = rows {
        s.set_dirs(ModelRc::new(VecModel::from(rows)));
        s.set_items(ModelRc::default());
    }
}

/// Browse the current folder: sub folders + its media.
fn load(a: &Rc<AppInner>) {
    let Some((q, epoch)) = with(|p| {
        let cur = p.stack.last().cloned()?;
        p.epoch += 1;
        p.dirs.clear();
        p.items.clear();
        p.tiles = None;
        p.asked.clear();
        let images = p.opts.as_ref().is_some_and(|o| o.images_only);
        let q = BrowseQuery {
            folder_id: Some(cur.folder_id),
            dir: Some(cur.dir),
            kind: Some(if images { KindFilter::Image } else { KindFilter::All }),
            limit: Some(1000),
            ..Default::default()
        };
        Some((q, p.epoch))
    }) else {
        return;
    };
    let s = a.window.global::<MediaPickerState>();
    s.set_loading(true);
    s.set_dirs(ModelRc::default());
    s.set_items(ModelRc::default());
    publish(a);
    let fut = a.backend.browse(q);
    let w = Rc::downgrade(a);
    a.rt.spawn_then(fut, move |r| {
        let Some(a) = w.upgrade() else { return };
        let s = a.window.global::<MediaPickerState>();
        let models = with(|p| {
            if p.epoch != epoch {
                return None;
            }
            let cur = p.stack.last().cloned()?;
            let Ok(r) = r else { return Some(None) };
            let images = p.opts.as_ref().is_some_and(|o| o.images_only);
            p.dirs = r.folders.iter().map(|f| Loc { folder_id: cur.folder_id.clone(), dir: f.dir.clone(), label: f.name.clone() }).collect();
            let rows: Vec<MediaDirRow> = r.folders.iter().map(|f| MediaDirRow { label: f.name.clone().into(), ..Default::default() }).collect();
            p.items = r
                .items
                .into_iter()
                .filter(|m| m.kind == MediaKind::Image || (!images && m.kind == MediaKind::Video))
                .collect();
            let tiles: Vec<MediaTileRow> = p
                .items
                .iter()
                .map(|m| MediaTileRow {
                    is_video: m.kind == MediaKind::Video,
                    dur: i18n::format_duration(m.duration).into(),
                    on: p.selected.iter().any(|s| s.id == m.id),
                    ..Default::default()
                })
                .collect();
            let tiles = Rc::new(VecModel::from(tiles));
            p.tiles = Some(tiles.clone());
            Some(Some((rows, tiles)))
        });
        let Some(models) = models else { return };
        s.set_loading(false);
        if let Some((rows, tiles)) = models {
            s.set_dirs(ModelRc::new(VecModel::from(rows)));
            s.set_items(ModelRc::from(tiles));
        }
    });
}

fn open_dir(a: &Rc<AppInner>, i: usize) {
    let next = with(|p| p.dirs.get(i).cloned());
    if let Some(l) = next {
        with(|p| p.stack.push(l));
        load(a);
    }
}

fn back(a: &Rc<AppInner>) {
    let empty = with(|p| {
        p.stack.pop();
        p.epoch += 1;
        p.stack.is_empty()
    });
    if empty {
        a.window.global::<MediaPickerState>().set_loading(false);
        publish(a);
    } else {
        load(a);
    }
}

/// a tile became visible: fetch its thumbnail once
fn need(a: &Rc<AppInner>, i: usize) {
    let Some((id, tiles, epoch)) = with(|p| {
        let (m, t) = (p.items.get(i)?, p.tiles.clone()?);
        let id = m.id.clone();
        p.asked.insert(i).then_some((id, t, p.epoch))
    }) else {
        return;
    };
    let w = Rc::downgrade(&tiles);
    crate::pages::cover(a, &id, 320, move |img| {
        let still = with(|p| p.epoch == epoch);
        if let (true, Some(m)) = (still, w.upgrade()) {
            if let Some(mut r) = m.row_data(i) {
                r.thumb = img;
                r.has_thumb = true;
                m.set_row_data(i, r);
            }
        }
    });
}

fn toggle(a: &Rc<AppInner>, i: usize) {
    if with(|p| p.opts.as_ref().is_some_and(|o| o.single)) {
        if let Some(m) = with(|p| p.items.get(i).cloned()) {
            finish(a, vec![m]);
        }
        return;
    }
    with(|p| {
        let Some(m) = p.items.get(i).cloned() else { return };
        let on = match p.selected.iter().position(|s| s.id == m.id) {
            Some(k) => {
                p.selected.remove(k);
                false
            }
            None => {
                p.selected.push(m);
                true
            }
        };
        if let Some(t) = &p.tiles {
            if let Some(mut r) = t.row_data(i) {
                r.on = on;
                t.set_row_data(i, r);
            }
        }
    });
    publish(a);
}

fn select_all(a: &Rc<AppInner>) {
    with(|p| {
        for (i, m) in p.items.iter().enumerate() {
            if !p.selected.iter().any(|s| s.id == m.id) {
                p.selected.push(m.clone());
            }
            if let Some(t) = &p.tiles {
                if let Some(mut r) = t.row_data(i) {
                    r.on = true;
                    t.set_row_data(i, r);
                }
            }
        }
    });
    publish(a);
}

fn finish(a: &Rc<AppInner>, picked: Vec<MediaItem>) {
    let then = with(|p| p.on_pick.take());
    close(a);
    if let Some(f) = then {
        f(a, picked);
    }
}

fn add(a: &Rc<AppInner>) {
    let picked = with(|p| std::mem::take(&mut p.selected));
    if !picked.is_empty() {
        finish(a, picked);
    }
}

pub(crate) fn install(app: &Rc<AppInner>) {
    let s = app.window.global::<MediaPickerState>();
    macro_rules! on {
        ($cb:ident, |$a:ident $(, $arg:ident)*| $body:expr) => {{
            let w = Rc::downgrade(app);
            s.$cb(move |$($arg),*| {
                if let Some($a) = w.upgrade() {
                    $body;
                }
            });
        }};
    }
    on!(on_close, |a| close(&a));
    on!(on_open_dir, |a, i| open_dir(&a, i as usize));
    on!(on_back, |a| back(&a));
    on!(on_need, |a, i| need(&a, i as usize));
    on!(on_toggle, |a, i| toggle(&a, i as usize));
    on!(on_select_all, |a| select_all(&a));
    on!(on_add, |a| add(&a));
}
