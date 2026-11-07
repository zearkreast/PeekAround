//! The photo map (MapPage.tsx) as a custom slippy map, and the viewer's mini map (MiniMap.tsx).
//!
//! - tiles: `/api/tiles/<style>/<z>/<x>/<y>.png` through the server's proxy + cache (Settings
//!   mapTiles osm / topo / satellite, `off` hides them), decoded off the UI thread; osm / topo get
//!   the web UI's dark-theme CSS filter (`invert hue-rotate(180) brightness .95 contrast .9
//!   saturate .6`) applied to the pixels; an LRU of decoded tiles, a few requests at a time
//!   (centre first), ancestor tiles stand in while zooming, new tiles fade in; retina screens get
//!   the next zoom level's tiles at half size.
//! - view: drag pan with inertia, wheel / pinch zoom around the pointer (animated), double click,
//!   +/− buttons, keys, fit bounds, fly to a cluster.
//! - markers: `/api/geo/points` clustered with the supercluster port (`map_cluster.rs`, radius 72,
//!   maxZoom 17): cover thumbnail + count badge; click a cluster → zoom in (or the newest of
//!   identical locations), a single → viewer.
//! - side panel "photos in this area": the points in view, newest first, justified grid.
//! - focus mode (`/explore/map?focus=<id>&lat=&lon=`, "Show on map" from the viewer): centred at
//!   zoom 16 with a pulsing ring.
//! - problem banner: offline / refused (HTTP status) / tiles off, with retry.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use pa_core::{BrowseQuery, GeoPoints, MapTileStyle, MediaItem, MediaKind};
use slint::{ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel};

use crate::app::AppInner;
use crate::explore_labels as lb;
use crate::map_cluster::{lat_y, lng_x, Feature, Options, Supercluster};
use crate::map_math::{self, TileProblem, View};
use crate::router::Route;
use crate::{MapMarker, MapPanelTile, MapState, MapTile, MiniMapState, MiniTile};

const MIN_ZOOM: f64 = 2.0;
const MAX_PARALLEL: usize = 6;
const CACHE_TILES: usize = 320;
const CLUSTER_MAX_ZOOM: i32 = 17;

/// (style, dark, z, x, y)
type TileKey = (MapTileStyle, bool, u32, i64, i64);

/// CSS `invert(1) hue-rotate(180deg) brightness(0.95) contrast(0.9) saturate(0.6)` (sRGB, clamped
/// after every step like the browsers' shorthand filters)
pub fn dark_filter(px: &mut [u8]) {
    // hue-rotate(180deg): the Filter Effects matrix with cos = -1, sin = 0
    const H: [[f32; 3]; 3] = [
        [0.213 - 0.787, 0.715 + 0.715, 0.072 + 0.072],
        [0.213 + 0.213, 0.715 - 0.285, 0.072 + 0.072],
        [0.213 + 0.213, 0.715 + 0.715, 0.072 - 0.928],
    ];
    const S: f32 = 0.6;
    const M: [[f32; 3]; 3] = [
        [0.213 + 0.787 * S, 0.715 - 0.715 * S, 0.072 - 0.072 * S],
        [0.213 - 0.213 * S, 0.715 + 0.285 * S, 0.072 - 0.072 * S],
        [0.213 - 0.213 * S, 0.715 - 0.715 * S, 0.072 + 0.928 * S],
    ];
    let cl = |v: f32| v.clamp(0.0, 1.0);
    for p in px.chunks_exact_mut(4) {
        let c = [
            1.0 - p[0] as f32 / 255.0,
            1.0 - p[1] as f32 / 255.0,
            1.0 - p[2] as f32 / 255.0,
        ];
        let mut h = [0f32; 3];
        for (i, row) in H.iter().enumerate() {
            h[i] = cl(row[0] * c[0] + row[1] * c[1] + row[2] * c[2]);
        }
        for v in h.iter_mut() {
            *v = cl(*v * 0.95);
            *v = cl((*v - 0.5) * 0.9 + 0.5);
        }
        for (i, row) in M.iter().enumerate() {
            p[i] = (cl(row[0] * h[0] + row[1] * h[1] + row[2] * h[2]) * 255.0 + 0.5) as u8;
        }
    }
}

fn decode_tile(bytes: &[u8], dark: bool) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let img = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    let mut raw = img.into_raw();
    if dark {
        dark_filter(&mut raw);
    }
    Some(SharedPixelBuffer::clone_from_slice(&raw, w, h))
}

fn problem_of(e: &crate::backend::BackendError) -> TileProblem {
    // the proxy's message carries the upstream status: "Tile server refused (HTTP 403)"
    let up = e
        .message
        .rsplit("HTTP ")
        .next()
        .and_then(|s| s.trim_end_matches(')').trim().parse::<u16>().ok());
    map_math::tile_problem_of_status(e.status, up)
}

#[derive(Debug, Clone, Copy)]
enum Anim {
    /// fly from → to (ease in-out); `anchor`: keep this screen point fixed while zooming
    Fly {
        from: View,
        to: View,
        t0: Instant,
        ms: f64,
        anchor: Option<(f64, f64)>,
    },
    /// wheel: chase a target zoom around a screen point
    Wheel { zoom: f64, sx: f64, sy: f64 },
    /// inertia (px / ms)
    Glide { vx: f64, vy: f64 },
}

#[derive(Debug, Clone)]
struct DragSt {
    x: f64,
    y: f64,
    x0: f64,
    y0: f64,
    moved: bool,
    samples: Vec<(Instant, f64, f64)>,
}

#[derive(Debug, Clone)]
struct MarkerInfo {
    key: String,
    feature: Feature,
    mx: f64,
    my: f64,
    cover: String,
    count: usize,
}

/// The tile service of the map and the mini map (UI thread).
struct Tiles {
    cache: lru::LruCache<TileKey, Image>,
    inflight: HashSet<TileKey>,
    failed: HashSet<TileKey>,
    ok: u32,
    bad: u32,
    problem: Option<TileProblem>,
    /// tiles that arrive → who wants to know
    epoch: u64,
}

struct MapCtl {
    app: Weak<AppInner>,
    me: Weak<MapCtl>,
    // data
    place: RefCell<String>,
    points: RefCell<Vec<(String, f64, f64)>>,
    sc: RefCell<Supercluster>,
    items: RefCell<HashMap<String, MediaItem>>,
    points_loading: Cell<bool>,
    items_loading: Cell<bool>,
    data_epoch: Cell<u64>,
    style: Cell<Option<MapTileStyle>>,
    settings_loaded: Cell<bool>,
    // view
    view: Cell<View>,
    size: Cell<(f64, f64)>,
    anim: Cell<Option<Anim>>,
    drag: RefCell<Option<DragSt>>,
    pinch: Cell<Option<(View, f64, f64)>>,
    last_tick: Cell<Instant>,
    timer: slint::Timer,
    settle: slint::Timer,
    fitted: Cell<bool>,
    /// the view is still the automatic one (fit / focus): re-fit when the map resizes (side panel)
    auto: Cell<bool>,
    focus: RefCell<Option<(String, f64, f64)>>,
    // tiles
    tiles: RefCell<Tiles>,
    front: Rc<VecModel<MapTile>>,
    back: Rc<VecModel<MapTile>>,
    front_keys: RefCell<Vec<Option<TileKey>>>,
    back_keys: RefCell<Vec<Option<TileKey>>>,
    // markers
    markers: Rc<VecModel<MapMarker>>,
    marker_keys: RefCell<Vec<Option<String>>>,
    marker_info: RefCell<Vec<Option<MarkerInfo>>>,
    cluster_zoom: Cell<i32>,
    hovered: Cell<Option<usize>>,
    // panel
    in_view: RefCell<Vec<MediaItem>>,
    panel_layout: RefCell<Option<crate::grid::layout::GridLayout>>,
    panel_model: Rc<VecModel<MapPanelTile>>,
    panel_slots: RefCell<Vec<Option<usize>>>,
    panel_vp: Cell<(f64, f64, f64)>,
    // perf
    frames: RefCell<Vec<f64>>,
    last_frame: Cell<Option<Instant>>,
    // mini map
    mini: RefCell<Option<(f64, f64, String)>>,
    mini_model: Rc<VecModel<MiniTile>>,
}

