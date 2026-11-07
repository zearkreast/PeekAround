// Golden fixtures for pa-core: calls the TypeScript reference (packages/shared) on generated inputs.
// Run: cd /home/user/peekaround && TZ=UTC node_modules/.bin/tsx <this file> <outDir>
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import * as S from '/home/user/peekaround/packages/shared/src/index.ts';
import { rgbaToThumbHash } from '/home/user/peekaround/node_modules/thumbhash/thumbhash.js';

const outDir = process.argv[2];
fs.mkdirSync(outDir, { recursive: true });

// deterministic PRNG
let seed = 0x9e3779b9;
const rnd = () => {
  seed |= 0;
  seed = (seed + 0x6d2b79f5) | 0;
  let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
  t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
};
const ri = (a: number, b: number) => Math.floor(a + rnd() * (b - a + 1));
const pick = <T,>(a: T[]): T => a[Math.floor(rnd() * a.length)];
const range = (n: number) => Array.from({ length: n }, (_, i) => i);

/** JSON-safe encoding: undefined → {"$u":1}, NaN/±Infinity → {"$n":"NaN"} */
const enc = (v: unknown): unknown => {
  if (v === undefined) return { $u: 1 };
  if (typeof v === 'number' && !Number.isFinite(v)) return { $n: String(v) };
  if (v instanceof Map) return { $map: [...v.entries()].map(([k, x]) => [enc(k), enc(x)]) };
  if (v instanceof Uint8Array) return { $bytes: [...v] };
  if (Array.isArray(v)) return v.map(enc);
  if (v && typeof v === 'object') {
    const o: Record<string, unknown> = {};
    for (const [k, x] of Object.entries(v)) if (x !== undefined) o[k] = enc(x);
    return o;
  }
  return v;
};

const files: Record<string, { args: unknown[]; out: unknown }[]> = {};
const add = (fn: string, args: unknown[], f: (...a: any[]) => unknown) => {
  let out: unknown;
  try {
    out = f(...args);
  } catch (e) {
    out = { $err: String((e as Error).message) };
  }
  (files[fn] ??= []).push({ args: args.map(enc), out: enc(out) });
};

const L = (iso: string) => Date.parse(`${iso}Z`);
const DAY = 86_400_000;

// ───────────── time ─────────────
const dateStrings = [
  '2024:07:01 23:30:00', '2024-07-01T23:30:00.250', '2024:07:01', '0000:00:00 00:00:00', 'garbage', '',
  '2024-07-03T21:00:00+0300', '2024-07-03T21:00:00+03:00', '2024-07-03T21:00:00-05:30', '2024-07-03T21:00:00Z',
  '2024-07-03t21:00:00z', '  2024:07:01 10:00:00  ', '2024:7:1 1:00', '2024:07:01 10:00', '2024:07:01 10:00:5',
  '2024:07:01 10:00:00.', '2024:07:01 10:00:00.9995', '2024:07:01 10:00:00,5', '2024/07/01 10:00:00',
  '1970:01:01 00:00:00', '1971:01:01 00:00:00', '2200:12:31 23:59:59', '2201:01:01 00:00:00', '2024:13:01 00:00:00',
  '2024:02:31 00:00:00', '2024:02:29 12:00:00', '2024:00:10 00:00:00', '2024:01:32 00:00:00', '2024:07:01 24:00:00',
  '2024:07:01 99:99:99', '2024:07:01 10:00:00 +03:00', '2024:07:01+03:00', '2024:07:01 10:00:00+3', '2024:07:01 10:00:00+15:00',
  '2024:07:01 10:00:00+14:00', '2024:07:01 10:00:00-14:01', '2024:07:01 123:00', '2024:07:01 10:00:00:00', '2024:07:011',
  '2024:07:01\t10:00:00', '2024:07:01T10:00:00.123456789', `2024:01:01${' '.repeat(70)}x`, '2024-07-01 10:00:00 Z', '20240701',
  '2024:07:01 10:00:00+0', '2024:07:01 10:00:00 + 03:00', '2024:07:01 1:2:3', '2024:07:01 01:02:03.000', ' 2024:07:01 10:00:00　',
];
for (const s of dateStrings) add('parseDateTimeWithOffset', [s], S.parseDateTimeWithOffset);
for (let i = 0; i < 200; i++) {
  const y = ri(1965, 2210);
  const sep = pick([':', '-', '/']);
  const s = `${y}${sep}${String(ri(0, 13)).padStart(pick([1, 2]), '0')}${sep}${String(ri(0, 32)).padStart(2, '0')}${pick(['', ` ${ri(0, 25)}:${String(ri(0, 60)).padStart(2, '0')}`, `T${ri(0, 23)}:${String(ri(0, 59)).padStart(2, '0')}:${String(ri(0, 59)).padStart(2, '0')}${pick(['', '.5', '.123', ',25'])}`])}${pick(['', 'Z', '+03:00', '-0530', '+1400', ' +01:00', '+2'])}`;
  add('parseDateTimeWithOffset', [s], S.parseDateTimeWithOffset);
}
for (const s of ['+03:00', '-05:30', '+0545', 'Z', 'z', ' Z ', '+20:00', '+14:00', '-14:00', '+14:01', '+3', '+03', '+030', '+03:0', '+03:', '+1:30', '03:00', '', 'abc', '+0300 ', '-0', '+12345', '+99'])
  add('parseOffset', [s], S.parseOffset);
const zones = ['UTC', 'Europe/Istanbul', 'Europe/Berlin', 'America/New_York', 'America/Los_Angeles', 'Asia/Kolkata', 'Asia/Kathmandu', 'Australia/Lord_Howe', 'Pacific/Chatham', 'America/Sao_Paulo', 'Africa/Casablanca', 'Asia/Tehran', 'Pacific/Apia', 'America/St_Johns', 'Europe/London', 'Asia/Tokyo', 'Etc/GMT+3', 'Asia/Calcutta', 'US/Eastern', 'europe/istanbul'];
for (const tz of [...zones, 'Not/AZone', '', 'Mars/Phobos', '+03:00', '-05:30', '+0300', '+03', 'utc', 'Etc/UTC', 'GMT', 'EST5EDT', 'Europe/Kiev', 'Europe/Kyiv'])
  add('isValidTimeZone', [tz], S.isValidTimeZone);
