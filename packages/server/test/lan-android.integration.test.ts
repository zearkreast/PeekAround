/**
 * The phone's view of the desktop app's LAN share. Everything goes through `setLan(true)` — the
 * second listener on 0.0.0.0 whose requests are flagged peekLan — reached on this machine's LAN
 * address when it has one, with the headers the Android app sends: its WebView origin
 * http://localhost (capacitor androidScheme http) for the client mode, no Origin for the native
 * link service.
 */
import { afterAll, beforeAll, describe, expect, it, vi } from 'vitest';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import sharp from 'sharp';
import WebSocket from 'ws';
import { createServer, mdnsService, type PeekServer } from '../src/app';
import { lanUrls } from '../src/config';

const APP_ORIGIN = 'http://localhost';

function freePort(): Promise<number> {
  return new Promise((resolve) => {
    const s = net.createServer();
    s.listen(0, '0.0.0.0', () => {
      const p = (s.address() as net.AddressInfo).port;
      s.close(() => resolve(p));
    });
  });
}

/** a non-loopback IPv4 of this machine (what the phone uses), else loopback */
function lanAddress(): string {
  for (const list of Object.values(os.networkInterfaces())) for (const ni of list ?? []) if (ni.family === 'IPv4' && !ni.internal) return ni.address;
  return '127.0.0.1';
}

function wsOpen(url: string, origin?: string): Promise<{ ok: boolean; status?: number; first?: any }> {
  return new Promise((resolve) => {
    const ws = new WebSocket(url, origin ? { headers: { Origin: origin } } : {});
    ws.once('message', (d: Buffer) => {
      resolve({ ok: true, first: JSON.parse(String(d)) });
      ws.close();
    });
    ws.once('unexpected-response', (_req: unknown, res: { statusCode?: number }) => {
      resolve({ ok: false, status: res.statusCode });
      ws.terminate();
    });
    ws.once('error', () => resolve({ ok: false }));
  });
}

describe('desktop LAN share from the Android app', () => {
  let srv: PeekServer;
  let base: string;
  let imgId: string;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-lan-'));
  const lib = path.join(tmp, 'Fotoğraflar');

  beforeAll(async () => {
    fs.mkdirSync(path.join(lib, 'Tatil'), { recursive: true });
    await sharp({ create: { width: 640, height: 400, channels: 3, background: '#3b82f6' } }).jpeg().toFile(path.join(lib, 'Tatil', 'deniz.jpg'));
    srv = await createServer({ mode: 'desktop', dataDir: path.join(tmp, 'data'), roots: [{ path: lib, name: 'Fotoğraflar' }], port: 0, host: '127.0.0.1', mdns: false });
    for (let i = 0; i < 50; i++) {
      const items = (await srv.app.inject('/api/browse?folderId=1&recursive=1')).json().items as { id: string; width: number | null }[];
      if (items.length === 1 && items[0].width) {
        imgId = items[0].id;
        break;
      }
      await new Promise((r) => setTimeout(r, 100));
    }
    const port = await freePort();
    const lan = await srv.setLan(true, port);
    expect(lan.enabled).toBe(true);
    base = `http://${lanAddress()}:${port}`;
  });

  afterAll(async () => {
    await srv.close();
  });

  it('passes the connect screen check (GET /api/info from the app origin)', async () => {
    const r = await fetch(`${base}/api/info`, { headers: { Origin: APP_ORIGIN } });
    expect(r.status).toBe(200);
    expect(r.headers.get('access-control-allow-origin')).toBe(APP_ORIGIN);
    const info = (await r.json()) as any;
    expect(info).toMatchObject({ name: 'PeekAround', mode: 'desktop', lan: { enabled: true } });
    expect(info.lan.urls.some((u: string) => u.endsWith(`:${new URL(base).port}`))).toBe(true);
  });

  it('serves libraries, browse and thumbnails cross-origin', async () => {
    const libs = await fetch(`${base}/api/libraries`, { headers: { Origin: APP_ORIGIN } });
    expect(libs.headers.get('access-control-allow-origin')).toBe(APP_ORIGIN);
    expect(((await libs.json()) as any).libraries[0].name).toBe('Fotoğraflar');
    const browse = (await (await fetch(`${base}/api/browse?folderId=1&recursive=1`, { headers: { Origin: APP_ORIGIN } })).json()) as any;
    expect(browse.items.map((i: { name: string }) => i.name)).toEqual(['deniz.jpg']);
    const sk = await fetch(`${base}/api/browse/skeleton?folderId=1&recursive=1`, { headers: { Origin: APP_ORIGIN } });
    expect(sk.status).toBe(200);
    const t = await fetch(`${base}/api/media/${imgId}/thumb?s=640`, { headers: { Origin: APP_ORIGIN } });
    expect(t.status).toBe(200);
    expect(t.headers.get('content-type')).toContain('image/');
    const f = await fetch(`${base}/api/media/${imgId}/file`, { headers: { Origin: APP_ORIGIN, Range: 'bytes=0-9' } });
    expect(f.status).toBe(206);
  });

  it('answers the CORS preflight of a linked phone (Authorization + X-Peek-Device, PATCH)', async () => {
    const r = await fetch(`${base}/api/settings`, {
      method: 'OPTIONS',
      headers: { Origin: APP_ORIGIN, 'Access-Control-Request-Method': 'PATCH', 'Access-Control-Request-Headers': 'authorization,content-type,x-peek-device' },
    });
    expect(r.status).toBe(204);
    expect(r.headers.get('access-control-allow-origin')).toBe(APP_ORIGIN);
    expect(r.headers.get('access-control-allow-methods')).toContain('PATCH');
    expect(r.headers.get('access-control-allow-headers')?.toLowerCase()).toContain('x-peek-device');
    const p = await fetch(`${base}/api/settings`, { method: 'PATCH', headers: { Origin: APP_ORIGIN, 'Content-Type': 'application/json' }, body: JSON.stringify({ showHidden: false }) });
    expect(p.status).toBe(200);
  });

  it('LAN-only refusals stay readable by the app (403 with CORS, not a network error)', async () => {
    const r = await fetch(`${base}/api/remote/account`, { headers: { Origin: APP_ORIGIN } });
    expect(r.status).toBe(403);
    expect(r.headers.get('access-control-allow-origin')).toBe(APP_ORIGIN);
    // the same request from the PC window works
    expect((await srv.app.inject('/api/remote/account')).statusCode).toBe(200);
  });

  it('opens the live events socket from the app origin; foreign origins are refused', async () => {
    const ws = `${base.replace(/^http/, 'ws')}/api/events`;
    const ok = await wsOpen(ws, APP_ORIGIN);
    expect(ok.ok).toBe(true);
    expect(ok.first).toMatchObject({ type: 'hello', info: { name: 'PeekAround' } });
    // the native link service sends no Origin
    expect((await wsOpen(ws)).ok).toBe(true);
    const evil = await wsOpen(ws, 'https://evil.example');
    expect(evil).toMatchObject({ ok: false, status: 403 });
  });

  it('stays reachable after sharing is switched off and on again (tray / settings toggle)', async () => {
    const port = Number(new URL(base).port);
    await srv.setLan(false);
    await expect(fetch(`${base}/api/info`)).rejects.toThrow();
    await srv.setLan(true, port);
    expect((await fetch(`${base}/api/info`, { headers: { Origin: APP_ORIGIN } })).status).toBe(200);
  });
});

