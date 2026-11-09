//! The viewer (features/viewer/Viewer.tsx + Slide.tsx + InfoPanel.tsx): an overlay over any grid
//! (Photos, Favorites, search, albums, folders — whatever the shared [`GridCtl`] shows), paging
//! through the whole result set (`get_at` / `index_of_id`, pages loaded around the current item),
//! the image pipeline thumbhash → 640 → 1600 → zoom rendition, zoom / pan with rubber banding,
//! the shared-element open / close from the tile, keyboard, filmstrip, details panel, top bar
//! actions (src/actions.rs registry) and the video / audio player (src/video.rs).
//!
//! All motion runs on one 16 ms timer with springs (the motion/react springs of the web UI:
//! stiffness 340 / damping 36, pager 300 / 34); Settings → Animations "off" snaps.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use pa_core::{MediaDetails, MediaItem, MediaKind};
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};

use crate::actions::{self, Surface};
use crate::actions_ops::ImageSize;
use crate::app::AppInner;
use crate::grid::GridCtl;
use crate::i18n::{self, HourCycle, Lang};
use crate::thumbs;
use crate::video;
use crate::viewer_fmt as fmt;
use crate::{ActionItem, InfoRow, SlideData, SlideKind, StripItem, ViewerState};

const GAP: f64 = 24.0;
const MAX_ZOOM: f64 = 8.0;
const HIRES_AT: f64 = 1.5;
const TICK: Duration = Duration::from_millis(16);

// ───────── springs ─────────

#[derive(Debug, Clone, Copy)]
struct Spring {
    v: f64,
    t: f64,
    vel: f64,
    k: f64,
    c: f64,
    eps: f64,
}

impl Spring {
    fn new(v: f64, eps: f64) -> Spring {
        Spring {
            v,
            t: v,
            vel: 0.0,
            k: 340.0,
            c: 36.0,
            eps,
        }
    }
    /// jump (no animation)
    fn set(&mut self, v: f64) {
        self.v = v;
        self.t = v;
        self.vel = 0.0;
    }
    fn to(&mut self, t: f64) {
        self.t = t;
        self.k = 340.0;
        self.c = 36.0;
    }
    fn aim(&mut self, t: f64, k: f64, c: f64, vel: Option<f64>) {
        self.t = t;
        self.k = k;
        self.c = c;
        if let Some(v) = vel {
            self.vel = v;
        }
    }
    fn moving(&self) -> bool {
        (self.v - self.t).abs() > self.eps || self.vel.abs() > self.eps * 10.0
    }
    fn step(&mut self, dt: f64) {
        if !self.moving() {
            self.v = self.t;
            self.vel = 0.0;
            return;
        }
        let n = (dt / (1.0 / 240.0)).ceil().max(1.0) as usize;
        let h = dt / n as f64;
        for _ in 0..n {
            let a = -self.k * (self.v - self.t) - self.c * self.vel;
            self.vel += a * h;
            self.v += self.vel * h;
        }
        if !self.moving() {
            self.v = self.t;
            self.vel = 0.0;
        }
    }
}

fn rubber(v: f64, min: f64, max: f64) -> f64 {
    if v < min {
        min + (v - min) * 0.3
    } else if v > max {
        max + (v - max) * 0.3
    } else {
        v
    }
}

/// fit (contain) of `aspect` into w×h
pub fn fit_contain(w: f64, h: f64, aspect: f64) -> (f64, f64) {
    if w <= 0.0 || h <= 0.0 {
        return (0.0, 0.0);
    }
    let (mut fw, mut fh) = (w, w / aspect);
    if fh > h {
        fh = h;
        fw = h * aspect;
    }
    (fw, fh)
}

fn aspect_of(m: &MediaItem) -> f64 {
    match (m.width, m.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => w as f64 / h as f64,
        _ => 1.5,
    }
}

fn hour_cycle(a: &AppInner) -> HourCycle {
    match a.prefs.borrow().hour_cycle.as_str() {
        "h23" => HourCycle::H23,
        "h12" => HourCycle::H12,
        _ => HourCycle::Auto,
    }
}

/// `text-transform: uppercase` with the Turkish dotted / dotless i
fn upper(s: &str, lang: Lang) -> String {
    if lang == Lang::Tr {
        s.replace('i', "İ").replace('ı', "I").to_uppercase()
    } else {
        s.to_uppercase()
    }
}

fn viewable(m: &MediaItem) -> bool {
    m.kind != MediaKind::Archive
}

fn has_picture(m: &MediaItem) -> bool {
    matches!(m.kind, MediaKind::Image | MediaKind::Video)
}

type Rect = (f64, f64, f64, f64);

