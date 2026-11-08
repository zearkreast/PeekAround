//! The app shell: window, router (Shell.page + a back stack), preferences, toasts, live events and
//! the grid controller, all wired to the Slint globals.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use pa_core::{BrowseQuery, KindFilter, ServerEvent, SortDir, SortKey};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

use crate::backend::{Backend, BackendEvent, Connection};
use crate::grid::{GridHandle, ViewOptions};
use crate::i18n::{self, HourCycle, Lang};
use crate::perf;
use crate::prefs::Prefs;
use crate::rt::Rt;
use crate::theme;
use crate::{
    AppWindow, Conn, GridState, I18n, Motion, Page, PrefsState, Shell, Theme, ThemeMode, ToastData,
    ToastKind,
};

/// Options of [`run`] (the binary maps CLI flags / env vars onto them).
#[derive(Debug, Clone, Default)]
pub struct UiOptions {
    /// where Settings → Appearance etc. are stored (None: not persisted)
    pub prefs_path: Option<PathBuf>,
    /// automation script (screenshots): `wait 2000; shot a.png; nav settings; set theme light; quit`
    pub script: Option<String>,
    /// scroll the grid by itself and log frame timings (perf check)
    pub autoscroll: Option<perf::AutoScroll>,
    /// log frame timings while running
    pub frame_log: bool,
}

pub(crate) struct AppInner {
    pub(crate) window: AppWindow,
    pub(crate) backend: Arc<dyn Backend>,
    pub(crate) rt: Rt,
    lang: Rc<Cell<Lang>>,
    pub(crate) prefs: RefCell<Prefs>,
    prefs_path: Option<PathBuf>,
    pub(crate) grid: GridHandle,
    /// back stack of routes + the current one (router.rs)
    pub(crate) history: RefCell<Vec<crate::router::Route>>,
    pub(crate) route: RefCell<crate::router::Route>,
    /// the desktop shell (dialogs, LAN, autostart …)
    pub(crate) host: Rc<dyn crate::host::Host>,
    /// library hub / albums / library pages / dialogs / settings
    pub(crate) pages: crate::pages::Pages,
    pub(crate) toasts: Rc<VecModel<ToastData>>,
    toast_seq: Cell<i32>,
    search_q: RefCell<String>,
}

/// Wire the string callbacks of an `I18n` global (any window type).
macro_rules! install_i18n {
    ($win:expr, $lang:expr) => {{
        let i = $win.global::<I18n>();
        let l = $lang.clone();
        i.on_t(move |k, _| i18n::t(l.get(), &k).into());
        let l = $lang.clone();
        i.on_t1(move |k, n, v, _| i18n::tv(l.get(), &k, &[(&n, &v)]).into());
        let l = $lang.clone();
        i.on_t2(move |k, n1, v1, n2, v2, _| {
            i18n::tv(l.get(), &k, &[(&n1, &v1), (&n2, &v2)]).into()
        });
    }};
}

fn sample_ms() -> i64 {
    // the web UI's sample date: 2024-07-05 14:30
    i18n::days_from_civil(2024, 7, 5) * 86_400_000 + (14 * 60 + 30) * 60_000
}

impl AppInner {
    pub(crate) fn lang(&self) -> Lang {
        self.lang.get()
    }

    pub(crate) fn t(&self, key: &str) -> String {
        i18n::t(self.lang(), key)
    }

