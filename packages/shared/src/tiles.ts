/**
 * Map tile styles (pure). The server proxies + caches these (GET /api/tiles/:style/:z/:x/:y.png)
 * so desktop / Docker / Android client mode never hit the tile servers from the browser with a
 * loopback Referer and a generic browser User-Agent (OSM's tile usage policy); Android local mode
 * has no server and uses the upstream URLs directly.
 */

export type MapTileStyle = 'osm' | 'topo' | 'satellite';

export const MAP_TILE_STYLES: readonly MapTileStyle[] = ['osm', 'topo', 'satellite'];

export interface TileStyleInfo {
  id: MapTileStyle;
  /** upstream URL template: {s} subdomain, {z}/{x}/{y} tile coordinates */
  url: string;
  subdomains: string[];
  maxZoom: number;
  /** attribution (HTML, links open in a new tab) */
  attribution: string;
  /** same attribution as plain text (static mini maps, logs) */
  attributionText: string;
  /** light-only imagery: darken with a CSS filter in dark themes */
  darkFilter: boolean;
  /** tile content type */
  type: 'image/png' | 'image/jpeg';
}

const OSM_LINK = '<a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noreferrer">OpenStreetMap</a>';

export const TILE_STYLES: Record<MapTileStyle, TileStyleInfo> = {
  osm: {
    id: 'osm',
    url: 'https://tile.openstreetmap.org/{z}/{x}/{y}.png',
    subdomains: [],
    maxZoom: 19,
    attribution: `© ${OSM_LINK} contributors`,
    attributionText: '© OpenStreetMap contributors',
    darkFilter: true,
    type: 'image/png',
  },
  topo: {
    id: 'topo',
    url: 'https://{s}.tile.opentopomap.org/{z}/{x}/{y}.png',
    subdomains: ['a', 'b', 'c'],
    maxZoom: 17,
    attribution: `© ${OSM_LINK} contributors, SRTM · © <a href="https://opentopomap.org" target="_blank" rel="noreferrer">OpenTopoMap</a> (CC-BY-SA)`,
    attributionText: '© OpenStreetMap contributors, SRTM · © OpenTopoMap (CC-BY-SA)',
    darkFilter: true,
    type: 'image/png',
  },
  satellite: {
    id: 'satellite',
    url: 'https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}',
    subdomains: [],
    maxZoom: 19,
    attribution:
      'Tiles © <a href="https://www.esri.com" target="_blank" rel="noreferrer">Esri</a> — Source: Esri, Maxar, Earthstar Geographics, and the GIS User Community',
    attributionText: 'Tiles © Esri — Source: Esri, Maxar, Earthstar Geographics, and the GIS User Community',
    darkFilter: false,
    type: 'image/jpeg',
  },
};

/** Settings.mapTiles → tile style (legacy 'carto' → osm), null when tiles are off */
export function tileStyleOf(setting: string | null | undefined): MapTileStyle | null {
  if (setting === 'off') return null;
  if (setting === 'topo' || setting === 'satellite' || setting === 'osm') return setting;
  return 'osm';
}

export function isTileStyle(s: unknown): s is MapTileStyle {
  return s === 'osm' || s === 'topo' || s === 'satellite';
}

/** integer z in [0, maxZoom], x/y in [0, 2^z) */
export function isValidTile(style: MapTileStyle, z: number, x: number, y: number): boolean {
  if (![z, x, y].every((v) => Number.isInteger(v))) return false;
  if (z < 0 || z > TILE_STYLES[style].maxZoom) return false;
  const n = 2 ** z;
  return x >= 0 && x < n && y >= 0 && y < n;
}

/** upstream URL of one tile (subdomain picked deterministically) */
export function upstreamTileUrl(style: MapTileStyle, z: number, x: number, y: number): string {
  const s = TILE_STYLES[style];
  const sub = s.subdomains.length ? s.subdomains[(x + y) % s.subdomains.length] : '';
  return s.url.replace('{s}', sub).replace('{z}', String(z)).replace('{x}', String(x)).replace('{y}', String(y));
}

/** Web Mercator: tile containing (lat, lon) at zoom z, plus the pixel offset (0..256) inside it */
export function tileOf(lat: number, lon: number, z: number): { x: number; y: number; px: number; py: number } {
  const n = 2 ** z;
  const la = Math.max(-85.05112878, Math.min(85.05112878, lat));
  const fx = ((lon + 180) / 360) * n;
  const r = (la * Math.PI) / 180;
  const fy = ((1 - Math.log(Math.tan(r) + 1 / Math.cos(r)) / Math.PI) / 2) * n;
  const x = Math.min(n - 1, Math.max(0, Math.floor(fx)));
  const y = Math.min(n - 1, Math.max(0, Math.floor(fy)));
  return { x, y, px: (fx - x) * 256, py: (fy - y) * 256 };
}
