//! The library hub, albums, album, library / folder pages, their dialogs and Settings:
//! state kept here, models pushed into the Slint globals of `ui/pages/state.slint` & co.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

use pa_core::{
    Album, BrowseQuery, BrowseResult, KindFilter, LibrariesResponse, LibraryStats, ServerEvent,
    SortDir, SortKey,
};
use slint::Image;

use crate::app::AppInner;
use crate::backend::BackendEvent;
use crate::router::Route;
use crate::thumbs::{self, ThumbCache};

pub(crate) mod albums;
pub(crate) mod dialogs;
pub(crate) mod fonts;
pub(crate) mod hub;
pub(crate) mod jobs;
pub(crate) mod library;
pub(crate) mod settings;

thread_local! {
    static APP: RefCell<Weak<AppInner>> = const { RefCell::new(Weak::new()) };
}

/// the running app (UI thread)
pub(crate) fn app() -> Option<Rc<AppInner>> {
    APP.with(|a| a.borrow().upgrade())
}

type CoverCb = Box<dyn FnOnce(Image)>;

/// Cover / card thumbnails (decoded off the UI thread, cached).
pub(crate) struct Covers {
    cache: RefCell<ThumbCache>,
    waiters: RefCell<HashMap<(String, u32), Vec<CoverCb>>>,
    failed: RefCell<HashSet<(String, u32)>>,
}

impl Default for Covers {
    fn default() -> Self {
        Covers {
            cache: RefCell::new(ThumbCache::new(96 << 20)),
            waiters: Default::default(),
            failed: Default::default(),
        }
    }
}

/// State of the library / album / settings screens.
#[derive(Default)]
pub(crate) struct Pages {
    /// GET /api/libraries
    pub libs: RefCell<Option<LibrariesResponse>>,
    /// GET /api/albums
    pub albums: RefCell<Option<Vec<Album>>>,
    /// library stats by id
    pub stats: RefCell<HashMap<String, LibraryStats>>,
    /// newest picture of a library without a picked cover
    pub auto_covers: RefCell<HashMap<String, Option<String>>>,
    /// thumbnails
    pub covers: Covers,
    /// the open album
    pub album: RefCell<Option<Album>>,
    /// browse page 0 of the open library / folder (crumbs, sub folders, roots, archives, target)
    pub meta: RefCell<Option<BrowseResult>>,
    /// bumps on every page change (stale results are dropped)
    pub epoch: Cell<u64>,
    /// debounce of library / album refreshes after server events
    pub refresh_pending: Cell<bool>,
    /// dialogs
    pub dialogs: dialogs::Dialogs,
    /// settings page
    pub settings: settings::SettingsCtl,
    /// jobs (panel / top bar)
    pub jobs: jobs::JobsCtl,
}

/// Load a thumbnail (`size` 320 / 640 / 1600); `then` runs on the UI thread.
pub(crate) fn cover(app: &Rc<AppInner>, id: &str, size: u32, then: impl FnOnce(Image) + 'static) {
    let key = (id.to_string(), size);
    let hit = app.pages.covers.cache.borrow_mut().get(id, size);
    if let Some(img) = hit {
        then(img);
        return;
    }
    if app.pages.covers.failed.borrow().contains(&key) {
        return;
    }
    let mut w = app.pages.covers.waiters.borrow_mut();
    let first = !w.contains_key(&key);
    w.entry(key.clone()).or_default().push(Box::new(then));
    drop(w);
    if !first {
        return;
    }
    let fut = app.backend.thumb(id.to_string(), size);
    let weak = Rc::downgrade(app);
    app.rt.spawn_then(
        async move {
            let d = fut.await.ok()?;
            tokio::task::spawn_blocking(move || thumbs::decode(d))
                .await
                .ok()
                .flatten()
        },
        move |px| {
            let Some(a) = weak.upgrade() else { return };
            let waiters = a
                .pages
                .covers
                .waiters
                .borrow_mut()
                .remove(&key)
                .unwrap_or_default();
            match px {
                Some(px) => {
                    let img = a
                        .pages
                        .covers
                        .cache
                        .borrow_mut()
                        .put(key.0.clone(), key.1, px);
                    for f in waiters {
                        f(img.clone());
                    }
                }
                None => {
                    a.pages.covers.failed.borrow_mut().insert(key);
                }
            }
        },
    );
}

