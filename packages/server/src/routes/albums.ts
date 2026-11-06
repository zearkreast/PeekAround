import type { FastifyRequest } from 'fastify';
import type { Album, AlbumOrder } from '@peekaround/shared';
import type { Ctx } from '../context.js';
import { viewerOf } from '../devices/access.js';
import { HttpError } from '../paths.js';
import { FS_LIMIT } from '../security.js';
import type { Plugin } from './util.js';

/**
 * Albums — user-curated collections (migration 005).
 *
 *   GET    /api/albums                     list (sort order)
 *   POST   /api/albums                     { name, ids? } → Album
 *   POST   /api/albums/order               { ids } new list order
 *   POST   /api/albums/membership          { ids } → { counts: { albumId: n } } (picker check marks)
 *   GET    /api/albums/:id                 Album
 *   PATCH  /api/albums/:id                 { name?, coverId?, order? }
 *   DELETE /api/albums/:id                 (media untouched)
 *   POST   /api/albums/:id/items           { ids } add (at the end of the custom order)
 *   POST   /api/albums/:id/items/remove    { ids }
 *   POST   /api/albums/:id/reorder         { ids, before? } move ids (in this order) before `before` (null = end)
 *
 * Items are browsed with /api/browse?albumId=… (+ byPosition=1 for the custom order) and zipped
 * with /api/download?albumId=…. Album rows only reference media ids: renames / moves inside
 * PeekAround keep them (scanner.relocate*), deleted media leave every album (FK cascade).
 * Albums are database-only (like favourites), so they work in read-only mode too.
 */

const ORDERS = new Set<AlbumOrder>(['desc', 'asc', 'custom']);
const MAX_IDS = 20_000;

interface AlbumRow {
  id: number;
  name: string;
  cover_media_id: number | null;
  item_order: string;
  created: number;
  updated: number;
  sort: number;
  n: number;
  p: number | null;
  v: number | null;
  f: number | null;
  t: number | null;
  auto: number | null;
}

const LIST_SQL = `
SELECT a.*, count(m.id) n, sum(m.kind = 'image') p, sum(m.kind = 'video') v, min(m.taken_local) f, max(m.taken_local) t,
  (SELECT ai2.media_id FROM album_items ai2 JOIN media m2 ON m2.id = ai2.media_id
    WHERE ai2.album_id = a.id AND m2.kind IN ('image','video') ORDER BY m2.taken_local DESC LIMIT 1) auto
FROM albums a
LEFT JOIN album_items ai ON ai.album_id = a.id
LEFT JOIN media m ON m.id = ai.media_id
`;

function toAlbum(r: AlbumRow): Album {
  const cover = r.cover_media_id ?? r.auto;
  return {
    id: String(r.id),
    name: r.name,
    coverId: cover == null ? null : String(cover),
    coverPicked: r.cover_media_id != null,
    count: r.n,
    photos: r.p ?? 0,
    videos: r.v ?? 0,
    from: r.n ? r.f : null,
    to: r.n ? r.t : null,
    order: ORDERS.has(r.item_order as AlbumOrder) ? (r.item_order as AlbumOrder) : 'desc',
    created: r.created,
    updated: r.updated,
    sort: r.sort,
  };
}

export class AlbumService {
  constructor(private ctx: Ctx) {}

  list(): Album[] {
    return this.ctx.db.all<AlbumRow>(`${LIST_SQL} GROUP BY a.id ORDER BY a.sort, a.id`).map(toAlbum);
  }

  get(id: string | number): Album {
    const r = this.ctx.db.get<AlbumRow>(`${LIST_SQL} WHERE a.id = ? GROUP BY a.id`, Number(id));
    if (!r) throw new HttpError(404, 'Album not found');
    return toAlbum(r);
  }

  private exists(id: string | number): number {
    const r = this.ctx.db.get<{ id: number }>('SELECT id FROM albums WHERE id = ?', Number(id));
    if (!r) throw new HttpError(404, 'Album not found');
    return r.id;
  }

  private changed(ids: number[], layout = true) {
    // album results are cached per media generation (browse skeleton) → membership / order changes bump it
    if (layout) this.ctx.events.bump();
    this.ctx.events.emit({ type: 'albums', ids: ids.map(String) });
  }

  create(name: string, ids: unknown[] = []): Album {
    const now = Date.now();
    const clean = cleanName(name);
    // new albums go first in the list
    this.ctx.db.run('UPDATE albums SET sort = sort + 1');
    const id = Number(this.ctx.db.run('INSERT INTO albums (name, item_order, created, updated, sort) VALUES (?, ?, ?, ?, 0)', clean, 'desc', now, now).lastInsertRowid);
    if (ids.length) this.add(id, ids, false);
    this.changed([id]);
    return this.get(id);
  }

