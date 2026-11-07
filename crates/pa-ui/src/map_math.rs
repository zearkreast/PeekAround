//! Slippy-map math (Web Mercator, 256 px tiles): the view (centre + fractional zoom) ↔ screen,
//! the tiles that cover a view, bounds fitting, and the helpers of the web UI's `mapUtil.ts`
//! (`normalizeBbox`, `boundsFilter`, `tileProblemOf`).

use crate::map_cluster::{lat_y, lng_x, x_lng, y_lat};

/// tile size in CSS px
pub const TILE: f64 = 256.0;

/// A map view: centre in mercator units ([0, 1] on both axes, x wraps) and a fractional zoom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    /// mercator x of the centre
    pub cx: f64,
    /// mercator y of the centre
    pub cy: f64,
    /// zoom (2^zoom × 256 px = the world)
    pub zoom: f64,
}

impl View {
    /// a view centred on (lat, lon)
    pub fn at(lat: f64, lon: f64, zoom: f64) -> View {
        View {
            cx: lng_x(lon),
            cy: lat_y(lat),
            zoom,
        }
    }

    /// world size in px
    pub fn world(&self) -> f64 {
        TILE * 2f64.powf(self.zoom)
    }

    /// screen position (px from the top-left of a `w`×`h` viewport) of a mercator point;
    /// x picks the world copy nearest to the centre
    pub fn to_screen(&self, mx: f64, my: f64, w: f64, h: f64) -> (f64, f64) {
        let mut dx = mx - self.cx;
        dx -= dx.round();
        (
            w / 2.0 + dx * self.world(),
            h / 2.0 + (my - self.cy) * self.world(),
        )
    }

    /// mercator point under a screen position (x not wrapped)
    pub fn to_merc(&self, sx: f64, sy: f64, w: f64, h: f64) -> (f64, f64) {
        (
            self.cx + (sx - w / 2.0) / self.world(),
            self.cy + (sy - h / 2.0) / self.world(),
        )
    }

    /// (lat, lon) of the centre
    pub fn center(&self) -> (f64, f64) {
        (y_lat(self.cy), x_lng(self.cx))
    }

    /// pan by screen px
    pub fn pan(&mut self, dx: f64, dy: f64) {
        let wd = self.world();
        self.cx -= dx / wd;
        self.cy -= dy / wd;
    }

    /// set the zoom keeping the mercator point under screen (sx, sy) in place
    pub fn zoom_around(&mut self, zoom: f64, sx: f64, sy: f64, w: f64, h: f64) {
        let (mx, my) = self.to_merc(sx, sy, w, h);
        self.zoom = zoom;
        let wd = self.world();
        self.cx = mx - (sx - w / 2.0) / wd;
        self.cy = my - (sy - h / 2.0) / wd;
    }

    /// keep the centre in range: x wraps into [0, 1), y stays inside the world (or centred when the
    /// world is shorter than the viewport)
    pub fn clamp(&mut self, h: f64) {
        self.cx = self.cx.rem_euclid(1.0);
        let wd = self.world();
        let half = h / 2.0 / wd;
        if half >= 0.5 {
            self.cy = 0.5;
        } else {
            self.cy = self.cy.clamp(half, 1.0 - half);
        }
    }

    /// `[west, south, east, north]` of the viewport (west/east unwrapped: may pass ±180)
    pub fn bounds(&self, w: f64, h: f64) -> [f64; 4] {
        let (x0, y0) = self.to_merc(0.0, 0.0, w, h);
        let (x1, y1) = self.to_merc(w, h, w, h);
        [
            x_lng(x0),
            y_lat(y1.clamp(0.0, 1.0)),
            x_lng(x1),
            y_lat(y0.clamp(0.0, 1.0)),
        ]
    }
}

/// One tile to draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileSpot {
    /// integer zoom
    pub z: u32,
    /// tile x (wrapped into [0, 2^z))
    pub x: i64,
    /// tile x before wrapping (world copies: one slot each)
    pub ux: i64,
    /// tile y
    pub y: i64,
    /// screen x of the tile's top-left
    pub sx: f64,
    /// screen y
    pub sy: f64,
    /// on-screen size (256 × 2^(zoom - z))
    pub size: f64,
}

