import fs from 'node:fs';
import fsp from 'node:fs/promises';
import path from 'node:path';
import { normalizeLibraryOptions, type FolderRef, type Library, type LibraryOptions, type LibraryStats, type RemoteFolderInfo } from '@peekaround/shared';
import type { Db, Row } from './db.js';
import type { EventHub } from './events.js';
import { HttpError, isInside } from './paths.js';

const PALETTE = ['#8b5cf6', '#ec4899', '#f97316', '#10b981', '#06b6d4', '#3b82f6', '#eab308', '#ef4444'];
const ICONS = ['images', 'camera', 'film', 'heart', 'mountain', 'plane', 'palette', 'star'];

export interface FolderRow {
  id: number;
  library_id: number | null;
  path: string;
  name: string;
  temp: number;
  expires_at: number | null;
  source_name: string | null;
  sort: number;
  /** linked phone folders (migration 002): path = device://<device_id>/<device_key> */
  device_id: string | null;
  device_key: string | null;
  /** remote (streaming) folders (migration 003): JSON {kind:'mega', url, linkId}; path = remote://mega/<linkId> */
  remote: string | null;
  remote_status: string | null;
  /** last completed scan (migration 005) */
  scanned_at?: number | null;
}

export const DEVICE_SCHEME = 'device://';
export const REMOTE_SCHEME = 'remote://';

export function isDeviceFolder(r: Pick<FolderRow, 'device_id'> | null | undefined): boolean {
  return !!r?.device_id;
}

/** a MEGA link shown without downloading (read + copy only) */
export function isRemoteFolder(r: Pick<FolderRow, 'remote'> | null | undefined): boolean {
  return !!r?.remote;
}

/** where a folder's bytes live */
export function folderSource(r: Pick<FolderRow, 'device_id' | 'remote'>): 'local' | 'device' | 'remote' {
  return r.device_id ? 'device' : r.remote ? 'remote' : 'local';
}

export class LibraryService {
  constructor(
    private db: Db,
    private events: EventHub,
  ) {}

  /** set by the DeviceHub: name + online state of a linked phone */
  deviceLookup: ((deviceId: string) => { id: string; name: string; online: boolean }) | null = null;
  /** set by the RemoteService: status of a remote (MEGA) folder — never includes the link key */
  remoteLookup: ((r: FolderRow) => RemoteFolderInfo) | null = null;

  folderRow(id: string | number): FolderRow {
    const r = this.db.get<FolderRow>('SELECT * FROM folders WHERE id = ?', Number(id));
    if (!r) throw new HttpError(404, 'Folder not found');
    return r;
  }

  allFolderRows(): FolderRow[] {
    return this.db.all<FolderRow>('SELECT * FROM folders ORDER BY sort, id');
  }

  folderRef(r: FolderRow, counts?: Map<number, number>): FolderRef {
    return {
      id: String(r.id),
      path: r.path,
      name: r.name,
      libraryId: r.library_id == null ? null : String(r.library_id),
      temp: !!r.temp,
      expiresAt: r.expires_at,
      sourceName: r.source_name,
      mediaCount: counts?.get(r.id) ?? this.countFolder(r.id),
      device: r.device_id ? (this.deviceLookup?.(r.device_id) ?? { id: r.device_id, name: r.name, online: false }) : null,
      remote: r.remote ? (this.remoteLookup?.(r) ?? null) : null,
    };
  }

  private countFolder(id: number): number {
    return this.db.get<{ c: number }>("SELECT count(*) c FROM media WHERE folder_id = ? AND kind IN ('image','video','audio')", id)?.c ?? 0;
  }

  counts(): Map<number, number> {
    const m = new Map<number, number>();
    for (const r of this.db.all<{ folder_id: number; c: number }>(
      "SELECT folder_id, count(*) c FROM media WHERE kind IN ('image','video','audio') GROUP BY folder_id",
    ))
      m.set(r.folder_id, r.c);
    return m;
  }

