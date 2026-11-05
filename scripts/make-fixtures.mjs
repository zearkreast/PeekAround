#!/usr/bin/env node
// Generates a demo library under ./fixtures/library (images with EXIF dates, videos, audio, a zip).
// Usage: node scripts/make-fixtures.mjs [outDir]
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';

const require = createRequire(path.resolve('packages/server/package.json'));
const sharp = require('sharp');
const yazl = require('yazl');

const out = path.resolve(process.argv[2] ?? 'fixtures/library');
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

async function image(rel, w, h, i, date, fmt = 'jpg') {
  const file = path.join(out, rel);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  let img = sharp(Buffer.from(landscapeSvg(w, h, i)));
  if (fmt === 'jpg') {
    const dt = date.toISOString().slice(0, 19).replace('T', ' ').replace(/-/g, ':');
    img = img.withExif({ IFD0: { Make: 'PeekAround', Model: 'Demo Cam' }, IFD2: { DateTimeOriginal: dt } }).jpeg({ quality: 88 });
  } else if (fmt === 'png') img = img.png();
  else if (fmt === 'webp') img = img.webp();
  await img.toFile(file);
  fs.utimesSync(file, date, date);
}

function ff(args) {
  execFileSync('ffmpeg', ['-hide_banner', '-loglevel', 'error', '-y', ...args]);
}

const sizes = [
  [1600, 1067],
  [1067, 1600],
  [1600, 900],
  [1200, 1200],
  [1600, 1200],
  [900, 1600],
  [2000, 800],
];

let i = 0;
const base = new Date('2024-07-01T09:00:00Z').getTime();
const tatil = '2024 Yaz Tatili';
for (let k = 0; k < 14; k++, i++) {
  const [w, h] = pick(sizes);
  await image(`Fotoğraflar/${tatil}/IMG_${2400 + k}.jpg`, w, h, i, new Date(base + k * 3.7e6 + Math.floor(k / 5) * 86400e3));
}
for (let k = 0; k < 8; k++, i++) {
  const [w, h] = pick(sizes);
  await image(`Fotoğraflar/Aile/aile_${k + 1}.jpg`, w, h, i, new Date(base - 120 * 86400e3 + k * 9e6));
}
for (let k = 0; k < 6; k++, i++) {
  const [w, h] = pick(sizes);
  await image(`Fotoğraflar/Doğa/İstanbul Boğazı ${k + 1}.${k % 3 === 0 ? 'png' : 'jpg'}`, w, h, i, new Date(base - 400 * 86400e3 + k * 86400e3), k % 3 === 0 ? 'png' : 'jpg');
}
for (let k = 0; k < 4; k++, i++) {
  const [w, h] = pick(sizes);
  await image(`Fotoğraflar/ekran_${k + 1}.webp`, w, h, i, new Date(base + 30 * 86400e3 + k * 1e6), 'webp');
}

// videos
fs.mkdirSync(path.join(out, 'Videolar'), { recursive: true });
ff(['-f', 'lavfi', '-i', 'testsrc2=size=1280x720:rate=30', '-f', 'lavfi', '-i', 'sine=frequency=440:sample_rate=44100', '-t', '12', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-shortest', '-metadata', 'creation_time=2024-07-03T18:00:00Z', path.join(out, 'Videolar', 'gün batımı.mp4')]);
ff(['-f', 'lavfi', '-i', 'mandelbrot=size=720x1280:rate=30', '-t', '8', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-metadata', 'creation_time=2024-07-04T12:00:00Z', path.join(out, 'Videolar', 'dikey_klip.mp4')]);
ff(['-f', 'lavfi', '-i', 'life=size=640x360:rate=25:mold=10:ratio=0.2:death_color=#1a1a2e:life_color=#f093fb', '-f', 'lavfi', '-i', 'sine=frequency=220', '-t', '10', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-c:a', 'aac', '-shortest', path.join(out, 'Videolar', 'eski_kayit.mkv')]);

// audio for slideshows
fs.mkdirSync(path.join(out, 'Müzik'), { recursive: true });
ff(['-f', 'lavfi', '-i', 'sine=frequency=330:duration=30', '-f', 'lavfi', '-i', 'sine=frequency=495:duration=30', '-filter_complex', '[0][1]amix=inputs=2,volume=0.4', '-c:a', 'libmp3lame', '-q:a', '5', path.join(out, 'Müzik', 'arka plan.mp3')]);

// a zip with a few images (UTF-8 names) to peek into
const zipImgs = path.join(out, '.zip-src');
for (let k = 0; k < 5; k++, i++) await image(`.zip-src/kare_${k + 1}.jpg`, 1200, 1200, i, new Date(base - 30 * 86400e3 + k * 1e6));
const zip = new yazl.ZipFile();
for (const f of fs.readdirSync(zipImgs)) zip.addFile(path.join(zipImgs, f), `Düğün Albümü/${f}`);
zip.end();
await new Promise((res) => zip.outputStream.pipe(fs.createWriteStream(path.join(out, 'Fotoğraflar', 'düğün_albümü.zip'))).on('close', res));
fs.rmSync(zipImgs, { recursive: true, force: true });

console.log(`fixtures written to ${out}`);