/// what the top bar offers on this route (TopBar.tsx useChrome): sort / filter, Add, crumbs
pub(crate) fn chrome(app: &Rc<AppInner>) {
    use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
    let route = app.route.borrow().clone();
    let ch = app.window.global::<crate::Chrome>();
    ch.set_view_sort(!matches!(route, Route::Photos | Route::Album(_)));
    ch.set_view_kind(!matches!(route, Route::Favorites));
    let target = library::target(app);
    ch.set_can_add(target.is_some());
    let crumbs: Vec<SharedString> = match &route {
        Route::Folder(..) => app
            .pages
            .meta
            .borrow()
            .as_ref()
            .map(|m| {
                m.crumbs
                    .iter()
                    .take(m.crumbs.len().saturating_sub(1))
                    .map(|c| c.label.clone().into())
                    .collect()
            })
            .unwrap_or_default(),
        _ => vec![],
    };
    ch.set_crumbs(ModelRc::from(Rc::new(VecModel::from(crumbs))));
    if let Some(t) = target {
        let label = app
            .window
            .global::<crate::GridState>()
            .get_title()
            .to_string();
        crate::actions_ui::set_drop_target(Some((t, label)));
    }
}

/// wire every callback of these screens' globals
pub(crate) fn install(app: &Rc<AppInner>) {
    APP.with(|a| *a.borrow_mut() = Rc::downgrade(app));
    crate::actions_ui::set_folder_pages(true);
    hub::install(app);
    albums::install(app);
    library::install(app);
    dialogs::install(app);
    settings::install(app);
    jobs::install(app);
    fonts::install(app);
    // first data
    hub::load_libraries(app);
    albums::load_albums(app);
    jobs::load(app);
}

/// a route was entered
pub(crate) fn enter(app: &Rc<AppInner>, route: &Route) {
    app.pages.epoch.set(app.pages.epoch.get() + 1);
    crate::actions_ui::set_current_album(match route {
        Route::Album(id) => Some(id.clone()),
        _ => None,
    });
    crate::actions_ui::set_drop_target(None);
    crate::actions_ui::set_folder_pages(true);
    chrome(app);
    match route {
        Route::Library => {
            hub::load_libraries(app);
            albums::load_albums(app);
        }
        Route::Albums => albums::load_albums(app),
        Route::Album(id) => albums::enter_album(app, id),
        Route::Lib(_) | Route::Folder(..) => library::enter(app),
        Route::Settings => settings::enter(app),
        Route::Fonts => fonts::enter(app),
        _ => {}
    }
}

/// the grid query of an album / library / folder route (None: not ready yet)
pub(crate) fn grid_query(app: &AppInner) -> Option<BrowseQuery> {
    let p = app.prefs.borrow();
    let kind = match p.kind.as_str() {
        "image" => KindFilter::Image,
        "video" => KindFilter::Video,
        "favorites" => KindFilter::Favorites,
        _ => KindFilter::All,
    };
    let sort = serde_json::from_value(serde_json::Value::String(p.sort.clone()))
        .unwrap_or(SortKey::TakenAt);
    let order = if p.order == "asc" {
        SortDir::Asc
    } else {
        SortDir::Desc
    };
    let route = app.route.borrow().clone();
    match &route {
        Route::Album(id) => {
            let a = app.pages.album.borrow();
            let a = a.as_ref().filter(|a| &a.id == id)?;
            Some(BrowseQuery {
                album_id: Some(id.clone()),
                kind: Some(kind),
                sort: Some(SortKey::TakenAt),
                order: Some(if a.order == pa_core::AlbumOrder::Asc {
                    SortDir::Asc
                } else {
                    SortDir::Desc
                }),
                by_position: (a.order == pa_core::AlbumOrder::Custom).then_some(true),
                ..Default::default()
            })
        }
        Route::Lib(id) => Some(BrowseQuery {
            library_id: Some(id.clone()),
            recursive: Some(library::view_mode(&p, &route) == "all"),
            sort: Some(sort),
            order: Some(order),
            kind: Some(kind),
            ..Default::default()
        }),
        Route::Folder(id, dir) => Some(BrowseQuery {
            folder_id: Some(id.clone()),
            dir: Some(dir.clone()),
            recursive: Some(library::view_mode(&p, &route) == "all"),
            sort: Some(sort),
            order: Some(order),
            kind: Some(kind),
            ..Default::default()
        }),
        _ => None,
    }
}

