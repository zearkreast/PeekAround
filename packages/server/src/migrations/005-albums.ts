import type { Migration } from './index.js';

/**
 * Albums + library options (1.2):
 *  - albums: user-curated collections. cover_media_id = picked cover (NULL = newest item);
 *    sort = position in the album list; item_order = 'desc' | 'asc' | 'custom'.
 *  - album_items: membership. Rows follow the media row: renames / moves inside PeekAround keep
 *    the media id (scanner.relocate*), deleting the media row (file deleted, folder removed,
 *    file vanished on rescan) removes it from every album (ON DELETE CASCADE).
 *    position = custom order (album_items.position, ascending).
 *  - libraries.cover_media_id: picked library cover; libraries.options: JSON LibraryOptions
 *    (exclusions, minimum size, hidden folders) honoured by the scanner.
 *  - folders.scanned_at: last completed scan (ms) — library header "last scanned".
 */
export const m005Albums: Migration = {
  name: '005-albums',
  up(db) {
    db.exec(`
      CREATE TABLE IF NOT EXISTS albums (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL,
        cover_media_id INTEGER REFERENCES media(id) ON DELETE SET NULL,
        item_order TEXT NOT NULL DEFAULT 'desc',
        created INTEGER NOT NULL,
        updated INTEGER NOT NULL,
        sort INTEGER NOT NULL DEFAULT 0
      );
      CREATE TABLE IF NOT EXISTS album_items (
        album_id INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
        media_id INTEGER NOT NULL REFERENCES media(id) ON DELETE CASCADE,
        added INTEGER NOT NULL,
        position REAL NOT NULL DEFAULT 0,
        PRIMARY KEY (album_id, media_id)
      ) WITHOUT ROWID;
      CREATE INDEX IF NOT EXISTS album_items_media ON album_items (media_id);
      CREATE INDEX IF NOT EXISTS album_items_pos ON album_items (album_id, position);
    `);
    const libCols = new Set((db.prepare('PRAGMA table_info(libraries)').all() as { name: string }[]).map((c) => c.name));
    if (!libCols.has('cover_media_id')) db.exec('ALTER TABLE libraries ADD COLUMN cover_media_id INTEGER REFERENCES media(id) ON DELETE SET NULL');
    if (!libCols.has('options')) db.exec('ALTER TABLE libraries ADD COLUMN options TEXT');
    const folderCols = new Set((db.prepare('PRAGMA table_info(folders)').all() as { name: string }[]).map((c) => c.name));
    if (!folderCols.has('scanned_at')) db.exec('ALTER TABLE folders ADD COLUMN scanned_at INTEGER');
  },
};
