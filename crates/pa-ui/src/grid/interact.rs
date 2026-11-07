//! Grid interaction state + the item / geometry accessors the viewer, the selection, the context
//! menu, drag & drop, keyboard navigation and the fast scroller use (the `GridItems` half of
//! `grid/source.ts`: getAt / indexOf / ensureRange / itemsInRange, plus MediaGrid's scrollToIndex,
//! labelAt and the year ticks). The pointer / key handling itself lives in `actions_ui.rs`.

use std::rc::Rc;

use slint::{ComponentHandle, Image};

use super::{BrowseQuery, Entry, GridCtl, Page, PageState, PAGE};
use crate::backend::BackendError;
use crate::i18n;
use crate::selection::Selection;
use crate::thumbs;
use pa_core::MediaItem;

/// Per-grid interaction state (lives in [`GridCtl`]).
#[derive(Default)]
pub(crate) struct Interact {
    /// selected items
    pub selection: Selection,
    /// keyboard focus (tile index)
    pub focus: Option<usize>,
    /// hovered tile (mouse)
    pub hover: Option<usize>,
    /// draw the focus ring (keyboard navigation, not after a click: `:focus-visible`)
    pub ring: bool,
    /// window position of the grid's content origin, measured while the grid was scrolled to
    /// `origin_scroll` (the grid moves up by the scroll delta since)
    pub origin: (f64, f64),
    /// scroll offset at the time `origin` was measured
    pub origin_scroll: f64,
    /// notified (deferred, on the UI thread) after pages arrive / the structure changes
    pub listeners: Vec<Rc<dyn Fn()>>,
    /// item ids of the last published selection (avoid rewriting every slot for nothing)
    published_sel: usize,
}

impl GridCtl {
    /// items in the query
    pub fn total(&self) -> usize {
        self.total
    }

    /// the item at index `i` (loaded pages only), cloned
    pub fn get_at(&self, i: usize) -> Option<MediaItem> {
        self.entry(i).map(|e| e.item.clone())
    }

    /// index of a loaded item
    pub fn index_of_id(&self, id: &str) -> Option<usize> {
        if let Some(&i) = self
            .item_slot
            .keys()
            .find(|&&i| self.entry(i).is_some_and(|e| e.item.id == id))
        {
            return Some(i);
        }
        self.pages.iter().find_map(|(p, pg)| {
            pg.items
                .iter()
                .position(|e| e.item.id == id)
                .map(|k| p * PAGE + k)
        })
    }

    /// Load the pages of `[a, b]` (also outside the viewport: viewer neighbours, ranges).
    pub fn ensure_range(&mut self, a: usize, b: usize) {
        if self.total == 0 || !self.ready {
            return;
        }
        let b = b.min(self.total - 1);
        self.clock += 1;
        for p in a / PAGE..=b / PAGE {
            match self.pages.get_mut(&p) {
                Some(pg) if matches!(pg.state, PageState::Ok | PageState::Loading) => {
                    pg.used = self.clock;
                }
                _ => self.load_page(p),
            }
        }
    }

    /// All items of `[a, b]`, loading missing pages first (Shift ranges, "select all", day
    /// selection across unloaded items). `then` runs on the UI thread.
    pub fn items_in_range(
        &mut self,
        a: usize,
        b: usize,
        then: impl FnOnce(Vec<MediaItem>) + 'static,
    ) {
        // `then` never runs inside this borrow of the grid
        let defer = |v: Vec<MediaItem>, then: Box<dyn FnOnce(Vec<MediaItem>)>| {
            slint::Timer::single_shot(std::time::Duration::ZERO, move || then(v));
        };
        if self.total == 0 {
            defer(vec![], Box::new(then));
            return;
        }
        let b = b.min(self.total - 1);
        let (a, b) = (a.min(b), b);
        let missing: Vec<usize> = (a / PAGE..=b / PAGE)
            .filter(|p| self.pages.get(p).is_none_or(|pg| pg.state != PageState::Ok))
            .collect();
        if missing.is_empty() {
            let v = (a..=b).filter_map(|i| self.get_at(i)).collect();
            defer(v, Box::new(then));
            return;
        }
        let epoch = self.epoch;
        let query = self.query.clone();
        let backend = self.backend.clone();
        let weak = self.weak.clone();
        self.rt.spawn_then(
            async move {
                let mut out = Vec::new();
                for p in missing {
                    let q = BrowseQuery {
                        offset: Some((p * PAGE) as i64),
                        limit: Some(PAGE as i64),
                        ..query.clone()
                    };
                    let r = backend.browse(q).await?;
                    let entries: Vec<Entry> = r
                        .items
                        .into_iter()
                        .map(|item| {
                            let hash = item.thumbhash.as_deref().and_then(thumbs::thumbhash_pixels);
                            Entry { item, hash }
                        })
                        .collect();
                    out.push((p, entries));
                }
                Ok::<_, BackendError>(out)
            },
            move |r| {
                let Some(ctl) = weak.upgrade() else { return };
                let v = {
                    let mut c = ctl.borrow_mut();
                    if c.epoch != epoch {
                        return;
                    }
                    match r {
                        Ok(pages) => {
                            let clock = c.clock;
                            for (p, items) in pages {
                                c.pages.insert(
                                    p,
                                    Page {
                                        items,
                                        state: PageState::Ok,
                                        used: clock,
                                    },
                                );
                            }
                            (a..=b).filter_map(|i| c.get_at(i)).collect()
                        }
                        Err(e) => {
                            log::warn!("items {a}..={b}: {e}");
                            (a..=b).filter_map(|i| c.get_at(i)).collect()
                        }
                    }
                };
                then(v);
            },
        );
    }