const instants = [
  Date.parse('2024-01-15T12:00:00Z'), Date.parse('2024-07-15T12:00:00Z'), Date.parse('2024-03-31T00:59:00Z'), Date.parse('2024-03-31T01:00:00Z'),
  Date.parse('2024-10-27T00:59:59Z'), Date.parse('2024-10-27T01:00:00Z'), Date.parse('2024-03-10T06:59:59Z'), Date.parse('2024-03-10T07:00:00Z'),
  Date.parse('1900-01-01T00:00:00Z'), Date.parse('1905-06-01T00:00:00Z'), Date.parse('1969-12-31T23:59:59.999Z'), 0, -1, 1500, Date.parse('2038-01-19T03:14:08Z'),
  Date.parse('2100-07-01T00:00:00Z'), Date.parse('2011-12-30T09:59:59Z'), Date.parse('2011-12-30T10:00:00Z'),
];
for (let i = 0; i < 150; i++) instants.push(Date.UTC(ri(1950, 2060), ri(0, 11), ri(1, 28), ri(0, 23), ri(0, 59), ri(0, 59), ri(0, 999)));
for (const tz of [...zones, 'Not/AZone', '+03:00']) for (const t of instants) add('tzOffsetAt', [t, tz], S.tzOffsetAt);
for (const tz of zones) for (const t of instants.slice(0, 60)) add('instantToLocal', [t, tz], S.instantToLocal);
const locals = [
  L('2024-03-31T02:30:00'), L('2024-10-27T02:30:00'), L('2024-03-10T02:30:00'), L('2024-11-03T01:30:00'), L('2024-07-01T23:30:00'),
  L('2011-12-30T12:00:00'), L('2011-12-29T23:30:00'), L('1900-01-01T00:00:00'), L('2024-04-07T02:30:00'), L('2024-10-06T02:15:00'),
];
for (let i = 0; i < 80; i++) locals.push(Date.UTC(ri(1950, 2060), ri(0, 11), ri(1, 28), ri(0, 23), pick([0, 15, 30, 45, ri(0, 59)])));
for (const tz of [...zones, 'Not/AZone']) for (const t of locals) add('localToInstant', [t, tz], S.localToInstant);
for (const [a, b] of [[L('2024-07-01T23:30:00'), Date.parse('2024-07-01T20:29:47Z')], [L('2024-07-01T02:15:00'), Date.parse('2024-06-30T20:30:00Z')], [L('2024-07-01T02:15:00'), Date.parse('2024-06-20T20:30:00Z')], [0, 7.5 * 60000], [0, -7.5 * 60000], [0, 14 * 3600e3], [0, 14 * 3600e3 + 450000]])
  add('offsetFromGps', [a, b], S.offsetFromGps);
for (let i = 0; i < 60; i++) {
  const l = ri(0, 2e12);
  add('offsetFromGps', [l, l - ri(-16 * 3600e3, 16 * 3600e3)], S.offsetFromGps);
}
for (const [d, t] of [['2024:07:01', [20, 29, 47]], ['2024:07:01', '10:00:00'], ['2024-07-01', '10:00:00.5'], ['2024:07:01', [20, 29]], ['2024:07:01', []], ['2024:07:01', [1, 2, 3.25]], ['2024:07:01', '1:2:3'], ['2024:07:01', 'x'], ['1970:01:01', [0, 0, 1]], ['1971:01:01', [0, 0, 0]], ['2024:7:01', [1, 2, 3]], ['2024:07:01 extra', '10:00:00zz'], ['  2024:07:01', ' 10:00:00'], ['2024:02:30', [25, 61, 61]], ['2024:07:01', '10:00:00.'], ['2024:07:01', '123:00:00']] as [string, unknown][])
  add('parseGpsDateTime', [d, t], S.parseGpsDateTime);
for (const tz of ['UTC', 'Europe/Berlin', 'Europe/Istanbul', 'America/New_York']) {
  for (const l of locals.slice(0, 30)) {
    add('resolveFromLocal', [l, null, tz], S.resolveFromLocal);
    add('resolveFromLocal', [l, 180, tz], S.resolveFromLocal);
    add('resolveFromInstant', [l, tz], S.resolveFromInstant);
  }
}
for (const t of [...instants.slice(0, 40), -DAY - 1, -DAY, 253402300799999]) {
  add('localDayKey', [t], S.localDayKey);
  add('localDayStart', [t], S.localDayStart);
}

// ───────────── geo ─────────────
const geo = S.createGeoIndex(zlib.gunzipSync(fs.readFileSync('/home/user/peekaround/packages/web/public/geo/cities.bin')));
const points: [number, number, number?][] = [
  [40.9903, 29.029], [39.9208, 32.8541], [36.8969, 30.7133], [52.52, 13.405], [30, -40], [0, 0], [0.00005, 0.00005], [0.0002, 0], [34.5, 32.5, 1],
  [-33.8688, 151.2093], [64.1466, -21.9426], [-54.8019, -68.303], [71.2906, -156.7887], [-16.5, 179.99], [-16.5, -179.99], [65.0, 179.9], [65.0, -179.95],
  [89.9, 0], [-89.9, 0], [90, 180], [-90, -180], [91, 0], [0, 181], [48.8566, 2.3522, 0.5], [41.0082, 28.9784, 200], [41.0082, 28.9784, 1000],
  [1.3521, 103.8198], [-0.0001, -0.0001], [37.9715, 23.7267], [38.7223, -9.1393], [55.7558, 37.6173], [35.6762, 139.6503], [40.7128, -74.006],
  [19.4326, -99.1332], [-34.6037, -58.3816], [-1.2921, 36.8219], [30.0444, 31.2357], [21.3069, -157.8583], [-41.2865, 174.7762], [78.2232, 15.6267],
];
for (let i = 0; i < 400; i++) points.push([rnd() * 180 - 90, rnd() * 360 - 180]);
for (let i = 0; i < 400; i++) points.push([rnd() * 30 + 35, rnd() * 40 - 5]); // Europe / Turkey (dense)
for (const [la, lo, mk] of points) add('geo.nearest', mk === undefined ? [la, lo] : [la, lo, mk], (a, b, c) => geo.nearest(a, b, c));
for (const id of [0, 1, 2, 100, 5000, geo.size - 1, geo.size, -1]) add('geo.place', [id], (i) => geo.place(i));
for (const [cc, code] of [['TR', '34'], ['TR', null], ['DE', '16'], ['XX', '00'], ['US', 'NY'], ['TR', '']] as [string, string | null][])
  add('geo.admin1Name', [cc, code], (a, b) => geo.admin1Name(a, b));
add('geo.info', [], () => ({ size: geo.size, attribution: geo.attribution }));
for (let i = 0; i < 100; i++) add('distanceKm', [rnd() * 180 - 90, rnd() * 360 - 180, rnd() * 180 - 90, rnd() * 360 - 180], S.distanceKm);
add('distanceKm', [41, 29, 41, 29], S.distanceKm);
add('distanceKm', [0, 179.9, 0, -179.9], S.distanceKm);
for (const [a, b] of [[91, 0], [0, 0], [41, 29], [0.00009, 0], [0.0001, 0], [-90, -180], [90, 180.0001], [NaN, 1]] as [number, number][]) add('isValidGps', [a, b], S.isValidGps);
for (const s of ['+41.0123+028.9744+050.000/', '-33.8688+151.2093/', '+4100.738+02858.464/', '+410044.28+0285827.84/', '+00.0000+000.0000/', 'nope', '+41.0123+028.9744', ' +41.0123+028.9744/ ', '+41.0123+028.9744CRSWGS_84/', '+41.0123+028.9744+12CRS/x', '+41+29/', '+41.+29/', '+91+029/', '+41.0123+028.9744 /', '+41.0123+028.9744//', `+1+1CRS${' '.repeat(300)}x`, '+48.8566+002.3522+035.000CRSWGS_84/', '-12.3456-077.1234/', '+4.5-74.1/'])
  add('parseIso6709', [s], S.parseIso6709);