/// the integer zoom whose tiles are drawn for a fractional zoom (Leaflet rounds)
pub fn tile_zoom(zoom: f64, max_native: u32) -> u32 {
    (zoom.round().max(0.0) as u32).min(max_native)
}

/// tiles at zoom `z` that cover a `w`×`h` viewport of `v` (+ `margin` tiles around it)
pub fn cover(v: &View, w: f64, h: f64, z: u32, margin: i64) -> Vec<TileSpot> {
    let n = 1i64 << z;
    let size = TILE * 2f64.powf(v.zoom - z as f64);
    // the world position (in tile units of zoom z) of the viewport's top-left
    let tx0 = (v.cx * n as f64) - (w / 2.0) / size;
    let ty0 = (v.cy * n as f64) - (h / 2.0) / size;
    let (x0, y0) = (tx0.floor() as i64 - margin, ty0.floor() as i64 - margin);
    let (x1, y1) = (
        (tx0 + w / size).floor() as i64 + margin,
        (ty0 + h / size).floor() as i64 + margin,
    );
    let mut out = Vec::new();
    for ty in y0.max(0)..=y1.min(n - 1) {
        for tx in x0..=x1 {
            out.push(TileSpot {
                z,
                x: tx.rem_euclid(n),
                ux: tx,
                y: ty,
                sx: (tx as f64 - tx0) * size,
                sy: (ty as f64 - ty0) * size,
                size,
            });
        }
    }
    // centre tiles first (they load first)
    let (cx, cy) = (w / 2.0, h / 2.0);
    out.sort_by(|a, b| {
        let da = (a.sx + a.size / 2.0 - cx).powi(2) + (a.sy + a.size / 2.0 - cy).powi(2);
        let db = (b.sx + b.size / 2.0 - cx).powi(2) + (b.sy + b.size / 2.0 - cy).powi(2);
        da.total_cmp(&db)
    });
    out
}

/// The view that shows `[south, west, north, east]` inside `w`×`h` with `pad` px on every side
/// (Leaflet `fitBounds`, zoom snapped to `snap`, at most `max_zoom`).
pub fn fit(bounds: [f64; 4], w: f64, h: f64, pad: f64, max_zoom: f64, snap: f64) -> View {
    let [s, west, n, e] = bounds;
    let (x0, x1) = (lng_x(west), lng_x(e));
    let (y0, y1) = (lat_y(n), lat_y(s));
    let (bw, bh) = ((x1 - x0).abs().max(1e-12), (y1 - y0).abs().max(1e-12));
    let (aw, ah) = ((w - 2.0 * pad).max(1.0), (h - 2.0 * pad).max(1.0));
    let z = ((aw / (bw * TILE)).min(ah / (bh * TILE))).log2();
    let z = if snap > 0.0 {
        (z / snap).floor() * snap
    } else {
        z
    };
    View {
        cx: (x0 + x1) / 2.0,
        cy: (y0 + y1) / 2.0,
        zoom: z.clamp(0.0, max_zoom),
    }
}

fn wrap_lon(lon: f64) -> f64 {
    ((lon + 180.0) % 360.0 + 360.0) % 360.0 - 180.0
}

/// `normalizeBbox`: unwrapped view bounds → supercluster bbox `[w, s, e, n]` (a view wider than
/// the world becomes the world; west > east crosses the antimeridian)
pub fn normalize_bbox(w: f64, s: f64, e: f64, n: f64) -> Option<[f64; 4]> {
    if ![w, s, e, n].iter().all(|v| v.is_finite()) {
        return None;
    }
    let (sv, nv) = (s.max(-90.0), n.min(90.0));
    if e - w >= 360.0 {
        return Some([-180.0, sv, 180.0, nv]);
    }
    let mut wv = wrap_lon(w);
    let mut ev = wrap_lon(e);
    if ev == -180.0 && e > w {
        ev = 180.0;
    }
    if wv == 180.0 {
        wv = -180.0;
    }
    Some([wv, sv, ev, nv])
}

