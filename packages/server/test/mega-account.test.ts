import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import type { FolderRef, Job, MegaAccountStatus, MegaLoginResult, ServerEvent } from '@peekaround/shared';
import { createServer, type PeekServer } from '../src/app';
import { parseDetails, planOf } from '../src/remote/account';
import { FakeMega } from './helpers/fake-mega';

/**
 * Optional MEGA account login, end to end against a fake MEGA gateway: the REAL megajs Storage login
 * (v2: PBKDF2 → uh, RSA csid → sid), the real link client (sid on "f" / "g"), the import job.
 */

const FIX = path.join(__dirname, '../../../fixtures/library');
const PASSWORD = 'correct horse battery staple Ünïcode-Pässwörd 42';
const EMAIL = 'peek.user@example.com';
const tick = (ms = 50) => new Promise((r) => setTimeout(r, ms));

async function until<T>(fn: () => T | Promise<T>, ms = 20_000): Promise<NonNullable<T>> {
  const t0 = Date.now();
  for (;;) {
    const v = await fn();
    if (v) return v as NonNullable<T>;
    if (Date.now() - t0 > ms) throw new Error('timeout');
    await tick(40);
  }
}

describe('MEGA account helpers', () => {
  it('maps account types and transfer numbers', () => {
    expect(planOf(0)).toEqual({ type: 'free', plan: null });
    expect(planOf(1)).toEqual({ type: 'pro', plan: 'Pro I' });
    expect(planOf(4)).toEqual({ type: 'pro', plan: 'Pro Lite' });
    expect(planOf(100)).toEqual({ type: 'business', plan: 'Business' });
    expect(planOf(null)).toEqual({ type: null, plan: null });
    expect(parseDetails({ email: 'a@b.c' }, { utype: 2, mxfer: 1000, caxfer: 10 })).toEqual({ email: 'a@b.c', utype: 2, transferUsed: 10, transferTotal: 1000 });
    // free accounts: hourly history, no fixed total
    expect(parseDetails({}, { utype: 0, tah: [5, 6, 'x'] })).toEqual({ email: null, utype: 0, transferUsed: 11, transferTotal: null });
  });
});

