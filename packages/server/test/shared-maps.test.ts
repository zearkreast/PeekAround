import { describe, expect, it } from 'vitest';
import {
  base64Head,
  calendarDayKey,
  calendarOf,
  colorBucketOf,
  colorIndexOfThumbhash,
  COLOR_BUCKETS,
  computeInsights,
  dayTimeOf,
  estimateInstant,
  isMessagingName,
  isValidTile,
  matchSidecar,
  mediaNameVariants,
  orientationOf,
  parseGpsCoordinate,
  parseSidecarName,
  parseTakeoutJson,
  sunAltitude,
  thumbhashAverage,
  tileOf,
  tileStyleOf,
  timeOfDayOf,
  upstreamTileUrl,
  videoLengthOf,
  yearReview,
  type InsightRow,
} from '@peekaround/shared';
import { rgbaToThumbHash } from 'thumbhash';
import { parseVideoLocation, videoGroupMeta } from '../src/media/metadata';

const ISTANBUL = { lat: 41.0082, lon: 28.9784 };

describe('sun position', () => {
  it('matches known solar altitudes', () => {
    // solar noon in Istanbul at the June solstice: 90 - (41.0 - 23.44) ≈ 72.4°
    const noon = Date.parse('2024-06-21T10:05:00Z');
    expect(sunAltitude(noon, ISTANBUL.lat, ISTANBUL.lon)).toBeCloseTo(72.4, 0);
    // December solstice noon: 90 - 41 - 23.44 ≈ 25.6°
    expect(sunAltitude(Date.parse('2024-12-21T10:08:00Z'), ISTANBUL.lat, ISTANBUL.lon)).toBeCloseTo(25.5, 0);
    // midnight: deep below the horizon
    expect(sunAltitude(Date.parse('2024-06-21T21:05:00Z'), ISTANBUL.lat, ISTANBUL.lon)).toBeLessThan(-20);
    // equator at the equinox, solar noon at Greenwich ≈ 90° (equation of time ≈ -7 min)
    expect(sunAltitude(Date.parse('2024-03-20T12:07:00Z'), 0, 0)).toBeGreaterThan(88);
  });

  it('sunrise / sunset are near the horizon and golden', () => {
    // Istanbul 2024-06-21: sunrise 05:32 local (+03:00), sunset 20:39
    const rise = sunAltitude(Date.parse('2024-06-21T02:32:00Z'), ISTANBUL.lat, ISTANBUL.lon);
    const set = sunAltitude(Date.parse('2024-06-21T17:39:00Z'), ISTANBUL.lat, ISTANBUL.lon);
    expect(Math.abs(rise + 0.8)).toBeLessThan(1.2);
    expect(Math.abs(set + 0.8)).toBeLessThan(1.2);
    expect(timeOfDayOf(rise)).toBe('golden');
    expect(timeOfDayOf(3)).toBe('golden');
    expect(timeOfDayOf(30)).toBe('day');
    expect(timeOfDayOf(-6)).toBe('twilight');
    expect(timeOfDayOf(-20)).toBe('night');
  });

  it('day times: sun when known, wall clock otherwise, never for file dates', () => {
    const at = (h: number) => Date.UTC(2024, 0, 1, h, 0);
    expect(dayTimeOf(-30, at(12), true)).toBe('night');
    expect(dayTimeOf(2, at(12), true)).toBe('golden');
    expect(dayTimeOf(40, at(23), true)).toBeNull();
    expect(dayTimeOf(null, at(23), true)).toBe('night');
    expect(dayTimeOf(null, at(3), true)).toBe('night');
    expect(dayTimeOf(null, at(12), true)).toBeNull();
    expect(dayTimeOf(null, at(23), false)).toBeNull();
  });

  it('estimates the instant of a wall clock abroad from the longitude', () => {
    const local = Date.UTC(2024, 6, 1, 20, 0);
    // at home (Istanbul +03:00, lon 29 → +2h solar): home-zone instant kept
    const home = local - 180 * 60_000;
    expect(estimateInstant(local, home, 29)).toBe(home);
    // New York (lon -74 → -5h) with an Istanbul home zone: 8 h apart → longitude wins
    expect(estimateInstant(local, home, -74)).toBe(local + 5 * 3600_000);
  });
});

