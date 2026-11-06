import crypto from 'node:crypto';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import path from 'node:path';

/**
 * Size-capped LRU disk cache for bytes fetched from remote (MEGA) folders: whole originals
 * that were viewed (images, small files) so zooming / re-opening never downloads them again.
 * Entries are keyed like thumbnails (abs|mtime|size); recency = last access, kept in memory
 * (seeded from file mtimes at startup, refreshed with utimes on access).
 */
export class RemoteCache {
  private entries = new Map<string, { size: number; at: number }>();
  private total = 0;
  private ready: Promise<void>;

  constructor(
    readonly dir: string,
    /** cap in bytes (Settings.remoteCacheMb) */
    public capBytes: number,
  ) {
    fs.mkdirSync(dir, { recursive: true });
    this.ready = this.load();
  }

  key(abs: string, mtime: number, size: number): string {
    return crypto.createHash('sha1').update(`${abs}|${mtime}|${size}`).digest('hex');
  }

  file(key: string): string {
    return path.join(this.dir, key.slice(0, 2), key);
  }

  private async load() {
    for (const sub of await fsp.readdir(this.dir).catch(() => [] as string[])) {
      const d = path.join(this.dir, sub);
      for (const name of await fsp.readdir(d).catch(() => [] as string[])) {
        const p = path.join(d, name);
        if (name.endsWith('.tmp')) {
          await fsp.rm(p, { force: true });
          continue;
        }
        const st = await fsp.stat(p).catch(() => null);
        if (!st?.isFile()) continue;
        this.entries.set(name, { size: st.size, at: st.mtimeMs });
        this.total += st.size;
      }
    }
  }

  /** path of a complete cached entry (marks it as recently used), or null */
  async get(key: string): Promise<string | null> {
    await this.ready;
    const e = this.entries.get(key);
    if (!e) return null;
    const f = this.file(key);
    if (!(await fsp.access(f).then(() => true, () => false))) {
      this.entries.delete(key);
      this.total -= e.size;
      return null;
    }
    e.at = Date.now();
    const now = new Date();
    void fsp.utimes(f, now, now).catch(() => {});
    return f;
  }

  /** whether an entry of `size` bytes may be cached at all (≤ ¼ of the cap) */
  accepts(size: number): boolean {
    return this.capBytes > 0 && size <= this.capBytes / 4;
  }

  /** a temp file next to the cache (same file system → atomic rename on commit) */
  tmpFile(key: string): string {
    return path.join(this.dir, `${key}.${process.pid}.${crypto.randomBytes(4).toString('hex')}.tmp`);
  }

  /** move a completely written temp file into the cache and evict old entries over the cap */
  async commit(key: string, tmp: string): Promise<string> {
    await this.ready;
    const st = await fsp.stat(tmp);
    const f = this.file(key);
    await fsp.mkdir(path.dirname(f), { recursive: true });
    await fsp.rename(tmp, f);
    const old = this.entries.get(key);
    if (old) this.total -= old.size;
    this.entries.set(key, { size: st.size, at: Date.now() });
    this.total += st.size;
    await this.evict(key);
    return f;
  }

  async evict(keep?: string) {
    if (this.total <= this.capBytes) return;
    const order = [...this.entries.entries()].sort((a, b) => a[1].at - b[1].at);
    for (const [k, e] of order) {
      if (this.total <= this.capBytes) break;
      if (k === keep) continue;
      this.entries.delete(k);
      this.total -= e.size;
      await fsp.rm(this.file(k), { force: true });
    }
  }

  async setCap(bytes: number) {
    this.capBytes = Math.max(0, bytes);
    await this.ready;
    await this.evict();
  }

  async sizeBytes(): Promise<number> {
    await this.ready;
    return this.total;
  }

  async clear() {
    await this.ready;
    this.entries.clear();
    this.total = 0;
    await fsp.rm(this.dir, { recursive: true, force: true });
    await fsp.mkdir(this.dir, { recursive: true });
  }
}
