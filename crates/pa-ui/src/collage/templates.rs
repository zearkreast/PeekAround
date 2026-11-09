//! Collage layouts (features/collage/templates.ts): each makes `n` cells in normalized content
//! coordinates (0…1). Grid templates tile the content box; free ones float, rotate and overlap
//! (their x / y is the centre).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellStyle {
    Plain,
    Polaroid,
    Print,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellSpec {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// degrees, clockwise
    pub rot: f64,
    pub style: CellStyle,
}

impl CellSpec {
    fn rect(x: f64, y: f64, w: f64, h: f64) -> CellSpec {
        CellSpec { x, y, w, h, rot: 0.0, style: CellStyle::Plain }
    }
}

pub struct Template {
    pub id: &'static str,
    pub label: &'static str,
    pub free: bool,
}

pub const TEMPLATES: [Template; 8] = [
    Template { id: "grid", label: "collage.tpl.grid", free: false },
    Template { id: "mosaic", label: "collage.tpl.mosaic", free: false },
    Template { id: "bigLeft", label: "collage.tpl.bigLeft", free: false },
    Template { id: "bigTop", label: "collage.tpl.bigTop", free: false },
    Template { id: "rows", label: "collage.tpl.rows", free: false },
    Template { id: "columns", label: "collage.tpl.columns", free: false },
    Template { id: "polaroid", label: "collage.tpl.polaroid", free: true },
    Template { id: "scatter", label: "collage.tpl.scatter", free: true },
];

pub fn template(id: &str) -> &'static Template {
    TEMPLATES.iter().find(|t| t.id == id).unwrap_or(&TEMPLATES[0])
}

/// mulberry32, as the JS `rng` (same seed → same scatter as 1.x)
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b_79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        (t ^ (t >> 14)) as f64 / 4_294_967_296.0
    }
}

/// `counts[i]` cells in row i; row heights / cell widths follow the weights.
fn rows_layout(counts: &[usize], row_weights: Option<&[f64]>, cell_weights: Option<&dyn Fn(usize, usize) -> Vec<f64>>) -> Vec<CellSpec> {
    let ones = vec![1.0; counts.len()];
    let rw = row_weights.unwrap_or(&ones);
    let total: f64 = rw.iter().sum();
    let mut out = Vec::new();
    let mut y = 0.0;
    for (r, &n) in counts.iter().enumerate() {
        let h = rw[r] / total;
        let cw = cell_weights.map_or_else(|| vec![1.0; n], |f| f(r, n));
        let ct: f64 = cw.iter().sum();
        let mut x = 0.0;
        for wgt in cw.iter().take(n) {
            let w = wgt / ct;
            out.push(CellSpec::rect(x, y, w, h));
            x += w;
        }
        y += h;
    }
    out
}

fn transpose(cells: Vec<CellSpec>) -> Vec<CellSpec> {
    cells.into_iter().map(|c| CellSpec { x: c.y, y: c.x, w: c.h, h: c.w, ..c }).collect()
}

fn even(n: usize, groups: usize) -> Vec<usize> {
    let base = n / groups;
    let extra = n % groups;
    (0..groups).map(|i| base + usize::from(i < extra)).collect()
}

/// Columns that make cells closest to square for this canvas aspect.
pub fn best_cols(n: usize, aspect: f64) -> usize {
    let mut best = 1;
    let mut score = f64::INFINITY;
    for c in 1..=n {
        let r = n.div_ceil(c);
        let cell = aspect * r as f64 / c as f64;
        let s = cell.ln().abs() + (c * r - n) as f64 * 0.25;
        if s < score {
            score = s;
            best = c;
        }
    }
    best
}

fn grid_cells(n: usize, aspect: f64) -> Vec<CellSpec> {
    let c = best_cols(n, aspect);
    let r = n.div_ceil(c);
    let counts: Vec<usize> = (0..r).map(|i| if i < r - 1 { c } else { n - c * (r - 1) }).collect();
    rows_layout(&counts, None, None)
}

