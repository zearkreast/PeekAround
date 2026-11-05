import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { describe, expect, it } from 'vitest';
import {
  classify,
  createGeoIndex,
  findTrips,
  foldText,
  homesByYear,
  instantToLocal,
  isScreenshotName,
  isValidGps,
  localToInstant,
  MEDIA_FLAG,
  normalizeCamera,
  offsetFromGps,
  onThisDay,
  parseDateTimeWithOffset,
  parseExifDateTime,
  parseGpsDateTime,
  parseIso6709,
  parseOffset,
  parseSearch,
  resolveFromInstant,
  resolveFromLocal,
  tzOffsetAt,
  type TripItem,
} from '@peekaround/shared';

const DAY = 86_400_000;
const L = (iso: string) => Date.parse(`${iso}Z`); // wall clock as UTC

describe('time', () => {
  it('parses EXIF strings as wall clock', () => {
    expect(parseExifDateTime('2024:07:01 23:30:00')).toBe(L('2024-07-01T23:30:00'));
    expect(parseExifDateTime('2024-07-01T23:30:00.250')).toBe(L('2024-07-01T23:30:00.250'));
    expect(parseExifDateTime('2024:07:01')).toBe(L('2024-07-01T00:00:00'));
    expect(parseExifDateTime('0000:00:00 00:00:00')).toBeNull();
    expect(parseExifDateTime('garbage')).toBeNull();
    expect(parseDateTimeWithOffset('2024-07-03T21:00:00+0300')).toEqual({ local: L('2024-07-03T21:00:00'), offset: 180 });
  });

  it('parses offsets', () => {
    expect(parseOffset('+03:00')).toBe(180);
    expect(parseOffset('-05:30')).toBe(-330);
    expect(parseOffset('+0545')).toBe(345);
    expect(parseOffset('Z')).toBe(0);
    expect(parseOffset('+20:00')).toBeNull();
  });

  it('computes DST-correct offsets with Intl', () => {
    expect(tzOffsetAt(Date.parse('2024-01-15T12:00:00Z'), 'Europe/Berlin')).toBe(60);
    expect(tzOffsetAt(Date.parse('2024-07-15T12:00:00Z'), 'Europe/Berlin')).toBe(120);
    expect(tzOffsetAt(Date.parse('2024-07-15T12:00:00Z'), 'Europe/Istanbul')).toBe(180);
    expect(tzOffsetAt(Date.parse('2024-07-15T12:00:00Z'), 'America/New_York')).toBe(-240);
    expect(tzOffsetAt(Date.parse('2024-07-15T12:00:00Z'), 'Asia/Kolkata')).toBe(330);
    // around the switch (2024-03-31 01:00 UTC)
    expect(tzOffsetAt(Date.parse('2024-03-31T00:59:00Z'), 'Europe/Berlin')).toBe(60);
    expect(tzOffsetAt(Date.parse('2024-03-31T01:00:00Z'), 'Europe/Berlin')).toBe(120);
  });

  it('round-trips local ↔ instant', () => {
    for (const tz of ['Europe/Istanbul', 'Europe/Berlin', 'America/Los_Angeles', 'UTC']) {
      for (const iso of ['2024-01-10T08:00:00Z', '2024-07-10T23:30:00Z', '2023-10-29T00:30:00Z']) {
        const t = Date.parse(iso);
        expect(localToInstant(instantToLocal(t, tz), tz)).toBe(t);
      }
    }
    // a 23:30 Istanbul photo stays on its day
    const r = resolveFromLocal(L('2024-07-01T23:30:00'), 180, 'UTC');
    expect(r).toEqual({ takenAt: Date.parse('2024-07-01T20:30:00Z'), localAt: L('2024-07-01T23:30:00'), tzOffset: 180 });
    // no offset → home zone decides the instant only
    expect(resolveFromLocal(L('2024-07-01T23:30:00'), null, 'Europe/Berlin').takenAt).toBe(Date.parse('2024-07-01T21:30:00Z'));
    // video (UTC creation_time) shown in home zone
    expect(resolveFromInstant(Date.parse('2025-12-31T21:30:00Z'), 'Europe/Istanbul').localAt).toBe(L('2026-01-01T00:30:00'));
    // DST gap (02:30 does not exist in Berlin on 2024-03-31) → shifted forward; repeated hour → earlier instant
    expect(localToInstant(L('2024-03-31T02:30:00'), 'Europe/Berlin')).toBe(Date.parse('2024-03-31T01:30:00Z'));
    expect(localToInstant(L('2024-10-27T02:30:00'), 'Europe/Berlin')).toBe(Date.parse('2024-10-27T00:30:00Z'));
  });

  it('derives the offset from GPS time, rounded to 15 minutes', () => {
    const gps = parseGpsDateTime('2024:07:01', [20, 29, 47]);
    expect(gps).toBe(Date.parse('2024-07-01T20:29:47Z'));
    expect(offsetFromGps(L('2024-07-01T23:30:00'), gps!)).toBe(180);
    expect(offsetFromGps(L('2024-07-01T02:15:00'), Date.parse('2024-06-30T20:30:00Z'))).toBe(345);
    expect(offsetFromGps(L('2024-07-01T02:15:00'), Date.parse('2024-06-20T20:30:00Z'))).toBeNull();
    expect(parseGpsDateTime('2024:07:01', '10:00:00')).toBe(Date.parse('2024-07-01T10:00:00Z'));
  });
});

