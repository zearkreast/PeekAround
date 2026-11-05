import type { Migration } from './index.js';

/**
 * Linked phones (see docs/link-protocol.md):
 *  - devices: one row per PAIRED phone (pending pairings live in memory only).
 *    token_hash = sha256(device token) as hex; the token itself is never stored.
 *  - folders.device_id / folders.device_key: a folder shared by a phone. path = `device://<deviceId>/<key>`,
 *    library_id NULL, temp 0. Forgetting a device cascades to its folders and their media rows.
 *  - device_dirs: every directory the phone reported in its last `list` (including empty ones),
 *    used for sub folder listings because device folders cannot be read with readdir.
 */
export const m002Devices: Migration = {
  name: '002-devices',
  up(db) {
    db.exec(`
      CREATE TABLE IF NOT EXISTS devices (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        model TEXT NOT NULL DEFAULT '',
        token_hash TEXT,
        paired INTEGER NOT NULL DEFAULT 0,
        caps TEXT,
        created_at INTEGER NOT NULL,
        last_seen INTEGER NOT NULL DEFAULT 0
      );
    `);
    const have = new Set((db.prepare('PRAGMA table_info(folders)').all() as { name: string }[]).map((c) => c.name));
    if (!have.has('device_id')) db.exec('ALTER TABLE folders ADD COLUMN device_id TEXT REFERENCES devices(id) ON DELETE CASCADE');
    if (!have.has('device_key')) db.exec('ALTER TABLE folders ADD COLUMN device_key TEXT');
    db.exec(`
      CREATE INDEX IF NOT EXISTS folders_device ON folders (device_id, device_key);
      CREATE TABLE IF NOT EXISTS device_dirs (
        folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
        dir TEXT NOT NULL,
        PRIMARY KEY (folder_id, dir)
      );
    `);
  },
};
