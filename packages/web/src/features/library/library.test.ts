import { afterEach, describe, expect, it, vi } from 'vitest';
import type { MediaItem, SubFolder } from '@peekaround/shared';

vi.mock('@capacitor/core', () => ({ registerPlugin: () => ({}) }));

const { HttpBackend, browseQs } = await import('@/backend/http');
const { MEDIA_MIME, isMediaDrag, readMediaDrop, startMediaDrag, endMediaDrag } = await import('./dnd');

/** minimal DataTransfer for node */
class FakeDT {
  data = new Map<string, string>();
  effectAllowed = '';
  dropEffect = '';
  get types() {
    return [...this.data.keys()];
  }
  setData(k: string, v: string) {
    this.data.set(k, v);
  }
  getData(k: string) {
    return this.data.get(k) ?? '';
  }
  setDragImage() {}
}

const item = (id: string, name: string, ext = 'jpg'): MediaItem =>
  ({ id, name, ext, kind: 'image', folderId: '1', dir: '', size: 1, mtime: 0, width: 1, height: 1, duration: null, takenAt: 0, localAt: 0, flags: 0, thumbhash: null, favorite: false, playback: 'direct' }) as MediaItem;

describe('folder sorting', async () => {
  // LibraryHeader pulls in UI modules; sortFolders is pure
  const { sortFolders } = await import('./LibraryHeader');
  const f = (name: string, mediaCount: number, latest: number | null): SubFolder => ({ name, dir: name, mediaCount, coverIds: [], latest });
  const list = [f('b 10', 5, 300), f('a', 20, null), f('b 9', 5, 900)];
  it('by name (numeric aware), date (newest first, unknown last) and count', () => {
    expect(sortFolders(list, 'name').map((x) => x.name)).toEqual(['a', 'b 9', 'b 10']);
    expect(sortFolders(list, 'date').map((x) => x.name)).toEqual(['b 9', 'b 10', 'a']);
    expect(sortFolders(list, 'count').map((x) => x.name)).toEqual(['a', 'b 9', 'b 10']);
    expect(list.map((x) => x.name)).toEqual(['b 10', 'a', 'b 9']); // not mutated
  });
});

describe('album range label', async () => {
  const { rangeLabel } = await import('./AlbumCard');
  it('one month or a range', () => {
    expect(rangeLabel(null, null, 'en')).toBeNull();
    expect(rangeLabel(Date.UTC(2024, 5, 1), Date.UTC(2024, 5, 20), 'en')).toBe('June 2024');
    expect(rangeLabel(Date.UTC(2023, 2, 1), Date.UTC(2024, 5, 20), 'en')).toBe('March 2023 – June 2024');
  });
});

describe('media drag data', () => {
  afterEach(() => endMediaDrag());
  const backend = new HttpBackend('http://pc:8080');

  it('carries ids, file urls and a DownloadURL for a single file', () => {
    const dt = new FakeDT();
    expect(startMediaDrag({ dataTransfer: dt } as unknown as DragEvent, [item('7', 'a:b.jpg')], backend)).toBe(true);
    expect(isMediaDrag({ dataTransfer: dt as unknown as DataTransfer })).toBe(true);
    expect(JSON.parse(dt.getData(MEDIA_MIME))).toEqual(['7']);
    expect(dt.getData('text/uri-list')).toBe('http://pc:8080/api/media/7/file?download=1');
    expect(dt.getData('DownloadURL')).toBe('image/jpeg:a_b.jpg:http://pc:8080/api/media/7/file?download=1');
    expect(readMediaDrop({ dataTransfer: dt } as unknown as DragEvent)).toEqual(['7']);
  });

  it('several items: no DownloadURL, all urls', () => {
    const dt = new FakeDT();
    startMediaDrag({ dataTransfer: dt } as unknown as DragEvent, [item('1', 'x.jpg'), item('2', 'y.mp4', 'mp4')], backend);
    expect(dt.getData('DownloadURL')).toBe('');
    expect(dt.getData('text/uri-list').split('\r\n')).toHaveLength(2);
    expect(isMediaDrag({ dataTransfer: { types: ['Files'] } as unknown as DataTransfer })).toBe(false);
  });
});

describe('album browse query string', () => {
  it('passes albumId / byPosition', () => {
    expect(browseQs({ albumId: '3', byPosition: true, sort: 'takenAt' })).toBe('?sort=takenAt&albumId=3&byPosition=1');
  });
  it('album endpoints', async () => {
    const fetchMock = vi.fn(async () => new Response(JSON.stringify({ albums: [{ id: '1' }], counts: { 1: 2 } }), { headers: { 'content-type': 'application/json' } }));
    vi.stubGlobal('fetch', fetchMock);
    const b = new HttpBackend('http://pc:8080');
    expect(await b.albums()).toEqual([{ id: '1' }]);
    expect(await b.albumMembership(['5', '6'])).toEqual({ 1: 2 });
    const calls = fetchMock.mock.calls as unknown as [string, RequestInit][];
    expect(calls[1][0]).toBe('http://pc:8080/api/albums/membership');
    expect(JSON.parse(String(calls[1][1].body))).toEqual({ ids: ['5', '6'] });
    expect(b.albumDownloadUrl('4', 'Yaz')).toBe('http://pc:8080/api/download?albumId=4&name=Yaz');
    vi.unstubAllGlobals();
  });
});
