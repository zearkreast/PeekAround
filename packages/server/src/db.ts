import { DatabaseSync, type StatementSync } from 'node:sqlite';
import fs from 'node:fs';
import path from 'node:path';
import { foldText, instantToLocal, isValidTimeZone, localToInstant, systemTimeZone } from '@peekaround/shared';
import { MIGRATIONS } from './migrations/index.js';

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
  private arrayCache = new Map<string, StatementSync>();
  private readerConn: DatabaseSync | null | undefined;
  private readerCache = new Map<string, StatementSync>();

  constructor(private file: string) {
    fs.mkdirSync(path.dirname(file), { recursive: true });
    this.raw = new DatabaseSync(file);
    this.raw.exec(SCHEMA);
    registerFunctions(this.raw);
    this.migrate(file);
  }

  /** Apply pending migrations (PRAGMA user_version), backing up the DB file first. */
  private migrate(file: string) {
    const current = Number((this.raw.prepare('PRAGMA user_version').get() as { user_version: number }).user_version) || 0;
    if (current >= MIGRATIONS.length) return;
    if (file !== ':memory:' && fs.existsSync(file) && fs.statSync(file).size > 0) {
      try {
        this.raw.exec('PRAGMA wal_checkpoint(TRUNCATE)');
        fs.copyFileSync(file, `${file}.bak-v${current}`);
      } catch {
        /* backup is best effort */
      }
    }
    for (let v = current; v < MIGRATIONS.length; v++) {
      this.raw.exec('BEGIN');
      try {
        MIGRATIONS[v].up(this.raw);
        this.raw.exec(`PRAGMA user_version = ${v + 1}`);
        this.raw.exec('COMMIT');
      } catch (e) {
        this.raw.exec('ROLLBACK');
        throw new Error(`Migration ${MIGRATIONS[v].name} failed: ${(e as Error).message}`);
      }
    }
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

  /** Rows as arrays (column order of the SELECT) — ~2× faster than objects for large result sets. */
  allArrays(sql: string, ...params: any[]): unknown[][] {
    let s = this.arrayCache.get(sql);
    if (!s) {
      s = this.raw.prepare(sql);
      if (typeof s.setReturnArrays === 'function') s.setReturnArrays(true);
      this.arrayCache.set(sql, s);
    }
    const rows = s.all(...params) as unknown[];
    return rows.length && !Array.isArray(rows[0]) ? rows.map((r) => Object.values(r as object)) : (rows as unknown[][]);
  }

  /**
   * A second, read-only connection for long reads that are consumed in chunks (yielding to the event
   * loop in between): in WAL mode it reads a consistent snapshot and never blocks the writers.
   * null for in-memory databases or when it cannot be opened.
   */
  private reader(): DatabaseSync | null {
    if (this.readerConn !== undefined) return this.readerConn;
    try {
      if (this.file === ':memory:') throw new Error('in-memory');
      const r = new DatabaseSync(this.file, { readOnly: true });
      registerFunctions(r);
      this.readerConn = r;
    } catch {
      this.readerConn = null;
    }
    return this.readerConn;
  }

  /**
   * Rows as arrays, one at a time (reader connection when possible). Callers that take a break
   * between rows must not hold the iterator across a close().
   */
  *iterateArrays(sql: string, ...params: any[]): Generator<unknown[]> {
    const conn = this.reader();
    let s: StatementSync | undefined = conn ? this.readerCache.get(sql) : undefined;
    if (conn && !s) {
      try {
        s = conn.prepare(sql);
        if (typeof s.setReturnArrays === 'function') s.setReturnArrays(true);
        this.readerCache.set(sql, s);
      } catch {
        s = undefined; // e.g. a function only the main connection has
      }
    }
    if (!s) {
      yield* this.allArrays(sql, ...params);
      return;
    }
    for (const r of s.iterate(...params) as Iterable<unknown>) yield Array.isArray(r) ? r : Object.values(r as object);
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

  /** IANA zone used for UTC-only timestamps (stored settings → server zone). */
  homeTimeZone(): string {
    const tz = this.getSetting<{ homeTimeZone?: string }>('settings', {}).homeTimeZone;
    return isValidTimeZone(tz) ? tz : systemTimeZone();
  }

  close() {
    this.readerCache.clear();
    try {
      this.readerConn?.close();
    } catch {
      /* already closed */
    }
    this.readerConn = null;
    this.raw.close();
  }
}

/** SQL helpers usable from queries and migrations. */
function registerFunctions(db: DatabaseSync) {
  const det = { deterministic: true, useBigIntArguments: false };
  db.function('peek_fold', det, (s: unknown) => (s == null ? '' : foldText(String(s))));
  db.function('peek_local', det, (instant: unknown, tz: unknown) =>
    typeof instant === 'number' ? instantToLocal(instant, String(tz || 'UTC')) : null,
  );
  db.function('peek_instant', det, (local: unknown, tz: unknown) =>
    typeof local === 'number' ? localToInstant(local, String(tz || 'UTC')) : null,
  );
}
