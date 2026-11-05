import type { DatabaseSync } from 'node:sqlite';
import { isValidTimeZone, systemTimeZone } from '@peekaround/shared';
import type { Migration } from './index.js';

/**
 * Metadata for automatic grouping (Explore):
 *  - lat/lon (null when unknown or (0,0)), geo_id (index into the GeoNames asset), cc, admin1 (code), city (label)
 *  - make/model (normalised, see normalizeCamera), lens
 *  - flags (MEDIA_FLAG bitmask), meta_v (enrichment version, see META_V in media/metadata.ts)
 *  - taken_local (wall clock encoded as UTC ms), tz_off (minutes east of UTC when known)
 *  - time_src: 0 = file mtime, 1 = embedded wall clock without offset, 2 = wall clock + offset, 3 = UTC instant (video)
 * Existing rows get taken_local from taken_at in the home zone so the UI works before enrichment.
 */
const COLUMNS: [string, string][] = [
  ['lat', 'REAL'],
  ['lon', 'REAL'],
  ['geo_id', 'INTEGER'],
  ['cc', 'TEXT'],
  ['admin1', 'TEXT'],
  ['city', 'TEXT'],
  ['make', 'TEXT'],
  ['model', 'TEXT'],
  ['lens', 'TEXT'],
  ['flags', 'INTEGER NOT NULL DEFAULT 0'],
  ['meta_v', 'INTEGER NOT NULL DEFAULT 0'],
  ['taken_local', 'INTEGER'],
  ['tz_off', 'INTEGER'],
  ['time_src', 'INTEGER NOT NULL DEFAULT 0'],
];

export function homeZoneFromSettings(db: DatabaseSync): string {
  try {
    const r = db.prepare("SELECT value FROM settings WHERE key = 'settings'").get() as { value: string } | undefined;
    const tz = r ? (JSON.parse(r.value) as { homeTimeZone?: string }).homeTimeZone : undefined;
    if (isValidTimeZone(tz)) return tz;
  } catch {
    /* fall through */
  }
  return systemTimeZone();
}

export const m001Groups: Migration = {
  name: '001-groups',
  up(db) {
    const have = new Set((db.prepare('PRAGMA table_info(media)').all() as { name: string }[]).map((c) => c.name));
    for (const [name, type] of COLUMNS) if (!have.has(name)) db.exec(`ALTER TABLE media ADD COLUMN ${name} ${type}`);
    db.exec(`
      -- ordered by capture time; also covers the skeleton query (index-only scan over 100k rows)
      CREATE INDEX IF NOT EXISTS media_local ON media (taken_local, id, kind, folder_id, width, height, city);
      CREATE INDEX IF NOT EXISTS media_place ON media (cc, admin1);
      CREATE INDEX IF NOT EXISTS media_model ON media (model);
      CREATE INDEX IF NOT EXISTS media_flags ON media (flags);
      CREATE INDEX IF NOT EXISTS media_metav ON media (meta_v);
    `);
    // Old taken_at values of EXIF photos were wall clocks read in the server zone; videos were UTC.
    db.prepare(
      `UPDATE media SET
         time_src = CASE WHEN taken_at != mtime AND meta_state = 1 THEN (CASE kind WHEN 'video' THEN 3 ELSE 1 END) ELSE 0 END,
         taken_local = peek_local(CASE WHEN taken_at > 0 THEN taken_at ELSE mtime END, ?)
       WHERE taken_local IS NULL`,
    ).run(homeZoneFromSettings(db));
  },
};