  listLibraries(): Library[] {
    const counts = this.counts();
    const libs = this.db.all<Row>('SELECT * FROM libraries ORDER BY sort, id');
    const folders = this.allFolderRows();
    return libs.map((l) => {
      const fs = folders.filter((f) => f.library_id === l.id).map((f) => this.folderRef(f, counts));
      return {
        id: String(l.id),
        name: l.name,
        color: l.color,
        icon: l.icon,
        sort: l.sort,
        folders: fs,
        mediaCount: fs.reduce((a, f) => a + f.mediaCount, 0),
        coverId: l.cover_media_id == null ? null : String(l.cover_media_id),
        options: parseOptions(l.options),
      };
    });
  }

  /** scan options of the library a folder belongs to (defaults for temp / unknown) */
  optionsOfFolder(folderId: number): LibraryOptions {
    const r = this.db.get<{ options: string | null }>('SELECT l.options FROM folders f JOIN libraries l ON l.id = f.library_id WHERE f.id = ?', folderId);
    return parseOptions(r?.options ?? null);
  }

  /** new order of the libraries (ids first → last; others keep their relative order after them) */
  reorderLibraries(ids: string[]) {
    const all = this.db.all<{ id: number }>('SELECT id FROM libraries ORDER BY sort, id').map((r) => r.id);
    const want = ids.map(Number).filter((id) => all.includes(id));
    const rest = all.filter((id) => !want.includes(id));
    this.db.tx(() => [...want, ...rest].forEach((id, i) => this.db.run('UPDATE libraries SET sort = ? WHERE id = ?', i, id)));
    this.events.emit({ type: 'folders' });
  }

  /** header numbers of a library page */
  stats(id: string, busy: (folderIds: number[]) => boolean): LibraryStats {
    const lib = this.library(id);
    const ids = lib.folders.map((f) => Number(f.id));
    const empty: LibraryStats = { items: 0, photos: 0, videos: 0, audio: 0, bytes: 0, from: null, to: null, folders: 0, scannedAt: null, scanning: false, pending: 0 };
    if (!ids.length) return empty;
    const inList = ids.map(() => '?').join(',');
    const r = this.db.get<{ n: number; p: number; v: number; a: number; b: number | null; f: number | null; t: number | null; d: number; q: number }>(
      `SELECT count(*) n, sum(kind = 'image') p, sum(kind = 'video') v, sum(kind = 'audio') a, sum(size) b,
              min(taken_local) f, max(taken_local) t, count(DISTINCT folder_id || '/' || dir) d,
              sum(meta_state = 0 OR (thumb_state = 0 AND kind IN ('image','video'))) q
       FROM media WHERE folder_id IN (${inList}) AND kind IN ('image','video','audio')`,
      ...ids,
    );
    const scanned = this.db.get<{ s: number | null }>(`SELECT max(scanned_at) s FROM folders WHERE id IN (${inList})`, ...ids)?.s ?? null;
    return {
      items: r?.n ?? 0,
      photos: r?.p ?? 0,
      videos: r?.v ?? 0,
      audio: r?.a ?? 0,
      bytes: r?.b ?? 0,
      from: r?.n ? (r.f ?? null) : null,
      to: r?.n ? (r.t ?? null) : null,
      folders: r?.d ?? 0,
      scannedAt: scanned,
      scanning: busy(ids),
      pending: r?.q ?? 0,
    };
  }

  listTemp(): FolderRef[] {
    const counts = this.counts();
    return this.db
      .all<FolderRow>('SELECT * FROM folders WHERE temp = 1 ORDER BY id DESC')
      .map((f) => this.folderRef(f, counts));
  }

  library(id: string | number): Library {
    const lib = this.listLibraries().find((l) => l.id === String(id));
    if (!lib) throw new HttpError(404, 'Library not found');
    return lib;
  }

