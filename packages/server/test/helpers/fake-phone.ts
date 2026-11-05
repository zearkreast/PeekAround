/**
 * A Node "phone" speaking docs/link-protocol.md: WebSocket control channel + streaming reply PUTs +
 * pull GETs. Used by the device tests (directly and through the desktop LAN server).
 */
import crypto from 'node:crypto';
import fs from 'node:fs';
import fsp from 'node:fs/promises';
import http from 'node:http';
import path from 'node:path';
import { Readable } from 'node:stream';
import sharp from 'sharp';
import WebSocket from 'ws';
import { kindOf } from '@peekaround/shared';

export interface PhoneFolder {
  key: string;
  name: string;
  root: string;
}

/** generated file content (deterministic per offset) for big transfers */
export interface VirtualFile {
  size: number;
  mtime: number;
  /** bytes per write when streaming (default 64 KiB) */
  chunk?: number;
}

export function virtualByte(i: number): number {
  return (i * 31 + (i >> 8)) & 0xff;
}

export function virtualBytes(start: number, end: number): Buffer {
  const b = Buffer.allocUnsafe(end - start);
  for (let i = start; i < end; i++) b[i - start] = virtualByte(i);
  return b;
}

export type ReqFrame = { t: 'req'; rid: string; op: string; args: Record<string, any> };

export interface PutResult {
  status: number;
  /** bytes the phone managed to hand to the socket */
  sent: number;
}

export class FakePhone {
  ws: any = null;
  token: string | null;
  frames: any[] = [];
  closeCode: number | null = null;
  /** when set, the first `list` reply uses this NDJSON body */
  listBody: string | null = null;
  /** virtual files by "<key>/<rel>" */
  virtual = new Map<string, VirtualFile>();
  /** return true to take over a request (test-specific behaviour) */
  intercept: ((f: ReqFrame) => boolean | void) | null = null;
  /** ops that are swallowed (never answered) */
  ignore = new Set<string>();
  /** results of reply PUTs by rid */
  puts = new Map<string, Promise<PutResult>>();
  /** headers of the last pull GET (receive) */
  lastPullHeaders: Record<string, string> = {};
  /** bytes written so far into the current virtual stream (backpressure test) */
  written = 0;
  private waiters: { pred: (f: any) => boolean; resolve: (f: any) => void }[] = [];

  constructor(
    public base: string,
    public id: string,
    public folders: PhoneFolder[],
    opts: { token?: string | null; name?: string; caps?: { read: boolean; receive: boolean } } = {},
  ) {
    this.token = opts.token ?? null;
    this.name = opts.name ?? 'Test Phone';
    this.caps = opts.caps ?? { read: true, receive: true };
  }
  name: string;
  caps: { read: boolean; receive: boolean };

  helloFrame() {
    return {
      t: 'hello',
      v: 1,
      id: this.id,
      name: this.name,
      model: 'Google Pixel 8',
      app: '1.1.0',
      folders: this.folders.map((f) => ({ key: f.key, name: f.name, path: `/storage/emulated/0/${f.name}` })),
      caps: this.caps,
    };
  }

  /** open the socket and send hello; resolves with the server's first answer (welcome / pending) */
  connect(hello: object = this.helloFrame()): Promise<any> {
    const url = `${this.base.replace(/^http/, 'ws')}/api/devices/connect?v=1`;
    const headers: Record<string, string> = { 'X-Peek-Device': this.id };
    if (this.token) headers.Authorization = `Bearer ${this.token}`;
    this.closeCode = null;
    const ws = new WebSocket(url, { headers });
    this.ws = ws;
    ws.on('message', (data: Buffer) => this.onFrame(JSON.parse(String(data))));
    ws.on('close', (code: number) => {
      if (this.ws === ws) this.closeCode = code;
    });
    ws.on('error', () => {});
    const first = this.waitFrame((f) => f.t === 'welcome' || f.t === 'pending' || f.t === 'rejected');
    ws.on('open', () => ws.send(JSON.stringify(hello)));
    return first;
  }

  send(frame: object) {
    this.ws?.send(JSON.stringify(frame));
  }

  close() {
    this.ws?.close();
  }

  closed(): Promise<number> {
    if (this.ws?.readyState === 3) return Promise.resolve(this.closeCode ?? 0);
    return new Promise((r) => this.ws.once('close', (code: number) => r(code)));
  }

  waitFrame(pred: (f: any) => boolean, timeout = 10_000): Promise<any> {
    const hit = this.frames.find(pred);
    if (hit) {
      this.frames.splice(this.frames.indexOf(hit), 1);
      return Promise.resolve(hit);
    }
    return new Promise((resolve, reject) => {
      const t = setTimeout(() => reject(new Error('timeout waiting for frame')), timeout);
      this.waiters.push({
        pred,
        resolve: (f) => {
          clearTimeout(t);
          resolve(f);
        },
      });
    });
  }

