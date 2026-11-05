#!/usr/bin/env node
// Generates a demo library under ./fixtures/library: photos with EXIF dates + time zone offsets, GPS
// (home İstanbul, trips to Antalya / Kapadokya / Berlin), camera make/model/lens, screenshots, a selfie,
// panoramas, animated GIF/WebP, videos with location + creation_time, audio and a zip.
// Items on today's month-day in previous years feed "On this day".
//
// Usage: node scripts/make-fixtures.mjs [outDir] [--many N]
//   --many N  also writes N small random photos (random dates / places / cameras) under Toplu/ for perf tests
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { parseArgs } from 'node:util';

const require = createRequire(path.resolve('packages/server/package.json'));
const sharp = require('sharp');
const yazl = require('yazl');

const { values: opts, positionals } = parseArgs({ allowPositionals: true, options: { many: { type: 'string' } } });
const out = path.resolve(positionals[0] ?? 'fixtures/library');
const many = Math.max(0, Number(opts.many) || 0);
fs.rmSync(out, { recursive: true, force: true });

let seed = 7;
const rnd = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
const pick = (a) => a[Math.floor(rnd() * a.length)];

const palettes = [
  ['#ff9a8b', '#ff6a88', '#ff99ac', '#2b1055'],
  ['#0f2027', '#203a43', '#2c5364', '#f7b733'],
  ['#ffecd2', '#fcb69f', '#ff8177', '#3a1c71'],
  ['#a1c4fd', '#c2e9fb', '#667eea', '#1e3c72'],
  ['#d4fc79', '#96e6a1', '#43cea2', '#185a9d'],
  ['#f093fb', '#f5576c', '#4facfe', '#1a1a2e'],
  ['#fddb92', '#d1fdff', '#f6d365', '#2d3436'],
  ['#30cfd0', '#330867', '#a8edea', '#fed6e3'],
];

function landscapeSvg(w, h, i) {
  const [a, b, c, d] = palettes[i % palettes.length];
  const sunX = (0.2 + rnd() * 0.6) * w;
  const sunY = (0.2 + rnd() * 0.3) * h;
  const r = Math.min(w, h) * (0.08 + rnd() * 0.08);
  const ridge = (base, amp, col, op) => {
    let p = `M0 ${h} L0 ${base}`;
    const n = 8;
    for (let k = 0; k <= n; k++) p += ` L${(k / n) * w} ${base - rnd() * amp}`;
    return `<path d="${p} L${w} ${h} Z" fill="${col}" opacity="${op}"/>`;
  };
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}">
  <defs>
    <linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${a}"/><stop offset=".6" stop-color="${b}"/><stop offset="1" stop-color="${c}"/></linearGradient>
    <radialGradient id="s"><stop offset="0" stop-color="#fff" stop-opacity=".95"/><stop offset="1" stop-color="#fff" stop-opacity="0"/></radialGradient>
  </defs>
  <rect width="100%" height="100%" fill="url(#g)"/>
  <circle cx="${sunX}" cy="${sunY}" r="${r * 2.4}" fill="url(#s)"/>
  <circle cx="${sunX}" cy="${sunY}" r="${r}" fill="#fff" opacity=".9"/>
  ${ridge(h * 0.62, h * 0.18, d, 0.35)}
  ${ridge(h * 0.72, h * 0.16, d, 0.6)}
  ${ridge(h * 0.85, h * 0.12, d, 0.9)}
</svg>`;
}

function screenSvg(w, h, i) {
  const [a, b] = palettes[i % palettes.length];
  let rows = '';
  for (let y = 0.12 * h; y < h * 0.9; y += h * 0.07) rows += `<rect x="${w * 0.06}" y="${y}" width="${w * (0.5 + rnd() * 0.38)}" height="${h * 0.035}" rx="${h * 0.01}" fill="#fff" opacity=".85"/>`;
  return `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}"><rect width="100%" height="100%" fill="${a}"/><rect width="100%" height="${h * 0.07}" fill="${b}"/>${rows}</svg>`;
}

