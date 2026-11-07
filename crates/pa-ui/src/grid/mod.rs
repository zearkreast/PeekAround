//! Photos grid controller — the native counterpart of `grid/source.ts` (skeleton + paged items) and
//! `MediaGrid.tsx` (virtualized rendering).
//!
//! - The skeleton (`/api/browse/skeleton`) gives every item's aspect byte and the day groups, so the
//!   whole grid is laid out at once ([`layout`]); pages of 300 items (`/api/browse`) fill in the
//!   details later and never move a tile.
//! - Only the rows in view (+ ¾ screen of overscan each way) are in the Slint models. Tiles live in
//!   a pool of slots: an item keeps its slot while it stays in view, so a scrolled frame rewrites
//!   just the slots of the rows that entered.
//! - Thumbnails load visible-first with at most [`THUMB_CONCURRENCY`] requests, are decoded on the
//!   blocking pool and kept in a byte-bounded LRU; the thumbhash is shown until they arrive.

pub mod interact;
pub mod layout;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use pa_core::{BrowseQuery, KindCounts, MediaItem, MediaKind};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::backend::{Backend, BackendError};
use crate::i18n::{self, Lang};
use crate::rt::Rt;
use crate::thumbs::{self, Pixels, PlaceholderCache, ThumbCache};
use crate::{HeaderData, TileData, TileKind};
use layout::{grid_params, layout_grid, Day, GridLayout, GroupBy, LayoutOptions};

/// items per page
pub const PAGE: usize = 300;
/// pages kept (≈ 12k items)
const MAX_PAGES: usize = 40;
const PAGE_CONCURRENCY: usize = 3;
/// parallel thumbnail requests
pub const THUMB_CONCURRENCY: usize = 6;

/// An item + its decoded thumbhash.
struct Entry {
    item: MediaItem,
    hash: Option<Pixels>,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum PageState {
    Loading,
    Ok,
    Stale,
    Error,
}

struct Page {
    items: Vec<Entry>,
    state: PageState,
    used: u64,
}

/// What the grid shows (title etc. are set by the app).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GridQuery {
    /// browse query (no paging)
    pub query: BrowseQuery,
}

/// what the current layout was computed for (width, density, square, row height, total, epoch, headers)
type LayoutKey = (i64, i32, bool, i64, usize, u64, (bool, Option<GroupBy>));

/// view options (density level, square tiles, target row height)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewOptions {
    /// 0..3
    pub density: i32,
    /// square tiles
    pub square: bool,
    /// target row height
    pub row_height: f64,
    /// date headers (Settings → Gallery → dateHeaders)
    pub headers: bool,
    /// header grouping: None = by density ("auto"), else day / month
    pub group: Option<GroupBy>,
}

/// timing of the per-frame work (perf log)
#[derive(Debug, Default, Clone)]
pub struct UpdateStats {
    /// viewport updates
    pub updates: u64,
    /// total µs spent in them
    pub total_us: u64,
    /// slowest, µs
    pub max_us: u64,
    /// slots rewritten
    pub slot_writes: u64,
    /// thumbnails loaded
    pub thumbs_loaded: u64,
    /// pages loaded
    pub pages_loaded: u64,
}