/// `boundsFilter`: indices of the `(lat, lon)` points inside a (possibly wrapped) view
pub fn bounds_filter(points: &[(f64, f64)], w: f64, s: f64, e: f64, n: f64) -> Vec<usize> {
    let Some([wv, sv, ev, nv]) = normalize_bbox(w, s, e, n) else {
        return vec![];
    };
    let cross = wv > ev;
    points
        .iter()
        .enumerate()
        .filter(|(_, &(lat, lon))| {
            lat >= sv
                && lat <= nv
                && if cross {
                    lon >= wv || lon <= ev
                } else {
                    lon >= wv && lon <= ev
                }
        })
        .map(|(i, _)| i)
        .collect()
}

/// Why tiles fail (the banner's message).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileProblem {
    /// the server can't reach the tile servers
    Offline,
    /// the tile server refused (HTTP status)
    Refused(u16),
    /// something else
    Unknown,
}

/// `tileProblemOf`: `X-Peek-Tile-Error` (offline | refused:<status>) + HTTP status → problem
pub fn tile_problem_of(header: Option<&str>, status: u16) -> TileProblem {
    let h = header.unwrap_or("").trim();
    if h == "offline" {
        return TileProblem::Offline;
    }
    if let Some(rest) = h.strip_prefix("refused") {
        let s = rest.trim_start_matches(':').parse().unwrap_or(0);
        return TileProblem::Refused(if s > 0 { s } else { status });
    }
    if status == 504 || status == 0 {
        return TileProblem::Offline;
    }
    TileProblem::Unknown
}

/// The proxy answers 504 when offline and 502 when the tile server refused; the client only sees
/// the status (+ the JSON `status` the server passes on), so map that back like the header would.
pub fn tile_problem_of_status(status: u16, upstream: Option<u16>) -> TileProblem {
    match status {
        504 | 0 => TileProblem::Offline,
        502 => TileProblem::Refused(upstream.filter(|s| *s > 0).unwrap_or(502)),
        _ => TileProblem::Unknown,
    }
}

