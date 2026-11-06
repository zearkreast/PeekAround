import {
  calendarOf,
  MESSAGING_SQL,
  type BrowseQuery,
  type CalendarYear,
  type DuplicateGroup,
  type ExploreResult,
  type MediaItem,
  type MetaLibraryStatus,
  type MetaStatus,
} from '@peekaround/shared';
import type { Ctx } from '../context.js';
import { clearSidecarCache, META_V } from '../media/metadata.js';
import { ITEM_COLS, toItem } from '../media/mapper.js';
import { HttpError } from '../paths.js';
import { FS_LIMIT } from '../security.js';
import { parseBrowseQuery, resolveScope, whereSql } from './browse.js';
import type { Plugin } from './util.js';

/** the Explore scope of a request: location + kind only (images + videos by default) */
function exploreScope(ctx: Ctx, raw: Record<string, string>) {
  const q = parseBrowseQuery(raw);
  const eq: BrowseQuery = { libraryId: q.libraryId, folderId: q.folderId, dir: q.dir, kind: raw.kind ? q.kind : ('image-video' as BrowseQuery['kind']), recursive: true };
  return resolveScope(ctx, eq).s;
}

const statusMemo = new WeakMap<Ctx, { at: number; ms: number; v: MetaStatus }>();

/**
 * per-library metadata diagnostics (Settings → Location & metadata). The aggregate reads every row
 * (~100 ms for 100k items) and the settings card polls it while indexing: a slow result is reused
 * for 10× its compute time (fresh = true skips that).
 */
export function metaStatus(ctx: Ctx, fresh = false): MetaStatus {
  const m = statusMemo.get(ctx);
  if (!fresh && m && m.ms >= 30 && Date.now() - m.at < Math.min(10_000, m.ms * 10)) return m.v;
  const t0 = performance.now();
  const v = metaStatusNow(ctx);
  statusMemo.set(ctx, { at: Date.now(), ms: performance.now() - t0, v });
  return v;
}

function metaStatusNow(ctx: Ctx): MetaStatus {
  const rows = ctx.db.all<{ lib: number; name: string; total: number; loc: number; pending: number; bad: number; msg: number }>(
    `SELECT l.id lib, l.name name, count(m.id) total,
       coalesce(sum(m.lat IS NOT NULL), 0) loc,
       coalesce(sum(m.lat IS NULL AND (m.meta_state IN (0, 3) OR (m.meta_state = 1 AND m.meta_v < ?))), 0) pending,
       coalesce(sum(m.meta_state = 2), 0) bad,
       coalesce(sum(m.lat IS NULL AND ${MESSAGING_SQL.replace(/\b(name|dir)\b/g, 'm.$1')}), 0) msg
     FROM libraries l
     LEFT JOIN folders f ON f.library_id = l.id AND f.temp = 0
     LEFT JOIN media m ON m.folder_id = f.id AND m.kind IN ('image','video')
     GROUP BY l.id ORDER BY l.sort, l.id`,
    META_V,
  );
  const libraries: MetaLibraryStatus[] = rows.map((r) => ({
    id: String(r.lib),
    name: r.name,
    total: r.total,
    withLocation: r.loc,
    withoutLocation: Math.max(0, r.total - r.loc - r.pending - r.bad),
    pending: r.pending,
    unreadable: r.bad,
    messaging: r.msg,
  }));
  return { libraries, enriching: ctx.scanner.enriching || ctx.scanner.busy, indexing: ctx.scanner.indexing() };
}

const rekicks = new WeakMap<Ctx, NodeJS.Timeout>();

/**
 * Read the metadata of a library (or everything) again: rows go back to the enrichment queue
 * (unreadable ones to the full meta pass). The enrichment loop only picks rows up when it (re)starts,
 * so it is re-kicked until the queue is empty.
 */
export function rereadMeta(ctx: Ctx, libraryId?: string): number {
  const where = libraryId
    ? 'folder_id IN (SELECT id FROM folders WHERE library_id = ? AND temp = 0 AND device_id IS NULL AND remote IS NULL)'
    : 'folder_id IN (SELECT id FROM folders WHERE temp = 0 AND device_id IS NULL AND remote IS NULL)';
  const params = libraryId ? [Number(libraryId)] : [];
  if (libraryId && !ctx.db.get('SELECT id FROM libraries WHERE id = ?', Number(libraryId))) throw new HttpError(404, 'Library not found');
  const n = ctx.db.tx(() => {
    const a = Number(ctx.db.run(`UPDATE media SET meta_v = 0 WHERE meta_state = 1 AND kind IN ('image','video') AND ${where}`, ...params).changes);
    const b = Number(ctx.db.run(`UPDATE media SET meta_state = 0 WHERE meta_state = 2 AND kind IN ('image','video') AND ${where}`, ...params).changes);
    return a + b;
  });
  clearSidecarCache();
  ctx.scanner.kick();
  ctx.scanner.kickEnrich();
  if (!rekicks.has(ctx)) {
    const started = Date.now();
    const t = setInterval(() => {
      const left = ctx.db.get<{ c: number }>(
        "SELECT count(*) c FROM media WHERE meta_state = 1 AND meta_v < ? AND kind IN ('image','video') AND folder_id IN (SELECT id FROM folders WHERE remote IS NULL)",
        META_V,
      )?.c;
      if (!left || Date.now() - started > 6 * 3600_000) {
        clearInterval(t);
        rekicks.delete(ctx);
        return;
      }
      ctx.scanner.kickEnrich();
    }, 3000);
    t.unref?.();
    rekicks.set(ctx, t);
  }
  ctx.explore.invalidate(true);
  return n;
}