/// The grid state (UI thread only). Use through [`GridHandle`].
pub struct GridCtl {
    weak: Weak<RefCell<GridCtl>>,
    app: slint::Weak<crate::AppWindow>,
    backend: Arc<dyn Backend>,
    rt: Rt,
    lang: Lang,
    query: BrowseQuery,
    epoch: u64,
    // skeleton
    ready: bool,
    error: Option<String>,
    gen: String,
    total: usize,
    aspects: Vec<u8>,
    days: Option<Vec<Day>>,
    kind_counts: KindCounts,
    // layout
    view: ViewOptions,
    layout: GridLayout,
    layout_key: Option<LayoutKey>,
    // pages
    pages: HashMap<usize, Page>,
    page_inflight: usize,
    clock: u64,
    // viewport (grid coordinates)
    scroll: f64,
    view_h: f64,
    width: f64,
    dir: i32,
    scale: f32,
    // slots
    tiles: Rc<VecModel<TileData>>,
    headers: Rc<VecModel<HeaderData>>,
    slot_item: Vec<Option<usize>>,
    item_slot: HashMap<usize, usize>,
    header_groups: Vec<usize>,
    // thumbnails
    thumbs: ThumbCache,
    placeholders: PlaceholderCache,
    thumb_inflight: HashSet<(String, u32)>,
    thumb_failed: HashSet<String>,
    visible_ids: Arc<Mutex<HashSet<String>>>,
    thumb_sem: Arc<tokio::sync::Semaphore>,
    current_year: i64,
    /// perf counters
    pub stats: UpdateStats,
    /// selection / focus / hover / listeners (interact.rs)
    pub(crate) ix: interact::Interact,
}

/// Shared handle to the grid controller.
#[derive(Clone)]
pub struct GridHandle(pub Rc<RefCell<GridCtl>>);

fn now_year() -> i64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    i18n::civil_from_days(secs / 86_400).0
}

impl GridHandle {
    /// New controller bound to the window's `GridState` global.
    pub fn new(
        app: &crate::AppWindow,
        backend: Arc<dyn Backend>,
        rt: Rt,
        lang: Lang,
        view: ViewOptions,
    ) -> GridHandle {
        let tiles = Rc::new(VecModel::<TileData>::default());
        let headers = Rc::new(VecModel::<HeaderData>::default());
        let gs = app.global::<crate::GridState>();
        gs.set_tiles(ModelRc::from(tiles.clone()));
        gs.set_headers(ModelRc::from(headers.clone()));
        let ctl = Rc::new_cyclic(|weak| {
            RefCell::new(GridCtl {
                weak: weak.clone(),
                app: app.as_weak(),
                backend,
                rt,
                lang,
                query: BrowseQuery::default(),
                epoch: 0,
                ready: false,
                error: None,
                gen: String::new(),
                total: 0,
                aspects: vec![],
                days: None,
                kind_counts: KindCounts::default(),
                view,
                layout: GridLayout::default(),
                layout_key: None,
                pages: HashMap::new(),
                page_inflight: 0,
                clock: 0,
                scroll: 0.0,
                view_h: 800.0,
                width: 0.0,
                dir: 1,
                scale: 1.0,
                tiles,
                headers,
                slot_item: vec![],
                item_slot: HashMap::new(),
                header_groups: vec![],
                // ≈ 600 thumbnails of 320 px
                thumbs: ThumbCache::new(160 << 20),
                placeholders: PlaceholderCache::new(4000),
                thumb_inflight: HashSet::new(),
                thumb_failed: HashSet::new(),
                visible_ids: Arc::new(Mutex::new(HashSet::new())),
                thumb_sem: Arc::new(tokio::sync::Semaphore::new(THUMB_CONCURRENCY)),
                current_year: now_year(),
                stats: UpdateStats::default(),
                ix: interact::Interact::default(),
            })
        });
        GridHandle(ctl)
    }
}

impl GridCtl {
    fn app(&self) -> Option<crate::AppWindow> {
        self.app.upgrade()
    }

    /// Show another query (resets everything but the thumbnail caches).
    pub fn set_query(&mut self, q: BrowseQuery) {
        if q == self.query && self.ready && self.error.is_none() {
            return;
        }
        self.query = q;
        self.ix.selection.clear();
        self.ix.focus = None;
        self.reset();
        self.selection_changed();
        self.load_skeleton();
    }

    /// Reload the current query (retry / structure changed).
    pub fn reload(&mut self) {
        self.reset();
        self.load_skeleton();
    }

