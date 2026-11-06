import fs from 'node:fs';
import fsp from 'node:fs/promises';
import path from 'node:path';
import { isValidTile, TILE_STYLES, upstreamTileUrl, type MapTileStyle } from '@peekaround/shared';

/**
 * Map tile proxy + on-disk cache (GET /api/tiles/:style/:z/:x/:y.png, routes/tiles.ts).
 *
 * Why: in the desktop app the map is a page served from http://127.0.0.1:<port>, so browsers sent tile
 * requests with a loopback Referer and a generic Chromium/Electron User-Agent — exactly what the
 * OpenStreetMap tile usage policy blocks. The server fetches tiles itself with an identifying
 * User-Agent, keeps them on disk (offline maps for viewed areas, fewer upstream hits) and tells the UI
 * WHY a tile is missing (offline vs. refused).
 *
 *  - LRU cache under <cacheDir>/tiles, capped at Settings.tileCacheMb (trimmed to 90 % when exceeded)
 *  - concurrent requests for one tile share a single upstream fetch (coalescing)
 *  - fresh for 14 days; older tiles are revalidated, and served stale when upstream fails
 *  - a refusal (403/429/…) pauses that style's upstream for a minute (no hammering)
 */

export type TileErrorKind = 'offline' | 'refused' | 'invalid';

export class TileError extends Error {
  constructor(
    readonly kind: TileErrorKind,
    message: string,
    readonly status = 0,
  ) {
    super(message);
  }
}

export interface TileResult {
  body: Buffer;
  type: string;
  /** 'hit' | 'miss' (fetched) | 'stale' (cached copy served because upstream failed) */
  cache: 'hit' | 'miss' | 'stale';
}

interface Entry {
  size: number;
  used: number;
}

const FRESH_MS = 14 * 86_400_000;
const REFUSED_PAUSE_MS = 60_000;
const TIMEOUT_MS = 12_000;
const MAX_TILE_BYTES = 2 * 1024 * 1024;
/** upstream fetches in flight per style (OSM asks apps to keep this low) */
const MAX_PARALLEL = 4;

export interface TileServiceOptions {
  dir: string;
  userAgent: string;
  /** cache cap in MB (read on every write so the setting applies immediately) */
  capMb: () => number;
  /** tests: replace the upstream URL of a tile */
  upstream?: (style: MapTileStyle, z: number, x: number, y: number) => string;
  fetch?: typeof fetch;
}

export class TileService {
  private index: Map<string, Entry> | null = null;
  private indexing: Promise<Map<string, Entry>> | null = null;
  private total = 0;
  private inflight = new Map<string, Promise<TileResult>>();
  private paused = new Map<MapTileStyle, { until: number; status: number }>();
  private active = new Map<MapTileStyle, number>();
  private queue = new Map<MapTileStyle, (() => void)[]>();
  private trimP: Promise<void> | null = null;
  /** last upstream problem per style (UI status) */
  readonly lastError = new Map<MapTileStyle, { kind: TileErrorKind; status: number; at: number }>();
  upstream: (style: MapTileStyle, z: number, x: number, y: number) => string;
  private fetchFn: typeof fetch;

  constructor(private opts: TileServiceOptions) {
    this.upstream = opts.upstream ?? upstreamTileUrl;
    this.fetchFn = opts.fetch ?? ((...a: Parameters<typeof fetch>) => fetch(...a));
  }

  private file(style: MapTileStyle, z: number, x: number, y: number): string {
    const ext = TILE_STYLES[style].type === 'image/jpeg' ? 'jpg' : 'png';
    return path.join(this.opts.dir, style, String(z), String(x), `${y}.${ext}`);
  }

  /** LRU index of the cache directory (built once, lazily) */
  private async getIndex(): Promise<Map<string, Entry>> {
    if (this.index) return this.index;
    if (!this.indexing) {
      this.indexing = (async () => {
        const idx = new Map<string, Entry>();
        let total = 0;
        const walk = async (d: string) => {
          let entries: fs.Dirent[];
          try {
            entries = await fsp.readdir(d, { withFileTypes: true });
          } catch {
            return;
          }
          for (const e of entries) {
            const p = path.join(d, e.name);
            if (e.isDirectory()) await walk(p);
            else if (e.isFile() && !e.name.endsWith('.tmp')) {
              const st = await fsp.stat(p).catch(() => null);
              if (!st) continue;
              idx.set(p, { size: st.size, used: st.mtimeMs });
              total += st.size;
            } else if (e.name.endsWith('.tmp')) await fsp.rm(p, { force: true }).catch(() => {});
          }
        };
        await walk(this.opts.dir);
        this.total = total;
        this.index = idx;
        return idx;
      })();
    }
    return this.indexing;
  }

  async stats(): Promise<{ bytes: number; files: number; limitMb: number }> {
    const idx = await this.getIndex();
    return { bytes: this.total, files: idx.size, limitMb: this.opts.capMb() };
  }

  async clear(): Promise<void> {
    await this.getIndex();
    await fsp.rm(this.opts.dir, { recursive: true, force: true }).catch(() => {});
    this.index = new Map();
    this.indexing = Promise.resolve(this.index);
    this.total = 0;
  }

  /** one tile: cache → upstream (coalesced) → stale cache; throws TileError */
  async get(style: MapTileStyle, z: number, x: number, y: number): Promise<TileResult> {
    if (!isValidTile(style, z, x, y)) throw new TileError('invalid', 'Tile out of range', 400);
    const key = `${style}/${z}/${x}/${y}`;
    const running = this.inflight.get(key);
    if (running) return running;
    const p = this.load(style, z, x, y).finally(() => this.inflight.delete(key));
    this.inflight.set(key, p);
    return p;
  }