fn lerp(a: Rect, b: Rect, k: f64) -> Rect {
    (
        a.0 + (b.0 - a.0) * k,
        a.1 + (b.1 - a.1) * k,
        a.2 + (b.2 - a.2) * k,
        a.3 + (b.3 - a.3) * k,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Mid,
    Hi,
    Sprite,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Axis {
    X,
    Y,
}

#[derive(Debug, Clone)]
struct Drag {
    x0: f64,
    y0: f64,
    pan0: (f64, f64),
    axis: Option<Axis>,
    moved: bool,
    /// recent (time, x, y) for the release velocity
    samples: Vec<(Instant, f64, f64)>,
}

struct St {
    open: bool,
    closing: bool,
    index: Option<usize>,
    item: Option<MediaItem>,
    /// neighbours of the current item (to move on when it disappears)
    neighbours: Vec<MediaItem>,
    stage: Rect,
    track: Spring,
    scale: Spring,
    zx: Spring,
    zy: Spring,
    dismiss: Spring,
    lift: Spring,
    appear: Spring,
    hero: Option<(Rect, Rect)>,
    hires: bool,
    drag: Option<Drag>,
    last_tap: Option<(Instant, f64, f64)>,
    details: Option<MediaDetails>,
    details_for: String,
    revealed: Option<String>,
    player: Option<video::Player>,
    player_for: String,
    last_status: Instant,
    cache: lru::LruCache<(String, Kind), Image>,
    inflight: HashSet<(String, Kind)>,
    failed: HashSet<(String, Kind)>,
    strip_loading: HashSet<String>,
    zoom_goal: f64,
    lang: Lang,
}

/// The viewer controller (UI thread).
pub(crate) struct Viewer {
    app: Weak<AppInner>,
    me: Weak<Viewer>,
    st: RefCell<St>,
    anim: slint::Timer,
    anim_on: Cell<bool>,
    last_tick: Cell<Instant>,
    chrome_timer: slint::Timer,
    controls_timer: slint::Timer,
    video_timer: slint::Timer,
    tap_timer: slint::Timer,
    settle_timer: slint::Timer,
    /// Pages over this list instead of the grid (map panel, duplicates); indexes refer to it.
    list: RefCell<Option<Rc<Vec<MediaItem>>>>,
}

type List = Option<Rc<Vec<MediaItem>>>;

fn src_get(g: &GridCtl, l: &List, i: usize) -> Option<MediaItem> {
    match l {
        Some(l) => l.get(i).cloned(),
        None => g.get_at(i),
    }
}

fn src_total(g: &GridCtl, l: &List) -> usize {
    match l {
        Some(l) => l.len(),
        None => g.total(),
    }
}

thread_local! {
    static VIEWER: RefCell<Option<Rc<Viewer>>> = const { RefCell::new(None) };
}

/// The viewer of this UI thread.
pub(crate) fn get() -> Option<Rc<Viewer>> {
    VIEWER.with(|v| v.borrow().clone())
}

/// Stop playback before thread-local storage is torn down (see explore_player::shutdown).
pub(crate) fn shutdown() {
    if let Some(v) = get() {
        v.video_timer.stop();
        if let Ok(mut s) = v.st.try_borrow_mut() {
            s.player = None;
        }
    }
}

fn key(k: slint::platform::Key) -> SharedString {
    let c: char = k.into();
    c.to_string().into()
}

impl Viewer {
    fn app(&self) -> Option<Rc<AppInner>> {
        self.app.upgrade()
    }

    fn with_vs(&self, f: impl FnOnce(&ViewerState)) {
        if let Some(a) = self.app() {
            f(&a.window.global::<ViewerState>());
        }
    }

    // ───────── open / close ─────────

    /// Open at grid index `i`.
    pub fn open_index(&self, i: usize) {
        let Some(a) = self.app() else { return };
        *self.list.borrow_mut() = None;
        let (item, tile, thumb) = {
            let mut g = a.grid.0.borrow_mut();
            let Some(item) = g.get_at(i) else { return };
            g.ensure_range(i.saturating_sub(6), i + 6);
            let tile = g.tile_window_rect(i);
            let thumb = g
                .cached_thumb(&item.id, 640)
                .or_else(|| g.cached_thumb(&item.id, 320))
                .or_else(|| g.placeholder_of(i));
            (item, tile, thumb)
        };
        self.open_with(Some(i), item, tile, thumb);
    }

    /// Open one item that is not in the grid (no paging).
    pub fn open_item(&self, item: MediaItem) {
        *self.list.borrow_mut() = None;
        self.open_with(None, item, None, None);
    }

    /// Open `items[i]` and page over `items` (pages that are not the shared grid).
    pub fn open_list(&self, items: Vec<MediaItem>, i: usize) {
        let Some(item) = items.get(i).cloned() else { return };
        *self.list.borrow_mut() = Some(Rc::new(items));
        self.open_with(Some(i), item, None, None);
    }

    fn list(&self) -> List {
        self.list.borrow().clone()
    }

    /// `i` as a grid index (None while paging over a list: grid caches are keyed by grid index).
    fn grid_index(&self, i: Option<usize>) -> Option<usize> {
        if self.list.borrow().is_some() {
            None
        } else {
            i
        }
    }

    fn open_with(
        &self,
        index: Option<usize>,
        item: MediaItem,
        tile: Option<Rect>,
        thumb: Option<Image>,
    ) {
        let Some(a) = self.app() else { return };
        let prefs = a.prefs.borrow().clone();
        let info_open = match prefs.info_default.as_str() {
            "open" => true,
            "closed" => false,
            _ => prefs.info_open,
        };
        let stage = self.compute_stage(&a, info_open);
        {
            let mut s = self.st.borrow_mut();
            let s = &mut *s;
            s.lang = a.lang();
            s.open = true;
            s.closing = false;
            s.index = index;
            s.item = Some(item.clone());
            s.stage = stage;
            for sp in [
                &mut s.track,
                &mut s.dismiss,
                &mut s.lift,
                &mut s.zx,
                &mut s.zy,
            ] {
                sp.set(0.0);
            }
            s.scale.set(1.0);
            s.zoom_goal = 1.0;
            s.hires = false;
            s.drag = None;
            s.details = None;
            s.details_for.clear();
            s.player = None;
            s.player_for.clear();
            let motion = a.window.global::<crate::Motion>().get_level() > 0;
            let hero_ok = motion && thumb.is_some() && tile.is_some() && has_picture(&item);
            if hero_ok {
                let to = self.fit_rect_window(s, &item);
                s.hero = Some((tile.unwrap(), to));
                s.appear.set(0.0);
                s.appear.to(1.0);
            } else {
                s.hero = None;
                s.appear.set(if motion { 0.0 } else { 1.0 });
                s.appear.to(1.0);
            }
        }
        let vs = a.window.global::<ViewerState>();
        vs.set_bg(prefs.viewer_bg.clone().into());
        vs.set_filmstrip(prefs.filmstrip);
        vs.set_info_open(info_open);
        vs.set_closing(false);
        vs.set_chrome(true);
        vs.set_video_active(false);
        vs.set_has_frame(false);
        vs.set_menu_open(false);
        if let Some(t) = &thumb {
            vs.set_hero_image(t.clone());
        }
        vs.set_open(true);
        if prefs.info_default != "remember" {
            let open = prefs.info_default == "open";
            if open != prefs.info_open {
                a.set_pref("infoOpen", if open { "true" } else { "false" });
            }
        }
        self.publish_all();
        self.start_anim();
        self.poke();
        self.load_details();
        crate::actions_ui::viewer_opened(true);
    }

    /// Close (animates back to the tile when it is on screen).
    pub fn close(&self) {
        let Some(a) = self.app() else { return };
        let (index, item) = {
            let mut s = self.st.borrow_mut();
            if !s.open || s.closing {
                return;
            }
            s.closing = true;
            s.player = None;
            s.player_for.clear();
            (s.index, s.item.clone())
        };
        let vs = a.window.global::<ViewerState>();
        vs.set_video_active(false);
        vs.set_closing(true);
        // bring the tile into view, then fly back to it
        let mut tile = None;
        let mut thumb = None;
        if let (Some(i), Some(item)) = (self.grid_index(index), item.as_ref()) {
            let mut g = a.grid.0.borrow_mut();
            g.scroll_to_index(i, false);
            tile = g.tile_window_rect(i);
            thumb = g
                .cached_thumb(&item.id, 640)
                .or_else(|| g.cached_thumb(&item.id, 320));
            g.set_focus(Some(i));
        }
        let motion = a.window.global::<crate::Motion>().get_level() > 0;
        {
            let mut s = self.st.borrow_mut();
            let from = item
                .as_ref()
                .map(|m| self.fit_rect_window(&s, m))
                .unwrap_or((0.0, 0.0, 0.0, 0.0));
            // zoom resets for the flight
            s.scale.set(1.0);
            s.zx.set(0.0);
            s.zy.set(0.0);
            let dismiss = s.dismiss.v;
            match (tile, thumb, motion, item.as_ref().is_some_and(has_picture)) {
                (Some(t), Some(img), true, true) => {
                    let from = (from.0, from.1 + dismiss, from.2, from.3);
                    s.hero = Some((t, from));
                    vs.set_hero_image(img);
                }
                _ => s.hero = None,
            }
            s.dismiss.set(0.0);
            if motion {
                s.appear.aim(0.0, 340.0, 36.0, None);
            } else {
                s.appear.set(0.0);
            }
        }
        self.publish_motion();
        self.start_anim();
        crate::actions_ui::viewer_opened(false);
    }

    /// Close at once (navigating away).
    pub fn close_now(&self) {
        if self.is_open() {
            self.st.borrow_mut().closing = true;
            self.anim.stop();
            self.anim_on.set(false);
            self.finish_close();
            crate::actions_ui::viewer_opened(false);
        }
    }

    fn finish_close(&self) {
        let Some(a) = self.app() else { return };
        {
            let mut s = self.st.borrow_mut();
            s.open = false;
            s.closing = false;
            s.hero = None;
            s.item = None;
            s.index = None;
            s.player = None;
        }
        let vs = a.window.global::<ViewerState>();
        vs.set_open(false);
        vs.set_hero_on(false);
        vs.set_closing(false);
        if vs.get_fullscreen() {
            a.window.window().set_fullscreen(false);
            vs.set_fullscreen(false);
        }
        self.video_timer.stop();
        crate::actions_ui::focus_grid();
    }

    /// is the viewer showing
    pub fn is_open(&self) -> bool {
        self.st.borrow().open
    }

    // ───────── geometry ─────────

    fn compute_stage(&self, a: &AppInner, info: bool) -> Rect {
        let sz = a.window.window().size();
        let sc = a.window.window().scale_factor() as f64;
        let (w, h) = (sz.width as f64 / sc, sz.height as f64 / sc);
        let right = if info { 356.0 } else { 64.0 };
        (
            64.0,
            64.0,
            (w - 64.0 - right).max(100.0),
            (h - 144.0).max(100.0),
        )
    }

    /// the current item's picture rect at zoom 1 (window coordinates)
    fn fit_rect_window(&self, s: &St, m: &MediaItem) -> Rect {
        let (sx, sy, sw, sh) = s.stage;
        let (fw, fh) = fit_contain(sw, sh, aspect_of(m));
        (sx + (sw - fw) / 2.0, sy + (sh - fh) / 2.0, fw, fh)
    }

    fn bounds(&self, s: &St, scale: f64) -> (f64, f64) {
        let Some(m) = &s.item else { return (0.0, 0.0) };
        let (fw, fh) = fit_contain(s.stage.2, s.stage.3, aspect_of(m));
        (
            ((fw * scale - s.stage.2) / 2.0).max(0.0),
            ((fh * scale - s.stage.3) / 2.0).max(0.0),
        )
    }

    /// Slint reported the stage rectangle (window resized / info panel toggled).
    fn on_stage(&self, x: f64, y: f64, w: f64, h: f64) {
        if let Ok(mut s) = self.st.try_borrow_mut() {
            s.stage = (x, y, w, h);
        }
    }

    // ───────── items ─────────

    fn step(g: &GridCtl, l: &List, i: usize, d: i64) -> Option<(usize, MediaItem)> {
        let total = src_total(g, l) as i64;
        let mut k = i as i64 + d;
        let mut n = 0;
        while k >= 0 && k < total && n < 50 {
            let it = src_get(g, l, k as usize)?;
            if viewable(&it) {
                return Some((k as usize, it));
            }
            k += d;
            n += 1;
        }
        None
    }

    fn slide(
        &self,
        g: &mut GridCtl,
        s: &mut St,
        i: Option<usize>,
        m: &MediaItem,
        cur: bool,
    ) -> SlideData {
        let mut d = SlideData {
            id: m.id.clone().into(),
            kind: match m.kind {
                MediaKind::Video => SlideKind::Video,
                MediaKind::Audio => SlideKind::Audio,
                _ => SlideKind::Image,
            },
            aspect: aspect_of(m) as f32,
            name: m.name.clone().into(),
            duration: i18n::format_duration(m.duration).into(),
            ..Default::default()
        };
        if m.kind == MediaKind::Audio {
            return d;
        }
        if let Some(h) = i.and_then(|i| g.placeholder_of(i)) {
            d.hash = h;
            d.has_hash = true;
        }
        if let Some(t) = g
            .cached_thumb(&m.id, 640)
            .or_else(|| g.cached_thumb(&m.id, 320))
        {
            d.low = t;
            d.has_low = true;
        }
        if let Some(img) = s.cache.get(&(m.id.clone(), Kind::Mid)) {
            d.mid = img.clone();
            d.has_mid = true;
        } else {
            self.want(s, &m.id, Kind::Mid);
        }
        if cur && m.kind == MediaKind::Image && s.hires {
            if let Some(img) = s.cache.get(&(m.id.clone(), Kind::Hi)) {
                d.hi = img.clone();
                d.has_hi = true;
            } else {
                self.want(s, &m.id, Kind::Hi);
            }
        }
        d
    }

    /// Rebuild prev / cur / next, title, actions, filmstrip.
    fn publish_all(&self) {
        let Some(a) = self.app() else { return };
        let vs = a.window.global::<ViewerState>();
        let mut g = a.grid.0.borrow_mut();
        let mut s = self.st.borrow_mut();
        let Some(item) = s.item.clone() else { return };
        let idx = s.index;
        let l = self.list();
        let gidx = self.grid_index(idx);
        let (prev, next) = match idx {
            Some(i) => (Self::step(&g, &l, i, -1), Self::step(&g, &l, i, 1)),
            None => (None, None),
        };
        s.neighbours = [next.as_ref(), prev.as_ref()]
            .into_iter()
            .flatten()
            .map(|(_, m)| m.clone())
            .collect();
        let cur = self.slide(&mut g, &mut s, gidx, &item, true);
        vs.set_cur(cur);
        vs.set_has_prev(prev.is_some());
        vs.set_has_next(next.is_some());
        if let Some((pi, pm)) = &prev {
            let d = self.slide(&mut g, &mut s, self.grid_index(Some(*pi)), pm, false);
            vs.set_prev(d);
        }
        if let Some((ni, nm)) = &next {
            let d = self.slide(&mut g, &mut s, self.grid_index(Some(*ni)), nm, false);
            vs.set_next(d);
        }
        // backdrop: the thumbhash (naturally blurry)
        match gidx.and_then(|i| g.placeholder_of(i)) {
            Some(h) => {
                vs.set_backdrop(h);
                vs.set_has_backdrop(true);
            }
            None => vs.set_has_backdrop(false),
        }
        // title: name · date · i / n
        let lang = a.lang();
        let hc = hour_cycle(&a);
        let when = if item.local_at != 0 {
            fmt::date_long_time(item.local_at, lang, hc)
        } else {
            fmt::date_long_time(item.taken_at + fmt::local_offset_ms(), lang, hc)
        };
        let pos = idx
            .map(|i| {
                format!(
                    " · {}",
                    i18n::tv(
                        lang,
                        "mobile.viewer.position",
                        &[
                            ("i", &i18n::format_count(lang, i as i64 + 1)),
                            ("n", &i18n::format_count(lang, src_total(&g, &l) as i64))
                        ]
                    )
                )
            })
            .unwrap_or_default();
        vs.set_title(item.name.clone().into());
        vs.set_subtitle(format!("{when}{pos}").into());
        vs.set_is_image(item.kind == MediaKind::Image);
        // preload ±2 neighbours at 1600
        if let Some(i) = idx {
            for k in [1i64, -1, 2, -2] {
                let j = i as i64 + k;
                if j < 0 {
                    continue;
                }
                if let Some(m) = src_get(&g, &l, j as usize) {
                    if has_picture(&m) {
                        self.want(&mut s, &m.id, Kind::Mid);
                    }
                }
            }
            if l.is_none() {
                g.ensure_range(i.saturating_sub(6), i + 6);
            }
        }
        // filmstrip (±30 viewable items)
        let mut strip = Vec::new();
        let mut center = 0.0;
        if let Some(i) = idx {
            let (a0, b0) = (
                i.saturating_sub(30),
                (i + 30).min(src_total(&g, &l).saturating_sub(1)),
            );
            let mut x = 0.0f64;
            let mut missing = Vec::new();
            for k in a0..=b0 {
                let Some(m) = src_get(&g, &l, k) else { continue };
                if !viewable(&m) {
                    continue;
                }
                let cur = k == i;
                let w = if cur { 80.0 } else { 40.0 };
                if !strip.is_empty() {
                    x += 6.0;
                }
                let cx = x + w / 2.0;
                if cur {
                    center = cx;
                }
                let img = if m.kind == MediaKind::Audio {
                    None
                } else {
                    g.cached_thumb(&m.id, 320)
                };
                if img.is_none() && m.kind != MediaKind::Audio {
                    missing.push(m.id.clone());
                }
                strip.push(StripItem {
                    index: k as i32,
                    x: cx as f32,
                    has_image: img.is_some(),
                    image: img.unwrap_or_default(),
                    current: cur,
                    audio: m.kind == MediaKind::Audio,
                });
                x += w;
            }
            for id in missing {
                self.want_strip(&mut s, &id);
            }
        }
        vs.set_strip(ModelRc::new(VecModel::from(strip)));
        vs.set_strip_center(center as f32);
        let lang2 = s.lang;
        drop(s);
        drop(g);
        self.publish_actions(&a, &item, lang2);
    }

    fn publish_actions(&self, a: &Rc<AppInner>, item: &MediaItem, lang: Lang) {
        let vs = a.window.global::<ViewerState>();
        let items = [item.clone()];
        let ctx = crate::actions_ui::ctx();
        let list = actions::for_items(&items, Surface::Viewer, &ctx);
        let top_ids = ["favorite", "edit", "download", "share", "delete"];
        let mut top = Vec::new();
        let mut more = Vec::new();
        let mut group = None;
        for act in list {
            if act.id == "slideshow" {
                continue;
            }
            let d = ActionItem {
                id: act.id.clone().into(),
                icon: crate::actions_ui::icon(a, act.icon_for(&items)),
                label: act.label_for(&items, &ctx, lang).into(),
                active: act.is_active(&items),
                danger: act.destructive,
                sep: group.is_some_and(|g| g != act.group),
            };
            if top_ids.contains(&act.id.as_str()) {
                top.push(d);
            } else {
                group = Some(act.group);
                more.push(d);
            }
        }
        if let Some(f) = more.first_mut() {
            f.sep = false;
        }
        vs.set_top_actions(ModelRc::new(VecModel::from(top)));
        vs.set_more_actions(ModelRc::new(VecModel::from(more)));
    }

    // ───────── image loading ─────────

    fn want(&self, s: &mut St, id: &str, kind: Kind) {
        let key = (id.to_string(), kind);
        if s.inflight.contains(&key) || s.failed.contains(&key) || s.cache.contains(&key) {
            return;
        }
        let Some(a) = self.app() else { return };
        let Some(ops) = a.backend.ops() else { return };
        s.inflight.insert(key.clone());
        let size = match kind {
            Kind::Mid => ImageSize::Thumb(1600),
            Kind::Hi => ImageSize::Zoom,
            Kind::Sprite => ImageSize::Filmstrip(20, 90),
        };
        let fut = ops.image(id, size);
        let me = self.me.clone();
        a.rt.spawn_then(
            async move {
                let data = fut.await.ok()?;
                tokio::task::spawn_blocking(move || thumbs::decode(data))
                    .await
                    .ok()
                    .flatten()
            },
            move |px| {
                let Some(v) = me.upgrade() else { return };
                {
                    let mut s = v.st.borrow_mut();
                    s.inflight.remove(&key);
                    match px {
                        Some(px) => {
                            s.cache.put(key.clone(), Image::from_rgba8(px));
                        }
                        None => {
                            s.failed.insert(key.clone());
                        }
                    }
                }
                if key.1 == Kind::Sprite {
                    v.publish_sprite();
                } else if v.is_open() {
                    v.refresh_slides();
                }
            },
        );
    }

    fn want_strip(&self, s: &mut St, id: &str) {
        if s.strip_loading.contains(id) || s.strip_loading.len() > 24 {
            return;
        }
        let Some(a) = self.app() else { return };
        let Some(ops) = a.backend.ops() else { return };
        s.strip_loading.insert(id.to_string());
        let fut = ops.image(id, ImageSize::Thumb(320));
        let me = self.me.clone();
        let id = id.to_string();
        a.rt.spawn_then(
            async move {
                let data = fut.await.ok()?;
                tokio::task::spawn_blocking(move || thumbs::decode(data))
                    .await
                    .ok()
                    .flatten()
            },
            move |px| {
                let Some(v) = me.upgrade() else { return };
                let Some(a) = v.app() else { return };
                v.st.borrow_mut().strip_loading.remove(&id);
                if let Some(px) = px {
                    a.grid.0.borrow_mut().put_thumb(&id, 320, px);
                    if v.is_open() {
                        v.refresh_strip_only();
                    }
                }
            },
        );
    }

    fn refresh_slides(&self) {
        let Some(a) = self.app() else { return };
        let vs = a.window.global::<ViewerState>();
        let mut g = a.grid.0.borrow_mut();
        let mut s = self.st.borrow_mut();
        let Some(item) = s.item.clone() else { return };
        let idx = s.index;
        let l = self.list();
        let cur = self.slide(&mut g, &mut s, self.grid_index(idx), &item, true);
        vs.set_cur(cur);
        if let Some(i) = idx {
            if let Some((pi, pm)) = Self::step(&g, &l, i, -1) {
                let d = self.slide(&mut g, &mut s, self.grid_index(Some(pi)), &pm, false);
                vs.set_prev(d);
            }
            if let Some((ni, nm)) = Self::step(&g, &l, i, 1) {
                let d = self.slide(&mut g, &mut s, self.grid_index(Some(ni)), &nm, false);
                vs.set_next(d);
            }
        }
    }

    fn refresh_strip_only(&self) {
        let Some(a) = self.app() else { return };
        let vs = a.window.global::<ViewerState>();
        let strip = vs.get_strip();
        let l = self.list();
        let mut g = a.grid.0.borrow_mut();
        for r in 0..strip.row_count() {
            let Some(mut it) = strip.row_data(r) else {
                continue;
            };
            if it.has_image || it.audio {
                continue;
            }
            if let Some(m) = src_get(&g, &l, it.index as usize) {
                if let Some(img) = g.cached_thumb(&m.id, 320) {
                    it.image = img;
                    it.has_image = true;
                    strip.set_row_data(r, it);
                }
            }
        }
    }

    /// The grid's pages / structure changed: neighbours may be known now, or the item is gone.
    pub fn on_grid_changed(&self) {
        if !self.is_open() || self.st.borrow().closing || self.list.borrow().is_some() {
            return;
        }
        let Some(a) = self.app() else { return };
        let (id, idx) = {
            let s = self.st.borrow();
            (s.item.as_ref().map(|m| m.id.clone()), s.index)
        };
        let (Some(id), Some(idx)) = (id, idx) else {
            return;
        };
        let found = {
            let g = a.grid.0.borrow();
            match g.get_at(idx) {
                Some(m) if m.id == id => Some((idx, m)),
                _ => g.index_of_id(&id).and_then(|i| g.get_at(i).map(|m| (i, m))),
            }
        };
        match found {
            Some((i, m)) => {
                {
                    let mut s = self.st.borrow_mut();
                    s.index = Some(i);
                    s.item = Some(m);
                }
                self.publish_all();
            }
            None => {
                // the page may simply not be loaded again yet (skeleton reload): wait for it
                let loaded = a
                    .grid
                    .0
                    .borrow()
                    .get_at(idx.min(a.grid.0.borrow().total().saturating_sub(1)))
                    .is_some();
                if !loaded && a.grid.0.borrow().total() > 0 {
                    a.grid
                        .0
                        .borrow_mut()
                        .ensure_range(idx.saturating_sub(2), idx + 2);
                    return;
                }
                let nb = self.st.borrow().neighbours.first().cloned();
                match nb.and_then(|m| a.grid.0.borrow().index_of_id(&m.id).map(|i| (i, m))) {
                    Some((i, m)) => self.go_to(i, m),
                    None => self.close(),
                }
            }
        }
    }

    // ───────── navigation ─────────

    fn go_to(&self, i: usize, m: MediaItem) {
        {
            let mut s = self.st.borrow_mut();
            s.index = Some(i);
            s.item = Some(m);
            s.scale.set(1.0);
            s.zx.set(0.0);
            s.zy.set(0.0);
            s.zoom_goal = 1.0;
            s.hires = false;
            s.dismiss.set(0.0);
            s.lift.set(0.0);
            s.player = None;
            s.player_for.clear();
            s.details = None;
            s.details_for.clear();
        }
        self.with_vs(|vs| {
            vs.set_video_active(false);
            vs.set_has_frame(false);
            vs.set_zoomed(false);
        });
        self.publish_all();
        self.publish_motion();
        self.load_details();
        self.schedule_settle();
    }

    /// Page one slide (`dir` ±1): commits at once, the pager slides into place.
    pub fn page(&self, dir: i64) {
        let Some(a) = self.app() else { return };
        let target = {
            let s = self.st.borrow();
            let Some(i) = s.index else { return };
            let g = a.grid.0.borrow();
            Self::step(&g, &self.list(), i, dir)
        };
        let motion = a.window.global::<crate::Motion>().get_level() > 0;
        let Some((i, m)) = target else {
            self.st.borrow_mut().track.to(0.0);
            self.start_anim();
            return;
        };
        let w = self.st.borrow().stage.2;
        self.go_to(i, m);
        {
            let mut s = self.st.borrow_mut();
            if motion {
                let v = s.track.v + dir as f64 * (w + GAP);
                let vel = s.track.vel;
                s.track.set(v);
                s.track.aim(0.0, 300.0, 34.0, Some(vel));
            } else {
                s.track.set(0.0);
            }
        }
        self.start_anim();
    }

    fn strip_click(&self, i: usize) {
        let Some(a) = self.app() else { return };
        let m = src_get(&a.grid.0.borrow(), &self.list(), i);
        if let Some(m) = m {
            let cur = self.st.borrow().index;
            if cur != Some(i) {
                self.go_to(i, m);
            }
        }
    }

    // ───────── zoom ─────────

    fn zoom_to(&self, s: &mut St, target: f64, cx: f64, cy: f64, animated: bool) {
        let s0 = s.scale.v.max(0.01);
        let ns = target.clamp(1.0, MAX_ZOOM);
        let k = ns / s0;
        let (bx, by) = self.bounds(s, ns);
        let nx = (cx - (cx - s.zx.v) * k).clamp(-bx, bx);
        let ny = (cy - (cy - s.zy.v) * k).clamp(-by, by);
        if animated {
            s.scale.to(ns);
            s.zx.to(nx);
            s.zy.to(ny);
        } else {
            s.scale.set(ns);
            s.zx.set(nx);
            s.zy.set(ny);
        }
        s.zoom_goal = ns;
        if ns > HIRES_AT {
            s.hires = true;
        }
    }

    fn zoom_by(&self, k: f64) {
        let ok = {
            let mut s = self.st.borrow_mut();
            if s.item.as_ref().is_none_or(|m| m.kind != MediaKind::Image) {
                false
            } else {
                let base = if s.scale.moving() {
                    s.zoom_goal
                } else {
                    s.scale.v
                };
                self.zoom_to(&mut s, base * k, 0.0, 0.0, true);
                true
            }
        };
        if ok {
            self.after_zoom();
        }
    }

    fn reset_zoom(&self) {
        {
            let mut s = self.st.borrow_mut();
            s.scale.to(1.0);
            s.zx.to(0.0);
            s.zy.to(0.0);
            s.zoom_goal = 1.0;
        }
        self.after_zoom();
    }

    fn after_zoom(&self) {
        let (zoomed, hires) = {
            let s = self.st.borrow();
            (s.zoom_goal > 1.02, s.hires)
        };
        self.with_vs(|vs| vs.set_zoomed(zoomed));
        if hires {
            self.refresh_slides();
        }
        self.start_anim();
    }

    // ───────── pointer ─────────

    fn picture_rect_stage(&self, s: &St) -> Rect {
        let Some(m) = &s.item else {
            return (0.0, 0.0, 0.0, 0.0);
        };
        let (sw, sh) = (s.stage.2, s.stage.3);
        let (fw, fh) = fit_contain(sw, sh, aspect_of(m));
        let k = s.scale.v;
        (
            (sw - fw * k) / 2.0 + s.zx.v,
            (sh - fh * k) / 2.0 + s.zy.v + s.dismiss.v,
            fw * k,
            fh * k,
        )
    }

    fn on_pointer(&self, kind: i32, button: i32, x: f64, y: f64) {
        let Some(a) = self.app() else { return };
        if !self.is_open() || self.st.borrow().closing {
            return;
        }
        match (kind, button) {
            (0, 0) => {
                let mut s = self.st.borrow_mut();
                s.drag = Some(Drag {
                    x0: x,
                    y0: y,
                    pan0: (s.zx.v, s.zy.v),
                    axis: None,
                    moved: false,
                    samples: vec![(Instant::now(), x, y)],
                });
                // grabbed while sliding: stay where it is
                let (t, z) = (s.track.v, (s.zx.v, s.zy.v));
                s.track.set(t);
                if s.scale.v > 1.02 {
                    s.zx.set(z.0);
                    s.zy.set(z.1);
                }
            }
            (2, 0) => {
                let (dismiss_axis, page_axis) = {
                    let mut s = self.st.borrow_mut();
                    let Some(mut d) = s.drag.take() else { return };
                    let (mx, my) = (x - d.x0, y - d.y0);
                    d.samples.push((Instant::now(), x, y));
                    if d.samples.len() > 6 {
                        d.samples.remove(0);
                    }
                    if mx.abs() > 3.0 || my.abs() > 3.0 {
                        d.moved = true;
                    }
                    let mut r = (false, false);
                    if s.scale.v > 1.02 {
                        let (bx, by) = self.bounds(&s, s.scale.v);
                        s.zx.set(rubber(d.pan0.0 + mx, -bx, bx));
                        s.zy.set(rubber(d.pan0.1 + my, -by, by));
                    } else {
                        if d.axis.is_none() && (mx.abs() >= 8.0 || my.abs() >= 8.0) {
                            d.axis = Some(if mx.abs() >= my.abs() {
                                Axis::X
                            } else {
                                Axis::Y
                            });
                        }
                        match d.axis {
                            Some(Axis::X) => {
                                let has_prev = a.window.global::<ViewerState>().get_has_prev();
                                let has_next = a.window.global::<ViewerState>().get_has_next();
                                let edge = (!has_prev && mx > 0.0) || (!has_next && mx < 0.0);
                                s.track.set(if edge { mx * 0.25 } else { mx });
                                r.1 = true;
                            }
                            Some(Axis::Y) => {
                                if my >= 0.0 {
                                    s.dismiss.set(my);
                                    s.lift.set(0.0);
                                } else {
                                    s.dismiss.set(0.0);
                                    s.lift.set(my * 0.3);
                                }
                                r.0 = true;
                            }
                            None => {}
                        }
                    }
                    s.drag = Some(d);
                    r
                };
                let _ = (dismiss_axis, page_axis);
                self.with_vs(|vs| vs.set_grabbing(true));
                self.publish_motion();
            }
            (1, 0) => {
                self.with_vs(|vs| vs.set_grabbing(false));
                let Some(d) = self.st.borrow_mut().drag.take() else {
                    return;
                };
                let (mx, my) = (x - d.x0, y - d.y0);
                // release velocity (px/ms) over the last samples
                let (vx, vy) = match (d.samples.first(), d.samples.last()) {
                    (Some(f), Some(l)) => {
                        let dt = l.0.duration_since(f.0).as_secs_f64() * 1000.0;
                        if dt > 1.0 {
                            ((l.1 - f.1) / dt, (l.2 - f.2) / dt)
                        } else {
                            (0.0, 0.0)
                        }
                    }
                    _ => (0.0, 0.0),
                };
                if !d.moved {
                    self.tap(x, y);
                    return;
                }
                let zoomed = self.st.borrow().scale.v > 1.02;
                if zoomed {
                    let mut s = self.st.borrow_mut();
                    let (bx, by) = self.bounds(&s, s.scale.v);
                    let (zx, zy) = (s.zx.v, s.zy.v);
                    s.zx.to((zx + vx * 160.0).clamp(-bx, bx));
                    s.zy.to((zy + vy * 160.0).clamp(-by, by));
                    drop(s);
                    self.start_anim();
                    return;
                }
                match d.axis {
                    Some(Axis::X) => {
                        let w = self.st.borrow().stage.2.max(1.0);
                        let fling = vx.abs() > 0.3 && mx.abs() > 24.0;
                        let vs = a.window.global::<ViewerState>();
                        if mx < 0.0 && vs.get_has_next() && (mx < -w * 0.28 || (fling && vx < 0.0))
                        {
                            self.page(1);
                        } else if mx > 0.0
                            && vs.get_has_prev()
                            && (mx > w * 0.28 || (fling && vx > 0.0))
                        {
                            self.page(-1);
                        } else {
                            self.st.borrow_mut().track.to(0.0);
                            self.start_anim();
                        }
                    }
                    Some(Axis::Y) => {
                        if my > 110.0 || (vy > 0.45 && my > 30.0) {
                            self.close();
                        } else {
                            self.st.borrow_mut().dismiss.to(0.0);
                        }
                        if my < -70.0 || (vy < -0.4 && my < -25.0) {
                            self.set_info(true);
                        }
                        self.st.borrow_mut().lift.to(0.0);
                        self.start_anim();
                    }
                    None => {}
                }
            }
            (3, _) => {
                self.with_vs(|vs| vs.set_grabbing(false));
                let mut s = self.st.borrow_mut();
                s.drag = None;
                s.track.to(0.0);
                s.dismiss.to(0.0);
                s.lift.to(0.0);
                drop(s);
                self.start_anim();
            }
            _ => {}
        }
    }

    fn tap(&self, x: f64, y: f64) {
        let now = Instant::now();
        let prev = self.st.borrow_mut().last_tap.take();
        if let Some((t, px, py)) = prev {
            if now.duration_since(t) < Duration::from_millis(280) && (x - px).hypot(y - py) < 40.0 {
                // double click: zoom toggle (images)
                self.tap_timer.stop();
                let (is_img, scale, cx, cy) = {
                    let s = self.st.borrow();
                    (
                        s.item.as_ref().is_some_and(|m| m.kind == MediaKind::Image),
                        s.scale.v,
                        x - s.stage.2 / 2.0,
                        y - s.stage.3 / 2.0,
                    )
                };
                if !is_img {
                    return;
                }
                if scale > 1.02 {
                    self.reset_zoom();
                } else {
                    let dbl = self.app().map(|a| a.prefs.borrow().dbl_zoom).unwrap_or(2.5);
                    {
                        let mut s = self.st.borrow_mut();
                        self.zoom_to(&mut s, dbl, cx, cy, true);
                    }
                    self.after_zoom();
                }
                return;
            }
        }
        self.st.borrow_mut().last_tap = Some((now, x, y));
        let me = self.me.clone();
        self.tap_timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(260),
            move || {
                let Some(v) = me.upgrade() else { return };
                let (inside, zoomed, kind) = {
                    let s = v.st.borrow();
                    let r = v.picture_rect_stage(&s);
                    (
                        x >= r.0 && x <= r.0 + r.2 && y >= r.1 && y <= r.1 + r.3,
                        s.scale.v > 1.02,
                        s.item.as_ref().map(|m| m.kind),
                    )
                };
                match kind {
                    // the audio card / player sit in the middle: a click beside them closes
                    Some(MediaKind::Audio) => {
                        if let Some(a) = v.app() {
                            let (_, _, w, h) = v.st.borrow().stage;
                            let cw = 384f64.min(w - 48.0);
                            let card = ((w - cw) / 2.0, h / 2.0 - 200.0, cw, 400.0);
                            let _ = a;
                            if !(x >= card.0
                                && x <= card.0 + card.2
                                && y >= card.1
                                && y <= card.1 + card.3)
                            {
                                v.close();
                            }
                        }
                    }
                    Some(MediaKind::Video) if inside => v.play_toggle(),
                    _ if !inside && !zoomed => v.close(),
                    _ => {}
                }
            },
        );
    }

    fn on_wheel(&self, x: f64, y: f64, dy: f64) {
        if dy == 0.0 {
            return;
        }
        let ok = {
            let mut s = self.st.borrow_mut();
            if s.item.as_ref().is_none_or(|m| m.kind != MediaKind::Image) || s.closing {
                false
            } else {
                let (cx, cy) = (x - s.stage.2 / 2.0, y - s.stage.3 / 2.0);
                // Slint: positive delta-y = wheel up (zoom in)
                let k = if dy > 0.0 { 1.15 } else { 1.0 / 1.15 };
                let base = s.scale.v;
                self.zoom_to(&mut s, base * k, cx, cy, false);
                if s.scale.v <= 1.0 + 1e-6 {
                    s.zx.set(0.0);
                    s.zy.set(0.0);
                }
                true
            }
        };
        if ok {
            self.after_zoom();
            self.publish_motion();
        }
    }

    // ───────── keyboard ─────────

    fn on_key(&self, text: &str, ctrl: bool, _shift: bool, alt: bool) -> bool {
        use slint::platform::Key;
        if alt || !self.is_open() {
            return false;
        }
        let Some(a) = self.app() else { return false };
        if a.window.global::<ViewerState>().get_menu_open() || crate::actions_ui::dialog_open() {
            return false;
        }
        let video = self.st.borrow().player.is_some();
        let t = SharedString::from(text);
        let is = |k: Key| t == key(k);
        let lower = text.to_lowercase();
        let zstep = a.prefs.borrow().zoom_step;
        if is(Key::Escape) {
            if a.window.global::<ViewerState>().get_fullscreen() {
                self.toggle_fullscreen();
            } else {
                self.close();
            }
        } else if is(Key::RightArrow) {
            self.page(1);
        } else if is(Key::LeftArrow) {
            self.page(-1);
        } else if is(Key::Delete) {
            self.run_action("delete");
        } else if ctrl {
            return false;
        } else if lower == "i" {
            self.set_info(!a.window.global::<ViewerState>().get_info_open());
        } else if lower == "f" {
            self.toggle_fullscreen();
        } else if text == "+" || text == "=" {
            self.zoom_by(zstep);
        } else if text == "-" {
            self.zoom_by(1.0 / zstep);
        } else if text == "0" {
            self.reset_zoom();
        } else if video && (text == " " || lower == "k") {
            self.play_toggle();
        } else if video && lower == "j" {
            self.seek_rel(-10.0);
        } else if video && lower == "l" {
            self.seek_rel(10.0);
        } else if video && lower == "m" {
            self.toggle_mute();
        } else {
            return false;
        }
        self.poke();
        true
    }

    // ───────── chrome / info / fullscreen ─────────

    fn poke(&self) {
        if self.st.borrow().player.is_some() {
            self.video_poke();
        }
        self.with_vs(|vs| {
            if !vs.get_chrome() {
                vs.set_chrome(true);
            }
        });
        let me = self.me.clone();
        self.chrome_timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(3000),
            move || {
                if let Some(v) = me.upgrade() {
                    let busy = v.st.borrow().drag.is_some();
                    v.with_vs(|vs| {
                        if !busy && !vs.get_menu_open() {
                            vs.set_chrome(false);
                        }
                    });
                }
            },
        );
    }

    fn set_info(&self, open: bool) {
        let Some(a) = self.app() else { return };
        a.set_pref("infoOpen", if open { "true" } else { "false" });
        a.window.global::<ViewerState>().set_info_open(open);
        let st = self.compute_stage(&a, open);
        self.st.borrow_mut().stage = st;
        self.clamp_pan();
        if open {
            self.load_details();
        }
    }

    fn clamp_pan(&self) {
        let mut s = self.st.borrow_mut();
        let (bx, by) = self.bounds(&s, s.scale.v);
        let (x, y) = (s.zx.v.clamp(-bx, bx), s.zy.v.clamp(-by, by));
        s.zx.to(x);
        s.zy.to(y);
        drop(s);
        self.start_anim();
    }

    fn toggle_fullscreen(&self) {
        let Some(a) = self.app() else { return };
        let vs = a.window.global::<ViewerState>();
        let on = !vs.get_fullscreen();
        a.window.window().set_fullscreen(on);
        vs.set_fullscreen(on);
    }

    // ───────── details ─────────

    fn load_details(&self) {
        let Some(a) = self.app() else { return };
        let Some(ops) = a.backend.ops() else { return };
        let id = {
            let s = self.st.borrow();
            match &s.item {
                Some(m) if s.details_for != m.id => m.id.clone(),
                _ => return,
            }
        };
        self.st.borrow_mut().details_for = id.clone();
        a.window.global::<ViewerState>().set_info_loading(true);
        self.publish_info();
        let fut = ops.media(&id);
        let me = self.me.clone();
        a.rt.spawn_then(fut, move |r| {
            let Some(v) = me.upgrade() else { return };
            {
                let mut s = v.st.borrow_mut();
                if s.item.as_ref().is_none_or(|m| m.id != id) {
                    return;
                }
                match r {
                    Ok(d) => s.details = Some(d),
                    Err(e) => log::warn!("media {id}: {e}"),
                }
            }
            v.with_vs(|vs| vs.set_info_loading(false));
            v.publish_info();
            v.maybe_start_player();
        });
    }

    fn publish_info(&self) {
        let Some(a) = self.app() else { return };
        let vs = a.window.global::<ViewerState>();
        let s = self.st.borrow();
        let Some(item) = &s.item else { return };
        let lang = s.lang;
        let hc = hour_cycle(&a);
        let t = |k: &str| i18n::t(lang, k);
        let d = s.details.as_ref().filter(|d| d.item.id == item.id);
        let mut rows = Vec::new();
        let row = |icon: &str, label: String, value: String, sub: String| InfoRow {
            icon: icon.into(),
            label: upper(&label, lang).into(),
            value: value.into(),
            sub: sub.into(),
            ..Default::default()
        };
        let local = if item.local_at != 0 {
            item.local_at
        } else {
            item.taken_at + fmt::local_offset_ms()
        };
        rows.push(row(
            "calendar",
            t("viewer.taken"),
            fmt::date_full(local, lang),
            fmt::time_short(local, lang, hc),
        ));
        if let (Some(w), Some(h)) = (item.width, item.height) {
            let mp = (w * h) as f64 / 1e6;
            let mp = format!("{mp:.1}");
            rows.push(row(
                "maximize",
                t("viewer.dimensions"),
                format!("{w} × {h}"),
                format!("{mp} MP"),
            ));
        }
        rows.push(row(
            "hard-drive",
            t("viewer.size"),
            crate::fmt::format_bytes(item.size),
            item.ext.to_uppercase(),
        ));
        if item.duration.is_some() {
            rows.push(row(
                "clock",
                t("viewer.duration"),
                i18n::format_duration(item.duration),
                String::new(),
            ));
        }
        if let Some(v) = d.and_then(|d| d.video.as_ref()) {
            let codec = [v.codec.clone(), v.audio_codec.clone()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" / ")
                .to_uppercase();
            let sub = [
                v.fps
                    .map(|f| format!("{} fps", (f * 100.0).round() / 100.0)),
                v.bitrate.map(|b| format!("{:.1} Mbps", b / 1e6)),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            rows.push(row("film", t("viewer.codec"), codec, sub));
        }
        let exif = d.and_then(|d| d.exif.as_ref());
        let ex = |k: &str| -> Option<String> {
            exif.and_then(|e| e.get(k))
                .and_then(|v| v.as_ref())
                .map(|v| match v {
                    pa_core::ExifValue::Num(n) => {
                        if n.fract() == 0.0 {
                            format!("{}", *n as i64)
                        } else {
                            format!("{n}")
                        }
                    }
                    pa_core::ExifValue::Str(s) => s.clone(),
                })
        };
        let exn = |k: &str| -> Option<f64> {
            exif.and_then(|e| e.get(k))
                .and_then(|v| v.as_ref())
                .and_then(|v| match v {
                    pa_core::ExifValue::Num(n) => Some(*n),
                    pa_core::ExifValue::Str(s) => s.parse().ok(),
                })
        };
        let cam = d.and_then(|d| d.camera.as_ref());
        let make = cam.and_then(|c| c.make.clone()).or_else(|| ex("Make"));
        let model = cam.and_then(|c| c.model.clone()).or_else(|| ex("Model"));
        let lens = cam.and_then(|c| c.lens.clone()).or_else(|| ex("LensModel"));
        if make.is_some() || model.is_some() {
            rows.push(row(
                "camera",
                t("viewer.camera"),
                fmt::camera_label(make.as_deref(), model.as_deref()),
                lens.unwrap_or_default(),
            ));
        }
        let exposure = [
            ex("FNumber").map(|f| format!("ƒ/{f}")),
            exn("ExposureTime").map(|e| {
                if e < 1.0 && e > 0.0 {
                    format!("1/{}s", (1.0 / e).round())
                } else {
                    format!("{e}s")
                }
            }),
            ex("ISO").map(|i| format!("ISO {i}")),
            ex("FocalLength").map(|f| format!("{f}mm")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
        if !exposure.is_empty() {
            rows.push(row(
                "aperture",
                t("viewer.exposure"),
                exposure,
                String::new(),
            ));
        }
        let gps = d.and_then(|d| d.gps.as_ref()).map(|g| (g.lat, g.lon));
        let hide = a.prefs.borrow().hide_location && s.revealed.as_deref() != Some(&item.id);
        // the info panel's mini map (MiniMap.tsx)
        crate::map::set_mini(
            gps.filter(|_| !hide)
                .map(|(la, lo)| (la, lo, item.id.clone())),
        );
        if let Some((lat, lon)) = gps {
            if hide {
                let mut r = row(
                    "map-pin-off",
                    t("viewer.location"),
                    t("pref.locationHidden"),
                    String::new(),
                );
                r.dim = true;
                r.link = "reveal-location".into();
                r.link_label = t("pref.locationReveal").into();
                rows.push(r);
            } else {
                let place = d.and_then(|d| d.place.as_ref());
                let coords = format!("{lat:.5}, {lon:.5}");
                let (value, sub) = match place {
                    Some(p) => {
                        let mut v = p.city.clone();
                        if let Some(a1) = p.admin1.as_ref().filter(|a1| **a1 != p.city) {
                            v.push_str(", ");
                            v.push_str(a1);
                        }
                        (format!("{v} ·{}{}", " ", fmt::country(&p.cc, lang)), coords)
                    }
                    None => (coords, String::new()),
                };
                let mut r = row("map-pin", t("viewer.location"), value, sub);
                r.link = "open-map".into();
                r.link_label = t("map.info.showOnMap").into();
                rows.push(r);
            }
        } else if d.is_some() && has_picture(item) {
            let mut r = row(
                "map-pin-off",
                t("viewer.location"),
                t("map.info.noLocation"),
                String::new(),
            );
            r.dim = true;
            rows.push(r);
        }
        let mut p = row(
            "folder-open",
            t("viewer.path"),
            d.map(|d| d.path.clone()).unwrap_or_else(|| "…".into()),
            String::new(),
        );
        p.mono = true;
        rows.push(p);
        rows.push(row(
            "file-image",
            t("viewer.modified"),
            fmt::date_medium_time(item.mtime + fmt::local_offset_ms(), lang, hc),
            String::new(),
        ));
        vs.set_info_name(item.name.clone().into());
        vs.set_info_rows(ModelRc::new(VecModel::from(rows)));
    }

    fn info_link(&self, l: &str) {
        match l {
            "reveal-location" => {
                {
                    let mut s = self.st.borrow_mut();
                    s.revealed = s.item.as_ref().map(|m| m.id.clone());
                }
                self.publish_info();
            }
            "open-map" => {
                // the in-app map, centred on this item ("Haritada göster")
                let gps = self.st.borrow().details.as_ref().and_then(|d| d.gps);
                let id = self.st.borrow().item.as_ref().map(|m| m.id.clone());
                if let (Some(g), Some(id)) = (gps, id) {
                    crate::map::open_focus(&id, g.lat, g.lon);
                }
            }
            _ => {}
        }
    }

    // ───────── actions ─────────

    fn run_action(&self, id: &str) {
        let item = self.st.borrow().item.clone();
        let Some(item) = item else { return };
        if id == "slideshow-from-here" {
            let Some(a) = self.app() else { return };
            let idx = self.st.borrow().index;
            if let (Some(l), Some(i)) = (self.list(), idx) {
                let list: Vec<MediaItem> =
                    l.iter().skip(i).filter(|m| has_picture(m)).cloned().collect();
                crate::actions_ui::run(&a, "slideshow", list, Surface::Viewer);
                return;
            }
            match idx {
                Some(i) => {
                    let total = a.grid.0.borrow().total();
                    let w = Rc::downgrade(&a);
                    a.grid.0.borrow_mut().items_in_range(
                        i,
                        (i + 1000).min(total.saturating_sub(1)),
                        move |list| {
                            if let Some(a) = w.upgrade() {
                                let list: Vec<MediaItem> =
                                    list.into_iter().filter(has_picture).collect();
                                crate::actions_ui::run(&a, "slideshow", list, Surface::Viewer);
                            }
                        },
                    );
                }
                None => {
                    if let Some(a) = self.app() {
                        crate::actions_ui::run(&a, "slideshow", vec![item], Surface::Viewer);
                    }
                }
            }
            return;
        }
        if let Some(a) = self.app() {
            crate::actions_ui::run(&a, id, vec![item], Surface::Viewer);
        }
    }

    /// An item changed outside (favourite toggled): refresh the copy + the actions.
    pub fn patch_current(&self, f: &dyn Fn(&mut MediaItem)) {
        let item = {
            let mut s = self.st.borrow_mut();
            let Some(m) = s.item.as_mut() else { return };
            f(m);
            m.clone()
        };
        if let Some(a) = self.app() {
            let lang = self.st.borrow().lang;
            self.publish_actions(&a, &item, lang);
        }
    }

    // ───────── video ─────────

    fn schedule_settle(&self) {
        let me = self.me.clone();
        self.settle_timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(120),
            move || {
                if let Some(v) = me.upgrade() {
                    v.maybe_start_player();
                }
            },
        );
    }

    fn maybe_start_player(&self) {
        let Some(a) = self.app() else { return };
        let (item, path, video_info) = {
            let s = self.st.borrow();
            if !s.open || s.closing || s.hero.is_some() || s.track.moving() {
                drop(s);
                self.schedule_settle();
                return;
            }
            let Some(m) = s.item.clone() else { return };
            if !matches!(m.kind, MediaKind::Video | MediaKind::Audio) || s.player_for == m.id {
                return;
            }
            let d = s.details.as_ref().filter(|d| d.item.id == m.id);
            if d.is_none() && s.details_for == m.id {
                // details on the way (local path, fps, audio): give them a moment
                drop(s);
                let me = self.me.clone();
                let id = m.id.clone();
                self.settle_timer.start(
                    slint::TimerMode::SingleShot,
                    Duration::from_millis(400),
                    move || {
                        if let Some(v) = me.upgrade() {
                            let mut s = v.st.borrow_mut();
                            if s.details.is_none() && s.item.as_ref().is_some_and(|m| m.id == id) {
                                // give up waiting: play from the URL
                                s.details_for = format!("{id}#");
                            }
                            drop(s);
                            v.maybe_start_player();
                        }
                    },
                );
                return;
            }
            (
                m,
                d.map(|d| d.path.clone()),
                d.and_then(|d| d.video.clone()),
            )
        };
        let Some(ops) = a.backend.ops() else { return };
        let Some(src) = ops.source(&item, path.as_deref()) else {
            self.with_vs(|vs| {
                vs.set_video_error(i18n::t(a.lang(), "native.videoUnavailable").into())
            });
            return;
        };
        let prefs = a.prefs.borrow().clone();
        let stage = self.st.borrow().stage;
        let scale = a.window.window().scale_factor() as f64;
        let is_video = item.kind == MediaKind::Video;
        let (w, h) = video::output_size(
            aspect_of(&item),
            stage.2 * scale,
            stage.3 * scale,
            item.width,
            item.height,
        );
        let fps = video_info
            .as_ref()
            .and_then(|v| v.fps)
            .filter(|f| *f > 1.0)
            .unwrap_or(30.0)
            .min(60.0);
        let audio = if item.kind == MediaKind::Audio {
            Some(true)
        } else {
            video_info.as_ref().map(|v| v.audio_codec.is_some())
        };
        let source = video::Source {
            input: src.as_input(),
            video: is_video,
            width: w,
            height: h,
            fps,
            duration: item.duration,
            audio,
            vf_pre: Vec::new(),
            vf_post: Vec::new(),
        };
        let volume = if prefs.remember_volume {
            prefs.video_volume
        } else {
            1.0
        };
        let speed = if [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0].contains(&prefs.video_speed) {
            prefs.video_speed
        } else {
            1.0
        };
        let autoplay = prefs.video_autoplay;
        let looping =
            prefs.loop_short_sec > 0.0 && item.duration.is_some_and(|d| d < prefs.loop_short_sec);
        log::info!(
            "play {} ({}x{} @ {fps} fps) from {}",
            item.name,
            w,
            h,
            source.input
        );
        let player = video::Player::new(source, 0.0, autoplay, volume, prefs.video_muted, speed);
        {
            let mut s = self.st.borrow_mut();
            s.player = Some(player);
            s.player_for = item.id.clone();
            s.last_status = Instant::now() - Duration::from_secs(1);
        }
        let vs = a.window.global::<ViewerState>();
        vs.set_video_active(true);
        vs.set_has_frame(false);
        vs.set_playing(autoplay);
        vs.set_position(0.0);
        vs.set_buffered(0.0);
        vs.set_duration(item.duration.unwrap_or(0.0) as f32);
        vs.set_duration_text(fmt::clock(item.duration.unwrap_or(0.0)).into());
        vs.set_time_text(fmt::clock(0.0).into());
        vs.set_volume(volume as f32);
        vs.set_muted(prefs.video_muted);
        vs.set_speed(speed as f32);
        vs.set_looping(looping);
        vs.set_video_error("".into());
        vs.set_controls(true);
        vs.set_has_sprite(false);
        if is_video && item.duration.is_some() {
            let mut s = self.st.borrow_mut();
            self.want(&mut s, &item.id, Kind::Sprite);
            drop(s);
            self.publish_sprite();
        }
        self.video_poke();
        let me = self.me.clone();
        self.video_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(4),
            move || {
                if let Some(v) = me.upgrade() {
                    v.video_tick();
                }
            },
        );
    }

    fn publish_sprite(&self) {
        let s = self.st.borrow();
        let Some(id) = s.item.as_ref().map(|m| m.id.clone()) else {
            return;
        };
        let img = s.cache.peek(&(id, Kind::Sprite)).cloned();
        drop(s);
        if let Some(img) = img {
            self.with_vs(|vs| {
                vs.set_sprite(img);
                vs.set_has_sprite(true);
            });
        }
    }

    fn video_tick(&self) {
        let Some(a) = self.app() else { return };
        let mut s = self.st.borrow_mut();
        let Some(p) = s.player.as_mut() else {
            drop(s);
            self.video_timer.stop();
            return;
        };
        let frame = p.take_frame();
        let status_due = s.last_status.elapsed() > Duration::from_millis(100);
        let vs = a.window.global::<ViewerState>();
        if let Some(f) = frame {
            vs.set_frame(Image::from_rgba8(f.px));
            vs.set_has_frame(true);
            video::count_presented();
        }
        if !status_due {
            return;
        }
        s.last_status = Instant::now();
        let p = s.player.as_mut().unwrap();
        let st = p.status();
        let looping = vs.get_looping();
        if st.ended && st.playing {
            if looping {
                p.seek(0.0);
            } else {
                p.set_playing(false);
                vs.set_playing(false);
                vs.set_controls(true);
            }
        }
        vs.set_position(st.position as f32);
        vs.set_buffered(st.buffered as f32);
        vs.set_buffering(st.buffering);
        vs.set_time_text(fmt::clock(st.position).into());
        vs.set_no_audio_device(st.no_device);
        if let Some(e) = st.error {
            vs.set_video_error(e.into());
        }
    }

    fn play_toggle(&self) {
        let Some(a) = self.app() else { return };
        let vs = a.window.global::<ViewerState>();
        let mut s = self.st.borrow_mut();
        let Some(p) = s.player.as_mut() else { return };
        let st = p.status();
        let on = !st.playing;
        if on && st.ended {
            p.seek(0.0);
        }
        p.set_playing(on);
        vs.set_playing(on);
        drop(s);
        self.video_poke();
    }

    fn seek_frac(&self, f: f64) {
        let dur = self
            .st
            .borrow()
            .item
            .as_ref()
            .and_then(|m| m.duration)
            .unwrap_or(0.0);
        self.seek_abs(f * dur);
    }

    fn seek_rel(&self, d: f64) {
        let pos = self
            .st
            .borrow()
            .player
            .as_ref()
            .map(|p| p.clock())
            .unwrap_or(0.0);
        self.seek_abs(pos + d);
    }

    fn seek_abs(&self, t: f64) {
        let mut s = self.st.borrow_mut();
        if let Some(p) = s.player.as_mut() {
            p.seek(t);
        }
        drop(s);
        self.with_vs(|vs| {
            vs.set_position(t as f32);
            vs.set_time_text(fmt::clock(t).into());
        });
        self.video_poke();
    }

    fn set_volume(&self, v: f64) {
        let Some(a) = self.app() else { return };
        if let Some(p) = self.st.borrow().player.as_ref() {
            p.set_volume(v);
            p.set_muted(false);
        }
        let vs = a.window.global::<ViewerState>();
        vs.set_volume(v as f32);
        vs.set_muted(false);
        if a.prefs.borrow().remember_volume {
            a.set_pref("videoVolume", &format!("{v:.2}"));
        }
    }

    fn toggle_mute(&self) {
        let Some(a) = self.app() else { return };
        let vs = a.window.global::<ViewerState>();
        let m = !vs.get_muted();
        if let Some(p) = self.st.borrow().player.as_ref() {
            p.set_muted(m);
        }
        vs.set_muted(m);
    }

    fn set_speed(&self, sp: f64) {
        if let Some(p) = self.st.borrow_mut().player.as_mut() {
            p.set_speed(sp);
        }
        self.with_vs(|vs| vs.set_speed(sp as f32));
    }

    fn save_frame(&self) {
        let Some(a) = self.app() else { return };
        let (item, pos, input) = {
            let s = self.st.borrow();
            let Some(p) = s.player.as_ref() else { return };
            (s.item.clone(), p.clock(), p.source().input.clone())
        };
        if let Some(item) = item {
            crate::actions_ui::save_frame(&a, item, input, pos);
        }
    }

    fn video_poke(&self) {
        self.with_vs(|vs| vs.set_controls(true));
        let me = self.me.clone();
        self.controls_timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(2400),
            move || {
                if let Some(v) = me.upgrade() {
                    v.with_vs(|vs| {
                        if vs.get_playing() && !vs.get_menu_open() {
                            vs.set_controls(false);
                        }
                    });
                }
            },
        );
    }

    // ───────── animation ─────────

    fn start_anim(&self) {
        if self.anim_on.get() {
            return;
        }
        self.anim_on.set(true);
        self.last_tick.set(Instant::now());
        let me = self.me.clone();
        self.anim.start(slint::TimerMode::Repeated, TICK, move || {
            if let Some(v) = me.upgrade() {
                v.tick();
            }
        });
    }

    fn tick(&self) {
        let now = Instant::now();
        let dt = now
            .duration_since(self.last_tick.get())
            .as_secs_f64()
            .min(0.05);
        self.last_tick.set(now);
        let (moving, done_close, hero_done) = {
            let mut guard = self.st.borrow_mut();
            let s = &mut *guard;
            let dragging = s.drag.is_some();
            for sp in [&mut s.scale, &mut s.zx, &mut s.zy, &mut s.appear] {
                sp.step(dt);
            }
            if !dragging {
                for sp in [&mut s.track, &mut s.dismiss, &mut s.lift] {
                    sp.step(dt);
                }
            }
            let moving = [
                &s.track, &s.scale, &s.zx, &s.zy, &s.dismiss, &s.lift, &s.appear,
            ]
            .iter()
            .any(|sp| sp.moving());
            let done_close = s.closing && !s.appear.moving() && s.appear.v < 0.02;
            let hero_done = s.hero.is_some() && !s.appear.moving() && !s.closing;
            if hero_done {
                s.hero = None;
            }
            (moving, done_close, hero_done)
        };
        self.publish_motion();
        if done_close {
            self.anim.stop();
            self.anim_on.set(false);
            self.finish_close();
            return;
        }
        if hero_done {
            self.maybe_start_player();
        }
        if !moving && self.st.borrow().drag.is_none() {
            self.anim.stop();
            self.anim_on.set(false);
            if !self.st.borrow().closing {
                self.maybe_start_player();
            }
        }
    }

    fn publish_motion(&self) {
        let Some(a) = self.app() else { return };
        let vs = a.window.global::<ViewerState>();
        let s = self.st.borrow();
        vs.set_track_x(s.track.v as f32);
        vs.set_scale(s.scale.v as f32);
        vs.set_zx(s.zx.v as f32);
        vs.set_zy(s.zy.v as f32);
        vs.set_dismiss_y(s.dismiss.v.max(0.0) as f32);
        vs.set_lift_y(s.lift.v as f32);
        vs.set_appear(s.appear.v.clamp(0.0, 1.0) as f32);
        match s.hero {
            Some((from, to)) => {
                let k = s.appear.v.clamp(0.0, 1.2);
                let r = lerp(from, to, k);
                vs.set_hero_on(true);
                vs.set_hero_x(r.0 as f32);
                vs.set_hero_y(r.1 as f32);
                vs.set_hero_w(r.2.max(1.0) as f32);
                vs.set_hero_h(r.3.max(1.0) as f32);
            }
            None => vs.set_hero_on(false),
        }
    }
}