  createLibrary(input: { name: string; color?: string; icon?: string; folders?: string[] }): Library {
    const n = this.db.get<{ c: number }>('SELECT count(*) c FROM libraries')?.c ?? 0;
    const res = this.db.run(
      'INSERT INTO libraries (name, color, icon, sort, created_at) VALUES (?, ?, ?, ?, ?)',
      input.name.trim() || 'Library',
      input.color ?? PALETTE[n % PALETTE.length],
      input.icon ?? ICONS[n % ICONS.length],
      n,
      Date.now(),
    );
    const id = Number(res.lastInsertRowid);
    for (const p of input.folders ?? []) this.addFolder(id, p);
    this.events.emit({ type: 'folders' });
    return this.library(id);
  }

  /**
   * coverId: a media id of this library (null = automatic). options: scan options — returns
   * `rescan: true` when they changed in a way that needs a rescan (exclusions / hidden / size).
   */
  updateLibrary(
    id: string,
    patch: { name?: string; color?: string; icon?: string; sort?: number; coverId?: string | null; options?: Partial<LibraryOptions> },
  ): Library & { rescan?: boolean } {
    const lib = this.db.get<Row>('SELECT * FROM libraries WHERE id = ?', Number(id));
    if (!lib) throw new HttpError(404, 'Library not found');
    let cover: number | null = lib.cover_media_id ?? null;
    if (patch.coverId !== undefined) {
      if (patch.coverId === null || patch.coverId === '') cover = null;
      else {
        const m = this.db.get<{ id: number }>(
          'SELECT m.id FROM media m JOIN folders f ON f.id = m.folder_id WHERE m.id = ? AND f.library_id = ?',
          Number(patch.coverId),
          Number(id),
        );
        if (!m) throw new HttpError(400, 'The cover must be an item of this library');
        cover = m.id;
      }
    }
    const before = parseOptions(lib.options);
    const options = patch.options !== undefined ? normalizeLibraryOptions({ ...before, ...patch.options }) : before;
    this.db.run(
      'UPDATE libraries SET name = ?, color = ?, icon = ?, sort = ?, cover_media_id = ?, options = ? WHERE id = ?',
      patch.name?.trim() || lib.name,
      patch.color ?? lib.color,
      patch.icon ?? lib.icon,
      patch.sort ?? lib.sort,
      cover,
      JSON.stringify(options),
      Number(id),
    );
    this.events.emit({ type: 'folders' });
    const rescan = JSON.stringify(before) !== JSON.stringify(options);
    return rescan ? { ...this.library(id), rescan } : this.library(id);
  }

  deleteLibrary(id: string) {
    this.db.run('DELETE FROM libraries WHERE id = ?', Number(id));
    this.events.emit({ type: 'folders' });
  }

  addFolder(libraryId: number | string, p: string, name?: string): FolderRow {
    const abs = path.resolve(p);
    let st: fs.Stats;
    try {
      st = fs.statSync(abs);
    } catch {
      throw new HttpError(400, `Folder does not exist: ${abs}`);
    }
    if (!st.isDirectory()) throw new HttpError(400, 'Not a directory');
    const existing = this.db.get<FolderRow>('SELECT * FROM folders WHERE library_id = ? AND path = ?', Number(libraryId), abs);
    if (existing) return existing;
    const n = this.db.get<{ c: number }>('SELECT count(*) c FROM folders WHERE library_id = ?', Number(libraryId))?.c ?? 0;
    const res = this.db.run(
      'INSERT INTO folders (library_id, path, name, temp, sort) VALUES (?, ?, ?, 0, ?)',
      Number(libraryId),
      abs,
      name?.trim() || path.basename(abs) || abs,
      n,
    );
    this.events.emit({ type: 'folders' });
    return this.folderRow(Number(res.lastInsertRowid));
  }

