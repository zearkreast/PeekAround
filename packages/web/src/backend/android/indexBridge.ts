/**
 * Bridge to the native PeekIndex (Kotlin, SQLite) — GPS / camera / capture offset / hint flags
 * of local files. Every call is defensive: an older native build without the index methods
 * simply yields "no metadata" and Explore falls back to dates, kinds and names.
 *
 *   indexQuery({ paths })  → columnar { path[], lat[], lon[], make[], model[], lens[], offset[], flags[] }
 *                            (paths = folder roots or exact files; rows under them are returned)
 *   indexStatus()          → { done, total, running }
 *   indexSync({ paths })   → (re)start the background pass over these roots
 */
import { PeekLocal } from './plugin';

/** PeekIndex hint bits (NOT MediaFlag): see PeekMeta.kt */
export const INDEX_HINT = { gpano: 1, animated: 2 } as const;

export interface IndexMeta {
  lat: number | null;
  lon: number | null;
  make: string | null;
  model: string | null;
  lens: string | null;
  /** minutes east of UTC when known */
  offset: number | null;
  hint: number;
}

export interface IndexStatus {
  done: number;
  total: number;
  running: boolean;
}

interface Columns {
  path?: string[];
  lat?: (number | null)[];
  lon?: (number | null)[];
  make?: (string | null)[];
  model?: (string | null)[];
  lens?: (string | null)[];
  offset?: (number | null)[];
  flags?: (number | null)[];
}

interface IndexApi {
  indexQuery?(o: { paths: string[] }): Promise<Columns>;
  indexStatus?(): Promise<Partial<IndexStatus>>;
  indexSync?(o: { paths: string[] }): Promise<unknown>;
}

const api = () => PeekLocal as unknown as IndexApi;
let unsupported = false;

const num = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null);
const str = (v: unknown): string | null => (typeof v === 'string' && v.trim() ? v : null);

/** media id (URL-safe base64 of the UTF-8 absolute path, see local.ts encId) → path */
export function decodeId(id: string): string {
  const b = atob(id.replace(/-/g, '+').replace(/_/g, '/'));
  return new TextDecoder().decode(Uint8Array.from(b, (c) => c.charCodeAt(0)));
}

export function dirOf(p: string): string {
  const i = p.lastIndexOf('/');
  return i <= 0 ? '/' : p.slice(0, i);
}

/** smallest set of directories covering all paths (their parent dirs, minus nested ones) */
export function rootsOf(paths: Iterable<string>): string[] {
  const dirs = [...new Set([...paths].map(dirOf))].sort();
  const out: string[] = [];
  for (const d of dirs) {
    const last = out[out.length - 1];
    if (last !== undefined && (d === last || d.startsWith(last === '/' ? '/' : `${last}/`))) continue;
    out.push(d);
  }
  return out;
}

/** columnar result → path → metadata */
export function parseColumns(c: Columns | null | undefined): Map<string, IndexMeta> {
  const out = new Map<string, IndexMeta>();
  const paths = c?.path;
  if (!Array.isArray(paths)) return out;
  for (let i = 0; i < paths.length; i++) {
    out.set(paths[i], {
      lat: num(c!.lat?.[i]),
      lon: num(c!.lon?.[i]),
      make: str(c!.make?.[i]),
      model: str(c!.model?.[i]),
      lens: str(c!.lens?.[i]),
      offset: num(c!.offset?.[i]),
      hint: num(c!.flags?.[i]) ?? 0,
    });
  }
  return out;
}

/** metadata for everything under `roots` (null when the native index is unavailable) */
export async function queryIndex(roots: string[]): Promise<Map<string, IndexMeta> | null> {
  if (unsupported || !roots.length) return null;
  try {
    const fn = api().indexQuery;
    if (typeof fn !== 'function') {
      unsupported = true;
      return null;
    }
    return parseColumns(await fn.call(PeekLocal, { paths: roots }));
  } catch (e) {
    if (/not implemented|unimplemented|not available/i.test(String((e as Error)?.message ?? e))) unsupported = true;
    return null;
  }
}

export async function indexStatus(): Promise<IndexStatus | null> {
  if (unsupported) return null;
  try {
    const fn = api().indexStatus;
    if (typeof fn !== 'function') return null;
    const s = await fn.call(PeekLocal);
    return { done: num(s?.done) ?? 0, total: num(s?.total) ?? 0, running: !!s?.running };
  } catch {
    return null;
  }
}

let syncedFor = '';
/**
 * Make sure a background pass over `roots` has run once this session (local.ts may also start it;
 * a repeated call only restarts the native walk, which is cheap for already indexed files).
 */
export async function ensureIndexed(roots: string[], status: IndexStatus | null): Promise<void> {
  if (unsupported || !roots.length) return;
  const sig = roots.join('\n');
  if (sig === syncedFor) return;
  if (status?.running && syncedFor === '') {
    // someone (local.ts) already started a pass; adopt it
    syncedFor = sig;
    return;
  }
  syncedFor = sig;
  try {
    const fn = api().indexSync;
    if (typeof fn === 'function') await fn.call(PeekLocal, { paths: roots });
  } catch {
    /* older native build */
  }
}

/** test hook */
export function _resetIndexBridge() {
  unsupported = false;
  syncedFor = '';
}
