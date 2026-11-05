import fsp from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { kindOf, extOf, joinRel } from '@peekaround/shared';
import type { Db } from '../db.js';
import type { EventHub } from '../events.js';
import type { Ffmpeg } from '../ffmpeg.js';
import type { FolderRow, LibraryService } from '../library.js';
import { isHiddenName, toPosix } from '../paths.js';
import type { MediaRow } from './mapper.js';
import { extractMeta } from './metadata.js';
import type { ThumbService } from './thumbs.js';

export const TRASH_DIR = '.peekaround-trash';

const UPSERT = `
INSERT INTO media (folder_id, dir, name, ext, kind, size, mtime, taken_at, seen)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1)
ON CONFLICT (folder_id, dir, name) DO UPDATE SET
  seen = 1,
  meta_state = CASE WHEN media.size != excluded.size OR media.mtime != excluded.mtime THEN 0 ELSE media.meta_state END,
  thumb_state = CASE WHEN media.size != excluded.size OR media.mtime != excluded.mtime THEN 0 ELSE media.thumb_state END,
  taken_at = CASE WHEN media.size != excluded.size OR media.mtime != excluded.mtime THEN excluded.taken_at ELSE media.taken_at END,
  size = excluded.size,
  mtime = excluded.mtime
`;

export class Scanner {
  private scanning = new Set<number>();
  private metaRunning = false;
  private thumbRunning = false;
  private progress = { done: 0, total: 0 };
  private lastProgressEmit = 0;
  private stopped = false;
  private loops: Promise<void>[] = [];
  showHidden = false;

  constructor(
    private db: Db,
    private lib: LibraryService,
    private ffmpeg: Ffmpeg,
    private thumbs: ThumbService,
    readonly events: EventHub,
  ) {}

  get busy(): boolean {
    return this.scanning.size > 0 || this.metaRunning || this.thumbRunning;
  }

  abs(folder: FolderRow, dir: string, name: string): string {
    return path.join(folder.path, ...dir.split('/').filter(Boolean), name);
  }

  skipName(name: string): boolean {
    if (name === TRASH_DIR) return true;
    return !this.showHidden && isHiddenName(name);
  }

  async scanAll() {
    for (const f of this.lib.allFolderRows()) await this.scanFolder(f.id);
  }

  /**
   * (Re)scan one folder — or only `subDir` of it — and drop rows of files that disappeared.
   */
  async scanFolder(folderId: number, subDir = '') {
    if (this.stopped) return;
    const lockKey = subDir ? -Math.random() : folderId;
    if (!subDir && this.scanning.has(lockKey)) return;
    this.scanning.add(lockKey);
    this.emitScan(true);
    try {
      const folder = this.lib.folderRow(folderId);
      const sub = subDir.replace(/^\/+|\/+$/g, '');
      const scope = sub ? ' AND (dir = ? OR substr(dir, 1, ?) = ?)' : '';
      const scopeParams = sub ? [sub, sub.length + 1, `${sub}/`] : [];
      this.db.run(`UPDATE media SET seen = 0 WHERE folder_id = ?${scope}`, folderId, ...scopeParams);
      let batch: any[][] = [];
      const flush = () => {
        if (!batch.length || this.stopped) return;
        const rows = batch;
        batch = [];
        this.db.tx(() => {
          for (const r of rows) this.db.run(UPSERT, ...r);
        });
      };
      const walk = async (dirAbs: string, rel: string) => {
        let entries;
        try {
          entries = await fsp.readdir(dirAbs, { withFileTypes: true });
        } catch {
          return;
        }
        for (const e of entries) {
          if (this.skipName(e.name)) continue;
          const p = path.join(dirAbs, e.name);
          if (e.isDirectory()) {
            await walk(p, joinRel(rel, e.name));
            continue;
          }
          const kind = kindOf(e.name);
          if (!kind) continue;
          let st;
          try {
            st = await fsp.stat(p);
          } catch {
            continue;
          }
          if (!st.isFile()) continue;
          batch.push([folderId, rel, e.name, extOf(e.name), kind, st.size, Math.round(st.mtimeMs), Math.round(st.mtimeMs)]);
          if (batch.length >= 400) flush();
        }
      };
      await walk(sub ? path.join(folder.path, ...sub.split('/')) : folder.path, sub);
      if (this.stopped) return;
      flush();
      this.db.run(`DELETE FROM media WHERE folder_id = ? AND seen = 0${scope}`, folderId, ...scopeParams);
      this.events.emit({ type: 'media', folderId: String(folderId), dirs: ['*'] });
      this.events.emit({ type: 'folders' });
    } finally {
      this.scanning.delete(lockKey);
      this.kick();
    }
  }

