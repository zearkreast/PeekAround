//! Exact grid layout engine (mirrors `features/gallery/grid/layout.ts`).
//!
//! Lays out EVERY item of a browse result up front from the skeleton aspect bytes (no item data
//! needed), so the total height, every row and every tile position are known before a single page
//! is loaded. Flat vectors keep 100k items in a few milliseconds and a few MB.

use pa_core::{byte_to_aspect, SkeletonDay};

/// Group headers by day or month (None: no headers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    /// one header per day
    Day,
    /// consecutive days merged into months
    Month,
}

/// A header group.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutGroup {
    /// 'YYYY-MM-DD' (day) or 'YYYY-MM' (month); '' when ungrouped
    pub key: String,
    /// dominant place
    pub place: Option<String>,
    /// first item
    pub start: usize,
    /// items
    pub count: usize,
    /// row index of the header
    pub row: usize,
    /// y of the header
    pub top: f64,
}

/// Layout result.
#[derive(Debug, Clone, Default)]
pub struct GridLayout {
    /// items
    pub total: usize,
    /// layout width
    pub width: f64,
    /// total height
    pub height: f64,
    /// row tops
    pub row_top: Vec<f64>,
    /// row heights
    pub row_h: Vec<f32>,
    /// first item of a row (header rows: first item of their group)
    pub row_start: Vec<u32>,
    /// exclusive end (header rows: == row_start)
    pub row_end: Vec<u32>,
    /// header row
    pub row_header: Vec<bool>,
    /// group of a row
    pub row_group: Vec<u32>,
    /// x of an item
    pub item_x: Vec<f32>,
    /// width of an item
    pub item_w: Vec<f32>,
    /// row of an item
    pub item_row: Vec<u32>,
    /// groups
    pub groups: Vec<LayoutGroup>,
    /// grouping actually applied
    pub group_by: Option<GroupBy>,
}

/// Layout options.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutOptions {
    /// available width
    pub width: f64,
    /// target row height (justified) or tile size (square)
    pub target: f64,
    /// gap between tiles
    pub gap: f64,
    /// uniform square tiles
    pub square: bool,
    /// headers
    pub group_by: Option<GroupBy>,
    /// header row height
    pub header_h: f64,
}

const MIN_A: f64 = 0.42;
const MAX_A: f64 = 3.2;

fn aspect_lut() -> &'static [f64; 256] {
    static LUT: std::sync::OnceLock<[f64; 256]> = std::sync::OnceLock::new();
    LUT.get_or_init(|| {
        let mut l = [1.0; 256];
        for (b, v) in l.iter_mut().enumerate() {
            *v = byte_to_aspect(b as u8).clamp(MIN_A, MAX_A);
        }
        l
    })
}

/// `Math.round`
#[inline]
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// A day group (owned copy of the skeleton's).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Day {
    /// key
    pub key: String,
    /// items
    pub count: usize,
    /// place
    pub place: Option<String>,
}

impl From<&SkeletonDay> for Day {
    fn from(d: &SkeletonDay) -> Day {
        Day {
            key: d.key.clone(),
            count: d.count.max(0) as usize,
            place: d.place.clone(),
        }
    }
}

/// Merge consecutive day groups into month groups.
pub fn month_groups(days: &[Day]) -> Vec<Day> {
    let mut out: Vec<Day> = Vec::new();
    for d in days {
        let k = d.key.get(..7).unwrap_or(&d.key);
        match out.last_mut() {
            Some(last) if last.key == k => {
                last.count += d.count;
                if last.place.is_none() {
                    last.place = d.place.clone();
                }
            }
            _ => out.push(Day {
                key: k.to_string(),
                count: d.count,
                place: d.place.clone(),
            }),
        }
    }
    out
}

struct Builder<'a> {
    l: GridLayout,
    aspects: &'a [u8],
    y: f64,
    width: f64,
    gap: f64,
}

