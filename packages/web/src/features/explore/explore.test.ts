import { describe, expect, it, vi } from 'vitest';

// labels.ts imports the i18n module, which reads the prefs store (localStorage) — keep it in memory
vi.mock('@/store/persist', () => ({ load: (_k: string, d: unknown) => d, save: () => {} }));

const { cameraLabel, countryName, dateRange, exploreQuery, flagEmoji, memoryLabel, memoryShort, spanDays, yearRange } = await import('./labels');
const { boundsFilter, normalizeBbox, tileConfig } = await import('./mapUtil');
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