    fn reset(&mut self) {
        self.epoch += 1;
        self.ready = false;
        self.error = None;
        self.gen.clear();
        self.total = 0;
        self.aspects.clear();
        self.days = None;
        self.pages.clear();
        self.page_inflight = 0;
        self.layout = GridLayout::default();
        self.layout_key = None;
        self.clear_slots();
        self.scroll = 0.0;
        if let Some(app) = self.app() {
            let gs = app.global::<crate::GridState>();
            gs.set_loading(true);
            gs.set_error("".into());
            gs.set_total(0);
            gs.set_grid_height(0.0);
            gs.set_scroll_y(0.0);
        }
    }

    fn clear_slots(&mut self) {
        self.tiles.set_vec(vec![]);
        self.headers.set_vec(vec![]);
        self.slot_item.clear();
        self.item_slot.clear();
        self.header_groups.clear();
    }

    /// UI language changed (headers, subtitle)
    pub fn set_lang(&mut self, lang: Lang) {
        self.lang = lang;
        self.header_groups.clear();
        self.publish_meta();
        self.update();
    }

    /// view options changed (re-layout)
    pub fn set_view(&mut self, v: ViewOptions) {
        if v != self.view {
            self.view = v;
            self.update();
        }
    }

    fn load_skeleton(&mut self) {
        let epoch = self.epoch;
        let fut = self.backend.skeleton(self.query.clone());
        let weak = self.weak.clone();
        self.rt.spawn_then(
            async move {
                let sk = fut.await?;
                use base64::Engine;
                let aspects = base64::engine::general_purpose::STANDARD
                    .decode(sk.aspects.as_bytes())
                    .map_err(|e| BackendError {
                        status: 0,
                        message: format!("skeleton: {e}"),
                    })?;
                Ok::<_, BackendError>((sk, aspects))
            },
            move |r| {
                let Some(ctl) = weak.upgrade() else { return };
                let mut c = ctl.borrow_mut();
                if c.epoch != epoch {
                    return;
                }
                match r {
                    Ok((sk, aspects)) => c.apply_skeleton(sk, aspects),
                    Err(e) => {
                        c.error = Some(e.message.clone());
                        if let Some(app) = c.app() {
                            let gs = app.global::<crate::GridState>();
                            gs.set_loading(false);
                            gs.set_error(e.message.into());
                        }
                    }
                }
            },
        );
    }

    fn apply_skeleton(&mut self, sk: pa_core::BrowseSkeleton, mut aspects: Vec<u8>) {
        let total = sk.total.max(0) as usize;
        aspects.resize(total, 128);
        let changed = sk.gen != self.gen || total != self.total;
        if changed && self.ready {
            // structure changed under us: drop pages, keep scroll
            self.epoch += 1;
            self.pages.clear();
            self.page_inflight = 0;
            self.clear_slots();
        }
        self.gen = sk.gen;
        self.total = total;
        self.aspects = aspects;
        self.days = sk.days.map(|d| d.iter().map(Day::from).collect());
        self.kind_counts = sk.kind_counts;
        self.ready = true;
        self.layout_key = None;
        self.publish_meta();
        if let Some(app) = self.app() {
            app.global::<crate::GridState>().set_loading(false);
        }
        self.update();
    }

    fn publish_meta(&self) {
        let Some(app) = self.app() else { return };
        let gs = app.global::<crate::GridState>();
        gs.set_total(self.total as i32);
        let l = self.lang;
        let n = i18n::format_count(l, self.total as i64);
        let mut sub = i18n::tv(l, "common.items", &[("n", &n)]);
        let (p, v) = (self.kind_counts.image, self.kind_counts.video);
        if p > 0 || v > 0 {
            sub.push_str(" · ");
            sub.push_str(&i18n::tv(
                l,
                "home.stats",
                &[
                    ("photos", &i18n::format_count(l, p)),
                    ("videos", &i18n::format_count(l, v)),
                ],
            ));
        }
        gs.set_subtitle(sub.into());
    }

