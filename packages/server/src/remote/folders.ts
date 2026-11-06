import fs from 'node:fs';
import fsp from 'node:fs/promises';
import path from 'node:path';
import { PassThrough, Readable, Transform } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import type { FastifyReply, FastifyRequest } from 'fastify';
import exifr from 'exifr';
import sharp from 'sharp';
import {
  BROWSER_IMAGE_EXT,
  extOf,
  kindOf,
  type FileRef,
  type Job,
  type MediaDetails,
  type MediaKind,
  type RemoteCopySpec,
  type RemoteErrorCode,
  type RemoteFolderCreate,
  type RemoteFolderCreated,
  type RemoteFolderInfo,
  type RemoteFolderStatus,
} from '@peekaround/shared';
import { getSettings, type Ctx } from '../context.js';
import { clientGone, contentDisposition, mimeOf, relOfRow, singleRange } from '../devices/media.js';
import { isBrowserPlayable } from '../ffmpeg.js';
import { REMOTE_SCHEME, type FolderRow } from '../library.js';
import { toItem, type MediaRow } from '../media/mapper.js';
import { applyExif, GROUP_SET_SQL, groupColumns, groupParams, META_V, videoGroupMeta, type ExtractedMeta } from '../media/metadata.js';
import { pool } from '../media/scanner.js';
import type { Priority, RemoteImageSource } from '../media/thumbs.js';
import { HttpError, normalizeRel } from '../paths.js';
import { RemoteCache } from './cache.js';
import { parseFa } from './crypto.js';
import { startRemoteImport } from './import.js';
import { megaError, parseMegaUrl, type MegaLink, type MegaNode } from './mega.js';

/**
 * Remote (streaming) folders: a MEGA folder or file link added to a library WITHOUT downloading it.
 *
 *  - listing: the link's tree (megajs) → media rows (dir = path inside the link, mtime = node ts);
 *    re-listed every 30 min and on demand (diff: new rows added, vanished ones deleted);
 *  - bytes are fetched on demand with HTTP ranges (AES-CTR → random access), ≤ 4 transfers per link
 *    (2 for background work), originals that were viewed land in a size-capped LRU cache;
 *  - thumbnails: MEGA file attributes (preview → 120px thumbnail) → full download (images ≤ 30 MB)
 *    → a video frame via ffmpeg over the internal loopback URL;
 *  - metadata: background, rate limited: the first 256 KB of images → exifr/sharp, videos → ffmpeg probe
 *    over loopback; rows wait in meta_state 3 until then;
 *  - transfer quota (509 / EOVERQUOTA): no MEGA transfer until it resets (state 'quota', retryAt);
 *    an unreachable MEGA keeps the old rows (state 'offline') and retries with backoff.
 *
 * The link (with its key) lives in folders.remote and never leaves the server except through the
 * local-only GET /api/remote/folders/:id/link.
 */

interface RemoteSpec {
  kind: 'mega';
  url: string;
  linkId: string;
  linkKind: 'file' | 'folder';
}

interface Tree {
  link: MegaLink;
  root: MegaNode;
  /** rel path → file node (names made unique per dir like the rows) */
  files: Map<string, MegaNode>;
  /** rel path → directory node ('' = the root of a folder link) */
  dirs: Map<string, MegaNode>;
  at: number;
}

export const REMOTE_REFRESH_MS = 30 * 60_000;
const OFFLINE_RETRY_MS = [60_000, 2 * 60_000, 5 * 60_000, 10 * 60_000, 30 * 60_000];
const HEAD_BYTES = 256 * 1024;
const FULL_THUMB_MAX = 30 * 1024 * 1024;
const MAX_NODES = 200_000;
const DEFAULT_QUOTA_WAIT_S = 30 * 60;
/** meta_state of remote rows waiting for enrichment */
export const META_REMOTE_PENDING = 3;

const EXIF_OPTS = {
  tiff: true,
  exif: true,
  gps: true,
  xmp: true,
  icc: false,
  iptc: false,
  jfif: false,
  ihdr: false,
  interop: false,
  makerNote: false,
  userComment: false,
  reviveValues: false,
  translateValues: false,
  translateKeys: true,
  mergeOutput: true,
} as const;

function emptyMeta(playback: ExtractedMeta['playback']): ExtractedMeta {
  return {
    width: null,
    height: null,
    duration: null,
    playback,
    extra: null,
    local: null,
    offset: null,
    instant: null,
    lat: null,
    lon: null,
    make: null,
    model: null,
    lens: null,
    gpano: false,
    pages: null,
  };
}

function playbackOf(kind: MediaKind, ext: string): ExtractedMeta['playback'] {
  if (kind === 'image') return BROWSER_IMAGE_EXT.includes(ext) ? 'direct' : 'convert';
  if (kind === 'video') return isBrowserPlayable(ext, null) ? 'direct' : 'transcode';
  return 'direct';
}

/** a MEGA name as a single safe path segment */
function segment(name: string): string {
  const s = name.replace(/[\\/\u0000-\u001f]/g, '_').trim();
  return !s || s === '.' || s === '..' ? '_' : s.slice(0, 255);
}

function uniqueIn(used: Set<string>, name: string): string {
  if (!used.has(name)) {
    used.add(name);
    return name;
  }
  const dot = name.lastIndexOf('.');
  const base = dot > 0 ? name.slice(0, dot) : name;
  const ext = dot > 0 ? name.slice(dot) : '';
  for (let i = 2; ; i++) {
    const n = `${base} (${i})${ext}`;
    if (!used.has(n)) {
      used.add(n);
      return n;
    }
  }
}

const abortError = () => Object.assign(new Error('aborted'), { name: 'AbortError' });
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** two-lane semaphore: 'high' (viewer, playback) always first, 'low' (background) never above lowMax */
export class Lanes {
  private running = 0;
  private runningLow = 0;
  private queue: { prio: Priority; go: () => void }[] = [];

  constructor(
    readonly max = 4,
    readonly lowMax = 2,
  ) {}

  get stats() {
    return { running: this.running, waiting: this.queue.length };
  }

