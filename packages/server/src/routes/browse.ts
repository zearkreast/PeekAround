import fsp from 'node:fs/promises';
import {
  aspectToByte,
  cameraKey,
  localDayKey,
  MEDIA_FLAG,
  parseSearch,
  type BrowseQuery,
  type BrowseResult,
  type BrowseSkeleton,
  type Crumb,
  type GeoPoints,
  type KindFilter,
  type Library,
  type MediaFlagName,
  type SortDir,
  type SortKey,
  type SubFolder,
  type TimelineBucket,
} from '@peekaround/shared';
import type { Ctx } from '../context.js';
import type { FolderRow } from '../library.js';
import { ITEM_COLS, toItem } from '../media/mapper.js';
import { normalizeRel, resolveInRoot } from '../paths.js';
import type { Plugin } from './util.js';

const SORT_SQL: Record<SortKey, string> = {
  takenAt: 'taken_local',
  name: 'name COLLATE NOCASE',
  mtime: 'mtime',
  size: 'size',
  kind: 'kind',
};

export interface Scope {
  where: string[];
  params: any[];
}

const FLAG_NAMES = new Set<string>([...Object.keys(MEDIA_FLAG), 'video']);
const SORT_KEYS = new Set<string>(Object.keys(SORT_SQL));

/** Querystring → BrowseQuery (shared by every list endpoint). */
export function parseBrowseQuery(raw: Record<string, string | undefined>): BrowseQuery {
  const num = (v: string | undefined) => (v !== undefined && v !== '' && Number.isFinite(Number(v)) ? Number(v) : undefined);
  return {
    libraryId: raw.libraryId || undefined,
    folderId: raw.folderId || undefined,
    dir: normalizeRel(raw.dir),
    recursive: raw.recursive === '1' || raw.recursive === 'true',
    sort: raw.sort && SORT_KEYS.has(raw.sort) ? (raw.sort as SortKey) : 'takenAt',
    order: raw.order === 'asc' ? 'asc' : 'desc',
    kind: (raw.kind as KindFilter) || 'all',
    q: raw.q?.trim() || undefined,
    offset: Number(raw.offset) || 0,
    limit: Number(raw.limit) || 600,
    from: num(raw.from),
    to: num(raw.to),
    place: raw.place || undefined,
    camera: raw.camera || undefined,
    flag: raw.flag && FLAG_NAMES.has(raw.flag) ? (raw.flag as MediaFlagName) : undefined,
  };
}

/** true when the query narrows the result beyond a folder (→ flatten sub folders) */
function hasFilters(q: BrowseQuery): boolean {
  return !!(q.q || q.from !== undefined || q.to !== undefined || q.place || q.camera || q.flag);
}

function kindClause(kind: KindFilter | undefined, s: Scope) {
  if (kind === 'image') s.where.push("kind = 'image'");
  else if (kind === 'video') s.where.push("kind = 'video'");
  else if (kind === 'favorites') s.where.push("favorite = 1 AND kind IN ('image','video')");
  else if ((kind as string) === 'image-video') s.where.push("kind IN ('image','video')");
  else s.where.push("kind IN ('image','video','audio')");
}

function dirClause(folderId: number, dir: string, recursive: boolean, s: Scope) {
  s.where.push('folder_id = ?');
  s.params.push(folderId);
  if (recursive) {
    if (dir) {
      s.where.push('(dir = ? OR substr(dir, 1, ?) = ?)');
      s.params.push(dir, dir.length + 1, `${dir}/`);
    }
  } else {
    s.where.push('dir = ?');
    s.params.push(dir);
  }
}

function flagClause(flag: MediaFlagName, s: Scope) {
  if (flag === 'video') s.where.push("kind = 'video'");
  else {
    s.where.push('(flags & ?) != 0');
    s.params.push(MEDIA_FLAG[flag]);
  }
}