thread_local! {
    static MAP: RefCell<Option<Rc<MapCtl>>> = const { RefCell::new(None) };
}

fn get() -> Option<Rc<MapCtl>> {
    MAP.with(|m| m.borrow().clone())
}

fn ease_in_out(t: f64) -> f64 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

impl MapCtl {
    fn app(&self) -> Option<Rc<AppInner>> {
        self.app.upgrade()
    }

    fn motion(&self) -> i32 {
        self.app()
            .map(|a| a.window.global::<crate::Motion>().get_level())
            .unwrap_or(2)
    }

    fn dark(&self) -> bool {
        self.app()
            .map(|a| a.window.global::<crate::Theme>().get_dark())
            .unwrap_or(true)
    }

    fn on_route(&self) -> bool {
        self.app()
            .is_some_and(|a| matches!(&*a.route.borrow(), Route::ExploreMap { .. }))
    }

    fn max_zoom(&self) -> f64 {
        match self.style.get() {
            Some(s) => (pa_core::tile_style(s).max_zoom as f64).max(MIN_ZOOM),
            None => 18.0,
        }
    }

    fn t(&self, k: &str) -> String {
        self.app().map(|a| a.t(k)).unwrap_or_default()
    }

    // ───────── data ─────────

    fn query(&self) -> BrowseQuery {
        let p = self.place.borrow();
        BrowseQuery {
            place: (!p.is_empty()).then(|| p.clone()),
            ..lb::explore_query()
        }
    }

    fn load_settings(self: &Rc<Self>) {
        let Some(a) = self.app() else { return };
        let w = self.me.clone();
        crate::api::call::<pa_core::Settings>(&a, "GET", "/api/settings", None, move |_, r| {
            let Some(me) = w.upgrade() else { return };
            if let Ok(s) = r {
                me.set_style(s.map_tiles.style());
            }
            me.settings_loaded.set(true);
            me.publish_chrome();
            me.render();
            me.refresh_mini();
        });
    }

    fn set_style(&self, s: Option<MapTileStyle>) {
        if self.style.get() != s {
            self.style.set(s);
            let mut t = self.tiles.borrow_mut();
            t.problem = None;
            t.ok = 0;
            t.bad = 0;
            t.failed.clear();
        }
    }

    fn load_points(self: &Rc<Self>) {
        let Some(a) = self.app() else { return };
        self.data_epoch.set(self.data_epoch.get() + 1);
        let epoch = self.data_epoch.get();
        self.points_loading.set(true);
        self.publish_chrome();
        let q = self.query();
        let path = format!("/api/geo/points{}", pa_client_qs(&q));
        let w = self.me.clone();
        crate::api::call::<GeoPoints>(&a, "GET", &path, None, move |a, r| {
            let Some(me) = w.upgrade() else { return };
            if me.data_epoch.get() != epoch {
                return;
            }
            me.points_loading.set(false);
            let pts: Vec<(String, f64, f64)> = match r {
                Ok(p) => p.points.into_iter().map(|p| (p.0, p.1, p.2)).collect(),
                Err(e) => {
                    a.error_toast(&e.message);
                    vec![]
                }
            };
            let mut sc = Supercluster::new(Options {
                radius: 72.0,
                max_zoom: CLUSTER_MAX_ZOOM,
                ..Default::default()
            });
            sc.load(pts.iter().map(|p| (p.2, p.1)).collect());
            *me.sc.borrow_mut() = sc;
            *me.points.borrow_mut() = pts;
            me.cluster_zoom.set(-1);
            me.clear_markers();
            if !me.points.borrow().is_empty() {
                me.load_items(epoch);
            }
            me.initial_view();
            me.publish_chrome();
            me.render();
            me.settled();
        });
    }

    /// the media items behind the points (panel + viewer), paged like the web UI
    fn load_items(self: &Rc<Self>, epoch: u64) {
        let Some(a) = self.app() else { return };
        self.items_loading.set(true);
        let want: HashSet<String> = self.points.borrow().iter().map(|p| p.0.clone()).collect();
        let backend = a.backend.clone();
        let q = self.query();
        let fut = async move {
            let mut out = HashMap::new();
            const PAGE: i64 = 2000;
            let mut offset = 0;
            loop {
                let r = backend
                    .browse(BrowseQuery {
                        offset: Some(offset),
                        limit: Some(PAGE),
                        ..q.clone()
                    })
                    .await?;
                let n = r.items.len() as i64;
                for it in r.items {
                    if want.contains(&it.id) {
                        out.insert(it.id.clone(), it);
                    }
                }
                if out.len() >= want.len() || n < PAGE || offset + PAGE >= r.total {
                    break;
                }
                offset += PAGE;
            }
            Ok::<_, crate::backend::BackendError>(out)
        };
        let w = self.me.clone();
        a.rt.spawn_then(fut, move |r| {
            let Some(me) = w.upgrade() else { return };
            if me.data_epoch.get() != epoch {
                return;
            }
            me.items_loading.set(false);
            if let Ok(m) = r {
                *me.items.borrow_mut() = m;
            }
            me.publish_chrome();
            me.in_view.borrow_mut().clear();
            me.settled();
        });
    }

    /// first view: the focused item, else fit all points
    fn initial_view(&self) {
        let (w, h) = self.size.get();
        if w <= 0.0 {
            return;
        }
        if self.fitted.get() {
            return;
        }
        if let Some((_, lat, lon)) = self.focus.borrow().clone() {
            let mut v = View::at(lat, lon, 16.0f64.min(self.max_zoom()));
            v.clamp(h);
            self.view.set(v);
            self.auto.set(true);
            self.fitted
                .set(!self.points.borrow().is_empty() || !self.points_loading.get());
            return;
        }
        if self.points.borrow().is_empty() {
            return;
        }
        self.fitted.set(true);
        self.auto.set(true);
        self.view.set(self.fit_view());
    }

    fn fit_view(&self) -> View {
        let (w, h) = self.size.get();
        let pts = self.points.borrow();
        let (mut s, mut we, mut n, mut e) = (90.0f64, 180.0f64, -90.0f64, -180.0f64);
        for (_, lat, lon) in pts.iter() {
            s = s.min(*lat);
            n = n.max(*lat);
            we = we.min(*lon);
            e = e.max(*lon);
        }
        if pts.is_empty() {
            return View::at(30.0, 15.0, 2.0);
        }
        let mut v = map_math::fit([s, we, n, e], w.max(1.0), h.max(1.0), 56.0, 13.0, 0.5);
        v.zoom = v.zoom.max(MIN_ZOOM);
        v.clamp(h);
        v
    }

    // ───────── view ─────────

    fn set_view(&self, mut v: View) {
        let (_, h) = self.size.get();
        v.zoom = v.zoom.clamp(MIN_ZOOM, self.max_zoom());
        v.clamp(h);
        self.view.set(v);
    }

    fn animate(self: &Rc<Self>, a: Anim) {
        if self.motion() == 0 {
            // animations off: jump to the end
            match a {
                Anim::Fly { to, anchor, .. } => match anchor {
                    Some((sx, sy)) => {
                        let (w, h) = self.size.get();
                        let mut v = self.view.get();
                        v.zoom_around(to.zoom, sx, sy, w, h);
                        self.set_view(v);
                    }
                    None => self.set_view(to),
                },
                Anim::Wheel { zoom, sx, sy } => {
                    let (w, h) = self.size.get();
                    let mut v = self.view.get();
                    v.zoom_around(zoom, sx, sy, w, h);
                    self.set_view(v);
                }
                Anim::Glide { .. } => {}
            }
            self.anim.set(None);
            self.render();
            self.settle_soon();
            return;
        }
        self.anim.set(Some(a));
        self.start_ticks();
    }

