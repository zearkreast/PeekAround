import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import sharp from 'sharp';
import { describe, expect, it } from 'vitest';
import { createServer } from '../src/app';
import { Db } from '../src/db';
import { MIGRATIONS } from '../src/migrations/index';

/** A 1.1 database (migrations 001 + 002 applied, real rows) opened by 1.2: 003–005 on top, data kept. */
describe('upgrading a 1.1 data dir', () => {
  it('runs 003–005 on an existing 1.1 database, keeps rows and works end to end', async () => {
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-up11-'));
    const lib = path.join(tmp, 'lib');
    const dataDir = path.join(tmp, 'data');
    fs.mkdirSync(lib, { recursive: true });
    fs.mkdirSync(dataDir, { recursive: true });
    await sharp({ create: { width: 40, height: 20, channels: 3, background: '#123' } }).jpeg().toFile(path.join(lib, 'one.jpg'));
    await sharp({ create: { width: 20, height: 40, channels: 3, background: '#456' } }).jpeg().toFile(path.join(lib, 'two.jpg'));
    const dbFile = path.join(dataDir, 'peekaround.db');

    // build the 1.1 schema with the real runner (only the 1.1 migrations registered)
    const later = MIGRATIONS.splice(2);
    try {
      const old = new Db(dbFile);
      expect(old.get<{ user_version: number }>('PRAGMA user_version')!.user_version).toBe(2);
      old.run("INSERT INTO libraries (id, name, color, icon, created_at) VALUES (1, 'Lib', '#fff', 'images', 0)");
      old.run("INSERT INTO folders (id, library_id, path, name) VALUES (1, 1, ?, 'lib')", lib);
      old.run("INSERT INTO devices (id, name, created_at, paired) VALUES ('ph1', 'Phone', 0, 1)");
      old.run("INSERT INTO folders (id, library_id, path, name, device_id, device_key) VALUES (2, NULL, 'device://ph1/cam', 'Camera', 'ph1', 'cam')");
      const ins = (folder: number, name: string, size: number, mtime: number, lat: number | null, fav = 0) =>
        old.run(
          `INSERT INTO media (folder_id, dir, name, ext, kind, size, mtime, taken_at, taken_local, time_src, meta_state, meta_v, thumb_state, width, height, favorite, lat, lon, thumbhash)
           VALUES (?, '', ?, 'jpg', 'image', ?, ?, ?, ?, 1, 1, 1, 1, 40, 20, ?, ?, ?, 'YJqGPQw7sFlslqhFafSE+Q6oJ1h2iHB2Rw')`,
          folder,
          name,
          size,
          mtime,
          mtime,
          mtime,
          fav,
          lat,
          lat === null ? null : 29,
        );
      ins(1, 'one.jpg', fs.statSync(path.join(lib, 'one.jpg')).size, Math.round(fs.statSync(path.join(lib, 'one.jpg')).mtimeMs), 41, 1);
      ins(1, 'two.jpg', fs.statSync(path.join(lib, 'two.jpg')).size, Math.round(fs.statSync(path.join(lib, 'two.jpg')).mtimeMs), null);
      ins(2, 'IMG_1.jpg', 1234, 1_650_000_000_000, null);
      old.close();
    } finally {
      MIGRATIONS.push(...later);
    }

    const srv = await createServer({ dataDir, roots: [{ path: lib, name: 'Lib' }], port: 0, host: '127.0.0.1', mdns: false });
    try {
      const db = srv.ctx.db;
      expect(fs.existsSync(`${dbFile}.bak-v2`)).toBe(true);
      expect(db.get<{ user_version: number }>('PRAGMA user_version')!.user_version).toBe(MIGRATIONS.length);
      const rows = db.all<any>('SELECT * FROM media ORDER BY id');
      expect(rows).toHaveLength(3);
      // added_at backfilled from mtime; derived columns start uncomputed
      for (const r of rows) {
        expect(r.added_at).toBe(r.mtime);
        expect(r.chash).toBeNull();
      }
      expect(rows[0].favorite).toBe(1);
      // the 1.2 schema works on the upgraded file: albums (FK cascade), library options, covers
      const a = (await srv.app.inject({ method: 'POST', url: '/api/albums', payload: { name: 'A', ids: [String(rows[0].id), String(rows[1].id)] } })).json();
      expect(a.count).toBe(2);
      const patched = await srv.app.inject({ method: 'PATCH', url: '/api/libraries/1', payload: { coverId: String(rows[1].id) } });
      expect(patched.statusCode, patched.body).toBe(200);
      expect((await srv.app.inject('/api/libraries/1/stats')).statusCode).toBe(200);
      expect((await srv.app.inject('/api/explore')).statusCode).toBe(200);
      expect((await srv.app.inject('/api/browse/skeleton?color=red')).statusCode).toBe(200);
      expect((await srv.app.inject('/api/browse?daytime=golden')).statusCode).toBe(200);
      // triggers: a changed thumbhash / location resets the derived columns
      db.run('UPDATE media SET color = 3, sun = 10 WHERE id = ?', rows[1].id);
      db.run("UPDATE media SET thumbhash = 'x', lat = 1 WHERE id = ?", rows[1].id);
      expect(db.get<any>('SELECT color, sun FROM media WHERE id = ?', rows[1].id)).toEqual({ color: null, sun: null });
      // new rows get added_at = now
      db.run("INSERT INTO media (folder_id, dir, name, ext, kind) VALUES (1, '', 'new.jpg', 'jpg', 'image')");
      expect(db.get<any>("SELECT added_at FROM media WHERE name = 'new.jpg'")!.added_at).toBeGreaterThan(Date.now() - 60_000);
      // deleting media cleans album items and clears the library cover
      db.run('DELETE FROM media WHERE id = ?', rows[1].id);
      expect(db.get<any>('SELECT count(*) c FROM album_items')!.c).toBe(1);
      expect(db.get<any>('SELECT cover_media_id c FROM libraries WHERE id = 1')!.c).toBeNull();
      // removing the phone keeps working (cascade through folders → media → album items)
      db.run("DELETE FROM devices WHERE id = 'ph1'");
      expect(db.get<any>('SELECT count(*) c FROM media WHERE folder_id = 2')!.c).toBe(0);
    } finally {
      await srv.close();
    }
    // a second start is a no-op (no new backup)
    const again = await createServer({ dataDir, roots: [{ path: lib, name: 'Lib' }], port: 0, host: '127.0.0.1', mdns: false });
    expect(fs.readdirSync(dataDir).filter((f) => /\.bak-v[1-9]/.test(f))).toEqual(['peekaround.db.bak-v2']);
    await again.close();
  });
});