  /** a remote (streaming) folder row; `remote` JSON holds the link (with key) — server side only */
  addRemoteFolder(libraryId: number | string, input: { path: string; name: string; remote: object; status: object }): FolderRow {
    const existing = this.db.get<FolderRow>('SELECT * FROM folders WHERE library_id = ? AND path = ?', Number(libraryId), input.path);
    if (existing) return existing;
    const n = this.db.get<{ c: number }>('SELECT count(*) c FROM folders WHERE library_id = ?', Number(libraryId))?.c ?? 0;
    const res = this.db.run(
      'INSERT INTO folders (library_id, path, name, temp, sort, remote, remote_status) VALUES (?, ?, ?, 0, ?, ?, ?)',
      Number(libraryId),
      input.path,
      input.name.trim() || 'MEGA',
      n,
      JSON.stringify(input.remote),
      JSON.stringify(input.status),
    );
    this.events.emit({ type: 'folders' });
    return this.folderRow(Number(res.lastInsertRowid));
  }

  removeFolder(id: string) {
    this.db.run('DELETE FROM folders WHERE id = ?', Number(id));
    this.events.emit({ type: 'folders' });
  }

  renameFolder(id: string, name: string) {
    this.db.run('UPDATE folders SET name = ? WHERE id = ?', name.trim(), Number(id));
    this.events.emit({ type: 'folders' });
  }

  createTempFolder(abs: string, sourceName: string, hours: number): FolderRow {
    const res = this.db.run(
      'INSERT INTO folders (library_id, path, name, temp, expires_at, source_name) VALUES (NULL, ?, ?, 1, ?, ?)',
      abs,
      sourceName.replace(/\.zip$/i, ''),
      Date.now() + hours * 3600_000,
      sourceName,
    );
    this.events.emit({ type: 'folders' });
    return this.folderRow(Number(res.lastInsertRowid));
  }

  /** Remove expired temp sessions (and their files, which always live in our temp dir). */
  async cleanupTemp(tempDir: string, all = false) {
    const rows = this.db.all<FolderRow>(
      all ? 'SELECT * FROM folders WHERE temp = 1' : 'SELECT * FROM folders WHERE temp = 1 AND expires_at < ?',
      ...(all ? [] : [Date.now()]),
    );
    for (const r of rows) await this.deleteTemp(r, tempDir);
    if (rows.length) this.events.emit({ type: 'folders' });
  }

  async deleteTemp(r: FolderRow, tempDir: string) {
    this.db.run('DELETE FROM folders WHERE id = ?', r.id);
    const sessionDir = path.dirname(r.path);
    if (isInside(tempDir, sessionDir) && sessionDir !== path.resolve(tempDir)) {
      await fsp.rm(sessionDir, { recursive: true, force: true });
    }
  }

  /** On first start create one library per configured root. */
  ensureRoots(roots: { path: string; name?: string; split?: boolean }[]) {
    if (!roots.length) return;
    const have = this.db.get<{ c: number }>('SELECT count(*) c FROM libraries')?.c ?? 0;
    if (have > 0) return;
    for (const r of roots) {
      const abs = path.resolve(r.path);
      if (!fs.existsSync(abs)) continue;
      if (r.split) {
        // e.g. Docker "/media" with several mounted folders → one library per mounted folder.
        const children = fs
          .readdirSync(abs, { withFileTypes: true })
          .filter((d) => d.isDirectory() && !d.name.startsWith('.'))
          .map((d) => d.name)
          .sort((a, b) => a.localeCompare(b));
        if (children.length) {
          for (const c of children) this.createLibrary({ name: c, folders: [path.join(abs, c)] });
          continue;
        }
      }
      this.createLibrary({ name: r.name || path.basename(abs) || abs, folders: [abs] });
    }
  }
}

function parseOptions(raw: string | null | undefined): LibraryOptions {
  if (!raw) return normalizeLibraryOptions(null);
  try {
    return normalizeLibraryOptions(JSON.parse(raw));
  } catch {
    return normalizeLibraryOptions(null);
  }
}
