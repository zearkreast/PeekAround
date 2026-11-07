//! Selection model of the media grids (`store/selection.ts` + the click / range / lasso rules of
//! `MediaGrid.tsx`): selected items by id (+ the items themselves, so actions can run on them
//! even after their page was evicted), the range anchor, and the pure helpers the grid uses.

use std::collections::HashMap;

use pa_core::MediaItem;

/// What a plain / modified click on a tile does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickAction {
    /// open the viewer
    Open,
    /// toggle this tile
    Toggle,
    /// select the range anchor..=index (adds to the selection)
    Range(usize, usize),
}

/// The selection.
#[derive(Debug, Default, Clone)]
pub struct Selection {
    items: HashMap<String, MediaItem>,
    /// insertion order (actions run in the order the user picked)
    order: Vec<String>,
    /// last clicked index (Shift+click ranges start here)
    pub anchor: Option<usize>,
}

impl Selection {
    /// number of selected items
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// nothing selected
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// is `id` selected
    pub fn contains(&self, id: &str) -> bool {
        self.items.contains_key(id)
    }

    /// selected items in selection order
    pub fn items(&self) -> Vec<MediaItem> {
        self.order
            .iter()
            .filter_map(|id| self.items.get(id).cloned())
            .collect()
    }

    /// selected ids in selection order
    pub fn ids(&self) -> Vec<String> {
        self.order.clone()
    }

    /// clear everything (anchor too)
    pub fn clear(&mut self) {
        self.items.clear();
        self.order.clear();
        self.anchor = None;
    }

    /// add (archives are never selectable)
    pub fn add(&mut self, list: impl IntoIterator<Item = MediaItem>) {
        for it in list {
            if it.kind == pa_core::MediaKind::Archive {
                continue;
            }
            if !self.items.contains_key(&it.id) {
                self.order.push(it.id.clone());
            }
            self.items.insert(it.id.clone(), it);
        }
    }

    /// remove by id
    pub fn remove(&mut self, ids: &[String]) {
        for id in ids {
            self.items.remove(id);
        }
        self.order.retain(|id| self.items.contains_key(id));
    }

    /// replace the whole selection
    pub fn set(&mut self, list: impl IntoIterator<Item = MediaItem>) {
        self.items.clear();
        self.order.clear();
        self.add(list);
    }

    /// toggle one item; sets the anchor
    pub fn toggle(&mut self, item: &MediaItem, index: usize) {
        if self.items.contains_key(&item.id) {
            self.remove(std::slice::from_ref(&item.id));
        } else {
            self.add([item.clone()]);
        }
        self.anchor = Some(index);
    }

    /// keep the stored copies in sync with patched items (favourite toggled…)
    pub fn patch(&mut self, f: impl Fn(&mut MediaItem)) {
        for it in self.items.values_mut() {
            f(it);
        }
    }

    /// The grid's click rule: Shift (with an anchor) → range, Ctrl/Cmd, the check circle or an
    /// active selection → toggle, otherwise open.
    pub fn click_action(
        &self,
        index: usize,
        ctrl: bool,
        shift: bool,
        on_check: bool,
    ) -> ClickAction {
        if shift {
            if let Some(a) = self.anchor {
                return ClickAction::Range(a.min(index), a.max(index));
            }
        }
        if ctrl || on_check || !self.is_empty() {
            ClickAction::Toggle
        } else {
            ClickAction::Open
        }
    }
}

/// Normalized lasso rectangle (x0, y0, x1, y1) from two corners.
pub fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> (f64, f64, f64, f64) {
    (x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1))
}

/// Indices of the tiles a lasso rectangle (grid coordinates) touches.
pub fn lasso_hits(l: &crate::grid::layout::GridLayout, r: (f64, f64, f64, f64)) -> Vec<usize> {
    let (min_x, min_y, max_x, max_y) = r;
    let mut out = Vec::new();
    if max_x - min_x < 4.0 && max_y - min_y < 4.0 {
        return out;
    }
    let Some(mut row) = l.row_at(min_y.max(0.0)) else {
        return out;
    };
    while row < l.row_count() && l.row_top[row] < max_y {
        if !l.row_header[row] && l.row_top[row] + l.row_h[row] as f64 >= min_y {
            for i in l.row_start[row] as usize..l.row_end[row] as usize {
                let (x, w) = (l.item_x[i] as f64, l.item_w[i] as f64);
                if x < max_x && x + w > min_x {
                    out.push(i);
                }
            }
        }
        row += 1;
    }
    out
}