  acquire(prio: Priority, signal?: AbortSignal): Promise<() => void> {
    return new Promise((resolve, reject) => {
      if (signal?.aborted) return reject(abortError());
      const entry = {
        prio,
        go: () => {
          signal?.removeEventListener('abort', onAbort);
          this.running++;
          if (prio === 'low') this.runningLow++;
          let released = false;
          resolve(() => {
            if (released) return;
            released = true;
            this.running--;
            if (prio === 'low') this.runningLow--;
            this.pump();
          });
        },
      };
      const onAbort = () => {
        const i = this.queue.indexOf(entry);
        if (i >= 0) this.queue.splice(i, 1);
        reject(abortError());
      };
      signal?.addEventListener('abort', onAbort, { once: true });
      if (prio === 'high') {
        // high requests queue in front of every waiting low request
        const firstLow = this.queue.findIndex((q) => q.prio === 'low');
        this.queue.splice(firstLow < 0 ? this.queue.length : firstLow, 0, entry);
      } else this.queue.push(entry);
      this.pump();
    });
  }

  private pump() {
    while (this.running < this.max) {
      const i = this.queue.findIndex((q) => q.prio === 'high' || this.runningLow < this.lowMax);
      if (i < 0) return;
      const [e] = this.queue.splice(i, 1);
      e.go();
    }
  }
}

/** Readable that opens its source only when first read (zip entries) */
export function lazyStream(open: (signal: AbortSignal) => Promise<Readable>): Readable {
  const ctrl = new AbortController();
  let src: Readable | null = null;
  let opening = false;
  const out = new Readable({
    read() {
      if (src) {
        src.resume();
        return;
      }
      if (opening) return;
      opening = true;
      open(ctrl.signal).then(
        (s) => {
          // destroyed while opening: never keep the source (and its transfer slot)
          if (out.destroyed) {
            s.destroy();
            return;
          }
          src = s;
          s.on('data', (c) => {
            if (!out.push(c)) s.pause();
          });
          s.once('end', () => out.push(null));
          s.once('error', (e) => out.destroy(e));
        },
        (e) => out.destroy(e),
      );
    },
    destroy(err, cb) {
      ctrl.abort();
      src?.destroy();
      cb(err);
    },
  });
  return out;
}

/** exactly bytes [start, end] of a whole-file stream (fallback for clients without ranged reads) */
function sliceStream(src: Readable, start: number, end: number): Readable {
  let pos = 0;
  const t = new Transform({
    transform(chunk: Buffer, _e, cb) {
      const from = Math.max(0, start - pos);
      const to = Math.min(chunk.length, end + 1 - pos);
      pos += chunk.length;
      if (to > from) this.push(chunk.subarray(from, to));
      if (pos > end) {
        this.push(null);
        src.destroy();
      }
      cb();
    },
  });
  src.on('error', (e) => t.destroy(e));
  return src.pipe(t);
}

function errorCode(err: HttpError, msg: string): RemoteErrorCode {
  if (err.statusCode === 429) return 'quota';
  if (/taken down/.test(msg)) return 'blocked';
  if (err.statusCode === 404) return 'not-found';
  if (err.statusCode === 400) return 'key';
  if (err.statusCode === 502 && /reach/i.test(msg)) return 'network';
  if (err.statusCode === 503) return 'unavailable';
  return 'other';
}

export class RemoteService implements RemoteImageSource {
  private trees = new Map<number, Tree>();
  private listing = new Map<number, Promise<Tree>>();
  private lanes = new Map<string, Lanes>();
  private failures = new Map<number, number>();
  /** thumbnails that came from MEGA's 120px square (their size says nothing about the aspect) */
  private squares = new Set<string>();
  private fetching = new Map<string, Promise<{ file: string; temp: boolean }>>();
  private timer: NodeJS.Timeout;
  private closed = false;
  /** aborted by close(): in-flight MEGA transfers stop instead of holding shutdown up */
  private shutdown = new AbortController();
  private loops = new Set<Promise<void>>();
  private metaRunning = false;
  private thumbRunning = false;
  /** MEGA transfer quota exhausted until (ms epoch) — shared by every link (the quota is per IP, or per account when logged in) */
  quotaUntil = 0;
  /** API rate limit / congestion back-off (EAGAIN / ERATELIMIT) */
  private backoffUntil = 0;
  readonly cache: RemoteCache;
  /** test hooks */
  metaGapMs = 150;
  refreshMs = REMOTE_REFRESH_MS;

  constructor(private ctx: Ctx) {
    ctx.thumbs.cloud = this;
    ctx.scanner.remoteScan = (id) => this.refresh(id).then(() => {});
    ctx.lib.remoteLookup = (r) => this.info(r);
    this.cache = new RemoteCache(path.join(ctx.cfg.cacheDir, 'remote'), getSettings(ctx).remoteCacheMb * 1024 * 1024);
    const q = ctx.db.getSetting<{ until?: number }>('remoteQuota', {});
    if (q.until && q.until > Date.now()) this.quotaUntil = q.until;
    this.timer = setInterval(() => this.tick(), 60_000);
    this.timer.unref();
  }

  // ───────────────────────── folder rows / status ─────────────────────────

  private folders(): FolderRow[] {
    return this.ctx.db.all<FolderRow>('SELECT * FROM folders WHERE remote IS NOT NULL ORDER BY id');
  }

  spec(folder: FolderRow): RemoteSpec {
    if (!folder.remote) throw new HttpError(400, 'Not a remote folder');
    return JSON.parse(folder.remote) as RemoteSpec;
  }

  private storedStatus(folder: FolderRow): RemoteFolderStatus {
    try {
      const s = JSON.parse(folder.remote_status ?? '');
      if (s && typeof s === 'object') return { state: 'ok', error: null, code: null, retryAt: null, listedAt: null, ...s };
    } catch {
      /* default */
    }
    return { state: 'listing', error: null, code: null, retryAt: null, listedAt: null };
  }

  private setStatus(folderId: number, patch: Partial<RemoteFolderStatus>) {
    const row = this.ctx.db.get<FolderRow>('SELECT * FROM folders WHERE id = ?', folderId);
    if (!row?.remote) return;
    const next = { ...this.storedStatus(row), ...patch };
    this.ctx.db.run('UPDATE folders SET remote_status = ? WHERE id = ?', JSON.stringify(next), folderId);
    this.ctx.events.emit({ type: 'folders', status: true });
  }