    fn start_ticks(self: &Rc<Self>) {
        if self.timer.running() {
            return;
        }
        self.last_tick.set(Instant::now());
        let w = self.me.clone();
        self.timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(16),
            move || {
                if let Some(me) = w.upgrade() {
                    me.tick();
                }
            },
        );
    }

    fn tick(self: &Rc<Self>) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick.get()).as_secs_f64() * 1000.0;
        self.last_tick.set(now);
        let (w, h) = self.size.get();
        let mut v = self.view.get();
        let mut done = true;
        match self.anim.get() {
            Some(Anim::Fly {
                from,
                to,
                t0,
                ms,
                anchor,
            }) => {
                let t = (now.duration_since(t0).as_secs_f64() * 1000.0 / ms).min(1.0);
                let k = ease_in_out(t);
                match anchor {
                    Some((sx, sy)) => {
                        let z = from.zoom + (to.zoom - from.zoom) * k;
                        v.zoom_around(z, sx, sy, w, h);
                    }
                    None => {
                        // zoom moves in log space, the centre in screen space of the zoom
                        let mut dx = to.cx - from.cx;
                        dx -= dx.round();
                        v.cx = from.cx + dx * k;
                        v.cy = from.cy + (to.cy - from.cy) * k;
                        v.zoom = from.zoom + (to.zoom - from.zoom) * k;
                    }
                }
                done = t >= 1.0;
            }
            Some(Anim::Wheel { zoom, sx, sy }) => {
                let k = 1.0 - (-dt / 70.0).exp();
                let z = v.zoom + (zoom - v.zoom) * k;
                let z = if (zoom - z).abs() < 0.002 { zoom } else { z };
                v.zoom_around(z, sx, sy, w, h);
                done = z == zoom;
            }
            Some(Anim::Glide { vx, vy }) => {
                v.pan(vx * dt, vy * dt);
                // Leaflet: inertiaDeceleration 3400 px/s²
                let sp = (vx * vx + vy * vy).sqrt();
                let ns = (sp - 3.4e-3 * dt).max(0.0);
                if ns > 0.02 {
                    let f = ns / sp;
                    self.anim.set(Some(Anim::Glide {
                        vx: vx * f,
                        vy: vy * f,
                    }));
                    done = false;
                }
            }
            None => {}
        }
        self.set_view(v);
        if done {
            self.anim.set(None);
        }
        self.render();
        if done && self.drag.borrow().is_none() {
            // keep ticking a little for the tile fade-ins, then settle
            self.timer.stop();
            self.settle_soon();
        }
    }

    fn settle_soon(self: &Rc<Self>) {
        let w = self.me.clone();
        self.settle.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(60),
            move || {
                if let Some(me) = w.upgrade() {
                    me.settled();
                }
            },
        );
    }

    /// `moveend`: recluster + the panel list
    fn settled(self: &Rc<Self>) {
        self.cluster_zoom.set(-1);
        self.render();
        self.update_in_view();
    }

    fn zoom_by(self: &Rc<Self>, d: f64) {
        self.auto.set(false);
        let (w, h) = self.size.get();
        let cur = match self.anim.get() {
            Some(Anim::Wheel { zoom, .. }) => zoom,
            Some(Anim::Fly { to, .. }) => to.zoom,
            _ => self.view.get().zoom,
        };
        let z = ((cur + d) * 2.0).round() / 2.0;
        let to = View {
            zoom: z.clamp(MIN_ZOOM, self.max_zoom()),
            ..self.view.get()
        };
        self.animate(Anim::Fly {
            from: self.view.get(),
            to,
            t0: Instant::now(),
            ms: 250.0,
            anchor: Some((w / 2.0, h / 2.0)),
        });
    }

    fn fly_to(self: &Rc<Self>, lat: f64, lon: f64, zoom: f64) {
        let to = View::at(lat, lon, zoom.clamp(MIN_ZOOM, self.max_zoom()));
        self.animate(Anim::Fly {
            from: self.view.get(),
            to,
            t0: Instant::now(),
            ms: 600.0,
            anchor: None,
        });
    }

    fn fit(self: &Rc<Self>) {
        let to = self.fit_view();
        self.animate(Anim::Fly {
            from: self.view.get(),
            to,
            t0: Instant::now(),
            ms: 450.0,
            anchor: None,
        });
    }

    // ───────── input ─────────

    fn press(&self, x: f64, y: f64) {
        self.anim.set(None);
        self.auto.set(false);
        *self.drag.borrow_mut() = Some(DragSt {
            x,
            y,
            x0: x,
            y0: y,
            moved: false,
            samples: vec![(Instant::now(), x, y)],
        });
    }

    fn drag_to(self: &Rc<Self>, x: f64, y: f64) {
        if self.pinch.get().is_some() {
            return;
        }
        let (dx, dy) = {
            let mut d = self.drag.borrow_mut();
            let Some(d) = d.as_mut() else { return };
            let (dx, dy) = (x - d.x, y - d.y);
            d.x = x;
            d.y = y;
            if (x - d.x0).abs() > 3.0 || (y - d.y0).abs() > 3.0 {
                d.moved = true;
            }
            let now = Instant::now();
            d.samples.push((now, x, y));
            d.samples
                .retain(|s| now.duration_since(s.0) < Duration::from_millis(100));
            (dx, dy)
        };
        if let Some(a) = self.app() {
            a.window.global::<MapState>().set_grabbing(true);
        }
        let mut v = self.view.get();
        v.pan(dx, dy);
        self.set_view(v);
        self.render();
    }

    fn release(self: &Rc<Self>, x: f64, y: f64) {
        if let Some(a) = self.app() {
            a.window.global::<MapState>().set_grabbing(false);
        }
        let Some(d) = self.drag.borrow_mut().take() else {
            return;
        };
        if !d.moved {
            self.click(x, y);
            return;
        }
        // inertia from the last 100 ms
        let now = Instant::now();
        let first = d.samples.first().copied();
        if let (Some(f), true) = (first, self.motion() > 0) {
            let ms = now.duration_since(f.0).as_secs_f64() * 1000.0;
            if ms > 5.0 {
                let (mut vx, mut vy) = ((x - f.1) / ms, (y - f.2) / ms);
                let sp = (vx * vx + vy * vy).sqrt();
                // Leaflet maxSpeed 1500 px/s
                if sp > 1.5 {
                    vx *= 1.5 / sp;
                    vy *= 1.5 / sp;
                }
                if sp > 0.1 {
                    self.animate(Anim::Glide { vx, vy });
                    return;
                }
            }
        }
        self.settled();
    }

    fn marker_at(&self, x: f64, y: f64) -> Option<usize> {
        let info = self.marker_info.borrow();
        let v = self.view.get();
        let (w, h) = self.size.get();
        // topmost first (later slots draw on top)
        let mut best = None;
        for (i, m) in info.iter().enumerate() {
            let Some(m) = m else { continue };
            let size = if m.count > 1 { 62.0 } else { 54.0 };
            let (sx, sy) = v.to_screen(m.mx, m.my, w, h);
            let (l, t) = (sx - size / 2.0, sy - size - 7.0);
            if x >= l && x <= l + size && y >= t && y <= sy {
                best = Some(i);
            }
        }
        best
    }

    fn click(self: &Rc<Self>, x: f64, y: f64) {
        let Some(i) = self.marker_at(x, y) else {
            return;
        };
        let m = self.marker_info.borrow()[i].clone();
        let Some(m) = m else { return };
        match m.feature {
            Feature::Point(pi) => {
                let id = self.points.borrow().get(pi).map(|p| p.0.clone());
                if let Some(id) = id {
                    self.open_item(&id);
                }
            }
            Feature::Cluster { id, lat, lon, .. } => {
                let z = self.sc.borrow().get_cluster_expansion_zoom(id);
                if z > CLUSTER_MAX_ZOOM || self.view.get().zoom >= CLUSTER_MAX_ZOOM as f64 {
                    // identical locations: open the newest of them
                    let leaf = self.sc.borrow().get_leaves(id, 1, 0).first().copied();
                    let pid = leaf.and_then(|l| self.points.borrow().get(l).map(|p| p.0.clone()));
                    if let Some(pid) = pid {
                        self.open_item(&pid);
                    }
                    return;
                }
                self.fly_to(lat, lon, (z as f64 + 0.5).min(18.0));
            }
        }
    }

    fn open_item(&self, id: &str) {
        let it = self.items.borrow().get(id).cloned();
        match it {
            Some(it) => crate::actions_ui::open_viewer_item(it),
            None => {
                // not loaded yet: fetch it
                let Some(a) = self.app() else { return };
                crate::api::call::<pa_core::MediaDetails>(
                    &a,
                    "GET",
                    &format!("/api/media/{}", crate::api::enc(id)),
                    None,
                    |_, r| {
                        if let Ok(d) = r {
                            crate::actions_ui::open_viewer_item(d.item);
                        }
                    },
                );
            }
        }
    }

    fn hover(&self, x: f64, y: f64) {
        let h = self.marker_at(x, y);
        if h == self.hovered.get() {
            return;
        }
        for i in [self.hovered.get(), h].into_iter().flatten() {
            if let Some(mut row) = self.markers.row_data(i) {
                row.hovered = Some(i) == h;
                self.markers.set_row_data(i, row);
            }
        }
        self.hovered.set(h);
    }

    fn wheel(self: &Rc<Self>, x: f64, y: f64, dy: f64) {
        if dy == 0.0 {
            return;
        }
        self.auto.set(false);
        let cur = match self.anim.get() {
            Some(Anim::Wheel { zoom, .. }) => zoom,
            _ => self.view.get().zoom,
        };
        // a mouse notch (≥ 50 px) = one zoom level, snapped to halves; touchpads glide
        let mut z = cur + dy / 60.0 * if dy.abs() >= 50.0 { 1.0 } else { 0.6 };
        if dy.abs() >= 50.0 {
            z = (z * 2.0).round() / 2.0;
        }
        let z = z.clamp(MIN_ZOOM, self.max_zoom());
        self.animate(Anim::Wheel {
            zoom: z,
            sx: x,
            sy: y,
        });
    }

    // ───────── rendering ─────────

    fn render(self: &Rc<Self>) {
        let Some(a) = self.app() else { return };
        if !self.on_route() {
            return;
        }
        let t0 = Instant::now();
        if let Some(l) = self.last_frame.replace(Some(t0)) {
            let gap = t0.duration_since(l).as_secs_f64() * 1000.0;
            if gap < 100.0 {
                let mut f = self.frames.borrow_mut();
                if f.len() < 100_000 {
                    f.push(gap);
                }
            }
        }
        let (w, h) = self.size.get();
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let v = self.view.get();
        self.render_tiles(&a, &v, w, h);
        self.render_markers(&a, &v, w, h);
        let st = a.window.global::<MapState>();
        match self.focus.borrow().as_ref() {
            Some((_, lat, lon)) => {
                let (sx, sy) = v.to_screen(lng_x(*lon), lat_y(*lat), w, h);
                st.set_focus_on(true);
                st.set_focus_x(sx as f32);
                st.set_focus_y(sy as f32);
            }
            None => st.set_focus_on(false),
        }
    }

    fn render_tiles(self: &Rc<Self>, a: &Rc<AppInner>, v: &View, w: f64, h: f64) {
        let st = a.window.global::<MapState>();
        let Some(style) = self.style.get() else {
            st.set_tiles_on(false);
            self.assign(&self.front, &self.front_keys, vec![]);
            self.assign(&self.back, &self.back_keys, vec![]);
            return;
        };
        st.set_tiles_on(true);
        let info = pa_core::tile_style(style);
        let dark = info.dark_filter && self.dark();
        let scale = a.window.window().scale_factor() as f64;
        let bump = if scale >= 1.5 { 1.0 } else { 0.0 };
        let z = map_math::tile_zoom(v.zoom + bump, info.max_zoom);
        let spots = map_math::cover(v, w, h, z, 0);
        let mut front = vec![];
        let mut back = vec![];
        let mut want = vec![];
        {
            let mut t = self.tiles.borrow_mut();
            for s in &spots {
                let key = (style, dark, s.z, s.x, s.y);
                // a slot per world copy (x unwrapped), the cache per tile
                let slot = (style, dark, s.z, s.ux, s.y);
                let base = MapTile {
                    x: s.sx as f32,
                    y: s.sy as f32,
                    size: s.size as f32 + 0.5,
                    visible: true,
                    clip_x: 0,
                    clip_y: 0,
                    clip_w: 256,
                    clip_h: 256,
                    ..Default::default()
                };
                if let Some(img) = t.cache.get(&key) {
                    front.push((
                        slot,
                        MapTile {
                            image: img.clone(),
                            loaded: true,
                            ..base.clone()
                        },
                    ));
                    continue;
                }
                if !t.failed.contains(&key) {
                    want.push(key);
                }
                // a placeholder: front slot (fades in when it arrives) + an ancestor behind
                front.push((slot, base.clone()));
                for dz in 1..=5u32 {
                    if dz > s.z {
                        break;
                    }
                    let pk = (style, dark, s.z - dz, s.x >> dz, s.y >> dz);
                    if let Some(img) = t.cache.peek(&pk) {
                        let sub = 256 >> dz;
                        let (ox, oy) =
                            ((s.x & ((1 << dz) - 1)) * sub, (s.y & ((1 << dz) - 1)) * sub);
                        back.push((
                            (style, dark, 100 + s.z, s.ux, s.y),
                            MapTile {
                                image: img.clone(),
                                back: true,
                                clip_x: ox as i32,
                                clip_y: oy as i32,
                                clip_w: sub as i32,
                                clip_h: sub as i32,
                                ..base.clone()
                            },
                        ));
                        break;
                    }
                }
            }
        }
        self.assign(&self.front, &self.front_keys, front);
        self.assign(&self.back, &self.back_keys, back);
        for k in want {
            self.request(k);
        }
    }

    /// keep a slot per key (fade-ins run once per tile); unused slots hide
    fn assign<K: PartialEq + Clone>(
        &self,
        model: &Rc<VecModel<MapTile>>,
        keys: &RefCell<Vec<Option<K>>>,
        rows: Vec<(K, MapTile)>,
    ) {
        let mut keys = keys.borrow_mut();
        let mut used = vec![false; keys.len()];
        let mut pending = vec![];
        for (k, t) in rows {
            if let Some(i) = keys.iter().position(|x| x.as_ref() == Some(&k)) {
                used[i] = true;
                if model.row_data(i).as_ref() != Some(&t) {
                    model.set_row_data(i, t);
                }
            } else {
                pending.push((k, t));
            }
        }
        for (k, t) in pending {
            match used.iter().position(|u| !u) {
                Some(i) => {
                    used[i] = true;
                    keys[i] = Some(k);
                    model.set_row_data(i, t);
                }
                None => {
                    keys.push(Some(k));
                    used.push(true);
                    model.push(t);
                }
            }
        }
        for (i, u) in used.iter().enumerate() {
            if !u {
                keys[i] = None;
                if let Some(mut r) = model.row_data(i) {
                    if r.visible || r.loaded {
                        r.visible = false;
                        r.loaded = false;
                        model.set_row_data(i, r);
                    }
                }
            }
        }
    }

    fn request(self: &Rc<Self>, key: TileKey) {
        let Some(a) = self.app() else { return };
        {
            let mut t = self.tiles.borrow_mut();
            if t.inflight.contains(&key)
                || t.inflight.len() >= MAX_PARALLEL
                || t.cache.contains(&key)
            {
                return;
            }
            t.inflight.insert(key);
        }
        let (style, dark, z, x, y) = key;
        let path = format!("/api/tiles/{}/{z}/{x}/{y}.png", style.as_str());
        let fut = a.backend.get_bytes(&path);
        let w = self.me.clone();
        a.rt.spawn_then(
            async move {
                match fut.await {
                    Ok(b) => tokio::task::spawn_blocking(move || decode_tile(&b, dark))
                        .await
                        .ok()
                        .flatten()
                        .ok_or(TileProblem::Unknown),
                    Err(e) => Err(problem_of(&e)),
                }
            },
            move |r| {
                let Some(me) = w.upgrade() else { return };
                let mut t = me.tiles.borrow_mut();
                t.inflight.remove(&key);
                match r {
                    Ok(px) => {
                        t.cache.put(key, Image::from_rgba8(px));
                        t.ok += 1;
                        t.bad = 0;
                        t.problem = None;
                        t.epoch += 1;
                    }
                    Err(p) => {
                        t.failed.insert(key);
                        t.bad += 1;
                        if t.bad >= 2 {
                            t.problem = Some(p);
                        }
                    }
                }
                drop(t);
                me.publish_chrome();
                if me.on_route() {
                    me.render();
                    // the fade-in needs frames only from the UI; nothing to tick here
                }
                me.mini_tile_arrived(key);
            },
        );
    }

    fn clear_markers(&self) {
        let n = self.markers.row_count();
        for i in 0..n {
            if let Some(mut r) = self.markers.row_data(i) {
                r.visible = false;
                self.markers.set_row_data(i, r);
            }
        }
        for k in self.marker_keys.borrow_mut().iter_mut() {
            *k = None;
        }
        for m in self.marker_info.borrow_mut().iter_mut() {
            *m = None;
        }
    }

    fn render_markers(self: &Rc<Self>, a: &Rc<AppInner>, v: &View, w: f64, h: f64) {
        let zr = v.zoom.round() as i32;
        if zr != self.cluster_zoom.get() {
            self.cluster_zoom.set(zr);
            self.recluster(a, v, w, h);
        }
        // move the markers with the map
        let info = self.marker_info.borrow();
        for (i, m) in info.iter().enumerate() {
            let Some(m) = m else { continue };
            let (sx, sy) = v.to_screen(m.mx, m.my, w, h);
            if let Some(mut r) = self.markers.row_data(i) {
                if r.x != sx as f32 || r.y != sy as f32 {
                    r.x = sx as f32;
                    r.y = sy as f32;
                    self.markers.set_row_data(i, r);
                }
            }
        }
    }

    fn recluster(self: &Rc<Self>, a: &Rc<AppInner>, v: &View, w: f64, h: f64) {
        let b = v.bounds(w * 1.2, h * 1.2);
        let Some(bbox) = map_math::normalize_bbox(b[0], b[1], b[2], b[3]) else {
            return;
        };
        let feats = self.sc.borrow().get_clusters(bbox, v.zoom.round());
        let pts = self.points.borrow();
        let mut rows: Vec<MarkerInfo> = feats
            .into_iter()
            .filter_map(|f| match f {
                Feature::Point(i) => {
                    let p = pts.get(i)?;
                    Some(MarkerInfo {
                        key: format!("p{}", p.0),
                        feature: f,
                        mx: lng_x(p.2),
                        my: lat_y(p.1),
                        cover: p.0.clone(),
                        count: 1,
                    })
                }
                Feature::Cluster {
                    id,
                    count,
                    lon,
                    lat,
                    cover,
                } => Some(MarkerInfo {
                    key: format!("c{id}:{count}"),
                    feature: f,
                    mx: lng_x(lon),
                    my: lat_y(lat),
                    cover: pts.get(cover).map(|p| p.0.clone()).unwrap_or_default(),
                    count,
                }),
            })
            .collect();
        drop(pts);
        // southern markers on top (Leaflet's zIndex by latitude)
        rows.sort_by(|a, b| a.my.total_cmp(&b.my));
        let mut loads: Vec<(String, String)> = vec![];
        let mut keys = self.marker_keys.borrow_mut();
        let mut info = self.marker_info.borrow_mut();
        let mut used = vec![false; keys.len()];
        let mut fresh = vec![];
        for m in rows {
            match keys
                .iter()
                .position(|k| k.as_deref() == Some(m.key.as_str()))
            {
                Some(i) => {
                    used[i] = true;
                    info[i] = Some(m);
                }
                None => fresh.push(m),
            }
        }
        for m in fresh {
            let i = match used.iter().position(|u| !u) {
                Some(i) => i,
                None => {
                    keys.push(None);
                    info.push(None);
                    used.push(false);
                    self.markers.push(MapMarker::default());
                    keys.len() - 1
                }
            };
            used[i] = true;
            keys[i] = Some(m.key.clone());
            let row = MapMarker {
                count: m.count as i32,
                label: map_math::count_label(m.count).into(),
                visible: true,
                hovered: false,
                cover: Image::default(),
                x: 0.0,
                y: 0.0,
            };
            self.markers.set_row_data(i, row);
            loads.push((m.key.clone(), m.cover.clone()));
            info[i] = Some(m);
        }
        for (i, u) in used.iter().enumerate() {
            if !u {
                keys[i] = None;
                info[i] = None;
                if let Some(mut r) = self.markers.row_data(i) {
                    r.visible = false;
                    r.cover = Image::default();
                    self.markers.set_row_data(i, r);
                }
            }
        }
        drop(keys);
        drop(info);
        self.hovered.set(None);
        // covers (a cached one answers right away: no borrows may be held here)
        for (key, cover) in loads {
            let wm = Rc::downgrade(&self.markers);
            let wk = self.me.clone();
            crate::pages::cover(a, &cover, 320, move |img| {
                let (Some(model), Some(me)) = (wm.upgrade(), wk.upgrade()) else {
                    return;
                };
                let Some(i) = me
                    .marker_keys
                    .borrow()
                    .iter()
                    .position(|k| k.as_deref() == Some(key.as_str()))
                else {
                    return;
                };
                if let Some(mut r) = model.row_data(i) {
                    r.cover = img;
                    model.set_row_data(i, r);
                }
            });
        }
    }

    // ───────── chrome / panel ─────────

    fn publish_chrome(&self) {
        let Some(a) = self.app() else { return };
        let st = a.window.global::<MapState>();
        let lang = a.lang();
        let place = self.place.borrow().clone();
        let ex = crate::explore::get();
        let (title, nogps) = {
            let d = ex.as_ref().map(|e| e.data.borrow());
            let r = d.as_ref().and_then(|d| d.as_ref());
            let title = r
                .and_then(|r| r.places.iter().find(|p| p.key == place))
                .map(|p| p.name.clone())
                .filter(|_| !place.is_empty())
                .unwrap_or_else(|| self.t("explore.map"));
            let nl = r.and_then(|r| r.extras.no_location).unwrap_or(0);
            (title, nl)
        };
        st.set_title(title.into());
        st.set_place_mode(!place.is_empty());
        st.set_no_location(if nogps > 0 {
            crate::i18n::tv(lang, "map.noLocation", &[("n", &nogps.to_string())]).into()
        } else {
            "".into()
        });
        let loading = self.points_loading.get() || !self.settings_loaded.get();
        st.set_loading(loading);
        let n_pts = self.points.borrow().len();
        st.set_empty(!loading && n_pts == 0);
        st.set_has_points(n_pts > 0);
        st.set_panel_loading(self.items_loading.get() && n_pts > 0);
        match self.style.get() {
            Some(s) => {
                let info = pa_core::tile_style(s);
                st.set_style(s.as_str().into());
                st.set_style_label(
                    format!(
                        "{}: {}",
                        self.t("map.style"),
                        self.t(&format!("map.style.{}", s.as_str()))
                    )
                    .into(),
                );
                st.set_attribution(info.attribution_text.into());
            }
            None => {
                st.set_style("off".into());
                st.set_attribution("".into());
            }
        }
        let (kind, text) = if self.style.get().is_none() && self.settings_loaded.get() {
            ("off", self.t("explore.map.tilesOff"))
        } else {
            match self.tiles.borrow().problem {
                None => ("", String::new()),
                Some(TileProblem::Offline) => ("offline", self.t("map.error.offline")),
                Some(TileProblem::Refused(s)) => (
                    "refused",
                    crate::i18n::tv(lang, "map.error.refused", &[("status", &s.to_string())]),
                ),
                Some(TileProblem::Unknown) => ("unknown", self.t("map.error.refusedDirect")),
            }
        };
        st.set_problem(kind.into());
        st.set_problem_text(text.into());
    }

    fn update_in_view(&self) {
        let Some(a) = self.app() else { return };
        if !self.on_route() {
            return;
        }
        let (w, h) = self.size.get();
        let b = self.view.get().bounds(w, h);
        let pts = self.points.borrow();
        let ll: Vec<(f64, f64)> = pts.iter().map(|p| (p.1, p.2)).collect();
        let idx = map_math::bounds_filter(&ll, b[0], b[1], b[2], b[3]);
        let lang = a.lang();
        let st = a.window.global::<MapState>();
        st.set_in_view(
            crate::i18n::tv(lang, "explore.map.inView", &[("n", &idx.len().to_string())]).into(),
        );
        st.set_panel_count(idx.len().to_string().into());
        let items = self.items.borrow();
        let mut list: Vec<MediaItem> = idx
            .iter()
            .filter_map(|&i| items.get(&pts[i].0).cloned())
            .collect();
        list.sort_by(|a, b| {
            b.local_at.cmp(&a.local_at).then_with(|| {
                b.id.parse::<i64>()
                    .unwrap_or(0)
                    .cmp(&a.id.parse::<i64>().unwrap_or(0))
            })
        });
        drop(items);
        drop(pts);
        let same = {
            let cur = self.in_view.borrow();
            cur.len() == list.len() && cur.iter().zip(&list).all(|(a, b)| a.id == b.id)
        };
        if same {
            return;
        }
        *self.in_view.borrow_mut() = list;
        self.panel_layout.borrow_mut().take();
        for s in self.panel_slots.borrow_mut().iter_mut() {
            *s = None;
        }
        st.set_panel_scroll(0.0);
        self.render_panel();
    }

    fn render_panel(&self) {
        let Some(a) = self.app() else { return };
        let (y, h, w) = self.panel_vp.get();
        if w <= 0.0 {
            return;
        }
        let items = self.in_view.borrow();
        let mut lay = self.panel_layout.borrow_mut();
        if lay.as_ref().is_none_or(|l| (l.width - w).abs() > 0.5) {
            let aspects: Vec<u8> = items
                .iter()
                .map(|m| match (m.width, m.height) {
                    (Some(w), Some(h)) if w > 0 && h > 0 => {
                        pa_core::aspect_to_byte(w as f64 / h as f64)
                    }
                    _ => pa_core::aspect_to_byte(1.0),
                })
                .collect();
            *lay = Some(crate::grid::layout::layout_grid(
                &aspects,
                items.len(),
                None,
                &crate::grid::layout::LayoutOptions {
                    width: w,
                    target: 140.0,
                    gap: 4.0,
                    square: false,
                    group_by: None,
                    header_h: 0.0,
                },
            ));
        }
        let Some(l) = lay.as_ref() else { return };
        let st = a.window.global::<MapState>();
        st.set_panel_height(l.height as f32);
        // visible items (± one screen)
        let (top, bottom) = (y - h, y + 2.0 * h);
        let mut vis = vec![];
        for r in 0..l.row_count() {
            let rt = l.row_top[r];
            let rh = l.row_h[r] as f64;
            if rt + rh < top || rt > bottom {
                continue;
            }
            for i in l.row_start[r] as usize..l.row_end[r] as usize {
                vis.push(i);
            }
        }
        let mut loads: Vec<(usize, usize, String)> = vec![];
        let mut slots = self.panel_slots.borrow_mut();
        let mut used = vec![false; slots.len()];
        let mut fresh = vec![];
        for &i in &vis {
            match slots.iter().position(|s| *s == Some(i)) {
                Some(k) => used[k] = true,
                None => fresh.push(i),
            }
        }
        for i in fresh {
            let k = match used.iter().position(|u| !u) {
                Some(k) => k,
                None => {
                    slots.push(None);
                    used.push(false);
                    self.panel_model.push(MapPanelTile::default());
                    slots.len() - 1
                }
            };
            used[k] = true;
            slots[k] = Some(i);
            let it = &items[i];
            let r = l.item_row[i] as usize;
            self.panel_model.set_row_data(
                k,
                MapPanelTile {
                    index: i as i32,
                    x: l.item_x[i],
                    y: l.row_top[r] as f32,
                    w: l.item_w[i],
                    h: l.row_h[r],
                    image: Image::default(),
                    video: it.kind == MediaKind::Video,
                    badge: if it.kind == MediaKind::Video {
                        crate::i18n::format_duration(it.duration).into()
                    } else {
                        "".into()
                    },
                    visible: true,
                },
            );
            loads.push((k, i, it.id.clone()));
        }
        for (k, u) in used.iter().enumerate() {
            if !u {
                slots[k] = None;
                if let Some(mut r) = self.panel_model.row_data(k) {
                    if r.visible {
                        r.visible = false;
                        r.image = Image::default();
                        self.panel_model.set_row_data(k, r);
                    }
                }
            }
        }
        drop(slots);
        drop(lay);
        drop(items);
        for (k, i, id) in loads {
            let wm = Rc::downgrade(&self.panel_model);
            let me = self.me.clone();
            crate::pages::cover(&a, &id, 320, move |img| {
                let (Some(m), Some(me)) = (wm.upgrade(), me.upgrade()) else {
                    return;
                };
                if me.panel_slots.borrow().get(k).copied().flatten() != Some(i) {
                    return;
                }
                if let Some(mut row) = m.row_data(k) {
                    row.image = img;
                    m.set_row_data(k, row);
                }
            });
        }
    }

    // ───────── mini map ─────────

    fn mini_style(&self) -> Option<(MapTileStyle, bool, u32)> {
        let s = self.style.get()?;
        let info = pa_core::tile_style(s);
        Some((s, info.dark_filter && self.dark(), 14.min(info.max_zoom)))
    }

    fn refresh_mini(self: &Rc<Self>) {
        let Some(a) = self.app() else { return };
        let st = a.window.global::<MiniMapState>();
        let Some((lat, lon, _)) = self.mini.borrow().clone() else {
            st.set_on(false);
            return;
        };
        if !self.settings_loaded.get() {
            self.load_settings();
            return;
        }
        let Some((style, dark, z)) = self.mini_style() else {
            st.set_on(false);
            return;
        };
        st.set_on(true);
        st.set_attribution(pa_core::tile_style(style).attribution_text.into());
        let t = pa_core::tile_of(lat, lon, z);
        let n = 1i64 << z;
        st.set_ox((256.0 + t.px) as f32);
        st.set_oy((256.0 + t.py) as f32);
        let mut rows = vec![];
        let mut want = vec![];
        let mut failed = 0;
        for dy in -1..=1i64 {
            for dx in -1..=1i64 {
                let ty = t.y + dy;
                if ty < 0 || ty >= n {
                    continue;
                }
                let key = (style, dark, z, (t.x + dx).rem_euclid(n), ty);
                let img = {
                    let mut c = self.tiles.borrow_mut();
                    if c.failed.contains(&key) {
                        failed += 1;
                    }
                    c.cache.get(&key).cloned()
                };
                if img.is_none() {
                    want.push(key);
                }
                rows.push(MiniTile {
                    x: ((dx + 1) * 256) as f32,
                    y: ((dy + 1) * 256) as f32,
                    image: img.unwrap_or_default(),
                });
            }
        }
        st.set_failed(failed >= 3);
        self.mini_model.set_vec(rows);
        for k in want {
            self.request(k);
        }
    }

    fn mini_tile_arrived(self: &Rc<Self>, key: TileKey) {
        let Some((lat, lon, _)) = self.mini.borrow().clone() else {
            return;
        };
        let Some((style, dark, z)) = self.mini_style() else {
            return;
        };
        if (key.0, key.1, key.2) != (style, dark, z) {
            return;
        }
        let t = pa_core::tile_of(lat, lon, z);
        let dx = (key.3 - t.x).abs();
        if (dx <= 1 || dx >= (1 << z) - 1) && (key.4 - t.y).abs() <= 1 {
            self.refresh_mini();
        }
    }
}

