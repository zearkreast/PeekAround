import fsp from 'node:fs/promises';
import path from 'node:path';
import { kindOf, extOf, joinRel, instantToLocal, systemTimeZone, excludeMatcher, type ExcludeMatcher } from '@peekaround/shared';
import type { Db } from '../db.js';
import type { EventHub } from '../events.js';
import type { Ffmpeg } from '../ffmpeg.js';
import type { GeoService } from '../geo.js';
import { DEVICE_SCHEME, REMOTE_SCHEME, type FolderRow, type LibraryService } from '../library.js';
import { isHiddenName, toPosix, trimChars } from '../paths.js';
import type { MediaRow } from './mapper.js';
import { extractMeta, GROUP_SET_SQL, groupColumns, groupParams, META_V, TIME_SRC } from './metadata.js';
import type { ThumbService } from './thumbs.js';

export const TRASH_DIR = '.peekaround-trash';

// background pass results are only written while the file is still the one that was read
const UNCHANGED = 'size = ? AND mtime = ?';
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

/** per-folder scan rules from the library options (exclusions, minimum size, hidden names) */
interface FolderRules {
  match: ExcludeMatcher;
  minBytes: number;
  showHidden: boolean | null;
}

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
  /** background items at once (Settings.indexIntensity via runtime-settings.ts; the budget itself is thumbs.bg) */
  concurrency: { meta: number; thumb: number } | null = null;
  /** DeviceHub: (re)list a device folder instead of walking the file system */
  deviceScan: ((folderId: number, subDir: string) => Promise<void>) | null = null;
  /** RemoteService: (re)list a remote (MEGA) folder instead of walking the file system */
  remoteScan: ((folderId: number) => Promise<void>) | null = null;
  private rules = new Map<number, FolderRules>();
  /** full scans requested while one was running */
  private rescanAfter = new Set<number>();

  constructor(
    private db: Db,
    private lib: LibraryService,
    private ffmpeg: Ffmpeg,
    private thumbs: ThumbService,
    readonly events: EventHub,
    private geo: GeoService,
  ) {
    // library options may have changed: recompute the per-folder rules lazily
    events.on((e) => {
      if (e.type === 'folders' && !e.status) this.rules.clear();
    });
  }

  get busy(): boolean {
    return this.scanning.size > 0 || this.metaRunning || this.thumbRunning;
  }

  /** a full walk of one of these folders is running (library header) */
  busyFor(folderIds: number[]): boolean {
    return folderIds.some((id) => this.scanning.has(id));
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
    // remote folders: remote://mega/<linkId>/<rel> (no key; thumbnail cache key + RemoteService lookups)
    if (folder.path.startsWith(REMOTE_SCHEME)) return `${folder.path}/${joinRel(dir, name)}`;
    return path.join(folder.path, ...dir.split('/').filter(Boolean), name);
  }

  skipName(name: string): boolean {
    if (name === TRASH_DIR) return true;
    return !this.showHidden && isHiddenName(name);
  }

  private rulesOf(folderId: number): FolderRules {
    let r = this.rules.get(folderId);
    if (!r) {
      const o = this.lib.optionsOfFolder(folderId);
      r = { match: excludeMatcher(o.exclude), minBytes: o.minBytes, showHidden: o.showHidden };
      this.rules.set(folderId, r);
    }
    return r;
  }

  /** forget cached library options (after a library update) */
  invalidateRules() {
    this.rules.clear();
  }

  /** a name skipped in this folder: trash, hidden names (library override → global setting) */
  private skipNameIn(folderId: number, name: string, r = this.rulesOf(folderId)): boolean {
    if (name === TRASH_DIR) return true;
    return !(r.showHidden ?? this.showHidden) && isHiddenName(name);
  }

  /**
   * A directory (relative to the folder root) that must not be indexed / listed:
   * hidden / trash segments or a library exclusion pattern.
   */
  skipDir(folderId: number, rel: string): boolean {
    if (!rel) return false;
    const r = this.rulesOf(folderId);
    if (rel.split('/').some((s) => this.skipNameIn(folderId, s, r))) return true;
    return !r.match.empty && r.match.path(rel);
  }

  /** A file that must not be indexed: skipped dir/name, exclusion pattern, or an image below the library's minimum size. */
  skipFile(folderId: number, rel: string, size?: number): boolean {
    const r = this.rulesOf(folderId);
    if (rel.split('/').some((s) => this.skipNameIn(folderId, s, r))) return true;
    if (!r.match.empty && r.match.path(rel)) return true;
    if (r.minBytes > 0 && size !== undefined && size < r.minBytes && kindOf(rel) === 'image') return true;
    return false;
  }

  async scanAll() {
    for (const f of this.lib.allFolderRows()) await this.scanFolder(f.id);
  }

  /**
   * (Re)scan one folder — or only `subDir` of it — and drop rows of files that disappeared.
   */
  async scanFolder(folderId: number, subDir = '') {
    if (this.stopped) return;
    const row = this.db.get<{ device_id: string | null; remote: string | null }>('SELECT device_id, remote FROM folders WHERE id = ?', folderId);
    if (row?.remote) {
      // never walk (or wipe) a remote folder from disk; the RemoteService re-lists the link (in the background)
      void this.remoteScan?.(folderId).catch(() => {});
      return;
    }
    if (row?.device_id) {
      // never walk (or wipe) a phone folder from disk; the hub lists it when the phone is online
      await this.deviceScan?.(folderId, subDir).catch(() => {});
      return;
    }
    const lockKey = subDir ? -Math.random() : folderId;
    if (!subDir && this.scanning.has(lockKey)) {
      // a full scan is already running: run once more afterwards (options may have changed meanwhile)
      this.rescanAfter.add(folderId);
      return;
    }
    this.scanning.add(lockKey);
    this.emitScan(true);
    try {
      const folder = this.lib.folderRow(folderId);
      const rules = this.rulesOf(folderId);
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
          if (this.skipNameIn(folderId, e.name, rules)) continue;
          const p = path.join(dirAbs, e.name);
          const childRel = joinRel(rel, e.name);
          if (e.isDirectory()) {
            if (!rules.match.empty && rules.match.path(childRel)) continue;
            await walk(p, childRel);
            continue;
          }
          const kind = kindOf(e.name);
          if (!kind) continue;
          if (!rules.match.empty && rules.match.path(childRel)) continue;
          let st;
          try {
            st = await fsp.stat(p);
          } catch {
            continue;
          }
          if (!st.isFile()) continue;
          if (rules.minBytes > 0 && kind === 'image' && st.size < rules.minBytes) continue;
          const mt = Math.round(st.mtimeMs);
          batch.push([folderId, rel, e.name, extOf(e.name), kind, st.size, mt, mt, this.localOf(mt)]);
          if (batch.length >= 400) flush();
        }
      };
      // a sub dir that is itself hidden / excluded (renamed or moved there): nothing in it is indexed
      if (!sub || !this.skipDir(folderId, sub)) await walk(sub ? path.join(folder.path, ...sub.split('/')) : folder.path, sub);
      if (this.stopped) return;
      flush();
      const removed = Number(this.db.run(`DELETE FROM media WHERE folder_id = ? AND seen = 0${scope}`, folderId, ...scopeParams).changes);
      if (!sub) this.db.run('UPDATE folders SET scanned_at = ? WHERE id = ?', Date.now(), folderId);
      const added = this.db.get<{ c: number }>('SELECT count(*) c FROM media WHERE folder_id = ? AND id > ?', folderId, maxBefore)?.c ?? 0;
      if (added) this.events.mediaChanged(String(folderId), '*', 'add');
      else if (removed) this.events.mediaChanged(String(folderId), '*', 'remove');
      if (added || removed) this.onGroupsChanged?.();
      this.events.emit({ type: 'folders' });
    } finally {
      this.scanning.delete(lockKey);
      if (!subDir && this.rescanAfter.delete(folderId)) void this.scanFolder(folderId);
      this.kick();
    }
  }

  /** Incremental update for a single file (watcher, uploads, edits). */
  async upsertFile(folderId: number, relPath: string): Promise<number | null> {
    if (this.stopped) return null;
    const folder = this.lib.folderRow(folderId);
    if (folder.device_id || folder.remote) return null; // device rows come from the phone's `list`, remote rows from the link listing
    const rel = trimChars(toPosix(relPath), '/');
    const name = path.posix.basename(rel);
    const dir = path.posix.dirname(rel) === '.' ? '' : path.posix.dirname(rel);
    if (this.skipFile(folderId, rel)) return null;
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
    // below the library's minimum size (icons, stickers): not indexed — drop a row that may exist
    if (this.skipFile(folderId, rel, st.size)) {
      this.removeFile(folderId, rel);
      return null;
    }
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

  /**
   * A file renamed / moved by PeekAround: move its row instead of delete + insert, so the media id
   * — and with it favourites, album membership, covers and metadata — survives. Returns false when
   * there was no row to move (or the kind changes); callers then fall back to removeFile + upsertFile.
   */
  relocateFile(fromFolder: number, fromRel: string, toFolder: number, toRel: string): boolean {
    if (this.stopped) return false;
    const src = trimChars(toPosix(fromRel), '/');
    const dst = trimChars(toPosix(toRel), '/');
    const split = (r: string) => [path.posix.dirname(r) === '.' ? '' : path.posix.dirname(r), path.posix.basename(r)] as const;
    const [fd, fn] = split(src);
    const [td, tn] = split(dst);
    const row = this.db.get<{ id: number; kind: string; size: number }>('SELECT id, kind, size FROM media WHERE folder_id = ? AND dir = ? AND name = ?', fromFolder, fd, fn);
    if (!row) return false;
    const to = this.db.get<{ device_id: string | null; remote: string | null }>('SELECT device_id, remote FROM folders WHERE id = ?', toFolder);
    if (!to || to.device_id || to.remote || kindOf(tn) !== row.kind) return false;
    if (this.skipFile(toFolder, dst, row.size)) {
      this.removeFile(fromFolder, src);
      return true;
    }
    this.db.tx(() => {
      this.db.run('DELETE FROM media WHERE folder_id = ? AND dir = ? AND name = ? AND id != ?', toFolder, td, tn, row.id);
      this.db.run('UPDATE media SET folder_id = ?, dir = ?, name = ?, ext = ? WHERE id = ?', toFolder, td, tn, extOf(tn), row.id);
    });
    this.events.mediaChanged(String(fromFolder), fd, 'remove');
    this.events.mediaChanged(String(toFolder), td, 'add');
    this.onGroupsChanged?.();
    return true;
  }

  /** A directory renamed / moved by PeekAround: move the rows under it (ids kept, see relocateFile). Returns the number of rows moved. */
  relocateDir(fromFolder: number, fromRel: string, toFolder: number, toRel: string): number {
    if (this.stopped) return 0;
    const src = trimChars(toPosix(fromRel), '/');
    const dst = trimChars(toPosix(toRel), '/');
    if (!src || !dst) return 0;
    const to = this.db.get<{ device_id: string | null; remote: string | null }>('SELECT device_id, remote FROM folders WHERE id = ?', toFolder);
    if (!to || to.device_id || to.remote) return 0;
    const under = '(dir = ? OR substr(dir, 1, ?) = ?)';
    const n = this.db.tx(() => {
      // stale rows at the destination (not on disk any more — the destination name is unique)
      this.db.run(`DELETE FROM media WHERE folder_id = ? AND ${under}`, toFolder, dst, dst.length + 1, `${dst}/`);
      return Number(
        this.db.run(
          `UPDATE media SET folder_id = ?, dir = CASE WHEN dir = ? THEN ? ELSE ? || substr(dir, ?) END WHERE folder_id = ? AND ${under}`,
          toFolder,
          src,
          dst,
          dst,
          src.length + 1,
          fromFolder,
          src,
          src.length + 1,
          `${src}/`,
        ).changes,
      );
    });
    if (n) {
      const parent = (r: string) => (path.posix.dirname(r) === '.' ? '' : path.posix.dirname(r));
      this.events.mediaChanged(String(fromFolder), parent(src), 'remove');
      this.events.mediaChanged(String(fromFolder), src, 'remove');
      this.events.mediaChanged(String(toFolder), dst, 'add');
      this.onGroupsChanged?.();
    }
    return n;
  }

  /** Start background metadata + thumbnail workers if idle. */
  kick() {
    if (this.stopped) return;
    if (!this.metaRunning) this.track(this.metaLoop());
    if (!this.thumbRunning) this.track(this.thumbLoop());
  }

  /**
   * Start the low-priority enrichment loop once scanning + the meta pass are done. It also runs
   * (throttled, one file at a time) beside a long thumbnail pass, so re-reading metadata after an
   * upgrade (META_V) does not wait hours for a big library's thumbnails.
   */
  kickEnrich() {
    if (this.stopped || this.enrichRunning || this.metaRunning || this.scanning.size) return;
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
    this.bg.close();
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
        "SELECT count(*) c FROM media WHERE thumb_state = 0 AND kind IN ('image','video') AND folder_id IN (SELECT id FROM folders WHERE device_id IS NULL AND remote IS NULL)",
      )?.c ?? 0)
    );
  }

  /** every background item takes a slot of the shared budget (Settings.indexIntensity) */
  private get bg() {
    return this.thumbs.bg;
  }

  /**
   * Row updates of the background passes are committed together: a commit per row cost ~4× the
   * main-thread time (WAL append + index updates per transaction), measured at 0.2–0.4 ms × 100k rows.
   * `after` (events, progress) runs once the rows are written. Updates are conditional on the row
   * not having changed meanwhile (size + mtime), so a watcher re-index is never overwritten.
   */
  private writes: { run: () => void; after?: () => void }[] = [];
  private writesSince = 0;

  private write(run: () => void, after?: () => void) {
    if (this.stopped) return;
    if (!this.writes.length) this.writesSince = Date.now();
    this.writes.push({ run, after });
    if (this.writes.length >= 48 || Date.now() - this.writesSince > 1000) this.flushWrites();
  }

  private flushWrites() {
    if (!this.writes.length) return;
    const list = this.writes;
    this.writes = [];
    if (this.stopped) return;
    try {
      this.db.tx(() => {
        for (const w of list) {
          try {
            w.run();
          } catch {
            /* this row only */
          }
        }
      });
    } catch {
      return; // database closed
    }
    for (const w of list) w.after?.();
  }

  private async metaLoop() {
    this.metaRunning = true;
    this.progress = { done: 0, total: this.pendingCount() };
    try {
      for (;;) {
        if (this.stopped) break;
        const rows = this.db.all<MediaRow>('SELECT * FROM media WHERE meta_state = 0 LIMIT 64');
        if (!rows.length) break;
        await this.bg.each(rows, async (r) => {
          if (this.stopped) return;
          const folder = safe(() => this.lib.folderRow(r.folder_id));
          if (!folder) return;
          const abs = this.abs(folder, r.dir, r.name);
          const done = () => {
            this.events.mediaChanged(String(r.folder_id), r.dir, 'meta');
            this.progress.done++;
            this.emitScan();
          };
          try {
            const m = await extractMeta(abs, r, this.ffmpeg);
            if (this.stopped) return;
            const g = groupColumns(r, m, this.geo, this.homeTimeZone);
            this.write(
              () =>
                this.db.run(
                  `UPDATE media SET width = ?, height = ?, duration = ?, playback = ?, extra = ?, meta_state = 1, meta_v = ?, ${GROUP_SET_SQL} WHERE id = ? AND ${UNCHANGED}`,
                  m.width,
                  m.height,
                  m.duration,
                  m.playback,
                  m.extra ? JSON.stringify(m.extra) : null,
                  META_V,
                  ...groupParams(g),
                  r.id,
                  r.size,
                  r.mtime,
                ),
              done,
            );
          } catch {
            this.write(() => this.db.run(`UPDATE media SET meta_state = 2, meta_v = ? WHERE id = ? AND ${UNCHANGED}`, META_V, r.id, r.size, r.mtime), done);
          }
        }, () => this.stopped);
        this.flushWrites();
        // grouping data changed: one Explore invalidation per batch (not per file)
        this.onGroupsChanged?.();
      }
    } finally {
      this.flushWrites();
      this.metaRunning = false;
      this.kickThumbs();
      this.kickEnrich();
      this.emitScan(true);
    }
  }

  private kickThumbs() {
    if (!this.thumbRunning && !this.stopped) this.track(this.thumbLoop());
  }

  private async thumbLoop() {
    this.thumbRunning = true;
    try {
      for (;;) {
        if (this.stopped) break;
        // only thumbnail rows whose metadata is known (so videos have duration)
        const rows = this.db.all<MediaRow>(
          // device folders are thumbnailed by the DeviceHub (through the tunnel, only while online)
          "SELECT * FROM media WHERE thumb_state = 0 AND meta_state != 0 AND kind IN ('image','video') AND folder_id IN (SELECT id FROM folders WHERE device_id IS NULL AND remote IS NULL) ORDER BY taken_at DESC LIMIT 32",
        );
        if (!rows.length) break;
        await this.bg.each(rows, async (r) => {
          if (this.stopped) return;
          const folder = safe(() => this.lib.folderRow(r.folder_id));
          if (!folder) return;
          const abs = this.abs(folder, r.dir, r.name);
          let reason: 'thumb' | 'meta' = 'thumb';
          const done = () => {
            this.events.mediaChanged(String(r.folder_id), r.dir, reason);
            this.progress.done++;
            this.emitScan();
          };
          const failed = () => this.db.run(`UPDATE media SET thumb_state = 2 WHERE id = ? AND ${UNCHANGED}`, r.id, r.size, r.mtime);
          try {
            const res = await this.thumbs.generateBase(abs, r, 'low');
            if (this.stopped) return;
            if (res) {
              const needDims = !r.width || !r.height;
              if (needDims && r.kind === 'image') reason = 'meta';
              this.write(
                () =>
                  this.db.run(
                    `UPDATE media SET thumbhash = ?, thumb_state = 1, width = COALESCE(width, ?), height = COALESCE(height, ?) WHERE id = ? AND ${UNCHANGED}`,
                    res.thumbhash,
                    needDims && r.kind === 'image' ? res.width : null,
                    needDims && r.kind === 'image' ? res.height : null,
                    r.id,
                    r.size,
                    r.mtime,
                  ),
                done,
              );
            } else this.write(failed, done);
          } catch {
            this.write(failed, done);
          }
        }, () => this.stopped);
        this.flushWrites();
      }
    } finally {
      this.flushWrites();
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
      // result sets of place / camera / flag filters may have changed → one new generation per flush
      // (not per row: see EventHub.mediaChanged), no event of its own
      if (changed) this.events.bumpThrottled();
      if (changed) this.onGroupsChanged?.();
      changed = false;
      lastFlush = Date.now();
    };
    try {
      let lastId = 0;
      for (;;) {
        if (this.stopped || this.metaRunning || this.scanning.size) break;
        // beside the thumbnail pass: small paced batches so thumbnails keep most of the I/O
        if (this.thumbRunning) await new Promise((r) => setTimeout(r, 400));
        const rows = this.db.all<MediaRow>(
          // remote (MEGA) rows are enriched by the RemoteService
          "SELECT * FROM media WHERE meta_v < ? AND meta_state = 1 AND kind IN ('image','video') AND id > ? AND folder_id IN (SELECT id FROM folders WHERE remote IS NULL) ORDER BY id LIMIT 32",
          META_V,
          lastId,
        );
        if (!rows.length) break;
        lastId = rows[rows.length - 1].id;
        await this.bg.each(rows, async (r) => {
          if (this.stopped) return;
          const folder = safe(() => this.lib.folderRow(r.folder_id));
          const abs = folder ? this.abs(folder, r.dir, r.name) : null;
          const st = abs ? await fsp.stat(abs).catch(() => null) : null;
          if (!abs || !st) {
            this.write(() => this.db.run('UPDATE media SET meta_v = ? WHERE id = ?', META_V, r.id));
            return;
          }
          try {
            // dimensions are known: JPEGs only need their EXIF / XMP read again
            const m = await extractMeta(abs, r, this.ffmpeg, r);
            if (this.stopped) return;
            // keep known dimensions (thumbnail pass may have filled them)
            if (!m.width || !m.height) {
              m.width = r.width;
              m.height = r.height;
            }
            const g = groupColumns(r, m, this.geo, this.homeTimeZone);
            this.write(
              () =>
                this.db.run(
                  `UPDATE media SET meta_v = ?, width = COALESCE(width, ?), height = COALESCE(height, ?), ${GROUP_SET_SQL} WHERE id = ? AND ${UNCHANGED}`,
                  META_V,
                  m.width,
                  m.height,
                  ...groupParams(g),
                  r.id,
                  r.size,
                  r.mtime,
                ),
              () => {
                changed = true;
                if (g.taken_local !== r.taken_local || (!r.width && m.width)) {
                  let set = touched.get(r.folder_id);
                  if (!set) touched.set(r.folder_id, (set = new Set()));
                  set.add(r.dir);
                }
              },
            );
          } catch {
            this.write(() => this.db.run(`UPDATE media SET meta_v = ? WHERE id = ? AND ${UNCHANGED}`, META_V, r.id, r.size, r.mtime));
          }
        }, () => this.stopped || this.metaRunning || this.scanning.size > 0);
        this.flushWrites();
        if (Date.now() - lastFlush > ENRICH_FLUSH_MS) flush();
      }
    } finally {
      this.flushWrites();
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
