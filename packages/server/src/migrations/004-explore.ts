import type { Migration } from './index.js';

/**
 * Explore insights + maps (1.2):
 *  - color: dominant colour bucket of the thumbhash average (index into COLOR_BUCKETS, -1 = none,
 *    NULL = not computed yet). Filled lazily (insights.ts fillInsightColumns) and reset by a trigger
 *    whenever the thumbhash changes.
 *  - sun: sun altitude (degrees) at capture time and place; 999 = not applicable (no GPS / file date
 *    only), NULL = not computed yet. Reset by a trigger when time or location change.
 *  - chash: content hash for exact-duplicate detection, computed lazily only for same-size
 *    candidates (duplicates.ts); reset by a trigger when size / mtime change. 'x:<id>' = unreadable.
 *  - added_at: when the row joined the library (ms); existing rows start with their mtime.
 * Plain SQL triggers only (no app functions), so tools and old builds can still write the table.
 */
const COLUMNS: [string, string][] = [
  ['color', 'INTEGER'],
  ['sun', 'REAL'],
  ['chash', 'TEXT'],
  ['added_at', 'INTEGER'],
];

export const m004Explore: Migration = {
  name: '004-explore',
  up(db) {
    const have = new Set((db.prepare('PRAGMA table_info(media)').all() as { name: string }[]).map((c) => c.name));
    for (const [name, type] of COLUMNS) if (!have.has(name)) db.exec(`ALTER TABLE media ADD COLUMN ${name} ${type}`);
    db.exec(`
      UPDATE media SET added_at = mtime WHERE added_at IS NULL;
      -- duplicate candidates (same size): index-only scans
      CREATE INDEX IF NOT EXISTS media_size ON media (size, kind, folder_id, chash);
      CREATE INDEX IF NOT EXISTS media_chash ON media (chash) WHERE chash IS NOT NULL;
      CREATE INDEX IF NOT EXISTS media_color ON media (color);
      CREATE INDEX IF NOT EXISTS media_added ON media (added_at);
      -- rows whose derived columns still have to be computed (empty once filled)
      CREATE INDEX IF NOT EXISTS media_sun_todo ON media (id) WHERE sun IS NULL;
      CREATE INDEX IF NOT EXISTS media_color_todo ON media (id) WHERE color IS NULL AND thumbhash IS NOT NULL;

      CREATE TRIGGER IF NOT EXISTS media_added_at AFTER INSERT ON media WHEN NEW.added_at IS NULL
      BEGIN
        UPDATE media SET added_at = CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER) WHERE id = NEW.id;
      END;
      CREATE TRIGGER IF NOT EXISTS media_color_reset AFTER UPDATE OF thumbhash ON media
        WHEN NEW.thumbhash IS NOT OLD.thumbhash
      BEGIN
        UPDATE media SET color = NULL WHERE id = NEW.id;
      END;
      CREATE TRIGGER IF NOT EXISTS media_sun_reset AFTER UPDATE OF taken_at, time_src, lat, lon ON media
        WHEN NEW.taken_at IS NOT OLD.taken_at OR NEW.time_src IS NOT OLD.time_src OR NEW.lat IS NOT OLD.lat OR NEW.lon IS NOT OLD.lon
      BEGIN
        UPDATE media SET sun = NULL WHERE id = NEW.id;
      END;
      CREATE TRIGGER IF NOT EXISTS media_chash_reset AFTER UPDATE OF size, mtime ON media
        WHEN NEW.size IS NOT OLD.size OR NEW.mtime IS NOT OLD.mtime
      BEGIN
        UPDATE media SET chash = NULL WHERE id = NEW.id;
      END;
    `);
  },
};
