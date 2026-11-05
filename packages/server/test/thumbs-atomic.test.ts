import { describe, expect, it } from 'vitest';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import sharp from 'sharp';
import type { Ffmpeg } from '../src/ffmpeg';
import { ThumbService } from '../src/media/thumbs';

/**
 * Regression: generateBase wrote the 640 px thumbnail in place, so a request arriving while it was
 * being written found an existing (empty / partial) file and served it with an immutable cache header.
 */
describe('thumbnail cache files appear atomically', () => {
  it('a 640/320 file is never observed before it is complete', async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-atomic-'));
    let partial = 0;
    for (let k = 0; k < 4; k++) {
      const img = path.join(dir, `i${k}.jpg`);
      await sharp({ create: { width: 2400, height: 1600, channels: 3, background: '#808080', noise: { type: 'gaussian', mean: 128, sigma: 60 } } as never }).jpeg().toFile(img);
      const st = fs.statSync(img);
      const ts = new ThumbService(path.join(dir, `cache${k}`), { available: false } as unknown as Ffmpeg);
      const row = { kind: 'image', ext: 'jpg', mtime: Math.round(st.mtimeMs), size: st.size, duration: null } as const;
      const files = ['640.webp', '320.webp'].map((v) => ts.file(ts.key(img, row), v));
      const seen: number[] = [-1, -1];
      let stop = false;
      const poll = (async () => {
        while (!stop && seen.some((s) => s < 0)) {
          for (let i = 0; i < files.length; i++) {
            if (seen[i] >= 0) continue;
            const s = await fsp.stat(files[i]).catch(() => null);
            if (s) seen[i] = s.size;
          }
          await new Promise((r) => setImmediate(r));
        }
      })();
      await ts.generateBase(img, row, 'high');
      stop = true;
      await poll;
      files.forEach((f, i) => {
        if (seen[i] >= 0 && seen[i] !== fs.statSync(f).size) partial++;
      });
      expect(fs.readdirSync(path.dirname(files[0])).filter((n) => n.endsWith('.tmp'))).toEqual([]);
    }
    expect(partial).toBe(0);
  }, 60_000);
});