describe('geo', () => {
  const asset = path.resolve(__dirname, '../../web/public/geo/cities.bin');
  const geo = createGeoIndex(zlib.gunzipSync(fs.readFileSync(asset)));

  it('finds Turkish districts with local spelling', () => {
    expect(geo.nearest(40.9903, 29.029)).toMatchObject({ city: 'Kadıköy', admin1: 'İstanbul', cc: 'TR', key: 'TR-34' });
    const kizilay = geo.nearest(39.9208, 32.8541)!;
    expect(kizilay).toMatchObject({ admin1: 'Ankara', cc: 'TR', key: 'TR-68' });
    expect(['Çankaya', 'Ankara']).toContain(kizilay.city);
    expect(geo.nearest(36.8969, 30.7133)).toMatchObject({ admin1: 'Antalya', cc: 'TR', key: 'TR-07' });
  });

  it('works abroad and rejects the ocean / null island', () => {
    const b = geo.nearest(52.52, 13.405)!;
    expect(b).toMatchObject({ cc: 'DE', admin1: 'Berlin' });
    expect(['Mitte', 'Berlin']).toContain(b.city);
    expect(geo.nearest(30, -40)).toBeNull(); // mid Atlantic
    expect(geo.nearest(0, 0)).toBeNull();
    expect(geo.nearest(NaN, 10)).toBeNull();
    expect(geo.attribution).toMatch(/GeoNames/);
  });

  it('respects the 80 km limit', () => {
    const m = geo.nearest(40.9903, 29.029)!;
    expect(m.km).toBeLessThan(5);
    // Ankara → Istanbul is ~350 km: with a 1 km limit a remote point finds nothing
    expect(geo.nearest(34.5, 32.5, 1)).toBeNull();
  });

  it('parses ISO 6709 and validates GPS', () => {
    expect(parseIso6709('+41.0123+028.9744+050.000/')).toEqual({ lat: 41.0123, lon: 28.9744 });
    expect(parseIso6709('-33.8688+151.2093/')).toEqual({ lat: -33.8688, lon: 151.2093 });
    expect(parseIso6709('+4100.738+02858.464/')!.lat).toBeCloseTo(41.0123, 3);
    expect(parseIso6709('+00.0000+000.0000/')).toBeNull();
    expect(parseIso6709('nope')).toBeNull();
    expect(isValidGps(91, 0)).toBe(false);
    expect(isValidGps(0, 0)).toBe(false);
    expect(isValidGps(41, 29)).toBe(true);
  });
});