describe('colour buckets', () => {
  it('buckets average colours', () => {
    expect(colorBucketOf(0.85, 0.15, 0.12)).toBe('red');
    expect(colorBucketOf(0.9, 0.5, 0.1)).toBe('orange');
    expect(colorBucketOf(0.9, 0.82, 0.2)).toBe('yellow');
    expect(colorBucketOf(0.2, 0.6, 0.25)).toBe('green');
    expect(colorBucketOf(0.2, 0.35, 0.8)).toBe('blue');
    expect(colorBucketOf(0.5, 0.25, 0.75)).toBe('purple');
    expect(colorBucketOf(0.9, 0.3, 0.6)).toBe('pink');
    expect(colorBucketOf(0.5, 0.5, 0.52)).toBe('bw');
    expect(colorBucketOf(0.04, 0.02, 0.03)).toBe('bw');
    expect(colorBucketOf(0.98, 0.97, 0.96)).toBe('bw');
  });

  it('reads the average colour from a real thumbhash', () => {
    const w = 16;
    const h = 16;
    const px = new Uint8Array(w * h * 4);
    for (let i = 0; i < w * h; i++) px.set([40, 90, 210, 255], i * 4);
    const hash = rgbaToThumbHash(w, h, px);
    const avg = thumbhashAverage(hash)!;
    expect(avg.b).toBeGreaterThan(avg.r);
    const b64 = Buffer.from(hash).toString('base64');
    expect([...base64Head(b64, 6)!]).toEqual([...hash.slice(0, 6)]);
    expect(COLOR_BUCKETS[colorIndexOfThumbhash(b64)]).toBe('blue');
    expect(colorIndexOfThumbhash(null)).toBe(-1);
    expect(colorIndexOfThumbhash('!!')).toBe(-1);
  });
});