fn scatter(n: usize, aspect: f64, seed: u32, style: CellStyle) -> Vec<CellSpec> {
    let mut rand = Rng(seed.wrapping_mul(97).wrapping_add(n as u32));
    let c = best_cols(n, aspect);
    let r = n.div_ceil(c);
    let polaroid = style == CellStyle::Polaroid;
    let mut out = Vec::new();
    for i in 0..n {
        let row = i / c;
        let in_row = if row == r - 1 { n - c * (r - 1) } else { c };
        let col = i % c;
        let pix_aspect = if polaroid {
            0.84
        } else if i % 3 == 1 {
            0.78
        } else {
            1.3
        };
        let k = if polaroid { 1.02 } else { 1.14 };
        let mut w = k / c as f64;
        let mut h = w * aspect / pix_aspect;
        if h > k / r as f64 {
            h = k / r as f64;
            w = h * pix_aspect / aspect;
        }
        let jx = (rand.next() - 0.5) * (0.22 / c as f64);
        let jy = (rand.next() - 0.5) * (0.22 / r as f64);
        let offset = (c - in_row) as f64 / 2.0;
        // keep each photo inside the canvas (rotation may still nudge a corner out)
        let keep = |v: f64, half: f64| if half >= 0.5 { 0.5 } else { v.clamp(half, 1.0 - half) };
        let x = keep((col as f64 + offset + 0.5) / c as f64 + jx, w / 2.0);
        let y = keep((row as f64 + 0.5) / r as f64 + jy, h / 2.0);
        let rot = (rand.next() - 0.5) * if polaroid { 16.0 } else { 22.0 };
        out.push(CellSpec { x, y, w, h, rot, style });
    }
    out
}

fn mosaic(n: usize) -> Vec<CellSpec> {
    let counts: Vec<usize> = match n {
        2 => vec![2],
        3 => vec![1, 2],
        4 => vec![2, 2],
        5 => vec![2, 3],
        6 => vec![3, 3],
        7 => vec![2, 3, 2],
        8 => vec![3, 2, 3],
        9 => vec![3, 3, 3],
        _ => vec![n],
    };
    let rw: Vec<f64> = (0..counts.len()).map(|i| if i % 2 == 1 { 1.0 } else { 1.25 }).collect();
    let cw = |row: usize, k: usize| -> Vec<f64> {
        match (k, row % 2 == 1) {
            (1, _) => vec![1.0],
            (2, true) => vec![2.0, 3.0],
            (2, false) => vec![3.0, 2.0],
            (_, true) => vec![1.0, 1.5, 1.0],
            (_, false) => vec![1.4, 1.0, 1.2],
        }
    };
    rows_layout(&counts, Some(&rw), Some(&cw))
}

fn big_left(n: usize) -> Vec<CellSpec> {
    let big = if n == 2 { 0.58 } else { 0.62 };
    let m = n - 1;
    let cols = if m > 4 { 2 } else { 1 };
    let right = transpose(rows_layout(&even(m, cols), None, None));
    let mut out = vec![CellSpec::rect(0.0, 0.0, big, 1.0)];
    out.extend(right.into_iter().map(|c| CellSpec::rect(big + c.x * (1.0 - big), c.y, c.w * (1.0 - big), c.h)));
    out
}

fn big_top(n: usize) -> Vec<CellSpec> {
    let big = if n == 2 { 0.56 } else { 0.6 };
    let m = n - 1;
    let rows = if m > 4 { 2 } else { 1 };
    let bottom = rows_layout(&even(m, rows), None, None);
    let mut out = vec![CellSpec::rect(0.0, 0.0, 1.0, big)];
    out.extend(bottom.into_iter().map(|c| CellSpec::rect(c.x, big + c.y * (1.0 - big), c.w, c.h * (1.0 - big))));
    out
}

fn line_counts(n: usize) -> Vec<usize> {
    if n <= 4 {
        vec![1; n]
    } else {
        even(n, n.div_ceil(2))
    }
}