describe('classify', () => {
  const base = { ext: 'png', kind: 'image', width: 1080, height: 2340 };
  it('detects screenshots by name in many languages and by folder', () => {
    for (const name of [
      'Screenshot_20240812-101500.png',
      'Screen Shot 2023-05-02 at 10.11.12.png',
      'Ekran Görüntüsü 2025-03-01 141230.png',
      'Ekran Resmi 2024-01-01 10.00.00.png',
      'Bildschirmfoto 2025-04-19 um 09.30.00.png',
      'Capture d’écran 2024-02-02 à 10.00.00.png',
      'Captura de pantalla 2024-02-02.png',
      'Скриншот 2024-02-02.png',
      'スクリーンショット 2024-02-02.png',
    ]) {
      expect(isScreenshotName(name), name).toBe(true);
    }
    expect(isScreenshotName('IMG_0001.png', 'DCIM/Screenshots')).toBe(true);
    expect(isScreenshotName('a.png', 'Resimler/Ekran görüntüleri')).toBe(true);
    expect(isScreenshotName('IMG_0001.jpg', 'DCIM/Camera')).toBe(false);
    // phone-shaped PNG without camera EXIF
    expect(classify({ ...base, name: 'IMG_7781.png' }) & MEDIA_FLAG.screenshot).toBeTruthy();
    expect(classify({ ...base, name: 'IMG_7781.png', make: 'Apple' }) & MEDIA_FLAG.screenshot).toBeFalsy();
    expect(classify({ ...base, name: 'logo.png', width: 400, height: 400 })).toBe(0);
  });

  it('detects selfies, panoramas, animations', () => {
    const jpg = { ext: 'jpg', kind: 'image', name: 'IMG_1.jpg', width: 3000, height: 4000, make: 'samsung', model: 'SM-G991B' };
    expect(classify({ ...jpg, lens: 'Samsung Galaxy S21 front camera 2.2mm f/2.2' })).toBe(MEDIA_FLAG.selfie);
    expect(classify({ ...jpg, lens: 'iPhone 13 front TrueDepth camera 2.71mm f/2.2' })).toBe(MEDIA_FLAG.selfie);
    expect(classify({ ...jpg, name: 'IMG_20240101_120000_selfie.jpg' })).toBe(MEDIA_FLAG.selfie);
    expect(classify({ ...jpg, lens: 'back camera' })).toBe(0);
    expect(classify({ ...jpg, width: 9000, height: 3000 })).toBe(MEDIA_FLAG.panorama);
    expect(classify({ ...jpg, width: 3000, height: 9000 })).toBe(MEDIA_FLAG.panorama); // vertical
    expect(classify({ ...jpg, width: 2000, height: 1000, gpano: true })).toBe(MEDIA_FLAG.panorama);
    expect(classify({ ...jpg, width: 2000, height: 800, make: null, model: null })).toBe(0); // banner without camera
    expect(classify({ ext: 'gif', kind: 'image', name: 'a.gif' })).toBe(MEDIA_FLAG.animated);
    expect(classify({ ext: 'webp', kind: 'image', name: 'a.webp', pages: 10 })).toBe(MEDIA_FLAG.animated);
    expect(classify({ ext: 'webp', kind: 'image', name: 'a.webp', pages: 1 })).toBe(0);
    expect(classify({ ext: 'mp4', kind: 'video', name: 'a.mp4' })).toBe(0);
  });

  it('normalises camera names', () => {
    expect(normalizeCamera('NIKON CORPORATION', 'NIKON D750')).toEqual({ make: 'Nikon', model: 'D750', key: 'nikon|d750' });
    expect(normalizeCamera('SAMSUNG', 'SM-G991B')).toEqual({ make: 'Samsung', model: 'SM-G991B', key: 'samsung|sm-g991b' });
    expect(normalizeCamera('samsung', 'Galaxy S21')).toMatchObject({ make: 'Samsung', model: 'Galaxy S21' });
    expect(normalizeCamera('Canon', 'Canon EOS R5')).toMatchObject({ make: 'Canon', model: 'EOS R5' });
    expect(normalizeCamera('Apple', 'iPhone 13')).toMatchObject({ make: 'Apple', model: 'iPhone 13' });
    expect(normalizeCamera('OLYMPUS IMAGING CORP.', 'E-M10')).toMatchObject({ make: 'Olympus', model: 'E-M10' });
    expect(normalizeCamera('FOO ELECTRONICS', 'X1')).toMatchObject({ make: 'Foo', model: 'X1' });
    expect(normalizeCamera(null, null)).toEqual({ make: null, model: null, key: null });
  });
});