    /// The Flickable moved / resized. `y`: offset inside the grid (may be negative above it).
    pub fn on_viewport(&mut self, y: f64, h: f64, w: f64, scale: f32) {
        if y > self.scroll {
            self.dir = 1;
        } else if y < self.scroll {
            self.dir = -1;
        }
        self.scroll = y;
        self.view_h = h.max(1.0);
        self.width = w;
        self.scale = scale;
        self.update();
    }

    fn ensure_layout(&mut self) -> bool {
        let opts = grid_params(
            self.width,
            self.view.density,
            self.view.row_height,
            self.view.square,
        );
        let key = (
            self.width.round() as i64,
            self.view.density,
            self.view.square,
            self.view.row_height.round() as i64,
            self.total,
            self.epoch,
            (self.view.headers, self.view.group),
        );
        if self.layout_key == Some(key) {
            return false;
        }
        self.layout_key = Some(key);
        let days = self.days.as_deref();
        let opts = LayoutOptions {
            group_by: if days.is_some() && self.view.headers {
                self.view.group.or(opts.group_by)
            } else {
                None
            },
            ..opts
        };
        self.layout = layout_grid(&self.aspects, self.total, days, &opts);
        if let Some(app) = self.app() {
            app.global::<crate::GridState>()
                .set_grid_height(self.layout.height as f32);
        }
        self.publish_ticks();
        self.notify_listeners();
        // every slot moves: rewrite them all
        self.header_groups.clear();
        for s in 0..self.slot_item.len() {
            self.slot_item[s] = None;
        }
        self.item_slot.clear();
        true
    }

    fn entry(&self, i: usize) -> Option<&Entry> {
        let p = self.pages.get(&(i / PAGE))?;
        p.items.get(i % PAGE)
    }

    fn thumb_size(&self, h: f32) -> u32 {
        if h * self.scale > 330.0 {
            640
        } else {
            320
        }
    }

    fn tile_for(&mut self, i: usize) -> TileData {
        let l = &self.layout;
        let r = l.item_row[i] as usize;
        let (x, y, w, h) = (l.item_x[i], l.row_top[r] as f32, l.item_w[i], l.row_h[r]);
        let size = self.thumb_size(h);
        let mut t = TileData {
            index: i as i32,
            x,
            y,
            w,
            h,
            kind: TileKind::Placeholder,
            visible: true,
            ..Default::default()
        };
        let Some(e) = self.entry(i) else { return t };
        t.name = e.item.name.as_str().into();
        let (id, kind, dur, fav) = (
            e.item.id.clone(),
            e.item.kind,
            e.item.duration,
            e.item.favorite,
        );
        let hash = e.hash.clone();
        t.kind = match kind {
            MediaKind::Video => TileKind::Video,
            MediaKind::Audio => TileKind::Audio,
            _ => TileKind::Image,
        };
        if matches!(kind, MediaKind::Video | MediaKind::Audio) {
            t.badge = i18n::format_duration(dur).into();
        }
        t.favorite = fav;
        t.selected = self.ix.selection.contains(&id);
        t.focused = self.ix.ring && self.ix.focus == Some(i);
        if let Some(px) = &hash {
            t.placeholder = self.placeholders.get_or_insert(&id, px);
            t.has_placeholder = true;
        }
        if kind != MediaKind::Audio {
            if let Some(img) = self.thumbs.get(&id, size) {
                t.image = img;
                t.loaded = true;
            }
        }
        t
    }

