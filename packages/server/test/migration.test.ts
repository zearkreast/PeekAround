import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { describe, expect, it } from 'vitest';
import { instantToLocal, systemTimeZone } from '@peekaround/shared';
import { Db } from '../src/db';
import { MIGRATIONS } from '../src/migrations/index';
import { OLD_SCHEMA_V10 } from './fixtures/old-schema';

function oldDb(file: string, settings?: object) {
  const d = new DatabaseSync(file);
  d.exec(OLD_SCHEMA_V10);
  d.exec("INSERT INTO libraries (id, name, color, icon, created_at) VALUES (1, 'Lib', '#fff', 'images', 0)");
  d.exec("INSERT INTO folders (id, library_id, path, name) VALUES (1, 1, '/x', 'x')");
  const ins = d.prepare(
    'INSERT INTO media (folder_id, dir, name, ext, kind, size, mtime, taken_at, meta_state, thumb_state, width, height) VALUES (1, ?, ?, ?, ?, 1, ?, ?, ?, 1, 100, 50)',
  );
  ins.run('', 'exif.jpg', 'jpg', 'image', 1_700_000_000_000, 1_690_000_000_000, 1);
  ins.run('', 'clip.mp4', 'mp4', 'video', 1_700_000_000_000, 1_680_000_000_000, 1);
  ins.run('a', 'plain.png', 'png', 'image', 1_650_000_000_000, 1_650_000_000_000, 1);
  ins.run('a', 'pending.jpg', 'jpg', 'image', 1_660_000_000_000, 1_660_000_000_000, 0);
  if (settings) d.prepare("INSERT INTO settings (key, value) VALUES ('settings', ?)").run(JSON.stringify(settings));
  d.close();
}

describe('migrations on a 1.0 database', () => {
  it('adds the grouping columns, indexes and backfills taken_local', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-mig-'));
    const file = path.join(dir, 'peekaround.db');
    oldDb(file, { zipAction: 'ask', homeTimeZone: 'Europe/Istanbul' });

    const db = new Db(file);
    expect((db.get<{ user_version: number }>('PRAGMA user_version')!).user_version).toBe(MIGRATIONS.length);
    const cols = db.all<{ name: string }>('PRAGMA table_info(media)').map((c) => c.name);
    for (const c of ['lat', 'lon', 'geo_id', 'cc', 'admin1', 'city', 'make', 'model', 'lens', 'flags', 'meta_v', 'taken_local', 'tz_off', 'time_src']) {
      expect(cols).toContain(c);
    }
    const idx = db.all<{ name: string }>("SELECT name FROM sqlite_master WHERE type = 'index' AND tbl_name = 'media'").map((r) => r.name);
    expect(idx).toEqual(expect.arrayContaining(['media_local', 'media_place', 'media_model', 'media_flags', 'media_metav']));

    const rows = db.all<{ name: string; taken_at: number; taken_local: number; time_src: number; flags: number; meta_v: number }>(
      'SELECT name, taken_at, taken_local, time_src, flags, meta_v FROM media ORDER BY id',
    );
    for (const r of rows) {
      expect(r.taken_local).toBe(instantToLocal(r.taken_at, 'Europe/Istanbul'));
      expect(r.flags).toBe(0);
      expect(r.meta_v).toBe(0); // → enrichment loop picks them up
    }
    expect(rows.map((r) => r.time_src)).toEqual([1, 3, 0, 0]);
    // existing data untouched
    expect(db.get<{ c: number }>('SELECT count(*) c FROM media')!.c).toBe(4);
    db.close();
    expect(fs.existsSync(`${file}.bak-v0`)).toBe(true);

    // re-opening is a no-op
    const again = new Db(file);
    expect(again.get<{ c: number }>('SELECT count(*) c FROM media WHERE taken_local IS NULL')!.c).toBe(0);
    again.close();
  });

  it('falls back to the server time zone without stored settings', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-mig-'));
    const file = path.join(dir, 'peekaround.db');
    oldDb(file);
    const db = new Db(file);
    const r = db.get<{ taken_at: number; taken_local: number }>("SELECT taken_at, taken_local FROM media WHERE name = 'exif.jpg'")!;
    expect(r.taken_local).toBe(instantToLocal(r.taken_at, systemTimeZone()));
    expect(db.homeTimeZone()).toBe(systemTimeZone());
    db.close();
  });

  it('creates the new schema from scratch', () => {
    const db = new Db(path.join(fs.mkdtempSync(path.join(os.tmpdir(), 'peek-mig-')), 'new.db'));
    const cols = db.all<{ name: string }>('PRAGMA table_info(media)').map((c) => c.name);
    expect(cols).toContain('taken_local');
    expect(db.get<{ v: string }>("SELECT peek_fold('İSTANBUL Kadıköy') v")!.v).toBe('istanbul kadikoy');
    db.close();
  });
});