impl Builder<'_> {
    fn aspect(&self, i: usize) -> f64 {
        aspect_lut()[*self.aspects.get(i).unwrap_or(&128) as usize]
    }

    fn push_row(&mut self, top: f64, h: f64, s: usize, e: usize, header: bool, g: usize) -> usize {
        let l = &mut self.l;
        l.row_top.push(top);
        l.row_h.push(h as f32);
        l.row_start.push(s as u32);
        l.row_end.push(e as u32);
        l.row_header.push(header);
        l.row_group.push(g as u32);
        l.row_top.len() - 1
    }

    fn finish_row(&mut self, s: usize, e: usize, h: f64, stretch: bool, g: usize) {
        let hh = js_round(h).max(1.0);
        let r = self.push_row(self.y, hh, s, e, false, g);
        let n = (e - s) as f64;
        let available = self.width - self.gap * (n - 1.0);
        let mut x = 0.0;
        let mut used = 0.0;
        for i in s..e {
            let mut w = self.aspect(i) * h;
            w = if stretch && i == e - 1 {
                available - used
            } else {
                js_round(w)
            };
            used += w;
            self.l.item_x[i] = x as f32;
            self.l.item_w[i] = w as f32;
            self.l.item_row[i] = r as u32;
            x += w + self.gap;
        }
        self.y += hh + self.gap;
    }

    fn justified(&mut self, start: usize, end: usize, target: f64, g: usize) {
        let mut rs = start;
        let mut sum = 0.0;
        for i in start..end {
            sum += self.aspect(i);
            let n = (i - rs + 1) as f64;
            if sum * target + self.gap * (n - 1.0) >= self.width {
                self.finish_row(
                    rs,
                    i + 1,
                    (self.width - self.gap * (n - 1.0)) / sum,
                    true,
                    g,
                );
                rs = i + 1;
                sum = 0.0;
            }
        }
        if rs < end {
            let n = (end - rs) as f64;
            self.finish_row(
                rs,
                end,
                target.min((self.width - self.gap * (n - 1.0)) / sum),
                false,
                g,
            );
        }
    }
}

/// Lay out `total` items (aspect bytes) with optional day groups.
pub fn layout_grid(
    aspects: &[u8],
    total: usize,
    days: Option<&[Day]>,
    o: &LayoutOptions,
) -> GridLayout {
    let width = o.width.max(0.0);
    let gap = o.gap;
    let grouped = o.group_by.is_some() && days.is_some_and(|d| !d.is_empty());
    let groups: Vec<Day> = match (grouped, o.group_by) {
        (true, Some(GroupBy::Month)) => month_groups(days.unwrap()),
        (true, _) => days.unwrap().to_vec(),
        _ => vec![Day {
            key: String::new(),
            count: total,
            place: None,
        }],
    };
    let cap = total + groups.len() + 1;
    let mut b = Builder {
        l: GridLayout {
            total,
            width,
            item_x: vec![0.0; total],
            item_w: vec![0.0; total],
            item_row: vec![0; total],
            row_top: Vec::with_capacity(cap / 3),
            ..Default::default()
        },
        aspects,
        y: 0.0,
        width,
        gap,
    };

    if width > 0.0 && total > 0 {
        let mut start = 0usize;
        let cols = js_round((width + gap) / (o.target + gap)).max(2.0);
        let size = (width - gap * (cols - 1.0)) / cols;
        let cols_n = cols as usize;
        for (g, grp) in groups.iter().enumerate() {
            let count = grp.count.min(total - start);
            if count == 0 {
                break;
            }
            let end = start + count;
            if grouped {
                let top = b.y;
                let r = b.push_row(top, o.header_h, start, start, true, g);
                b.l.groups.push(LayoutGroup {
                    key: grp.key.clone(),
                    place: grp.place.clone(),
                    start,
                    count,
                    row: r,
                    top,
                });
                b.y += o.header_h;
            } else {
                let row = b.l.row_top.len();
                b.l.groups.push(LayoutGroup {
                    key: String::new(),
                    place: None,
                    start,
                    count,
                    row,
                    top: b.y,
                });
            }
            if o.square {
                let sz = js_round(size);
                let mut s = start;
                while s < end {
                    let e = end.min(s + cols_n);
                    let r = b.push_row(b.y, sz, s, e, false, g);
                    for i in s..e {
                        let k = (i - s) as f64;
                        let x0 = js_round(k * (size + gap));
                        let x1 = if i - s == cols_n - 1 {
                            width
                        } else {
                            js_round(k * (size + gap) + size)
                        };
                        b.l.item_x[i] = x0 as f32;
                        b.l.item_w[i] = (x1 - x0) as f32;
                        b.l.item_row[i] = r as u32;
                    }
                    b.y += sz + gap;
                    s = e;
                }
            } else {
                b.justified(start, end, o.target, g);
            }
            start = end;
        }
        // items the skeleton groups did not cover (defensive): one more ungrouped block
        if start < total {
            b.justified(start, total, o.target, groups.len() - 1);
        }
    }
    let mut l = b.l;
    let rows = l.row_top.len();
    l.height = if rows > 0 {
        l.row_top[rows - 1] + l.row_h[rows - 1] as f64
    } else {
        0.0
    };
    l.group_by = if grouped { o.group_by } else { None };
    l
}