/// The cells of template `id` for `n` photos on a content box of ratio `aspect`.
pub fn make(id: &str, n: usize, aspect: f64, seed: u32) -> Vec<CellSpec> {
    let n = n.max(1);
    match template(id).id {
        "mosaic" => mosaic(n),
        "bigLeft" if n >= 2 => big_left(n),
        "bigTop" if n >= 2 => big_top(n),
        "rows" => rows_layout(&line_counts(n), None, None),
        "columns" => transpose(rows_layout(&line_counts(n), None, None)),
        "polaroid" => scatter(n, aspect, seed, CellStyle::Polaroid),
        "scatter" => scatter(n, aspect, seed, CellStyle::Print),
        _ => grid_cells(n, aspect),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(cells: &[CellSpec]) -> f64 {
        cells.iter().map(|c| c.w * c.h).sum()
    }

    #[test]
    fn rng_matches_mulberry32() {
        // values of the JS rng (node)
        let mut r = Rng(7 * 97 + 4);
        assert!((r.next() - 0.605_854_978_086_426_9).abs() < 1e-15);
        assert!((r.next() - 0.766_716_143_814_846_9).abs() < 1e-15);
        let mut r0 = Rng(0);
        assert!((r0.next() - 0.266_429_208_684_712_65).abs() < 1e-15);
        assert!((r0.next() - 0.000_329_745_700_582_861_9).abs() < 1e-15);
    }

    #[test]
    fn grid_templates_tile_the_box() {
        for t in TEMPLATES.iter().filter(|t| !t.free) {
            for n in 2..=9 {
                let cells = make(t.id, n, 0.8, 7);
                assert_eq!(cells.len(), n, "{} {n}", t.id);
                assert!((area(&cells) - 1.0).abs() < 1e-9, "{} {n}: {}", t.id, area(&cells));
                for c in &cells {
                    assert!(c.x >= -1e-9 && c.y >= -1e-9 && c.x + c.w <= 1.0 + 1e-9 && c.y + c.h <= 1.0 + 1e-9);
                }
            }
        }
    }

    #[test]
    fn best_cols_prefers_square_cells() {
        assert_eq!(best_cols(4, 1.0), 2);
        assert_eq!(best_cols(3, 3.0), 3);
        assert_eq!(best_cols(2, 0.5), 1);
        assert_eq!(best_cols(9, 1.0), 3);
        let g = make("grid", 5, 1.0, 0);
        assert_eq!(g.iter().filter(|c| c.y == 0.0).count(), 2);
    }

    #[test]
    fn hero_layouts_put_the_first_cell_big() {
        let l = make("bigLeft", 4, 1.0, 0);
        assert_eq!((l[0].w, l[0].h), (0.62, 1.0));
        assert!(l[1..].iter().all(|c| (c.x - 0.62).abs() < 1e-9));
        let t = make("bigTop", 7, 1.0, 0);
        assert_eq!((t[0].w, t[0].h), (1.0, 0.6));
        assert_eq!(t[1..].iter().filter(|c| (c.y - 0.6).abs() < 1e-9).count(), 3);
        let c = make("columns", 3, 1.0, 0);
        assert!(c.iter().all(|c| c.h == 1.0));
    }

    #[test]
    fn cells_match_the_web_app() {
        let near = |c: CellSpec, e: [f64; 5]| {
            let got = [c.x, c.y, c.w, c.h, c.rot];
            assert!(got.iter().zip(e).all(|(a, b)| (a - b).abs() < 1e-12), "{got:?} vs {e:?}");
        };
        // templates.ts under node
        near(make("polaroid", 6, 0.8, 7)[2], [0.1975983088091016, 0.5170317496048907, 0.357, 0.34, 6.930958446115255]);
        near(make("scatter", 5, 1.3, 9)[4], [0.6348255509945253, 0.7150000000000001, 0.342, 0.57, -0.9710031687282026]);
        near(make("mosaic", 7, 1.0, 0)[3], [0.2857142857142857, 0.35714285714285715, 0.42857142857142855, 0.2857142857142857, 0.0]);
    }

    #[test]
    fn scatter_is_seeded_and_stays_inside() {
        let a = make("polaroid", 6, 0.8, 7);
        assert_eq!(a, make("polaroid", 6, 0.8, 7));
        assert_ne!(a, make("polaroid", 6, 0.8, 8));
        for c in a.iter().chain(make("scatter", 9, 1.6, 3).iter()) {
            assert!(c.x - c.w / 2.0 >= -1e-9 && c.x + c.w / 2.0 <= 1.0 + 1e-9, "{c:?}");
            assert!(c.rot.abs() <= 11.0);
            assert_ne!(c.style, CellStyle::Plain);
        }
    }
}
