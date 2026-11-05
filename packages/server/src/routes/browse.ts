import fsp from 'node:fs/promises';
import type { BrowseQuery, BrowseResult, Crumb, KindFilter, SortDir, SortKey, SubFolder, TimelineBucket } from '@peekaround/shared';
import type { Ctx } from '../context.js';
import type { FolderRow } from '../library.js';
import { ITEM_COLS, toItem } from '../media/mapper.js';
import { normalizeRel, resolveInRoot } from '../paths.js';
import type { Plugin } from './util.js';

const SORT_SQL: Record<SortKey, string> = {
  takenAt: 'taken_at',
  name: 'name COLLATE NOCASE',
  mtime: 'mtime',
  size: 'size',
  kind: 'kind',
};

interface Scope {
  where: string[];
  params: any[];
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

function query(ctx: Ctx, s: Scope, q: BrowseQuery) {
  if (q.q) {
    s.where.push("name LIKE ? ESCAPE '\\'");
    s.params.push(`%${q.q.replace(/[\\%_]/g, (m) => `\\${m}`)}%`);
  }
  const order = SORT_SQL[q.sort ?? 'takenAt'] ?? 'taken_at';
  const dir: SortDir = q.order === 'asc' ? 'asc' : 'desc';
  const where = s.where.length ? `WHERE ${s.where.join(' AND ')}` : '';
  const limit = Math.max(1, Math.min(5000, Number(q.limit) || 600));
  const offset = Math.max(0, Number(q.offset) || 0);
  const total = ctx.db.get<{ c: number }>(`SELECT count(*) c FROM media ${where}`, ...s.params)?.c ?? 0;
  const rows = ctx.db.all(
    `SELECT ${ITEM_COLS} FROM media ${where} ORDER BY ${order} ${dir}, id ${dir} LIMIT ? OFFSET ?`,
    ...s.params,
    limit,
    offset,
  );
  return { total, items: rows.map(toItem) };
}

async function subFolders(ctx: Ctx, folder: FolderRow, dir: string): Promise<SubFolder[]> {
  const abs = resolveInRoot(folder.path, dir);
  const entries = await fsp.readdir(abs, { withFileTypes: true }).catch(() => []);
  const names = entries.filter((e) => e.isDirectory() && !ctx.scanner.skipName(e.name)).map((e) => e.name);
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
        `SELECT id FROM media WHERE folder_id = ? AND kind IN ('image','video') AND (dir = ? OR substr(dir, 1, ?) = ?) ORDER BY taken_at DESC LIMIT 3`,
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
    const raw = req.query;
    const q: BrowseQuery = {
      libraryId: raw.libraryId || undefined,
      folderId: raw.folderId || undefined,
      dir: normalizeRel(raw.dir),
      recursive: raw.recursive === '1' || raw.recursive === 'true',
      sort: (raw.sort as SortKey) || 'takenAt',
      order: (raw.order as SortDir) || 'desc',
      kind: (raw.kind as KindFilter) || 'all',
      q: raw.q || undefined,
      offset: Number(raw.offset) || 0,
      limit: Number(raw.limit) || 600,
    };
    const s: Scope = { where: [], params: [] };
    kindClause(q.kind, s);

    // ── a specific folder (library folder or temp session) ──
    let folderId = q.folderId;
    let lib = q.libraryId ? ctx.lib.library(q.libraryId) : null;
    if (!folderId && lib && lib.folders.length === 1) folderId = lib.folders[0].id;

    if (folderId) {
      const folder = ctx.lib.folderRow(folderId);
      if (!lib && folder.library_id != null) lib = ctx.lib.library(folder.library_id);
      const dir = q.dir ?? '';
      const crumbs: Crumb[] = [];
      if (lib) crumbs.push({ label: lib.name, libraryId: lib.id });
      if (!lib || lib.folders.length > 1 || folder.temp) crumbs.push({ label: folder.name, libraryId: lib?.id, folderId: String(folder.id), dir: '' });
      let acc = '';
      for (const seg of dir.split('/').filter(Boolean)) {
        acc = acc ? `${acc}/${seg}` : seg;
        crumbs.push({ label: seg, libraryId: lib?.id, folderId: String(folder.id), dir: acc });
      }
      dirClause(folder.id, dir, !!q.recursive || !!q.q, s);
      const { total, items } = query(ctx, s, q);
      return {
        crumbs,
        folders: q.offset ? [] : await subFolders(ctx, folder, dir),
        roots: [],
        items,
        total,
        archives: q.offset ? [] : archives(ctx, folder.id, dir),
        target: ctx.cfg.readonly ? null : { folderId: String(folder.id), dir },
      };
    }

    // ── library root with several folders ──
    if (lib) {
      const ids = lib.folders.map((f) => Number(f.id));
      if (!ids.length) return { crumbs: [{ label: lib.name, libraryId: lib.id }], folders: [], roots: [], items: [], total: 0, archives: [], target: null };
      s.where.push(`folder_id IN (${ids.map(() => '?').join(',')})`);
      s.params.push(...ids);
      const { total, items } = q.recursive || q.q ? query(ctx, s, q) : { total: 0, items: [] };
      return {
        crumbs: [{ label: lib.name, libraryId: lib.id }],
        folders: [],
        roots: lib.folders,
        items,
        total,
        archives: [],
        target: null,
      };
    }

    // ── everything (home / timeline) ──
    s.where.push('folder_id IN (SELECT id FROM folders WHERE temp = 0)');
    const { total, items } = query(ctx, s, q);
    return { crumbs: [], folders: [], roots: [], items, total, archives: [], target: null };
  });

  app.get<{ Querystring: { libraryId?: string; kind?: KindFilter } }>('/api/timeline', async (req): Promise<{ buckets: TimelineBucket[] }> => {
    const s: Scope = { where: [], params: [] };
    kindClause(req.query.kind && req.query.kind !== 'all' ? req.query.kind : 'image-video' as KindFilter, s);
    if (req.query.libraryId) {
      const lib = ctx.lib.library(req.query.libraryId);
      const ids = lib.folders.map((f) => Number(f.id));
      if (!ids.length) return { buckets: [] };
      s.where.push(`folder_id IN (${ids.map(() => '?').join(',')})`);
      s.params.push(...ids);
    } else {
      s.where.push('folder_id IN (SELECT id FROM folders WHERE temp = 0)');
    }
    const rows = ctx.db.all<{ key: string; c: number }>(
      `SELECT strftime('%Y-%m', taken_at / 1000, 'unixepoch', 'localtime') key, count(*) c FROM media WHERE ${s.where.join(' AND ')} GROUP BY key ORDER BY key DESC`,
      ...s.params,
    );
    return { buckets: rows.map((r) => ({ key: r.key, count: r.c })) };
  });

  app.get<{ Querystring: { q: string; limit?: string } }>('/api/search', async (req) => {
    const term = (req.query.q ?? '').trim();
    if (!term) return { items: [], total: 0 };
    const s: Scope = { where: ["kind IN ('image','video')", 'folder_id IN (SELECT id FROM folders WHERE temp = 0)'], params: [] };
    return query(ctx, s, { q: term, limit: Number(req.query.limit) || 200, sort: 'takenAt', order: 'desc' });
  });

};