const gpsVals: [unknown, unknown][] = [
  ['41,0.738N', undefined], ['28,58.476E', undefined], ['33,51,54.5S', undefined], ['70,30.0W', undefined], ['41.0123', undefined], ['-12.5', undefined],
  ['12.5', 'S'], [[41, 0, 44.28], 'N'], [28.97, 'W'], [`41°0'44.28"N`, undefined], ['N 41.5', undefined], ['41,75.0N', undefined], ['abc', undefined], ['', undefined],
  [undefined, undefined], [-28.97, 'E'], [[41], undefined], [[], 'N'], [[41, 'x'], 'N'], ['41 0 44.28', 'n'], ['s 12.5', 'N'], ['+12.5', 'w'], ['-12.5', 'N'],
  ['41,0,60', undefined], ['41.5.3', undefined], ['1,2,3,4', undefined], ['41′30″', 'S'], ['  41:30:00 N ', undefined], ['N', undefined], ['NN', undefined],
  ['41,0123', undefined], [41, ' south'], [41, 'west'], [null, undefined], [true, undefined], ['x'.repeat(70), undefined],
];
for (const [v, r] of gpsVals) add('parseGpsCoordinate', [v, r], S.parseGpsCoordinate);

// ───────────── classify ─────────────
const names = [
  'Screenshot_20240812-101500.png', 'Screen Shot 2023-05-02 at 10.11.12.png', 'Ekran Görüntüsü 2025-03-01 141230.png', 'Ekran Resmi 2024-01-01 10.00.00.png',
  'Bildschirmfoto 2025-04-19 um 09.30.00.png', 'Capture d’écran 2024-02-02 à 10.00.00.png', 'Captura de pantalla 2024-02-02.png', 'Скриншот 2024-02-02.png',
  'スクリーンショット 2024-02-02.png', 'IMG_0001.png', 'IMG_7781.png', 'logo.png', 'IMG_1.jpg', 'IMG_20240101_120000_selfie.jpg', 'PANO_20240705_181500.jpg',
  'selfie.MP4', 'a.gif', 'a.webp', 'SCREENSHOT.PNG', 'ekran görüntüsü.jpg', 'EKRAN GÖRÜNTÜSÜ.JPG', 'Snímek obrazovky.png', 'pano.jpg', 'IMG_SELFIE.HEIC', 'İstanbul.jpg',
];
const dirs = ['', 'DCIM/Screenshots', 'Resimler/Ekran görüntüleri', 'DCIM/Camera', 'Pictures/Screenshots/', 'x/SCREENSHOTS/y', 'Bildschirmfotos', "Captures d'écran"];
for (const n of names) for (const d of dirs) add('isScreenshotName', [n, d], S.isScreenshotName);
const makes = ['NIKON CORPORATION', 'SAMSUNG', 'samsung', 'Canon', 'Apple', 'OLYMPUS IMAGING CORP.', 'FOO ELECTRONICS', null, '', 'Google', 'HUAWEI', 'Xiaomi', 'LGE', 'LG Electronics', 'OnePlus', 'FUJIFILM', 'Fuji Photo Film Co., Ltd.', 'RICOH IMAGING COMPANY, LTD.', 'RICOH', 'PENTAX Corporation', 'Panasonic', 'SONY', 'HMD Global', 'GoPro', 'DJI', 'OM Digital Solutions', 'Leica Camera AG', 'Kodak', 'EASTMAN KODAK COMPANY', 'CASIO COMPUTER CO.,LTD.', 'Hasselblad', 'Nikonos', 'nikon', 'ACME DIGITAL CAMERA GMBH', 'ACME S.A.', 'ACME SA', 'X  CO LTD', 'ZEISS', 'motorola', 'asus', ' \0Apple\0 ', 'TECNO MOBILE LIMITED', 'Sony Ericsson', 'VIVO', 'realme', 'ZTE', 'TCL', 'HTC', 'OPPO', `x${' ,'.repeat(300)}co ltdx`, 'ΑΒΓ ΔΕΖ', 'ÉCLAIR-CAMÉRA', 'BIG/SMALL-CAM THING', 'AB'];
const models = ['NIKON D750', 'SM-G991B', 'Galaxy S21', 'Canon EOS R5', 'iPhone 13', 'E-M10', 'X1', null, '', 'Pixel 8 Pro', 'HUAWEI P30', 'Redmi Note 8', 'LM-G710', 'X-T3', 'DC-GH5', 'ILCE-7M3', 'HERO9 Black', 'FC3170', 'NIKON', 'Nikon D850', 'OLYMPUS E-M1', 'SAMSUNG-SM-G950F', 'samsung_x', 'Apple iPhone', 'COOLPIX P1000', 'M'];
for (const mk of makes) {
  add('normalizeMake', [mk], S.normalizeMake);
  for (const md of models) if (rnd() < 0.35 || md === null) add('normalizeCamera', [mk, md], S.normalizeCamera);
}
for (const [a, b] of [['NIKON CORPORATION', 'NIKON D750'], ['SAMSUNG', 'SM-G991B'], ['samsung', 'Galaxy S21'], ['Canon', 'Canon EOS R5'], ['Apple', 'iPhone 13'], ['OLYMPUS IMAGING CORP.', 'E-M10'], ['FOO ELECTRONICS', 'X1'], [null, null], [null, 'Model Only'], ['Make', null]] as [string | null, string | null][])
  add('normalizeCamera', [a, b], S.normalizeCamera);
