#!/usr/bin/env node
// Generates every app icon from brand/logo.svg (the albatross):
//   web favicon + PWA icons, Electron icons (png set + Windows .ico), Android launcher/adaptive icons + splash.
// Usage: node scripts/make-icons.mjs
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';

const root = path.resolve(path.dirname(new URL(import.meta.url).pathname), '..');
const require = createRequire(path.join(root, 'packages/server/package.json'));
const sharp = require('sharp');

const logo = fs.readFileSync(path.join(root, 'brand/logo.svg'), 'utf8');
/** full-bleed square (no rounded corners) — for maskable / adaptive backgrounds */
const square = logo.replace(/rx="60"/g, 'rx="0"');
/** only the bird on transparent, scaled into the adaptive-icon safe zone */
const inner = logo.slice(logo.indexOf('<g clip-path'), logo.lastIndexOf('</svg>'));
const defs = logo.slice(logo.indexOf('<defs>'), logo.indexOf('</defs>') + 7);
const foreground = (scale) =>
  `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256">${defs}<g transform="translate(140 256) scale(${scale}) translate(-140 -256)">${inner.replace('clip-path="url(#frame)"', '')}</g></svg>`;
const background = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256">${defs}<rect width="256" height="256" fill="url(#bg)"/></svg>`;

const png = (svg, size) => sharp(Buffer.from(svg), { density: Math.max(72, (size / 256) * 72 * 2) }).resize(size, size).png().toBuffer();
const write = async (file, buf) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, buf);
  console.log('  ', path.relative(root, file));
};

/** ICO containing PNG images (Vista+). */
function ico(pngs) {
  const header = Buffer.alloc(6);
  header.writeUInt16LE(0, 0);
  header.writeUInt16LE(1, 2);
  header.writeUInt16LE(pngs.length, 4);
  const dir = Buffer.alloc(16 * pngs.length);
  let offset = 6 + dir.length;
  pngs.forEach(({ size, data }, i) => {
    const o = i * 16;
    dir.writeUInt8(size >= 256 ? 0 : size, o);
    dir.writeUInt8(size >= 256 ? 0 : size, o + 1);
    dir.writeUInt8(0, o + 2);
    dir.writeUInt8(0, o + 3);
    dir.writeUInt16LE(1, o + 4);
    dir.writeUInt16LE(32, o + 6);
    dir.writeUInt32LE(data.length, o + 8);
    dir.writeUInt32LE(offset, o + 12);
    offset += data.length;
  });
  return Buffer.concat([header, dir, ...pngs.map((p) => p.data)]);
}

console.log('web');
const web = path.join(root, 'packages/web/public');
await write(path.join(web, 'favicon.svg'), Buffer.from(logo));
await write(path.join(web, 'icons/icon-192.png'), await png(logo, 192));
await write(path.join(web, 'icons/icon-512.png'), await png(logo, 512));
await write(path.join(web, 'icons/icon-maskable-512.png'), await png(square, 512));
await write(path.join(web, 'icons/apple-touch-icon.png'), await png(square, 180));

console.log('desktop');
const desk = path.join(root, 'packages/desktop/build');
await write(path.join(desk, 'icon.png'), await png(logo, 512));
for (const s of [16, 24, 32, 48, 64, 128, 256, 512]) await write(path.join(desk, `icons/${s}x${s}.png`), await png(logo, s));
const icoSizes = [16, 24, 32, 48, 64, 128, 256];
await write(path.join(desk, 'icon.ico'), ico(await Promise.all(icoSizes.map(async (size) => ({ size, data: await png(logo, size) })))));

console.log('android');
const res = path.join(root, 'packages/android/android/app/src/main/res');
if (fs.existsSync(res)) {
  const dens = { mdpi: 1, hdpi: 1.5, xhdpi: 2, xxhdpi: 3, xxxhdpi: 4 };
  for (const [d, k] of Object.entries(dens)) {
    await write(path.join(res, `mipmap-${d}/ic_launcher.png`), await png(logo, Math.round(48 * k)));
    const round = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256"><defs><clipPath id="c"><circle cx="128" cy="128" r="128"/></clipPath></defs><g clip-path="url(#c)">${square.replace(/^<svg[^>]*>/, '').replace(/<\/svg>\s*$/, '')}</g></svg>`;
    await write(path.join(res, `mipmap-${d}/ic_launcher_round.png`), await png(round, Math.round(48 * k)));
    await write(path.join(res, `mipmap-${d}/ic_launcher_foreground.png`), await png(foreground(0.78), Math.round(108 * k)));
    await write(path.join(res, `mipmap-${d}/ic_launcher_background.png`), await png(background, Math.round(108 * k)));
  }
  fs.writeFileSync(
    path.join(res, 'mipmap-anydpi-v26/ic_launcher.xml'),
    '<?xml version="1.0" encoding="utf-8"?>\n<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">\n    <background android:drawable="@mipmap/ic_launcher_background"/>\n    <foreground android:drawable="@mipmap/ic_launcher_foreground"/>\n    <monochrome android:drawable="@mipmap/ic_launcher_foreground"/>\n</adaptive-icon>\n',
  );
  if (fs.existsSync(path.join(res, 'mipmap-anydpi-v26/ic_launcher_round.xml'))) {
    fs.copyFileSync(path.join(res, 'mipmap-anydpi-v26/ic_launcher.xml'), path.join(res, 'mipmap-anydpi-v26/ic_launcher_round.xml'));
  }
  // splash screens: dark background + centred logo
  for (const dir of fs.readdirSync(res).filter((d) => d.startsWith('drawable'))) {
    const f = path.join(res, dir, 'splash.png');
    if (!fs.existsSync(f)) continue;
    const { width, height } = await sharp(f).metadata();
    const s = Math.round(Math.min(width, height) * 0.28);
    const mark = await png(logo, s);
    const buf = await sharp({ create: { width, height, channels: 4, background: '#09090d' } })
      .composite([{ input: mark, left: Math.round((width - s) / 2), top: Math.round((height - s) / 2) }])
      .png()
      .toBuffer();
    await write(f, buf);
  }
}
console.log('done');