  private async load(style: MapTileStyle, z: number, x: number, y: number): Promise<TileResult> {
    const file = this.file(style, z, x, y);
    const idx = await this.getIndex();
    const type = TILE_STYLES[style].type;
    const st = await fsp.stat(file).catch(() => null);
    let cached: Buffer | null = null;
    if (st?.isFile()) {
      cached = await fsp.readFile(file).catch(() => null);
      if (cached && Date.now() - st.mtimeMs < FRESH_MS) {
        this.touch(idx, file, cached.length);
        return { body: cached, type, cache: 'hit' };
      }
    }
    try {
      const body = await this.fetchUpstream(style, z, x, y);
      await this.store(idx, file, body);
      return { body, type, cache: 'miss' };
    } catch (e) {
      if (cached) {
        this.touch(idx, file, cached.length);
        return { body: cached, type, cache: 'stale' };
      }
      throw e;
    }
  }

  private touch(idx: Map<string, Entry>, file: string, size: number) {
    const e = idx.get(file);
    if (e) e.used = Date.now();
    else {
      idx.set(file, { size, used: Date.now() });
      this.total += size;
    }
  }

  private async slot(style: MapTileStyle): Promise<() => void> {
    const n = this.active.get(style) ?? 0;
    if (n >= MAX_PARALLEL) {
      await new Promise<void>((resolve) => {
        const q = this.queue.get(style) ?? [];
        q.push(resolve);
        this.queue.set(style, q);
      });
    }
    this.active.set(style, (this.active.get(style) ?? 0) + 1);
    return () => {
      this.active.set(style, (this.active.get(style) ?? 1) - 1);
      const next = this.queue.get(style)?.shift();
      next?.();
    };
  }

  private async fetchUpstream(style: MapTileStyle, z: number, x: number, y: number): Promise<Buffer> {
    const pause = this.paused.get(style);
    if (pause && pause.until > Date.now()) throw new TileError('refused', `Tile server refused (HTTP ${pause.status})`, pause.status);
    const release = await this.slot(style);
    const ac = new AbortController();
    const timer = setTimeout(() => ac.abort(), TIMEOUT_MS);
    try {
      let res: Response;
      try {
        res = await this.fetchFn(this.upstream(style, z, x, y), {
          headers: { 'User-Agent': this.opts.userAgent, Accept: 'image/png,image/jpeg,image/*;q=0.8' },
          signal: ac.signal,
          redirect: 'follow',
        });
      } catch (e) {
        this.lastError.set(style, { kind: 'offline', status: 0, at: Date.now() });
        throw new TileError('offline', `Tile server unreachable: ${(e as Error).message}`);
      }
      if (!res.ok) {
        // 404 = no tile there (ocean at deep zoom on some servers): not a refusal
        if (res.status !== 404) {
          this.paused.set(style, { until: Date.now() + REFUSED_PAUSE_MS, status: res.status });
          this.lastError.set(style, { kind: 'refused', status: res.status, at: Date.now() });
        }
        await res.arrayBuffer().catch(() => null);
        throw new TileError('refused', `Tile server answered HTTP ${res.status}`, res.status);
      }
      const ct = res.headers.get('content-type') ?? '';
      const buf = Buffer.from(await res.arrayBuffer());
      if (!ct.startsWith('image/') || buf.length === 0 || buf.length > MAX_TILE_BYTES) {
        this.lastError.set(style, { kind: 'refused', status: res.status, at: Date.now() });
        throw new TileError('refused', 'Tile server sent something that is not a tile', 502);
      }
      this.paused.delete(style);
      this.lastError.delete(style);
      return buf;
    } catch (e) {
      if (e instanceof TileError) throw e;
      this.lastError.set(style, { kind: 'offline', status: 0, at: Date.now() });
      throw new TileError('offline', (e as Error).message);
    } finally {
      clearTimeout(timer);
      release();
    }
  }

  private async store(idx: Map<string, Entry>, file: string, body: Buffer) {
    try {
      await fsp.mkdir(path.dirname(file), { recursive: true });
      const tmp = `${file}.${process.pid}.${Math.random().toString(36).slice(2)}.tmp`;
      await fsp.writeFile(tmp, body);
      await fsp.rename(tmp, file);
      const old = idx.get(file);
      if (old) this.total -= old.size;
      idx.set(file, { size: body.length, used: Date.now() });
      this.total += body.length;
      if (this.total > this.capBytes()) void this.trim();
    } catch {
      /* cache is best effort */
    }
  }

  private capBytes(): number {
    const mb = Math.max(1, Math.min(64 * 1024, Number(this.opts.capMb()) || 512));
    return mb * 1024 * 1024;
  }

  /** drop least recently used tiles until the cache is at 90 % of its cap (one trim at a time) */
  trim(): Promise<void> {
    if (!this.trimP) {
      this.trimP = this.doTrim().finally(() => {
        this.trimP = null;
      });
    }
    return this.trimP;
  }

  private async doTrim(): Promise<void> {
    const idx = await this.getIndex();
    const target = this.capBytes() * 0.9;
    if (this.total <= this.capBytes()) return;
    const order = [...idx.entries()].sort((a, b) => a[1].used - b[1].used);
    for (const [file, e] of order) {
      if (this.total <= target) break;
      await fsp.rm(file, { force: true }).catch(() => {});
      idx.delete(file);
      this.total -= e.size;
    }
  }
}
