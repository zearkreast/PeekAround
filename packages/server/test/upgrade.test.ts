import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import sharp from 'sharp';
import { describe, expect, it } from 'vitest';
import { createServer } from '../src/app';
import type { ServerOptions } from '../src/config';
import { MIGRATIONS } from '../src/migrations/index';
import { OLD_SCHEMA_V10 } from './fixtures/old-schema';

/** A real 1.0 data dir (schema + rows + settings) started by 1.1 twice: migrate once, back up once, everything works. */
describe('upgrading a 1.0 data dir', () => {
  it('migrates, keeps data, serves browse / skeleton / explore / devices, and restarts cleanly', async () => {
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-up-'));
    const lib = path.join(tmp, 'lib');
    const dataDir = path.join(tmp, 'data');
    fs.mkdirSync(path.join(lib, 'a'), { recursive: true });
    fs.mkdirSync(dataDir, { recursive: true });
    await sharp({ create: { width: 40, height: 20, channels: 3, background: '#123' } }).jpeg().toFile(path.join(lib, 'one.jpg'));
    await sharp({ create: { width: 20, height: 40, channels: 3, background: '#456' } }).jpeg().toFile(path.join(lib, 'a', 'two.jpg'));
    fs.utimesSync(path.join(lib, 'one.jpg'), new Date(), new Date(1_600_000_000_000));
    fs.utimesSync(path.join(lib, 'a', 'two.jpg'), new Date(), new Date(1_650_000_000_000));

    const dbFile = path.join(dataDir, 'peekaround.db');
    const d = new DatabaseSync(dbFile);
    d.exec(OLD_SCHEMA_V10);
    d.exec("INSERT INTO libraries (id, name, color, icon, created_at) VALUES (1, 'Lib', '#fff', 'images', 0)");
    d.prepare("INSERT INTO folders (id, library_id, path, name) VALUES (1, 1, ?, 'lib')").run(lib);
    const ins = d.prepare(
      "INSERT INTO media (folder_id, dir, name, ext, kind, size, mtime, taken_at, meta_state, thumb_state, width, height, favorite) VALUES (1, ?, ?, 'jpg', 'image', ?, ?, ?, 1, 0, ?, ?, ?)",
    );
    ins.run('', 'one.jpg', fs.statSync(path.join(lib, 'one.jpg')).size, 1_600_000_000_000, 1_600_000_000_000, 40, 20, 1);
    ins.run('a', 'two.jpg', fs.statSync(path.join(lib, 'a', 'two.jpg')).size, 1_650_000_000_000, 1_650_000_000_000, 20, 40, 0);
    d.prepare("INSERT INTO settings (key, value) VALUES ('settings', ?)").run(JSON.stringify({ zipAction: 'ask', editSuffix: '_x' }));
    d.close();

    const opts: ServerOptions = { dataDir, roots: [{ path: lib, name: 'Lib' }], port: 0, host: '127.0.0.1', mdns: false };
    const srv = await createServer(opts);
    try {
      expect(fs.existsSync(`${dbFile}.bak-v0`)).toBe(true);
      expect(srv.ctx.db.get<{ user_version: number }>('PRAGMA user_version')!.user_version).toBe(MIGRATIONS.length);
      // settings kept, home zone added
      const s = (await srv.app.inject('/api/settings')).json();
      expect(s).toMatchObject({ zipAction: 'ask', editSuffix: '_x' });
      expect(typeof s.homeTimeZone).toBe('string');
      const libs = (await srv.app.inject('/api/libraries')).json();
      expect(libs.libraries).toHaveLength(1);
      expect(libs.devices).toEqual([]);
      const b = (await srv.app.inject('/api/browse?folderId=1&recursive=1')).json();
      expect(b.items.map((i: { name: string }) => i.name)).toEqual(['two.jpg', 'one.jpg']);
      expect(b.items.find((i: { name: string }) => i.name === 'one.jpg').favorite).toBe(true);
      const sk = (await srv.app.inject('/api/browse/skeleton?folderId=1&recursive=1')).json();
      expect(sk.total).toBe(2);
      expect((await srv.app.inject('/api/explore')).statusCode).toBe(200);
      expect((await srv.app.inject('/api/devices')).json()).toEqual([]);
      expect((await srv.app.inject('/api/search?q=two')).json().total).toBe(1);
    } finally {
      await srv.close();
    }

    // second start on the same data dir: no new backup, rows intact (ids stable → favorites/links survive)
    const before = fs.readdirSync(dataDir).filter((n) => n.includes('.bak-'));
    const again = await createServer(opts);
    try {
      expect(fs.readdirSync(dataDir).filter((n) => n.includes('.bak-'))).toEqual(before);
      const b = (await again.app.inject('/api/browse?folderId=1&recursive=1')).json();
      expect(b.items.map((i: { id: string; name: string }) => [i.id, i.name])).toEqual([
        ['2', 'two.jpg'],
        ['1', 'one.jpg'],
      ]);
    } finally {
      await again.close();
    }
  });
});
