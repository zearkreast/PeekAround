import fs from 'node:fs';
import path from 'node:path';
import sharp from 'sharp';
import { ThumbService } from '/home/user/peekaround/packages/server/src/media/thumbs';
import { Ffmpeg } from '/home/user/peekaround/packages/server/src/ffmpeg';
import { closeHeicWorker } from '/home/user/peekaround/packages/server/src/media/heic';

const [listFile, outDir] = process.argv.slice(2);
const files: { abs: string; kind: 'image' | 'video'; ext: string; duration?: number }[] = JSON.parse(fs.readFileSync(listFile, 'utf8'));
fs.mkdirSync(outDir, { recursive: true });
const ff = new Ffmpeg('ffmpeg', 'ffprobe');
await ff.check();
const ts = new ThumbService(path.join(outDir, 'cache'), ff);
const results: unknown[] = [];
let i = 0;
for (const f of files) {
  const id = String(i++).padStart(3, '0');
  const r: Record<string, unknown> = { id, abs: f.abs };
  try {
    const m = await sharp(f.abs, { failOn: 'none' }).metadata();
    r.meta = { width: m.width, height: m.height, pages: m.pages ?? null, pageHeight: m.pageHeight ?? null, orientation: m.orientation ?? null, format: m.format };
  } catch (e) {
    r.meta = { error: String(e) };
  }
  const row = { kind: f.kind, ext: f.ext, mtime: 0, size: 0, duration: f.duration ?? null } as any;
  const key = ts.key(f.abs, row);
  try {
    const t0 = performance.now();
    r.base = await ts.generateBase(f.abs, row, 'high');
    r.baseMs = performance.now() - t0;
    for (const v of ['640.webp', '320.webp']) if (fs.existsSync(ts.file(key, v))) fs.copyFileSync(ts.file(key, v), path.join(outDir, `${id}-${v}`));
    const f640 = path.join(outDir, `${id}-640.webp`);
    if (fs.existsSync(f640)) {
      const small = await sharp(fs.readFileSync(f640)).resize(100, 100, { fit: 'inside' }).ensureAlpha().raw().toBuffer({ resolveWithObject: true });
      fs.writeFileSync(path.join(outDir, `${id}-hash.rgba`), small.data);
      r.hashInput = { w: small.info.width, h: small.info.height };
    }
  } catch (e) {
    r.baseErr = String(e);
  }
  try {
    const p = await ts.thumb(f.abs, row, 1600, 'high');
    if (p) fs.copyFileSync(p, path.join(outDir, `${id}-1600.webp`));
  } catch (e) {
    r.t1600Err = String(e);
  }
  if (f.kind === 'image') {
    for (const max of [null, 2048]) {
      try {
        const p = await ts.display(f.abs, row, max);
        if (p) fs.copyFileSync(p, path.join(outDir, `${id}-display-${max ?? 'full'}${path.extname(p)}`));
      } catch (e) {
        r[`display${max ?? 'full'}Err`] = String(e);
      }
    }
  }
  for (const n of fs.readdirSync(outDir).filter((x) => x.startsWith(`${id}-`) && !x.endsWith('.rgba'))) {
    const m = await sharp(path.join(outDir, n)).metadata();
    (r.outputs ??= {} as any)[n.slice(4)] = { w: m.width, h: m.height, bytes: fs.statSync(path.join(outDir, n)).size, format: m.format, progressive: m.isProgressive ?? null, exif: !!m.exif, icc: !!m.icc };
  }
  results.push(r);
  console.error(id, f.abs, r.baseErr ?? '', r.baseMs ?? '');
}
fs.writeFileSync(path.join(outDir, 'results.json'), JSON.stringify(results, null, 1));
await closeHeicWorker();
process.exit(0);