// ── EXIF helpers ──
const MIN = 60_000;
const DAY = 86_400_000;
/** wall clock (encoded as UTC ms) → "YYYY:MM:DD HH:MM:SS" */
const exifDate = (local) => new Date(local).toISOString().slice(0, 19).replace('T', ' ').replace(/-/g, ':');
const offStr = (off) => `${off < 0 ? '-' : '+'}${String(Math.floor(Math.abs(off) / 60)).padStart(2, '0')}:${String(Math.abs(off) % 60).padStart(2, '0')}`;
/** decimal degrees → EXIF rational triple "41/1 0/1 2952/100" */
const dms = (v) => {
  const a = Math.abs(v);
  const d = Math.floor(a);
  const m = Math.floor((a - d) * 60);
  const s = Math.round(((a - d) * 60 - m) * 60 * 100);
  return `${d}/1 ${m}/1 ${s}/100`;
};
const local = (iso) => Date.parse(`${iso}Z`); // '2024-07-03T10:00:00' → wall clock as UTC

const CAMS = {
  s21: { Make: 'samsung', Model: 'SM-G991B', LensModel: 'Samsung Galaxy S21 back camera 5.4mm f/1.8' },
  s21front: { Make: 'samsung', Model: 'SM-G991B', LensModel: 'Samsung Galaxy S21 front camera 2.2mm f/2.2' },
  iphone: { Make: 'Apple', Model: 'iPhone 13', LensModel: 'iPhone 13 back dual wide camera 5.1mm f/1.6' },
  nikon: { Make: 'NIKON CORPORATION', Model: 'NIKON D750', LensModel: 'AF-S NIKKOR 24-120mm f/4G ED VR' },
  pixel: { Make: 'Google', Model: 'Pixel 7', LensModel: 'Pixel 7 back camera 6.81mm f/1.85' },
};

const PLACES = {
  kadikoy: [40.9903, 29.029],
  besiktas: [41.0422, 29.0067],
  sultanahmet: [41.0054, 28.9768],
  uskudar: [41.026, 29.0155],
  kaleici: [36.8841, 30.7056],
  konyaalti: [36.8721, 30.638],
  lara: [36.8536, 30.7917],
  kemer: [36.5977, 30.56],
  kizilay: [39.9208, 32.8541],
  goreme: [38.6431, 34.8289],
  urgup: [38.6302, 34.9116],
  berlin: [52.52, 13.405],
  kreuzberg: [52.4986, 13.4033],
};

/**
 * @param {string} rel
 * @param {{w:number,h:number,i:number,at:number,off?:number|null,place?:number[]|null,cam?:object|null,fmt?:string,svg?:string,xmp?:string}} o
 *   at = wall clock (UTC encoded); off = minutes east of UTC (default +180 = Türkiye)
 */
async function image(rel, o) {
  const file = path.join(out, rel);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  const fmt = o.fmt ?? 'jpg';
  const off = o.off === undefined ? 180 : o.off;
  let img = sharp(Buffer.from(o.svg ?? landscapeSvg(o.w, o.h, o.i)));
  if (fmt === 'jpg') {
    const exif = { IFD0: {}, IFD2: { DateTimeOriginal: exifDate(o.at) } };
    if (off !== null) exif.IFD2.OffsetTimeOriginal = offStr(off);
    if (o.cam) {
      exif.IFD0.Make = o.cam.Make;
      exif.IFD0.Model = o.cam.Model;
      if (o.cam.LensModel) exif.IFD2.LensModel = o.cam.LensModel;
    }
    if (o.place) {
      const [lat, lon] = o.place;
      const utc = new Date(o.at - (off ?? 0) * MIN);
      exif.IFD3 = {
        GPSLatitudeRef: lat < 0 ? 'S' : 'N',
        GPSLatitude: dms(lat),
        GPSLongitudeRef: lon < 0 ? 'W' : 'E',
        GPSLongitude: dms(lon),
        GPSDateStamp: utc.toISOString().slice(0, 10).replace(/-/g, ':'),
        GPSTimeStamp: `${utc.getUTCHours()}/1 ${utc.getUTCMinutes()}/1 ${utc.getUTCSeconds()}/1`,
      };
    }
    img = img.withExif(exif);
    if (o.xmp) img = img.withXmp(o.xmp);
    img = img.jpeg({ quality: 86 });
  } else if (fmt === 'png') img = img.png();
  else if (fmt === 'webp') img = img.webp();
  await img.toFile(file);
  const instant = new Date(o.at - (off ?? 180) * MIN);
  fs.utimesSync(file, instant, instant);
}

