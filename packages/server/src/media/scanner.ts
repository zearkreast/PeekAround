import fsp from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { kindOf, extOf, joinRel, instantToLocal, systemTimeZone } from '@peekaround/shared';
import type { Db } from '../db.js';
import type { EventHub } from '../events.js';
import type { Ffmpeg } from '../ffmpeg.js';
import type { GeoService } from '../geo.js';
import { DEVICE_SCHEME, type FolderRow, type LibraryService } from '../library.js';
import { isHiddenName, toPosix, trimChars } from '../paths.js';
import type { MediaRow } from './mapper.js';
import { extractMeta, GROUP_SET_SQL, groupColumns, groupParams, META_V, TIME_SRC } from './metadata.js';
import type { ThumbService } from './thumbs.js';

export const TRASH_DIR = '.peekaround-trash';

// a changed file (size/mtime) is re-read from scratch by the meta pass
const CHANGED = 'media.size != excluded.size OR media.mtime != excluded.mtime';
const UPSERT = `
INSERT INTO media (folder_id, dir, name, ext, kind, size, mtime, taken_at, taken_local, time_src, seen)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ${TIME_SRC.mtime}, 1)
ON CONFLICT (folder_id, dir, name) DO UPDATE SET
  seen = 1,
  meta_state = CASE WHEN ${CHANGED} THEN 0 ELSE media.meta_state END,
  thumb_state = CASE WHEN ${CHANGED} THEN 0 ELSE media.thumb_state END,
  meta_v = CASE WHEN ${CHANGED} THEN 0 ELSE media.meta_v END,
  taken_at = CASE WHEN ${CHANGED} THEN excluded.taken_at ELSE media.taken_at END,
  taken_local = CASE WHEN ${CHANGED} THEN excluded.taken_local ELSE media.taken_local END,
  time_src = CASE WHEN ${CHANGED} THEN excluded.time_src ELSE media.time_src END,
  size = excluded.size,
  mtime = excluded.mtime
`;

/** how often the enrichment loop publishes its progress (explore + layout events) */
const ENRICH_FLUSH_MS = 30_000;

export class Scanner {
  private scanning = new Set<number>();
  private metaRunning = false;
  private thumbRunning = false;
  private enrichRunning = false;
  /** zone for UTC-only timestamps (videos, mtime) — see Settings.homeTimeZone */
  homeTimeZone = systemTimeZone();
  /** called when grouping data (places, cameras, flags, dates) changed in bulk */
  onGroupsChanged: (() => void) | null = null;
  private progress = { done: 0, total: 0 };
  private lastProgressEmit = 0;
  private stopped = false;
  private loops: Promise<void>[] = [];
  showHidden = false;
  /** DeviceHub: (re)list a device folder instead of walking the file system */
  deviceScan: ((folderId: number, subDir: string) => Promise<void>) | null = null;

  constructor(
    private db: Db,
    private lib: LibraryService,
    private ffmpeg: Ffmpeg,
    private thumbs: ThumbService,
    readonly events: EventHub,
    private geo: GeoService,
  ) {}

  get busy(): boolean {
    return this.scanning.size > 0 || this.metaRunning || this.thumbRunning;
  }

  /** true while the low-priority metadata enrichment runs */
  get enriching(): boolean {
    return this.enrichRunning;
  }

  private localOf(instant: number): number {
    return instantToLocal(instant, this.homeTimeZone);
  }

  /**
   * Home zone changed: re-derive the clock that was inferred from it
   * (wall clock for UTC-only sources, instant for wall-clock-only EXIF).
   */
  setHomeTimeZone(tz: string) {
    if (tz === this.homeTimeZone) return;
    this.homeTimeZone = tz;
    this.db.tx(() => {
      this.db.run(
        `UPDATE media SET taken_local = peek_local(taken_at, ?) WHERE time_src IN (${TIME_SRC.mtime}, ${TIME_SRC.instant})`,
        tz,
      );
      this.db.run(`UPDATE media SET taken_at = peek_instant(taken_local, ?) WHERE time_src = ${TIME_SRC.local}`, tz);
    });
    for (const f of this.lib.allFolderRows()) this.events.mediaChanged(String(f.id), '*', 'meta');
    this.onGroupsChanged?.();
  }