describe('Google Takeout sidecars', () => {
  it('parses sidecar names', () => {
    expect(parseSidecarName('IMG_1.jpg.json')).toEqual({ target: 'IMG_1.jpg', dup: 0, truncated: false });
    expect(parseSidecarName('IMG_1.jpg.supplemental-metadata.json')).toMatchObject({ target: 'IMG_1.jpg', dup: 0 });
    expect(parseSidecarName('IMG_1.jpg.supplemental-metadata(2).json')).toMatchObject({ target: 'IMG_1.jpg', dup: 2 });
    expect(parseSidecarName('IMG_1.jpg(1).json')).toMatchObject({ target: 'IMG_1.jpg', dup: 1 });
    expect(parseSidecarName('IMG_20190705_121212345_HDR.jpg.supplemental-me.json')).toMatchObject({ target: 'IMG_20190705_121212345_HDR.jpg', truncated: true });
    expect(parseSidecarName('metadata.json')).toMatchObject({ target: 'metadata' });
    expect(parseSidecarName('photo.jpg')).toBeNull();
  });

  it('media name variants: (n) and edited copies', () => {
    expect(mediaNameVariants('IMG_1(1).jpg')).toEqual({ names: ['IMG_1.jpg'], dup: 1 });
    expect(mediaNameVariants('IMG_1-edited.jpg')).toEqual({ names: ['IMG_1-edited.jpg', 'IMG_1.jpg'], dup: 0 });
    expect(mediaNameVariants('IMG_1-düzenlendi.jpg').names).toContain('IMG_1.jpg');
  });

  it('matches files to their sidecars', () => {
    const dir = [
      'IMG_1.jpg.json',
      'IMG_1.jpg(1).json',
      'IMG_2.jpg.supplemental-metadata.json',
      'IMG_3.jpg.supplemental-metadata(1).json',
      'Screenshot_20190101-120000_Some Long App Na.json',
      'IMG_20190705_121212345_HDR.jpg.supplemental-me.json',
      'IMG_9.HEIC.json',
      'metadata.json',
    ];
    expect(matchSidecar('IMG_1.jpg', dir)).toBe('IMG_1.jpg.json');
    expect(matchSidecar('IMG_1(1).jpg', dir)).toBe('IMG_1.jpg(1).json');
    expect(matchSidecar('IMG_1-edited.jpg', dir)).toBe('IMG_1.jpg.json');
    expect(matchSidecar('IMG_2.jpg', dir)).toBe('IMG_2.jpg.supplemental-metadata.json');
    expect(matchSidecar('IMG_3(1).jpg', dir)).toBe('IMG_3.jpg.supplemental-metadata(1).json');
    expect(matchSidecar('IMG_3.jpg', dir)).toBeNull(); // only the (1) copy has one
    expect(matchSidecar('Screenshot_20190101-120000_Some Long App Name.jpg', dir)).toBe('Screenshot_20190101-120000_Some Long App Na.json');
    expect(matchSidecar('IMG_20190705_121212345_HDR.jpg', dir)).toBe('IMG_20190705_121212345_HDR.jpg.supplemental-me.json');
    expect(matchSidecar('IMG_9.MP4', dir)).toBe('IMG_9.HEIC.json'); // live photo video
    expect(matchSidecar('IMG_4.jpg', dir)).toBeNull();
    expect(matchSidecar('IMG_1.jpeg', ['IMG_1.jpg.json', 'IMG_1.jpeg.json'])).toBe('IMG_1.jpeg.json');
  });

  it('reads photoTakenTime and geoData (0,0 = none)', () => {
    expect(parseTakeoutJson({ photoTakenTime: { timestamp: '1562235010' }, geoData: { latitude: 37.9715, longitude: 23.7267 } })).toEqual({ takenAt: 1562235010000, lat: 37.9715, lon: 23.7267 });
    expect(parseTakeoutJson({ photoTakenTime: { timestamp: '1562235010' }, geoData: { latitude: 0, longitude: 0 }, geoDataExif: { latitude: 41, longitude: 29 } })).toMatchObject({ lat: 41, lon: 29 });
    expect(parseTakeoutJson({ photoTakenTime: { timestamp: '1562235010' }, geoData: { latitude: 0, longitude: 0 } })).toEqual({ takenAt: 1562235010000, lat: null, lon: null });
    expect(parseTakeoutJson({ title: 'x' })).toBeNull();
    expect(parseTakeoutJson(null)).toBeNull();
  });
});

describe('GPS strings (XMP / EXIF)', () => {
  it('parses every common form', () => {
    expect(parseGpsCoordinate('41,0.738N')).toBeCloseTo(41.0123, 4);
    expect(parseGpsCoordinate('28,58.476E')).toBeCloseTo(28.9746, 4);
    expect(parseGpsCoordinate('33,51,54.5S')).toBeCloseTo(-33.86514, 4);
    expect(parseGpsCoordinate('70,30.0W')).toBeCloseTo(-70.5, 6);
    expect(parseGpsCoordinate('41.0123')).toBeCloseTo(41.0123, 6);
    expect(parseGpsCoordinate('-12.5')).toBe(-12.5);
    expect(parseGpsCoordinate('12.5', 'S')).toBe(-12.5);
    expect(parseGpsCoordinate([41, 0, 44.28], 'N')).toBeCloseTo(41.0123, 4);
    expect(parseGpsCoordinate(28.97, 'W')).toBe(-28.97);
    expect(parseGpsCoordinate(`41°0'44.28"N`)).toBeCloseTo(41.0123, 4);
    expect(parseGpsCoordinate('N 41.5')).toBe(41.5);
    expect(parseGpsCoordinate('41,75.0N')).toBeNull(); // minutes ≥ 60
    expect(parseGpsCoordinate('abc')).toBeNull();
    expect(parseGpsCoordinate('')).toBeNull();
    expect(parseGpsCoordinate(undefined)).toBeNull();
  });

  it('video location tags', () => {
    expect(parseVideoLocation('+41.0370+028.9850/')).toEqual({ lat: 41.037, lon: 28.985 });
    expect(parseVideoLocation('41.0370, 28.9850')).toEqual({ lat: 41.037, lon: 28.985 });
    expect(parseVideoLocation('-33.86 151.21 12.0')).toEqual({ lat: -33.86, lon: 151.21 });
    expect(parseVideoLocation('nowhere')).toBeNull();
    const probe = (tags: Record<string, string>) => videoGroupMeta({ tags } as never);
    expect(probe({ 'com.apple.quicktime.location.iso6709': '+40.9903+029.0290+012.000/' })).toMatchObject({ lat: 40.9903, lon: 29.029 });
    expect(probe({ 'location-eng': '+36.8841+030.7056/' })).toMatchObject({ lat: 36.8841, lon: 30.7056 });
    expect(probe({ 'com.samsung.android.location': '41.0082,28.9784' })).toMatchObject({ lat: 41.0082, lon: 28.9784 });
    expect(probe({ 'com.vendor.gps.position': '+48.8566+002.3522/' })).toMatchObject({ lat: 48.8566, lon: 2.3522 });
    expect(probe({ title: 'x' })).toMatchObject({ lat: null, lon: null });
  });

  it('spots messaging-app files', () => {
    expect(isMessagingName('IMG-20240101-WA0001.jpg')).toBe(true);
    expect(isMessagingName('VID-20240101-WA0012.mp4')).toBe(true);
    expect(isMessagingName('WhatsApp Image 2024-01-01 at 12.00.00.jpeg')).toBe(true);
    expect(isMessagingName('photo_2024-01-01_12-00-00.jpg')).toBe(true);
    expect(isMessagingName('x.jpg', 'Telegram/Telegram Images')).toBe(true);
    expect(isMessagingName('IMG_20240101_120000.jpg')).toBe(false);
  });
});

