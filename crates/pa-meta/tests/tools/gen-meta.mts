// Golden fixtures for pa-meta: runs the TypeScript reference (packages/server/src/media/metadata.ts:
// exifr + sharp + Takeout sidecars + groupColumns) on every file under the given roots.
// Run: cd /home/user/peekaround && TZ=UTC node_modules/.bin/tsx gen-meta.ts <out.json> <name=root>...
import fs from 'node:fs';
import path from 'node:path';
import sharp from '/home/user/peekaround/node_modules/sharp/dist/index.mjs';
import { extractMeta, readExif, groupColumns, videoGroupMeta, parseVideoLocation, clearSidecarCache } from '/home/user/peekaround/packages/server/src/media/metadata.ts';
import { Ffmpeg } from '/home/user/peekaround/packages/server/src/ffmpeg.ts';
import { GeoService } from '/home/user/peekaround/packages/server/src/geo.ts';
import { kindOf, extOf } from '/home/user/peekaround/packages/shared/src/index.ts';

const [outFile, ...roots] = process.argv.slice(2);
const ffmpeg = new Ffmpeg('ffmpeg', 'ffprobe');
await ffmpeg.check();
process.env.PEEK_GEO_FILE = '/home/user/peekaround/packages/web/public/geo/cities.bin';
const geo = new GeoService(null);
const ZONES = ['UTC', 'Europe/Istanbul', 'America/New_York', 'Australia/Lord_Howe'];
const MTIME = 1_700_000_000_000;

// an AVIF with EXIF (sharp/libheif can write it, PIL can't)
const craftDir = roots.map((r) => r.split('=')[1]).find((r) => r.endsWith('craft'));
if (craftDir && !fs.existsSync(path.join(craftDir, 'exif.avif'))) {
  await sharp({ create: { width: 48, height: 32, channels: 3, background: '#3366cc' } })
    .avif({ quality: 40 })
    .withExif({ IFD0: { Make: 'AvifMake', Model: 'AvifModel', Orientation: '6' } })
    .toFile(path.join(craftDir, 'exif.avif'));
  await sharp({ create: { width: 30, height: 20, channels: 3, background: '#cc3366' } }).heif({ compression: 'av1', quality: 40 }).toFile(path.join(craftDir, 'av1.heif'));
}

const walk = (dir: string, base = ''): string[] =>
  fs.readdirSync(dir, { withFileTypes: true }).sort((a, b) => (a.name < b.name ? -1 : 1)).flatMap((e) => (e.isDirectory() ? walk(path.join(dir, e.name), base ? `${base}/${e.name}` : e.name) : [base ? `${base}/${e.name}` : e.name]));

const files: unknown[] = [];
for (const spec of roots) {
  const [name, root] = spec.split('=');
  for (const rel of walk(root)) {
    const kind = kindOf(rel);
    if (!kind || kind === 'archive') continue;
    const abs = path.join(root, rel);
    const ext = extOf(rel);
    const dir = rel.includes('/') ? rel.slice(0, rel.lastIndexOf('/')) : '';
    const fileName = path.basename(rel);
    clearSidecarCache();
    const probe = kind === 'video' || kind === 'audio' ? await ffmpeg.probe(abs) : null;
    const meta = await extractMeta(abs, { kind, ext }, ffmpeg);
    clearSidecarCache();
    const known = await extractMeta(abs, { kind, ext }, ffmpeg, { width: 100, height: 50 });
    const exif = kind === 'image' ? await readExif(abs) : null;
    const row = { name: fileName, dir, ext, kind, mtime: MTIME };
    const group: Record<string, unknown> = {};
    for (const tz of ZONES) group[tz] = groupColumns(row, meta, geo, tz);
    files.push({ root: name, rel, kind, ext, probe, meta, known, exif, row, group });
  }
}

// video tags: synthetic ProbeResults (every location tag form + vendor variants + Apple dates)
const probes: unknown[] = [];
const base = { duration: 1, width: 32, height: 32, rotation: 0, codec: 'h264', audioCodec: null, fps: 25, bitrate: 1000, container: 'mov,mp4', creationTime: 1720000000000, hasAudio: false };
const tagSets: Record<string, string>[] = [
  { 'com.apple.quicktime.location.iso6709': '+40.9903+029.0290+012.000/' },
  { 'location-eng': '+36.8841+030.7056/' },
  { 'com.samsung.android.location': '41.0082,28.9784' },
  { 'com.vendor.gps.position': '+48.8566+002.3522/' },
  { title: 'x' },
  { location: '+00.0000+000.0000/', '©xyz': '+41.0123+028.9744/' },
  { gpscoordinates: '-33.86 151.21 12.0' },
  { 'com.android.location': 'nowhere', xyz: '+1+2/' },
  { 'location-tur': '41.0370; 28.9850' },
  { 'com.apple.quicktime.creationdate': '2024-07-03T21:00:00+0300', 'com.apple.quicktime.make': 'Apple', 'com.apple.quicktime.model': 'iPhone 15 Pro' },
  { 'com.apple.quicktime.creationdate': '2024-07-03T21:00:00' },
  { 'com.apple.quicktime.creationdate': 'garbage', make: 'GoPro', model: 'HERO9' },
  { 'com.android.manufacturer': 'Xiaomi', 'com.android.model': 'Redmi 9', make: 'ignored' },
  { gps: '91.0, 10.0', 'com.xyz.location': '+10.5-020.25/' },
  { 'com.apple.quicktime.location.iso6709': '+4100.738+02858.464/' },
  { 'location': ' 41.0370 , 28.9850 , 50 ' },
  {},
];
for (const tags of tagSets) {
  const p = { ...base, tags };
  probes.push({ probe: p, out: videoGroupMeta(p as never) });
}
probes.push({ probe: null, out: videoGroupMeta(null) });
const locs = ['+41.0370+028.9850/', '41.0370, 28.9850', '-33.86 151.21 12.0', 'nowhere', '41.0370;28.9850', '41,28', '+41+28', '410.5, 28', '41.5,181', `${'1'.repeat(90)}`, '  -12.5  ,  -77.25  '];
const locOut = locs.map((s) => ({ s, out: parseVideoLocation(s) }));

fs.writeFileSync(outFile, JSON.stringify({ zones: ZONES, mtime: MTIME, files, probes, locations: locOut }, null, 1));
console.log(files.length, 'files', probes.length, 'probes');