/** filters: from/to (localAt), place, camera, flag, free-text q (parsed by parseSearch) */
function filterClauses(ctx: Ctx, q: BrowseQuery, s: Scope) {
  if (q.from !== undefined) {
    s.where.push('taken_local >= ?');
    s.params.push(q.from);
  }
  if (q.to !== undefined) {
    s.where.push('taken_local < ?');
    s.params.push(q.to);
  }
  if (q.place) {
    const [cc, code] = q.place.split('-', 2);
    s.where.push('cc = ?');
    s.params.push(cc.toUpperCase());
    if (code !== undefined) {
      if (code === '00' || code === '') s.where.push("(admin1 IS NULL OR admin1 = '')");
      else {
        s.where.push('admin1 = ?');
        s.params.push(code);
      }
    }
  }
  if (q.camera) {
    const match = cameraLookup(ctx).get(q.camera.toLowerCase());
    if (!match) s.where.push('0');
    else {
      s.where.push('model = ? AND make IS ?');
      s.params.push(match.model, match.make);
    }
  }
  if (q.flag) flagClause(q.flag, s);
  if (q.q) {
    const p = parseSearch(q.q);
    if (p.flag && p.flag !== q.flag) flagClause(p.flag, s);
    if (p.from !== undefined && p.to !== undefined) {
      s.where.push('taken_local >= ? AND taken_local < ?');
      s.params.push(p.from, p.to);
    }
    if (p.month !== undefined) {
      s.where.push("CAST(strftime('%m', taken_local / 1000, 'unixepoch') AS INTEGER) = ?");
      s.params.push(p.month);
    }
    for (const t of p.terms) {
      s.where.push('instr(peek_search(name, city, cc, admin1, make, model), ?) > 0');
      s.params.push(t);
    }
  }
}

/** normalised camera key → stored make/model (few distinct values, so a lookup beats a SQL function) */
function cameraLookup(ctx: Ctx): Map<string, { make: string | null; model: string }> {
  const m = new Map<string, { make: string | null; model: string }>();
  for (const r of ctx.db.all<{ make: string | null; model: string }>('SELECT DISTINCT make, model FROM media WHERE model IS NOT NULL')) {
    m.set(cameraKey(r.make, r.model), r);
  }
  return m;
}

export interface ResolvedScope {
  s: Scope;
  /** false for a multi-folder library root without filters (only its folders are listed) */
  items: boolean;
  folder: FolderRow | null;
  lib: Library | null;
  dir: string;
}

/**
 * THE where-clause builder: location (library / folder / dir / recursive) + kind + filters.
 * Used by /api/browse, /api/browse/skeleton, /api/geo/points, /api/explore and /api/search.
 */
export function resolveScope(ctx: Ctx, q: BrowseQuery): ResolvedScope {
  const s: Scope = { where: [], params: [] };
  kindClause(q.kind, s);
  let folderId = q.folderId;
  let lib = q.libraryId ? ctx.lib.library(q.libraryId) : null;
  if (!folderId && lib && lib.folders.length === 1) folderId = lib.folders[0].id;
  const dir = q.dir ?? '';
  let items = true;
  let folder: FolderRow | null = null;
  if (folderId) {
    folder = ctx.lib.folderRow(folderId);
    if (!lib && folder.library_id != null) lib = ctx.lib.library(folder.library_id);
    dirClause(folder.id, dir, !!q.recursive || hasFilters(q), s);
  } else if (lib) {
    const ids = lib.folders.map((f) => Number(f.id));
    if (!ids.length) s.where.push('0');
    else {
      // unary + keeps the planner on the ORDER BY index (media_local) instead of sorting via media_dir
      s.where.push(`+folder_id IN (${ids.map(() => '?').join(',')})`);
      s.params.push(...ids);
    }
    items = !!q.recursive || hasFilters(q);
  } else {
    // home / timeline / explore / search: libraries only (temp sessions and linked phones are browsed explicitly)
    s.where.push('+folder_id IN (SELECT id FROM folders WHERE temp = 0 AND device_id IS NULL)');
  }
  filterClauses(ctx, q, s);
  if (!items) s.where.push('0');
  return { s, items, folder, lib, dir };
}

