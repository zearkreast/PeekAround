import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import sharp from 'sharp';
import { excludeMatcher, normalizeLibraryOptions, type Album, type BrowseSkeleton, type Library, type LibraryStats, type MediaItem } from '@peekaround/shared';
import { createServer, type PeekServer } from '../src/app';
import { Db } from '../src/db';
import { MIGRATIONS } from '../src/migrations/index';

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function img(file: string, w: number, h: number, color: string, date?: string) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  let s = sharp({ create: { width: w, height: h, channels: 3, background: color } });
  if (date) s = s.withExif({ IFD2: { DateTimeOriginal: date } });
  await s.jpeg().toFile(file);
}

describe('exclusion matcher', () => {
  it('matches names, globs and paths case-insensitively', () => {
    const m = excludeMatcher(['Thumbnails', '*.tmp.jpg', 'WhatsApp/*/Sent', '**/.cache', 'stick?r']);
    expect(m.path('a/thumbnails/x.jpg')).toBe(true);
    expect(m.path('Thumbnails')).toBe(true);
    expect(m.path('a/b.tmp.jpg')).toBe(true);
    expect(m.path('a/b.jpg')).toBe(false);
    expect(m.path('WhatsApp/Media/Sent')).toBe(true);
    expect(m.path('WhatsApp/Media/Sent/x.jpg')).toBe(true);
    expect(m.path('WhatsApp/Sent')).toBe(false);
    expect(m.path('x/y/.cache/z.jpg')).toBe(true);
    expect(m.path('Sticker/a.jpg')).toBe(true);
    expect(m.path('')).toBe(false);
    expect(excludeMatcher([]).empty).toBe(true);
  });

  it('normalises options', () => {
    expect(normalizeLibraryOptions({ exclude: [' a ', 'a', '', '/b/'], minBytes: -5, showHidden: 'x' as never })).toEqual({ exclude: ['a', 'b'], minBytes: 0, showHidden: null });
    expect(normalizeLibraryOptions(null)).toEqual({ exclude: [], minBytes: 0, showHidden: null });
  });
});

describe('migration 005', () => {
  it('creates album tables and library / folder columns', () => {
    const db = new Db(':memory:');
    expect(db.get<{ user_version: number }>('PRAGMA user_version')!.user_version).toBe(MIGRATIONS.length);
    const tables = db.all<{ name: string }>("SELECT name FROM sqlite_master WHERE type = 'table'").map((r) => r.name);
    expect(tables).toEqual(expect.arrayContaining(['albums', 'album_items']));
    expect(db.all<{ name: string }>('PRAGMA table_info(libraries)').map((c) => c.name)).toEqual(expect.arrayContaining(['cover_media_id', 'options']));
    expect(db.all<{ name: string }>('PRAGMA table_info(folders)').map((c) => c.name)).toContain('scanned_at');
    db.close();
  });
});

