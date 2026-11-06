import { describe, expect, it, vi } from 'vitest';

// labels.ts imports the i18n module, which reads the prefs store (localStorage) — keep it in memory
vi.mock('@/store/persist', () => ({ load: (_k: string, d: unknown) => d, save: () => {} }));

const { cameraLabel, countryName, dateRange, exploreQuery, flagEmoji, memoryLabel, memoryShort, spanDays, yearRange } = await import('./labels');
const { boundsFilter, normalizeBbox, tileConfig, tileProblemOf } = await import('./mapUtil');
const { bytesLabel, dayLabel, dayRange, mapFocusHref } = await import('./labels');
const { memoryFromId } = await import('./memoryId');

const D = (y: number, m: number, d: number) => Date.UTC(y, m - 1, d);

describe('explore labels', () => {
  it('country names via Intl.DisplayNames', () => {
    expect(countryName('TR', 'tr')).toBe('Türkiye');
    expect(countryName('de', 'en')).toBe('Germany');
    expect(countryName('', 'en')).toBe('');
    expect(flagEmoji('TR')).toBe('🇹🇷');
    expect(flagEmoji('X')).toBe('');
  });

  it('date ranges are wall-clock (UTC) and end-exclusive', () => {
    expect(dateRange(D(2024, 7, 2), D(2024, 7, 6), 'en')).toMatch(/Jul 2\s*[–-]\s*5, 2024/);
    expect(dateRange(D(2024, 7, 2), D(2024, 7, 3), 'tr')).toBe('2 Tem 2024');
    expect(spanDays(D(2024, 7, 2), D(2024, 7, 6))).toBe(4);
  });

  it('memory titles', () => {
    const otd = { id: 'otd-2024-11-05', kind: 'onthisday' as const, yearsAgo: 2, from: D(2024, 11, 2), to: D(2024, 11, 9), count: 4, coverIds: [] };
    expect(memoryLabel(otd, 'tr')).toEqual({ title: '2 yıl önce bugün', subtitle: '5 Kasım 2024' });
    expect(memoryLabel({ ...otd, yearsAgo: 1 }, 'en').title).toBe('1 year ago today');
    expect(memoryShort(otd, 'en')).toBe('2 years ago');
    const trip = { id: 'trip-2024-07-02', kind: 'trip' as const, place: 'Antalya', cc: 'TR', from: D(2024, 7, 2), to: D(2024, 7, 6), count: 22, coverIds: [] };
    const l = memoryLabel(trip, 'tr');
    expect(l.title).toBe('Antalya gezisi');
    expect(l.subtitle).toMatch(/^Türkiye · 2.5 Tem 2024$/);
    expect(memoryShort(trip, 'tr')).toBe('Antalya · Tem 2024');
    expect(memoryLabel({ ...trip, place: undefined, cc: undefined }, 'en').title).toBe('Trip');
  });

  it('camera labels avoid repeating the make', () => {
    expect(cameraLabel('Nikon', 'D750')).toBe('Nikon D750');
    expect(cameraLabel('Apple', 'iPhone 13')).toBe('iPhone 13');
    expect(cameraLabel('Canon', 'Canon EOS R5')).toBe('Canon EOS R5');
    expect(cameraLabel(null, 'X100V')).toBe('X100V');
  });

  it('queries', () => {
    expect(exploreQuery({ place: 'TR-34' })).toMatchObject({ recursive: true, sort: 'takenAt', order: 'desc', place: 'TR-34' });
    expect(yearRange(2024)).toEqual({ from: D(2024, 1, 1), to: D(2025, 1, 1) });
    expect(memoryFromId('otd-2024-11-05')).toMatchObject({ from: D(2024, 11, 2), to: D(2024, 11, 9) });
    expect(memoryFromId('trip-2024-07-02')).toBeNull();
  });
});

