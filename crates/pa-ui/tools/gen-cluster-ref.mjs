// supercluster reference outputs for crates/pa-ui/src/map_cluster.rs tests
import Supercluster from '/home/user/peekaround/node_modules/supercluster/index.js';
let s = 12345;
const rnd = () => { s = (s * 1103515245 + 12345) % 2147483648; return s / 2147483648; };
const pts = [];
const centres = [[41.01, 28.97], [39.92, 32.85], [36.89, 30.70], [52.52, 13.40], [48.85, 2.35], [40.71, -74.0], [-33.86, 151.2], [35.68, 139.69], [64.1, -21.9], [-0.2, 179.9], [0.3, -179.8]];
let id = 0;
for (const [la, lo] of centres) {
  const n = 20 + Math.floor(rnd() * 120);
  const spread = rnd() < 0.5 ? 0.05 : 1.5;
  for (let i = 0; i < n; i++) pts.push([String(++id), +(la + (rnd() - 0.5) * spread).toFixed(5), +(lo + (rnd() - 0.5) * spread).toFixed(5)]);
}
for (let i = 0; i < 150; i++) pts.push([String(++id), +((rnd() - 0.5) * 140).toFixed(5), +((rnd() - 0.5) * 360).toFixed(5)]);
// identical locations
for (let i = 0; i < 6; i++) pts.push([String(++id), 37.0, 27.43]);
const sc = new Supercluster({ radius: 72, maxZoom: 17, map: (p) => ({ cover: p.id }), reduce: () => {} });
sc.load(pts.map(([id, lat, lon]) => ({ type: 'Feature', properties: { id }, geometry: { type: 'Point', coordinates: [lon, lat] } })));
const queries = [
  [[-180, -85, 180, 85], 0], [[-180, -85, 180, 85], 2], [[-180, -85, 180, 85], 4.6], [[20, 30, 45, 45], 5], [[26, 35, 34, 43], 7],
  [[28.5, 40.5, 29.5, 41.5], 10], [[28.9, 40.95, 29.05, 41.07], 13], [[170, -10, -170, 10], 3], [[170, -5, -170, 5], 8], [[27.4, 36.98, 27.46, 37.02], 18],
  [[-200, -60, 200, 60], 3],
];
const out = { points: pts, queries: [] };
for (const [bbox, z] of queries) {
  const cs = sc.getClusters(bbox, z).map((c) => {
    const p = c.properties;
    if (p.cluster) {
      return { c: p.cluster_id, n: p.point_count, lon: c.geometry.coordinates[0], lat: c.geometry.coordinates[1], cover: p.cover, ez: sc.getClusterExpansionZoom(p.cluster_id), leaf: sc.getLeaves(p.cluster_id, 1)[0].properties.id };
    }
    return { id: p.id };
  });
  out.queries.push({ bbox, z, clusters: cs });
}
console.log(JSON.stringify(out));