export function whereSql(s: Scope): string {
  return s.where.length ? `WHERE ${s.where.join(' AND ')}` : '';
}

export function orderSql(q: BrowseQuery): string {
  const col = SORT_SQL[q.sort ?? 'takenAt'] ?? 'taken_local';
  const dir: SortDir = q.order === 'asc' ? 'asc' : 'desc';
  return `ORDER BY ${col} ${dir}, id ${dir}`;
}

/** count + gen of a scope (gen = media table generation + count + max id) */
export function countAndGen(ctx: Ctx, s: Scope): { total: number; gen: string } {
  const r = ctx.db.get<{ c: number; m: number | null }>(`SELECT count(*) c, max(id) m FROM media ${whereSql(s)}`, ...s.params);
  const total = r?.c ?? 0;
  return { total, gen: `${ctx.events.gen}.${total}.${r?.m ?? 0}` };
}

function page(ctx: Ctx, s: Scope, q: BrowseQuery) {
  const limit = Math.max(1, Math.min(5000, Number(q.limit) || 600));
  const offset = Math.max(0, Number(q.offset) || 0);
  const { total, gen } = countAndGen(ctx, s);
  const rows = ctx.db.all(`SELECT ${ITEM_COLS} FROM media ${whereSql(s)} ${orderSql(q)} LIMIT ? OFFSET ?`, ...s.params, limit, offset);
  return { total, gen, items: rows.map(toItem) };
}

const skeletonCaches = new WeakMap<Ctx, Map<string, BrowseSkeleton>>();

/**
 * Exact layout skeleton: same WHERE + ORDER BY as the pages. Cached per media generation
 * (EventHub.gen changes on every layout-relevant write), so repeated requests are free.
 */
export function buildSkeleton(ctx: Ctx, q: BrowseQuery): BrowseSkeleton {
  let skeletonCache = skeletonCaches.get(ctx);
  if (!skeletonCache) skeletonCaches.set(ctx, (skeletonCache = new Map()));
  const key = `${ctx.events.gen}|${JSON.stringify({ ...q, offset: 0, limit: 0 })}`;
  const hit = skeletonCache.get(key);
  if (hit) return hit;
  const sk = computeSkeleton(ctx, q);
  if (skeletonCache.size >= 8) skeletonCache.delete(skeletonCache.keys().next().value!);
  skeletonCache.set(key, sk);
  return sk;
}

function computeSkeleton(ctx: Ctx, q: BrowseQuery): BrowseSkeleton {
  const { s } = resolveScope(ctx, q);
  const withDays = (q.sort ?? 'takenAt') === 'takenAt';
  // one query, rows as arrays: [id, width, height, kind, taken_local, city]
  const rows = ctx.db.allArrays(
    `SELECT id, width, height, kind${withDays ? ', taken_local, city' : ''} FROM media ${whereSql(s)} ${orderSql(q)}`,
    ...s.params,
  ) as [number, number | null, number | null, string, number?, string?][];
  const n = rows.length;
  let maxId = 0;
  const aspects = new Uint8Array(n);
  const kindCounts = { image: 0, video: 0, audio: 0 };
  const days: { key: string; count: number; place: string | null }[] | null = withDays ? [] : null;
  const DAY = 86_400_000;
  let curDay = NaN;
  let cities: Map<string, number> | null = null;
  const closeDay = () => {
    if (!days || !days.length || !cities) return;
    let best: string | null = null;
    let bestN = 0;
    for (const [c, k] of cities) if (k > bestN) [best, bestN] = [c, k];
    days[days.length - 1].place = best;
  };
  for (let i = 0; i < n; i++) {
    const [id, w, h, k, t, c] = rows[i];
    if (id > maxId) maxId = id;
    aspects[i] = aspectToByte(w && h ? w / h : 1);
    if (k === 'image' || k === 'video' || k === 'audio') kindCounts[k]++;
    if (days) {
      const lt = t ?? 0;
      const d = Math.floor(lt / DAY);
      if (d !== curDay) {
        closeDay();
        curDay = d;
        cities = new Map();
        days.push({ key: localDayKey(lt), count: 0, place: null });
      }
      days[days.length - 1].count++;
      if (c) cities!.set(c, (cities!.get(c) ?? 0) + 1);
    }
  }
  closeDay();
  const gen = `${ctx.events.gen}.${n}.${maxId}`;
  return { gen, total: n, kindCounts, aspects: Buffer.from(aspects.buffer, aspects.byteOffset, aspects.byteLength).toString('base64'), days };
}

