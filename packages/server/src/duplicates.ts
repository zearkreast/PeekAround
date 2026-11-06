import crypto from 'node:crypto';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import type { DuplicateGroup, DuplicateSummary } from '@peekaround/shared';
import type { Ctx } from './context.js';

/** files up to this size are hashed completely; bigger ones by size + three 4 MiB samples */
const FULL_HASH_MAX = 256 * 1024 * 1024;
const SAMPLE = 4 * 1024 * 1024;
const BATCH = 24;

/** local (not phone / remote) folders of libraries */
const LOCAL_FOLDERS = "SELECT id FROM folders WHERE temp = 0 AND device_id IS NULL AND path NOT LIKE 'remote://%' AND path NOT LIKE 'device://%'";
/** same-size image/video rows that still need a content hash */
const CANDIDATES = `
  FROM media WHERE chash IS NULL AND size > 0 AND kind IN ('image','video') AND folder_id IN (${LOCAL_FOLDERS})
  AND size IN (SELECT size FROM media INDEXED BY media_size WHERE size > 0 AND kind IN ('image','video') GROUP BY size HAVING count(*) > 1)`;

export async function contentHash(file: string, size: number): Promise<string> {
  const h = crypto.createHash('sha1');
  if (size <= FULL_HASH_MAX) {
    await new Promise<void>((resolve, reject) => {
      const rs = fs.createReadStream(file, { highWaterMark: 1024 * 1024 });
      rs.on('data', (c) => h.update(c));
      rs.on('end', () => resolve());
      rs.on('error', reject);
    });
    return h.digest('hex');
  }
  const fh = await fsp.open(file, 'r');
  try {
    h.update(`size:${size}`);
    const buf = Buffer.allocUnsafe(SAMPLE);
    for (const at of [0, Math.floor(size / 2 - SAMPLE / 2), size - SAMPLE]) {
      const { bytesRead } = await fh.read(buf, 0, SAMPLE, Math.max(0, at));
      h.update(buf.subarray(0, bytesRead));
    }
  } finally {
    await fh.close();
  }
  return `s${h.digest('hex')}`;
}

/**
 * Exact duplicates: only files whose size collides with another file are hashed (lazily, in the
 * background, one at a time) — a 50k library typically hashes a few hundred files.
 */
export class DuplicateService {
  private running = false;
  private stopped = false;
  private loop: Promise<void> | null = null;
  /** called after hashes were written (Explore invalidation) */
  onChanged: (() => void) | null = null;

  constructor(private ctx: Ctx) {}

  pending(): number {
    return this.ctx.db.get<{ c: number }>(`SELECT count(*) c ${CANDIDATES}`)?.c ?? 0;
  }

  get busy(): boolean {
    return this.running;
  }

  /** start hashing candidates if idle */
  kick() {
    if (this.running || this.stopped) return;
    this.running = true;
    this.loop = this.run()
      .catch(() => {})
      .finally(() => {
        this.running = false;
      });
  }

  async stop() {
    this.stopped = true;
    await this.loop?.catch(() => {});
  }

  private async run() {
    let wrote = 0;
    let lastNotify = Date.now();
    for (;;) {
      if (this.stopped) break;
      // the scanner's own passes come first
      while (!this.stopped && this.ctx.scanner.busy) await new Promise((r) => setTimeout(r, 1500));
      const rows = this.ctx.db.all<{ id: number; folder_id: number; dir: string; name: string; size: number }>(
        `SELECT id, folder_id, dir, name, size ${CANDIDATES} ORDER BY size DESC LIMIT ${BATCH}`,
      );
      if (!rows.length) break;
      for (const r of rows) {
        if (this.stopped) break;
        let hash: string;
        try {
          const folder = this.ctx.lib.folderRow(r.folder_id);
          const abs = this.ctx.scanner.abs(folder, r.dir, r.name);
          const st = await fsp.stat(abs);
          hash = st.size === r.size ? await contentHash(abs, st.size) : `x:${r.id}`;
        } catch {
          hash = `x:${r.id}`;
        }
        if (this.stopped) break;
        this.ctx.db.run('UPDATE media SET chash = ? WHERE id = ? AND size = ?', hash, r.id, r.size);
        wrote++;
      }
      if (wrote && Date.now() - lastNotify > 15_000) {
        lastNotify = Date.now();
        wrote = 0;
        this.onChanged?.();
      }
    }
    if (wrote) this.onChanged?.();
  }

  /** summary for the Explore card (whole library, image + video) */
  summary(scopeWhere: string, params: unknown[]): DuplicateSummary {
    const rows = this.ctx.db.all<{ chash: string; n: number; size: number; cover: number }>(
      `SELECT chash, count(*) n, max(size) size, max(id) cover FROM media INDEXED BY media_chash ${scopeWhere ? `${scopeWhere} AND` : 'WHERE'} chash IS NOT NULL AND chash < 'x'
       GROUP BY chash HAVING count(*) > 1 ORDER BY max(size) * (count(*) - 1) DESC`,
      ...params,
    );
    let files = 0;
    let wasted = 0;
    for (const r of rows) {
      files += r.n;
      wasted += r.size * (r.n - 1);
    }
    return { groups: rows.length, files, wasted, pending: this.pending(), coverIds: rows.slice(0, 4).map((r) => String(r.cover)) };
  }

  /** duplicate groups (biggest waste first) of a scope */
  groups(scopeWhere: string, params: unknown[], offset = 0, limit = 100): { groups: DuplicateGroup[]; total: number } {
    const all = this.ctx.db.all<{ chash: string; size: number; ids: string }>(
      `SELECT chash, max(size) size, group_concat(id) ids FROM media INDEXED BY media_chash ${scopeWhere ? `${scopeWhere} AND` : 'WHERE'} chash IS NOT NULL AND chash < 'x'
       GROUP BY chash HAVING count(*) > 1 ORDER BY max(size) * (count(*) - 1) DESC, chash`,
      ...params,
    );
    const page = all.slice(offset, offset + limit).map((r) => ({
      hash: r.chash,
      size: r.size,
      // oldest row (lowest id) first: usually the original
      ids: r.ids
        .split(',')
        .map(Number)
        .sort((a, b) => a - b)
        .map(String),
    }));
    return { groups: page, total: all.length };
  }
}