    /// Recompute what is in view and sync the slot models (called on every scrolled frame).
    pub fn update(&mut self) {
        let t0 = Instant::now();
        if !self.ready || self.width <= 0.0 {
            return;
        }
        self.ensure_layout();
        let l = &self.layout;
        let over = self.view_h * 0.75;
        let (y0, y1) = (self.scroll - over, self.scroll + self.view_h + over);
        let mut items: Vec<usize> = Vec::with_capacity(256);
        let mut groups: Vec<usize> = Vec::new();
        if let Some(mut r) = l.row_at(y0.max(0.0)) {
            while r < l.row_count() && l.row_top[r] <= y1 {
                if l.row_header[r] {
                    groups.push(l.row_group[r] as usize);
                } else {
                    items.extend(l.row_start[r] as usize..l.row_end[r] as usize);
                }
                r += 1;
            }
        }
        // ── slots ──
        let wanted: HashSet<usize> = items.iter().copied().collect();
        let mut free: Vec<usize> = Vec::new();
        for (s, it) in self.slot_item.iter_mut().enumerate() {
            match it {
                Some(i) if wanted.contains(i) => {}
                Some(i) => {
                    self.item_slot.remove(i);
                    *it = None;
                    free.push(s);
                }
                None => free.push(s),
            }
        }
        free.reverse();
        let mut writes = 0u64;
        for &i in &items {
            if self.item_slot.contains_key(&i) {
                continue;
            }
            let data = self.tile_for(i);
            writes += 1;
            if let Some(s) = free.pop() {
                self.slot_item[s] = Some(i);
                self.item_slot.insert(i, s);
                self.tiles.set_row_data(s, data);
            } else {
                let s = self.slot_item.len();
                self.slot_item.push(Some(i));
                self.item_slot.insert(i, s);
                self.tiles.push(data);
            }
        }
        // hide the slots nobody took (kept for reuse)
        for s in free {
            if let Some(mut d) = self.tiles.row_data(s) {
                if d.visible {
                    d.visible = false;
                    d.image = Default::default();
                    d.placeholder = Default::default();
                    self.tiles.set_row_data(s, d);
                    writes += 1;
                }
            }
        }
        // ── headers (few: rebuilt when the set in view changes) ──
        if groups != self.header_groups {
            let hs: Vec<HeaderData> = groups.iter().map(|&g| self.header_for(g)).collect();
            self.headers.set_vec(hs);
            self.header_groups = groups;
        }
        // ── data ──
        if let (Some(&a), Some(&b)) = (items.iter().min(), items.iter().max()) {
            self.ensure_pages(a, b);
        }
        {
            let mut vis = self.visible_ids.lock().unwrap();
            vis.clear();
            for &i in &items {
                if let Some(e) = self.entry(i) {
                    vis.insert(e.item.id.clone());
                }
            }
        }
        self.request_thumbs(&items);
        let us = t0.elapsed().as_micros() as u64;
        self.stats.updates += 1;
        self.stats.total_us += us;
        self.stats.max_us = self.stats.max_us.max(us);
        self.stats.slot_writes += writes;
    }

    fn header_for(&self, g: usize) -> HeaderData {
        let l = &self.layout;
        let grp = &l.groups[g];
        let label = match l.group_by {
            Some(GroupBy::Month) => i18n::format_month_key(&grp.key, self.lang),
            _ => i18n::format_day_key(&grp.key, self.lang, self.current_year),
        };
        HeaderData {
            y: grp.top as f32,
            h: l.row_h[grp.row],
            label: label.into(),
            place: grp.place.clone().unwrap_or_default().into(),
            count: i18n::format_count(self.lang, grp.count as i64).into(),
            visible: true,
            group: g as i32,
        }
    }

    /// Refresh the slots of items in `[a, b)` (page arrived / thumbnail arrived).
    fn refresh_range(&mut self, a: usize, b: usize) {
        let slots: Vec<(usize, usize)> = self
            .item_slot
            .iter()
            .filter(|(i, _)| **i >= a && **i < b)
            .map(|(i, s)| (*i, *s))
            .collect();
        for (i, s) in slots {
            let d = self.tile_for(i);
            if self.tiles.row_data(s).as_ref() != Some(&d) {
                self.tiles.set_row_data(s, d);
                self.stats.slot_writes += 1;
            }
        }
    }