/// Create the viewer and wire `ViewerState`.
pub(crate) fn install(app: &Rc<AppInner>) -> Rc<Viewer> {
    let v = Rc::new_cyclic(|me| Viewer {
        app: Rc::downgrade(app),
        me: me.clone(),
        st: RefCell::new(St {
            open: false,
            closing: false,
            index: None,
            item: None,
            neighbours: vec![],
            stage: (64.0, 64.0, 1000.0, 700.0),
            track: Spring::new(0.0, 0.3),
            scale: Spring::new(1.0, 0.0008),
            zx: Spring::new(0.0, 0.3),
            zy: Spring::new(0.0, 0.3),
            dismiss: Spring::new(0.0, 0.3),
            lift: Spring::new(0.0, 0.3),
            appear: Spring::new(0.0, 0.002),
            hero: None,
            hires: false,
            drag: None,
            last_tap: None,
            details: None,
            details_for: String::new(),
            revealed: None,
            player: None,
            player_for: String::new(),
            last_status: Instant::now(),
            cache: lru::LruCache::new(NonZeroUsize::new(14).unwrap()),
            inflight: HashSet::new(),
            failed: HashSet::new(),
            strip_loading: HashSet::new(),
            zoom_goal: 1.0,
            lang: app.lang(),
        }),
        anim: slint::Timer::default(),
        anim_on: Cell::new(false),
        last_tick: Cell::new(Instant::now()),
        chrome_timer: slint::Timer::default(),
        controls_timer: slint::Timer::default(),
        video_timer: slint::Timer::default(),
        tap_timer: slint::Timer::default(),
        settle_timer: slint::Timer::default(),
        list: RefCell::new(None),
    });
    VIEWER.with(|c| *c.borrow_mut() = Some(v.clone()));
    let vs = app.window.global::<ViewerState>();
    macro_rules! cb {
        ($on:ident, |$v:ident $(, $arg:ident)*| $body:expr) => {{
            let w = Rc::downgrade(&v);
            vs.$on(move |$($arg),*| {
                if let Some($v) = w.upgrade() {
                    $body
                }
            });
        }};
    }
    cb!(on_close, |v| v.close());
    cb!(on_page, |v, d| v.page(d as i64));
    cb!(on_pointer, |v, k, b, x, y| v
        .on_pointer(k, b, x as f64, y as f64));
    {
        let w = Rc::downgrade(&v);
        vs.on_wheel(move |x, y, dy, _ctrl| {
            if let Some(v) = w.upgrade() {
                v.on_wheel(x as f64, y as f64, dy as f64);
            }
        });
    }
    {
        let w = Rc::downgrade(&v);
        vs.on_key(move |t, c, s, al| w.upgrade().is_some_and(|v| v.on_key(&t, c, s, al)));
    }
    cb!(on_action, |v, id| v.run_action(&id));
    {
        let w = Rc::downgrade(&v);
        vs.on_toggle_info(move || {
            if let Some(v) = w.upgrade() {
                let open = v
                    .app()
                    .is_some_and(|a| a.window.global::<ViewerState>().get_info_open());
                v.set_info(!open);
            }
        });
    }
    cb!(on_toggle_fullscreen, |v| v.toggle_fullscreen());
    {
        let w = Rc::downgrade(&v);
        vs.on_zoom_by(move |d| {
            if let Some(v) = w.upgrade() {
                let step = v.app().map(|a| a.prefs.borrow().zoom_step).unwrap_or(1.4);
                v.zoom_by(if d > 0 { step } else { 1.0 / step });
            }
        });
    }
    cb!(on_strip_click, |v, i| v.strip_click(i.max(0) as usize));
    cb!(on_stage_changed, |v, x, y, w, h| v
        .on_stage(x as f64, y as f64, w as f64, h as f64));
    cb!(on_poke, |v| v.poke());
    cb!(on_info_link, |v, l| v.info_link(&l));
    cb!(on_play_toggle, |v| v.play_toggle());
    cb!(on_seek, |v, f| v.seek_frac(f as f64));
    cb!(on_set_volume, |v, x| v.set_volume(x as f64));
    cb!(on_toggle_mute, |v| v.toggle_mute());
    cb!(on_set_speed, |v, x| v.set_speed(x as f64));
    {
        let w = Rc::downgrade(&v);
        vs.on_toggle_loop(move || {
            if let Some(v) = w.upgrade() {
                v.with_vs(|vs| vs.set_looping(!vs.get_looping()));
            }
        });
    }
    cb!(on_save_frame, |v| v.save_frame());
    cb!(on_video_poke, |v| v.video_poke());
    vs.on_format_time(|t| fmt::clock(t as f64).into());
    // the grid's pages / structure changed
    let w = Rc::downgrade(&v);
    app.grid.0.borrow_mut().ix.listeners.push(Rc::new(move || {
        if let Some(v) = w.upgrade() {
            v.on_grid_changed();
        }
    }));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spring_settles() {
        let mut s = Spring::new(0.0, 0.01);
        s.to(100.0);
        let mut t = 0.0;
        while s.moving() && t < 3.0 {
            s.step(0.016);
            t += 0.016;
        }
        assert_eq!(s.v, 100.0);
        assert!(t < 1.0, "took {t}s");
    }

    #[test]
    fn helpers() {
        assert_eq!(fit_contain(1000.0, 500.0, 1.0), (500.0, 500.0));
        assert_eq!(fit_contain(1000.0, 500.0, 4.0), (1000.0, 250.0));
        assert_eq!(rubber(5.0, 0.0, 10.0), 5.0);
        assert_eq!(rubber(20.0, 0.0, 10.0), 13.0);
        assert_eq!(rubber(-10.0, 0.0, 10.0), -3.0);
    }
}