impl GridLayout {
    /// rows
    pub fn row_count(&self) -> usize {
        self.row_top.len()
    }

    /// Index of the last row whose top is <= y (0 when y is above the first row, None without rows).
    pub fn row_at(&self, y: f64) -> Option<usize> {
        if self.row_top.is_empty() {
            return None;
        }
        Some(self.row_top.partition_point(|&t| t <= y).saturating_sub(1))
    }

    /// Item under a point (layout coordinates).
    pub fn item_at(&self, x: f64, y: f64) -> Option<usize> {
        let r = self.row_at(y)?;
        if self.row_header[r] || y < self.row_top[r] || y > self.row_top[r] + self.row_h[r] as f64 {
            return None;
        }
        (self.row_start[r] as usize..self.row_end[r] as usize)
            .find(|&i| x >= self.item_x[i] as f64 && x < (self.item_x[i] + self.item_w[i]) as f64)
    }

    /// Nearest item to a point, also when the point is in a gap / header (range selection).
    pub fn nearest_item(&self, x: f64, y: f64) -> Option<usize> {
        if self.total == 0 {
            return None;
        }
        let mut r = match self.row_at(y) {
            None => return Some(0),
            Some(r) => r,
        };
        while r < self.row_count() && self.row_header[r] {
            r += 1;
        }
        if r >= self.row_count() {
            return Some(self.total - 1);
        }
        let (s, e) = (self.row_start[r] as usize, self.row_end[r] as usize);
        Some(
            (s..e)
                .find(|&i| x < (self.item_x[i] + self.item_w[i]) as f64)
                .unwrap_or(e - 1),
        )
    }

    /// Top of an item's row.
    pub fn item_top(&self, i: usize) -> f64 {
        self.item_row
            .get(i)
            .map(|&r| self.row_top[r as usize])
            .unwrap_or(0.0)
    }
}