  /** Incremental update for a single file (watcher, uploads, edits). */
  async upsertFile(folderId: number, relPath: string): Promise<number | null> {
    const folder = this.lib.folderRow(folderId);
    const rel = toPosix(relPath).replace(/^\/+/, '');
    const name = path.posix.basename(rel);
    const dir = path.posix.dirname(rel) === '.' ? '' : path.posix.dirname(rel);
    if (rel.split('/').some((s) => this.skipName(s))) return null;
    const kind = kindOf(name);
    if (!kind) return null;
    let st;
    try {
      st = await fsp.stat(this.abs(folder, dir, name));
    } catch {
      this.removeFile(folderId, rel);
      return null;
    }
    if (!st.isFile()) return null;
    this.db.run(UPSERT, folderId, dir, name, extOf(name), kind, st.size, Math.round(st.mtimeMs), Math.round(st.mtimeMs));
    this.events.mediaChanged(String(folderId), dir);
    this.kick();
    return this.db.get<{ id: number }>('SELECT id FROM media WHERE folder_id = ? AND dir = ? AND name = ?', folderId, dir, name)?.id ?? null;
  }

  removeFile(folderId: number, relPath: string) {
    const rel = toPosix(relPath).replace(/^\/+/, '');
    const name = path.posix.basename(rel);
    const dir = path.posix.dirname(rel) === '.' ? '' : path.posix.dirname(rel);
    this.db.run('DELETE FROM media WHERE folder_id = ? AND dir = ? AND name = ?', folderId, dir, name);
    this.events.mediaChanged(String(folderId), dir);
  }

  removeDir(folderId: number, relDir: string) {
    const dir = toPosix(relDir).replace(/^\/+|\/+$/g, '');
    this.db.run('DELETE FROM media WHERE folder_id = ? AND (dir = ? OR substr(dir, 1, ?) = ?)', folderId, dir, dir.length + 1, `${dir}/`);
    this.events.mediaChanged(String(folderId), path.posix.dirname(dir) === '.' ? '' : path.posix.dirname(dir));
    this.events.mediaChanged(String(folderId), dir);
  }

  /** Start background metadata + thumbnail workers if idle. */
  kick() {
    if (this.stopped) return;
    if (!this.metaRunning) this.track(this.metaLoop());
    if (!this.thumbRunning) this.track(this.thumbLoop());
  }

  private track(p: Promise<void>) {
    const t = p.catch(() => {}).finally(() => {
      this.loops = this.loops.filter((x) => x !== t);
    });
    this.loops.push(t);
  }

  /** Stop background work (server shutdown) and wait for in-flight items. */
  async stop() {
    this.stopped = true;
    await Promise.allSettled(this.loops);
  }

  private emitScan(force = false) {
    const now = Date.now();
    if (!force && now - this.lastProgressEmit < 700) return;
    this.lastProgressEmit = now;
    this.events.emit({ type: 'scan', scanning: this.busy, done: this.progress.done, total: this.progress.total });
  }

  private pendingCount(): number {
    return (
      (this.db.get<{ c: number }>('SELECT count(*) c FROM media WHERE meta_state = 0')?.c ?? 0) +
      (this.db.get<{ c: number }>("SELECT count(*) c FROM media WHERE thumb_state = 0 AND kind IN ('image','video')")?.c ?? 0)
    );
  }