function ff(args) {
  execFileSync('ffmpeg', ['-hide_banner', '-loglevel', 'error', '-y', ...args]);
}

const jitter = ([lat, lon], km = 1.5) => [lat + (rnd() - 0.5) * (km / 55), lon + (rnd() - 0.5) * (km / 45)];

const sizes = [
  [1600, 1067],
  [1067, 1600],
  [1600, 900],
  [1200, 1200],
  [1600, 1200],
  [900, 1600],
  [1800, 810],
];

let i = 0;

// ── 2024 summer trip to Antalya (home: İstanbul) — 5 consecutive days ──
const tatil = '2024 Yaz Tatili';
const antalya = [PLACES.kaleici, PLACES.konyaalti, PLACES.lara, PLACES.kemer];
for (let k = 0; k < 18; k++, i++) {
  const [w, h] = pick(sizes);
  const day = Math.floor(k / 4); // 0..4
  const at = local('2024-07-02T09:00:00') + day * DAY + (k % 4) * 3.1 * 3600e3;
  await image(`Fotoğraflar/${tatil}/IMG_${2400 + k}.jpg`, { w, h, i, at, place: jitter(antalya[day % antalya.length], 3), cam: k % 5 === 4 ? CAMS.nikon : CAMS.s21 });
}
// a late-evening shot: 23:40 local on Jul 4 must stay on Jul 4 in every server time zone
await image(`Fotoğraflar/${tatil}/IMG_2499.jpg`, { w: 1600, h: 1067, i: i++, at: local('2024-07-04T23:40:00'), place: PLACES.kaleici, cam: CAMS.s21 });
// panorama from the trip (camera EXIF + aspect ≥ 2.3)
await image(`Fotoğraflar/${tatil}/PANO_20240705_181500.jpg`, { w: 3000, h: 1000, i: i++, at: local('2024-07-05T18:15:00'), place: PLACES.konyaalti, cam: CAMS.s21 });

// ── family at home (İstanbul) spread over 2023-2026 ──
const home = [PLACES.kadikoy, PLACES.besiktas, PLACES.sultanahmet, PLACES.uskudar];
for (let k = 0; k < 16; k++, i++) {
  const [w, h] = pick(sizes);
  const at = local('2023-01-14T11:00:00') + Math.floor(k * 83.3) * DAY + k * 3600e3;
  await image(`Fotoğraflar/Aile/aile_${k + 1}.jpg`, { w, h, i, at, place: jitter(pick(home)), cam: k % 3 ? CAMS.iphone : CAMS.s21 });
}
// selfie (front camera lens)
await image('Fotoğraflar/Aile/IMG_20250601_141500.jpg', { w: 1080, h: 1440, i: i++, at: local('2025-06-01T14:15:00'), place: PLACES.kadikoy, cam: CAMS.s21front });
// Ankara visit, a single day (< 10 items → not a trip)
for (let k = 0; k < 3; k++, i++) {
  await image(`Fotoğraflar/Aile/ankara_${k + 1}.jpg`, { w: 1600, h: 1200, i, at: local('2025-02-08T12:00:00') + k * 1.5 * 3600e3, place: jitter(PLACES.kizilay), cam: CAMS.iphone });
}

// ── Kapadokya 2023 (3 days) and Berlin 2025 (abroad, offset +01:00) ──
for (let k = 0; k < 9; k++, i++) {
  const [w, h] = pick(sizes);
  await image(`Fotoğraflar/Kapadokya 2023/DSC_${3100 + k}.jpg`, {
    w,
    h,
    i,
    at: local('2023-09-15T07:30:00') + Math.floor(k / 3) * DAY + (k % 3) * 4 * 3600e3,
    place: jitter(k % 2 ? PLACES.urgup : PLACES.goreme),
    cam: CAMS.nikon,
  });
}
for (let k = 0; k < 12; k++, i++) {
  const [w, h] = pick(sizes);
  await image(`Fotoğraflar/Berlin/PXL_202504${String(18 + Math.floor(k / 6)).padStart(2, '0')}_${k}.jpg`, {
    w,
    h,
    i,
    at: local('2025-04-18T10:00:00') + Math.floor(k / 6) * DAY + (k % 6) * 1.6 * 3600e3,
    off: 120,
    place: jitter(k % 3 ? PLACES.berlin : PLACES.kreuzberg),
    cam: CAMS.pixel,
  });
}