    fn ensure_pages(&mut self, a: usize, b: usize) {
        let (pa, pb) = (a / PAGE, b / PAGE);
        // prefetch one page in the scroll direction
        let mut want: Vec<usize> = (pa..=pb).collect();
        let last = self.total.saturating_sub(1) / PAGE;
        if self.dir >= 0 && pb < last {
            want.push(pb + 1);
        } else if self.dir < 0 && pa > 0 {
            want.push(pa - 1);
        }
        self.clock += 1;
        for &p in &want {
            if let Some(pg) = self.pages.get_mut(&p) {
                pg.used = self.clock;
            }
        }
        for p in want {
            if self.page_inflight >= PAGE_CONCURRENCY {
                break;
            }
            let state = self.pages.get(&p).map(|pg| pg.state);
            // failed pages are retried by a reload (retry button / server event), not every frame
            if matches!(
                state,
                Some(PageState::Ok) | Some(PageState::Loading) | Some(PageState::Error)
            ) {
                continue;
            }
            self.load_page(p);
        }
        // evict least recently used pages beyond the budget (never the ones in view)
        if self.pages.len() > MAX_PAGES {
            let mut keys: Vec<(u64, usize)> = self
                .pages
                .iter()
                .filter(|(k, p)| !(pa..=pb).contains(*k) && p.state != PageState::Loading)
                .map(|(k, p)| (p.used, *k))
                .collect();
            keys.sort();
            for (_, k) in keys.into_iter().take(self.pages.len() - MAX_PAGES) {
                self.pages.remove(&k);
            }
        }
    }

    fn load_page(&mut self, p: usize) {
        let epoch = self.epoch;
        self.page_inflight += 1;
        let keep = self.pages.remove(&p).map(|pg| pg.items).unwrap_or_default();
        self.pages.insert(
            p,
            Page {
                items: keep,
                state: PageState::Loading,
                used: self.clock,
            },
        );
        let q = BrowseQuery {
            offset: Some((p * PAGE) as i64),
            limit: Some(PAGE as i64),
            ..self.query.clone()
        };
        let fut = self.backend.browse(q);
        let weak = self.weak.clone();
        self.rt.spawn_then(
            async move {
                let r = fut.await?;
                // thumbhash placeholders decoded here, off the UI thread
                let entries: Vec<Entry> = r
                    .items
                    .into_iter()
                    .map(|item| {
                        let hash = item.thumbhash.as_deref().and_then(thumbs::thumbhash_pixels);
                        Entry { item, hash }
                    })
                    .collect();
                Ok::<_, BackendError>((entries, r.gen))
            },
            move |r| {
                let Some(ctl) = weak.upgrade() else { return };
                let mut c = ctl.borrow_mut();
                if c.epoch != epoch {
                    return;
                }
                c.page_inflight = c.page_inflight.saturating_sub(1);
                match r {
                    Ok((entries, gen)) => {
                        c.stats.pages_loaded += 1;
                        let n = entries.len();
                        let clock = c.clock;
                        c.pages.insert(
                            p,
                            Page {
                                items: entries,
                                state: PageState::Ok,
                                used: clock,
                            },
                        );
                        c.refresh_range(p * PAGE, p * PAGE + n);
                        // a page from a newer generation: the structure changed → new skeleton
                        if gen.is_some_and(|g| g != c.gen) {
                            c.load_skeleton();
                        }
                        c.update();
                        c.notify_listeners();
                    }
                    Err(e) => {
                        log::warn!("page {p}: {e}");
                        if let Some(pg) = c.pages.get_mut(&p) {
                            pg.state = PageState::Error;
                        }
                    }
                }
            },
        );
    }

