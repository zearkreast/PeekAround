//! Point clustering for the map: a faithful port of `supercluster` 8 (ISC, Mapbox) over a port of
//! `kdbush` 4 (ISC) — same projection, same f32 tree coordinates, same KD sort / traversal order —
//! so clusters, cluster ids, covers and expansion zooms match the web UI's map exactly
//! (`map_cluster_ref.json` was produced by the npm package, tools/gen-cluster-ref.mjs).
//!
//! The web UI runs it with `radius: 72, maxZoom: 17, map: p => ({cover: p.id}), reduce: () => {}`:
//! a cluster's cover is the cover of the point (or cluster) it grew from.

const OFFSET_ZOOM: usize = 2;
const OFFSET_ID: usize = 3;
const OFFSET_PARENT: usize = 4;
const OFFSET_NUM: usize = 5;
const OFFSET_PROP: usize = 6;
/// x, y, zoom, id, parent, num, prop (the reducer variant: the web UI passes `reduce`)
const STRIDE: usize = 7;

/// longitude → spherical mercator x in [0, 1]
pub fn lng_x(lng: f64) -> f64 {
    lng / 360.0 + 0.5
}

/// latitude → spherical mercator y in [0, 1] (clamped)
pub fn lat_y(lat: f64) -> f64 {
    let sin = (lat * std::f64::consts::PI / 180.0).sin();
    let y = 0.5 - 0.25 * ((1.0 + sin) / (1.0 - sin)).ln() / std::f64::consts::PI;
    y.clamp(0.0, 1.0)
}

/// mercator x → longitude
pub fn x_lng(x: f64) -> f64 {
    (x - 0.5) * 360.0
}

/// mercator y → latitude
pub fn y_lat(y: f64) -> f64 {
    let y2 = (180.0 - y * 360.0) * std::f64::consts::PI / 180.0;
    360.0 * y2.exp().atan() / std::f64::consts::PI - 90.0
}

/// kdbush: a static KD index of f32 points
struct KdBush {
    ids: Vec<u32>,
    coords: Vec<f32>,
    node_size: usize,
}

impl KdBush {
    fn new(points: impl Iterator<Item = (f64, f64)>, node_size: usize) -> KdBush {
        let mut coords = Vec::new();
        for (x, y) in points {
            coords.push(x as f32);
            coords.push(y as f32);
        }
        let n = coords.len() / 2;
        let mut t = KdBush {
            ids: (0..n as u32).collect(),
            coords,
            node_size: node_size.clamp(2, 65535),
        };
        if n > 0 {
            t.sort(0, n as isize - 1, 0);
        }
        t
    }

    fn c(&self, i: isize, axis: usize) -> f64 {
        self.coords[2 * i as usize + axis] as f64
    }

    fn sort(&mut self, left: isize, right: isize, axis: usize) {
        if right - left <= self.node_size as isize {
            return;
        }
        let m = (left + right) >> 1;
        self.select(m, left, right, axis);
        self.sort(left, m - 1, 1 - axis);
        self.sort(m + 1, right, 1 - axis);
    }

    /// Floyd–Rivest selection (kdbush `select`)
    fn select(&mut self, k: isize, mut left: isize, mut right: isize, axis: usize) {
        while right > left {
            if right - left > 600 {
                let n = (right - left + 1) as f64;
                let m = (k - left + 1) as f64;
                let z = n.ln();
                let s = 0.5 * (2.0 * z / 3.0).exp();
                let sd =
                    0.5 * (z * s * (n - s) / n).sqrt() * if m - n / 2.0 < 0.0 { -1.0 } else { 1.0 };
                let new_left = left.max((k as f64 - m * s / n + sd).floor() as isize);
                let new_right = right.min((k as f64 + (n - m) * s / n + sd).floor() as isize);
                self.select(k, new_left, new_right, axis);
            }
            let t = self.c(k, axis);
            let mut i = left;
            let mut j = right;
            self.swap_item(left, k);
            if self.c(right, axis) > t {
                self.swap_item(left, right);
            }
            while i < j {
                self.swap_item(i, j);
                i += 1;
                j -= 1;
                while self.c(i, axis) < t {
                    i += 1;
                }
                while self.c(j, axis) > t {
                    j -= 1;
                }
            }
            if self.c(left, axis) == t {
                self.swap_item(left, j);
            } else {
                j += 1;
                self.swap_item(j, right);
            }
            if j <= k {
                left = j + 1;
            }
            if k <= j {
                right = j - 1;
            }
        }
    }