    /// Push every preference into the Slint globals.
    fn apply_prefs(&self) {
        let p = self.prefs.borrow().clone();
        let w = &self.window;
        let lang = Lang::parse(&p.lang);
        let lang_changed = lang != self.lang.get();
        self.lang.set(lang);
        let i = w.global::<I18n>();
        i.set_lang(lang.code().into());
        if lang_changed {
            i.set_rev(i.get_rev() + 1);
        }
        let th = w.global::<Theme>();
        th.set_mode(match p.theme.as_str() {
            "light" => ThemeMode::Light,
            "system" => ThemeMode::System,
            _ => ThemeMode::Dark,
        });
        th.set_accent(theme::color(&p.accent));
        th.set_accent_2(theme::color(&theme::mix(&p.accent, 38.0)));
        th.set_accent_3(theme::color(&theme::mix(&p.accent, 75.0)));
        w.global::<Motion>().set_level(match p.motion.as_str() {
            "off" => 0,
            "reduced" => 1,
            _ => 2,
        });
        let ps = w.global::<PrefsState>();
        ps.set_lang(p.lang.clone().into());
        ps.set_theme(p.theme.clone().into());
        ps.set_accent(theme::color(&p.accent));
        ps.set_motion(p.motion.clone().into());
        ps.set_wide_sidebar(p.wide_sidebar);
        ps.set_date_format(p.date_format.clone().into());
        ps.set_hour_cycle(p.hour_cycle.clone().into());
        ps.set_week_start(p.week_start.clone().into());
        ps.set_sort(p.sort.clone().into());
        let deco = w.global::<crate::TileDeco>();
        deco.set_names(p.tile_names);
        deco.set_duration(p.tile_duration);
        deco.set_favorites(p.tile_favorites);
        ps.set_order(p.order.clone().into());
        let hc = match p.hour_cycle.as_str() {
            "h23" => HourCycle::H23,
            "h12" => HourCycle::H12,
            _ => HourCycle::Auto,
        };
        let iso = p.date_format == "iso";
        ps.set_date_preview(i18n::format_date_time(sample_ms(), lang, iso, hc).into());
        // the web UI previews the clock with "now"
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        ps.set_time_preview(
            i18n::format_time(now.rem_euclid(86_400_000) / 60_000, lang, hc, false).into(),
        );
        let gs = w.global::<GridState>();
        gs.set_density(p.grid_zoom_desktop);
        gs.set_square(p.grid_mode == "square");
        gs.set_row_height(p.row_height as f32);
        if lang_changed {
            self.host.set_lang(lang.code());
            self.grid.0.borrow_mut().set_lang(lang);
            self.refresh_grid_title();
        }
    }

    pub(crate) fn save_prefs(&self) {
        if let Some(path) = &self.prefs_path {
            if let Err(e) = self.prefs.borrow().save(path) {
                log::warn!("saving {}: {e}", path.display());
            }
        }
    }

    pub(crate) fn set_pref(self: &Rc<Self>, key: &str, value: &str) {
        let before = self.prefs.borrow().clone();
        {
            let mut p = self.prefs.borrow_mut();
            match key {
                "lang" => p.lang = value.into(),
                "theme" => p.theme = value.into(),
                "accent" => p.accent = value.into(),
                "motion" => p.motion = value.into(),
                "wideSidebar" => p.wide_sidebar = value == "1" || value == "true",
                "dateFormat" => p.date_format = value.into(),
                "hourCycle" => p.hour_cycle = value.into(),
                "weekStart" => p.week_start = value.into(),
                "sort" => p.sort = value.into(),
                "order" => p.order = value.into(),
                "gridMode" => p.grid_mode = value.into(),
                "rowHeight" => p.row_height = value.parse().unwrap_or(p.row_height),
                "density" => p.grid_zoom_desktop = value.parse().unwrap_or(p.grid_zoom_desktop),
                _ => {
                    if !p.set_by_key(key, value) {
                        log::warn!("unknown pref {key}");
                    }
                }
            }
            *p = p.clone().sanitized();
        }
        let after = self.prefs.borrow().clone();
        if after == before {
            return;
        }
        self.apply_prefs();
        self.save_prefs();
        // labels / calendar week start / dark map tiles
        crate::explore::on_prefs(self);
        if (after.sort != before.sort || after.order != before.order || after.kind != before.kind)
            && self.window.global::<Shell>().get_page() != Page::Photos
        {
            self.load_page_query();
        }
        if after.grid_mode != before.grid_mode
            || after.row_height != before.row_height
            || after.grid_zoom_desktop != before.grid_zoom_desktop
            || after.date_headers != before.date_headers
            || after.group_by != before.group_by
        {
            self.grid.0.borrow_mut().set_view(self.view_options());
        }
    }