/// browse filters as a query string (`/api/geo/points` takes the browse query)
fn pa_client_qs(q: &BrowseQuery) -> String {
    let mut pairs: Vec<(String, String)> = vec![];
    if let Ok(serde_json::Value::Object(map)) = serde_json::to_value(q) {
        for (k, v) in map {
            match v {
                serde_json::Value::Bool(b) => pairs.push((k, if b { "1" } else { "0" }.into())),
                serde_json::Value::String(s) => pairs.push((k, s)),
                serde_json::Value::Number(n) => pairs.push((k, n.to_string())),
                _ => {}
            }
        }
    }
    let refs: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    crate::api::qs(&refs)
}

/// wire MapState / MiniMapState
pub(crate) fn install(app: &Rc<AppInner>) {
    let front = Rc::new(VecModel::<MapTile>::default());
    let back = Rc::new(VecModel::<MapTile>::default());
    let markers = Rc::new(VecModel::<MapMarker>::default());
    let panel = Rc::new(VecModel::<MapPanelTile>::default());
    let mini = Rc::new(VecModel::<MiniTile>::default());
    let ctl = Rc::new_cyclic(|me| MapCtl {
        app: Rc::downgrade(app),
        me: me.clone(),
        place: RefCell::new(String::new()),
        points: RefCell::new(vec![]),
        sc: RefCell::new(Supercluster::new(Options::default())),
        items: RefCell::new(HashMap::new()),
        points_loading: Cell::new(false),
        items_loading: Cell::new(false),
        data_epoch: Cell::new(0),
        style: Cell::new(Some(MapTileStyle::Osm)),
        settings_loaded: Cell::new(false),
        view: Cell::new(View::at(30.0, 15.0, 2.0)),
        size: Cell::new((0.0, 0.0)),
        anim: Cell::new(None),
        drag: RefCell::new(None),
        pinch: Cell::new(None),
        last_tick: Cell::new(Instant::now()),
        timer: slint::Timer::default(),
        settle: slint::Timer::default(),
        fitted: Cell::new(false),
        auto: Cell::new(true),
        focus: RefCell::new(None),
        tiles: RefCell::new(Tiles {
            cache: lru::LruCache::new(NonZeroUsize::new(CACHE_TILES).unwrap()),
            inflight: HashSet::new(),
            failed: HashSet::new(),
            ok: 0,
            bad: 0,
            problem: None,
            epoch: 0,
        }),
        front: front.clone(),
        back: back.clone(),
        front_keys: RefCell::new(vec![]),
        back_keys: RefCell::new(vec![]),
        markers: markers.clone(),
        marker_keys: RefCell::new(vec![]),
        marker_info: RefCell::new(vec![]),
        cluster_zoom: Cell::new(-1),
        hovered: Cell::new(None),
        in_view: RefCell::new(vec![]),
        panel_layout: RefCell::new(None),
        panel_model: panel.clone(),
        panel_slots: RefCell::new(vec![]),
        panel_vp: Cell::new((0.0, 0.0, 0.0)),
        frames: RefCell::new(vec![]),
        last_frame: Cell::new(None),
        mini: RefCell::new(None),
        mini_model: mini.clone(),
    });
    MAP.with(|m| *m.borrow_mut() = Some(ctl.clone()));
    let st = app.window.global::<MapState>();
    st.set_tiles(ModelRc::from(front));
    st.set_back_tiles(ModelRc::from(back));
    st.set_markers(ModelRc::from(markers));
    st.set_panel_tiles(ModelRc::from(panel));
    app.window
        .global::<MiniMapState>()
        .set_tiles(ModelRc::from(mini));

    macro_rules! on {
        ($setter:ident, |$c:ident $(, $arg:ident)*| $body:expr) => {{
            let w = Rc::downgrade(&ctl);
            st.$setter(move |$($arg),*| {
                if let Some($c) = w.upgrade() {
                    $body
                }
            });
        }};
    }
    on!(on_viewport, |c, w, h| {
        let first = c.size.get().0 <= 0.0;
        c.size.set((w as f64, h as f64));
        if first || (c.auto.get() && c.anim.get().is_none()) {
            c.fitted.set(false);
            c.initial_view();
        }
        let v = c.view.get();
        c.set_view(v);
        c.render();
        c.settle_soon();
    });
    on!(on_press, |c, x, y| c.press(x as f64, y as f64));
    on!(on_drag, |c, x, y| c.drag_to(x as f64, y as f64));
    on!(on_release, |c, x, y| c.release(x as f64, y as f64));
    on!(on_hover, |c, x, y| c.hover(x as f64, y as f64));
    on!(on_leave, |c| c.hover(-1e6, -1e6));
    on!(on_wheel, |c, x, y, _dx, dy| c
        .wheel(x as f64, y as f64, dy as f64));
    on!(on_dbl, |c, x, y| {
        let z = (c.view.get().zoom + 1.0).min(c.max_zoom());
        c.animate(Anim::Fly {
            from: c.view.get(),
            to: View {
                zoom: z,
                ..c.view.get()
            },
            t0: Instant::now(),
            ms: 250.0,
            anchor: Some((x as f64, y as f64)),
        });
    });
    on!(on_pinch_start, |c| {
        c.anim.set(None);
        c.drag.borrow_mut().take();
        c.pinch.set(Some((c.view.get(), 0.0, 0.0)));
    });
    on!(on_pinch, |c, scale, x, y| {
        if let Some((v0, _, _)) = c.pinch.get() {
            let (w, h) = c.size.get();
            let mut v = v0;
            let z = (v0.zoom + (scale.max(0.01) as f64).log2()).clamp(MIN_ZOOM, c.max_zoom());
            v.zoom_around(z, x as f64, y as f64, w, h);
            c.set_view(v);
            c.render();
        }
    });
    on!(on_pinch_end, |c| {
        c.pinch.set(None);
        c.settled();
    });
    on!(on_fit, |c| c.fit());
    on!(on_zoom_by, |c, d| c.zoom_by(d as f64));
    on!(on_key, |c, k| {
        let (dx, dy) = match k.as_str() {
            "left" => (80.0, 0.0),
            "right" => (-80.0, 0.0),
            "up" => (0.0, 80.0),
            _ => (0.0, -80.0),
        };
        let mut v = c.view.get();
        v.pan(dx, dy);
        c.set_view(v);
        c.render();
        c.settle_soon();
    });
    on!(on_cycle_style, |c| {
        let Some(a) = c.app() else { return };
        let cur = c.style.get().unwrap_or(MapTileStyle::Osm);
        let styles = pa_core::MAP_TILE_STYLES;
        let next = styles[(styles.iter().position(|s| *s == cur).unwrap_or(0) + 1) % styles.len()];
        let w = c.me.clone();
        crate::api::call::<pa_core::Settings>(
            &a,
            "PATCH",
            "/api/settings",
            Some(serde_json::json!({ "mapTiles": next.as_str() })),
            move |_, r| {
                // read-only server: keep the current style
                if let (Some(me), Ok(s)) = (w.upgrade(), r) {
                    me.set_style(s.map_tiles.style());
                    me.publish_chrome();
                    me.render();
                }
            },
        );
    });
    on!(on_retry, |c| {
        {
            let mut t = c.tiles.borrow_mut();
            t.failed.clear();
            t.problem = None;
            t.bad = 0;
        }
        c.publish_chrome();
        c.render();
    });
    on!(on_nogps, |c| {
        if let Some(a) = c.app() {
            a.go(Route::ExploreResult("nogps".into(), String::new()), true);
        }
    });
    on!(on_toggle_fullscreen, |c| {
        if let Some(a) = c.app() {
            let st = a.window.global::<MapState>();
            st.set_fullscreen(!st.get_fullscreen());
        }
    });
    on!(on_panel_viewport, |c, y, h, w| {
        c.panel_vp.set((y as f64, h as f64, w as f64));
        c.render_panel();
    });
    on!(on_panel_open_item, |c, i| {
        let items = c.in_view.borrow().clone();
        crate::actions_ui::open_viewer_list(items, i as usize);
    });
    let mst = app.window.global::<MiniMapState>();
    let w = Rc::downgrade(&ctl);
    mst.on_open(move || {
        if let Some(c) = w.upgrade() {
            let m = c.mini.borrow().clone();
            if let Some((lat, lon, id)) = m {
                open_focus(&id, lat, lon);
            }
        }
    });
}