    fn swap_item(&mut self, i: isize, j: isize) {
        let (i, j) = (i as usize, j as usize);
        self.ids.swap(i, j);
        self.coords.swap(2 * i, 2 * j);
        self.coords.swap(2 * i + 1, 2 * j + 1);
    }

    fn range(&self, min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Vec<usize> {
        let mut result = Vec::new();
        if self.ids.is_empty() {
            return result;
        }
        let mut stack: Vec<(isize, isize, usize)> = vec![(0, self.ids.len() as isize - 1, 0)];
        let inside = |x: f64, y: f64| x >= min_x && x <= max_x && y >= min_y && y <= max_y;
        while let Some((left, right, axis)) = stack.pop() {
            if right - left <= self.node_size as isize {
                for i in left..=right {
                    if inside(self.c(i, 0), self.c(i, 1)) {
                        result.push(self.ids[i as usize] as usize);
                    }
                }
                continue;
            }
            let m = (left + right) >> 1;
            let (x, y) = (self.c(m, 0), self.c(m, 1));
            if inside(x, y) {
                result.push(self.ids[m as usize] as usize);
            }
            // (the JS stack pops the last pushed frame first: push left, then right)
            if if axis == 0 { min_x <= x } else { min_y <= y } {
                stack.push((left, m - 1, 1 - axis));
            }
            if if axis == 0 { max_x >= x } else { max_y >= y } {
                stack.push((m + 1, right, 1 - axis));
            }
        }
        result
    }

    fn within(&self, qx: f64, qy: f64, r: f64) -> Vec<usize> {
        let mut result = Vec::new();
        if self.ids.is_empty() {
            return result;
        }
        let r2 = r * r;
        let sq = |x: f64, y: f64| {
            let (dx, dy) = (x - qx, y - qy);
            dx * dx + dy * dy
        };
        let mut stack: Vec<(isize, isize, usize)> = vec![(0, self.ids.len() as isize - 1, 0)];
        while let Some((left, right, axis)) = stack.pop() {
            if right - left <= self.node_size as isize {
                for i in left..=right {
                    if sq(self.c(i, 0), self.c(i, 1)) <= r2 {
                        result.push(self.ids[i as usize] as usize);
                    }
                }
                continue;
            }
            let m = (left + right) >> 1;
            let (x, y) = (self.c(m, 0), self.c(m, 1));
            if sq(x, y) <= r2 {
                result.push(self.ids[m as usize] as usize);
            }
            if if axis == 0 { qx - r <= x } else { qy - r <= y } {
                stack.push((left, m - 1, 1 - axis));
            }
            if if axis == 0 { qx + r >= x } else { qy + r >= y } {
                stack.push((m + 1, right, 1 - axis));
            }
        }
        result
    }
}

struct Tree {
    kd: KdBush,
    data: Vec<f64>,
}

/// Options (supercluster names).
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// min zoom to generate clusters on
    pub min_zoom: i32,
    /// max zoom to cluster points on
    pub max_zoom: i32,
    /// min points to form a cluster
    pub min_points: usize,
    /// cluster radius in pixels
    pub radius: f64,
    /// tile extent (radius is relative to it)
    pub extent: f64,
    /// KD leaf size
    pub node_size: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            min_zoom: 0,
            max_zoom: 16,
            min_points: 2,
            radius: 40.0,
            extent: 512.0,
            node_size: 64,
        }
    }
}

/// What `getClusters` returns.
#[derive(Debug, Clone, PartialEq)]
pub enum Feature {
    /// a single input point (its index in the loaded list)
    Point(usize),
    /// a cluster
    Cluster {
        /// cluster_id
        id: usize,
        /// point_count
        count: usize,
        /// centre
        lon: f64,
        /// centre
        lat: f64,
        /// index of the point whose id is the cover
        cover: usize,
    },
}