describe('tiles', () => {
  it('styles, bounds and upstream URLs', () => {
    expect(tileStyleOf('carto')).toBe('osm');
    expect(tileStyleOf('topo')).toBe('topo');
    expect(tileStyleOf('off')).toBeNull();
    expect(isValidTile('osm', 0, 0, 0)).toBe(true);
    expect(isValidTile('osm', 3, 7, 7)).toBe(true);
    expect(isValidTile('osm', 3, 8, 0)).toBe(false);
    expect(isValidTile('osm', 20, 0, 0)).toBe(false);
    expect(isValidTile('topo', 18, 0, 0)).toBe(false); // OpenTopoMap stops at 17
    expect(isValidTile('osm', 2, -1, 0)).toBe(false);
    expect(isValidTile('osm', 2.5, 0, 0)).toBe(false);
    expect(upstreamTileUrl('osm', 3, 4, 2)).toBe('https://tile.openstreetmap.org/3/4/2.png');
    expect(upstreamTileUrl('satellite', 3, 4, 2)).toBe('https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/3/2/4');
    expect(upstreamTileUrl('topo', 3, 4, 2)).toMatch(/^https:\/\/[abc]\.tile\.opentopomap\.org\/3\/4\/2\.png$/);
  });

  it('web mercator tile of a point', () => {
    expect(tileOf(0, 0, 1)).toMatchObject({ x: 1, y: 1 });
    const t = tileOf(41.0082, 28.9784, 14);
    expect(t).toMatchObject({ x: 9510, y: 6142 });
    expect(t.px).toBeGreaterThanOrEqual(0);
    expect(t.px).toBeLessThan(256);
  });
});

