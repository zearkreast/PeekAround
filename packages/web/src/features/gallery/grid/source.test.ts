import { afterEach, describe, expect, it, vi } from 'vitest';
import { QueryClient } from '@tanstack/react-query';
import { aspectToByte, type BrowseQuery, type BrowseResult, type BrowseSkeleton, type MediaItem } from '@peekaround/shared';
import { ApiError, type Backend } from '@/backend/types';
import { GridSource, PAGE, StaticSource, patchItems } from './source';
import { useLocation } from '@/store/location';

function mkItems(n: number, day0 = Date.UTC(2024, 6, 1)): MediaItem[] {
  return Array.from({ length: n }, (_, i) => ({
    id: String(i + 1),
    folderId: '1',
    dir: '',
    name: `IMG_${i}.jpg`,
    ext: 'jpg',
    kind: 'image',
    size: 1000,
    mtime: day0 - i * 3600_000,
    width: i % 2 ? 300 : 400,
    height: 300,
    duration: null,
    takenAt: day0 - i * 3600_000,
    localAt: day0 - i * 3600_000,
    flags: 0,
    thumbhash: null,
    favorite: false,
    playback: 'direct',
  }));
}

function fakeBackend(all: MediaItem[], opts: { skeleton?: boolean; gen?: string } = {}) {
  const calls = { browse: [] as BrowseQuery[], skeleton: 0 };
  const b = {
    id: 'test',
    kind: 'http',
    async browse(q: BrowseQuery): Promise<BrowseResult> {
      calls.browse.push(q);
      const off = q.offset ?? 0;
      return { crumbs: [], folders: [], roots: [], archives: [], target: null, total: all.length, items: all.slice(off, off + (q.limit ?? 600)).map((i) => ({ ...i })), gen: opts.gen };
    },
    async skeleton(): Promise<BrowseSkeleton> {
      calls.skeleton++;
      if (opts.skeleton === false) throw new ApiError(404, 'not found');
      const a = all.map((i) => aspectToByte((i.width ?? 1) / (i.height ?? 1)));
      return { gen: opts.gen ?? 'g1', total: all.length, kindCounts: { image: all.length, video: 0, audio: 0 }, aspects: btoa(String.fromCharCode(...a)), days: null };
    },
  } as unknown as Backend;
  return { b, calls };
}

const until = async (fn: () => boolean, ms = 2000) => {
  const t0 = Date.now();
  while (!fn()) {
    if (Date.now() - t0 > ms) throw new Error('timeout');
    await new Promise((r) => setTimeout(r, 5));
  }
};

describe('GridSource', () => {
  const sources: GridSource[] = [];
  afterEach(() => {
    for (const s of sources) s.stop();
    sources.length = 0;
  });
  const make = (b: Backend, q: BrowseQuery = { sort: 'takenAt', order: 'desc', recursive: true }) => {
    const s = new GridSource(`k${Math.random()}`, new QueryClient(), b, q);
    sources.push(s);
    s.start();
    return s;
  };

  it('loads the skeleton, then only the pages of the requested range (+ prefetch, + page 0 meta)', async () => {
    const all = mkItems(PAGE * 10 + 17);
    const { b, calls } = fakeBackend(all);
    const s = make(b);
    await until(() => s.ready);
    expect(s.total).toBe(all.length);
    expect(s.aspects.length).toBe(all.length);
    const a = PAGE * 5 + 3;
    s.ensureRange(a, a + 40, 1);
    await until(() => !!s.getAt(a) && !!s.getAt(a + 40) && !!s.meta);
    expect(s.getAt(a)!.id).toBe(all[a].id);
    expect(s.indexOf(all[a + 10].id)).toBe(a + 10);
    const offsets = new Set(calls.browse.map((q) => q.offset));
    expect(offsets.has(PAGE * 5)).toBe(true);
    expect(offsets.has(0)).toBe(true); // meta
    expect(offsets.has(PAGE * 2)).toBe(false); // never requested
    expect(s.getAt(PAGE * 2)).toBeUndefined();
  });

  it('idsInRange / itemsInRange also cover pages that are not loaded', async () => {
    const all = mkItems(PAGE * 4);
    const { b } = fakeBackend(all);
    const s = make(b);
    await until(() => s.ready);
    const ids = await s.idsInRange(PAGE - 5, PAGE * 3 + 5);
    expect(ids.length).toBe(PAGE * 2 + 11);
    expect(ids[0]).toBe(all[PAGE - 5].id);
    expect(ids[ids.length - 1]).toBe(all[PAGE * 3 + 5].id);
  });

  it('falls back to a client-built skeleton when the endpoint is missing', async () => {
    const all = mkItems(1234);
    const { b, calls } = fakeBackend(all, { skeleton: false });
    const s = make(b);
    await until(() => s.ready && !!s.getAt(1233));
    expect(calls.skeleton).toBe(1);
    expect(s.total).toBe(1234);
    expect(s.days?.length).toBeGreaterThan(0); // sorted by takenAt → day groups
    expect(s.days!.reduce((n, d) => n + d.count, 0)).toBe(1234);
  });

  it('patch() replaces loaded items in place without a relayout', async () => {
    const all = mkItems(50);
    const { b } = fakeBackend(all);
    const s = make(b);
    await until(() => s.ready);
    s.ensureRange(0, 49);
    await until(() => !!s.getAt(49));
    const lv = s.layoutVersion;
    const v = s.version;
    s.patch((it) => (it.id === '3' ? { ...it, favorite: true } : null));
    expect(s.getAt(2)!.favorite).toBe(true);
    expect(s.layoutVersion).toBe(lv);
    expect(s.version).toBeGreaterThan(v);
  });

  it('a new generation from the server resets the pages', async () => {
    const all = mkItems(40);
    let gen = 'a';
    const { b } = fakeBackend(all);
    (b as unknown as { skeleton: () => Promise<BrowseSkeleton> }).skeleton = vi.fn(async () => ({
      gen,
      total: all.length,
      kindCounts: { image: all.length, video: 0, audio: 0 },
      aspects: btoa(String.fromCharCode(...all.map(() => 128))),
      days: null,
    }));
    const s = make(b);
    await until(() => s.ready);
    s.ensureRange(0, 39);
    await until(() => !!s.getAt(0));
    const lv = s.layoutVersion;
    gen = 'b';
    all.pop();
    s.refresh();
    await until(() => s.layoutVersion > lv);
    expect(s.total).toBe(39);
  });

  it('StaticSource exposes a fixed list', async () => {
    const items = mkItems(7);
    const s = new StaticSource('x', items);
    expect(s.total).toBe(7);
    expect(s.getAt(3)!.id).toBe('4');
    expect(s.indexOf('5')).toBe(4);
    expect(await s.idsInRange(1, 2)).toEqual(['2', '3']);
  });

  it('patchItems updates the shown StaticSource (map / home: favorite toggles from the viewer)', () => {
    const s = new StaticSource('y', mkItems(3));
    const seen = vi.fn();
    const off = s.subscribe(seen);
    useLocation.getState().setView({ source: s });
    const v0 = s.version;
    patchItems(['2'], { favorite: true });
    expect(s.getAt(1)!.favorite).toBe(true);
    expect(s.getAt(0)!.favorite).toBe(false);
    expect(s.version).toBeGreaterThan(v0);
    expect(seen).toHaveBeenCalled();
    off();
    useLocation.getState().setView({ source: null });
  });
});
