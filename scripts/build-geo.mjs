#!/usr/bin/env node
// Builds the offline reverse-geocoding asset packages/web/public/geo/cities.bin (gzip) from GeoNames.
//
//   node scripts/build-geo.mjs [--cache dir] [--out file]
//
// Data © GeoNames (https://www.geonames.org), licensed CC BY 4.0. The attribution is stored in the
// asset header and in packages/web/public/geo/ATTRIBUTION.txt.
//
// Sources: cities5000.zip (every place with ≥ 5000 inhabitants) + admin1CodesASCII.txt.
// For DENSE countries the full country dump is used instead: every populated place is labelled with
// its district (ADM2, e.g. "Kadıköy", "Çankaya") so the label matches how people name places there.
// Layout: see packages/shared/src/geo.ts.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import zlib from 'node:zlib';
import { createRequire } from 'node:module';
import { parseArgs } from 'node:util';

const require = createRequire(path.resolve('packages/server/package.json'));
const yauzl = require('yauzl');

const { values } = parseArgs({
  options: { cache: { type: 'string' }, out: { type: 'string' } },
});
const cache = path.resolve(values.cache ?? path.join(os.tmpdir(), 'peek-geonames'));
const outFile = path.resolve(values.out ?? 'packages/web/public/geo/cities.bin');
const BASE = 'https://download.geonames.org/export/dump/';
const DENSE = ['TR'];
const SKIP_FCODE = new Set(['PPLH', 'PPLQ', 'PPLW', 'PPLCH']);
const ATTRIBUTION = 'Place names: GeoNames (https://www.geonames.org), CC BY 4.0';

fs.mkdirSync(cache, { recursive: true });

async function download(name) {
  const f = path.join(cache, name);
  if (fs.existsSync(f) && fs.statSync(f).size > 0) return f;
  console.log(`↓ ${BASE}${name}`);
  const res = await fetch(BASE + name);
  if (!res.ok) throw new Error(`${name}: HTTP ${res.status}`);
  fs.writeFileSync(f, Buffer.from(await res.arrayBuffer()));
  return f;
}

function unzipEntry(zipFile, entryName) {
  return new Promise((resolve, reject) => {
    yauzl.open(zipFile, { lazyEntries: true }, (err, zf) => {
      if (err) return reject(err);
      zf.on('entry', (e) => {
        if (e.fileName !== entryName) return zf.readEntry();
        zf.openReadStream(e, (err2, rs) => {
          if (err2) return reject(err2);
          const chunks = [];
          rs.on('data', (c) => chunks.push(c));
          rs.on('end', () => {
            zf.close();
            resolve(Buffer.concat(chunks).toString('utf8'));
          });
        });
      });
      zf.on('end', () => reject(new Error(`${entryName} not in ${zipFile}`)));
      zf.readEntry();
    });
  });
}

const rows = (txt) => txt.split('\n').filter((l) => l && !l.startsWith('#')).map((l) => l.split('\t'));

function cleanAdmin(name) {
  return name
    .replace(/^(State of|Province of|Provincia de|Provincia di|Province de|Région|Region of)\s+/i, '')
    .replace(/\s+(Province|Governorate|Prefecture|Oblast|Voivodeship|Region|District|County|İli)$/i, '')
    .trim();
}
function cleanDistrict(name) {
  return name.replace(/\s+(İlçesi|Ilcesi)$/i, '').replace(/\s+Merkez$/i, '').trim();
}

// ── admin1 names ──
const admin1 = new Map(); // "CC.code" → name
for (const r of rows(fs.readFileSync(await download('admin1CodesASCII.txt'), 'utf8'))) admin1.set(r[0], cleanAdmin(r[1]));

/** @type {{name:string, lat:number, lon:number, cc:string, a1:string, pop:number, fc:string}[]} */
const places = [];

// ── cities5000 ──
const citiesTxt = await unzipEntry(await download('cities5000.zip'), 'cities5000.txt');
for (const r of rows(citiesTxt)) {
  const cc = r[8];
  if (DENSE.includes(cc) || SKIP_FCODE.has(r[7]) || r[6] !== 'P') continue;
  places.push({ name: r[1], lat: Number(r[4]), lon: Number(r[5]), cc, a1: r[10] || '', pop: Number(r[14]) || 0, fc: r[7] });
}