describe('insights', () => {
  const row = (k: number, o: Partial<InsightRow> = {}): InsightRow => ({
    id: String(k),
    kind: 'image',
    localAt: Date.UTC(2024, k % 12, 1 + (k % 27), 12),
    size: 1000 + k,
    width: 400,
    height: 300,
    duration: null,
    lat: null,
    lon: null,
    cc: null,
    placeKey: null,
    city: null,
    flags: 0,
    mtime: 1_700_000_000_000 + k,
    ...o,
  });

  it('orientation and video length buckets', () => {
    expect(orientationOf(300, 400)).toBe('portrait');
    expect(orientationOf(400, 300)).toBe('landscape');
    expect(orientationOf(400, 390)).toBe('square');
    expect(orientationOf(4000, 1000, 4)).toBe('panorama');
    expect(orientationOf(null, 100)).toBeNull();
    expect(videoLengthOf(3)).toBe('short');
    expect(videoLengthOf(60)).toBeNull();
    expect(videoLengthOf(600)).toBe('long');
    expect(videoLengthOf(null)).toBeNull();
  });

  it('aggregates stats, countries, cities, facets and rankings', () => {
    const rows: InsightRow[] = [];
    for (let k = 0; k < 40; k++) {
      rows.push(
        row(k, {
          kind: k % 10 === 0 ? 'video' : 'image',
          duration: k % 10 === 0 ? (k === 0 ? 5 : 400) : null,
          width: k % 4 === 0 ? 300 : 400,
          height: k % 4 === 0 ? 400 : 300,
          lat: k % 3 ? 41 : null,
          lon: k % 3 ? 29 : null,
          cc: k % 3 ? (k % 2 ? 'TR' : 'IT') : null,
          placeKey: k % 3 ? (k % 2 ? 'TR-34' : 'IT-16') : null,
          city: k % 3 ? (k % 2 ? 'Kadıköy' : 'Firenze') : null,
          color: k % 8,
          sun: k % 5 === 0 ? -20 : k % 5 === 1 ? 2 : 40,
          addedAt: k === 7 ? 9e12 : null,
        }),
      );
    }
    rows.sort((a, b) => b.localAt - a.localAt);
    const r = computeInsights(rows);
    expect(r.stats).toMatchObject({ photos: 36, videos: 4, countries: 2, cities: 2, firstYear: 2024, lastYear: 2024, withLocation: 26 });
    expect(r.noLocation).toBe(14);
    expect(r.countries!.map((c) => c.cc).sort()).toEqual(['IT', 'TR']);
    expect(r.cities!.find((c) => c.name === 'Kadıköy')).toMatchObject({ key: 'TR-34|Kadıköy', placeKey: 'TR-34' });
    expect(r.dayTimes!.map((g) => g.value)).toEqual(['night', 'golden']);
    expect(r.dayTimes![0].count).toBe(8);
    expect(r.colors!.length).toBe(8);
    expect(r.orientations!.find((g) => g.value === 'portrait')!.count).toBe(10);
    expect(r.videoLengths).toEqual([
      expect.objectContaining({ value: 'short', count: 1 }),
      expect.objectContaining({ value: 'long', count: 3 }),
    ]);
    expect(r.largest!.map((x) => x.value)).toEqual([...rows].map((x) => x.size).sort((a, b) => b - a).slice(0, 20));
    expect(r.recent![0].id).toBe('7');
    expect(r.calendarYears).toEqual([2024]);
    expect(r.yearReviews).toHaveLength(1);
    expect(computeInsights(rows, { colors: false, dayTimes: false }).colors).toBeUndefined();
  });

  it('year in review spreads covers over the months', () => {
    const items = Array.from({ length: 60 }, (_, k) => ({ id: `i${k}`, localAt: Date.UTC(2023, k % 12, 1 + (k % 27)) }));
    items.sort((a, b) => b.localAt - a.localAt);
    const m = yearReview(2023, items)!;
    expect(m).toMatchObject({ id: 'year-2023', kind: 'year', count: 60, from: Date.UTC(2023, 0, 1), to: Date.UTC(2024, 0, 1) });
    expect(m.months).toEqual(new Array(12).fill(5));
    const months = new Set(m.coverIds.map((id) => new Date(items.find((i) => i.id === id)!.localAt).getUTCMonth()));
    expect(months.size).toBe(12);
    expect(yearReview(2023, items.slice(0, 5))).toBeNull();
  });

  it('calendar of a (leap) year', () => {
    const c = calendarOf(2024, [Date.UTC(2024, 0, 1, 5), Date.UTC(2024, 0, 1, 23), Date.UTC(2024, 11, 31, 12), Date.UTC(2023, 11, 31)]);
    expect(c.days).toHaveLength(366);
    expect(c.days[0]).toBe(2);
    expect(c.days[365]).toBe(1);
    expect(c).toMatchObject({ total: 3, max: 2 });
    expect(calendarDayKey(2024, 59)).toBe('2024-02-29');
    expect(calendarOf(2023, []).days).toHaveLength(365);
  });
});