describe('MEGA account login', () => {
  let srv: PeekServer;
  let mega: FakeMega;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-mega-acc-'));
  const lib = path.join(tmp, 'lib');
  const events: ServerEvent[] = [];
  let folderLink = '';
  let fileLink = '';
  let folder: FolderRef;
  let jpg: Buffer;
  const inject = (method: string, p: string, body?: unknown) => srv.app.inject({ method: method as 'GET', url: p, payload: body as object });
  const status = async () => (await inject('GET', '/api/remote/account')).json() as MegaAccountStatus;
  const login = async (body: Record<string, unknown>) => inject('POST', '/api/remote/account/login', body);
  const stored = () => srv.ctx.db.getSetting<Record<string, unknown>>('megaSession', {});
  const currentSid = () => String(stored().sid ?? '');

  beforeAll(async () => {
    fs.mkdirSync(path.join(lib, 'Local'), { recursive: true });
    jpg = fs.readFileSync(path.join(FIX, 'Fotoğraflar/2024 Yaz Tatili/IMG_2400.jpg'));
    mega = await new FakeMega().start();
    mega.accounts.push({ email: EMAIL, password: PASSWORD, utype: 1, mxfer: 2 * 1024 ** 4, caxfer: 5 * 1024 ** 3 });
    mega.accounts.push({ email: 'twofa@example.com', password: PASSWORD, mfa: '123456', utype: 0 });
    folderLink = await mega.addFolderLink('Fold1234', 'Trip', [
      { h: 'FiLe0001', name: 'a.jpg', data: jpg },
      { h: 'FiLe0002', name: 'notes.txt', data: Buffer.from('hello mega') },
    ]);
    fileLink = await mega.addFileLink('OneF1234', 'single.jpg', jpg);
    srv = await createServer({ mode: 'desktop', dataDir: path.join(tmp, 'data'), roots: [{ path: lib, name: 'Lib' }], port: 0, host: '127.0.0.1', mdns: false, megaGateway: mega.url });
    srv.ctx.remote.metaGapMs = 0;
    srv.ctx.events.on((e) => events.push(e));
  });

  afterAll(async () => {
    await srv?.close();
    await mega?.close();
    fs.rmSync(tmp, { recursive: true, force: true });
  });

  it('starts anonymous: link requests carry no sid', async () => {
    expect(await status()).toMatchObject({ loggedIn: false, email: null, expired: false });
    const r = await inject('POST', '/api/remote/folders', { url: folderLink });
    expect(r.statusCode, r.body).toBe(200);
    folder = r.json().folder;
    const f = mega.log.filter((l) => l.a === 'f');
    expect(f.length).toBeGreaterThan(0);
    expect(f.every((l) => l.sid === null && l.n === 'Fold1234')).toBe(true);
  });

  it('refuses wrong credentials and asks for / checks the two-factor code', async () => {
    const wrong = await login({ email: EMAIL, password: 'nope' });
    expect(wrong.statusCode).toBe(200);
    expect(wrong.json()).toEqual({ ok: false, reason: 'credentials' });
    expect(wrong.body).not.toContain('nope');
    expect((await login({ email: 'twofa@example.com', password: PASSWORD })).json()).toEqual({ ok: false, reason: 'mfa-required' });
    expect((await login({ email: 'twofa@example.com', password: PASSWORD, code: '000000' })).json()).toEqual({ ok: false, reason: 'mfa-invalid' });
    expect((await login({ email: 'not-an-email', password: PASSWORD })).statusCode).toBe(400);
    expect((await login({ email: EMAIL, password: PASSWORD, code: '12' })).statusCode).toBe(400);
    expect(stored().sid).toBeUndefined();
    // the real megajs login: us0 → us {user, uh, mfa} — the password itself never goes over the wire
    const us = mega.log.filter((l) => l.a === 'us');
    expect(us.length).toBeGreaterThanOrEqual(3);
    expect(us.find((l) => l.cmd.mfa === '000000')).toBeTruthy();
    expect(JSON.stringify(mega.log)).not.toContain(PASSWORD);
  });

  it('logs in with megajs (2FA too): stores only the session id, never the password', async () => {
    const tf = (await login({ email: 'twofa@example.com', password: PASSWORD, code: '123 456' })).json() as MegaLoginResult;
    expect(tf.ok).toBe(true);
    // pause on the quota (as if MEGA said so anonymously): a login lifts it
    srv.ctx.remote.quotaUntil = Date.now() + 3600_000;
    srv.ctx.db.setSetting('remoteQuota', { until: srv.ctx.remote.quotaUntil });
    const r = await login({ email: EMAIL.toUpperCase(), password: PASSWORD });
    expect(r.statusCode, r.body).toBe(200);
    const res = r.json() as MegaLoginResult;
    expect(res.ok).toBe(true);
    if (!res.ok) return;
    expect(res.account).toMatchObject({ loggedIn: true, email: EMAIL, type: 'pro', plan: 'Pro I', transfer: { used: 5 * 1024 ** 3, total: 2 * 1024 ** 4 }, expired: false });
    expect(srv.ctx.remote.quotaUntil).toBe(0);
    expect(srv.ctx.db.getSetting('remoteQuota', { x: 1 })).toEqual({});

    // stored: sid + email + login time — nothing else
    const s = stored();
    expect(Object.keys(s).sort()).toEqual(['email', 'sid', 'since']);
    expect(mega.sessions.has(String(s.sid))).toBe(true);
    // the previous (2FA) session was ended on MEGA's side
    await until(() => mega.log.some((l) => l.a === 'sml'));

    // the password is nowhere in the database files (the sid is: the scan works)
    const files = [srv.ctx.cfg.dbPath, `${srv.ctx.cfg.dbPath}-wal`, `${srv.ctx.cfg.dbPath}-shm`].filter((f) => fs.existsSync(f));
    const bytes = Buffer.concat(files.map((f) => fs.readFileSync(f)));
    expect(bytes.includes(Buffer.from(PASSWORD))).toBe(false);
    expect(bytes.includes(Buffer.from(PASSWORD, 'utf16le'))).toBe(false);
    expect(bytes.includes(Buffer.from(String(s.sid)))).toBe(true);
    // ... nor anywhere under the data dir
    const walk = (d: string): string[] => fs.readdirSync(d, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? walk(path.join(d, e.name)) : [path.join(d, e.name)]));
    for (const f of walk(path.join(tmp, 'data'))) expect(fs.readFileSync(f).includes(Buffer.from(PASSWORD)), f).toBe(false);

    // the sid never reaches clients
    for (const p of ['/api/remote/account', '/api/settings', '/api/libraries', '/api/info']) {
      const b = (await inject('GET', p)).body;
      expect(b, p).not.toContain(String(s.sid));
      expect(b, p).not.toContain(PASSWORD);
    }
    expect(JSON.stringify(events)).not.toContain(String(s.sid));
    expect(events.some((e) => e.type === 'megaAccount')).toBe(true);
    // account details were fetched WITH the session
    expect(mega.withSid(String(s.sid)).map((l) => l.a)).toEqual(expect.arrayContaining(['ug', 'uq']));
  });

  it('attaches the sid to listing and download requests (streaming + import job)', async () => {
    const sid = currentSid();
    await inject('POST', `/api/remote/folders/${folder.id}/refresh`);
    expect(mega.log.filter((l) => l.a === 'f').at(-1)).toMatchObject({ sid, n: 'Fold1234' });
    // streaming: "g" for the node, with the folder's n AND the account's sid
    const row = srv.ctx.db.get<{ id: number }>("SELECT id FROM media WHERE folder_id = ? AND name = 'a.jpg'", Number(folder.id))!;
    // (the background thumbnail pass may have cached the original while anonymous)
    await srv.ctx.remote.cache.clear();
    const before = mega.log.length;
    const res = await inject('GET', `/api/media/${row.id}/file`);
    expect(res.statusCode).toBe(200);
    expect(res.rawPayload.equals(jpg)).toBe(true);
    const g = mega.log.slice(before).filter((l) => l.a === 'g');
    expect(g.length).toBeGreaterThan(0);
    expect(g.every((l) => l.sid === sid && l.n === 'Fold1234' && l.cmd.g === 1 && l.cmd.n === 'FiLe0001')).toBe(true);

    // import job (megajs File.download, MAC checked): file link → local folder
    const local = srv.ctx.lib.listLibraries().find((l) => l.name === 'Lib')!.folders[0];
    const at = mega.log.length;
    const job = (await inject('POST', '/api/remote/import', { url: fileLink, mode: 'extract', folderId: local.id, dir: 'Local' })).json() as Job;
    const done = await until(() => {
      const j = srv.ctx.jobs.get(job.id);
      return j && j.status !== 'queued' && j.status !== 'running' ? j : null;
    });
    expect(done.status, done.error ?? '').toBe('done');
    expect(fs.readFileSync(path.join(lib, 'Local', 'single.jpg')).equals(jpg)).toBe(true);
    const imp = mega.log.slice(at).filter((l) => l.a === 'g');
    expect(imp.map((l) => [l.cmd.p, !!l.cmd.g])).toEqual(expect.arrayContaining([['OneF1234', false], ['OneF1234', true]]));
    expect(imp.every((l) => l.sid === sid)).toBe(true);
  });

  it('an expired session: dropped, shown as expired, requests fall back to anonymous', async () => {
    const sid = currentSid();
    mega.sessions.delete(sid); // e.g. "log out of all devices" on mega.nz
    const at = mega.log.length;
    const r = await inject('POST', `/api/remote/folders/${folder.id}/refresh`);
    expect(r.statusCode, r.body).toBe(200);
    const f = mega.log.slice(at).filter((l) => l.a === 'f');
    expect(f.map((l) => l.sid)).toEqual([sid, null]); // ESID → retried without the session
    expect(stored().sid).toBeUndefined();
    expect(await status()).toMatchObject({ loggedIn: false, expired: true, email: EMAIL });
    expect(events.filter((e) => e.type === 'megaAccount').length).toBeGreaterThan(1);
    // later requests: anonymous
    const b2 = mega.log.length;
    expect((await inject('POST', `/api/remote/folders/${folder.id}/refresh`)).statusCode).toBe(200);
    expect(mega.log.slice(b2).map((l) => [l.a, l.sid])).toEqual([['f', null]]);
  });

  it('a session MEGA rejects on the status check is dropped too', async () => {
    expect((await login({ email: EMAIL, password: PASSWORD })).json().ok).toBe(true);
    mega.rejectSessions = true;
    try {
      // the status cache is fresh right after a login: force a check like the UI's poll after a minute
      const st = await srv.ctx.megaAccount.refreshedStatus(true);
      expect(st).toMatchObject({ loggedIn: false, expired: true });
    } finally {
      mega.rejectSessions = false;
    }
  });

  it('logout: "sml" with the session, then the sid is gone and requests are anonymous', async () => {
    expect((await login({ email: EMAIL, password: PASSWORD })).json().ok).toBe(true);
    const sid = currentSid();
    const r = await inject('POST', '/api/remote/account/logout', {});
    expect(r.statusCode).toBe(200);
    expect(r.json()).toMatchObject({ loggedIn: false, email: null, expired: false });
    expect(mega.log.filter((l) => l.a === 'sml').at(-1)?.sid).toBe(sid);
    expect(mega.sessions.has(sid)).toBe(false);
    expect(stored()).toEqual({});
    const at = mega.log.length;
    await inject('POST', `/api/remote/folders/${folder.id}/refresh`);
    expect(mega.log.slice(at).every((l) => l.sid === null)).toBe(true);
  });

  it('LAN clients of the desktop get 403 on every account route', async () => {
    const lan = await srv.setLan(true, 0 as unknown as number).catch(() => null);
    const port = (srv.lanServer?.address() as { port: number } | null)?.port;
    expect(lan && port).toBeTruthy();
    const call = (method: string, p: string, body?: object) =>
      new Promise<{ status: number; body: string }>((resolve, reject) => {
        const data = body ? JSON.stringify(body) : undefined;
        const req = http.request({ host: '127.0.0.1', port, path: p, method, headers: data ? { 'content-type': 'application/json', 'content-length': Buffer.byteLength(data) } : {} }, (res) => {
          let b = '';
          res.on('data', (c) => (b += c));
          res.on('end', () => resolve({ status: res.statusCode ?? 0, body: b }));
        });
        req.on('error', reject);
        req.end(data);
      });
    try {
      const before = mega.log.length;
      expect((await call('GET', '/api/remote/account')).status).toBe(403);
      expect((await call('POST', '/api/remote/account/login', { email: EMAIL, password: PASSWORD })).status).toBe(403);
      expect((await call('POST', '/api/remote/account/logout', {})).status).toBe(403);
      expect(mega.log.length).toBe(before); // nothing reached MEGA
      expect(stored().sid).toBeUndefined();
      // the LAN may still browse
      expect((await call('GET', '/api/libraries')).status).toBe(200);
    } finally {
      await srv.setLan(false);
    }
  });
});