  /** FolderRef.remote — never contains the key */
  info(folder: FolderRow): RemoteFolderInfo {
    const spec = this.spec(folder);
    const st = this.storedStatus(folder);
    const quota = this.quotaUntil > Date.now() && st.state !== 'error' && st.state !== 'listing';
    const counts = this.ctx.db.get<{ total: number; todo: number }>(
      `SELECT count(*) total, coalesce(sum(CASE WHEN meta_state = ${META_REMOTE_PENDING} OR thumb_state = 0 THEN 1 ELSE 0 END), 0) todo
       FROM media WHERE folder_id = ? AND kind IN ('image','video')`,
      folder.id,
    );
    const total = counts?.total ?? 0;
    return {
      provider: 'mega',
      label: `mega.nz/${spec.linkKind}/${spec.linkId}`,
      linkKind: spec.linkKind,
      ...st,
      ...(quota ? { state: 'quota' as const, code: 'quota' as const, retryAt: this.quotaUntil, error: 'MEGA transfer quota exceeded' } : {}),
      indexing: { done: total - (counts?.todo ?? 0), total },
    };
  }

  /** the full link (with key) of a remote folder — local clients only (see routes/remote.ts) */
  linkOf(folder: FolderRow): string {
    return this.spec(folder).url;
  }

  // ───────────────────────── adding / listing ─────────────────────────

  async create(input: RemoteFolderCreate): Promise<RemoteFolderCreated> {
    const link = parseMegaUrl(input?.url);
    const root = await this.load(link);
    const name = (input.name?.trim() || root.name || 'MEGA').slice(0, 120);
    let libraryId: number;
    if (input.libraryId) libraryId = Number(this.ctx.lib.library(input.libraryId).id);
    else libraryId = Number(this.ctx.lib.createLibrary({ name: input.libraryName?.trim() || name, icon: 'cloud' }).id);
    const spec: RemoteSpec = { kind: 'mega', url: String(input.url).trim(), linkId: link.id, linkKind: link.type };
    const p = `${REMOTE_SCHEME}mega/${link.id}${link.sub ? `-${link.sub.id}` : ''}`;
    const status: RemoteFolderStatus = { state: 'ok', error: null, code: null, retryAt: null, listedAt: Date.now() };
    const row = this.ctx.lib.addRemoteFolder(libraryId, { path: p, name, remote: spec, status });
    const tree = this.buildTree(link, root);
    this.trees.set(row.id, tree);
    this.ingest(row, tree);
    this.kick();
    return { library: this.ctx.lib.library(libraryId), folder: this.ctx.lib.folderRef(this.ctx.lib.folderRow(row.id)) };
  }

  private async load(link: MegaLink): Promise<MegaNode> {
    try {
      return await this.ctx.mega.load(link);
    } catch (e) {
      throw megaError(e);
    }
  }

  /** re-list now (deduplicated); resolves when the rows are up to date */
  refresh(folderId: number): Promise<Tree> {
    const cur = this.listing.get(folderId);
    if (cur) return cur;
    const p = this.relist(folderId).finally(() => this.listing.delete(folderId));
    this.listing.set(folderId, p);
    return p;
  }

  private async relist(folderId: number): Promise<Tree> {
    if (this.closed) throw new HttpError(503, 'shutting down');
    const folder = this.ctx.lib.folderRow(folderId);
    const spec = this.spec(folder);
    const link = parseMegaUrl(spec.url);
    const prev = this.storedStatus(folder);
    this.setStatus(folderId, { state: 'listing' });
    let root: MegaNode;
    try {
      root = await this.load(link);
    } catch (e) {
      const err = megaError(e);
      const n = (this.failures.get(folderId) ?? 0) + 1;
      this.failures.set(folderId, n);
      const code = errorCode(err, err.message);
      const transient = code === 'network' || code === 'unavailable' || code === 'other' || code === 'quota';
      this.setStatus(folderId, {
        state: code === 'network' ? 'offline' : code === 'quota' ? 'quota' : 'error',
        error: err.message,
        code,
        retryAt: transient ? Date.now() + OFFLINE_RETRY_MS[Math.min(n - 1, OFFLINE_RETRY_MS.length - 1)] : null,
        listedAt: prev.listedAt,
      });
      throw err;
    }
    if (this.closed) throw new HttpError(503, 'shutting down');
    this.failures.delete(folderId);
    const tree = this.buildTree(link, root);
    // the folder may have been removed while listing
    const still = this.ctx.db.get<FolderRow>('SELECT * FROM folders WHERE id = ?', folderId);
    if (!still) throw new HttpError(404, 'Folder not found');
    this.trees.set(folderId, tree);
    this.ingest(still, tree);
    this.setStatus(folderId, { state: 'ok', error: null, code: null, retryAt: null, listedAt: Date.now() });
    this.kick();
    return tree;
  }

  /** the loaded tree of a folder (lists it once after a restart) */
  async tree(folder: FolderRow): Promise<Tree> {
    const t = this.trees.get(folder.id);
    if (t) return t;
    const st = this.storedStatus(folder);
    // offline / broken link: no request storm from every thumbnail until the next retry
    if ((st.state === 'offline' || st.state === 'error') && (st.retryAt ?? Infinity) > Date.now() && this.failures.get(folder.id)) {
      throw new HttpError(503, st.error ?? 'MEGA is not reachable');
    }
    return this.refresh(folder.id);
  }

