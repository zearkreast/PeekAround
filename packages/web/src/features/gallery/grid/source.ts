/**
 * GridSource — random-access view over a browse query.
 *
 *   total, aspects, days      ← backend.skeleton(q) (or a client-side fallback built from pages)
 *   getAt(i)                  ← offset page cache (PAGE items per page, LRU, priority queue)
 *   ensureRange(a, b, dir)    → loads the pages of a visible index range (+ prefetch ahead)
 *   indexOf(id), idsInRange(a, b), itemsInRange(a, b)
 *   meta                      ← crumbs / folders / roots / archives / target of page 0
 *
 * The grid lays out everything from the skeleton, so loading pages never moves a tile.
 * Sources are shared per (backend, query) and kept alive for a minute after the last user
 * (going back from the viewer / another page is instant).
 */
import { QueryObserver, type QueryClient } from '@tanstack/react-query';
import { useEffect, useMemo, useSyncExternalStore } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { aspectToByte, type BrowseQuery, type BrowseResult, type BrowseSkeleton, type MediaItem } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { ApiError, type Backend } from '@/backend/types';
import { buildSkeleton } from '@/backend/android/skeleton';
import { useLocation } from '@/store/location';

export const PAGE = 300;
const MAX_PAGES = 40; // ≈ 12k items kept in memory
const CONCURRENCY = 3;
const FALLBACK_PAGE = 5000;

export type SourceMeta = Omit<BrowseResult, 'items'>;

export interface GridItems {
  readonly key: string;
  readonly total: number;
  readonly aspects: Uint8Array;
  readonly days: BrowseSkeleton['days'];
  readonly kindCounts: BrowseSkeleton['kindCounts'];
  readonly gen: string;
  readonly ready: boolean;
  readonly error: Error | null;
  readonly meta: SourceMeta | null;
  /** bumps on every change (items loaded/patched) */
  readonly version: number;
  /** bumps when the layout input (total/aspects/days) changes */
  readonly layoutVersion: number;
  getAt(i: number): MediaItem | undefined;
  ensureRange(a: number, b: number, dir?: 1 | -1 | 0): void;
  indexOf(id: string): number;
  idsInRange(a: number, b: number): Promise<string[]>;
  itemsInRange(a: number, b: number): Promise<MediaItem[]>;
  subscribe(fn: () => void): () => void;
}

/** Older servers / Android: items may lack localAt → derive local wall-clock time. */
export function normalizeItem(it: MediaItem): MediaItem {
  if (typeof it.localAt !== 'number') (it as MediaItem).localAt = it.takenAt - new Date(it.takenAt).getTimezoneOffset() * 60000;
  if (typeof it.flags !== 'number') (it as MediaItem).flags = 0;
  return it;
}

