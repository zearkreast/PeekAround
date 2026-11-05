import { describe, expect, it } from 'vitest';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import yazl from 'yazl';
import { decodeZipName, extractZip } from '../src/zip';

async function makeZip(file: string, entries: { name: string; data: string }[]) {
  // yazl refuses "../" names, so write a same-length placeholder and patch the bytes afterwards
  const z = new yazl.ZipFile();
  for (const e of entries) z.addBuffer(Buffer.from(e.data), e.name.replaceAll('../', 'XX/'));
  z.end();
  const chunks: Buffer[] = [];
  for await (const c of z.outputStream) chunks.push(c as Buffer);
  let buf = Buffer.concat(chunks);
  for (const e of entries.filter((x) => x.name.includes('../'))) {
    const from = Buffer.from(e.name.replaceAll('../', 'XX/'));
    const to = Buffer.from(e.name);
    let i = buf.indexOf(from);
    while (i >= 0) {
      to.copy(buf, i);
      i = buf.indexOf(from, i + 1);
    }
  }
  fs.writeFileSync(file, buf);
}

describe('zip', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-zip-'));

  it('extracts files and skips zip-slip entries', async () => {
    const zip = path.join(dir, 'a.zip');
    await makeZip(zip, [
      { name: 'Düğün/kare 1.jpg', data: 'x' },
      { name: '../../evil.txt', data: 'bad' },
      { name: '__MACOSX/._junk', data: 'y' },
    ]);
    const out = path.join(dir, 'out');
    const r = await extractZip(zip, out);
    expect(r.files).toEqual(['Düğün/kare 1.jpg']);
    expect(fs.existsSync(path.join(dir, 'evil.txt'))).toBe(false);
    expect(fs.existsSync(path.join(out, 'Düğün', 'kare 1.jpg'))).toBe(true);
  });

  it('decodes Turkish DOS (CP857) names', () => {
    // "şğİı.jpg" in CP857: ş=0x9F ğ=0xA7 İ=0x98 ı=0x8D
    const raw = Buffer.from([0x9f, 0xa7, 0x98, 0x8d, 0x2e, 0x6a, 0x70, 0x67]);
    expect(decodeZipName(raw, 0, [])).toBe('şğİı.jpg');
    expect(decodeZipName(Buffer.from('ğüş.png', 'utf8'), 0x800, [])).toBe('ğüş.png');
  });
});
