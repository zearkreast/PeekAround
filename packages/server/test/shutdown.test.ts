import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import sharp from 'sharp';
import { createServer } from '../src/app';
import { FakePhone } from './helpers/fake-phone';

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * Regression: close() used to return while the device thumbnail loop (and running jobs) still
 * had work in flight; when that work settled it wrote to the closed database
 * ("database is not open" as an unhandled rejection).
 */
describe('server shutdown with background work in flight', () => {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-close-'));
  const phoneRoot = path.join(tmp, 'phone');
  const unhandled: unknown[] = [];
  const onUnhandled = (e: unknown) => unhandled.push(e);

  beforeAll(async () => {
    fs.mkdirSync(phoneRoot, { recursive: true });
    for (let i = 0; i < 4; i++) {
      await sharp({ create: { width: 64, height: 48, channels: 3, background: '#123456' } }).jpeg().toFile(path.join(phoneRoot, `IMG_${i}.jpg`));
    }
    process.on('unhandledRejection', onUnhandled);
  });
  afterAll(() => {
    process.off('unhandledRejection', onUnhandled);
  });

  it('close() waits for device thumbnails and running jobs; nothing touches the closed database', async () => {
    const srv = await createServer({ mode: 'desktop', dataDir: path.join(tmp, 'data'), roots: [], port: 0, host: '127.0.0.1', mdns: false });
    // thumbnails that only settle (with an error) well after close() was called
    let calls = 0;
    srv.ctx.thumbs.generateBase = (() => {
      calls++;
      return sleep(300).then(() => {
        throw new Error('decode failed');
      });
    }) as typeof srv.ctx.thumbs.generateBase;
    const phone = new FakePhone(srv.url, 'shutdown-phone-1', [{ key: 'cam', name: 'Camera', root: phoneRoot }]);
    expect((await phone.connect()).t).toBe('pending');
    srv.ctx.devices.approve(phone.id);
    await phone.waitFrame((f) => f.t === 'paired');
    for (let i = 0; i < 200 && !calls; i++) await sleep(20);
    expect(calls).toBeGreaterThan(0);

    // a job that ignores nothing: it ends (with a DB write) only after its signal fired
    let jobSawAbort = false;
    srv.ctx.jobs.enqueue('transfer', 'slow', async (jc) => {
      await new Promise<void>((r) => jc.signal.addEventListener('abort', () => r(), { once: true }));
      await sleep(100);
      jobSawAbort = true;
      srv.ctx.db.get('SELECT 1');
      return null;
    });
    await sleep(20);

    await srv.close();
    expect(jobSawAbort).toBe(true);
    await sleep(500);
    expect(unhandled).toEqual([]);
  });
});

describe('device thumbnails that fill in image dimensions', () => {
  it('start a new layout generation (skeleton aspects follow the rows)', async () => {
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-dims-'));
    const root = path.join(tmp, 'phone');
    fs.mkdirSync(root, { recursive: true });
    await sharp({ create: { width: 800, height: 400, channels: 3, background: '#345678' } }).jpeg().toFile(path.join(root, 'wide.jpg'));
    const srv = await createServer({ mode: 'desktop', dataDir: path.join(tmp, 'data'), roots: [], port: 0, host: '127.0.0.1', mdns: false });
    try {
      const phone = new FakePhone(srv.url, 'dims-phone-1', [{ key: 'cam', name: 'Camera', root }]);
      let release!: () => void;
      const gate = new Promise<void>((r) => (release = r));
      const held: string[] = [];
      phone.intercept = (f) => {
        if (f.op !== 'thumb') return false;
        held.push(f.rid);
        void gate.then(() => {
          phone.intercept = null;
          void (phone as unknown as { handle(f: unknown): Promise<void> }).handle(f);
        });
        return true;
      };
      await phone.connect();
      srv.ctx.devices.approve(phone.id);
      await phone.waitFrame((f) => f.t === 'paired');
      for (let i = 0; i < 200 && !held.length; i++) await sleep(20);
      const folderId = srv.ctx.db.get<{ id: number }>('SELECT id FROM folders WHERE device_id = ?', phone.id)!.id;
      const sk1 = (await srv.app.inject(`/api/browse/skeleton?folderId=${folderId}`)).json();
      expect(sk1.total).toBe(1);
      release();
      for (let i = 0; i < 200; i++) {
        if (srv.ctx.db.get<{ w: number | null }>('SELECT width w FROM media WHERE folder_id = ?', folderId)?.w) break;
        await sleep(20);
      }
      await sleep(100);
      const sk2 = (await srv.app.inject(`/api/browse/skeleton?folderId=${folderId}`)).json();
      const page = (await srv.app.inject(`/api/browse?folderId=${folderId}`)).json();
      // (the phone listed no dimensions: they come from the 640 px thumbnail, aspect 2:1)
      expect(page.items[0].width / page.items[0].height).toBe(2);
      expect(sk2.gen).toBe(page.gen);
      expect(sk2.gen).not.toBe(sk1.gen);
      expect(Buffer.from(sk2.aspects, 'base64')[0]).not.toBe(Buffer.from(sk1.aspects, 'base64')[0]);
      phone.close();
    } finally {
      await srv.close();
    }
  });
});