/// The cluster index.
pub struct Supercluster {
    o: Options,
    trees: Vec<Option<Tree>>,
    /// (lon, lat) of the loaded points
    points: Vec<(f64, f64)>,
    /// cluster props: the cover point index
    props: Vec<usize>,
}

impl Supercluster {
    /// an empty index
    pub fn new(o: Options) -> Supercluster {
        Supercluster {
            o,
            trees: (0..o.max_zoom + 2).map(|_| None).collect(),
            points: vec![],
            props: vec![],
        }
    }

    /// number of loaded points
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// no points
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    fn create_tree(&self, data: Vec<f64>) -> Tree {
        let kd = KdBush::new(data.chunks(STRIDE).map(|c| (c[0], c[1])), self.o.node_size);
        Tree { kd, data }
    }

    /// index `(lon, lat)` points
    pub fn load(&mut self, points: Vec<(f64, f64)>) {
        self.points = points;
        self.props.clear();
        let mut data = Vec::with_capacity(self.points.len() * STRIDE);
        for (i, &(lng, lat)) in self.points.iter().enumerate() {
            let x = lng_x(lng) as f32 as f64;
            let y = lat_y(lat) as f32 as f64;
            data.extend_from_slice(&[x, y, f64::INFINITY, i as f64, -1.0, 1.0, 0.0]);
        }
        let max = self.o.max_zoom;
        let mut tree = self.create_tree(data);
        for z in (self.o.min_zoom..=max).rev() {
            let next = self.cluster(&mut tree, z);
            self.trees[(z + 1) as usize] = Some(tree);
            tree = self.create_tree(next);
        }
        self.trees[self.o.min_zoom as usize] = Some(tree);
    }

    fn limit_zoom(&self, z: f64) -> usize {
        (z.floor() as i32).clamp(self.o.min_zoom, self.o.max_zoom + 1) as usize
    }

    fn feature(&self, data: &[f64], k: usize) -> Feature {
        if data[k + OFFSET_NUM] > 1.0 {
            Feature::Cluster {
                id: data[k + OFFSET_ID] as usize,
                count: data[k + OFFSET_NUM] as usize,
                lon: x_lng(data[k]),
                lat: y_lat(data[k + 1]),
                cover: self.props[data[k + OFFSET_PROP] as usize],
            }
        } else {
            Feature::Point(data[k + OFFSET_ID] as usize)
        }
    }

    /// clusters + points inside `[west, south, east, north]` at `zoom`
    pub fn get_clusters(&self, bbox: [f64; 4], zoom: f64) -> Vec<Feature> {
        let mut min_lng = ((bbox[0] + 180.0) % 360.0 + 360.0) % 360.0 - 180.0;
        let min_lat = bbox[1].clamp(-90.0, 90.0);
        let mut max_lng = if bbox[2] == 180.0 {
            180.0
        } else {
            ((bbox[2] + 180.0) % 360.0 + 360.0) % 360.0 - 180.0
        };
        let max_lat = bbox[3].clamp(-90.0, 90.0);
        if bbox[2] - bbox[0] >= 360.0 {
            min_lng = -180.0;
            max_lng = 180.0;
        } else if min_lng > max_lng {
            let mut e = self.get_clusters([min_lng, min_lat, 180.0, max_lat], zoom);
            e.extend(self.get_clusters([-180.0, min_lat, max_lng, max_lat], zoom));
            return e;
        }
        let Some(tree) = &self.trees[self.limit_zoom(zoom)] else {
            return vec![];
        };
        tree.kd
            .range(
                lng_x(min_lng),
                lat_y(max_lat),
                lng_x(max_lng),
                lat_y(min_lat),
            )
            .into_iter()
            .map(|id| self.feature(&tree.data, STRIDE * id))
            .collect()
    }

    fn origin_id(&self, cluster_id: usize) -> usize {
        (cluster_id - self.points.len()) >> 5
    }

    fn origin_zoom(&self, cluster_id: usize) -> usize {
        (cluster_id - self.points.len()) % 32
    }