/** immediate child directories of `dir` in a device folder (media dirs + dirs the phone reported) */
function deviceChildDirs(ctx: Ctx, folderId: number, dir: string): string[] {
  const prefix = dir ? `${dir}/` : '';
  const rows = ctx.db.all<{ dir: string }>(
    `SELECT DISTINCT dir FROM media WHERE folder_id = ? AND substr(dir, 1, ?) = ? AND dir != ?
     UNION SELECT dir FROM device_dirs WHERE folder_id = ? AND substr(dir, 1, ?) = ? AND dir != ?`,
    folderId,
    prefix.length,
    prefix,
    dir,
    folderId,
    prefix.length,
    prefix,
    dir,
  );
  const names = new Set<string>();
  for (const r of rows) {
    const child = r.dir.slice(prefix.length).split('/')[0];
    if (child && !ctx.scanner.skipName(child)) names.add(child);
  }
  return [...names];
}

async function subFolders(ctx: Ctx, folder: FolderRow, dir: string): Promise<SubFolder[]> {
  let names: string[];
  if (folder.device_id) names = deviceChildDirs(ctx, folder.id, dir);
  else {
    const abs = resolveInRoot(folder.path, dir);
    const entries = await fsp.readdir(abs, { withFileTypes: true }).catch(() => []);
    names = entries.filter((e) => e.isDirectory() && !ctx.scanner.skipName(e.name)).map((e) => e.name);
  }
  // counts for all descendants grouped by immediate child
  const prefix = dir ? `${dir}/` : '';
  const counts = new Map<string, number>();
  const rows = ctx.db.all<{ dir: string; c: number }>(
    `SELECT dir, count(*) c FROM media WHERE folder_id = ? AND kind IN ('image','video') AND substr(dir, 1, ?) = ? AND dir != ? GROUP BY dir`,
    folder.id,
    prefix.length,
    prefix,
    dir,
  );
  for (const r of rows) {
    const child = r.dir.slice(prefix.length).split('/')[0];
    counts.set(child, (counts.get(child) ?? 0) + r.c);
  }
  return names
    .sort((a, b) => a.localeCompare(b, undefined, { numeric: true, sensitivity: 'base' }))
    .map((name) => {
      const d = prefix + name;
      const covers = ctx.db.all<{ id: number }>(
        `SELECT id FROM media WHERE folder_id = ? AND kind IN ('image','video') AND (dir = ? OR substr(dir, 1, ?) = ?) ORDER BY taken_local DESC LIMIT 3`,
        folder.id,
        d,
        d.length + 1,
        `${d}/`,
      );
      return { name, dir: d, mediaCount: counts.get(name) ?? 0, coverIds: covers.map((c) => String(c.id)) };
    });
}

function archives(ctx: Ctx, folderId: number, dir: string) {
  return ctx.db
    .all(`SELECT ${ITEM_COLS} FROM media WHERE folder_id = ? AND dir = ? AND kind = 'archive' ORDER BY name`, folderId, dir)
    .map(toItem);
}