  private buildTree(link: MegaLink, root: MegaNode): Tree {
    const files = new Map<string, MegaNode>();
    const dirs = new Map<string, MegaNode>();
    let count = 0;
    if (!root.directory) {
      files.set(segment(root.name), root);
      return { link, root, files, dirs, at: Date.now() };
    }
    dirs.set('', root);
    const queue: { node: MegaNode; rel: string }[] = [{ node: root, rel: '' }];
    while (queue.length && count < MAX_NODES) {
      const { node, rel } = queue.shift()!;
      const used = new Set<string>();
      const kids = [...node.children].sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : a.id < b.id ? -1 : 1));
      for (const k of kids) {
        if (++count > MAX_NODES) break;
        const name = uniqueIn(used, segment(k.name));
        const r = rel ? `${rel}/${name}` : name;
        if (k.directory) {
          dirs.set(r, k);
          queue.push({ node: k, rel: r });
        } else files.set(r, k);
      }
    }
    return { link, root, files, dirs, at: Date.now() };
  }

  /** tree → media rows (diff by dir/name/size/mtime), empty dirs → device_dirs */
  private ingest(folder: FolderRow, tree: Tree) {
    const db = this.ctx.db;
    const tz = this.ctx.scanner.homeTimeZone;
    const maxBefore = db.get<{ m: number | null }>('SELECT max(id) m FROM media')?.m ?? 0;
    let changed = 0;
    const dirSet = new Set<string>();
    db.tx(() => {
      db.run('UPDATE media SET seen = 0 WHERE folder_id = ?', folder.id);
      for (const [rel, node] of tree.files) {
        const segs = rel.split('/');
        if (this.ctx.scanner.skipFile(folder.id, rel, node.size)) continue;
        const name = segs.pop()!;
        const dir = segs.join('/');
        const kind = kindOf(name);
        if (!kind) continue;
        const size = node.size;
        const mtime = node.timestamp && node.timestamp > 0 ? node.timestamp : 0;
        const same = db.run('UPDATE media SET seen = 1 WHERE folder_id = ? AND dir = ? AND name = ? AND size = ? AND mtime = ?', folder.id, dir, name, size, mtime);
        if (Number(same.changes)) continue;
        const ext = extOf(name);
        const m = emptyMeta(playbackOf(kind, ext));
        const g = groupColumns({ name, dir, ext, kind, mtime }, m, this.ctx.geo, tz);
        const metaState = kind === 'image' || kind === 'video' ? META_REMOTE_PENDING : 1;
        db.run(
          `INSERT INTO media (folder_id, dir, name, ext, kind, size, mtime, playback, meta_state, meta_v, thumb_state, seen,
                              taken_at, taken_local, tz_off, time_src, lat, lon, geo_id, cc, admin1, city, make, model, lens, flags)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0, 1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
           ON CONFLICT (folder_id, dir, name) DO UPDATE SET
             seen = 1, ext = excluded.ext, kind = excluded.kind, size = excluded.size, mtime = excluded.mtime,
             width = NULL, height = NULL, duration = NULL, extra = NULL, playback = excluded.playback,
             meta_state = excluded.meta_state, meta_v = excluded.meta_v, thumb_state = 0, thumbhash = NULL,
             ${GROUP_SET_SQL.replace(/(\w+) = \?/g, '$1 = excluded.$1')}`,
          folder.id,
          dir,
          name,
          ext,
          kind,
          size,
          mtime,
          m.playback,
          metaState,
          META_V,
          ...groupParams(g),
        );
        changed++;
      }
      for (const d of tree.dirs.keys()) if (d && !this.ctx.scanner.skipDir(folder.id, d)) dirSet.add(d);
    });
    const removed = Number(db.run('DELETE FROM media WHERE folder_id = ? AND seen = 0', folder.id).changes);
    db.tx(() => {
      db.run('DELETE FROM device_dirs WHERE folder_id = ?', folder.id);
      for (const d of dirSet) db.run('INSERT OR IGNORE INTO device_dirs (folder_id, dir) VALUES (?, ?)', folder.id, d);
    });
    const added = db.get<{ c: number }>('SELECT count(*) c FROM media WHERE folder_id = ? AND id > ?', folder.id, maxBefore)?.c ?? 0;
    const fid = String(folder.id);
    if (added) this.ctx.events.mediaChanged(fid, '*', 'add');
    else if (removed) this.ctx.events.mediaChanged(fid, '*', 'remove');
    else if (changed) this.ctx.events.mediaChanged(fid, '*', 'change');
    if (added || removed || changed) this.ctx.scanner.onGroupsChanged?.();
    this.ctx.events.emit({ type: 'folders' });
  }

  /** the folder row was deleted (DELETE /api/folders/:id) */
  forget(folderId: number) {
    this.trees.delete(folderId);
    this.failures.delete(folderId);
  }

  /**
   * startup: the scanner's scanAll() re-lists every remote folder (Scanner.remoteScan); a folder whose
   * last state was 'listing' (server stopped mid-listing) shows its old rows until then
   */
  start() {
    this.kick();
  }

  /**
   * the MEGA account session changed (login / logout / expired, remote/account.ts): transfers now count
   * against another quota → lift the quota pause and re-list folders that stopped on it. A quota that is
   * still exceeded shows again on the next transfer (noteError).
   */
  resumeTransfers() {
    if (this.closed) return;
    this.quotaUntil = 0;
    this.backoffUntil = 0;
    this.ctx.db.setSetting('remoteQuota', {});
    for (const f of this.folders()) {
      if (!this.listing.has(f.id) && this.storedStatus(f).state === 'quota') void this.refresh(f.id).catch(() => {});
    }
    this.ctx.events.emit({ type: 'folders', status: true });
    this.kick();
  }

  private tick() {
    if (this.closed) return;
    const now = Date.now();
    if (this.quotaUntil && this.quotaUntil <= now) {
      this.quotaUntil = 0;
      this.ctx.db.setSetting('remoteQuota', {});
      this.ctx.events.emit({ type: 'folders', status: true });
      this.kick();
    }
    for (const f of this.folders()) {
      if (this.listing.has(f.id)) continue;
      const st = this.storedStatus(f);
      const due = st.retryAt !== null && st.retryAt <= now && st.state !== 'ok';
      const stale = st.state === 'ok' && (st.listedAt ?? 0) + this.refreshMs <= now;
      if (due || stale) void this.refresh(f.id).catch(() => {});
    }
  }

  // ───────────────────────── nodes / transfers ─────────────────────────

  private lanesOf(folder: FolderRow): Lanes {
    const id = this.spec(folder).linkId;
    let l = this.lanes.get(id);
    if (!l) this.lanes.set(id, (l = new Lanes(4, 2)));
    return l;
  }

  laneStats(folder: FolderRow) {
    return this.lanesOf(folder).stats;
  }

  async nodeOf(folder: FolderRow, r: Pick<MediaRow, 'dir' | 'name'>): Promise<MegaNode> {
    const t = await this.tree(folder);
    const n = t.files.get(relOfRow(r));
    if (!n) throw new HttpError(404, 'File is no longer in the MEGA link');
    return n;
  }

  private checkQuota() {
    if (this.quotaUntil > Date.now()) {
      const s = Math.ceil((this.quotaUntil - Date.now()) / 1000);
      const err = new HttpError(429, `MEGA transfer quota exceeded, retry at ${new Date(this.quotaUntil).toISOString()}`) as HttpError & { retryAfter?: number };
      err.retryAfter = s;
      throw err;
    }
  }

  /** a failed transfer: quota → stop every transfer until it resets; congestion → short back-off */
  noteError(e: unknown) {
    const msg = String((e as Error)?.message ?? e);
    if (/ERATELIMIT|EAGAIN|\(-3\)|\(-4\)/.test(msg)) {
      this.backoffUntil = Date.now() + 10_000;
      return;
    }
    const err = megaError(e) as HttpError & { retryAfter?: number };
    if (err.statusCode !== 429) return;
    const wait = err.retryAfter && err.retryAfter > 0 ? err.retryAfter : DEFAULT_QUOTA_WAIT_S;
    const until = Date.now() + wait * 1000;
    if (until <= this.quotaUntil) return;
    this.quotaUntil = until;
    this.ctx.db.setSetting('remoteQuota', { until });
    this.ctx.events.emit({ type: 'folders', status: true });
  }

  /** a transfer's signal that also fires on shutdown */
  private signalOf(signal?: AbortSignal): AbortSignal {
    return signal ? AbortSignal.any([signal, this.shutdown.signal]) : this.shutdown.signal;
  }

  private async slot(folder: FolderRow, prio: Priority, signal?: AbortSignal): Promise<() => void> {
    this.checkQuota();
    if (this.backoffUntil > Date.now()) await sleep(this.backoffUntil - Date.now());
    this.checkQuota();
    return this.lanesOf(folder).acquire(prio, this.signalOf(signal));
  }

  /** run one bounded transfer (attribute fetch, small read) in a slot */
  private async transfer<T>(folder: FolderRow, prio: Priority, fn: (signal: AbortSignal) => Promise<T>, signal?: AbortSignal): Promise<T> {
    const release = await this.slot(folder, prio, signal);
    try {
      return await fn(this.signalOf(signal));
    } catch (e) {
      this.noteError(e);
      throw e;
    } finally {
      release();
    }
  }

  /** decrypted bytes [start, end]; the slot is held until the stream closes */
  async openStream(folder: FolderRow, node: MegaNode, start: number, end: number, signal: AbortSignal, prio: Priority = 'high'): Promise<Readable> {
    const release = await this.slot(folder, prio, signal);
    const sig = this.signalOf(signal);
    let s: Readable;
    try {
      if (sig.aborted) throw abortError();
      s = node.read ? node.read({ start, end, signal: sig }) : sliceStream(node.download({ signal: sig }), start, end);
    } catch (e) {
      release();
      throw e;
    }
    s.once('close', release);
    s.once('end', release);
    s.once('error', (e) => {
      if ((e as Error)?.name !== 'AbortError') this.noteError(e);
      release();
    });
    return s;
  }

  private async readBuffer(folder: FolderRow, node: MegaNode, start: number, end: number, prio: Priority): Promise<Buffer> {
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), 120_000);
    try {
      const s = await this.openStream(folder, node, start, end, ctrl.signal, prio);
      const chunks: Buffer[] = [];
      for await (const c of s) chunks.push(c as Buffer);
      return Buffer.concat(chunks);
    } finally {
      clearTimeout(timer);
    }
  }

  /** the whole original on local disk: from the LRU cache, else downloaded (temp = caller deletes it) */
  async fetchFile(folder: FolderRow, r: MediaRow, prio: Priority): Promise<{ file: string; temp: boolean }> {
    const abs = this.ctx.scanner.abs(folder, r.dir, r.name);
    const key = this.cache.key(abs, r.mtime, r.size);
    const hit = await this.cache.get(key);
    if (hit) return { file: hit, temp: false };
    const cacheable = this.cache.accepts(r.size);
    const cur = cacheable ? this.fetching.get(key) : undefined;
    if (cur) return cur;
    const p = (async () => {
      const node = await this.nodeOf(folder, r);
      const tmp = this.cache.tmpFile(key);
      const ctrl = new AbortController();
      try {
        if (node.size > 0) {
          const s = await this.openStream(folder, node, 0, node.size - 1, ctrl.signal, prio);
          await pipeline(s, fs.createWriteStream(tmp));
        } else await fsp.writeFile(tmp, '');
        const st = await fsp.stat(tmp);
        if (st.size !== node.size) throw new HttpError(502, 'Incomplete download from MEGA');
        if (cacheable) return { file: await this.cache.commit(key, tmp), temp: false };
        return { file: tmp, temp: true };
      } catch (e) {
        ctrl.abort();
        await fsp.rm(tmp, { force: true });
        throw e;
      }
    })();
    if (!cacheable) return p;
    const shared = p.finally(() => this.fetching.delete(key));
    this.fetching.set(key, shared);
    return shared;
  }

  // ───────────────────────── RemoteImageSource (ThumbService) ─────────────────────────

  private async resolve(abs: string): Promise<{ folder: FolderRow; row: MediaRow; node: MegaNode }> {
    const m = /^remote:\/\/mega\/([A-Za-z0-9_-]+)\/(.+)$/.exec(abs);
    if (!m) throw new HttpError(400, 'Not a remote path');
    const base = `${REMOTE_SCHEME}mega/${m[1]}`;
    const rel = normalizeRel(m[2]);
    const i = rel.lastIndexOf('/');
    const dir = i < 0 ? '' : rel.slice(0, i);
    const name = i < 0 ? rel : rel.slice(i + 1);
    const row = this.ctx.db.get<MediaRow & { fpath: string }>(
      'SELECT m.* FROM media m JOIN folders f ON f.id = m.folder_id WHERE f.path = ? AND m.dir = ? AND m.name = ? LIMIT 1',
      base,
      dir,
      name,
    );
    if (!row) throw new HttpError(404, 'Media not found');
    const folder = this.ctx.lib.folderRow(row.folder_id);
    return { folder, row, node: await this.nodeOf(folder, row) };
  }

  async thumb(abs: string, _size: 320 | 640 | 1600, prio: Priority): Promise<Buffer> {
    const { folder, row, node } = await this.resolve(abs);
    const signal = AbortSignal.timeout(90_000);
    const types = new Set(parseFa(node.fa).map((t) => t.type));
    const attr = async (type: 0 | 1) => {
      if (!node.attr || !types.has(type)) return null;
      try {
        const b = await this.transfer(folder, prio, (sig) => node.attr!(type, sig), signal);
        return b && b.length ? b : null;
      } catch (e) {
        if (megaError(e).statusCode === 429) throw e;
        return null;
      }
    };
    // 1. MEGA preview (~1000 px, aspect kept)
    const preview = await attr(1);
    if (preview) {
      this.squares.delete(abs);
      return preview;
    }
    // 2. MEGA thumbnail (120 px square)
    const square = await attr(0);
    if (square) {
      this.squares.add(abs);
      return square;
    }
    // 3. the original (small images only)
    if (row.kind === 'image' && row.size <= FULL_THUMB_MAX) {
      const f = await this.fetchFile(folder, row, prio);
      try {
        const src = await this.ctx.thumbs.source(f.file, row, 1600);
        if (!src) throw new HttpError(415, 'Cannot decode');
        this.squares.delete(abs);
        return await src.resize(1600, 1600, { fit: 'inside', withoutEnlargement: true }).jpeg({ quality: 88 }).toBuffer();
      } finally {
        if (f.temp) await fsp.rm(f.file, { force: true });
      }
    }
    // 4. a video frame, read with seeks through the internal loopback URL
    if (row.kind === 'video' && this.ctx.ffmpeg.available && this.ctx.devices.selfUrl) {
      this.checkQuota();
      const at = row.duration ? Math.min(row.duration * 0.1, 3) : 1;
      const frame = await this.ctx.ffmpeg
        .run(['-ss', at.toFixed(3), ...this.ffmpegInput(row), '-frames:v', '1', '-vf', "scale='min(1600,iw)':-2", '-f', 'image2pipe', '-vcodec', 'png', 'pipe:1'], {
          timeoutMs: 90_000,
        })
        .catch(() => Buffer.alloc(0));
      if (frame.length) {
        this.squares.delete(abs);
        return frame;
      }
    }
    throw new HttpError(404, 'No thumbnail');
  }

  async display(abs: string, max: number): Promise<Buffer> {
    const { folder, row } = await this.resolve(abs);
    if (row.kind !== 'image') throw new HttpError(400, 'Not an image');
    const f = await this.fetchFile(folder, row, 'high');
    try {
      const src = await this.ctx.thumbs.source(f.file, row, max);
      if (!src) throw new HttpError(415, 'Cannot convert');
      return await src
        .resize(max, max, { fit: 'inside', withoutEnlargement: true })
        .flatten({ background: '#ffffff' })
        .jpeg({ quality: 88, mozjpeg: true })
        .toBuffer();
    } finally {
      if (f.temp) await fsp.rm(f.file, { force: true });
    }
  }

  async strip(): Promise<Buffer> {
    // the video editor (only user of filmstrips) cannot save into a remote folder
    throw new HttpError(404, 'No filmstrip for MEGA videos');
  }

  /** true when the thumbnail of `abs` came from the 120px square (dimensions must not be derived from it) */
  isSquareThumb(abs: string): boolean {
    return this.squares.has(abs);
  }

  /** ffmpeg input args reading a remote file through our own ranged /file endpoint (seekable) */
  ffmpegInput(r: Pick<MediaRow, 'id'>): string[] {
    if (!this.ctx.devices.selfUrl) throw new HttpError(501, 'Transcoding MEGA videos is not available');
    return ['-headers', `X-Peek-Internal: ${this.ctx.devices.internalToken}\r\n`, '-i', `${this.ctx.devices.selfUrl}/api/media/${r.id}/file`];
  }

  // ───────────────────────── HTTP ─────────────────────────

  /** GET/HEAD /api/media/:id/file for remote media: Range → ranged MEGA read (or the LRU cache) */
  async serveFile(r: MediaRow, folder: FolderRow, req: FastifyRequest, reply: FastifyReply, download: boolean) {
    const total = r.size;
    const common = () => {
      reply.header('Accept-Ranges', 'bytes');
      reply.header('Cache-Control', 'private, max-age=3600');
      reply.header('Content-Type', mimeOf(r.ext));
      if (r.mtime > 0) reply.header('Last-Modified', new Date(r.mtime).toUTCString());
      if (download) reply.header('Content-Disposition', contentDisposition(r.name));
    };
    if (req.method === 'HEAD') {
      common();
      reply.header('Content-Length', String(total));
      reply.hijack();
      reply.raw.writeHead(200, reply.getHeaders() as Record<string, string>);
      reply.raw.end();
      return;
    }
    const range = parseRange(singleRange(req.headers.range), total);
    if (range === 'invalid') {
      reply.code(416).header('Content-Range', `bytes */${total}`);
      return reply.send();
    }
    const [start, end] = range ?? [0, total - 1];
    // the source first: errors (quota, offline, gone) must still answer with JSON + a status
    let body: Readable | string = '';
    if (total) {
      const abs = this.ctx.scanner.abs(folder, r.dir, r.name);
      let cached = await this.cache.get(this.cache.key(abs, r.mtime, r.size));
      // whole images: fetch once into the cache, later views (zoom, info, re-open) are local
      if (!cached && !range && r.kind === 'image' && this.cache.accepts(total)) cached = (await this.fetchFile(folder, r, 'high')).file;
      if (cached) body = fs.createReadStream(cached, { start, end });
      else {
        try {
          const node = await this.nodeOf(folder, r);
          body = await firstBytes(await this.openStream(folder, node, start, end, clientGone(reply), 'high'));
        } catch (e) {
          this.noteError(e);
          const err = megaError(e) as HttpError & { retryAfter?: number };
          if (err.retryAfter) reply.header('Retry-After', String(err.retryAfter));
          throw err;
        }
      }
    }
    common();
    if (range) {
      reply.code(206);
      reply.header('Content-Range', `bytes ${start}-${end}/${total}`);
    }
    reply.header('Content-Length', String(total ? end - start + 1 : 0));
    return reply.send(body);
  }

  /** viewer info: the stored row (enriched in the background); no EXIF read on demand */
  details(r: MediaRow, folder: FolderRow): MediaDetails {
    const extra = r.extra ? JSON.parse(r.extra) : null;
    return {
      ...toItem(r),
      path: [folder.name, relOfRow(r)].filter(Boolean).join('/'),
      exif: null,
      video:
        r.kind === 'video' && extra
          ? { codec: extra.codec ?? null, fps: extra.fps ?? null, bitrate: extra.bitrate ?? null, audioCodec: extra.audioCodec ?? null, rotation: extra.rotation ?? 0 }
          : undefined,
      gps: r.lat != null && r.lon != null ? { lat: r.lat, lon: r.lon } : null,
      place: this.ctx.geo.placeInfo(r.city, r.cc, r.admin1),
      camera: r.make || r.model || r.lens ? { make: r.make, model: r.model, lens: r.lens } : null,
      tzOffset: r.tz_off ?? null,
    };
  }

  /** zip entry: opened when the zip writer reaches it */
  zipEntry(folder: FolderRow, r: MediaRow): Readable {
    return lazyStream(async (signal) => {
      if (!r.size) return Readable.from([]);
      const node = await this.nodeOf(folder, r);
      return this.openStream(folder, node, 0, node.size - 1, signal, 'high');
    });
  }

  // ───────────────────────── copy to this computer ─────────────────────────

  /** copy remote media / dirs into a local folder (the MEGA import job, nothing re-listed) */
  async copy(spec: RemoteCopySpec): Promise<Job> {
    const refs: FileRef[] = [...(spec.refs ?? [])];
    for (const id of spec.ids ?? []) {
      const r = this.ctx.db.get<MediaRow>('SELECT * FROM media WHERE id = ?', Number(id));
      if (!r) throw new HttpError(404, 'Media not found');
      refs.push({ folderId: String(r.folder_id), rel: relOfRow(r) });
    }
    if (!refs.length) throw new HttpError(400, 'Nothing selected');
    const folderIds = new Set(refs.map((r) => String(r.folderId)));
    if (folderIds.size !== 1) throw new HttpError(400, 'Copy from one MEGA link at a time');
    const folder = this.ctx.lib.folderRow([...folderIds][0]);
    const spec2 = this.spec(folder);
    const tree = await this.tree(folder);
    const nodes: string[] = [];
    for (const ref of refs) {
      const rel = normalizeRel(ref.rel);
      const n = tree.files.get(rel) ?? tree.dirs.get(rel);
      if (!n) throw new HttpError(404, `Not in the MEGA link: ${rel}`);
      nodes.push(n.id);
    }
    this.checkQuota();
    return startRemoteImport(
      this.ctx,
      { url: spec2.url, mode: 'extract', folderId: spec.to.folderId, dir: spec.to.dir ?? '', nodes, zip: 'keep' },
      { root: async () => tree.root, relative: true, title: folder.name, onError: (e) => this.noteError(e) },
    );
  }

  // ───────────────────────── background: metadata + thumbnails ─────────────────────────

  kick() {
    if (this.closed) return;
    if (!this.metaRunning) this.track(this.metaLoop());
    if (!this.thumbRunning) this.track(this.thumbLoop());
  }

  private track(p: Promise<void>) {
    const t: Promise<void> = p.catch(() => {}).finally(() => this.loops.delete(t));
    this.loops.add(t);
  }

  private blocked(): boolean {
    return this.closed || this.quotaUntil > Date.now();
  }

  private async metaLoop() {
    this.metaRunning = true;
    const skip = new Set<number>();
    try {
      for (;;) {
        if (this.blocked()) break;
        const rows = this.ctx.db
          .all<MediaRow>(
            `SELECT m.* FROM media m JOIN folders f ON f.id = m.folder_id
             WHERE f.remote IS NOT NULL AND m.kind IN ('image','video')
               AND (m.meta_state = ${META_REMOTE_PENDING} OR (m.meta_state = 1 AND m.meta_v < ?))
             ORDER BY m.taken_local DESC LIMIT 24`,
            META_V,
          )
          .filter((r) => !skip.has(r.id));
        if (!rows.length) break;
        const touched = new Map<number, Set<string>>();
        let stop = false;
        for (const r of rows) {
          if (this.blocked()) {
            stop = true;
            break;
          }
          const res = await this.enrichOne(r).catch((e) => {
            const code = megaError(e).statusCode;
            return code === 429 || code === 502 || code === 503 ? 'stop' : 'failed';
          });
          if (res === 'stop') {
            stop = true;
            skip.add(r.id);
            break;
          }
          if (res === 'failed') {
            // keep the listing data; never retried until the file changes
            this.ctx.db.run('UPDATE media SET meta_state = 1, meta_v = ? WHERE id = ?', META_V, r.id);
          }
          let set = touched.get(r.folder_id);
          if (!set) touched.set(r.folder_id, (set = new Set()));
          set.add(r.dir);
          if (this.metaGapMs) await sleep(this.metaGapMs);
        }
        for (const [fid, dirs] of touched) for (const d of dirs) this.ctx.events.mediaChanged(String(fid), d, 'meta');
        if (touched.size) {
          this.ctx.scanner.onGroupsChanged?.();
          this.progressChanged();
        }
        if (stop) break;
      }
    } finally {
      this.metaRunning = false;
    }
  }

  /** read what the metadata needs from MEGA (head of images, probe of videos) and update the row */
  private async enrichOne(r: MediaRow): Promise<'ok'> {
    const folder = this.ctx.lib.folderRow(r.folder_id);
    const m = emptyMeta(r.playback);
    if (r.kind === 'image' && r.size > 0) {
      const node = await this.nodeOf(folder, r);
      const head = await this.readBuffer(folder, node, 0, Math.min(r.size, HEAD_BYTES) - 1, 'low');
      if (r.ext !== 'heic' && r.ext !== 'heif') {
        try {
          const s = await sharp(head, { failOn: 'none' }).metadata();
          m.width = s.width ?? null;
          m.height = s.height ?? null;
          if (s.pages && s.pages > 1 && s.pageHeight) m.height = s.pageHeight;
          m.pages = s.pages ?? null;
          if (s.orientation && s.orientation >= 5 && m.width && m.height) [m.width, m.height] = [m.height, m.width];
        } catch {
          /* header not in the first 256 KB */
        }
      }
      if (!['svg', 'gif', 'bmp', 'ico'].includes(r.ext)) {
        try {
          const ex = await exifr.parse(head, EXIF_OPTS as never);
          if (ex) applyExif(m, ex);
        } catch {
          /* no / truncated EXIF */
        }
      }
    } else if (r.kind === 'video' && this.ctx.ffmpeg.available && this.ctx.devices.selfUrl) {
      this.checkQuota();
      const p = await this.ctx.ffmpeg.probe(`${this.ctx.devices.selfUrl}/api/media/${r.id}/file`, {
        headers: `X-Peek-Internal: ${this.ctx.devices.internalToken}\r\n`,
      });
      if (p) {
        Object.assign(m, videoGroupMeta(p));
        m.width = p.width;
        m.height = p.height;
        m.duration = p.duration;
        m.playback = isBrowserPlayable(r.ext, p.codec) ? 'direct' : 'transcode';
        m.extra = { codec: p.codec, audioCodec: p.audioCodec, fps: p.fps, bitrate: p.bitrate, rotation: p.rotation, hasAudio: p.hasAudio };
      } else if (this.quotaUntil > Date.now()) throw new HttpError(429, 'quota');
    }
    if (this.closed) return 'ok';
    const g = groupColumns(r, m, this.ctx.geo, this.ctx.scanner.homeTimeZone);
    this.ctx.db.run(
      `UPDATE media SET width = COALESCE(?, width), height = COALESCE(?, height), duration = COALESCE(?, duration), playback = ?,
         extra = COALESCE(?, extra), meta_state = 1, meta_v = ?, ${GROUP_SET_SQL} WHERE id = ?`,
      m.width,
      m.height,
      m.duration,
      m.playback,
      m.extra ? JSON.stringify(m.extra) : null,
      META_V,
      ...groupParams(g),
      r.id,
    );
    return 'ok';
  }

  private async thumbLoop() {
    this.thumbRunning = true;
    const failed = new Set<number>();
    try {
      for (;;) {
        if (this.blocked()) break;
        const rows = this.ctx.db
          .all<MediaRow & { fpath: string }>(
            `SELECT m.*, f.path fpath FROM media m JOIN folders f ON f.id = m.folder_id
             WHERE f.remote IS NOT NULL AND m.thumb_state = 0 AND m.kind IN ('image','video') ORDER BY m.taken_local DESC LIMIT 16`,
          )
          .filter((r) => !failed.has(r.id));
        if (!rows.length) break;
        let stop = false;
        await pool(rows, 2, async (r) => {
          if (stop || this.blocked()) return;
          const abs = `${r.fpath}/${r.dir ? `${r.dir}/` : ''}${r.name}`;
          try {
            const res = await this.ctx.thumbs.generateBase(abs, r, 'low');
            if (this.closed) return;
            const dims = !!res && r.kind === 'image' && (!r.width || !r.height) && !this.squares.has(abs);
            this.ctx.db.run(
              'UPDATE media SET thumbhash = ?, thumb_state = ?, width = COALESCE(width, ?), height = COALESCE(height, ?) WHERE id = ?',
              res?.thumbhash ?? null,
              res ? 1 : 2,
              dims ? res!.width : null,
              dims ? res!.height : null,
              r.id,
            );
            this.ctx.events.mediaChanged(String(r.folder_id), r.dir, dims ? 'meta' : 'thumb');
          } catch (e) {
            const code = megaError(e).statusCode;
            if (this.closed) return;
            if (code === 429 || code === 503 || (code === 502 && /reach/i.test(megaError(e).message))) {
              stop = true;
              return;
            }
            failed.add(r.id);
            this.ctx.db.run('UPDATE media SET thumb_state = 2 WHERE id = ?', r.id);
            this.ctx.events.mediaChanged(String(r.folder_id), r.dir, 'thumb');
          }
        });
        this.progressChanged();
        if (stop) break;
      }
    } finally {
      this.thumbRunning = false;
    }
  }

  private progressTimer: NodeJS.Timeout | null = null;

  /** indexing progress (FolderRef.remote.indexing) changed: one 'folders' event per 2 s at most */
  private progressChanged() {
    if (this.progressTimer || this.closed) return;
    this.progressTimer = setTimeout(() => {
      this.progressTimer = null;
      if (!this.closed) this.ctx.events.emit({ type: 'folders', status: true });
    }, 2000);
    this.progressTimer.unref();
  }

  async close() {
    this.closed = true;
    this.shutdown.abort();
    clearInterval(this.timer);
    if (this.progressTimer) clearTimeout(this.progressTimer);
    await Promise.allSettled([...this.loops, ...this.listing.values()]);
  }
}