  update(id: string, patch: { name?: unknown; coverId?: unknown; order?: unknown }): Album {
    const aid = this.exists(id);
    const db = this.ctx.db;
    if (typeof patch.name === 'string') db.run('UPDATE albums SET name = ? WHERE id = ?', cleanName(patch.name), aid);
    if (patch.coverId !== undefined) {
      if (patch.coverId === null || patch.coverId === '') db.run('UPDATE albums SET cover_media_id = NULL WHERE id = ?', aid);
      else {
        const m = db.get<{ media_id: number }>('SELECT media_id FROM album_items WHERE album_id = ? AND media_id = ?', aid, Number(patch.coverId));
        if (!m) throw new HttpError(400, 'The cover must be an item of the album');
        db.run('UPDATE albums SET cover_media_id = ? WHERE id = ?', m.media_id, aid);
      }
    }
    if (patch.order !== undefined) {
      if (!ORDERS.has(patch.order as AlbumOrder)) throw new HttpError(400, 'Unknown order');
      db.run('UPDATE albums SET item_order = ? WHERE id = ?', patch.order, aid);
    }
    db.run('UPDATE albums SET updated = ? WHERE id = ?', Date.now(), aid);
    this.changed([aid], false);
    return this.get(aid);
  }

  remove(id: string) {
    const aid = this.exists(id);
    this.ctx.db.run('DELETE FROM albums WHERE id = ?', aid);
    this.changed([aid]);
  }

  /** add media (existing members keep their place); returns how many were new */
  add(id: string | number, ids: unknown[], emit = true): number {
    const aid = this.exists(id);
    const media = mediaIds(ids);
    const db = this.ctx.db;
    let added = 0;
    db.tx(() => {
      let pos = db.get<{ p: number | null }>('SELECT max(position) p FROM album_items WHERE album_id = ?', aid)?.p ?? 0;
      const now = Date.now();
      for (const m of media) {
        if (!db.get('SELECT 1 FROM media WHERE id = ?', m)) continue;
        const r = db.run('INSERT OR IGNORE INTO album_items (album_id, media_id, added, position) VALUES (?, ?, ?, ?)', aid, m, now, pos + 1);
        if (Number(r.changes)) {
          pos += 1;
          added++;
        }
      }
      db.run('UPDATE albums SET updated = ? WHERE id = ?', now, aid);
    });
    if (emit) this.changed([aid]);
    return added;
  }

  removeItems(id: string, ids: unknown[]): number {
    const aid = this.exists(id);
    const media = mediaIds(ids);
    const db = this.ctx.db;
    let n = 0;
    db.tx(() => {
      for (const m of media) n += Number(db.run('DELETE FROM album_items WHERE album_id = ? AND media_id = ?', aid, m).changes);
      db.run('UPDATE albums SET cover_media_id = NULL WHERE id = ? AND cover_media_id IS NOT NULL AND cover_media_id NOT IN (SELECT media_id FROM album_items WHERE album_id = ?)', aid, aid);
      db.run('UPDATE albums SET updated = ? WHERE id = ?', Date.now(), aid);
    });
    this.changed([aid]);
    return n;
  }

  /** move `ids` (keeping their given order) right before `before` (null / unknown = to the end) */
  reorder(id: string, ids: unknown[], before: unknown) {
    const aid = this.exists(id);
    const move = mediaIds(ids);
    const db = this.ctx.db;
    const order = db.all<{ media_id: number }>('SELECT media_id FROM album_items WHERE album_id = ? ORDER BY position, added, media_id', aid).map((r) => r.media_id);
    // Sets: albums and selections can hold 20k items (includes() would be quadratic)
    const members = new Set(order);
    const moving = move.filter((m) => members.has(m));
    if (!moving.length) return;
    const movingSet = new Set(moving);
    const rest = order.filter((m) => !movingSet.has(m));
    const b = before === null || before === undefined || before === '' ? -1 : rest.indexOf(Number(before));
    const at = b < 0 ? rest.length : b;
    const next = [...rest.slice(0, at), ...moving, ...rest.slice(at)];
    db.tx(() => {
      next.forEach((m, i) => db.run('UPDATE album_items SET position = ? WHERE album_id = ? AND media_id = ?', i + 1, aid, m));
      db.run("UPDATE albums SET item_order = 'custom', updated = ? WHERE id = ?", Date.now(), aid);
    });
    this.changed([aid]);
  }