const cls = (o: object) => add('classify', [o], S.classify);
for (let i = 0; i < 500; i++) {
  const kind = pick(['image', 'image', 'image', 'video', 'audio']);
  const ext = pick(['png', 'jpg', 'gif', 'webp', 'avif', 'apng', 'heic', 'PNG', 'mp4']);
  const o: Record<string, unknown> = { name: pick(names), ext, kind };
  if (rnd() < 0.7) o.dir = pick(dirs);
  if (rnd() < 0.85) o.width = pick([null, 0, ri(100, 12000), 1080, 1440, 640, 2000, 9000, 3000]);
  if (rnd() < 0.85) o.height = pick([null, 0, ri(100, 12000), 2340, 3200, 1136, 1000, 3000, 9000]);
  if (rnd() < 0.5) o.make = pick(['Apple', 'samsung', null, '']);
  if (rnd() < 0.5) o.model = pick(['iPhone 13', 'SM-G991B', null, '', 'front cam']);
  if (rnd() < 0.5) o.lens = pick(['Samsung Galaxy S21 front camera 2.2mm f/2.2', 'iPhone 13 front TrueDepth camera 2.71mm f/2.2', 'back camera', 'frontal', 'FRONT', null, 'x-front-y', 'front_cam']);
  if (rnd() < 0.3) o.gpano = rnd() < 0.5;
  if (rnd() < 0.4) o.pages = pick([null, 1, 2, 10]);
  cls(o);
}
for (const o of [{ ext: 'png', kind: 'image', width: 1080, height: 2340, name: 'IMG_7781.png' }, { ext: 'png', kind: 'image', width: 1080, height: 2340, name: 'IMG_7781.png', make: 'Apple' }, { ext: 'gif', kind: 'image', name: 'a.gif' }, { ext: 'webp', kind: 'image', name: 'a.webp', pages: 10 }, { ext: 'mp4', kind: 'video', name: 'selfie.mp4' }, { ext: 'jpg', kind: 'image', name: 'Screenshot_pano.jpg', width: 9000, height: 1000, make: 'x' }])
  cls(o);
for (const [n, d] of [['IMG-20240101-WA0001.jpg', ''], ['VID-20240101-WA0012.mp4', ''], ['WhatsApp Image 2024-01-01 at 12.00.00.jpeg', ''], ['photo_2024-01-01_12-00-00.jpg', ''], ['x.jpg', 'Telegram/Telegram Images'], ['IMG_20240101_120000.jpg', ''], ['signal-2024-01-01-120000.jpg', ''], ['received_1234567890.jpeg', ''], ['received_123.jpg', ''], ['a.jpg', 'Pictures/WhatsApp'], ['a.jpg', 'WhatsAppX'], ['a.jpg', 'my signal/x'], ['a.jpg', 'Messenger'], ['STK-20240101-WA001.webp', ''], ['IMG-2024010-WA0001.jpg', ''], ['a.jpg', 'Media/WhatsApp Images/Sent']])
  add('isMessagingName', [n, d], S.isMessagingName);

// ───────────── search ─────────────
for (const s of ['ISTANBUL', 'istanbul', 'İstanbul', 'İSTANBUL', 'ıstanbul', 'Kadıköy ŞİŞLİ Çağlayan Göztepe Ümraniye', 'Düsseldorf Straße', '', 'ẞ', 'Ñandú café', 'ﬁle', 'ＡＢＣ１２３', 'Ελληνικά ΟΔΟΣ', 'Œuvre Æsir', 'ǅemal'])
  add('foldText', [s], S.foldText);
add('foldText', [null], S.foldText);
const queries = ['istanbul 2023 video', 'Ağustos 2024', 'haziran', 'June selfie', 'ekran görüntüleri', 'EKRAN GÖRÜNTÜSÜ 2025', 'panorama kapadokya', 'animasyon', 'gif', 'ara', 'ara 2024', 'IMG_2400.jpg', '..istanbul.. video', 'video video', 'mayıs 1969', 'may 1970', 'dec 2199', 'aralık 2200', 'screen shots of berlin', '2023 2024', 'jun jul 2020', 'temmuz ağustos', '  ', '#hashtag @user!', "o'neil's", 'kasım-2023', 'İzmir,Çeşme;2022', 'gifs gifler gif', 'ozcekim selfies', 'videolar 1999 mart', '...', '-_-', 'a.b.c', 'sept 2021 sep', 'ＮＩＫＯＮ 2020', 'Ocak', 'şubat 2021'];
for (const q of queries) add('parseSearch', [q], S.parseSearch);
for (let i = 0; i < 100; i++) {
  const words = range(ri(1, 5)).map(() => pick(['istanbul', 'video', '2023', 'ağustos', 'agu', 'selfie', 'IMG', 'Kadıköy', 'ekran', 'görüntüsü', 'panoramas', 'x', '1985', 'nov', 'Berlin', '.', '-', 'çeşme']));
  add('parseSearch', [words.join(pick([' ', '  ', ', ']))], S.parseSearch);
}

// ───────────── memories ─────────────
add('spreadPick', [range(10).map((i) => ({ id: `x${i}` })), 3], S.spreadPick);
add('spreadPick', [range(2).map((i) => ({ id: `x${i}` })), 3], S.spreadPick);
add('spreadPick', [range(17).map((i) => ({ id: `x${i}` })), 6], S.spreadPick);
add('spreadPick', [range(7).map((i) => ({ id: `x${i}` })), 0], S.spreadPick);
{
  const now = L('2026-11-05T10:00:00');
  const items = [
    ...[-3, 0, 2].map((d, i) => ({ id: `a${i}`, localAt: L('2025-11-05T12:00:00') + d * DAY })),
    ...[0, 1].map((d, i) => ({ id: `b${i}`, localAt: L('2024-11-05T12:00:00') + d * DAY })),
    ...[0, 0, 1, -1].map((d, i) => ({ id: `c${i}`, localAt: L('2022-11-04T09:00:00') + d * DAY })),
    { id: 'far', localAt: L('2025-11-09T12:00:00') },
    { id: 'now', localAt: L('2026-11-05T08:00:00') },
  ];
  add('onThisDay', [items, now, {}], S.onThisDay);
  add('onThisDay', [[...items].reverse(), now, {}], S.onThisDay);
  add('onThisDay', [range(3).map((i) => ({ id: String(i), localAt: L('2027-02-28T10:00:00') + i * 3600e3 })), L('2028-02-29T10:00:00'), {}], S.onThisDay);
  add('onThisDay', [range(3).map((i) => ({ id: `j${i}`, localAt: L('2025-01-02T10:00:00') + i * 3600e3 })), L('2025-12-31T12:00:00'), {}], S.onThisDay);
  const dec = range(3).map((i) => ({ id: `d${i}`, localAt: L('2024-12-30T10:00:00') + i * 3600e3 }));
  add('onThisDay', [dec, L('2026-01-01T12:00:00'), {}], S.onThisDay);
  add('onThisDay', [dec, L('2026-07-01T12:00:00'), {}], S.onThisDay);
  add('onThisDay', [[], now, {}], S.onThisDay);
}
for (let k = 0; k < 40; k++) {
  const n = ri(0, 400);
  const items = range(n).map((i) => ({ id: `m${ri(0, 99999)}_${i}`, localAt: Date.UTC(ri(1990, 2026), ri(0, 11), ri(1, 31), ri(0, 23), ri(0, 59)) }));
  const now = Date.UTC(2026, ri(0, 11), ri(1, 31), ri(0, 23));
  const opts = pick([{}, { windowDays: ri(0, 10) }, { minItems: ri(1, 5), covers: ri(1, 8) }, { maxYears: ri(1, 40) }, { windowDays: 1, minItems: 1, covers: 2, maxYears: 50 }]);
  add('onThisDay', [items, now, opts], S.onThisDay);
}