// ── nature: PNG/JPG without GPS, one equirectangular (GPano) panorama ──
for (let k = 0; k < 6; k++, i++) {
  const [w, h] = [
    [1600, 1067],
    [1200, 1200],
    [1600, 1200],
  ][k % 3];
  const png = k % 3 === 0;
  await image(`Fotoğraflar/Doğa/İstanbul Boğazı ${k + 1}.${png ? 'png' : 'jpg'}`, { w, h, i, at: local('2023-05-28T08:00:00') + k * DAY, fmt: png ? 'png' : 'jpg', off: null });
}
const gpano = `<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?><x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:GPano="http://ns.google.com/photos/1.0/panorama/" GPano:ProjectionType="equirectangular" GPano:UsePanoramaViewer="True"/></rdf:RDF></x:xmpmeta><?xpacket end="w"?>`;
await image('Fotoğraflar/Doğa/PANO_360_kubbe.jpg', { w: 2400, h: 1200, i: i++, at: local('2023-05-30T12:00:00'), place: PLACES.sultanahmet, cam: CAMS.pixel, xmp: gpano });
for (let k = 0; k < 4; k++, i++) {
  const [w, h] = pick(sizes);
  await image(`Fotoğraflar/ekran_${k + 1}.webp`, { w, h, i, at: local('2024-08-01T10:00:00') + k * 1e6, fmt: 'webp' });
}

// ── screenshots (name patterns in several languages + a nameless phone-shaped PNG) ──
const shots = [
  ['Ekran Görüntüleri/Screenshot_20240812-101500.png', 1080, 2340, '2024-08-12T10:15:00'],
  ['Ekran Görüntüleri/Ekran görüntüsü 2025-03-01 141230.png', 1920, 1080, '2025-03-01T14:12:30'],
  ['Ekran Görüntüleri/Screen Shot 2023-05-02 at 10.11.12.png', 2880, 1800, '2023-05-02T10:11:12'],
  ['Ekran Görüntüleri/Bildschirmfoto 2025-04-19 um 09.30.00.png', 1440, 900, '2025-04-19T09:30:00'],
  ['Ekran Görüntüleri/IMG_7781.png', 1170, 2532, '2025-09-09T19:00:00'],
];
for (const [rel, w, h, at] of shots) await image(rel, { w, h, i: i++, at: local(at), fmt: 'png', svg: screenSvg(w, h, i) });

// ── on this day: today's month-day in previous years (±2 days) ──
const today = new Date();
for (const ago of [1, 2, 3]) {
  for (let k = 0; k < 4; k++, i++) {
    const [w, h] = pick(sizes);
    const at = Date.UTC(today.getFullYear() - ago, today.getMonth(), today.getDate(), 10 + k * 2) + (k - 1) * DAY;
    await image(`Fotoğraflar/Bugün Geçmişte/${today.getFullYear() - ago}_${k + 1}.jpg`, { w, h, i, at, place: jitter(pick(home)), cam: CAMS.iphone });
  }
}

// ── animated ──
fs.mkdirSync(path.join(out, 'Fotoğraflar', 'Komik'), { recursive: true });
ff(['-f', 'lavfi', '-i', 'testsrc=size=320x240:rate=8', '-t', '2', path.join(out, 'Fotoğraflar', 'Komik', 'dans eden kedi.gif')]);
ff(['-f', 'lavfi', '-i', 'mandelbrot=size=320x320:rate=8', '-t', '2', '-c:v', 'libwebp_anim', '-loop', '0', path.join(out, 'Fotoğraflar', 'Komik', 'sticker.webp')]);

