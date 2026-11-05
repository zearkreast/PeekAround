#!/usr/bin/env node
// Generates a large library for performance measurements (NOT committed data).
// 4000 small JPEGs (varied aspect ratios, EXIF dates over ~4 years, several folders)
// + ~150 short H.264 videos. Deterministic (seeded).
// Usage: node scripts/perf/gen-library.mjs <outDir> [--images 4000] [--videos 150]
import { execFile } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { promisify } from 'node:util';

const run = promisify(execFile);
const require = createRequire(path.resolve('packages/server/package.json'));
const sharp = require('sharp');

const args = process.argv.slice(2);
const out = path.resolve(args.find((a) => !a.startsWith('--')) ?? 'perf-library');
const opt = (name, def) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? Number(args[i + 1]) : def;
};
const IMAGES = opt('images', 4000);
const VIDEOS = opt('videos', 150);

let seed = 1234567;
const rnd = () => (seed = (seed * 16807) % 2147483647) / 2147483647;
const pick = (a) => a[Math.floor(rnd() * a.length)];

// aspect ratios seen on phones/cameras (w, h) at a small size
const SIZES = [
  [480, 360],
  [360, 480],
  [480, 270],
  [270, 480],
  [400, 400],
  [480, 320],
  [320, 480],
  [600, 240],
  [216, 480],
];
const FOLDERS = ['Kamera', 'Kamera/2023', 'WhatsApp Images', 'Seyahat/İtalya', 'Seyahat/Kapadokya', 'Aile', 'Ekran Görüntüleri'];

fs.mkdirSync(out, { recursive: true });
const start = Date.UTC(2021, 0, 1);
const span = Date.UTC(2025, 6, 1) - start;

async function image(i) {
  const [w, h] = pick(SIZES);
  // clustered dates: bursts of photos on the same day like a real camera roll
  const day = Math.floor(rnd() * (span / 86400e3));
  const t = new Date(start + day * 86400e3 + Math.floor(rnd() * 86400e3));
  const dir = path.join(out, pick(FOLDERS));
  fs.mkdirSync(dir, { recursive: true });
  const file = path.join(dir, `IMG_${String(i).padStart(5, '0')}.jpg`);
  if (fs.existsSync(file)) return;
  const hue = Math.floor(rnd() * 360);
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="hsl(${hue},70%,60%)"/><stop offset="1" stop-color="hsl(${(hue + 80) % 360},60%,25%)"/></linearGradient></defs><rect width="100%" height="100%" fill="url(#g)"/><circle cx="${w * rnd()}" cy="${h * rnd()}" r="${Math.min(w, h) / 5}" fill="#fff" opacity=".5"/><text x="12" y="${h - 14}" font-size="28" font-family="sans-serif" fill="#fff">${i}</text></svg>`;
  const dt = t.toISOString().slice(0, 19).replace('T', ' ').replace(/-/g, ':');
  await sharp(Buffer.from(svg)).withExif({ IFD0: { Make: 'Perf', Model: 'Cam' }, IFD2: { DateTimeOriginal: dt } }).jpeg({ quality: 70 }).toFile(file);
  fs.utimesSync(file, t, t);
}

async function video(i) {
  const dir = path.join(out, 'Videolar');
  fs.mkdirSync(dir, { recursive: true });
  const file = path.join(dir, `VID_${String(i).padStart(4, '0')}.mp4`);
  if (fs.existsSync(file)) return;
  const portrait = rnd() < 0.4;
  const size = portrait ? '240x426' : '426x240';
  const day = Math.floor(rnd() * (span / 86400e3));
  const t = new Date(start + day * 86400e3 + Math.floor(rnd() * 86400e3));
  const dur = (1 + rnd() * 3).toFixed(1);
  await run('ffmpeg', ['-hide_banner', '-loglevel', 'error', '-y', '-f', 'lavfi', '-i', `testsrc2=size=${size}:rate=24`, '-t', dur, '-c:v', 'libx264', '-preset', 'ultrafast', '-pix_fmt', 'yuv420p', '-metadata', `creation_time=${t.toISOString()}`, file]);
  fs.utimesSync(file, t, t);
}

const t0 = Date.now();
const queue = [];
for (let i = 0; i < IMAGES; i++) queue.push(() => image(i));
for (let i = 0; i < VIDEOS; i++) queue.push(() => video(i));
let done = 0;
await Promise.all(
  Array.from({ length: 6 }, async () => {
    while (queue.length) {
      await queue.shift()();
      if (++done % 500 === 0) console.log(`${done}/${IMAGES + VIDEOS}`);
    }
  }),
);
console.log(`perf library: ${IMAGES} images + ${VIDEOS} videos in ${out} (${((Date.now() - t0) / 1000).toFixed(1)}s)`);