/// "Show on map" (viewer): close the viewer, open the map centred on the item
pub(crate) fn open_focus(id: &str, lat: f64, lon: f64) {
    let Some(a) = get().and_then(|c| c.app()) else {
        return;
    };
    if let Some(v) = crate::viewer::get() {
        v.close_now();
    }
    a.go(
        Route::ExploreMap {
            place: String::new(),
            focus: Some((id.to_string(), format!("{lat:.6}"), format!("{lon:.6}"))),
        },
        true,
    );
}

/// a route was entered
pub(crate) fn enter(app: &Rc<AppInner>, route: &Route) {
    let Some(c) = get() else { return };
    let st = app.window.global::<MapState>();
    let Route::ExploreMap { place, focus } = route else {
        st.set_fullscreen(false);
        c.timer.stop();
        return;
    };
    // the map has no page title of its own: the top bar shows it
    app.window.global::<crate::Shell>().set_title_revealed(true);
    let focus = focus.as_ref().and_then(|(id, la, lo)| {
        let (lat, lon) = (la.parse::<f64>().ok()?, lo.parse::<f64>().ok()?);
        (lat.abs() <= 90.0 && lon.abs() <= 180.0).then(|| (id.clone(), lat, lon))
    });
    let place_changed = *c.place.borrow() != *place;
    let had_focus = c.focus.borrow().clone();
    *c.focus.borrow_mut() = focus.clone();
    // "Show on map" again with the map open: re-centre
    if let (Some((_, lat, lon)), true) = (
        &focus,
        had_focus.as_ref().map(|f| (&f.0, f.1, f.2)) != focus.as_ref().map(|f| (&f.0, f.1, f.2)),
    ) {
        if !place_changed && !c.points.borrow().is_empty() {
            c.fly_to(*lat, *lon, 16.0);
        }
    }
    if place_changed || c.points.borrow().is_empty() || !c.fitted.get() {
        *c.place.borrow_mut() = place.clone();
        c.fitted.set(false);
        c.in_view.borrow_mut().clear();
        c.load_points();
    }
    c.load_settings();
    c.publish_chrome();
    c.render();
}