/// live events
pub(crate) fn on_event(app: &Rc<AppInner>, e: &BackendEvent) {
    let BackendEvent::Server(ev) = e else { return };
    match ev {
        ServerEvent::Folders { .. }
        | ServerEvent::Devices
        | ServerEvent::Scan {
            scanning: false, ..
        } => schedule_refresh(app),
        ServerEvent::Albums { .. } => {
            albums::load_albums(app);
            if let Route::Album(id) = app.route.borrow().clone() {
                albums::reload_album(app, &id);
            }
        }
        ServerEvent::Job { job } => jobs::on_job(app, job),
        ServerEvent::Pair { device } => dialogs::on_pair(app, device),
        ServerEvent::MegaAccount => settings::on_mega_account(app),
        ServerEvent::Fonts => fonts::on_fonts_changed(app),
        _ => {}
    }
    if matches!(ev, ServerEvent::Devices) {
        dialogs::on_devices(app);
    }
}

/// libraries / devices changed: refresh the hub + the open library page (coalesced)
fn schedule_refresh(app: &Rc<AppInner>) {
    if app.pages.refresh_pending.replace(true) {
        return;
    }
    let weak = Rc::downgrade(app);
    slint::Timer::single_shot(std::time::Duration::from_millis(400), move || {
        let Some(a) = weak.upgrade() else { return };
        a.pages.refresh_pending.set(false);
        a.pages.stats.borrow_mut().clear();
        hub::load_libraries(&a);
        if matches!(&*a.route.borrow(), Route::Lib(_) | Route::Folder(..)) {
            library::refresh_meta(&a);
        }
        if matches!(&*a.route.borrow(), Route::Settings) {
            settings::refresh(&a);
        }
    });
}

/// A newer release (desktop update check): a toast with the release link.
pub fn announce_update(window: &crate::AppWindow, u: &crate::host::UpdateInfo) {
    let _ = window;
    if let Some(a) = app() {
        dialogs::update_toast(&a, u);
    }
}

/// automation script steps of these screens (perf.rs forwards unknown steps)
pub(crate) fn script_step(app: &Rc<AppInner>, cmd: &[String]) -> bool {
    let arg = cmd.get(1).cloned().unwrap_or_default();
    match cmd[0].as_str() {
        "go" => match Route::parse(&arg) {
            Some(r) => app.go(r, true),
            None => eprintln!("[script] bad route {arg}"),
        },
        "dialog" => dialogs::open_by_name(app, &arg, cmd.get(2).map(String::as_str).unwrap_or("")),
        "view" => library::set_view(app, &arg),
        "settings-search" => settings::search(app, &cmd[1..].join(" ")),
        "back" => {
            let prev = app.history.borrow_mut().pop();
            if let Some(p) = prev {
                app.go(p, false);
            }
        }
        _ => return false,
    }
    true
}

/// a winit window event observer (see [`observe_window_events`])
#[cfg(feature = "desktop")]
type WinitObserver = Box<dyn Fn(&slint::winit_030::winit::event::WindowEvent)>;

#[cfg(feature = "desktop")]
thread_local! {
    static OBSERVERS: RefCell<Vec<WinitObserver>> = const { RefCell::new(Vec::new()) };
}

/// Watch the window's winit events (focus, scale factor …). Slint keeps one winit event filter
/// per window and actions_ui owns it; it forwards every event here first.
#[cfg(feature = "desktop")]
pub fn observe_window_events(f: impl Fn(&slint::winit_030::winit::event::WindowEvent) + 'static) {
    OBSERVERS.with(|o| o.borrow_mut().push(Box::new(f)));
}

#[cfg(feature = "desktop")]
pub(crate) fn notify_window_event(ev: &slint::winit_030::winit::event::WindowEvent) {
    OBSERVERS.with(|o| {
        for f in o.borrow().iter() {
            f(ev);
        }
    });
}