    /// direct children of a cluster (empty for an unknown id)
    pub fn get_children(&self, cluster_id: usize) -> Vec<Feature> {
        if cluster_id < self.points.len() {
            return vec![];
        }
        let (oid, oz) = (self.origin_id(cluster_id), self.origin_zoom(cluster_id));
        let Some(Some(tree)) = self.trees.get(oz) else {
            return vec![];
        };
        let data = &tree.data;
        if oid * STRIDE >= data.len() {
            return vec![];
        }
        let r = self.o.radius / (self.o.extent * 2f64.powi(oz as i32 - 1));
        let (x, y) = (data[oid * STRIDE], data[oid * STRIDE + 1]);
        tree.kd
            .within(x, y, r)
            .into_iter()
            .filter(|&id| data[id * STRIDE + OFFSET_PARENT] == cluster_id as f64)
            .map(|id| self.feature(data, id * STRIDE))
            .collect()
    }

    /// up to `limit` points of a cluster (after skipping `offset`)
    pub fn get_leaves(&self, cluster_id: usize, limit: usize, offset: usize) -> Vec<usize> {
        let mut out = vec![];
        self.append_leaves(&mut out, cluster_id, limit.max(1), offset, 0);
        out
    }

    fn append_leaves(
        &self,
        out: &mut Vec<usize>,
        cluster_id: usize,
        limit: usize,
        offset: usize,
        mut skipped: usize,
    ) -> usize {
        for child in self.get_children(cluster_id) {
            match child {
                Feature::Cluster { id, count, .. } => {
                    if skipped + count <= offset {
                        skipped += count;
                    } else {
                        skipped = self.append_leaves(out, id, limit, offset, skipped);
                    }
                }
                Feature::Point(i) => {
                    if skipped < offset {
                        skipped += 1;
                    } else {
                        out.push(i);
                    }
                }
            }
            if out.len() == limit {
                break;
            }
        }
        skipped
    }

    /// the zoom at which a cluster splits up
    pub fn get_cluster_expansion_zoom(&self, mut cluster_id: usize) -> i32 {
        if cluster_id < self.points.len() {
            return self.o.max_zoom + 1;
        }
        let mut z = self.origin_zoom(cluster_id) as i32 - 1;
        while z <= self.o.max_zoom {
            let children = self.get_children(cluster_id);
            z += 1;
            if children.len() != 1 {
                break;
            }
            match children[0] {
                Feature::Cluster { id, .. } => cluster_id = id,
                Feature::Point(_) => break,
            }
        }
        z
    }

    /// cover point of a feature in the stored data (`_map`)
    fn map_cover(&self, data: &[f64], i: usize) -> usize {
        if data[i + OFFSET_NUM] > 1.0 {
            self.props[data[i + OFFSET_PROP] as usize]
        } else {
            data[i + OFFSET_ID] as usize
        }
    }