  private onFrame(f: any) {
    if (f.t === 'paired') this.token = f.token;
    const w = this.waiters.find((x) => x.pred(f));
    if (w) {
      this.waiters.splice(this.waiters.indexOf(w), 1);
      w.resolve(f);
    } else this.frames.push(f);
    if (f.t === 'req') void this.handle(f as ReqFrame);
  }

  private authHeaders(): Record<string, string> {
    return { Authorization: `Bearer ${this.token}`, 'X-Peek-Device': this.id };
  }

  private folder(key: string): PhoneFolder {
    const f = this.folders.find((x) => x.key === key);
    if (!f) throw new Error('unknown key');
    return f;
  }

  /** phone-side path guard (spec: relative, no .., stays inside the root) */
  private resolve(key: string, rel: string): string | null {
    const f = this.folder(key);
    if (!rel || rel.startsWith('/') || rel.split('/').includes('..')) return null;
    const abs = path.resolve(f.root, ...rel.split('/'));
    return abs.startsWith(path.resolve(f.root) + path.sep) ? abs : null;
  }

  private err(rid: string, status: number, message: string) {
    this.send({ t: 'err', rid, status, message });
  }

  private async handle(f: ReqFrame) {
    if (this.intercept?.(f)) return;
    if (this.ignore.has(f.op)) return;
    const a = f.args;
    try {
      switch (f.op) {
        case 'list': {
          const body = this.listBody ?? (await this.walk(a.key, a.dir ?? ''));
          this.listBody = null;
          this.reply(f.rid, { 'Content-Type': 'application/x-ndjson' }, Buffer.from(body));
          return;
        }
        case 'thumb':
        case 'display': {
          const abs = this.resolve(a.key, a.p);
          if (!abs) return this.err(f.rid, 403, 'outside shared folder');
          const s = f.op === 'thumb' ? a.s : a.max;
          const img = kindOf(abs) === 'image' && fs.existsSync(abs)
            ? await sharp(abs).rotate().resize(s, s, { fit: 'inside' }).jpeg().toBuffer()
            : await sharp({ create: { width: 64, height: 36, channels: 3, background: '#10b981' } }).jpeg().toBuffer();
          this.reply(f.rid, { 'Content-Type': 'image/jpeg', 'Content-Length': String(img.length) }, img);
          return;
        }
        case 'strip': {
          const img = await sharp({ create: { width: 160 * a.n, height: a.h, channels: 3, background: '#000' } }).jpeg().toBuffer();
          this.reply(f.rid, { 'Content-Type': 'image/jpeg', 'Content-Length': String(img.length) }, img);
          return;
        }
        case 'details': {
          const body = Buffer.from(JSON.stringify({ exif: { Make: 'Google', Model: 'Pixel 8', FNumber: 1.7 }, gps: { lat: 40.99, lon: 29.03 }, make: 'Google', model: 'Pixel 8', lens: null }));
          this.reply(f.rid, { 'Content-Type': 'application/json', 'Content-Length': String(body.length) }, body);
          return;
        }
        case 'file':
          return this.file(f);
        case 'receive':
          return this.receive(f);
        default:
          this.err(f.rid, 400, 'unknown op');
      }
    } catch (e) {
      this.err(f.rid, 500, String((e as Error).message));
    }
  }

  private async walk(key: string, dir: string): Promise<string> {
    const f = this.folder(key);
    const lines: string[] = [];
    const go = async (abs: string, rel: string) => {
      const entries = await fsp.readdir(abs, { withFileTypes: true }).catch(() => []);
      if (!entries.length && rel) lines.push(JSON.stringify({ dir: rel }));
      for (const e of entries) {
        const p = rel ? `${rel}/${e.name}` : e.name;
        if (e.isDirectory()) await go(path.join(abs, e.name), p);
        else if (!e.name.endsWith('.part')) {
          const st = await fsp.stat(path.join(abs, e.name));
          lines.push(JSON.stringify({ p, k: kindOf(e.name), s: st.size, m: Math.round(st.mtimeMs), w: null, h: null, d: null, t: null, id: null }));
        }
      }
    };
    await go(dir ? path.join(f.root, ...dir.split('/')) : f.root, dir);
    for (const [k, v] of this.virtual) {
      const [vk, ...rest] = k.split('/');
      const p = rest.join('/');
      if (vk === key && (!dir || p.startsWith(`${dir}/`))) lines.push(JSON.stringify({ p, k: kindOf(p), s: v.size, m: v.mtime, w: 1920, h: 1080, d: 60, t: v.mtime, id: null }));
    }
    return lines.join('\n') + '\n';
  }

