/** Map helpers — pure (unit tested). */
import type { Settings } from '@peekaround/shared';

export type Point = [id: string, lat: number, lon: number];

export interface TileConfig {
  url: string;
  attribution: string;
  subdomains?: string;
  /** tiles are light-only: darken them with a CSS filter in dark themes */
  className?: string;
}

const OSM_ATTR = '© <a href="https://www.openstreetmap.org/copyright" target="_blank" rel="noreferrer">OpenStreetMap</a> contributors';

/** tile layer for Settings.mapTiles + current theme; null = no tiles */
export function tileConfig(mode: Settings['mapTiles'], dark: boolean): TileConfig | null {
  if (mode === 'off') return null;
  // 'carto' is kept for old settings only: CARTO basemaps now require an API key, so every
  // mode renders OpenStreetMap tiles (darkened in dark themes).
  return { url: 'https://tile.openstreetmap.org/{z}/{x}/{y}.png', attribution: OSM_ATTR, className: dark ? 'pm-tiles-dark' : undefined };
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