  abs(folder: FolderRow, dir: string, name: string): string {
    // device folders: stable pseudo path device://<id>/<key>/<rel> (thumbnail cache key, hub requests)
    if (folder.path.startsWith(DEVICE_SCHEME)) return `${folder.path}/${joinRel(dir, name)}`;
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
    const row = this.db.get<{ device_id: string | null }>('SELECT device_id FROM folders WHERE id = ?', folderId);
    if (row?.device_id) {
      // never walk (or wipe) a phone folder from disk; the hub lists it when the phone is online
      await this.deviceScan?.(folderId, subDir).catch(() => {});
      return;
    }
    const lockKey = subDir ? -Math.random() : folderId;
    if (!subDir && this.scanning.has(lockKey)) return;
    this.scanning.add(lockKey);
    this.emitScan(true);
    try {
      const folder = this.lib.folderRow(folderId);
      const sub = trimChars(subDir, '/');
      const scope = sub ? ' AND (dir = ? OR substr(dir, 1, ?) = ?)' : '';
      const scopeParams = sub ? [sub, sub.length + 1, `${sub}/`] : [];
      this.db.run(`UPDATE media SET seen = 0 WHERE folder_id = ?${scope}`, folderId, ...scopeParams);
      const maxBefore = this.db.get<{ m: number | null }>('SELECT max(id) m FROM media')?.m ?? 0;
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
          const mt = Math.round(st.mtimeMs);
          batch.push([folderId, rel, e.name, extOf(e.name), kind, st.size, mt, mt, this.localOf(mt)]);
          if (batch.length >= 400) flush();
        }
      };
      await walk(sub ? path.join(folder.path, ...sub.split('/')) : folder.path, sub);
      if (this.stopped) return;
      flush();
      const removed = Number(this.db.run(`DELETE FROM media WHERE folder_id = ? AND seen = 0${scope}`, folderId, ...scopeParams).changes);
      const added = this.db.get<{ c: number }>('SELECT count(*) c FROM media WHERE folder_id = ? AND id > ?', folderId, maxBefore)?.c ?? 0;
      if (added) this.events.mediaChanged(String(folderId), '*', 'add');
      else if (removed) this.events.mediaChanged(String(folderId), '*', 'remove');
      if (added || removed) this.onGroupsChanged?.();
      this.events.emit({ type: 'folders' });
    } finally {
      this.scanning.delete(lockKey);
      this.kick();
    }
  }

  /** Incremental update for a single file (watcher, uploads, edits). */
  async upsertFile(folderId: number, relPath: string): Promise<number | null> {
    if (this.stopped) return null;
    const folder = this.lib.folderRow(folderId);
    if (folder.device_id) return null; // device rows come from the phone's `list`
    const rel = trimChars(toPosix(relPath), '/');
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
    if (!st.isFile() || this.stopped) return null;
    const mt = Math.round(st.mtimeMs);
    const before = this.db.get<{ size: number; mtime: number }>('SELECT size, mtime FROM media WHERE folder_id = ? AND dir = ? AND name = ?', folderId, dir, name);
    this.db.run(UPSERT, folderId, dir, name, extOf(name), kind, st.size, mt, mt, this.localOf(mt));
    if (!before) this.events.mediaChanged(String(folderId), dir, 'add');
    else if (before.size !== st.size || before.mtime !== mt) this.events.mediaChanged(String(folderId), dir, 'change');
    this.kick();
    return this.db.get<{ id: number }>('SELECT id FROM media WHERE folder_id = ? AND dir = ? AND name = ?', folderId, dir, name)?.id ?? null;
  }

  removeFile(folderId: number, relPath: string) {
    if (this.stopped) return;
    const rel = trimChars(toPosix(relPath), '/');
    const name = path.posix.basename(rel);
    const dir = path.posix.dirname(rel) === '.' ? '' : path.posix.dirname(rel);
    const res = this.db.run('DELETE FROM media WHERE folder_id = ? AND dir = ? AND name = ?', folderId, dir, name);
    if (Number(res.changes)) this.events.mediaChanged(String(folderId), dir, 'remove');
  }

  removeDir(folderId: number, relDir: string) {
    if (this.stopped) return;
    const dir = trimChars(toPosix(relDir), '/');
    this.db.run('DELETE FROM media WHERE folder_id = ? AND (dir = ? OR substr(dir, 1, ?) = ?)', folderId, dir, dir.length + 1, `${dir}/`);
    this.events.mediaChanged(String(folderId), path.posix.dirname(dir) === '.' ? '' : path.posix.dirname(dir), 'remove');
    this.events.mediaChanged(String(folderId), dir, 'remove');
  }

  /** Start background metadata + thumbnail workers if idle. */
  kick() {
    if (this.stopped) return;
    if (!this.metaRunning) this.track(this.metaLoop());
    if (!this.thumbRunning) this.track(this.thumbLoop());
  }

  /** Start the low-priority enrichment loop when nothing else is going on. */
  kickEnrich() {
    if (this.stopped || this.enrichRunning || this.metaRunning || this.thumbRunning || this.scanning.size) return;
    this.track(this.enrichLoop());
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
      (this.db.get<{ c: number }>(
        "SELECT count(*) c FROM media WHERE thumb_state = 0 AND kind IN ('image','video') AND folder_id IN (SELECT id FROM folders WHERE device_id IS NULL)",
      )?.c ?? 0)
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
            const g = groupColumns(r, m, this.geo, this.homeTimeZone);
            this.db.run(
              `UPDATE media SET width = ?, height = ?, duration = ?, playback = ?, extra = ?, meta_state = 1, meta_v = ?, ${GROUP_SET_SQL} WHERE id = ?`,
              m.width,
              m.height,
              m.duration,
              m.playback,
              m.extra ? JSON.stringify(m.extra) : null,
              META_V,
              ...groupParams(g),
              r.id,
            );
          } catch {
            if (!this.stopped) this.db.run('UPDATE media SET meta_state = 2, meta_v = ? WHERE id = ?', META_V, r.id);
          }
          this.events.mediaChanged(String(r.folder_id), r.dir, 'meta');
          this.onGroupsChanged?.();
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
          // device folders are thumbnailed by the DeviceHub (through the tunnel, only while online)
          "SELECT * FROM media WHERE thumb_state = 0 AND meta_state != 0 AND kind IN ('image','video') AND folder_id IN (SELECT id FROM folders WHERE device_id IS NULL) ORDER BY taken_at DESC LIMIT 32",
        );
        if (!rows.length) break;
        await pool(rows, conc, async (r) => {
          if (this.stopped) return;
          const folder = safe(() => this.lib.folderRow(r.folder_id));
          if (!folder) return;
          const abs = this.abs(folder, r.dir, r.name);
          let reason: 'thumb' | 'meta' = 'thumb';
          try {
            const res = await this.thumbs.generateBase(abs, r, 'low');
            if (this.stopped) return;
            if (res) {
              const needDims = !r.width || !r.height;
              if (needDims && r.kind === 'image') reason = 'meta';
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
          this.events.mediaChanged(String(r.folder_id), r.dir, reason);
          this.progress.done++;
          this.emitScan();
        });
      }
    } finally {
      this.thumbRunning = false;
      this.emitScan(true);
      this.kickEnrich();
    }
  }

  /** image/video rows still waiting for the grouping metadata (ExploreResult.indexing) */
  indexing(): { done: number; total: number } {
    const r = this.db.get<{ total: number; todo: number }>(
      "SELECT count(*) total, coalesce(sum(CASE WHEN meta_v < ? AND meta_state != 2 THEN 1 ELSE 0 END), 0) todo FROM media WHERE kind IN ('image','video') AND folder_id IN (SELECT id FROM folders WHERE temp = 0 AND device_id IS NULL)",
      META_V,
    );
    return { done: (r?.total ?? 0) - (r?.todo ?? 0), total: r?.total ?? 0 };
  }

  /**
   * Low-priority enrichment of rows indexed by an older version (meta_v < META_V): GPS, camera,
   * time zone, flags. Runs only while the meta/thumbnail queues are idle; never emits per-row
   * events — one coalesced layout event per folder + an explore event every ENRICH_FLUSH_MS and at the end.
   */
  private async enrichLoop() {
    this.enrichRunning = true;
    const touched = new Map<number, Set<string>>();
    let changed = false;
    let lastFlush = Date.now();
    const flush = () => {
      for (const [fid, dirs] of touched) for (const d of dirs) this.events.mediaChanged(String(fid), d, 'meta');
      touched.clear();
      if (changed) this.onGroupsChanged?.();
      changed = false;
      lastFlush = Date.now();
    };
    try {
      let lastId = 0;
      for (;;) {
        if (this.stopped || this.metaRunning || this.thumbRunning || this.scanning.size) break;
        const rows = this.db.all<MediaRow>(
          "SELECT * FROM media WHERE meta_v < ? AND meta_state = 1 AND kind IN ('image','video') AND id > ? ORDER BY id LIMIT 32",
          META_V,
          lastId,
        );
        if (!rows.length) break;
        lastId = rows[rows.length - 1].id;
        await pool(rows, 2, async (r) => {
          if (this.stopped) return;
          const folder = safe(() => this.lib.folderRow(r.folder_id));
          const abs = folder ? this.abs(folder, r.dir, r.name) : null;
          const st = abs ? await fsp.stat(abs).catch(() => null) : null;
          if (!abs || !st) {
            this.db.run('UPDATE media SET meta_v = ? WHERE id = ?', META_V, r.id);
            return;
          }
          try {
            const m = await extractMeta(abs, r, this.ffmpeg);
            if (this.stopped) return;
            // keep known dimensions (thumbnail pass may have filled them)
            if (!m.width || !m.height) {
              m.width = r.width;
              m.height = r.height;
            }
            const g = groupColumns(r, m, this.geo, this.homeTimeZone);
            this.db.run(
              `UPDATE media SET meta_v = ?, width = COALESCE(width, ?), height = COALESCE(height, ?), ${GROUP_SET_SQL} WHERE id = ?`,
              META_V,
              m.width,
              m.height,
              ...groupParams(g),
              r.id,
            );
            changed = true;
            // result sets of place/camera/flag filters may change → new generation, no event
            this.events.bump();
            if (g.taken_local !== r.taken_local || (!r.width && m.width)) {
              let set = touched.get(r.folder_id);
              if (!set) touched.set(r.folder_id, (set = new Set()));
              set.add(r.dir);
            }
          } catch {
            if (!this.stopped) this.db.run('UPDATE media SET meta_v = ? WHERE id = ?', META_V, r.id);
          }
        });
        if (Date.now() - lastFlush > ENRICH_FLUSH_MS) flush();
      }
    } finally {
      this.enrichRunning = false;
      flush();
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