// ───────────── trips ─────────────
{
  const IST = { placeKey: 'TR-34', placeName: 'İstanbul', cc: 'TR', lat: 41.01, lon: 29.0 };
  const ANK = { placeKey: 'TR-68', placeName: 'Ankara', cc: 'TR', lat: 39.92, lon: 32.85 };
  const ANT = { placeKey: 'TR-07', placeName: 'Antalya', cc: 'TR', lat: 36.88, lon: 30.7 };
  const BER = { placeKey: 'DE-16', placeName: 'Berlin', cc: 'DE', lat: 52.52, lon: 13.4 };
  const PAR = { placeKey: 'FR-11', placeName: 'Île-de-France', cc: 'FR', lat: 48.85, lon: 2.35 };
  const NOKEY = { placeKey: null, placeName: null, cc: null, lat: 37.98, lon: 23.72 };
  let n = 0;
  const at = (iso: string, p: object, count = 1) => Array.from({ length: count }, (_, i) => ({ id: `i${n++}`, localAt: L(iso) + i * 3600e3, ...p }));
  const life = (fromIso: string, days: number, p: object, step = 3) => Array.from({ length: Math.ceil(days / step) }, (_, k) => at(new Date(L(fromIso) + k * step * DAY).toISOString().slice(0, 19), p)).flat();
  const sets: unknown[][] = [
    [...life('2024-01-01T12:00:00', 360, IST), ...at('2024-07-02T10:00:00', ANT, 4), ...at('2024-07-03T10:00:00', ANT, 3), ...at('2024-07-05T10:00:00', ANT, 2), { id: 'nogps', localAt: L('2024-07-04T10:00:00') }],
    [...life('2024-01-01T12:00:00', 360, IST), ...at('2024-05-10T10:00:00', ANK, 5)],
    [...life('2024-01-01T12:00:00', 360, IST), ...at('2024-05-10T08:00:00', ANK, 10)],
    [...life('2024-01-01T12:00:00', 360, IST), ...at('2024-05-10T08:00:00', BER, 3), ...at('2024-05-11T08:00:00', IST, 1), ...at('2024-05-12T08:00:00', BER, 3)],
    [...life('2023-01-01T12:00:00', 360, IST), ...life('2024-01-01T12:00:00', 360, ANK), ...at('2024-08-10T10:00:00', IST, 6), ...at('2024-08-11T10:00:00', IST, 6)],
    [...life('2024-01-01T12:00:00', 150, IST), ...life('2024-06-01T12:00:00', 210, ANK), ...at('2024-09-10T10:00:00', BER, 6), ...at('2024-09-11T10:00:00', BER, 6)],
    [...life('2022-01-01T12:00:00', 200, IST, 5), ...at('2023-12-30T10:00:00', PAR, 3), ...at('2023-12-31T10:00:00', PAR, 3), ...at('2024-01-01T10:00:00', PAR, 3), ...at('2024-01-02T10:00:00', PAR, 2), ...at('2025-03-01T10:00:00', NOKEY, 12)],
    [...at('2020-01-01T10:00:00', NOKEY, 2), ...at('2020-02-01T10:00:00', BER, 3)],
    [],
  ];
  for (const s of sets) {
    add('homesByYear', [s], S.homesByYear);
    add('findTrips', [s, {}], S.findTrips);
    add('findTrips', [s, { minKm: 10, minDays: 1, minItems: 3, maxGapDays: 0, covers: 3 }], S.findTrips);
  }
  // random travellers
  const places = [IST, ANK, ANT, BER, PAR, NOKEY, { placeKey: 'US-NY', placeName: 'New York', cc: 'US', lat: 40.71, lon: -74.0 }, { placeKey: 'TR-35', placeName: 'İzmir', cc: 'TR', lat: 38.42, lon: 27.14 }];
  for (let k = 0; k < 25; k++) {
    const items: unknown[] = [];
    const home = pick(places);
    const y0 = ri(2018, 2024);
    items.push(...life(`${y0}-01-0${ri(1, 9)}T12:00:00`, ri(30, 700), home, ri(1, 6)));
    for (let t = 0; t < ri(0, 6); t++) {
      const p = pick(places);
      const start = Date.UTC(y0 + ri(-1, 2), ri(0, 11), ri(1, 28), ri(6, 20));
      for (let d = 0; d < ri(1, 6); d++) items.push(...at(new Date(start + d * DAY * ri(1, 2)).toISOString().slice(0, 19), { ...p, lat: p.lat + (rnd() - 0.5) * 0.4, lon: p.lon + (rnd() - 0.5) * 0.4 }, ri(1, 8)));
    }
    for (let t = 0; t < ri(0, 20); t++) items.push({ id: `ng${n++}`, localAt: Date.UTC(y0, ri(0, 11), ri(1, 28)) });
    add('homesByYear', [items], S.homesByYear);
    add('findTrips', [items, pick([{}, { maxGapDays: 2 }, { minKm: 100 }])], S.findTrips);
  }
}

// ───────────── takeout ─────────────
for (const s of ['IMG_1.jpg.json', 'IMG_1.jpg.supplemental-metadata.json', 'IMG_1.jpg.supplemental-metadata(2).json', 'IMG_1.jpg(1).json', 'IMG_20190705_121212345_HDR.jpg.supplemental-me.json', 'metadata.json', 'photo.jpg', '.json', 'a.JSON', 'x(1234).json', 'x(12).json', '(1).json', 'IMG.s.json', 'IMG.jpg.s.json', 'IMG.jpg.SUPPL.json', 'Screenshot_20190101-120000_Some Long App Na.json', 'Ekran görüntüsü 2019-01-01 120000 çok uzun ad.jp.json', 'a.supplemental-metadata.json'])
  add('parseSidecarName', [s], S.parseSidecarName);