    fn request_thumbs(&mut self, items: &[usize]) {
        // nearest to the viewport centre first
        let mid = self.scroll + self.view_h / 2.0;
        let mut todo: Vec<(f64, String, u32)> = Vec::new();
        for &i in items {
            let Some(e) = self.entry(i) else { continue };
            if e.item.kind == MediaKind::Audio || e.item.kind == MediaKind::Archive {
                continue;
            }
            let r = self.layout.item_row[i] as usize;
            let size = self.thumb_size(self.layout.row_h[r]);
            let id = e.item.id.clone();
            if self.thumb_failed.contains(&id) || self.thumb_inflight.contains(&(id.clone(), size))
            {
                continue;
            }
            if self.thumbs.get(&id, size).is_some() {
                continue;
            }
            todo.push(((self.layout.row_top[r] - mid).abs(), id, size));
        }
        todo.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, id, size) in todo {
            self.load_thumb(id, size);
        }
    }

    fn load_thumb(&mut self, id: String, size: u32) {
        self.thumb_inflight.insert((id.clone(), size));
        let backend = self.backend.clone();
        let sem = self.thumb_sem.clone();
        let vis = self.visible_ids.clone();
        let weak = self.weak.clone();
        let id2 = id.clone();
        self.rt.spawn_then(
            async move {
                let _permit = sem.acquire_owned().await.ok()?;
                // scrolled away while waiting: skip (it is requested again when it comes back)
                if !vis.lock().unwrap().contains(&id2) {
                    return Some(None);
                }
                let data = match backend.thumb(id2, size).await {
                    Ok(d) => d,
                    Err(_) => return None,
                };
                Some(
                    tokio::task::spawn_blocking(move || thumbs::decode(data))
                        .await
                        .ok()
                        .flatten(),
                )
            },
            move |r: Option<Option<Pixels>>| {
                let Some(ctl) = weak.upgrade() else { return };
                let mut c = ctl.borrow_mut();
                c.thumb_inflight.remove(&(id.clone(), size));
                match r {
                    Some(Some(px)) => {
                        c.stats.thumbs_loaded += 1;
                        c.thumbs.put(id.clone(), size, px);
                        if let Some(i) = c.index_of(&id) {
                            c.refresh_range(i, i + 1);
                        }
                    }
                    Some(None) => {}
                    None => {
                        c.thumb_failed.insert(id);
                    }
                }
            },
        );
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.item_slot
            .keys()
            .copied()
            .find(|&i| self.entry(i).is_some_and(|e| e.item.id == id))
    }

    /// The item at a grid index (if its page is loaded).
    pub fn item(&self, i: usize) -> Option<&MediaItem> {
        self.entry(i).map(|e| &e.item)
    }

    /// A `media` server event: structure changes refetch the skeleton, thumbnail updates retry
    /// the failed thumbnails, other changes refresh the loaded pages.
    pub fn on_media_event(&mut self, reason: Option<pa_core::MediaReason>) {
        use pa_core::MediaReason as R;
        match reason {
            Some(R::Thumb) => {
                self.thumb_failed.clear();
                for p in self.pages.values_mut() {
                    if p.state == PageState::Ok {
                        p.state = PageState::Stale;
                    }
                }
                self.update();
            }
            Some(R::Change) => {
                for p in self.pages.values_mut() {
                    if p.state == PageState::Ok {
                        p.state = PageState::Stale;
                    }
                }
                self.update();
            }
            _ => self.load_skeleton(),
        }
    }

    /// visible tile slots / thumbnail cache (perf log)
    pub fn debug_counts(&self) -> (usize, usize, usize, usize) {
        (
            self.slot_item.iter().filter(|s| s.is_some()).count(),
            self.slot_item.len(),
            self.thumbs.len(),
            self.thumbs.bytes() >> 20,
        )
    }

    /// is the skeleton loaded
    pub fn ready(&self) -> bool {
        self.ready
    }

    /// grid content height
    pub fn height(&self) -> f64 {
        self.layout.height
    }
}