  private async metaLoop() {
    this.metaRunning = true;
    this.progress = { done: 0, total: this.pendingCount() };
    try {
      const conc = Math.max(2, Math.min(6, os.cpus().length));
      for (;;) {
        if (this.stopped) break;
        const rows = this.db.all<MediaRow>('SELECT * FROM media WHERE meta_state = 0 LIMIT 64');
        if (!rows.length) break;
        await pool(rows, conc, async (r) => {
          if (this.stopped) return;
          const folder = safe(() => this.lib.folderRow(r.folder_id));
          if (!folder) return;
          const abs = this.abs(folder, r.dir, r.name);
          try {
            const m = await extractMeta(abs, r, this.ffmpeg);
            if (this.stopped) return;
            this.db.run(
              'UPDATE media SET width = ?, height = ?, duration = ?, taken_at = ?, playback = ?, extra = ?, meta_state = 1 WHERE id = ?',
              m.width,
              m.height,
              m.duration,
              m.takenAt ?? r.mtime,
              m.playback,
              m.extra ? JSON.stringify(m.extra) : null,
              r.id,
            );
          } catch {
            if (!this.stopped) this.db.run('UPDATE media SET meta_state = 2 WHERE id = ?', r.id);
          }
          this.events.mediaChanged(String(r.folder_id), r.dir);
          this.progress.done++;
          this.emitScan();
        });
      }
    } finally {
      this.metaRunning = false;
      this.kickThumbs();
      this.emitScan(true);
    }
  }

  private kickThumbs() {
    if (!this.thumbRunning && !this.stopped) this.track(this.thumbLoop());
  }

  private async thumbLoop() {
    this.thumbRunning = true;
    try {
      const conc = Math.max(1, Math.min(4, os.cpus().length - 1));
      for (;;) {
        if (this.stopped) break;
        // only thumbnail rows whose metadata is known (so videos have duration)
        const rows = this.db.all<MediaRow>(
          "SELECT * FROM media WHERE thumb_state = 0 AND meta_state != 0 AND kind IN ('image','video') ORDER BY taken_at DESC LIMIT 32",
        );
        if (!rows.length) break;
        await pool(rows, conc, async (r) => {
          if (this.stopped) return;
          const folder = safe(() => this.lib.folderRow(r.folder_id));
          if (!folder) return;
          const abs = this.abs(folder, r.dir, r.name);
          try {
            const res = await this.thumbs.generateBase(abs, r);
            if (this.stopped) return;
            if (res) {
              const needDims = !r.width || !r.height;
              this.db.run(
                'UPDATE media SET thumbhash = ?, thumb_state = 1, width = COALESCE(width, ?), height = COALESCE(height, ?) WHERE id = ?',
                res.thumbhash,
                needDims && r.kind === 'image' ? res.width : null,
                needDims && r.kind === 'image' ? res.height : null,
                r.id,
              );
            } else {
              this.db.run('UPDATE media SET thumb_state = 2 WHERE id = ?', r.id);
            }
          } catch {
            if (!this.stopped) this.db.run('UPDATE media SET thumb_state = 2 WHERE id = ?', r.id);
          }
          this.events.mediaChanged(String(r.folder_id), r.dir);
          this.progress.done++;
          this.emitScan();
        });
      }
    } finally {
      this.thumbRunning = false;
      this.emitScan(true);
    }
  }
}

function safe<T>(fn: () => T): T | null {
  try {
    return fn();
  } catch {
    return null;
  }
}

export async function pool<T>(items: T[], n: number, fn: (t: T) => Promise<void>) {
  let i = 0;
  const workers = Array.from({ length: Math.min(n, items.length) }, async () => {
    while (i < items.length) {
      const it = items[i++];
      await fn(it);
    }
  });
  await Promise.all(workers);
}