    fn view_options(&self) -> ViewOptions {
        let p = self.prefs.borrow();
        view_options_of(&p)
    }

    fn sort(&self) -> (SortKey, SortDir) {
        let p = self.prefs.borrow();
        let k = serde_json::from_value(serde_json::Value::String(p.sort.clone()))
            .unwrap_or(SortKey::TakenAt);
        let d = if p.order == "asc" {
            SortDir::Asc
        } else {
            SortDir::Desc
        };
        (k, d)
    }

    fn refresh_grid_title(&self) {
        let gs = self.window.global::<GridState>();
        let title = match self.window.global::<Shell>().get_page() {
            Page::Favorites => self.t("nav.favorites"),
            Page::Search => format!("“{}”", self.search_q.borrow()),
            // titles of these come from pages/*.rs
            Page::Album | Page::Lib | Page::Folder => return,
            // explore pages draw their own titles
            Page::Explore
            | Page::ExploreResult
            | Page::ExplorePlaces
            | Page::ExploreDups
            | Page::ExploreMap => return,
            _ => self.t("nav2.photos"),
        };
        gs.set_title(title.into());
    }

    /// (Re)load the grid query of the current page.
    pub(crate) fn load_page_query(&self) {
        let page = self.window.global::<Shell>().get_page();
        let (sort, order) = self.sort();
        let q = match page {
            Page::Photos => BrowseQuery {
                sort: Some(SortKey::TakenAt),
                order: Some(SortDir::Desc),
                kind: Some(KindFilter::All),
                recursive: Some(true),
                ..Default::default()
            },
            Page::Favorites => BrowseQuery {
                sort: Some(sort),
                order: Some(order),
                kind: Some(KindFilter::Favorites),
                recursive: Some(true),
                ..Default::default()
            },
            Page::Search => BrowseQuery {
                sort: Some(sort),
                order: Some(order),
                kind: Some(KindFilter::All),
                q: Some(self.search_q.borrow().clone()),
                recursive: Some(true),
                ..Default::default()
            },
            // album / library / folder pages
            // explore result grids / duplicates
            _ => {
                match crate::pages::grid_query(self).or_else(|| crate::explore::grid_query(self)) {
                    Some(q) => q,
                    None => return,
                }
            }
        };
        self.refresh_grid_title();
        self.grid.0.borrow_mut().set_query(q);
    }

    pub(crate) fn navigate(self: &Rc<Self>, page: Page, push: bool) {
        let r = crate::router::Route::of_page(page, &self.search_q.borrow());
        self.go(r, push);
    }

    /// show a route (router.rs): Shell.page, back stack, the page's grid query + data
    pub(crate) fn go(self: &Rc<Self>, route: crate::router::Route, push: bool) {
        let shell = self.window.global::<Shell>();
        let cur = self.route.borrow().clone();
        if push && cur != route {
            self.history.borrow_mut().push(cur);
        }
        *self.route.borrow_mut() = route.clone();
        // a search route reached by back / links / scripts carries its own query
        if let crate::router::Route::Search(q) = &route {
            *self.search_q.borrow_mut() = q.clone();
            shell.set_search_text(q.as_str().into());
        }
        shell.set_page(route.page());
        shell.set_nav_serial(shell.get_nav_serial() + 1);
        shell.set_title_revealed(false);
        self.load_page_query();
        crate::pages::enter(self, &route);
        // explore pages / map
        crate::explore::enter(self, &route);
        crate::slideshow_ui::enter(self, &route);
    }

    /// error toast
    pub(crate) fn error_toast(self: &Rc<Self>, msg: &str) {
        self.toast(ToastKind::Error, &self.t("common.error"), msg);
    }