export const browseRoutes: Plugin = (app, ctx) => {
  app.get<{ Querystring: Record<string, string> }>('/api/browse', async (req): Promise<BrowseResult> => {
    const q = parseBrowseQuery(req.query);
    const { s, folder, lib, dir } = resolveScope(ctx, q);

    // ── a specific folder (library folder or temp session) ──
    if (folder) {
      const crumbs: Crumb[] = [];
      if (lib) crumbs.push({ label: lib.name, libraryId: lib.id });
      if (!lib || lib.folders.length > 1 || folder.temp) crumbs.push({ label: folder.name, libraryId: lib?.id, folderId: String(folder.id), dir: '' });
      let acc = '';
      for (const seg of dir.split('/').filter(Boolean)) {
        acc = acc ? `${acc}/${seg}` : seg;
        crumbs.push({ label: seg, libraryId: lib?.id, folderId: String(folder.id), dir: acc });
      }
      const { total, items, gen } = page(ctx, s, q);
      return {
        crumbs,
        folders: q.offset ? [] : await subFolders(ctx, folder, dir),
        roots: [],
        items,
        total,
        archives: q.offset ? [] : archives(ctx, folder.id, dir),
        // device folders: no uploads/drops (copy into them with /api/fs/move copy=true → transfer job)
        target: ctx.cfg.readonly || folder.device_id ? null : { folderId: String(folder.id), dir },
        gen,
      };
    }

    // ── library root with several folders ──
    if (lib) {
      const { total, items, gen } = page(ctx, s, q);
      return { crumbs: [{ label: lib.name, libraryId: lib.id }], folders: [], roots: lib.folders, items, total, archives: [], target: null, gen };
    }

    // ── everything (home / timeline) ──
    const { total, items, gen } = page(ctx, s, q);
    return { crumbs: [], folders: [], roots: [], items, total, archives: [], target: null, gen };
  });

  app.get<{ Querystring: Record<string, string> }>('/api/browse/skeleton', async (req): Promise<BrowseSkeleton> => {
    return buildSkeleton(ctx, parseBrowseQuery(req.query));
  });

  app.get<{ Querystring: Record<string, string> }>('/api/geo/points', async (req): Promise<GeoPoints> => {
    const { s } = resolveScope(ctx, parseBrowseQuery(req.query));
    const rows = ctx.db
      .stmt(`SELECT id, lat, lon FROM media ${whereSql({ where: [...s.where, 'lat IS NOT NULL', 'lon IS NOT NULL'], params: s.params })} ORDER BY id`)
      .all(...s.params) as { id: number; lat: number; lon: number }[];
    return { points: rows.map((r) => [String(r.id), Math.round(r.lat * 1e5) / 1e5, Math.round(r.lon * 1e5) / 1e5]) };
  });

  app.get<{ Querystring: { libraryId?: string; kind?: KindFilter } }>('/api/timeline', async (req): Promise<{ buckets: TimelineBucket[] }> => {
    const q = parseBrowseQuery({ libraryId: req.query.libraryId, recursive: '1' });
    q.kind = req.query.kind && req.query.kind !== 'all' ? req.query.kind : ('image-video' as KindFilter);
    const { s } = resolveScope(ctx, q);
    const rows = ctx.db.all<{ key: string; c: number }>(
      `SELECT strftime('%Y-%m', taken_local / 1000, 'unixepoch') key, count(*) c FROM media ${whereSql(s)} GROUP BY key ORDER BY key DESC`,
      ...s.params,
    );
    return { buckets: rows.map((r) => ({ key: r.key, count: r.c })) };
  });

  app.get<{ Querystring: { q: string; limit?: string } }>('/api/search', async (req) => {
    const term = (req.query.q ?? '').trim();
    if (!term) return { items: [], total: 0 };
    const q: BrowseQuery = { q: term, kind: 'image-video' as KindFilter, limit: Number(req.query.limit) || 200, sort: 'takenAt', order: 'desc' };
    const { s } = resolveScope(ctx, q);
    const { total, items } = page(ctx, s, q);
    return { items, total };
  });
};