/// Keyboard focus movement in a justified grid: ←/→ step, ↑/↓ pick the tile of the neighbouring
/// (non-header) row under the current tile's centre.
pub fn key_step(l: &crate::grid::layout::GridLayout, idx: usize, key: KeyStep) -> usize {
    if l.total == 0 {
        return 0;
    }
    let idx = idx.min(l.total - 1);
    match key {
        KeyStep::Right => (idx + 1).min(l.total - 1),
        KeyStep::Left => idx.saturating_sub(1),
        KeyStep::Down | KeyStep::Up => {
            let d: i64 = if key == KeyStep::Down { 1 } else { -1 };
            let mut r = l.item_row[idx] as i64 + d;
            while r >= 0 && (r as usize) < l.row_count() && l.row_header[r as usize] {
                r += d;
            }
            if r < 0 || r as usize >= l.row_count() {
                return idx;
            }
            let r = r as usize;
            let cx = l.item_x[idx] + l.item_w[idx] / 2.0;
            let mut best = l.row_start[r] as usize;
            for i in l.row_start[r] as usize..l.row_end[r] as usize {
                if l.item_x[i] <= cx {
                    best = i;
                }
            }
            best
        }
        KeyStep::Home => 0,
        KeyStep::End => l.total - 1,
    }
}

/// A focus movement key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyStep {
    /// →
    Right,
    /// ←
    Left,
    /// ↓
    Down,
    /// ↑
    Up,
    /// Home
    Home,
    /// End
    End,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::layout::{layout_grid, Day, GroupBy, LayoutOptions};

    fn item(i: usize) -> MediaItem {
        MediaItem {
            id: format!("m{i}"),
            folder_id: "f".into(),
            dir: String::new(),
            name: format!("{i}.jpg"),
            ext: "jpg".into(),
            kind: pa_core::MediaKind::Image,
            size: 1,
            mtime: 0,
            width: Some(100),
            height: Some(100),
            duration: None,
            taken_at: 0,
            local_at: 0,
            flags: 0,
            thumbhash: None,
            favorite: false,
            playback: pa_core::Playback::Direct,
        }
    }

    #[test]
    fn click_rules() {
        let mut s = Selection::default();
        assert_eq!(s.click_action(3, false, false, false), ClickAction::Open);
        assert_eq!(s.click_action(3, true, false, false), ClickAction::Toggle);
        assert_eq!(s.click_action(3, false, false, true), ClickAction::Toggle);
        // shift without an anchor: a plain click
        assert_eq!(s.click_action(3, false, true, false), ClickAction::Open);
        s.toggle(&item(3), 3);
        assert_eq!(s.anchor, Some(3));
        assert_eq!(s.click_action(8, false, false, false), ClickAction::Toggle);
        assert_eq!(
            s.click_action(1, false, true, false),
            ClickAction::Range(1, 3)
        );
        assert_eq!(
            s.click_action(9, false, true, false),
            ClickAction::Range(3, 9)
        );
        s.toggle(&item(3), 3);
        assert!(s.is_empty());
    }

    #[test]
    fn add_remove_order() {
        let mut s = Selection::default();
        s.add([item(2), item(1), item(2)]);
        assert_eq!(s.ids(), vec!["m2", "m1"]);
        let mut a = item(5);
        a.kind = pa_core::MediaKind::Archive;
        s.add([a]);
        assert_eq!(s.len(), 2);
        s.remove(&["m2".to_string()]);
        assert_eq!(s.ids(), vec!["m1"]);
        s.patch(|m| m.favorite = true);
        assert!(s.items()[0].favorite);
        s.set([item(7)]);
        assert_eq!(s.ids(), vec!["m7"]);
    }

    fn grid() -> crate::grid::layout::GridLayout {
        // 2 days × 6 square tiles, 3 per row
        let days = vec![
            Day {
                key: "2024-07-05".into(),
                count: 6,
                place: None,
            },
            Day {
                key: "2024-07-04".into(),
                count: 6,
                place: None,
            },
        ];
        let opts = LayoutOptions {
            width: 300.0,
            target: 100.0,
            gap: 0.0,
            square: true,
            group_by: Some(GroupBy::Day),
            header_h: 40.0,
        };
        layout_grid(&[128; 12], 12, Some(&days), &opts)
    }

    #[test]
    fn lasso_and_keys() {
        let l = grid();
        assert_eq!(l.total, 12);
        // the first two rows of day 1 below its header: 40..240
        let hits = lasso_hits(&l, rect(10.0, 50.0, 150.0, 150.0));
        assert!(hits.contains(&0) && hits.contains(&1) && hits.contains(&3) && hits.contains(&4));
        assert!(!hits.contains(&2) && !hits.contains(&6));
        // a click-sized lasso hits nothing
        assert!(lasso_hits(&l, rect(10.0, 50.0, 12.0, 52.0)).is_empty());
        // ↓ from the last row of day 1 skips the header of day 2
        let down = key_step(&l, 4, KeyStep::Down);
        assert_eq!(down, 7);
        assert_eq!(key_step(&l, 7, KeyStep::Up), 4);
        assert_eq!(key_step(&l, 0, KeyStep::Up), 0);
        assert_eq!(key_step(&l, 11, KeyStep::Right), 11);
        assert_eq!(key_step(&l, 0, KeyStep::Left), 0);
        assert_eq!(key_step(&l, 5, KeyStep::End), 11);
    }
}