/// Explore data arrived (place names, no-location count)
pub(crate) fn on_explore_data(_app: &Rc<AppInner>) {
    if let Some(c) = get() {
        c.publish_chrome();
    }
}

/// server events: new / removed media → reload the points while the map is open
pub(crate) fn on_event(_app: &Rc<AppInner>, ev: &pa_core::ServerEvent) {
    let Some(c) = get() else { return };
    if matches!(ev, pa_core::ServerEvent::Explore) && c.on_route() {
        c.load_points();
    }
}

/// theme / motion changed: dark tiles are other cache keys
pub(crate) fn on_prefs(_app: &Rc<AppInner>) {
    if let Some(c) = get() {
        c.publish_chrome();
        c.render();
        c.refresh_mini();
    }
}

/// The viewer shows an item with a location (None: hide the mini map).
pub(crate) fn set_mini(point: Option<(f64, f64, String)>) {
    let Some(c) = get() else { return };
    if *c.mini.borrow() == point {
        return;
    }
    *c.mini.borrow_mut() = point;
    c.refresh_mini();
}

/// frame gaps (ms) of the map while it moved (perf report)
pub(crate) fn frame_stats(reset: bool) -> Vec<f64> {
    let Some(c) = get() else { return vec![] };
    if reset {
        c.frames.borrow_mut().clear();
        c.last_frame.set(None);
        return vec![];
    }
    let f = c.frames.borrow().clone();
    f
}