    fn cluster(&mut self, tree: &mut Tree, zoom: i32) -> Vec<f64> {
        let r = self.o.radius / (self.o.extent * 2f64.powi(zoom));
        let zf = zoom as f64;
        let n_points = self.points.len();
        let mut next = Vec::new();
        let data = &mut tree.data;
        let mut i = 0;
        while i < data.len() {
            if data[i + OFFSET_ZOOM] <= zf {
                i += STRIDE;
                continue;
            }
            data[i + OFFSET_ZOOM] = zf;
            let (x, y) = (data[i], data[i + 1]);
            let neighbors = tree.kd.within(x, y, r);
            let num_origin = data[i + OFFSET_NUM];
            let mut num = num_origin;
            for &nb in &neighbors {
                let k = nb * STRIDE;
                if data[k + OFFSET_ZOOM] > zf {
                    num += data[k + OFFSET_NUM];
                }
            }
            if num > num_origin && num >= self.o.min_points as f64 {
                let mut wx = x * num_origin;
                let mut wy = y * num_origin;
                let mut prop_index: f64 = -1.0;
                let id = (((i / STRIDE) << 5) + (zoom as usize + 1) + n_points) as f64;
                for &nb in &neighbors {
                    let k = nb * STRIDE;
                    if data[k + OFFSET_ZOOM] <= zf {
                        continue;
                    }
                    data[k + OFFSET_ZOOM] = zf;
                    let n2 = data[k + OFFSET_NUM];
                    wx += data[k] * n2;
                    wy += data[k + 1] * n2;
                    data[k + OFFSET_PARENT] = id;
                    if prop_index < 0.0 {
                        // `_map(data, i, true)`: the origin's props (its cover)
                        let cover = self.map_cover(data, i);
                        prop_index = self.props.len() as f64;
                        self.props.push(cover);
                    }
                }
                data[i + OFFSET_PARENT] = id;
                next.extend_from_slice(&[
                    wx / num,
                    wy / num,
                    f64::INFINITY,
                    id,
                    -1.0,
                    num,
                    prop_index,
                ]);
            } else {
                next.extend_from_slice(&data[i..i + STRIDE]);
                if num > 1.0 {
                    for &nb in &neighbors {
                        let k = nb * STRIDE;
                        if data[k + OFFSET_ZOOM] <= zf {
                            continue;
                        }
                        data[k + OFFSET_ZOOM] = zf;
                        next.extend_from_slice(&data[k..k + STRIDE]);
                    }
                }
            }
            i += STRIDE;
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn reference() -> Value {
        serde_json::from_str(include_str!("map_cluster_ref.json")).unwrap()
    }

    #[test]
    fn projection_roundtrip() {
        for lat in [-80.0, -33.86, 0.0, 41.01, 85.0] {
            assert!((y_lat(lat_y(lat)) - lat).abs() < 1e-9);
        }
        assert_eq!(lng_x(-180.0), 0.0);
        assert_eq!(lng_x(180.0), 1.0);
        assert!((x_lng(lng_x(28.97)) - 28.97).abs() < 1e-12);
    }

    #[test]
    fn matches_supercluster() {
        let r = reference();
        let pts: Vec<(String, f64, f64)> = r["points"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                (
                    p[0].as_str().unwrap().to_string(),
                    p[1].as_f64().unwrap(),
                    p[2].as_f64().unwrap(),
                )
            })
            .collect();
        let mut sc = Supercluster::new(Options {
            radius: 72.0,
            max_zoom: 17,
            ..Default::default()
        });
        sc.load(pts.iter().map(|p| (p.2, p.1)).collect());
        let mut checked = 0;
        for q in r["queries"].as_array().unwrap() {
            let b: Vec<f64> = q["bbox"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect();
            let z = q["z"].as_f64().unwrap();
            let got = sc.get_clusters([b[0], b[1], b[2], b[3]], z);
            let want = q["clusters"].as_array().unwrap();
            assert_eq!(got.len(), want.len(), "count at z{z} {b:?}");
            for (g, w) in got.iter().zip(want) {
                match g {
                    Feature::Point(i) => assert_eq!(pts[*i].0, w["id"].as_str().unwrap()),
                    Feature::Cluster {
                        id,
                        count,
                        lon,
                        lat,
                        cover,
                    } => {
                        assert_eq!(*id as u64, w["c"].as_u64().unwrap());
                        assert_eq!(*count as u64, w["n"].as_u64().unwrap());
                        assert!((lon - w["lon"].as_f64().unwrap()).abs() < 1e-9);
                        assert!((lat - w["lat"].as_f64().unwrap()).abs() < 1e-9);
                        assert_eq!(pts[*cover].0, w["cover"].as_str().unwrap());
                        assert_eq!(
                            sc.get_cluster_expansion_zoom(*id) as i64,
                            w["ez"].as_i64().unwrap()
                        );
                        let leaf = sc.get_leaves(*id, 1, 0);
                        assert_eq!(pts[leaf[0]].0, w["leaf"].as_str().unwrap());
                    }
                }
                checked += 1;
            }
        }
        assert!(checked > 400);
    }

    #[test]
    fn empty_and_single() {
        let mut sc = Supercluster::new(Options::default());
        sc.load(vec![]);
        assert!(sc
            .get_clusters([-180.0, -85.0, 180.0, 85.0], 3.0)
            .is_empty());
        sc.load(vec![(29.0, 41.0)]);
        assert_eq!(
            sc.get_clusters([-180.0, -85.0, 180.0, 85.0], 3.0),
            vec![Feature::Point(0)]
        );
    }
}