    pub(crate) fn search(self: &Rc<Self>, q: &str) {
        let q = q.trim();
        if q.is_empty() {
            return;
        }
        *self.search_q.borrow_mut() = q.to_string();
        let shell = self.window.global::<Shell>();
        shell.set_search_text(q.into());
        if shell.get_page() == Page::Search {
            self.load_page_query();
        } else {
            self.navigate(Page::Search, true);
        }
    }

    pub(crate) fn toast(self: &Rc<Self>, kind: ToastKind, title: &str, body: &str) {
        let id = self.toast_seq.get() + 1;
        self.toast_seq.set(id);
        self.toasts.push(ToastData {
            id,
            kind,
            title: title.into(),
            body: body.into(),
            action: SharedString::new(),
            progress: 0.0,
        });
        // at most 4 on screen
        while self.toasts.row_count() > 4 {
            self.toasts.remove(0);
        }
        let weak = Rc::downgrade(self);
        slint::Timer::single_shot(Duration::from_millis(4500), move || {
            if let Some(a) = weak.upgrade() {
                a.dismiss_toast(id);
            }
        });
    }

    pub(crate) fn dismiss_toast(&self, id: i32) {
        if let Some(pos) = self.toasts.iter().position(|t| t.id == id) {
            self.toasts.remove(pos);
        }
    }

    fn on_event(self: &Rc<Self>, e: BackendEvent) {
        let shell = self.window.global::<Shell>();
        // libraries / albums / devices / jobs → hub, pages, dialogs
        crate::pages::on_event(self, &e);
        // action conditions (server info, libraries)
        crate::actions_ui::on_event(&e);
        // explore data / map points
        crate::explore::on_event(self, &e);
        match e {
            BackendEvent::Connection(c) => {
                let was = shell.get_conn();
                shell.set_conn(match c {
                    Connection::Connecting => Conn::Connecting,
                    Connection::Online => Conn::Online,
                    Connection::Offline => Conn::Offline,
                });
                // back online after a drop: the library may have changed meanwhile
                if c == Connection::Online && was == Conn::Offline {
                    self.grid.0.borrow_mut().reload();
                }
            }
            BackendEvent::Server(ev) => match ev {
                ServerEvent::Hello { info } => {
                    shell.set_readonly(info.caps.readonly);
                    shell.set_scanning(info.scanning);
                }
                ServerEvent::Media { reason, .. } => {
                    self.grid.0.borrow_mut().on_media_event(reason)
                }
                ServerEvent::Scan {
                    scanning,
                    done,
                    total,
                } => {
                    shell.set_scanning(scanning && total > 0);
                    shell.set_scan_progress(if total > 0 {
                        done as f32 / total as f32
                    } else {
                        0.0
                    });
                    let (d, t) = (
                        i18n::format_count(self.lang(), done),
                        i18n::format_count(self.lang(), total),
                    );
                    shell.set_scan_text(
                        i18n::tv(self.lang(), "nav.scanning", &[("done", &d), ("total", &t)])
                            .into(),
                    );
                    if !scanning {
                        self.grid.0.borrow_mut().on_media_event(None);
                    }
                }
                _ => {}
            },
        }
    }
}

/// A hook that runs once the window exists (see [`Integration::setup`]).
pub type WindowSetup = Box<dyn FnOnce(&AppWindow)>;

/// How the program around the UI integrates (pa-desktop: the desktop shell).
pub struct Integration {
    /// native dialogs, LAN share, shell options … ([`crate::host::NoHost`]: none)
    pub host: Rc<dyn crate::host::Host>,
    /// show the window at start (false: tray only)
    pub show: bool,
    /// runs once the window exists, before it is shown (bounds, close handler, tray)
    pub setup: Option<WindowSetup>,
}

/// Build the window, wire everything, run the event loop until the window closes.
pub fn run(
    backend: Arc<dyn Backend>,
    rt: tokio::runtime::Handle,
    opts: UiOptions,
) -> Result<(), slint::PlatformError> {
    run_with(
        backend,
        rt,
        opts,
        Integration {
            host: Rc::new(crate::host::NoHost),
            show: true,
            setup: None,
        },
    )
}