/// automation: `map-fit`, `map-zoom <d>`, `map-pan <dx> <dy> <steps>`, `map-wheel <x> <y> <dy>`,
/// `map-perf <n>` (pans + zooms for n frames and prints frame times), `map-click <x> <y>`
pub(crate) fn script_step(app: &Rc<AppInner>, cmd: &[String]) -> bool {
    let Some(c) = get() else { return false };
    let f = |i: usize| {
        cmd.get(i)
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    match cmd[0].as_str() {
        "map-fit" => c.fit(),
        "map-zoom" => c.zoom_by(f(1)),
        "map-wheel" => c.wheel(f(1), f(2), f(3)),
        "map-click" => c.click(f(1), f(2)),
        "map-pan" => {
            let (dx, dy, n) = (f(1), f(2), f(3).max(1.0) as usize);
            let (w, h) = c.size.get();
            c.press(w / 2.0, h / 2.0);
            let me = c.clone();
            let step = Rc::new(Cell::new(0usize));
            let t = Rc::new(slint::Timer::default());
            let t2 = t.clone();
            t.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(16),
                move || {
                    let k = step.get() + 1;
                    step.set(k);
                    me.drag_to(
                        w / 2.0 + dx * k as f64 / n as f64,
                        h / 2.0 + dy * k as f64 / n as f64,
                    );
                    if k >= n {
                        me.release(w / 2.0 + dx, h / 2.0 + dy);
                        t2.stop();
                    }
                },
            );
            std::mem::forget(t);
        }
        "map-perf" => {
            let n = f(1).max(60.0) as usize;
            frame_stats(true);
            let me = c.clone();
            let k = Rc::new(Cell::new(0usize));
            let t = Rc::new(slint::Timer::default());
            let t2 = t.clone();
            let (w, h) = c.size.get();
            c.press(w / 2.0, h / 2.0);
            t.start(
                slint::TimerMode::Repeated,
                Duration::from_millis(16),
                move || {
                    let i = k.get() + 1;
                    k.set(i);
                    let ph = i as f64 / 60.0 * std::f64::consts::PI;
                    me.drag_to(
                        w / 2.0 + 220.0 * ph.sin(),
                        h / 2.0 + 120.0 * (ph * 0.7).sin(),
                    );
                    if i == n / 2 {
                        me.release(w / 2.0, h / 2.0);
                        me.wheel(w / 2.0, h / 2.0, -120.0);
                    }
                    if i > n / 2 && i.is_multiple_of(20) {
                        me.wheel(
                            w / 2.0 + 100.0,
                            h / 2.0,
                            if (i / 20).is_multiple_of(2) {
                                120.0
                            } else {
                                -120.0
                            },
                        );
                    }
                    if i >= n {
                        t2.stop();
                        let mut fr = frame_stats(false);
                        fr.sort_by(|a, b| a.total_cmp(b));
                        let q = |p: f64| {
                            fr.get(((fr.len() as f64 - 1.0) * p) as usize)
                                .copied()
                                .unwrap_or(0.0)
                        };
                        eprintln!(
                            "[map-perf] frames {} gap p50 {:.1} ms p95 {:.1} ms max {:.1} ms",
                            fr.len(),
                            q(0.5),
                            q(0.95),
                            q(1.0)
                        );
                    }
                },
            );
            std::mem::forget(t);
        }
        "map-style" => {
            let s = cmd.get(1).map(String::as_str);
            c.set_style(pa_core::tile_style_of(s));
            c.publish_chrome();
            c.render();
        }
        "map-panel" => {
            let st = app.window.global::<MapState>();
            st.set_panel_open(cmd.get(1).map(String::as_str) != Some("off"));
        }
        "map-full" => {
            let st = app.window.global::<MapState>();
            st.set_fullscreen(!st.get_fullscreen());
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_filter_matches_css() {
        // white → near the browser's dark map background, black → light grey
        let mut px = [255u8, 255, 255, 255, 0, 0, 0, 255, 170, 211, 223, 255];
        dark_filter(&mut px);
        assert!(px[0] < 40 && px[1] < 40 && px[2] < 40, "{px:?}");
        assert!(px[4] > 200 && px[5] > 200 && px[6] > 200, "{px:?}");
        // OSM water (#aad3df) turns into a dark blue-ish grey
        assert!(px[8] < 90 && px[10] > px[8], "{px:?}");
        assert_eq!(px[3], 255);
    }

    #[test]
    fn query_string() {
        let q = BrowseQuery {
            place: Some("TR-34".into()),
            ..lb::explore_query()
        };
        let s = pa_client_qs(&q);
        assert!(
            s.contains("place=TR-34")
                && s.contains("kind=image-video")
                && s.contains("recursive=1")
        );
    }

    #[test]
    fn problems_from_errors() {
        let e = crate::backend::BackendError {
            status: 502,
            message: "HTTP 502: Tile server refused (HTTP 403)".into(),
        };
        assert_eq!(problem_of(&e), TileProblem::Refused(403));
        let e = crate::backend::BackendError {
            status: 504,
            message: "x".into(),
        };
        assert_eq!(problem_of(&e), TileProblem::Offline);
    }
}