/**
 * Wait for the first chunk (or the end) of a stream — rejects with its error, so a failing MEGA read
 * still becomes a status code instead of a cut-off 200/206.
 */
function firstBytes(s: Readable): Promise<Readable> {
  return new Promise((resolve, reject) => {
    const out = new PassThrough();
    const cleanup = () => {
      s.off('data', onData);
      s.off('end', onEnd);
      s.off('error', onError);
    };
    const onData = (c: Buffer) => {
      cleanup();
      s.pause();
      out.write(c);
      s.on('error', (e) => out.destroy(e));
      out.on('close', () => s.destroy());
      s.pipe(out);
      resolve(out);
    };
    const onEnd = () => {
      cleanup();
      out.end();
      resolve(out);
    };
    const onError = (e: unknown) => {
      cleanup();
      reject(e ?? new Error('stream failed'));
    };
    s.on('data', onData);
    s.once('end', onEnd);
    s.once('error', onError);
  });
}

/** "bytes=a-b" (already validated by singleRange) → [start, end] | null (whole file) | 'invalid' */
export function parseRange(h: string | undefined, total: number): [number, number] | null | 'invalid' {
  if (!h) return null;
  const m = /^bytes=(\d*)-(\d*)$/.exec(h);
  if (!m) return null;
  let start: number;
  let end: number;
  if (m[1] === '') {
    const n = Number(m[2]);
    if (!n) return 'invalid';
    start = Math.max(0, total - n);
    end = total - 1;
  } else {
    start = Number(m[1]);
    end = m[2] === '' ? total - 1 : Math.min(Number(m[2]), total - 1);
  }
  if (start >= total || start > end) return 'invalid';
  return [start, end];
}