describe('albums + library features', () => {
  let srv: PeekServer;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-albums-'));
  const lib = path.join(tmp, 'lib');
  const get = async <T>(url: string) => (await srv.app.inject(url)).json() as T;
  const send = async <T>(method: 'POST' | 'PATCH' | 'DELETE', url: string, payload?: object) => {
    const r = await srv.app.inject({ method, url, payload });
    if (r.statusCode >= 400) throw new Error(`${method} ${url} → ${r.statusCode} ${r.body}`);
    return r.json() as T;
  };
  const all = async () => (await get<{ items: MediaItem[] }>('/api/browse?libraryId=1&recursive=1&limit=500')).items;
  const byName = async (n: string) => (await all()).find((i) => i.name === n)!;

  beforeAll(async () => {
    await img(path.join(lib, 'a.jpg'), 400, 300, '#3b82f6', '2024:01:01 10:00:00');
    await img(path.join(lib, 'Tatil', 'b.jpg'), 300, 400, '#f97316', '2024:06:01 10:00:00');
    await img(path.join(lib, 'Tatil', 'Gün 2', 'c.jpg'), 500, 500, '#10b981', '2023:03:01 10:00:00');
    await img(path.join(lib, 'Thumbnails', 'thumb.jpg'), 400, 300, '#000000');
    await img(path.join(lib, 'icon.jpg'), 8, 8, '#ff0000');
    // noisy → well above the minimum size used below
    const noise = Buffer.alloc(200 * 150 * 3);
    for (let i = 0; i < noise.length; i++) noise[i] = (i * 7919) % 251;
    fs.mkdirSync(path.join(lib, '.hidden'), { recursive: true });
    await sharp(noise, { raw: { width: 200, height: 150, channels: 3 } }).jpeg().toFile(path.join(lib, '.hidden', 'h.jpg'));
    srv = await createServer({ dataDir: path.join(tmp, 'data'), roots: [{ path: lib, name: 'Lib' }], port: 0, host: '127.0.0.1', mdns: false });
    srv.ctx.events.intervals = { add: 0, remove: 0, change: 0, meta: 0, thumb: 0 };
    for (let i = 0; i < 80; i++) {
      const items = await all();
      if (items.length === 5 && items.every((x) => x.width)) break;
      await wait(100);
    }
  });

  afterAll(async () => {
    await srv.close();
  });

  it('library stats', async () => {
    const s = await get<LibraryStats>('/api/libraries/1/stats');
    expect(s.items).toBe(5);
    expect(s.photos).toBe(5);
    expect(s.videos).toBe(0);
    expect(s.bytes).toBeGreaterThan(0);
    expect(s.folders).toBe(4); // '', Tatil, Tatil/Gün 2, Thumbnails
    expect(s.from).toBeLessThan(s.to!);
    expect(s.scannedAt).toBeGreaterThan(0);
  });

  it('library exclusions: names, globs, minimum size and hidden folders', async () => {
    const lib1 = await send<Library>('PATCH', '/api/libraries/1', { options: { exclude: ['thumbnails'], minBytes: 1000 } });
    expect(lib1.options).toEqual({ exclude: ['thumbnails'], minBytes: 1000, showHidden: null });
    for (let i = 0; i < 50 && (await all()).length !== 3; i++) await wait(100);
    expect((await all()).map((i) => i.name).sort()).toEqual(['a.jpg', 'b.jpg', 'c.jpg']);
    // excluded folders are not listed either
    const root = await get<{ folders: { name: string }[] }>('/api/browse?folderId=1');
    expect(root.folders.map((f) => f.name)).toEqual(['Tatil']);
    // per-library hidden override
    await send('PATCH', '/api/libraries/1', { options: { showHidden: true } });
    for (let i = 0; i < 50 && (await all()).length !== 4; i++) await wait(100);
    expect((await all()).map((i) => i.name).sort()).toEqual(['a.jpg', 'b.jpg', 'c.jpg', 'h.jpg']);
    await send('PATCH', '/api/libraries/1', { options: { showHidden: null } });
    for (let i = 0; i < 50 && (await all()).length !== 3; i++) await wait(100);
    expect((await all()).length).toBe(3);
  });

  it('watcher / uploads honour exclusions', async () => {
    const body = fs.readFileSync(path.join(lib, 'a.jpg'));
    const r = await srv.app.inject({ method: 'PUT', url: '/api/upload?folderId=1&dir=&path=Thumbnails%2Fnew.jpg', payload: body, headers: { 'content-type': 'application/octet-stream' } });
    expect(r.statusCode).toBe(200);
    await wait(150);
    expect((await all()).some((i) => i.name === 'new.jpg')).toBe(false);
    // a partial rescan of a hidden / excluded dir (rename or move into it) indexes nothing there
    await srv.ctx.scanner.scanFolder(1, '.hidden');
    await srv.ctx.scanner.scanFolder(1, 'Thumbnails');
    expect((await all()).map((i) => i.name).sort()).toEqual(['a.jpg', 'b.jpg', 'c.jpg']);
  });

  it('library cover + order', async () => {
    const b = await byName('b.jpg');
    const l = await send<Library>('PATCH', '/api/libraries/1', { coverId: b.id });
    expect(l.coverId).toBe(b.id);
    const bad = await srv.app.inject({ method: 'PATCH', url: '/api/libraries/1', payload: { coverId: '999999' } });
    expect(bad.statusCode).toBe(400);
    await send('POST', '/api/libraries', { name: 'Second' });
    const ordered = await send<{ libraries: Library[] }>('POST', '/api/libraries/order', { ids: ['2', '1'] });
    expect(ordered.libraries.map((x) => x.name)).toEqual(['Second', 'Lib']);
  });

  let album: Album;
  it('album CRUD + items + browse / skeleton filter', async () => {
    const [a, b, c] = await Promise.all(['a.jpg', 'b.jpg', 'c.jpg'].map(byName));
    album = await send<Album>('POST', '/api/albums', { name: '  Yaz   2024 ', ids: [b.id, a.id] });
    expect(album.name).toBe('Yaz 2024');
    expect(album.count).toBe(2);
    expect(album.coverId).toBe(b.id); // newest item
    expect(album.coverPicked).toBe(false);
    const empty = await srv.app.inject({ method: 'POST', url: '/api/albums', payload: { name: ' ' } });
    expect(empty.statusCode).toBe(400);

    const add = await send<{ added: number; album: Album }>('POST', `/api/albums/${album.id}/items`, { ids: [c.id, a.id, 'x', 999999] });
    expect(add.added).toBe(1);
    expect(add.album.count).toBe(3);

    const page = await get<{ items: MediaItem[]; total: number }>(`/api/browse?albumId=${album.id}`);
    expect(page.total).toBe(3);
    expect(page.items.map((i) => i.name)).toEqual(['b.jpg', 'a.jpg', 'c.jpg']); // takenAt desc
    const sk = await get<BrowseSkeleton>(`/api/browse/skeleton?albumId=${album.id}`);
    expect(sk.total).toBe(3);
    expect(sk.days?.map((d) => d.count)).toEqual([1, 1, 1]);
    // filters still apply inside an album
    expect((await get<{ total: number }>(`/api/browse?albumId=${album.id}&from=${Date.UTC(2024, 0, 1)}`)).total).toBe(2);

    // custom order
    expect((await get<{ items: MediaItem[] }>(`/api/browse?albumId=${album.id}&byPosition=1`)).items.map((i) => i.name)).toEqual(['b.jpg', 'a.jpg', 'c.jpg']);
    await send('POST', `/api/albums/${album.id}/reorder`, { ids: [c.id], before: b.id });
    const custom = await get<{ items: MediaItem[] }>(`/api/browse?albumId=${album.id}&byPosition=1`);
    expect(custom.items.map((i) => i.name)).toEqual(['c.jpg', 'b.jpg', 'a.jpg']);
    const sk2 = await get<BrowseSkeleton>(`/api/browse/skeleton?albumId=${album.id}&byPosition=1`);
    expect(sk2.total).toBe(3);
    expect(sk2.days).toBeNull();
    expect((await get<Album>(`/api/albums/${album.id}`)).order).toBe('custom');

    // membership + cover + rename
    const mem = await send<{ counts: Record<string, number> }>('POST', '/api/albums/membership', { ids: [a.id, c.id] });
    expect(mem.counts[album.id]).toBe(2);
    const upd = await send<Album>('PATCH', `/api/albums/${album.id}`, { name: 'Yaz', coverId: c.id, order: 'asc' });
    expect([upd.name, upd.coverId, upd.coverPicked, upd.order]).toEqual(['Yaz', c.id, true, 'asc']);

    // remove
    const rm = await send<{ removed: number; album: Album }>('POST', `/api/albums/${album.id}/items/remove`, { ids: [c.id] });
    expect(rm.removed).toBe(1);
    expect(rm.album.count).toBe(2);
    expect(rm.album.coverPicked).toBe(false); // cover left with the item
  });

  it('album items survive renames and moves (same id), leave on delete', async () => {
    const a = await byName('a.jpg');
    const r = await send<{ rel: string }>('POST', '/api/fs/rename', { folderId: a.folderId, rel: 'a.jpg', name: 'renamed.jpg' });
    expect(r.rel).toBe('renamed.jpg');
    const renamed = await byName('renamed.jpg');
    expect(renamed.id).toBe(a.id);
    await send('POST', '/api/fs/move', { refs: [{ folderId: a.folderId, rel: 'renamed.jpg' }], to: { folderId: a.folderId, dir: 'Tatil' } });
    const moved = await byName('renamed.jpg');
    expect([moved.id, moved.dir]).toEqual([a.id, 'Tatil']);
    // directory rename keeps ids too
    const b = await byName('b.jpg');
    await send('POST', '/api/fs/rename', { folderId: b.folderId, rel: 'Tatil', name: 'Yaz' });
    const b2 = await byName('b.jpg');
    expect([b2.id, b2.dir]).toEqual([b.id, 'Yaz']);
    expect((await get<{ items: MediaItem[] }>(`/api/browse?albumId=${album.id}`)).items.map((i) => i.id).sort()).toEqual([a.id, b.id].sort());

    await send('POST', '/api/fs/delete', { refs: [{ folderId: a.folderId, rel: 'Yaz/renamed.jpg' }] });
    const after = await get<Album>(`/api/albums/${album.id}`);
    expect(after.count).toBe(1);
    expect((await get<{ total: number }>(`/api/browse?albumId=${album.id}`)).total).toBe(1);
  });

  it('album zip, list order and delete', async () => {
    const z = await srv.app.inject(`/api/download?albumId=${album.id}&name=Yaz`);
    expect(z.statusCode).toBe(200);
    expect(z.headers['content-type']).toBe('application/zip');
    const second = await send<Album>('POST', '/api/albums', { name: 'İkinci' });
    expect((await get<{ albums: Album[] }>('/api/albums')).albums.map((x) => x.name)).toEqual(['İkinci', 'Yaz']);
    await send('POST', '/api/albums/order', { ids: [album.id] });
    expect((await get<{ albums: Album[] }>('/api/albums')).albums.map((x) => x.name)).toEqual(['Yaz', 'İkinci']);
    await send('DELETE', `/api/albums/${second.id}`);
    await send('DELETE', `/api/albums/${album.id}`);
    expect((await get<{ albums: Album[] }>('/api/albums')).albums).toEqual([]);
    expect((await srv.app.inject(`/api/albums/${album.id}`)).statusCode).toBe(404);
    // media untouched
    expect((await all()).length).toBeGreaterThan(0);
  });

  it('reordering a 20k item album stays fast (no quadratic membership checks)', async () => {
    const db = srv.ctx.db;
    db.run("INSERT INTO folders (library_id, path, name, temp) VALUES (NULL, '/nonexistent-big', 'big', 1)");
    const fid = db.get<{ id: number }>("SELECT id FROM folders WHERE path = '/nonexistent-big'")!.id;
    db.tx(() => {
      for (let i = 0; i < 20_000; i++) db.run("INSERT INTO media (folder_id, dir, name, ext, kind) VALUES (?, '', ?, 'jpg', 'image')", fid, `x${i}.jpg`);
    });
    const ids = db.all<{ id: number }>('SELECT id FROM media WHERE folder_id = ? ORDER BY id', fid).map((r) => String(r.id));
    const big = await send<Album>('POST', '/api/albums', { name: 'Big', ids });
    expect(big.count).toBe(20_000);
    const t0 = Date.now();
    await send('POST', `/api/albums/${big.id}/reorder`, { ids: [...ids].reverse(), before: null });
    expect(Date.now() - t0).toBeLessThan(3000);
    const first = db.get<{ media_id: number }>('SELECT media_id FROM album_items WHERE album_id = ? ORDER BY position LIMIT 1', Number(big.id))!;
    expect(String(first.media_id)).toBe(ids[ids.length - 1]);
    await send('DELETE', `/api/albums/${big.id}`);
    db.run('DELETE FROM folders WHERE id = ?', fid);
  });

  it('sub folders carry their newest date', async () => {
    const r = await get<{ folders: { name: string; latest?: number | null }[] }>('/api/browse?folderId=1');
    expect(r.folders.find((f) => f.name === 'Yaz')?.latest).toBeGreaterThan(0);
  });
});
