import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import sharp from 'sharp';
import { ThumbService } from '/home/user/peekaround/packages/server/src/media/thumbs';
import { Ffmpeg } from '/home/user/peekaround/packages/server/src/ffmpeg';
import { closeHeicWorker } from '/home/user/peekaround/packages/server/src/media/heic';

const files: string[] = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
const threads = Number(process.argv[3] ?? 0);
if (threads) sharp.concurrency(threads);
const dir = fs.mkdtempSync('/dev/shm/r2/tsbench-');
const ts = new ThumbService(dir, new Ffmpeg('ffmpeg', 'ffprobe'));
ts.limiter.resize(8, 8);
let n = 0;
const row = (abs: string) => ({ kind: 'image', ext: path.extname(abs).slice(1).toLowerCase(), mtime: ++n, size: 0, duration: null }) as any;
const med = (a: number[]) => a.sort((x, y) => x - y)[Math.floor(a.length / 2)];
const out: Record<string, unknown> = {};
for (const f of files) {
  const N = f.endsWith('.heic') ? 5 : 10;
  const r: Record<string, number> = {};
  for (const [name, op] of [
    ['base', (a: string) => ts.generateBase(a, row(a), 'high')],
    ['t1600', (a: string) => ts.thumb(a, row(a), 1600, 'high')],
    ['display2048', (a: string) => ts.display(a, row(a), 2048)],
  ] as const) {
    await op(f); // warm up
    const times: number[] = [];
    for (let i = 0; i < N; i++) {
      const t0 = performance.now();
      await op(f);
      times.push(performance.now() - t0);
    }
    r[`${name}Ms`] = Math.round(med(times) * 10) / 10;
  }
  // throughput: 4 base renditions in parallel, 3 rounds
  const t0 = performance.now();
  for (let k = 0; k < 3; k++) await Promise.all([0, 1, 2, 3].map(() => ts.generateBase(f, row(f), 'high')));
  r.base4xPerSec = Math.round((12 / ((performance.now() - t0) / 1000)) * 10) / 10;
  out[path.basename(f)] = r;
  console.error(path.basename(f), JSON.stringify(r));
}
console.log(JSON.stringify(out));
fs.rmSync(dir, { recursive: true, force: true });
await closeHeicWorker();
process.exit(0);