for (const s of ['IMG_1(1).jpg', 'IMG_1-edited.jpg', 'IMG_1-düzenlendi.jpg', 'IMG_1-EDITED.JPG', 'IMG_1', '.hidden', 'IMG_1(1)-edited.jpg', 'IMG_1-edited(1).jpg', 'İMG-düzenlendi.jpg', 'x-編集済み.png'])
  add('mediaNameVariants', [s], S.mediaNameVariants);
{
  const dir = ['IMG_1.jpg.json', 'IMG_1.jpg(1).json', 'IMG_2.jpg.supplemental-metadata.json', 'IMG_3.jpg.supplemental-metadata(1).json', 'Screenshot_20190101-120000_Some Long App Na.json', 'IMG_20190705_121212345_HDR.jpg.supplemental-me.json', 'IMG_9.HEIC.json', 'metadata.json'];
  for (const m of ['IMG_1.jpg', 'IMG_1(1).jpg', 'IMG_1-edited.jpg', 'IMG_2.jpg', 'IMG_3(1).jpg', 'IMG_3.jpg', 'Screenshot_20190101-120000_Some Long App Name.jpg', 'IMG_20190705_121212345_HDR.jpg', 'IMG_9.MP4', 'IMG_4.jpg', 'img_1.JPG', 'metadata', 'IMG_9.heic'])
    add('matchSidecar', [m, dir], S.matchSidecar);
  add('matchSidecar', ['IMG_1.jpeg', ['IMG_1.jpg.json', 'IMG_1.jpeg.json']], S.matchSidecar);
  add('matchSidecar', ['IMG_1.jpg', []], S.matchSidecar);
}
for (const j of [
  { photoTakenTime: { timestamp: '1562235010' }, geoData: { latitude: 37.9715, longitude: 23.7267 } },
  { photoTakenTime: { timestamp: '1562235010' }, geoData: { latitude: 0, longitude: 0 }, geoDataExif: { latitude: 41, longitude: 29 } },
  { photoTakenTime: { timestamp: '1562235010' }, geoData: { latitude: 0, longitude: 0 } },
  { title: 'x' }, null, [], 'str', 5, { photoTakenTime: { timestamp: 'abc' } }, { photoTakenTime: { timestamp: true } }, { photoTakenTime: 1562235010 },
  { photoTakenTime: { timestamp: 1562235010.5 }, geoData: { latitude: '41.123456789', longitude: '29.987654321' } }, { geoData: { latitude: 91, longitude: 0 } },
  { geoData: [41, 29] }, { geoData: { latitude: 41.0000005, longitude: -29.0000005 } }, { photoTakenTime: { timestamp: '-5' } }, { photoTakenTime: { timestamp: [12] } }, { geoData: { latitude: null, longitude: 5 } },
])
  add('parseTakeoutJson', [j], S.parseTakeoutJson);

// ───────────── sun ─────────────
for (let i = 0; i < 200; i++) add('sunAltitude', [Date.UTC(ri(1950, 2050), ri(0, 11), ri(1, 28), ri(0, 23), ri(0, 59)), rnd() * 180 - 90, rnd() * 360 - 180], S.sunAltitude);
for (const a of [-30, -8, -8.0001, -6, -4, -4.0001, 0, 3, 6, 6.0001, 30, 90]) add('timeOfDayOf', [a], S.timeOfDayOf);
for (const h of range(24)) add('isNightHour', [h], S.isNightHour);
for (let i = 0; i < 60; i++) {
  const local = Date.UTC(2024, 6, 1, ri(0, 23), ri(0, 59));
  add('estimateInstant', [local, local - ri(-14, 14) * 3600e3 - pick([0, 1800e3]), rnd() * 360 - 180], S.estimateInstant);
}

// ───────────── color ─────────────
for (let i = 0; i < 300; i++) {
  const [r, g, b] = [rnd(), rnd(), rnd()];
  add('colorBucketOf', [r, g, b], S.colorBucketOf);
  add('rgbToHsl', [r, g, b], S.rgbToHsl);
}
for (const [r, g, b] of [[0.85, 0.15, 0.12], [0.9, 0.5, 0.1], [0.9, 0.82, 0.2], [0.2, 0.6, 0.25], [0.2, 0.35, 0.8], [0.5, 0.25, 0.75], [0.9, 0.3, 0.6], [0.5, 0.5, 0.52], [0.04, 0.02, 0.03], [0.98, 0.97, 0.96], [1, 0, 0], [1, 1, 1], [0, 0, 0], [0.5, 0.5, 0.5]])
  add('colorBucketOf', [r, g, b], S.colorBucketOf);
const hashes: string[] = [];
for (let i = 0; i < 60; i++) {
  const w = ri(1, 24);
  const h = ri(1, 24);
  const px = new Uint8Array(w * h * 4);
  const alpha = rnd() < 0.3;
  for (let p = 0; p < w * h; p++) px.set([ri(0, 255), ri(0, 255), ri(0, 255), alpha ? ri(0, 255) : 255], p * 4);
  const hash = rgbaToThumbHash(w, h, px);
  hashes.push(Buffer.from(hash).toString('base64'));
  add('thumbhashAverage', [[...hash]], (a: number[]) => S.thumbhashAverage(Uint8Array.from(a)));
}
for (const hsh of [...hashes, '!!', '', 'AAAA', 'YJqGPQw7sFlslqhFafSE+Q6oJ1h2iHB2Rw', 'YJqGPQw7sFlslqhFafSE-Q6oJ1h2iHB2Rw', 'YJq=GPQw7', 'Y€JqGPQw7']) {
  add('colorIndexOfThumbhash', [hsh], S.colorIndexOfThumbhash);
  add('base64Head', [hsh, 6], (s: string, n: number) => {
    const r = S.base64Head(s, n);
    return r ? [...r] : null;
  });
}
add('colorIndexOfThumbhash', [null], S.colorIndexOfThumbhash);

// ───────────── insights ─────────────
for (const [w, h, f] of [[300, 400, 0], [400, 300, 0], [400, 390, 0], [4000, 1000, 4], [null, 100, 0], [0, 100, 0], [-5, 100, 0], [100, 100, 0], [100, 95, 0], [100, 94.9, 0], [null, null, 4]] as [number | null, number | null, number][])
  add('orientationOf', [w, h, f], S.orientationOf);