/// supercluster's count label: 999, 1.2k, 12k
pub fn count_label(n: usize) -> String {
    if n >= 10_000 {
        format!("{}k", (n as f64 / 1000.0).round())
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_of_matches_pa_core() {
        for (lat, lon, z) in [(41.0082, 28.9784, 14u32), (-33.86, 151.2, 9), (0.0, 0.0, 0)] {
            let t = pa_core::tile_of(lat, lon, z);
            let v = View::at(lat, lon, z as f64);
            let n = (1u64 << z) as f64;
            let (fx, fy) = (v.cx * n, v.cy * n);
            assert_eq!(fx.floor() as i64, t.x);
            assert_eq!(fy.floor() as i64, t.y);
            assert!(((fx - fx.floor()) * 256.0 - t.px).abs() < 1e-6);
            assert!(((fy - fy.floor()) * 256.0 - t.py).abs() < 1e-6);
        }
    }

    #[test]
    fn screen_roundtrip_and_zoom_anchor() {
        let mut v = View::at(41.0, 29.0, 10.3);
        let (w, h) = (1000.0, 700.0);
        let (mx, my) = v.to_merc(123.0, 456.0, w, h);
        let (sx, sy) = v.to_screen(mx, my, w, h);
        assert!((sx - 123.0).abs() < 1e-6 && (sy - 456.0).abs() < 1e-6);
        // the point under the cursor stays under it
        v.zoom_around(12.7, 123.0, 456.0, w, h);
        let (sx, sy) = v.to_screen(mx, my, w, h);
        assert!((sx - 123.0).abs() < 1e-6 && (sy - 456.0).abs() < 1e-6);
        // pan moves the map with the pointer
        let before = v.to_screen(mx, my, w, h);
        v.pan(10.0, -5.0);
        let after = v.to_screen(mx, my, w, h);
        assert!((after.0 - before.0 - 10.0).abs() < 1e-6);
        assert!((after.1 - before.1 + 5.0).abs() < 1e-6);
    }

    #[test]
    fn cover_tiles() {
        // zoom 2 world = 1024 px; a 1024×1024 viewport centred shows exactly the 4×4 tiles
        let v = View {
            cx: 0.5,
            cy: 0.5,
            zoom: 2.0,
        };
        let t = cover(&v, 1024.0, 1024.0, 2, 0);
        // the right / bottom edges touch the next tile column / row (x wraps, y is clipped)
        assert!(t.len() >= 16);
        assert!(t.iter().all(|s| s.size == 256.0 && s.y >= 0 && s.y < 4));
        let tl = t.iter().find(|s| s.sx == 0.0 && s.sy == 0.0).unwrap();
        assert_eq!((tl.x, tl.y), (0, 0));
        // fractional zoom: tiles of the rounded zoom, scaled
        let v = View {
            cx: 0.5,
            cy: 0.5,
            zoom: 3.4,
        };
        let t = cover(&v, 800.0, 600.0, tile_zoom(3.4, 19), 0);
        assert!(t
            .iter()
            .all(|s| s.z == 3 && (s.size - 256.0 * 2f64.powf(0.4)).abs() < 1e-9));
        // wrap across the antimeridian
        let v = View {
            cx: 0.001,
            cy: 0.5,
            zoom: 3.0,
        };
        let t = cover(&v, 800.0, 600.0, 3, 0);
        assert!(t.iter().any(|s| s.x == 7) && t.iter().any(|s| s.x == 0));
        assert_eq!(tile_zoom(18.6, 17), 17);
    }

    #[test]
    fn fit_bounds() {
        let v = fit([40.8, 28.6, 41.2, 29.4], 1000.0, 800.0, 56.0, 13.0, 0.5);
        let (lat, lon) = v.center();
        assert!((lon - 29.0).abs() < 1e-9 && (lat - 41.0).abs() < 0.01);
        // the bounds fit
        let b = v.bounds(1000.0, 800.0);
        assert!(b[0] <= 28.6 && b[2] >= 29.4 && b[1] <= 40.8 && b[3] >= 41.2);
        assert_eq!(v.zoom.fract() * 2.0, (v.zoom.fract() * 2.0).round());
        // a single point: max zoom
        let v = fit([41.0, 29.0, 41.0, 29.0], 1000.0, 800.0, 56.0, 13.0, 0.5);
        assert_eq!(v.zoom, 13.0);
    }

    #[test]
    fn bbox_normalize() {
        assert_eq!(
            normalize_bbox(-200.0, -10.0, 200.0, 10.0),
            Some([-180.0, -10.0, 180.0, 10.0])
        );
        assert_eq!(
            normalize_bbox(170.0, -10.0, 190.0, 10.0),
            Some([170.0, -10.0, -170.0, 10.0])
        );
        assert_eq!(
            normalize_bbox(-180.0, -95.0, 180.0, 95.0),
            Some([-180.0, -90.0, 180.0, 90.0])
        );
        assert_eq!(
            normalize_bbox(0.0, 0.0, 180.0, 10.0),
            Some([0.0, 0.0, 180.0, 10.0])
        );
        assert_eq!(normalize_bbox(f64::NAN, 0.0, 1.0, 1.0), None);
        let pts = [(0.0, 175.0), (0.0, -175.0), (0.0, 0.0), (50.0, 175.0)];
        assert_eq!(bounds_filter(&pts, 170.0, -10.0, 190.0, 10.0), vec![0, 1]);
    }

    #[test]
    fn problems() {
        assert_eq!(tile_problem_of(Some("offline"), 504), TileProblem::Offline);
        assert_eq!(
            tile_problem_of(Some("refused:403"), 502),
            TileProblem::Refused(403)
        );
        assert_eq!(
            tile_problem_of(Some("refused"), 502),
            TileProblem::Refused(502)
        );
        assert_eq!(tile_problem_of(None, 504), TileProblem::Offline);
        assert_eq!(tile_problem_of(None, 500), TileProblem::Unknown);
        assert_eq!(
            tile_problem_of_status(502, Some(429)),
            TileProblem::Refused(429)
        );
        assert_eq!(count_label(999), "999");
        assert_eq!(count_label(1234), "1.2k");
        assert_eq!(count_label(12_345), "12k");
    }
}