/// Density levels: 0 big … 3 month overview (MediaGrid.tsx gridParams). Phones are sized by width,
/// desktop by the row-height preference.
pub fn grid_params(width: f64, level: i32, row_height: f64, square: bool) -> LayoutOptions {
    let p = |target: f64, square: bool, gap: f64, group_by: GroupBy, header_h: f64| LayoutOptions {
        width,
        target,
        gap,
        square,
        group_by: Some(group_by),
        header_h,
    };
    if width < 700.0 {
        return match level {
            0 => p(width / 2.05, square, 2.0, GroupBy::Day, 50.0),
            2 => p(width / 5.2, true, 2.0, GroupBy::Day, 44.0),
            3 => p(width / 8.5, true, 1.0, GroupBy::Month, 46.0),
            _ => p((width / 3.3).max(96.0), square, 2.0, GroupBy::Day, 48.0),
        };
    }
    match level {
        0 => p(row_height * 1.6, square, 6.0, GroupBy::Day, 62.0),
        2 => p((row_height * 0.55).max(72.0), true, 3.0, GroupBy::Day, 54.0),
        3 => p(56.0, true, 2.0, GroupBy::Month, 54.0),
        _ => p(row_height, square, 6.0, GroupBy::Day, 60.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pa_core::aspect_to_byte;

    fn bytes(a: &[f64]) -> Vec<u8> {
        a.iter().map(|&x| aspect_to_byte(x)).collect()
    }
    const OPTS: LayoutOptions = LayoutOptions {
        width: 1000.0,
        target: 200.0,
        gap: 4.0,
        square: false,
        group_by: None,
        header_h: 50.0,
    };

    #[test]
    fn places_every_item_and_fills_full_rows() {
        let a = bytes(&[
            1.5, 0.66, 1.0, 1.78, 1.33, 0.75, 2.0, 1.0, 1.5, 0.8, 1.0, 1.0,
        ]);
        let l = layout_grid(&a, a.len(), None, &OPTS);
        let mut placed = 0;
        for r in 0..l.row_count() {
            if l.row_header[r] {
                continue;
            }
            let (s, e) = (l.row_start[r] as usize, l.row_end[r] as usize);
            placed += e - s;
            if r < l.row_count() - 1 {
                assert!(((l.item_x[e - 1] + l.item_w[e - 1]) as f64 - 1000.0).abs() < 0.5);
            }
            for i in s..e {
                assert_eq!(l.item_row[i] as usize, r);
            }
        }
        assert_eq!(placed, a.len());
        assert_eq!(
            l.height,
            l.row_top[l.row_count() - 1] + l.row_h[l.row_count() - 1] as f64
        );
    }

    #[test]
    fn header_per_day_rows_inside_groups() {
        let days = vec![
            Day {
                key: "2024-07-03".into(),
                count: 3,
                place: Some("Kadıköy, İstanbul".into()),
            },
            Day {
                key: "2024-07-01".into(),
                count: 5,
                place: None,
            },
            Day {
                key: "2024-06-30".into(),
                count: 1,
                place: None,
            },
        ];
        let a = bytes(&[1.5; 9]);
        let l = layout_grid(
            &a,
            9,
            Some(&days),
            &LayoutOptions {
                group_by: Some(GroupBy::Day),
                ..OPTS
            },
        );
        assert_eq!(
            l.groups.iter().map(|g| g.key.as_str()).collect::<Vec<_>>(),
            ["2024-07-03", "2024-07-01", "2024-06-30"]
        );
        assert_eq!(l.groups[0].place.as_deref(), Some("Kadıköy, İstanbul"));
        assert!(l.row_header[l.groups[1].row]);
        for i in 3..8 {
            let t = l.row_top[l.item_row[i] as usize];
            assert!(t > l.groups[1].top && t < l.groups[2].top);
        }
        for r in 1..l.row_count() {
            assert!(l.row_top[r] > l.row_top[r - 1]);
        }
    }

    #[test]
    fn months() {
        let m = month_groups(&[
            Day {
                key: "2024-07-03".into(),
                count: 3,
                place: None,
            },
            Day {
                key: "2024-07-01".into(),
                count: 2,
                place: Some("Bodrum".into()),
            },
            Day {
                key: "2024-06-30".into(),
                count: 1,
                place: None,
            },
        ]);
        assert_eq!(
            m,
            vec![
                Day {
                    key: "2024-07".into(),
                    count: 5,
                    place: Some("Bodrum".into())
                },
                Day {
                    key: "2024-06".into(),
                    count: 1,
                    place: None
                }
            ]
        );
    }

    #[test]
    fn square_mode() {
        let l = layout_grid(
            &bytes(&[0.5; 10]),
            10,
            None,
            &LayoutOptions {
                square: true,
                target: 240.0,
                ..OPTS
            },
        );
        assert_eq!(l.row_end[0] - l.row_start[0], 4);
        assert_eq!(l.item_x[3] + l.item_w[3], 1000.0);
    }

    #[test]
    fn hit_testing() {
        let l = layout_grid(&bytes(&[1.0; 20]), 20, None, &OPTS);
        assert_eq!(l.row_at(l.row_top[2] + 5.0), Some(2));
        let s = l.row_start[2] as usize;
        assert_eq!(
            l.item_at(l.item_x[s] as f64 + 2.0, l.row_top[2] + 5.0),
            Some(s)
        );
        assert_eq!(l.item_at(5.0, -10.0), None);
        assert_eq!(l.nearest_item(99999.0, 99999999.0), Some(19));
    }

    #[test]
    fn hundred_k_items_fast() {
        let n = 100_000;
        let a: Vec<u8> = (0..n).map(|i| ((i * 37) % 255) as u8).collect();
        let mut days = Vec::new();
        let (mut left, mut d) = (n, 0);
        while left > 0 {
            let c = left.min(1 + d % 40);
            days.push(Day {
                key: format!("20{:02}-0{}-1{}", 10 + d % 15, 1 + d % 9, d % 9),
                count: c,
                place: None,
            });
            left -= c;
            d += 1;
        }
        let t0 = std::time::Instant::now();
        let l = layout_grid(
            &a,
            n,
            Some(&days),
            &LayoutOptions {
                group_by: Some(GroupBy::Day),
                ..OPTS
            },
        );
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(l.total, n);
        assert!(l.height > 0.0);
        // unoptimized test builds: generous bound
        assert!(ms < 400.0, "{ms} ms");
    }
}