// ── videos (UTC creation_time + ISO 6709 location + maker tags) ──
fs.mkdirSync(path.join(out, 'Videolar'), { recursive: true });
const tags = (iso, loc, make, model) => [
  '-metadata', `creation_time=${iso}`,
  ...(loc ? ['-metadata', `location=${loc}`] : []),
  ...(make ? ['-metadata', `com.android.manufacturer=${make}`, '-metadata', `com.android.model=${model}`, '-movflags', 'use_metadata_tags'] : []),
];
ff(['-f', 'lavfi', '-i', 'testsrc2=size=1280x720:rate=30', '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=44100', '-t', '12', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-shortest', ...tags('2024-07-03T18:00:00Z', '+36.8841+030.7056/', 'samsung', 'SM-G991B'), path.join(out, 'Videolar', 'gün batımı.mp4')]);
ff(['-f', 'lavfi', '-i', 'mandelbrot=size=720x1280:rate=30', '-t', '8', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', ...tags('2024-07-04T12:00:00Z', '+41.0082+028.9784/', 'Google', 'Pixel 7'), path.join(out, 'Videolar', 'dikey_klip.mp4')]);
// UTC-only creation_time: shown in the home time zone (Settings.homeTimeZone) — 00:30 on Jan 1 in İstanbul
ff(['-f', 'lavfi', '-i', 'testsrc=size=640x360:rate=25', '-t', '4', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', ...tags('2025-12-31T21:30:00Z', '+41.0422+029.0067/'), path.join(out, 'Videolar', 'yılbaşı.mp4')]);
ff(['-f', 'lavfi', '-i', 'life=size=640x360:rate=25:mold=10:ratio=0.2:death_color=#1a1a2e:life_color=#f093fb', '-f', 'lavfi', '-i', 'sine=frequency=220', '-t', '10', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-shortest', path.join(out, 'Videolar', 'eski_kayit.mkv')]);

// audio for slideshows
fs.mkdirSync(path.join(out, 'Müzik'), { recursive: true });
ff(['-f', 'lavfi', '-i', 'sine=frequency=330:duration=30', '-f', 'lavfi', '-i', 'sine=frequency=495:duration=30', '-filter_complex', '[0][1]amix=inputs=2,volume=0.4', '-c:a', 'libmp3lame', '-q:a', '5', path.join(out, 'Müzik', 'arka plan.mp3')]);

// a zip with a few images (UTF-8 names) to peek into
const zipImgs = path.join(out, '.zip-src');
for (let k = 0; k < 5; k++, i++) await image(`.zip-src/kare_${k + 1}.jpg`, { w: 1200, h: 1200, i, at: local('2024-06-01T15:00:00') + k * 1e6, cam: CAMS.iphone });
const zip = new yazl.ZipFile();
for (const f of fs.readdirSync(zipImgs)) zip.addFile(path.join(zipImgs, f), `Düğün Albümü/${f}`);
zip.end();
await new Promise((res) => zip.outputStream.pipe(fs.createWriteStream(path.join(out, 'Fotoğraflar', 'düğün_albümü.zip'))).on('close', res));
fs.rmSync(zipImgs, { recursive: true, force: true });

// ── --many N: quick small photos for performance testing ──
if (many) {
  const places = Object.values(PLACES);
  const cams = Object.values(CAMS);
  const start = Date.UTC(today.getFullYear() - 6, 0, 1);
  const span = Date.now() - start;
  const t0 = Date.now();
  for (let k = 0; k < many; k++) {
    const [w, h] = pick(sizes);
    const at = start + Math.floor(rnd() * span);
    const d = new Date(at);
    const rel = `Toplu/${d.getUTCFullYear()}/${String(d.getUTCMonth() + 1).padStart(2, '0')}/IMG_${String(k).padStart(6, '0')}.jpg`;
    const file = path.join(out, rel);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    const [lat, lon] = rnd() < 0.85 ? jitter(pick(places), 6) : [null, null];
    const cam = pick(cams);
    const exif = { IFD0: { Make: cam.Make, Model: cam.Model }, IFD2: { DateTimeOriginal: exifDate(at), OffsetTimeOriginal: '+03:00', LensModel: cam.LensModel } };
    if (lat !== null) exif.IFD3 = { GPSLatitudeRef: 'N', GPSLatitude: dms(lat), GPSLongitudeRef: 'E', GPSLongitude: dms(lon) };
    const [a, b] = palettes[k % palettes.length];
    await sharp({ create: { width: Math.round(w / 8), height: Math.round(h / 8), channels: 3, background: k % 2 ? a : b } })
      .withExif(exif)
      .jpeg({ quality: 60 })
      .toFile(file);
    const instant = new Date(at - 180 * MIN);
    fs.utimesSync(file, instant, instant);
  }
  console.log(`${many} extra photos in ${((Date.now() - t0) / 1000).toFixed(1)}s`);
}

console.log(`fixtures written to ${out}`);