  /** new list order (ids first → last, the rest keep their relative order after them) */
  setOrder(ids: unknown[]) {
    const db = this.ctx.db;
    const all = db.all<{ id: number }>('SELECT id FROM albums ORDER BY sort, id').map((r) => r.id);
    const known = new Set(all);
    const want = mediaIds(ids).filter((x) => known.has(x));
    const wanted = new Set(want);
    const rest = all.filter((x) => !wanted.has(x));
    db.tx(() => [...want, ...rest].forEach((x, i) => db.run('UPDATE albums SET sort = ? WHERE id = ?', i, x)));
    this.changed(all, false);
  }

  membership(ids: unknown[]): Record<string, number> {
    const media = mediaIds(ids);
    if (!media.length) return {};
    const out: Record<string, number> = {};
    // chunks: SQLite parameter limits
    for (let i = 0; i < media.length; i += 500) {
      const chunk = media.slice(i, i + 500);
      for (const r of this.ctx.db.all<{ album_id: number; c: number }>(
        `SELECT album_id, count(*) c FROM album_items WHERE media_id IN (${chunk.map(() => '?').join(',')}) GROUP BY album_id`,
        ...chunk,
      ))
        out[String(r.album_id)] = (out[String(r.album_id)] ?? 0) + r.c;
    }
    return out;
  }
}

function cleanName(name: unknown): string {
  const s = typeof name === 'string' ? name.replace(/\s+/g, ' ').trim().slice(0, 120) : '';
  if (!s) throw new HttpError(400, 'Album name required');
  return s;
}

function mediaIds(ids: unknown[]): number[] {
  if (!Array.isArray(ids)) throw new HttpError(400, 'ids must be an array');
  if (ids.length > MAX_IDS) throw new HttpError(413, 'Too many items');
  const out: number[] = [];
  const seen = new Set<number>();
  for (const v of ids) {
    const n = Number(v);
    if (Number.isInteger(n) && n > 0 && !seen.has(n)) {
      seen.add(n);
      out.push(n);
    }
  }
  return out;
}

/** true when this viewer may not see linked-phone media (LAN viewers / the phones themselves) */
export function hideDeviceMedia(ctx: Ctx, req: FastifyRequest): boolean {
  if (!ctx.devices.hasFolders()) return false;
  const v = viewerOf(ctx, req);
  return !v.devices || !!v.self;
}

const services = new WeakMap<Ctx, AlbumService>();
export function albumService(ctx: Ctx): AlbumService {
  let s = services.get(ctx);
  if (!s) services.set(ctx, (s = new AlbumService(ctx)));
  return s;
}

export const albumRoutes: Plugin = (app, ctx) => {
  const albums = albumService(ctx);
  type Body = { name?: unknown; ids?: unknown[]; coverId?: unknown; order?: unknown; before?: unknown };

  app.get('/api/albums', FS_LIMIT, async () => ({ albums: albums.list() }));
  app.post<{ Body: Body }>('/api/albums', FS_LIMIT, async (req) => albums.create(String(req.body?.name ?? ''), req.body?.ids ?? []));
  app.post<{ Body: Body }>('/api/albums/order', FS_LIMIT, async (req) => {
    albums.setOrder(req.body?.ids ?? []);
    return { albums: albums.list() };
  });
  app.post<{ Body: Body }>('/api/albums/membership', FS_LIMIT, async (req) => ({ counts: albums.membership(req.body?.ids ?? []) }));
  app.get<{ Params: { id: string } }>('/api/albums/:id', FS_LIMIT, async (req) => albums.get(req.params.id));
  app.patch<{ Params: { id: string }; Body: Body }>('/api/albums/:id', FS_LIMIT, async (req) => albums.update(req.params.id, req.body ?? {}));
  app.delete<{ Params: { id: string } }>('/api/albums/:id', FS_LIMIT, async (req) => {
    albums.remove(req.params.id);
    return { ok: true };
  });
  app.post<{ Params: { id: string }; Body: Body }>('/api/albums/:id/items', FS_LIMIT, async (req) => {
    const added = albums.add(req.params.id, req.body?.ids ?? []);
    return { added, album: albums.get(req.params.id) };
  });
  app.post<{ Params: { id: string }; Body: Body }>('/api/albums/:id/items/remove', FS_LIMIT, async (req) => {
    const removed = albums.removeItems(req.params.id, req.body?.ids ?? []);
    return { removed, album: albums.get(req.params.id) };
  });
  app.post<{ Params: { id: string }; Body: Body }>('/api/albums/:id/reorder', FS_LIMIT, async (req) => {
    albums.reorder(req.params.id, req.body?.ids ?? [], req.body?.before);
    return { ok: true };
  });
};
