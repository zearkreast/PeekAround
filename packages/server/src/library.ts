import fs from 'node:fs';
import fsp from 'node:fs/promises';
import path from 'node:path';
import type { FolderRef, Library } from '@peekaround/shared';
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
}

export class LibraryService {
  constructor(
    private db: Db,
    private events: EventHub,
  ) {}

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
    };
  }

  private countFolder(id: number): number {
    return this.db.get<{ c: number }>("SELECT count(*) c FROM media WHERE folder_id = ? AND kind IN ('image','video')", id)?.c ?? 0;
  }

  private counts(): Map<number, number> {
    const m = new Map<number, number>();
    for (const r of this.db.all<{ folder_id: number; c: number }>(
      "SELECT folder_id, count(*) c FROM media WHERE kind IN ('image','video') GROUP BY folder_id",
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
      };
    });
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

  updateLibrary(id: string, patch: { name?: string; color?: string; icon?: string; sort?: number }) {
    const lib = this.db.get<Row>('SELECT * FROM libraries WHERE id = ?', Number(id));
    if (!lib) throw new HttpError(404, 'Library not found');
    this.db.run(
      'UPDATE libraries SET name = ?, color = ?, icon = ?, sort = ? WHERE id = ?',
      patch.name?.trim() || lib.name,
      patch.color ?? lib.color,
      patch.icon ?? lib.icon,
      patch.sort ?? lib.sort,
      Number(id),
    );
    this.events.emit({ type: 'folders' });
    return this.library(id);
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