    /// the current layout
    pub fn layout(&self) -> &super::layout::GridLayout {
        &self.layout
    }

    /// tile rectangle (grid coordinates)
    pub fn tile_rect(&self, i: usize) -> Option<(f64, f64, f64, f64)> {
        let l = &self.layout;
        let r = *l.item_row.get(i)? as usize;
        Some((
            l.item_x[i] as f64,
            l.row_top[r],
            l.item_w[i] as f64,
            l.row_h[r] as f64,
        ))
    }

    /// Tile rectangle in window coordinates (None when it is not on screen).
    pub fn tile_window_rect(&self, i: usize) -> Option<(f64, f64, f64, f64)> {
        let (x, y, w, h) = self.tile_rect(i)?;
        let (ox, oy) = self.ix.origin;
        let oy = oy + self.ix.origin_scroll - self.scroll;
        let (wy, view_top) = (oy + y, oy + self.scroll);
        if wy + h < view_top || wy > view_top + self.view_h {
            return None;
        }
        Some((ox + x, wy, w, h))
    }

    /// current scroll offset inside the grid / visible height
    pub fn viewport(&self) -> (f64, f64) {
        (self.scroll, self.view_h)
    }

    /// a cached thumbnail (≥ `size`)
    pub fn cached_thumb(&mut self, id: &str, size: u32) -> Option<Image> {
        self.thumbs.get(id, size)
    }

    /// keep a decoded thumbnail (viewer / filmstrip loads share the grid's cache)
    pub fn put_thumb(&mut self, id: &str, size: u32, px: thumbs::Pixels) -> Image {
        self.thumbs.put(id.to_string(), size, px)
    }

    /// thumbhash placeholder image of a loaded item
    pub fn placeholder_of(&mut self, i: usize) -> Option<Image> {
        let (id, px) = {
            let e = self.entry(i)?;
            (e.item.id.clone(), e.hash.clone()?)
        };
        Some(self.placeholders.get_or_insert(&id, &px))
    }

    /// Scroll so that item `i` is visible (`center`: centre it, else only when out of view).
    pub fn scroll_to_index(&mut self, i: usize, center: bool) {
        let Some((_, y, _, h)) = self.tile_rect(i) else {
            return;
        };
        let (top, vh) = (self.scroll, self.view_h);
        if !center && y >= top + 8.0 && y + h <= top + vh - 8.0 {
            return;
        }
        let target = if center {
            y - (vh - h) / 2.0
        } else if y < top {
            y - 8.0
        } else {
            y + h - vh + 8.0
        };
        let Some(app) = self.app() else { return };
        let gs = app.global::<crate::GridState>();
        // the Flickable's content-y: header above the grid, never past the end
        let header = -(gs.get_scroll_y() as f64) - top;
        let page_y = (target + header).max(0.0);
        gs.set_scroll_y(-(page_y as f32));
    }

    /// Patch loaded copies of items in place (optimistic favourite …) and redraw their tiles.
    pub fn patch_items(&mut self, ids: &[String], f: impl Fn(&mut MediaItem)) {
        let set: std::collections::HashSet<&String> = ids.iter().collect();
        for pg in self.pages.values_mut() {
            for e in pg.items.iter_mut() {
                if set.contains(&e.item.id) {
                    f(&mut e.item);
                }
            }
        }
        self.ix.selection.patch(|m| {
            if set.contains(&m.id) {
                f(m)
            }
        });
        self.refresh_all();
    }