function b64ToBytes(s: string): Uint8Array<ArrayBuffer> {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

interface SkeletonResult {
  sk: BrowseSkeleton;
  /** fallback mode: every item, already in order */
  all?: MediaItem[];
  meta?: SourceMeta;
}

/** backends whose skeleton endpoint is missing (until reload) */
const noSkeleton = new Set<string>();

async function fetchSkeleton(backend: Backend, q: BrowseQuery, forceFallback: boolean): Promise<SkeletonResult> {
  if (!forceFallback && !noSkeleton.has(backend.id)) {
    try {
      const sk = await backend.skeleton(q);
      if (sk && typeof sk.total === 'number' && typeof sk.aspects === 'string') return { sk };
    } catch (e) {
      if (!(e instanceof ApiError) || e.status === 0) throw e;
      if (e.status === 404 || e.status === 405 || e.status === 501) noSkeleton.add(backend.id);
    }
  }
  // fallback: page through the whole result with big pages
  const all: MediaItem[] = [];
  let meta: SourceMeta | undefined;
  let total = Infinity;
  for (let offset = 0; offset < total; ) {
    const r = await backend.browse({ ...q, offset, limit: FALLBACK_PAGE });
    if (!meta) {
      const { items: _i, ...m } = r;
      meta = m;
    }
    total = r.total;
    for (const it of r.items) all.push(normalizeItem(it));
    if (!r.items.length) break;
    offset += r.items.length;
  }
  return { sk: buildSkeleton(all, q), all, meta };
}

interface Page {
  items: MediaItem[] | null;
  state: 'queued' | 'loading' | 'ok' | 'stale' | 'error';
  used: number;
  pinned: boolean;
}

let useClock = 0;

export class GridSource implements GridItems {
  total = 0;
  aspects: Uint8Array = new Uint8Array(0);
  days: BrowseSkeleton['days'] = null;
  kindCounts: BrowseSkeleton['kindCounts'] = { image: 0, video: 0, audio: 0 };
  gen = '';
  ready = false;
  error: Error | null = null;
  meta: SourceMeta | null = null;
  version = 0;
  layoutVersion = 0;

  private pages = new Map<number, Page>();
  private ids = new Map<string, number>();
  private listeners = new Set<() => void>();
  private observer: QueryObserver<SkeletonResult> | null = null;
  private unsub: (() => void) | null = null;
  private inflight = 0;
  private window: [number, number, number] = [0, 60, 0];
  private notifyQueued = false;
  private fallback = false;
  private epoch = 0;
  /** aborts in-flight page requests of the previous epoch */
  private ctl = new AbortController();
  private mismatchChecks = 0;

  constructor(
    readonly key: string,
    private qc: QueryClient,
    private backend: Backend,
    readonly query: BrowseQuery,
  ) {}

  // ── lifecycle ──
  start() {
    if (this.observer) return;
    this.observer = new QueryObserver<SkeletonResult>(this.qc, {
      queryKey: ['browse', this.backend.id, 'skeleton', this.query, this.fallback],
      queryFn: () => fetchSkeleton(this.backend, this.query, this.fallback),
      staleTime: 5_000,
      gcTime: 60_000,
    });
    this.unsub = this.observer.subscribe((r) => {
      if (r.data) this.applySkeleton(r.data);
      if (r.error && !this.ready) {
        this.error = r.error as Error;
        this.bump();
      }
    });
    const cur = this.observer.getCurrentResult();
    if (cur.data) this.applySkeleton(cur.data);
  }

  stop() {
    this.unsub?.();
    this.unsub = null;
    this.observer?.destroy();
    this.observer = null;
  }

  /** refetch the skeleton (structure may have changed) */
  refresh() {
    void this.observer?.refetch();
  }

  /** re-request the loaded pages of the current window (fields changed, structure did not) */
  refreshPages() {
    for (const p of this.pages.values()) if (p.state === 'ok' && !p.pinned) p.state = 'stale';
    if (this.fallback) {
      this.refresh();
      return;
    }
    this.pump();
  }

  private switchToFallback() {
    if (this.fallback) return;
    this.fallback = true;
    this.stop();
    this.start();
  }

  private applySkeleton(r: SkeletonResult) {
    const sk = r.sk;
    const changed = sk.gen !== this.gen || sk.total !== this.total || !this.ready;
    if (changed) {
      this.epoch++;
      this.ctl.abort();
      this.ctl = new AbortController();
      this.gen = sk.gen;
      this.total = sk.total;
      this.aspects = b64ToBytes(sk.aspects);
      if (this.aspects.length < sk.total) {
        const a = new Uint8Array(sk.total).fill(aspectToByte(1));
        a.set(this.aspects);
        this.aspects = a;
      }
      this.days = sk.days;
      this.kindCounts = sk.kindCounts ?? this.kindCounts;
      this.pages.clear();
      this.ids.clear();
      this.layoutVersion++;
    } else {
      for (const p of this.pages.values()) if (p.state === 'ok') p.state = 'stale';
    }
    if (r.all) {
      this.pages.clear();
      this.ids.clear();
      for (let p = 0; p * PAGE < r.all.length; p++) {
        const items = r.all.slice(p * PAGE, (p + 1) * PAGE);
        this.pages.set(p, { items, state: 'ok', used: ++useClock, pinned: true });
        items.forEach((it, k) => this.ids.set(it.id, p * PAGE + k));
      }
    }
    if (r.meta) this.meta = r.meta;
    this.ready = true;
    this.error = null;
    this.bump();
    this.pump();
  }

  // ── reading ──
  getAt(i: number): MediaItem | undefined {
    const p = this.pages.get(Math.floor(i / PAGE));
    return p?.items?.[i % PAGE];
  }

  indexOf(id: string): number {
    return this.ids.get(id) ?? -1;
  }

  subscribe = (fn: () => void) => {
    this.listeners.add(fn);
    return () => void this.listeners.delete(fn);
  };
  getVersion = () => this.version;

  private bump() {
    this.version++;
    if (this.notifyQueued) return;
    this.notifyQueued = true;
    queueMicrotask(() => {
      this.notifyQueued = false;
      for (const l of [...this.listeners]) l();
    });
  }

  // ── loading ──
  ensureRange(a: number, b: number, dir: 1 | -1 | 0 = 0) {
    this.window = [Math.max(0, a), Math.max(a, b), dir];
    this.pump();
  }

  /** pages wanted for the current window, most important first */
  private wanted(): number[] {
    const [a, b, dir] = this.window;
    // page 0 also carries the meta (crumbs, sub folders, roots) — needed even for 0 items
    if (!this.ready || !this.total) return [0];
    const last = Math.ceil(this.total / PAGE) - 1;
    const pa = Math.floor(Math.min(a, this.total - 1) / PAGE);
    const pb = Math.floor(Math.min(b, this.total - 1) / PAGE);
    const out: number[] = [];
    const mid = (pa + pb) / 2;
    const vis: number[] = [];
    for (let p = pa; p <= pb; p++) vis.push(p);
    vis.sort((x, y) => Math.abs(x - mid) - Math.abs(y - mid));
    out.push(...vis);
    // prefetch ahead in the scroll direction (and one behind)
    if (dir >= 0 && pb + 1 <= last) out.push(pb + 1);
    if (dir <= 0 && pa - 1 >= 0) out.push(pa - 1);
    if (dir > 0 && pb + 2 <= last) out.push(pb + 2);
    if (dir < 0 && pa - 2 >= 0) out.push(pa - 2);
    if (!out.includes(0)) out.push(0); // meta (crumbs, folders) always
    return out;
  }

  private pump() {
    if (this.fallback || !this.ready) return;
    const want = this.wanted();
    for (const p of want) {
      if (this.inflight >= CONCURRENCY) break;
      const page = this.pages.get(p);
      if (page && (page.state === 'ok' || page.state === 'loading' || page.state === 'error')) {
        if (page.state === 'ok') page.used = ++useClock;
        continue;
      }
      this.load(p);
    }
  }

  private async load(p: number) {
    const epoch = this.epoch;
    const prev = this.pages.get(p);
    const entry: Page = { items: prev?.items ?? null, state: 'loading', used: ++useClock, pinned: false };
    this.pages.set(p, entry);
    this.inflight++;
    try {
      const r = await this.backend.browse({ ...this.query, offset: p * PAGE, limit: PAGE }, { signal: this.ctl.signal });
      if (epoch !== this.epoch) return;
      const gen = (r as BrowseResult & { gen?: string }).gen;
      if ((gen && this.gen && gen !== this.gen) || r.total !== this.total) {
        // the result set changed under us (or the skeleton doesn't match the pages)
        if (++this.mismatchChecks > 2) this.switchToFallback();
        else this.refresh();
      } else this.mismatchChecks = 0;
      const items = r.items.map(normalizeItem);
      if (entry.items) for (const old of entry.items) if (this.ids.get(old.id) !== undefined) this.ids.delete(old.id);
      entry.items = items;
      entry.state = 'ok';
      items.forEach((it, k) => this.ids.set(it.id, p * PAGE + k));
      if (p === 0) {
        const { items: _i, ...m } = r;
        this.meta = m;
      }
      this.evict();
      this.bump();
    } catch (e) {
      if (epoch !== this.epoch) return;
      entry.state = 'error';
      this.error = e as Error;
      setTimeout(() => {
        if (entry.state === 'error') {
          entry.state = 'stale';
          this.pump();
        }
      }, 4000);
      this.bump();
    } finally {
      this.inflight = Math.max(0, this.inflight - 1);
      this.pump();
    }
  }

  private evict() {
    if (this.pages.size <= MAX_PAGES) return;
    const keep = new Set(this.wanted());
    const list = [...this.pages.entries()].filter(([p, v]) => !keep.has(p) && !v.pinned && v.state !== 'loading').sort((a, b) => a[1].used - b[1].used);
    for (const [p, v] of list) {
      if (this.pages.size <= MAX_PAGES) break;
      for (const it of v.items ?? []) this.ids.delete(it.id);
      this.pages.delete(p);
    }
  }

  async itemsInRange(a: number, b: number): Promise<MediaItem[]> {
    a = Math.max(0, a);
    b = Math.min(this.total - 1, b);
    if (b < a) return [];
    const out: MediaItem[] = new Array(b - a + 1);
    const missing: number[] = [];
    for (let i = a; i <= b; i++) {
      const it = this.getAt(i);
      if (it) out[i - a] = it;
      else missing.push(i);
    }
    // fetch the gaps with big pages (not stored in the LRU)
    let k = 0;
    while (k < missing.length) {
      const start = missing[k];
      let end = start;
      while (k < missing.length && missing[k] - start < FALLBACK_PAGE) end = missing[k++];
      const r = await this.backend.browse({ ...this.query, offset: start, limit: end - start + 1 });
      r.items.forEach((it, j) => {
        if (start + j <= b && !out[start + j - a]) out[start + j - a] = normalizeItem(it);
      });
    }
    return out.filter(Boolean);
  }

  async idsInRange(a: number, b: number): Promise<string[]> {
    return (await this.itemsInRange(a, b)).map((i) => i.id);
  }

  /** replace matching loaded items in place (no relayout) */
  patch(fn: (it: MediaItem) => MediaItem | null) {
    let n = 0;
    for (const p of this.pages.values()) {
      if (!p.items) continue;
      for (let k = 0; k < p.items.length; k++) {
        const r = fn(p.items[k]);
        if (r && r !== p.items[k]) {
          p.items[k] = r;
          n++;
        }
      }
    }
    if (n) this.bump();
  }

  matches(folderId: string): boolean {
    return !this.query.folderId || this.query.folderId === folderId;
  }
}

/** A fixed, already loaded list (home "recent", pickers). */
export class StaticSource implements GridItems {
  readonly ready = true;
  readonly error = null;
  readonly meta = null;
  readonly gen: string;
  readonly days: BrowseSkeleton['days'];
  readonly kindCounts: BrowseSkeleton['kindCounts'];
  readonly aspects: Uint8Array;
  readonly total: number;
  version = 0;
  layoutVersion = 0;
  private ids = new Map<string, number>();
  constructor(
    readonly key: string,
    private items: MediaItem[],
  ) {
    items.forEach(normalizeItem);
    const sk = buildSkeleton(items, { sort: 'mtime' });
    this.gen = sk.gen;
    this.days = null;
    this.kindCounts = sk.kindCounts;
    this.total = items.length;
    this.aspects = b64ToBytes(sk.aspects);
    items.forEach((it, i) => this.ids.set(it.id, i));
  }
  getAt(i: number) {
    return this.items[i];
  }
  /** replace matching items in place (favorite toggles…) */
  patch(fn: (it: MediaItem) => MediaItem | null) {
    let n = 0;
    this.items = this.items.map((it) => {
      const r = fn(it);
      if (r && r !== it) n++;
      return r ?? it;
    });
    if (n) {
      this.version++;
      for (const l of [...this.listeners]) l();
    }
  }
  ensureRange() {}
  indexOf(id: string) {
    return this.ids.get(id) ?? -1;
  }
  async idsInRange(a: number, b: number) {
    return this.items.slice(a, b + 1).map((i) => i.id);
  }
  async itemsInRange(a: number, b: number) {
    return this.items.slice(a, b + 1);
  }
  private listeners = new Set<() => void>();
  subscribe = (fn: () => void) => {
    this.listeners.add(fn);
    return () => void this.listeners.delete(fn);
  };
  getVersion = () => this.version;
}

// ── registry ──

interface Entry {
  src: GridSource;
  refs: number;
  timer: ReturnType<typeof setTimeout> | null;
}
const registry = new Map<string, Entry>();

export function liveSources(): GridSource[] {
  return [...registry.values()].filter((e) => e.refs > 0 || e.timer).map((e) => e.src);
}

/** Patch an item everywhere it is loaded (favorite toggles, renames…). */
export function patchItems(ids: Iterable<string>, patch: Partial<MediaItem>) {
  const set = new Set(ids);
  const fn = (it: MediaItem) => (set.has(it.id) ? { ...it, ...patch } : null);
  for (const s of liveSources()) s.patch(fn);
  // fixed lists (map, home "recent") are not in the registry; the shown one is the viewer's source
  const shown = useLocation.getState().source;
  if (shown instanceof StaticSource) shown.patch(fn);
}

export function dropSources(backendId?: string) {
  for (const [k, e] of registry) {
    if (backendId && !k.startsWith(`${backendId}|`)) continue;
    if (e.refs > 0) continue;
    if (e.timer) clearTimeout(e.timer);
    e.src.stop();
    registry.delete(k);
  }
}

function getSource(qc: QueryClient, backend: Backend, q: BrowseQuery): GridSource {
  const key = `${backend.id}|${JSON.stringify(q)}`;
  let e = registry.get(key);
  if (!e) {
    e = { src: new GridSource(key, qc, backend, q), refs: 0, timer: null };
    registry.set(key, e);
  }
  return e.src;
}

function retain(src: GridSource) {
  const e = registry.get(src.key);
  if (!e) return () => {};
  e.refs++;
  if (e.timer) {
    clearTimeout(e.timer);
    e.timer = null;
  }
  src.start();
  return () => {
    e.refs--;
    if (e.refs > 0) return;
    e.timer = setTimeout(() => {
      e.timer = null;
      if (e.refs > 0) return;
      e.src.stop();
      registry.delete(src.key);
    }, 60_000);
  };
}

/** Shared GridSource for a browse query (null → disabled). Re-renders on every source change. */
export function useGridSource(q: BrowseQuery | null): GridSource | null {
  const backend = useBackend();
  const qc = useQueryClient();
  const key = q ? JSON.stringify(q) : '';
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const src = useMemo(() => (q ? getSource(qc, backend, q) : null), [backend, qc, key]);
  useEffect(() => (src ? retain(src) : undefined), [src]);
  useSourceVersion(src);
  return src;
}

const noopSub = () => () => {};
const zero = () => 0;
/** subscribe a component to a source's changes */
export function useSourceVersion(src: GridItems | null): number {
  return useSyncExternalStore(src ? src.subscribe : noopSub, src ? () => src.version : zero);
}