describe('discovery and the addresses shown for the phone', () => {
  it('announces an IPv4-only service under its own .local host name', () => {
    const s = mdnsService('DESKTOP-AB12CD', 8080, '1.3.0');
    expect(s).toMatchObject({ name: 'PeekAround DESKTOP-AB12CD', type: 'peekaround', port: 8080, disableIPv6: true, txt: { v: '1.3.0' } });
    expect(s.host).toBe('peekaround-desktop-ab12cd.local');
    expect(mdnsService('Melikşah’s PC', 1, 'x').host).toBe('peekaround-melik-ah-s-pc.local');
    expect(mdnsService('---', 1, 'x').host).toBe('peekaround-pc.local');
  });

  it('lists the Wi-Fi / Ethernet address first, virtual adapters last, never link-local', () => {
    const v4 = (address: string) => ({ address, family: 'IPv4', internal: false, netmask: '255.255.255.0', mac: '00:15:5d:00:00:01', cidr: null }) as os.NetworkInterfaceInfo;
    const v6 = (address: string) => ({ address, family: 'IPv6', internal: false, netmask: 'ffff:ffff:ffff:ffff::', mac: '00:15:5d:00:00:01', cidr: null, scopeid: 0 }) as os.NetworkInterfaceInfo;
    // the order Windows typically reports with WSL / VirtualBox installed (IPv6 before IPv4 per adapter)
    const spy = vi.spyOn(os, 'networkInterfaces').mockReturnValue({
      'vEthernet (WSL (Hyper-V firewall))': [v6('fe80::1'), v4('172.29.16.1')],
      'VirtualBox Host-Only Network': [v4('192.168.56.1')],
      'Ethernet 2': [v4('169.254.12.3')],
      'Wi-Fi': [v6('fe80::2'), v4('192.168.1.23')],
      'Loopback Pseudo-Interface 1': [{ ...v4('127.0.0.1'), internal: true }],
    });
    try {
      expect(lanUrls(8080)).toEqual(['http://192.168.1.23:8080', 'http://172.29.16.1:8080', 'http://192.168.56.1:8080']);
    } finally {
      spy.mockRestore();
    }
  });
});