describe('search', () => {
  it('folds Turkish case and diacritics', () => {
    for (const s of ['ISTANBUL', 'istanbul', 'İstanbul', 'İSTANBUL', 'ıstanbul']) expect(foldText(s)).toBe('istanbul');
    expect(foldText('Kadıköy ŞİŞLİ Çağlayan Göztepe Ümraniye')).toBe('kadikoy sisli caglayan goztepe umraniye');
    expect(foldText('Düsseldorf Straße')).toBe('dusseldorf strasse');
  });

  it('extracts dates, months and types', () => {
    expect(parseSearch('istanbul 2023 video')).toEqual({
      text: 'istanbul',
      terms: ['istanbul'],
      flag: 'video',
      from: Date.UTC(2023, 0, 1),
      to: Date.UTC(2024, 0, 1),
    });
    expect(parseSearch('Ağustos 2024')).toMatchObject({ terms: [], from: Date.UTC(2024, 7, 1), to: Date.UTC(2024, 8, 1) });
    expect(parseSearch('haziran')).toMatchObject({ terms: [], month: 6 });
    expect(parseSearch('June selfie')).toMatchObject({ month: 6, flag: 'selfie' });
    expect(parseSearch('ekran görüntüleri')).toMatchObject({ terms: [], flag: 'screenshot' });
    expect(parseSearch('EKRAN GÖRÜNTÜSÜ 2025')).toMatchObject({ flag: 'screenshot', from: Date.UTC(2025, 0, 1) });
    expect(parseSearch('panorama kapadokya')).toMatchObject({ terms: ['kapadokya'], flag: 'panorama' });
    expect(parseSearch('animasyon')).toMatchObject({ flag: 'animated' });
    expect(parseSearch('gif')).toMatchObject({ flag: 'animated' });
    // abbreviations only next to a year; "ara" alone is a word
    expect(parseSearch('ara')).toMatchObject({ terms: ['ara'] });
    expect(parseSearch('ara 2024')).toMatchObject({ from: Date.UTC(2024, 11, 1), to: Date.UTC(2025, 0, 1) });
    expect(parseSearch('IMG_2400.jpg')).toMatchObject({ terms: ['img_2400.jpg'] });
  });
});

describe('memories', () => {
  it('finds "on this day" ±3 days in previous years with ≥ 3 items', () => {
    const now = L('2026-11-05T10:00:00');
    const items = [
      ...[-3, 0, 2].map((d, i) => ({ id: `a${i}`, localAt: L('2025-11-05T12:00:00') + d * DAY })),
      ...[0, 1].map((d, i) => ({ id: `b${i}`, localAt: L('2024-11-05T12:00:00') + d * DAY })), // only 2
      ...[0, 0, 1, -1].map((d, i) => ({ id: `c${i}`, localAt: L('2022-11-04T09:00:00') + d * DAY })),
      { id: 'far', localAt: L('2025-11-09T12:00:00') }, // 4 days later → outside
      { id: 'now', localAt: L('2026-11-05T08:00:00') }, // this year
    ];
    const m = onThisDay(items, now);
    expect(m.map((x) => [x.yearsAgo, x.count])).toEqual([
      [1, 3],
      [4, 4],
    ]);
    expect(m[0].from).toBe(L('2025-11-02T00:00:00'));
    expect(m[0].to).toBe(L('2025-11-09T00:00:00'));
    expect(m[0].coverIds).toEqual(['a0', 'a1', 'a2']);
    // deterministic
    expect(onThisDay([...items].reverse(), now)).toEqual(m);
  });

  it('handles Feb 29', () => {
    const items = [0, 1, 2].map((i) => ({ id: String(i), localAt: L('2027-02-28T10:00:00') + i * 3600e3 }));
    expect(onThisDay(items, L('2028-02-29T10:00:00'))).toHaveLength(1);
  });

  it('a window that reaches into the next year still counts (Dec 31 ↔ Jan 2)', () => {
    const items = [0, 1, 2].map((i) => ({ id: `j${i}`, localAt: L('2025-01-02T10:00:00') + i * 3600e3 }));
    expect(onThisDay(items, L('2025-12-31T12:00:00')).map((m) => [m.yearsAgo, m.count])).toEqual([[1, 3]]);
    const dec = [0, 1, 2].map((i) => ({ id: `d${i}`, localAt: L('2024-12-30T10:00:00') + i * 3600e3 }));
    expect(onThisDay(dec, L('2026-01-01T12:00:00')).map((m) => [m.yearsAgo, m.count])).toEqual([[1, 3]]);
    expect(onThisDay(dec, L('2026-07-01T12:00:00'))).toEqual([]);
  });
});