/**
 * GET  /api/explore[?libraryId=&folderId=&dir=&kind=…] → ExploreResult (memories, places, types, cameras, years,
 *      insights: stats, year reviews, countries/cities, colours, time of day, orientation, video length,
 *      largest, recent, duplicates, calendar years, indexing).
 * GET  /api/explore/calendar?year=2024              → CalendarYear (items per day)
 * GET  /api/explore/duplicates?offset=&limit=       → duplicate groups + their items
 * GET  /api/meta/status                             → MetaStatus (per library: with / without location, pending …)
 * POST /api/meta/reread {libraryId?}                → re-read metadata (enrichment queue)
 * /api/geo/points lives in browse.ts (same filters as browse).
 */
export const exploreRoutes: Plugin = (app, ctx) => {
  app.get<{ Querystring: Record<string, string> }>('/api/explore', async (req): Promise<ExploreResult> => {
    const q = parseBrowseQuery(req.query);
    // only location + kind matter for Explore (it IS the grouping of everything inside)
    return ctx.explore.get({ libraryId: q.libraryId, folderId: q.folderId, dir: q.dir, kind: req.query.kind ? q.kind : undefined });
  });

  app.get<{ Querystring: Record<string, string> }>('/api/explore/calendar', async (req): Promise<CalendarYear> => {
    const year = Number(req.query.year);
    if (!Number.isInteger(year) || year < 1900 || year > 2200) throw new HttpError(400, 'Invalid year');
    const s = exploreScope(ctx, req.query);
    const from = Date.UTC(year, 0, 1);
    const to = Date.UTC(year + 1, 0, 1);
    const rows = ctx.db.all<{ d: number; c: number }>(
      `SELECT CAST((taken_local - ?) / 86400000 AS INTEGER) d, count(*) c FROM media ${whereSql({ where: [...s.where, 'taken_local >= ?', 'taken_local < ?'], params: [] })} GROUP BY d`,
      from,
      ...s.params,
      from,
      to,
    );
    const cal = calendarOf(year, []);
    for (const r of rows) {
      if (r.d < 0 || r.d >= cal.days.length) continue;
      cal.days[r.d] = r.c;
      cal.total += r.c;
      cal.max = Math.max(cal.max, r.c);
    }
    return cal;
  });

  app.get<{ Querystring: Record<string, string> }>(
    '/api/explore/duplicates',
    FS_LIMIT,
    async (req): Promise<{ groups: DuplicateGroup[]; total: number; items: MediaItem[]; pending: number; paths: Record<string, string> }> => {
      const s = exploreScope(ctx, req.query);
      const offset = Math.max(0, Number(req.query.offset) || 0);
      const limit = Math.max(1, Math.min(500, Number(req.query.limit) || 100));
      const { groups, total } = ctx.explore.dups.groups(whereSql(s), s.params, offset, limit);
      const ids = groups.flatMap((g) => g.ids.map(Number));
      const items: MediaItem[] = [];
      const paths: Record<string, string> = {};
      for (let i = 0; i < ids.length; i += 400) {
        const chunk = ids.slice(i, i + 400);
        const rows = ctx.db.all<Record<string, any>>(
          `SELECT ${ITEM_COLS}, (SELECT name FROM folders f WHERE f.id = media.folder_id) fname FROM media WHERE id IN (${chunk.map(() => '?').join(',')})`,
          ...chunk,
        );
        for (const r of rows) {
          items.push(toItem(r));
          paths[String(r.id)] = [r.fname, r.dir].filter(Boolean).join('/');
        }
      }
      const pending = ctx.explore.dups.pending();
      if (pending) ctx.explore.dups.kick();
      return { groups, total, items, pending, paths };
    },
  );

  app.get('/api/meta/status', async (): Promise<MetaStatus> => metaStatus(ctx));

  app.post<{ Body: { libraryId?: string } }>('/api/meta/reread', FS_LIMIT, async (req) => {
    const id = req.body?.libraryId;
    const queued = rereadMeta(ctx, id ? String(id) : undefined);
    return { queued, status: metaStatus(ctx, true) };
  });
};