describe('map helpers', () => {
  it('tile configs by setting and theme', () => {
    expect(tileConfig('off', true)).toBeNull();
    expect(tileConfig('carto', true)!.url).toContain('tile.openstreetmap.org');
    expect(tileConfig('osm', true)!.className).toBe('pm-tiles-dark');
    expect(tileConfig('osm', false)!.className).toBeUndefined();
    expect(tileConfig('osm', true)!.url).toContain('tile.openstreetmap.org');
  });

  it('HTTP backends load tiles through the server proxy; Android local mode goes upstream', () => {
    const proxy = (st: string) => `http://127.0.0.1:47213/api/tiles/${st}/{z}/{x}/{y}.png`;
    const osm = tileConfig('osm', true, proxy)!;
    expect(osm).toMatchObject({ style: 'osm', proxied: true, url: 'http://127.0.0.1:47213/api/tiles/osm/{z}/{x}/{y}.png', className: 'pm-tiles-dark', maxZoom: 19 });
    expect(tileConfig('carto', false, proxy)!.url).toContain('/api/tiles/osm/');
    const sat = tileConfig('satellite', true, proxy)!;
    expect(sat.className).toBeUndefined(); // imagery is never inverted
    expect(sat.attribution).toContain('Esri');
    const topo = tileConfig('topo', false)!;
    expect(topo).toMatchObject({ proxied: false, subdomains: 'abc', maxZoom: 17 });
    expect(topo.url).toContain('opentopomap.org');
    expect(tileConfig('off', false, proxy)).toBeNull();
  });

  it('explains tile failures', () => {
    expect(tileProblemOf('offline', 504)).toEqual({ kind: 'offline' });
    expect(tileProblemOf('refused:403', 502)).toEqual({ kind: 'refused', status: 403 });
    expect(tileProblemOf(null, 504)).toEqual({ kind: 'offline' });
    expect(tileProblemOf(null, 500)).toEqual({ kind: 'unknown' });
  });

  it('insight labels and links', () => {
    expect(dayRange('2024-02-29')).toEqual({ from: D(2024, 2, 29), to: D(2024, 3, 1) });
    expect(dayRange('2023-02-29')).toBeNull();
    expect(dayRange('nope')).toBeNull();
    expect(dayLabel('2024-03-05', 'tr')).toBe('5 Mart 2024');
    expect(bytesLabel(1536, 'en')).toBe('1.5 KB');
    expect(bytesLabel(5 * 1024 ** 3, 'tr')).toBe('5 GB');
    expect(mapFocusHref('12', 41.0082, 28.97841234)).toBe('/explore/map?focus=12&lat=41.008200&lon=28.978412');
    expect(memoryFromId('year-2023')).toMatchObject({ kind: 'year', from: D(2023, 1, 1), to: D(2024, 1, 1) });
    expect(memoryLabel({ id: 'year-2023', kind: 'year', from: D(2023, 1, 1), to: D(2024, 1, 1), count: 40, coverIds: [], months: [1, 0, 3, 0, 0, 0, 0, 0, 0, 0, 0, 2] }, 'tr')).toEqual({
      title: '2023 yılı',
      subtitle: '40 anı · 3 ay',
    });
  });

  it('normalises wrapped Leaflet bounds', () => {
    expect(normalizeBbox(-200, -100, 300, 100)).toEqual([-180, -90, 180, 90]);
    expect(normalizeBbox(170, 0, 190, 10)).toEqual([170, 0, -170, 10]);
    expect(normalizeBbox(NaN, 0, 1, 1)).toBeNull();
  });

  it('filters points in view, also across the antimeridian', () => {
    const pts: [string, number, number][] = [
      ['ist', 41, 29],
      ['fiji', -17, 179],
      ['samoa', -14, -172],
    ];
    expect(boundsFilter(pts, 20, 30, 40, 50)).toEqual(['ist']);
    expect(boundsFilter(pts, 170, -30, 190, 0)).toEqual(['fiji', 'samoa']);
    expect(boundsFilter(pts, -540, -90, 540, 90)).toHaveLength(3);
  });
});