  private async file(f: ReqFrame) {
    const a = f.args;
    const v = this.virtual.get(`${a.key}/${a.p}`);
    let size: number;
    let mtime: number;
    let abs: string | null = null;
    if (v) {
      size = v.size;
      mtime = v.mtime;
    } else {
      abs = this.resolve(a.key, a.p);
      if (!abs) return this.err(f.rid, 403, 'outside shared folder');
      const st = await fsp.stat(abs).catch(() => null);
      if (!st) return this.err(f.rid, 404, 'not found');
      size = st.size;
      mtime = Math.round(st.mtimeMs);
    }
    let start = 0;
    let end = size - 1;
    let status = 200;
    if (a.range) {
      const m = /^bytes=(\d*)-(\d*)$/.exec(a.range)!;
      if (m[1] === '') {
        start = Math.max(0, size - Number(m[2]));
      } else {
        start = Number(m[1]);
        if (m[2] !== '') end = Math.min(size - 1, Number(m[2]));
      }
      if (start >= size || start > end) {
        this.reply(f.rid, { 'X-Peek-Status': '416', 'X-Peek-Content-Range': `bytes */${size}`, 'X-Peek-Total': String(size), 'Content-Length': '0' }, Buffer.alloc(0));
        return;
      }
      status = 206;
    }
    const len = end - start + 1;
    const headers: Record<string, string> = {
      'X-Peek-Status': String(status),
      'Content-Type': kindOf(a.p) === 'video' ? 'video/mp4' : 'application/octet-stream',
      'Content-Length': String(len),
      'X-Peek-Total': String(size),
      'X-Peek-Mtime': String(mtime),
    };
    if (status === 206) headers['X-Peek-Content-Range'] = `bytes ${start}-${end}/${size}`;
    const body = v ? virtualStream(start, end + 1, v.chunk ?? 64 * 1024, (n) => (this.written += n)) : fs.createReadStream(abs!, { start, end });
    this.reply(f.rid, headers, body);
  }

  private async receive(f: ReqFrame) {
    const a = f.args;
    const dirAbs = a.dir ? this.resolve(a.key, a.dir) : path.resolve(this.folder(a.key).root);
    if (!dirAbs) return this.err(f.rid, 403, 'outside shared folder');
    await fsp.mkdir(dirAbs, { recursive: true });
    const res = await fetch(new URL(a.url, this.base), { headers: this.authHeaders() });
    if (res.status !== 200) return this.err(f.rid, 502, `pull failed ${res.status}`);
    this.lastPullHeaders = Object.fromEntries(res.headers.entries());
    const part = path.join(dirAbs, `${a.name}.part`);
    await fsp.writeFile(part, Buffer.from(await res.arrayBuffer()));
    let name = a.name;
    const ext = path.extname(name);
    for (let i = 1; fs.existsSync(path.join(dirAbs, name)); i++) name = `${a.name.slice(0, a.name.length - ext.length)} (${i})${ext}`;
    await fsp.rename(part, path.join(dirAbs, name));
    const mt = Number(res.headers.get('x-peek-mtime')) || a.mtime;
    await fsp.utimes(path.join(dirAbs, name), new Date(), new Date(mt));
    const p = a.dir ? `${a.dir}/${name}` : name;
    this.send({ t: 'res', rid: f.rid, ok: true, data: { p } });
    this.send({ t: 'changed', key: a.key, dirs: [a.dir ?? ''] });
  }

  /** stream a reply PUT; resolves with the server's status for it */
  reply(rid: string, headers: Record<string, string>, body: Buffer | Readable): Promise<PutResult> {
    const p = this.put(rid, headers, body);
    this.puts.set(rid, p);
    return p;
  }

  put(rid: string, headers: Record<string, string>, body: Buffer | Readable, auth = this.authHeaders()): Promise<PutResult> {
    return new Promise((resolve, reject) => {
      const u = new URL(`/api/devices/reply/${rid}`, this.base);
      const len = Buffer.isBuffer(body) ? String(body.length) : headers['Content-Length'];
      const req = http.request(u, { method: 'PUT', headers: { ...auth, ...headers, ...(len !== undefined ? { 'Content-Length': len } : {}) } });
      let sent = 0;
      let done = false;
      const finish = (status: number) => {
        if (done) return;
        done = true;
        if (!Buffer.isBuffer(body)) body.destroy();
        resolve({ status, sent });
      };
      req.on('response', (res) => {
        res.resume();
        finish(res.statusCode ?? 0);
      });
      req.on('error', (e) => (done ? undefined : sent ? finish(-1) : reject(e)));
      if (Buffer.isBuffer(body)) {
        sent = body.length;
        req.end(body);
      } else {
        body.on('data', (c: Buffer) => {
          sent += c.length;
          if (!req.write(c)) body.pause();
        });
        req.on('drain', () => body.resume());
        body.on('end', () => req.end());
        body.on('error', () => req.destroy());
      }
    });
  }
}

/** a lazily generated byte range [start, end) */
export function virtualStream(start: number, end: number, chunk: number, onWrite?: (n: number) => void): Readable {
  let pos = start;
  return new Readable({
    read() {
      if (pos >= end) return void this.push(null);
      const n = Math.min(chunk, end - pos);
      const b = virtualBytes(pos, pos + n);
      pos += n;
      onWrite?.(n);
      this.push(b);
    },
  });
}

export const sha = (b: Buffer) => crypto.createHash('sha256').update(b).digest('hex');