describe('trips', () => {
  const IST = { placeKey: 'TR-34', placeName: 'İstanbul', cc: 'TR', lat: 41.01, lon: 29.0 };
  const ANK = { placeKey: 'TR-68', placeName: 'Ankara', cc: 'TR', lat: 39.92, lon: 32.85 };
  const ANT = { placeKey: 'TR-07', placeName: 'Antalya', cc: 'TR', lat: 36.88, lon: 30.7 };
  const BER = { placeKey: 'DE-16', placeName: 'Berlin', cc: 'DE', lat: 52.52, lon: 13.4 };
  let n = 0;
  const at = (iso: string, p: object, count = 1): TripItem[] =>
    Array.from({ length: count }, (_, i) => ({ id: `i${n++}`, localAt: L(iso) + i * 3600e3, ...p }));
  /** a photo on every 3rd day of a range at a place */
  const life = (fromIso: string, days: number, p: object) => Array.from({ length: Math.ceil(days / 3) }, (_, k) => at(new Date(L(fromIso) + k * 3 * DAY).toISOString().slice(0, 19), p)).flat();

  it('detects a trip away from home', () => {
    const items = [
      ...life('2024-01-01T12:00:00', 360, IST),
      ...at('2024-07-02T10:00:00', ANT, 4),
      ...at('2024-07-03T10:00:00', ANT, 3),
      ...at('2024-07-05T10:00:00', ANT, 2), // 1 empty day in between is fine
      { id: 'nogps', localAt: L('2024-07-04T10:00:00') },
    ];
    const trips = findTrips(items);
    expect(trips).toHaveLength(1);
    expect(trips[0]).toMatchObject({ kind: 'trip', place: 'Antalya', cc: 'TR', from: L('2024-07-02T00:00:00'), to: L('2024-07-06T00:00:00') });
    // every item inside the range counts — including the one without GPS
    expect(trips[0].count).toBe(items.filter((i) => i.localAt >= trips[0].from && i.localAt < trips[0].to).length);
    expect(trips[0].count).toBeGreaterThanOrEqual(10);
  });

  it('needs ≥ 2 days or ≥ 10 items', () => {
    const base = life('2024-01-01T12:00:00', 360, IST);
    expect(findTrips([...base, ...at('2024-05-10T10:00:00', ANK, 5)])).toHaveLength(0);
    expect(findTrips([...base, ...at('2024-05-10T08:00:00', ANK, 10)])).toHaveLength(1);
    // a home day between two away days splits them
    const split = findTrips([...base, ...at('2024-05-10T08:00:00', BER, 3), ...at('2024-05-11T08:00:00', IST, 1), ...at('2024-05-12T08:00:00', BER, 3)]);
    expect(split).toHaveLength(0);
  });

  it('computes home per year: moving after a year', () => {
    const items = [
      ...life('2023-01-01T12:00:00', 360, IST),
      ...life('2024-01-01T12:00:00', 360, ANK),
      ...at('2024-08-10T10:00:00', IST, 6),
      ...at('2024-08-11T10:00:00', IST, 6),
    ];
    const homes = homesByYear(items);
    expect(homes.get(2023)![0].key).toBe('TR-34');
    expect(homes.get(2024)![0].key).toBe('TR-68');
    const trips = findTrips(items);
    // visiting the old city is a trip now
    expect(trips.map((t) => t.place)).toEqual(['İstanbul']);
  });

  it('moving city mid-year does not create a giant trip', () => {
    const items = [
      ...life('2024-01-01T12:00:00', 150, IST),
      ...life('2024-06-01T12:00:00', 210, ANK),
      ...at('2024-09-10T10:00:00', BER, 6),
      ...at('2024-09-11T10:00:00', BER, 6),
    ];
    const homes = homesByYear(items).get(2024)!.map((h) => h.key);
    expect(homes).toEqual(expect.arrayContaining(['TR-34', 'TR-68']));
    expect(findTrips(items).map((t) => t.place)).toEqual(['Berlin']);
  });
});

describe('parsers stay linear on hostile input (ReDoS)', () => {
  const fast = (f: () => unknown) => {
    const t = performance.now();
    f();
    return performance.now() - t;
  };
  it('date, ISO 6709 and search parsing', () => {
    expect(fast(() => expect(parseDateTimeWithOffset(`2024:01:01${' '.repeat(50_000)}x`)).toBeNull())).toBeLessThan(100);
    expect(parseDateTimeWithOffset('  2024:07:01 10:00:00  ')).toEqual({ local: Date.UTC(2024, 6, 1, 10), offset: null });
    expect(fast(() => expect(parseIso6709(`+1+1CRS${' '.repeat(50_000)}x`)).toBeNull())).toBeLessThan(100);
    expect(fast(() => parseSearch(`${'.'.repeat(200_000)}a ${'-'.repeat(200_000)}`))).toBeLessThan(500);
    expect(fast(() => normalizeCamera(`x${' ,'.repeat(30_000)}co ltdx`, 'M'))).toBeLessThan(100);
    expect(normalizeCamera('NIKON CORPORATION', 'NIKON D750')).toMatchObject({ make: 'Nikon', model: 'D750' });
    expect(parseSearch('..istanbul.. video')).toMatchObject({ terms: ['istanbul'], flag: 'video' });
  });
});