/// [`run`] with a desktop shell; runs until `slint::quit_event_loop` (closing the window quits
/// unless `setup` installs another close handler).
pub fn run_with(
    backend: Arc<dyn Backend>,
    rt: tokio::runtime::Handle,
    opts: UiOptions,
    integ: Integration,
) -> Result<(), slint::PlatformError> {
    let window = AppWindow::new()?;
    let rt = Rt::new(rt);
    let prefs = opts
        .prefs_path
        .as_deref()
        .map(Prefs::load)
        .unwrap_or_default();
    let lang = Rc::new(Cell::new(Lang::parse(&prefs.lang)));
    install_i18n!(window, lang);

    let view = view_options_of(&prefs);
    let grid = GridHandle::new(&window, backend.clone(), rt.clone(), lang.get(), view);
    let toasts = Rc::new(VecModel::<ToastData>::default());
    let shell = window.global::<Shell>();
    shell.set_toasts(ModelRc::from(toasts.clone()));
    shell.set_server_label(backend.label().into());
    let app = Rc::new(AppInner {
        window: window.clone_strong(),
        backend: backend.clone(),
        rt: rt.clone(),
        lang,
        prefs: RefCell::new(prefs),
        prefs_path: opts.prefs_path.clone(),
        grid,
        history: RefCell::new(vec![]),
        route: RefCell::new(crate::router::Route::Photos),
        host: integ.host.clone(),
        pages: crate::pages::Pages::default(),
        toasts,
        toast_seq: Cell::new(0),
        search_q: RefCell::new(String::new()),
    });
    // first apply: force the I18n rev so every binding evaluates with the right language
    app.apply_prefs();
    window.global::<I18n>().set_rev(1);

    // ── callbacks ──
    let w = Rc::downgrade(&app);
    shell.on_navigate(move |p| {
        if let Some(a) = w.upgrade() {
            a.navigate(p, true);
        }
    });
    let w = Rc::downgrade(&app);
    shell.on_back(move || {
        if let Some(a) = w.upgrade() {
            let prev = a.history.borrow_mut().pop();
            if let Some(p) = prev {
                a.go(p, false);
            }
        }
    });
    let w = Rc::downgrade(&app);
    shell.on_search(move |q| {
        if let Some(a) = w.upgrade() {
            a.search(&q);
        }
    });
    let w = Rc::downgrade(&app);
    shell.on_dismiss_toast(move |id| {
        if let Some(a) = w.upgrade() {
            a.dismiss_toast(id);
        }
    });
    shell.on_toast_action(|_| {});

    let ps = window.global::<PrefsState>();
    let w = Rc::downgrade(&app);
    ps.on_set(move |k, v| {
        if let Some(a) = w.upgrade() {
            a.set_pref(&k, &v);
        }
    });
    let w = Rc::downgrade(&app);
    ps.on_set_accent(move |c| {
        if let Some(a) = w.upgrade() {
            let c = c.to_argb_u8();
            a.set_pref("accent", &theme::to_hex((c.red, c.green, c.blue)));
        }
    });

    let gs = window.global::<GridState>();
    let w = Rc::downgrade(&app);
    gs.on_viewport_changed(move |y, h, width| {
        if let Some(a) = w.upgrade() {
            let scale = a.window.window().scale_factor();
            a.grid
                .0
                .borrow_mut()
                .on_viewport(y as f64, h as f64, width as f64, scale);
        }
    });
    let w = Rc::downgrade(&app);
    gs.on_view_changed(move || {
        if let Some(a) = w.upgrade() {
            let gs = a.window.global::<GridState>();
            a.set_pref("density", &gs.get_density().to_string());
            a.set_pref(
                "gridMode",
                if gs.get_square() {
                    "square"
                } else {
                    "justified"
                },
            );
            a.set_pref("rowHeight", &gs.get_row_height().round().to_string());
        }
    });
    let w = Rc::downgrade(&app);
    gs.on_open(move |i| {
        if w.upgrade().is_some() {
            crate::actions_ui::open_viewer(i.max(0) as usize);
        }
    });
    let w = Rc::downgrade(&app);
    gs.on_retry(move || {
        if let Some(a) = w.upgrade() {
            a.grid.0.borrow_mut().reload();
        }
    });
    let w = Rc::downgrade(&app);
    window.on_slideshow(move || {
        if let Some(a) = w.upgrade() {
            crate::slideshow_ui::open_from_grid(&a);
        }
    });

    // ── live events → UI thread ──
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<BackendEvent>();
    backend.subscribe(Box::new(move |e| {
        let _ = tx.send(e);
    }));
    let w = Rc::downgrade(&app);
    let _ = slint::spawn_local(async move {
        while let Some(e) = rx.recv().await {
            match w.upgrade() {
                Some(a) => a.on_event(e),
                None => break,
            }
        }
    });

    // server info (read-only flag, label)
    let w = Rc::downgrade(&app);
    rt.spawn_then(backend.info(), move |r| {
        let Some(a) = w.upgrade() else { return };
        match r {
            Ok(info) => {
                let shell = a.window.global::<Shell>();
                shell.set_readonly(info.caps.readonly);
                shell.set_scanning(info.scanning);
            }
            Err(e) => {
                a.toast(ToastKind::Error, &a.t("common.error"), &e.message);
            }
        }
    });

    // hub / albums / library pages / dialogs / settings callbacks
    crate::pages::install(&app);
    // viewer, selection / actions, drag & drop, fast scroller
    crate::actions_ui::install(&app);
    // explore pages, memory player, map, mini map
    crate::explore::install(&app);
    crate::slideshow_ui::install(&app);
    crate::editor::install(&app);

    app.navigate(Page::Photos, false);

    let frames = perf::install_frame_stats(&window, opts.frame_log);
    if let Some(s) = opts.script.clone() {
        perf::run_script(&app, &s);
    }
    if let Some(auto) = opts.autoscroll {
        perf::autoscroll(&app, auto, frames.clone());
    }
    window.window().on_close_requested(|| {
        let _ = slint::quit_event_loop();
        slint::CloseRequestResponse::HideWindow
    });
    if let Some(setup) = integ.setup {
        setup(&window);
    }
    let r = if integ.show { window.show() } else { Ok(()) }
        .and_then(|_| slint::run_event_loop_until_quit());
    let _ = window.hide();
    crate::explore_player::shutdown();
    crate::viewer::shutdown();
    crate::slideshow_ui::shutdown();
    perf::report(&frames, &app.grid.0.borrow());
    drop(app);
    r
}

/// The component gallery window (dev aid / screenshots): `script` as for [`UiOptions::script`].
pub fn run_gallery(
    theme_mode: &str,
    lang: &str,
    script: Option<String>,
) -> Result<(), slint::PlatformError> {
    let g = crate::ComponentGallery::new()?;
    let l = Rc::new(Cell::new(Lang::parse(lang)));
    install_i18n!(g, l);
    g.global::<I18n>().set_rev(1);
    g.global::<Theme>().set_mode(if theme_mode == "light" {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    });
    if let Some(s) = script {
        perf::run_window_script(g.as_weak(), &s);
    }
    g.run()
}

/// the grid's view options from the prefs (density, tiles, date headers / grouping)
fn view_options_of(p: &Prefs) -> ViewOptions {
    ViewOptions {
        density: p.grid_zoom_desktop,
        square: p.grid_mode == "square",
        row_height: p.row_height,
        headers: p.date_headers,
        group: match p.group_by.as_str() {
            "day" => Some(crate::grid::layout::GroupBy::Day),
            "month" => Some(crate::grid::layout::GroupBy::Month),
            _ => None,
        },
    }
}