// ── dense countries: populated places labelled with their district ──
for (const cc of DENSE) {
  const txt = await unzipEntry(await download(`${cc}.zip`), `${cc}.txt`);
  const all = rows(txt);
  const districts = new Map(); // admin2 code → { name, pop, a1, lat, lon }
  for (const r of all) {
    if (r[7] === 'ADM1' && r[10]) admin1.set(`${cc}.${r[10]}`, cleanAdmin(r[1]));
    if (r[7] === 'ADM2' && r[11]) {
      districts.set(r[11], { name: cleanDistrict(r[1]), pop: Number(r[14]) || 0, a1: r[10] || '', lat: Number(r[4]), lon: Number(r[5]) });
    }
  }
  const seen = new Set();
  const add = (d, lat, lon) => {
    const k = `${d.name}|${Math.round(lat * 50)}|${Math.round(lon * 50)}`; // ~2 km de-dup per district
    if (seen.has(k)) return;
    seen.add(k);
    places.push({ name: d.name, lat, lon, cc, a1: d.a1, pop: d.pop, fc: 'ADM2' });
  };
  for (const d of districts.values()) add(d, d.lat, d.lon);
  for (const r of all) {
    if (r[6] !== 'P' || SKIP_FCODE.has(r[7]) || !r[11]) continue;
    const d = districts.get(r[11]);
    if (d) add(d, Number(r[4]), Number(r[5]));
  }
}

// ── tables ──
places.sort((a, b) => (a.cc < b.cc ? -1 : a.cc > b.cc ? 1 : a.a1 < b.a1 ? -1 : a.a1 > b.a1 ? 1 : a.lat - b.lat));
const ccList = [...new Set(places.map((p) => p.cc))].sort();
const ccIdx = new Map(ccList.map((c, i) => [c, i]));
const adminKeys = [...new Set(places.map((p) => `${p.cc}.${p.a1}`))].sort();
const adminIdx = new Map(adminKeys.map((k, i) => [k, i]));
const adminTable = adminKeys.map((k) => {
  const [cc, code] = [k.slice(0, 2), k.slice(3)];
  return [ccIdx.get(cc), code, admin1.get(k) ?? ''];
});
const fcodes = [...new Set(places.map((p) => p.fc))].sort();
const fcIdx = new Map(fcodes.map((c, i) => [c, i]));
const names = [...new Set(places.map((p) => p.name))];
const nameIdx = new Map(names.map((c, i) => [c, i]));
if (adminTable.length > 65535) throw new Error('too many admin1 entries');

const n = places.length;
const header = Buffer.from(JSON.stringify({ v: 2, n, attribution: ATTRIBUTION, cc: ccList, admin: adminTable, fcode: fcodes, names }), 'utf8');
let off = 8 + header.length;
const pad = ((off + 3) & ~3) - off;
const body = Buffer.alloc(8 + header.length + pad + n * (4 + 4 + 4 + 4 + 2 + 1));
body.write('PKG1', 0, 'latin1');
body.writeUInt32LE(header.length, 4);
header.copy(body, 8);
off += pad;
// numeric columns are byte-plane shuffled (all low bytes, then the next byte…) — gzip loves that
const column = (size, write) => {
  const tmp = Buffer.alloc(n * size);
  places.forEach((p, i) => write(tmp, p, i * size));
  for (let k = 0; k < size; k++) for (let i = 0; i < n; i++) body[off + k * n + i] = tmp[i * size + k];
  off += n * size;
};
column(4, (b, p, o) => b.writeInt32LE(Math.round(p.lat * 1e5), o));
column(4, (b, p, o) => b.writeInt32LE(Math.round(p.lon * 1e5), o));
column(4, (b, p, o) => b.writeUInt32LE(Math.min(0xffffffff, p.pop), o));
column(4, (b, p, o) => b.writeUInt32LE(nameIdx.get(p.name), o));
column(2, (b, p, o) => b.writeUInt16LE(adminIdx.get(`${p.cc}.${p.a1}`), o));
column(1, (b, p, o) => b.writeUInt8(fcIdx.get(p.fc), o));

const gz = zlib.gzipSync(body, { level: 9 });
fs.mkdirSync(path.dirname(outFile), { recursive: true });
fs.writeFileSync(outFile, gz);
fs.writeFileSync(
  path.join(path.dirname(outFile), 'ATTRIBUTION.txt'),
  `cities.bin — offline reverse geocoding data for PeekAround (gzip, layout: packages/shared/src/geo.ts)\n` +
    `${ATTRIBUTION}\nhttps://creativecommons.org/licenses/by/4.0/\n` +
    `Derived from cities5000.zip, admin1CodesASCII.txt and ${DENSE.map((c) => `${c}.zip`).join(', ')} (${new Date().toISOString().slice(0, 10)}).\n`,
);
console.log(`${outFile}: ${n} places, ${names.length} names, ${(body.length / 1e6).toFixed(2)} MB raw → ${(gz.length / 1e6).toFixed(2)} MB gzip`);