for (const d of [3, 60, 600, null, 0, -1, 15, 14.99, 300, 300.01]) add('videoLengthOf', [d], S.videoLengthOf);
for (const [s, h, k] of [[-30, 12, true], [2, 12, true], [40, 23, true], [null, 23, true], [null, 3, true], [null, 12, true], [null, 23, false], [-8, 1, true], [-4, 1, true], [6, 1, true], [6.1, 1, false], [null, 22, true], [null, 5, true], [null, 4, true]] as [number | null, number, boolean][])
  add('dayTimeOf', [s, Date.UTC(2024, 0, 1, h, 0), k], S.dayTimeOf);
{
  const row = (k: number, o: Record<string, unknown> = {}) => ({ id: String(k), kind: 'image', localAt: Date.UTC(2024, k % 12, 1 + (k % 27), 12), size: 1000 + k, width: 400, height: 300, duration: null, lat: null, lon: null, cc: null, placeKey: null, city: null, flags: 0, mtime: 1_700_000_000_000 + k, ...o });
  const rows: any[] = [];
  for (let k = 0; k < 40; k++)
    rows.push(row(k, { kind: k % 10 === 0 ? 'video' : 'image', duration: k % 10 === 0 ? (k === 0 ? 5 : 400) : null, width: k % 4 === 0 ? 300 : 400, height: k % 4 === 0 ? 400 : 300, lat: k % 3 ? 41 : null, lon: k % 3 ? 29 : null, cc: k % 3 ? (k % 2 ? 'TR' : 'IT') : null, placeKey: k % 3 ? (k % 2 ? 'TR-34' : 'IT-16') : null, city: k % 3 ? (k % 2 ? 'Kadıköy' : 'Firenze') : null, color: k % 8, sun: k % 5 === 0 ? -20 : k % 5 === 1 ? 2 : 40, addedAt: k === 7 ? 9e12 : null }));
  rows.sort((a, b) => b.localAt - a.localAt);
  add('computeInsights', [rows, {}], S.computeInsights);
  add('computeInsights', [rows, { colors: false, dayTimes: false }], S.computeInsights);
  // random libraries with many cities (sorting ties with localeCompare)
  const cityNames = ['Kadıköy', 'kadıköy', 'Üsküdar', 'Ankara', 'ankara', 'Çankaya', 'Zürich', 'Århus', 'Ölüdeniz', 'Berlin', 'berlin', 'Ísafjörður', 'São Paulo', 'Sao Paulo', 'Łódź', 'Lodz', 'Ñuñoa', 'Nunoa', '東京', 'Москва', 'İzmir', 'Izmir', 'izmir', 'A-Town', 'A Town', 'A_Town', 'a', 'B', '1st City', 'Ελλάδα', 'ẞtadt', 'Østerbro', 'Œuf', 'Æro', 'Ébène', 'Eben', 'Ebène', 'Saint-Étienne', "L'Aquila", 'La Paz', 'Lapaz', 'ıspir', 'Ispir'];
  for (let k = 0; k < 12; k++) {
    const n = ri(0, 600);
    const rs: any[] = [];
    for (let i = 0; i < n; i++) {
      const gps = rnd() < 0.7;
      const cc = pick(['TR', 'DE', 'US', 'IT', 'BR', 'PL', 'JP', 'ru', 'Tr', '']);
      rs.push({
        id: String(i), kind: pick(['image', 'image', 'video']), localAt: Date.UTC(ri(2010, 2025), ri(0, 11), ri(1, 28), ri(0, 23), ri(0, 59)), size: pick([0, ri(1, 1e9)]),
        width: pick([null, 0, ri(1, 8000)]), height: pick([null, 0, ri(1, 8000)]), duration: pick([null, 0, rnd() * 900]), lat: gps ? rnd() * 180 - 90 : null, lon: gps ? rnd() * 360 - 180 : null,
        cc: gps || rnd() < 0.2 ? cc : null, placeKey: rnd() < 0.8 ? pick([`${cc}-34`, `${cc}-00`, `${cc}-07`, null, '']) : null, city: rnd() < 0.85 ? pick([...cityNames, null, '']) : null,
        flags: pick([0, 0, 4, 1, 2, 8]), color: pick([undefined, null, -1, 0, 3, 7, 8]), sun: pick([undefined, null, -20, 0, 3, 50]), clockKnown: pick([undefined, true, false]),
        addedAt: pick([undefined, null, ri(1e12, 2e12)]), mtime: ri(1e12, 2e12), cloud: pick([undefined, true, false]),
      });
    }
    rs.sort((a, b) => b.localAt - a.localAt);
    add('computeInsights', [rs, pick([{}, { covers: 2, ranked: 5, maxCities: 10 }, { ranked: 0 }])], S.computeInsights);
  }
  const items = range(60).map((k) => ({ id: `i${k}`, localAt: Date.UTC(2023, k % 12, 1 + (k % 27)) })).sort((a, b) => b.localAt - a.localAt);
  add('yearReview', [2023, items], S.yearReview);
  add('yearReview', [2023, items.slice(0, 5)], S.yearReview);
  add('yearReview', [2023, items.filter((x) => new Date(x.localAt).getUTCMonth() < 3)], S.yearReview);
  add('yearReview', [2023, items.filter((x) => new Date(x.localAt).getUTCMonth() === 5), 2], S.yearReview);
  add('yearReview', [2023, [], 0], S.yearReview);
  for (let k = 0; k < 10; k++) {
    const its = range(ri(0, 200)).map((i) => ({ id: `y${i}`, localAt: Date.UTC(2021, ri(0, 11), ri(1, 28)) })).sort((a, b) => b.localAt - a.localAt);
    add('yearReview', [2021, its], S.yearReview);
  }
}
add('calendarOf', [2024, [Date.UTC(2024, 0, 1, 5), Date.UTC(2024, 0, 1, 23), Date.UTC(2024, 11, 31, 12), Date.UTC(2023, 11, 31)]], S.calendarOf);
add('calendarOf', [2023, []], S.calendarOf);
add('calendarOf', [1900, [Date.UTC(1900, 1, 28), Date.UTC(1900, 2, 1)]], S.calendarOf);
add('calendarOf', [2000, range(500).map(() => Date.UTC(2000, ri(0, 11), ri(1, 31), ri(0, 23)))], S.calendarOf);
for (const [y, d] of [[2024, 59], [2023, 59], [2024, 365], [2024, 0], [2024, -1], [2024, 400]]) add('calendarDayKey', [y, d], S.calendarDayKey);

// ───────────── tiles ─────────────
for (const s of ['carto', 'topo', 'off', 'osm', 'satellite', null, undefined, 'x', 'OSM']) add('tileStyleOf', [s], S.tileStyleOf);
for (const [st, z, x, y] of [['osm', 0, 0, 0], ['osm', 3, 7, 7], ['osm', 3, 8, 0], ['osm', 20, 0, 0], ['osm', 19, 524287, 524287], ['topo', 18, 0, 0], ['topo', 17, 0, 0], ['osm', 2, -1, 0], ['osm', 2.5, 0, 0], ['satellite', 19, 0, 524288], ['osm', -1, 0, 0], ['osm', 2, 0, 3.0]] as [S.MapTileStyle, number, number, number][])
  add('isValidTile', [st, z, x, y], S.isValidTile);
for (const st of ['osm', 'topo', 'satellite'] as S.MapTileStyle[]) for (const [z, x, y] of [[3, 4, 2], [0, 0, 0], [5, 1, 1], [5, 2, 1], [5, 2, 2], [12, 2345, 1567]]) add('upstreamTileUrl', [st, z, x, y], S.upstreamTileUrl);
for (let i = 0; i < 100; i++) add('tileOf', [rnd() * 180 - 90, rnd() * 360 - 180, ri(0, 19)], S.tileOf);
for (const [la, lo, z] of [[0, 0, 1], [41.0082, 28.9784, 14], [90, 180, 3], [-90, -180, 3], [85.1, 0, 2], [0, 180, 0]]) add('tileOf', [la, lo, z], S.tileOf);
add('TILE_STYLES', [], () => S.TILE_STYLES);

