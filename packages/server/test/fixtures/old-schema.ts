// Base schema of PeekAround 1.0 (git 68f19e0 packages/server/src/db.ts) — used to test migrations on old databases.
export const OLD_SCHEMA_V10 = `
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
