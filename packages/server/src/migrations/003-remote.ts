import type { Migration } from './index.js';

/**
 * Remote (streaming) folders — MEGA links shown without downloading (see remote/folders.ts):
 *  - folders.remote: JSON {kind:'mega', url, linkId} — `url` carries the link KEY (a credential):
 *    it is never part of FolderRef; only local clients may read it (GET /api/remote/folders/:id/link).
 *    path = `remote://mega/<linkId>[-<subId>]` (stable pseudo path → thumbnail cache keys), library_id set.
 *  - folders.remote_status: JSON RemoteFolderStatus (listing / ok / offline / quota / error …), persisted so
 *    the last state survives restarts.
 *  - empty remote directories are kept in device_dirs (folder_id, dir) like phone folders.
 *  - media rows of remote folders use meta_state 3 = "listed, waiting for remote metadata enrichment";
 *    the local scanner never touches them.
 */
export const m003Remote: Migration = {
  name: '003-remote',
  up(db) {
    const have = new Set((db.prepare('PRAGMA table_info(folders)').all() as { name: string }[]).map((c) => c.name));
    if (!have.has('remote')) db.exec('ALTER TABLE folders ADD COLUMN remote TEXT');
    if (!have.has('remote_status')) db.exec('ALTER TABLE folders ADD COLUMN remote_status TEXT');
  },
};