// ───────────── albums ─────────────
for (const o of [null, undefined, {}, { exclude: ['Thumbnails', ' *.tmp ', '\\a\\b\\', '//x//', '', 'Thumbnails', 5, null], minBytes: 1234.5, showHidden: true }, { exclude: 'x', minBytes: '2048', showHidden: 'yes' }, { minBytes: -5 }, { minBytes: 1e12 }, { minBytes: 'abc' }, { exclude: range(150).map((i) => `p${i % 120}`) }, { exclude: ['x'.repeat(300)] }, { minBytes: true }, { minBytes: [42] }, { exclude: [['a', 'b']] }, 'str', [1, 2]])
  add('normalizeLibraryOptions', [o], S.normalizeLibraryOptions);
const patterns = [['Thumbnails'], ['*.tmp'], ['WhatsApp Stickers'], ['Telegram/*/Cache'], ['**/.thumbnails'], ['a?c'], ['**'], ['x/**/y'], ['\\Windows\\Temp\\'], ['  ', ''], ['[abc]', '(x)', 'a+b', '$HOME', '^x', 'a.b', '{a,b}', 'a|b'], ['THUMBS'], ['ç*'], ['dir/'], ['/root/sub']];
const paths = ['', 'Thumbnails', 'a/Thumbnails/b.jpg', 'thumbnails', 'x.tmp', 'x.TMP', 'x.tmp.jpg', 'WhatsApp Stickers/a.webp', 'Telegram/Foo/Cache', 'Telegram/Foo/Cache/x.jpg', 'Telegram/Cache', '.thumbnails', 'a/b/.thumbnails/c', 'abc', 'abbc', 'a/c', 'x/y', 'x/a/b/y', 'x/y/z', 'Windows/Temp/a', '[abc]', '(x)', 'a+b', '$HOME', '^x', 'a.b', 'axb', '{a,b}', 'a|b', 'çorba', 'Çorba', 'dir', 'dir/x', 'root/sub/x', 'thumbs', '//a//', 'a//b'];
for (const p of patterns) {
  add('excludeMatcher', [p, paths], (ps: string[], all: string[]) => {
    const m = S.excludeMatcher(ps);
    return { empty: m.empty, path: all.map((x) => m.path(x)), name: all.map((x) => m.name(x)) };
  });
}

// ───────────── index helpers ─────────────
for (const a of [1, 0.5, 2, 8, 1 / 8, 100, 0.001, 0, -1, NaN, Infinity, 4 / 3, 16 / 9, 9 / 16, 1.0001, 3]) add('aspectToByte', [a], S.aspectToByte);
for (const b of range(256)) add('byteToAspect', [b], S.byteToAspect);
for (const n of ['a.JPG', '.hidden', 'noext', 'a.b.c', 'a.', 'İMG.JPEG', 'x.tar.gz', '', 'a.Mp4', 'b.HEIC', 'c.zip', 'd.mp3', 'e.txt', 'f.TTF', 'g.m2ts', 'h.jfif', 'i.ICO', '.mp4', 'a.ẞ'])
  for (const f of ['extOf', 'kindOf'] as const) add(f, [n], S[f]);
for (const [d, n] of [['', 'a'], ['x', 'a'], ['x/y', 'b']]) add('joinRel', [d, n], S.joinRel);
for (const r of ['a', 'x/a', 'x/y/b', '', '/a']) add('parentDir', [r], S.parentDir);
add('DEFAULT_SETTINGS', [], () => S.DEFAULT_SETTINGS);
add('constants', [], () => ({
  APP_NAME: S.APP_NAME, API_VERSION: S.API_VERSION, MEDIA_FLAG: S.MEDIA_FLAG, IMAGE_EXT: S.IMAGE_EXT, BROWSER_IMAGE_EXT: S.BROWSER_IMAGE_EXT, VIDEO_EXT: S.VIDEO_EXT,
  AUDIO_EXT: S.AUDIO_EXT, ARCHIVE_EXT: S.ARCHIVE_EXT, FONT_EXT: S.FONT_EXT, COLOR_BUCKETS: S.COLOR_BUCKETS, COLOR_SWATCH: S.COLOR_SWATCH, MAP_TILE_STYLES: S.MAP_TILE_STYLES,
  ORIENTATIONS: S.ORIENTATIONS, VIDEO_LENGTHS: S.VIDEO_LENGTHS, DAY_TIMES: S.DAY_TIMES, SHORT_VIDEO_S: S.SHORT_VIDEO_S, LONG_VIDEO_S: S.LONG_VIDEO_S, YEAR_REVIEW_MIN: S.YEAR_REVIEW_MIN,
  GOLDEN_MIN: S.GOLDEN_MIN, GOLDEN_MAX: S.GOLDEN_MAX, NIGHT_MAX: S.NIGHT_MAX, TAKEOUT_MAX_NAME: S.TAKEOUT_MAX_NAME, DEFAULT_REMOTE_CACHE_MB: S.DEFAULT_REMOTE_CACHE_MB,
  DEFAULT_LIBRARY_OPTIONS: S.DEFAULT_LIBRARY_OPTIONS, GEO_ASSET_PATH: S.GEO_ASSET_PATH, GEO_MAX_KM: S.GEO_MAX_KM, DAY_MS: S.DAY_MS, MESSAGING_SQL: S.MESSAGING_SQL,
}));

// string comparison used by computeInsights' city sort
const cmpWords = ['Kadıköy', 'kadıköy', 'Kadikoy', 'Üsküdar', 'Usküdar', 'Ankara', 'ankara', 'Çankaya', 'Cankaya', 'Zürich', 'Zurich', 'Århus', 'Aarhus', 'Ölüdeniz', 'Oludeniz', 'İzmir', 'Izmir', 'izmir', 'ızmir', 'A-Town', 'A Town', 'A_Town', 'ATown', 'a', 'A', 'b', 'B', '1st', '10th', '2nd', 'Ελλάδα', 'Москва', '東京', 'ß', 'ss', 'Œuf', 'Oeuf', 'Æro', 'Aero', 'Ébène', 'Ebene', 'ebène', "L'Aquila", 'La Paz', 'Lapaz', 'Ñuñoa', 'Nunoa', 'Łódź', 'Lodz', 'ıspir', 'ispir', 'Ispir', '', ' ', '-', 'TR', 'IT', 'de', 'De', 'DE'];
for (let i = 0; i < 400; i++) {
  const a = pick(cmpWords);
  const b = pick(cmpWords);
  add('localeCompare', [a, b], (x: string, y: string) => Math.sign(x.localeCompare(y)));
}

for (const [fn, cases] of Object.entries(files)) fs.writeFileSync(path.join(outDir, `${fn}.json`), JSON.stringify(cases));
console.log(Object.entries(files).map(([k, v]) => `${k}:${v.length}`).join(' '));