    /// Rewrite every visible slot (selection / focus / hover changed).
    pub fn refresh_all(&mut self) {
        self.refresh_range(0, self.total.max(1));
    }

    /// The selection changed: redraw tiles and publish the count.
    pub fn selection_changed(&mut self) {
        self.refresh_all();
        self.ix.published_sel = self.ix.selection.len();
        if let Some(app) = self.app() {
            let gi = app.global::<crate::GridInput>();
            gi.set_selected_count(self.ix.selection.len() as i32);
            gi.set_selecting(!self.ix.selection.is_empty());
        }
    }

    /// Move the keyboard focus (redraws the two tiles involved).
    pub fn set_focus(&mut self, f: Option<usize>) {
        let old = self.ix.focus;
        if old == f {
            return;
        }
        self.ix.focus = f;
        for i in [old, f].into_iter().flatten() {
            self.refresh_range(i, i + 1);
        }
        if let Some(app) = self.app() {
            app.global::<crate::GridInput>()
                .set_focus_index(f.map(|i| i as i32).unwrap_or(-1));
        }
    }

    /// Hovered tile (mouse).
    pub fn set_hover(&mut self, h: Option<usize>) {
        if self.ix.hover == h {
            return;
        }
        self.ix.hover = h;
        if let Some(app) = self.app() {
            app.global::<crate::GridInput>()
                .set_hover_index(h.map(|i| i as i32).unwrap_or(-1));
        }
    }

    /// Run the listeners (deferred: never inside a borrow of the grid).
    pub(crate) fn notify_listeners(&self) {
        if self.ix.listeners.is_empty() {
            return;
        }
        let ls = self.ix.listeners.clone();
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            for l in &ls {
                l();
            }
        });
    }

    /// Fast scroller label for a grid y (month of the row there, or the sort key's value).
    pub fn label_at(&self, gy: f64) -> String {
        let l = &self.layout;
        let Some(r) = l.row_at(gy.max(0.0)) else {
            return String::new();
        };
        if l.group_by.is_some() {
            return l
                .groups
                .get(l.row_group[r] as usize)
                .map(|g| i18n::format_month_key(&g.key[..g.key.len().min(7)], self.lang))
                .unwrap_or_default();
        }
        let Some(it) = self.entry(l.row_start[r] as usize).map(|e| &e.item) else {
            return "…".into();
        };
        match self.query.sort {
            Some(pa_core::SortKey::Name) => it
                .name
                .chars()
                .next()
                .map(|c| c.to_uppercase().collect())
                .unwrap_or_default(),
            Some(pa_core::SortKey::Size) => crate::fmt::format_bytes(it.size),
            Some(pa_core::SortKey::Mtime) => {
                let d = it.mtime.div_euclid(86_400_000);
                let (y, m, _) = i18n::civil_from_days(d);
                i18n::format_month_key(&format!("{y:04}-{m:02}"), self.lang)
            }
            Some(pa_core::SortKey::Kind) => i18n::t(
                self.lang,
                if it.kind == pa_core::MediaKind::Video {
                    "gallery.filter.video"
                } else {
                    "gallery.filter.image"
                },
            ),
            _ => {
                let d = it.local_at.div_euclid(86_400_000);
                let (y, m, _) = i18n::civil_from_days(d);
                i18n::format_month_key(&format!("{y:04}-{m:02}"), self.lang)
            }
        }
    }

    /// Year marks of the fast scroller: (grid y, "2024") for the first group of every year.
    pub fn year_ticks(&self) -> Vec<(f64, String)> {
        let l = &self.layout;
        let date_sort = matches!(self.query.sort, None | Some(pa_core::SortKey::TakenAt));
        if l.group_by.is_none() || !date_sort {
            return vec![];
        }
        let mut out = Vec::new();
        let mut year = "";
        for g in &l.groups {
            let y = &g.key[..g.key.len().min(4)];
            if y != year {
                year = y;
                out.push((g.top, y.to_string()));
            }
        }
        out
    }

    /// the query shown
    pub fn query(&self) -> &BrowseQuery {
        &self.query
    }
}

impl GridCtl {
    /// Publish the fast scroller's year marks (after a re-layout).
    pub(crate) fn publish_ticks(&self) {
        let Some(app) = self.app() else { return };
        let v: Vec<crate::YearTick> = self
            .year_ticks()
            .into_iter()
            .map(|(y, l)| crate::YearTick {
                y: y as f32,
                label: l.into(),
            })
            .collect();
        app.global::<crate::GridInput>()
            .set_ticks(slint::ModelRc::new(slint::VecModel::from(v)));
    }
}
