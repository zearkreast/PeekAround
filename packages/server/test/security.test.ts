import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createServer, type PeekServer } from '../src/app';
import { hostName, isAllowedHost, isAllowedOrigin } from '../src/security';
import { isInside, resolveHostPath, sanitizeName, trimChars } from '../src/paths';

describe('security helpers', () => {
  it('parses Host headers', () => {
    expect(hostName('Example.com:8080')).toBe('example.com');
    expect(hostName('[::1]:8080')).toBe('::1');
    expect(hostName('192.168.1.5')).toBe('192.168.1.5');
  });

  it('allows local hosts and blocks rebinding domains', () => {
    for (const h of ['localhost:47213', '127.0.0.1:8080', '192.168.1.20:8080', '[::1]:1', 'nas:8080', 'nas.local', 'pc.lan', 'box.home.arpa', undefined])
      expect(isAllowedHost(h), String(h)).toBe(true);
    for (const h of ['evil.example.com', 'attacker.net:8080', '127.0.0.1.nip.io']) expect(isAllowedHost(h), h).toBe(false);
    expect(isAllowedHost('photos.example.com', ['photos.example.com'])).toBe(true);
    expect(isAllowedHost('a.example.com', ['*.example.com'])).toBe(true);
    expect(isAllowedHost('anything.com', ['*'])).toBe(true);
  });

  it('allows same origin, the app origin and loopback dev servers only', () => {
    expect(isAllowedOrigin(undefined, '127.0.0.1:8080')).toBe(true);
    expect(isAllowedOrigin('http://127.0.0.1:8080', '127.0.0.1:8080')).toBe(true);
    expect(isAllowedOrigin('http://192.168.1.5:8080', '192.168.1.5:8080')).toBe(true);
    expect(isAllowedOrigin('http://localhost', '192.168.1.5:8080')).toBe(true); // Android app
    expect(isAllowedOrigin('https://evil.example', '127.0.0.1:8080')).toBe(false);
    expect(isAllowedOrigin('null', '127.0.0.1:8080')).toBe(false);
    expect(isAllowedOrigin('http://localhost:5173', '127.0.0.1:8787')).toBe(false);
    expect(isAllowedOrigin('http://localhost:5173', '127.0.0.1:8787', [], true)).toBe(true);
    expect(isAllowedOrigin('https://my.app', '127.0.0.1:8787', ['https://my.app'])).toBe(true);
  });

  it('path helpers are linear and strict', () => {
    const t0 = Date.now();
    expect(trimChars('/'.repeat(200000) + 'a' + '/'.repeat(200000), '/')).toBe('a');
    expect(sanitizeName('x' + '. '.repeat(100000) + '!').length).toBeLessThanOrEqual(200);
    expect(sanitizeName('name' + ' .'.repeat(100000))).toBe('name');
    expect(Date.now() - t0).toBeLessThan(1000);
    expect(isInside('/a/b', '/a/b/c')).toBe(true);
    expect(isInside('/a/b', '/a/bc')).toBe(false);
    expect(isInside('/a/b', '/a/b/../c')).toBe(false);
    expect(() => resolveHostPath('/etc', ['/home'])).toThrow();
    expect(resolveHostPath('/home/x/../x/y', ['/home'])).toBe(path.resolve('/home/x/y'));
    expect(() => resolveHostPath('/a\0b', [])).toThrow();
  });
});

describe('security hooks', () => {
  let srv: PeekServer;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-sec-'));
  beforeAll(async () => {
    fs.mkdirSync(path.join(tmp, 'lib'), { recursive: true });
    srv = await createServer({ dataDir: path.join(tmp, 'data'), roots: [{ path: path.join(tmp, 'lib'), name: 'Lib' }], port: 0, host: '127.0.0.1', mdns: false });
  });
  afterAll(async () => {
    await srv.close();
  });

  it('refuses unknown Host names (DNS rebinding)', async () => {
    const r = await srv.app.inject({ url: '/api/info', headers: { host: 'evil.example.com' } });
    expect(r.statusCode).toBe(421);
    expect((await srv.app.inject({ url: '/api/info', headers: { host: '192.168.1.9:8080' } })).statusCode).toBe(200);
  });

  it('refuses cross-origin writes but allows same-origin and the app', async () => {
    const bad = await srv.app.inject({ method: 'POST', url: '/api/fs/mkdir', headers: { host: '127.0.0.1:8080', origin: 'https://evil.example' }, payload: { folderId: '1', dir: '', name: 'x' } });
    expect(bad.statusCode).toBe(403);
    const sneaky = await srv.app.inject({ method: 'POST', url: '/api/fs/mkdir', headers: { host: '127.0.0.1:8080', 'sec-fetch-site': 'cross-site' }, payload: { folderId: '1', dir: '', name: 'x' } });
    expect(sneaky.statusCode).toBe(403);
    const ok = await srv.app.inject({ method: 'POST', url: '/api/fs/mkdir', headers: { host: '127.0.0.1:8080', origin: 'http://127.0.0.1:8080' }, payload: { folderId: '1', dir: '', name: 'ok' } });
    expect(ok.statusCode).toBe(200);
    const app = await srv.app.inject({ method: 'POST', url: '/api/fs/mkdir', headers: { host: '192.168.1.9:8080', origin: 'http://localhost' }, payload: { folderId: '1', dir: '', name: 'ok2' } });
    expect(app.statusCode).toBe(200);
  });

  it('only reflects CORS for allowed origins', async () => {
    const good = await srv.app.inject({ url: '/api/info', headers: { host: '192.168.1.9:8080', origin: 'http://localhost' } });
    expect(good.headers['access-control-allow-origin']).toBe('http://localhost');
    const bad = await srv.app.inject({ url: '/api/info', headers: { host: '192.168.1.9:8080', origin: 'https://evil.example' } });
    expect(bad.headers['access-control-allow-origin']).toBeUndefined();
  });

  it('rate limits file-system routes', async () => {
    let last = 0;
    for (let i = 0; i < 260; i++) {
      last = (await srv.app.inject({ url: '/api/host/dirs', headers: { host: '127.0.0.1:8080' }, remoteAddress: '10.9.9.9' })).statusCode;
      if (last === 429) break;
    }
    expect(last).toBe(429);
  });
});
