/** Map helpers — pure (unit tested). */
import { TILE_STYLES, tileStyleOf, type MapTileStyle, type Settings } from '@peekaround/shared';

export type Point = [id: string, lat: number, lon: number];

export interface TileConfig {
  style: MapTileStyle;
  url: string;
  attribution: string;
  subdomains?: string;
  maxZoom: number;
  /** tiles are light-only: darken them with a CSS filter in dark themes */
  className?: string;
  /** loaded through the PeekAround server (proxy + cache) */
  proxied: boolean;
}

/**
 * Tile layer for Settings.mapTiles + current theme; null = no tiles.
 * @param proxy the server's tile URL template (Backend.tileUrl) — HTTP backends always proxy, so
 *   the browser / Electron renderer never talks to the tile servers itself (OSM tile policy:
 *   identifying User-Agent, no loopback Referer); Android local mode has no server → upstream.
 */
export function tileConfig(mode: Settings['mapTiles'], dark: boolean, proxy?: (style: MapTileStyle) => string): TileConfig | null {
  const style = tileStyleOf(mode);
  if (!style) return null;
  const s = TILE_STYLES[style];
  const className = dark && s.darkFilter ? 'pm-tiles-dark' : undefined;
  if (proxy) return { style, url: proxy(style), attribution: s.attribution, maxZoom: s.maxZoom, className, proxied: true };
  return {
    style,
    url: s.url,
    subdomains: s.subdomains.length ? s.subdomains.join('') : undefined,
    attribution: s.attribution,
    maxZoom: s.maxZoom,
    className,
    proxied: false,
  };
}

/** server answer for a failed tile (X-Peek-Tile-Error) → UI message kind */
export type TileProblem = { kind: 'offline' } | { kind: 'refused'; status: number } | { kind: 'unknown' };

export function tileProblemOf(header: string | null | undefined, httpStatus: number): TileProblem {
  const h = (header ?? '').trim();
  if (h === 'offline') return { kind: 'offline' };
  if (h.startsWith('refused')) return { kind: 'refused', status: Number(h.split(':')[1]) || httpStatus || 0 };
  if (httpStatus === 504 || httpStatus === 0) return { kind: 'offline' };
  return { kind: 'unknown' };
}

const wrapLon = (lon: number) => ((((lon + 180) % 360) + 360) % 360) - 180;

/**
 * Leaflet bounds (may exceed ±180 with worldCopyJump) → supercluster bbox [w, s, e, n].
 * A view wider than the world becomes the whole world; west > east means it crosses the antimeridian
 * (supercluster handles that).
 */
export function normalizeBbox(w: number, s: number, e: number, n: number): [number, number, number, number] | null {
  if (![w, s, e, n].every(Number.isFinite)) return null;
  const S = Math.max(-90, s);
  const N = Math.min(90, n);
  if (e - w >= 360) return [-180, S, 180, N];
  let W = wrapLon(w);
  let E = wrapLon(e);
  if (E === -180 && e > w) E = 180;
  if (W === 180) W = -180;
  return [W, S, E, N];
}

/** ids of the points inside a (possibly wrapped) view */
export function boundsFilter(points: Point[], w: number, s: number, e: number, n: number): string[] {
  const b = normalizeBbox(w, s, e, n);
  if (!b) return [];
  const [W, S, E, N] = b;
  const out: string[] = [];
  const cross = W > E;
  for (const [id, lat, lon] of points) {
    if (lat < S || lat > N) continue;
    if (cross ? lon >= W || lon <= E : lon >= W && lon <= E) out.push(id);
  }
  return out;
}
