import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import crypto from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import sharp from 'sharp';
import yazl from 'yazl';
import { createServer, type PeekServer } from '../src/app';

const sha = (f: string) => crypto.createHash('sha256').update(fs.readFileSync(f)).digest('hex');

describe('API integration', () => {
  let srv: PeekServer;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-api-'));
  const lib = path.join(tmp, 'lib');
  const photo = path.join(lib, 'Tatil', 'deniz.jpg');

  beforeAll(async () => {
    fs.mkdirSync(path.dirname(photo), { recursive: true });
    await sharp({ create: { width: 640, height: 400, channels: 3, background: '#3b82f6' } })
      .withExif({ IFD2: { DateTimeOriginal: '2024:07:01 10:00:00' } })
      .jpeg()
      .toFile(photo);
    await sharp({ create: { width: 300, height: 500, channels: 3, background: '#f97316' } }).png().toFile(path.join(lib, 'portre.png'));
    srv = await createServer({ dataDir: path.join(tmp, 'data'), roots: [{ path: lib, name: 'Lib' }], port: 0, host: '127.0.0.1', mdns: false });
    // wait for scan + metadata
    for (let i = 0; i < 50; i++) {
      const r = await srv.app.inject('/api/browse?folderId=1&recursive=1');
      const items = r.json().items as { width: number | null }[];
      if (items.length === 2 && items.every((x) => x.width)) break;
      await new Promise((res) => setTimeout(res, 100));
    }
  });

  afterAll(async () => {
    await srv.close();
  });

  it('browses folders with metadata and EXIF date', async () => {
    const r = (await srv.app.inject('/api/browse?folderId=1')).json();
    expect(r.folders.map((f: { name: string }) => f.name)).toEqual(['Tatil']);
    expect(r.items).toHaveLength(1);
    const all = (await srv.app.inject('/api/browse?folderId=1&recursive=1')).json();
    const deniz = all.items.find((i: { name: string }) => i.name === 'deniz.jpg');
    expect([deniz.width, deniz.height]).toEqual([640, 400]);
    expect(new Date(deniz.takenAt).getFullYear()).toBe(2024);
  });

  it('serves thumbnails and ranges', async () => {
    const id = (await srv.app.inject('/api/browse?folderId=1&dir=Tatil')).json().items[0].id;
    const t = await srv.app.inject(`/api/media/${id}/thumb?s=320`);
    expect(t.statusCode).toBe(200);
    expect(t.headers['content-type']).toContain('image/webp');
    const r = await srv.app.inject({ url: `/api/media/${id}/file`, headers: { range: 'bytes=0-9' } });
    expect(r.statusCode).toBe(206);
    expect(r.rawPayload.length).toBe(10);
  });

  it('uploads with folder structure and unique names', async () => {
    const body = fs.readFileSync(photo);
    for (let i = 0; i < 2; i++) {
      const r = await srv.app.inject({ method: 'PUT', url: '/api/upload?folderId=1&dir=&path=Yeni%2Fa.jpg', payload: body, headers: { 'content-type': 'application/octet-stream' } });
      expect(r.statusCode).toBe(200);
    }
    expect(fs.readdirSync(path.join(lib, 'Yeni')).sort()).toEqual(['a (1).jpg', 'a.jpg']);
  });

  it('extracts zips permanently or as temporary sessions', async () => {
    const z = new yazl.ZipFile();
    z.addBuffer(fs.readFileSync(photo), 'Albüm/1.jpg');
    z.end();
    const chunks: Buffer[] = [];
    for await (const c of z.outputStream) chunks.push(c as Buffer);
    const zip = Buffer.concat(chunks);
    const ex = await srv.app.inject({ method: 'PUT', url: '/api/upload?folderId=1&path=arsiv.zip&zip=extract', payload: zip, headers: { 'content-type': 'application/octet-stream' } });
    expect(ex.json().extractedDir).toBe('arsiv');
    expect(fs.existsSync(path.join(lib, 'arsiv', 'Albüm', '1.jpg'))).toBe(true);
    const tp = await srv.app.inject({ method: 'PUT', url: '/api/upload?folderId=1&path=arsiv.zip&zip=temp', payload: zip, headers: { 'content-type': 'application/octet-stream' } });
    const temp = tp.json().tempFolder;
    expect(temp.temp).toBe(true);
    expect(fs.existsSync(path.join(lib, 'arsiv.zip'))).toBe(false);
    const libs = (await srv.app.inject('/api/libraries')).json();
    expect(libs.temp.map((t: { id: string }) => t.id)).toContain(temp.id);
  });

  it('saves edits as a copy and never touches the original', async () => {
    const before = sha(photo);
    const id = (await srv.app.inject('/api/browse?folderId=1&dir=Tatil')).json().items[0].id;
    const png = await sharp({ create: { width: 64, height: 40, channels: 3, background: '#000' } }).png().toBuffer();
    for (const expected of ['deniz_edit.jpg', 'deniz_edit-2.jpg']) {
      const r = await srv.app.inject({ method: 'POST', url: `/api/edits/save?sourceId=${id}&format=jpg`, payload: png, headers: { 'content-type': 'application/octet-stream' } });
      expect(r.json().item.name).toBe(expected);
    }
    expect(sha(photo)).toBe(before);
    const meta = await sharp(path.join(lib, 'Tatil', 'deniz_edit.jpg')).metadata();
    expect(meta.exif).toBeTruthy(); // capture date carried over
  });

  it('renames, moves and deletes into the trash folder', async () => {
    const ref = { folderId: '1', rel: 'portre.png' };
    const rn = await srv.app.inject({ method: 'POST', url: '/api/fs/rename', payload: { ...ref, name: 'portre2.png' } });
    expect(rn.json().rel).toBe('portre2.png');
    await srv.app.inject({ method: 'POST', url: '/api/fs/move', payload: { refs: [{ folderId: '1', rel: 'portre2.png' }], to: { folderId: '1', dir: 'Tatil' } } });
    expect(fs.existsSync(path.join(lib, 'Tatil', 'portre2.png'))).toBe(true);
    await srv.app.inject({ method: 'POST', url: '/api/fs/delete', payload: { refs: [{ folderId: '1', rel: 'Tatil/portre2.png' }] } });
    expect(fs.existsSync(path.join(lib, 'Tatil', 'portre2.png'))).toBe(false);
    expect(fs.readdirSync(path.join(lib, '.peekaround-trash')).length).toBe(1);
  });

  it('accepts custom fonts and reads the family name', async () => {
    const font = ['/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf', '/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Medium.ttf'].find((f) => fs.existsSync(f));
    if (!font) return;
    const r = await srv.app.inject({ method: 'PUT', url: `/api/fonts?name=${path.basename(font)}`, payload: fs.readFileSync(font), headers: { 'content-type': 'application/octet-stream' } });
    const fonts = r.json().fonts;
    expect(fonts).toHaveLength(1);
    expect(fonts[0].family.length).toBeGreaterThan(2);
    expect((await srv.app.inject(fonts[0].url)).statusCode).toBe(200);
  });

  it('rejects path traversal', async () => {
    const r = await srv.app.inject({ method: 'PUT', url: '/api/upload?folderId=1&dir=../../etc&path=x.jpg', payload: 'x', headers: { 'content-type': 'application/octet-stream' } });
    expect(r.statusCode).toBe(400);
  });
});
