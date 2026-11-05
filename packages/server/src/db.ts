import { DatabaseSync, type StatementSync } from 'node:sqlite';
import fs from 'node:fs';
import path from 'node:path';

export type Row = Record<string, any>;

const SCHEMA = `
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS libraries (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  color TEXT NOT NULL,
  icon TEXT NOT NULL,
  sort INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS folders (
  id INTEGER PRIMARY KEY,
  library_id INTEGER REFERENCES libraries(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  name TEXT NOT NULL,
  temp INTEGER NOT NULL DEFAULT 0,
  expires_at INTEGER,
  source_name TEXT,
  sort INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS media (
  id INTEGER PRIMARY KEY,
  folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
  dir TEXT NOT NULL,
  name TEXT NOT NULL,
  ext TEXT NOT NULL,
  kind TEXT NOT NULL,
  size INTEGER NOT NULL DEFAULT 0,
  mtime INTEGER NOT NULL DEFAULT 0,
  width INTEGER,
  height INTEGER,
  duration REAL,
  taken_at INTEGER NOT NULL DEFAULT 0,
  thumbhash TEXT,
  favorite INTEGER NOT NULL DEFAULT 0,
  playback TEXT NOT NULL DEFAULT 'direct',
  meta_state INTEGER NOT NULL DEFAULT 0,
  thumb_state INTEGER NOT NULL DEFAULT 0,
  extra TEXT,
  seen INTEGER NOT NULL DEFAULT 0,
  UNIQUE (folder_id, dir, name)
);
CREATE INDEX IF NOT EXISTS media_dir ON media (folder_id, dir);
CREATE INDEX IF NOT EXISTS media_taken ON media (taken_at);
CREATE INDEX IF NOT EXISTS media_meta ON media (meta_state);
CREATE INDEX IF NOT EXISTS media_thumb ON media (thumb_state);

CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS fonts (
  id INTEGER PRIMARY KEY,
  family TEXT NOT NULL,
  subfamily TEXT NOT NULL,
  weight INTEGER NOT NULL,
  style TEXT NOT NULL,
  format TEXT NOT NULL,
  file TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
`;

export class Db {
  readonly raw: DatabaseSync;
  private cache = new Map<string, StatementSync>();

  constructor(file: string) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    this.raw = new DatabaseSync(file);
    this.raw.exec(SCHEMA);
  }

  stmt(sql: string): StatementSync {
    let s = this.cache.get(sql);
    if (!s) {
      s = this.raw.prepare(sql);
      this.cache.set(sql, s);
    }
    return s;
  }

  all<T = Row>(sql: string, ...params: any[]): T[] {
    return this.stmt(sql).all(...params) as T[];
  }

  get<T = Row>(sql: string, ...params: any[]): T | undefined {
    return this.stmt(sql).get(...params) as T | undefined;
  }

  run(sql: string, ...params: any[]) {
    return this.stmt(sql).run(...params);
  }

  tx<T>(fn: () => T): T {
    this.raw.exec('BEGIN');
    try {
      const r = fn();
      this.raw.exec('COMMIT');
      return r;
    } catch (e) {
      this.raw.exec('ROLLBACK');
      throw e;
    }
  }

  getSetting<T>(key: string, fallback: T): T {
    const r = this.get<{ value: string }>('SELECT value FROM settings WHERE key = ?', key);
    if (!r) return fallback;
    try {
      return JSON.parse(r.value) as T;
    } catch {
      return fallback;
    }
  }

  setSetting(key: string, value: unknown) {
    this.run(
      'INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value',
      key,
      JSON.stringify(value),
    );
  }

  close() {
    this.raw.close();
  }
}
