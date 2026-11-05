import crypto from 'node:crypto';
import os from 'node:os';
import { PassThrough, Readable } from 'node:stream';
import { StringDecoder } from 'node:string_decoder';
import type { FastifyRequest } from 'fastify';
import { BROWSER_IMAGE_EXT, extOf, kindOf, type DeviceInfo, type FolderRef } from '@peekaround/shared';
import type { Ctx } from '../context.js';
import { isBrowserPlayable } from '../ffmpeg.js';
import { DEVICE_SCHEME, type FolderRow } from '../library.js';
import type { MediaRow } from '../media/mapper.js';
import { GROUP_SET_SQL, groupColumns, groupParams, META_V, type ExtractedMeta } from '../media/metadata.js';
import { pool } from '../media/scanner.js';
import type { Priority, RemoteImageSource } from '../media/thumbs.js';
import { HttpError, normalizeRel } from '../paths.js';

/**
 * DeviceHub — server side of the phone ↔ PC link (docs/link-protocol.md).
 *
 * One WebSocket per phone carries small JSON control frames; every data reply flows over a
 * phone-initiated `PUT /api/devices/reply/:rid` that is piped (with backpressure, never buffered)
 * into the waiting consumer. PC → phone pushes use one-time pull URLs.
 */

/** the bits of a `ws` WebSocket the hub uses */
export interface WebSocket {
  readyState: number;
  send(data: string): void;
  close(code?: number, reason?: string): void;
  terminate(): void;
  ping(): void;
  on(event: string, listener: (...args: any[]) => void): unknown;
}

/** 0 = on-demand (visible) thumbnail, 1 = file / display / details / strip / receive, 2 = list / background thumbs */
export type Prio = 0 | 1 | 2;

export const MAX_INFLIGHT = 8;
/** background work never takes more than this many of the in-flight slots */
export const MAX_INFLIGHT_LOW = 4;
export const FIRST_BYTE_MS = { short: 20_000, file: 30_000 };
const HELLO_TIMEOUT_MS = 10_000;
const PING_MS = 25_000;
const IDLE_MS = 75_000;
const PULL_TTL_MS = 10 * 60_000;
const CHANGED_DEBOUNCE_MS = 2_000;
const MAX_PENDING = 16;

export interface HelloFolder {
  key: string;
  name: string;
  path?: string;
}

export interface TunnelReply {
  /** X-Peek-Status (200 / 206 / 416 …) */
  status: number;
  /** reply PUT headers, lower-cased */
  headers: Record<string, string>;
  /** the PUT body; destroy it to abort (→ cancel frame + 499 to the phone) */
  body: Readable;
}

interface Req {
  rid: string;
  conn: Conn;
  op: string;
  args: Record<string, unknown>;
  prio: Prio;
  state: 'queued' | 'sent' | 'streaming' | 'done';
  timer: NodeJS.Timeout | null;
  firstByteMs: number;
  resolve: (r: any) => void;
  reject: (e: Error) => void;
  signal?: AbortSignal;
  offAbort?: () => void;
  /** streaming: settle the reply PUT with this HTTP status */
  finish?: (code: number) => void;
  body?: PassThrough;
}

interface Conn {
  id: string;
  socket: WebSocket;
  name: string;
  model: string;
  app: string;
  caps: { read: boolean; receive: boolean };
  folders: HelloFolder[];
  paired: boolean;
  code: string | null;
  lan: boolean;
  queue: Req[];
  inflight: Map<string, Req>;
  lastSeen: number;
  closed: boolean;
  thumbRunning: boolean;
  changed: Map<string, Set<string>>;
  changedTimer: NodeJS.Timeout | null;
}

interface Pull {
  deviceId: string;
  abs: string;
  size: number;
  mtime: number;
  expires: number;
  onStart?: (stream: Readable) => void;
  onBytes?: (n: number) => void;
  onEnd?: () => void;
}

interface DeviceRow {
  id: string;
  name: string;
  model: string;
  token_hash: string | null;
  paired: number;
  caps: string | null;
  created_at: number;
  last_seen: number;
}

const ID_RE = /^[A-Za-z0-9._-]{8,64}$/;
const KEY_RE = /^[A-Za-z0-9_-]{1,64}$/;

const sha256 = (s: string) => crypto.createHash('sha256').update(s).digest('hex');
const clean = (v: unknown, max: number, fallback = '') =>
  typeof v === 'string' ? v.replace(/[\u0000-\u001f]/g, '').trim().slice(0, max) || fallback : fallback;

export function bearerOf(req: { headers: Record<string, unknown> }): string | null {
  const h = req.headers.authorization;
  if (typeof h !== 'string') return null;
  const m = /^Bearer\s+([A-Za-z0-9_-]{16,128})$/.exec(h.trim());
  return m ? m[1] : null;
}

export function deviceHeader(req: { headers: Record<string, unknown> }): string | null {
  const h = req.headers['x-peek-device'];
  return typeof h === 'string' && ID_RE.test(h) ? h : null;
}

/** Map a phone error status to the HTTP status a consumer gets. */
function consumerStatus(s: number): number {
  return [400, 403, 404, 409, 413, 416, 503].includes(s) ? s : 502;
}

export class DeviceHub implements RemoteImageSource {
  private conns = new Map<string, Conn>();
  private pending = new Map<string, Conn>();
  private reqs = new Map<string, Req>();
  private pulls = new Map<string, Pull>();
  /** folder id → device id for every device folder row (visibility checks without a query) */
  private folderOwners = new Map<number, string>();
  private heartbeat: NodeJS.Timeout;
  private closed = false;
  /** lets the server feed ffmpeg from its own /file endpoint (requests carrying it bypass visibility) */
  readonly internalToken = crypto.randomBytes(24).toString('base64url');
  /** loopback base URL of the main server (set by app.ts once listening) */
  selfUrl = '';

  constructor(private ctx: Ctx) {
    ctx.thumbs.remote = this;
    ctx.scanner.deviceScan = (folderId, subDir) => this.relist(folderId, subDir);
    ctx.lib.deviceLookup = (id) => {
      const c = this.conns.get(id);
      const row = c ? null : this.ctx.db.get<{ name: string }>('SELECT name FROM devices WHERE id = ?', id);
      return { id, name: c?.name ?? row?.name ?? id, online: !!c };
    };
    this.refreshOwners();
    this.heartbeat = setInterval(() => this.beat(), PING_MS);
    this.heartbeat.unref();
  }

  // ───────────────────────── state / listing ─────────────────────────

  isOnline(deviceId: string): boolean {
    return this.conns.has(deviceId);
  }

  hasFolders(): boolean {
    return this.folderOwners.size > 0;
  }

  folderOwner(folderId: number): string | null {
    return this.folderOwners.get(folderId) ?? null;
  }

  private refreshOwners() {
    this.folderOwners.clear();
    for (const r of this.ctx.db.all<{ id: number; device_id: string }>('SELECT id, device_id FROM folders WHERE device_id IS NOT NULL')) {
      this.folderOwners.set(r.id, r.device_id);
    }
  }

  /** Paired (remembered) + pending devices, except `hide` (the asking phone itself). */
  list(hide: string | null = null): DeviceInfo[] {
    const counts = this.ctx.lib.counts();
    const folders = this.ctx.db.all<FolderRow>('SELECT * FROM folders WHERE device_id IS NOT NULL ORDER BY sort, id');
    const out: DeviceInfo[] = [];
    for (const d of this.ctx.db.all<DeviceRow>('SELECT * FROM devices WHERE paired = 1 ORDER BY last_seen DESC')) {
      if (d.id === hide) continue;
      const c = this.conns.get(d.id);
      out.push({
        id: d.id,
        name: c?.name ?? d.name,
        model: c?.model ?? d.model,
        online: !!c,
        paired: true,
        lastSeen: c ? Date.now() : d.last_seen,
        folders: folders.filter((f) => f.device_id === d.id).map((f) => this.ctx.lib.folderRef(f, counts)),
        caps: c?.caps ?? parseCaps(d.caps),
      });
    }
    for (const p of this.pending.values()) {
      if (p.id === hide) continue;
      out.push(this.pendingInfo(p));
    }
    return out;
  }

  private pendingInfo(p: Conn): DeviceInfo {
    return { id: p.id, name: p.name, model: p.model, online: true, paired: false, lastSeen: p.lastSeen, folders: [], code: p.code ?? undefined, caps: p.caps };
  }

  private emitDevices() {
    this.ctx.events.emit({ type: 'devices' });
  }

  // ───────────────────────── auth ─────────────────────────

  /**
   * Phone → server HTTP auth: `Authorization: Bearer <token>` + `X-Peek-Device`.
   * 401 unknown device / bad token, 403 not paired.
   */
  authenticate(req: { headers: Record<string, unknown> }): string {
    const id = deviceHeader(req);
    const token = bearerOf(req);
    if (!id || !token) throw new HttpError(401, 'Device token required');
    const row = this.ctx.db.get<DeviceRow>('SELECT * FROM devices WHERE id = ?', id);
    if (!row || !row.token_hash || !safeEq(row.token_hash, sha256(token))) throw new HttpError(401, 'Unknown device or invalid token');
    if (!row.paired) throw new HttpError(403, 'Device is not paired');
    return id;
  }

  /** non-throwing variant */
  pairedDevice(req: { headers: Record<string, unknown> }): string | null {
    try {
      return this.authenticate(req);
    } catch {
      return null;
    }
  }

  // ───────────────────────── control socket ─────────────────────────

  /** WS /api/devices/connect */
  attach(socket: WebSocket, req: FastifyRequest) {
    if (this.closed) {
      socket.close(1001, 'shutting down');
      return;
    }
    const lan = !!(req.raw as { peekLan?: boolean }).peekLan;
    const headerId = req.headers['x-peek-device'];
    const token = bearerOf(req);
    let conn: Conn | null = null;
    const helloTimer = setTimeout(() => socket.close(4408, 'hello timeout'), HELLO_TIMEOUT_MS);
    socket.on('message', (data: Buffer, isBinary: boolean) => {
      if (this.closed) return; // shutting down: the database may already be gone
      if (isBinary) return socket.close(4400, 'text frames only');
      let msg: any;
      try {
        msg = JSON.parse(String(data));
      } catch {
        return;
      }
      if (!msg || typeof msg !== 'object') return;
      if (!conn) {
        clearTimeout(helloTimer);
        conn = this.onHello(socket, msg, { lan, headerId: typeof headerId === 'string' ? headerId : null, token });
        return;
      }
      conn.lastSeen = Date.now();
      this.onFrame(conn, msg);
    });
    socket.on('pong', () => {
      if (conn) conn.lastSeen = Date.now();
    });
    socket.on('close', () => {
      clearTimeout(helloTimer);
      if (conn && !this.closed) this.detach(conn);
    });
    socket.on('error', () => {});
  }

  private send(conn: Conn, frame: object) {
    if (conn.socket.readyState === 1) conn.socket.send(JSON.stringify(frame));
  }

  private onHello(socket: WebSocket, msg: any, o: { lan: boolean; headerId: string | null; token: string | null }): Conn | null {
    if (msg.t !== 'hello') {
      socket.close(4400, 'expected hello');
      return null;
    }
    if (msg.v !== 1) {
      socket.close(4426, 'unsupported protocol version');
      return null;
    }
    const id = typeof msg.id === 'string' ? msg.id : '';
    if (!ID_RE.test(id) || (o.headerId && o.headerId !== id)) {
      socket.close(4400, 'bad device id');
      return null;
    }
    const conn: Conn = {
      id,
      socket,
      name: clean(msg.name, 80, 'Phone'),
      model: clean(msg.model, 80),
      app: clean(msg.app, 20),
      caps: { read: msg.caps?.read !== false, receive: msg.caps?.receive === true },
      folders: parseFolders(msg.folders),
      paired: false,
      code: null,
      lan: o.lan,
      queue: [],
      inflight: new Map(),
      lastSeen: Date.now(),
      closed: false,
      thumbRunning: false,
      changed: new Map(),
      changedTimer: null,
    };
    const row = this.ctx.db.get<DeviceRow>('SELECT * FROM devices WHERE id = ?', id);
    if (row?.paired && row.token_hash && o.token && safeEq(row.token_hash, sha256(o.token))) {
      this.send(conn, { t: 'welcome', server: os.hostname(), paired: true });
      this.goOnline(conn);
      return conn;
    }
    // unknown device, no token, or a token we no longer accept (forgotten) → ask the PC user
    const old = this.pending.get(id);
    if (old) {
      old.closed = true;
      this.pending.delete(id);
      old.socket.close(4409, 'replaced');
    }
    if (this.pending.size >= MAX_PENDING) {
      socket.close(4429, 'too many pending pairings');
      return null;
    }
    conn.code = String(crypto.randomInt(0, 10_000)).padStart(4, '0');
    this.pending.set(id, conn);
    this.send(conn, { t: 'pending', code: conn.code });
    this.ctx.events.emit({ type: 'pair', device: this.pendingInfo(conn) });
    this.emitDevices();
    return conn;
  }

  approve(id: string) {
    const p = this.pending.get(id);
    if (!p) throw new HttpError(404, 'No pending pairing for this device');
    this.pending.delete(id);
    const token = crypto.randomBytes(32).toString('base64url');
    const now = Date.now();
    this.ctx.db.run(
      `INSERT INTO devices (id, name, model, token_hash, paired, caps, created_at, last_seen) VALUES (?, ?, ?, ?, 1, ?, ?, ?)
       ON CONFLICT(id) DO UPDATE SET name = excluded.name, model = excluded.model, token_hash = excluded.token_hash, paired = 1, caps = excluded.caps, last_seen = excluded.last_seen`,
      id,
      p.name,
      p.model,
      sha256(token),
      JSON.stringify(p.caps),
      now,
      now,
    );
    p.code = null;
    this.send(p, { t: 'paired', token });
    this.goOnline(p);
  }

  reject(id: string) {
    const p = this.pending.get(id);
    if (!p) throw new HttpError(404, 'No pending pairing for this device');
    this.pending.delete(id);
    p.closed = true;
    this.send(p, { t: 'rejected' });
    p.socket.close(4403, 'rejected');
    this.emitDevices();
  }

  /** Unpair: drop the token, its folders and their media rows (cached thumbnails age out with the cache). */
  forget(id: string) {
    const known = this.ctx.db.get('SELECT id FROM devices WHERE id = ?', id);
    const p = this.pending.get(id);
    if (!known && !p) throw new HttpError(404, 'Unknown device');
    if (p) this.reject(id);
    const c = this.conns.get(id);
    if (c) {
      this.detach(c);
      c.socket.close(4401, 'forgotten');
    }
    this.ctx.db.run('DELETE FROM devices WHERE id = ?', id);
    this.refreshOwners();
    this.ctx.events.bump();
    this.ctx.events.emit({ type: 'folders' });
    this.emitDevices();
  }

  private goOnline(conn: Conn) {
    conn.paired = true;
    const prev = this.conns.get(conn.id);
    if (prev && prev !== conn) {
      this.detach(prev);
      prev.socket.close(4409, 'replaced');
    }
    this.conns.set(conn.id, conn);
    this.ctx.db.run(
      'UPDATE devices SET name = ?, model = ?, caps = ?, last_seen = ? WHERE id = ?',
      conn.name,
      conn.model,
      JSON.stringify(conn.caps),
      Date.now(),
      conn.id,
    );
    const rows = this.syncFolders(conn);
    this.emitDevices();
    this.ctx.events.emit({ type: 'folders' });
    if (conn.caps.read) for (const r of rows) void this.relist(r.id, '').catch(() => {});
  }

  private detach(conn: Conn) {
    if (conn.closed && !this.conns.has(conn.id) && !this.pending.has(conn.id)) return;
    conn.closed = true;
    if (conn.changedTimer) clearTimeout(conn.changedTimer);
    if (this.pending.get(conn.id) === conn) {
      this.pending.delete(conn.id);
      this.emitDevices();
      return;
    }
    if (this.conns.get(conn.id) !== conn) return;
    this.conns.delete(conn.id);
    this.ctx.db.run('UPDATE devices SET last_seen = ? WHERE id = ?', Date.now(), conn.id);
    const err = new HttpError(503, 'device offline');
    for (const r of [...conn.queue, ...conn.inflight.values()]) this.fail(r, err, false);
    conn.queue = [];
    for (const [tok, p] of this.pulls) if (p.deviceId === conn.id) this.pulls.delete(tok);
    this.emitDevices();
    this.ctx.events.emit({ type: 'folders' });
  }

  private beat() {
    const now = Date.now();
    for (const c of [...this.conns.values(), ...this.pending.values()]) {
      if (now - c.lastSeen > IDLE_MS) {
        c.socket.terminate();
        this.detach(c);
      } else if (c.socket.readyState === 1) c.socket.ping();
    }
    for (const [tok, p] of this.pulls) if (p.expires < now) this.pulls.delete(tok);
  }

  private onFrame(conn: Conn, msg: any) {
    switch (msg.t) {
      case 'ping':
        this.send(conn, { t: 'pong' });
        return;
      case 'pong':
        return;
    }
    if (!conn.paired) return;
    switch (msg.t) {
      case 'folders': {
        conn.folders = parseFolders(msg.folders);
        const before = new Set(this.folderRowsOf(conn.id).map((r) => r.id));
        const rows = this.syncFolders(conn);
        this.ctx.events.emit({ type: 'folders' });
        this.emitDevices();
        if (conn.caps.read) for (const r of rows) if (!before.has(r.id)) void this.relist(r.id, '').catch(() => {});
        return;
      }
      case 'changed': {
        const key = typeof msg.key === 'string' ? msg.key : '';
        if (!conn.folders.some((f) => f.key === key)) return;
        let set = conn.changed.get(key);
        if (!set) conn.changed.set(key, (set = new Set()));
        for (const d of Array.isArray(msg.dirs) ? msg.dirs.slice(0, 256) : ['']) {
          try {
            set.add(normalizeRel(String(d)));
          } catch {
            /* ignore bad dir */
          }
        }
        if (!conn.changedTimer) {
          conn.changedTimer = setTimeout(() => {
            conn.changedTimer = null;
            if (this.closed) return;
            const work = conn.changed;
            conn.changed = new Map();
            for (const [k, dirs] of work) {
              const row = this.folderRowsOf(conn.id).find((r) => r.device_key === k);
              if (row) for (const d of collapseDirs([...dirs])) void this.relist(row.id, d).catch(() => {});
            }
          }, CHANGED_DEBOUNCE_MS);
          conn.changedTimer.unref?.();
        }
        return;
      }
      case 'err': {
        const r = this.reqs.get(String(msg.rid));
        if (!r || r.conn !== conn || r.state === 'streaming' || r.state === 'done') return;
        const status = Number(msg.status) || 500;
        this.fail(r, new HttpError(consumerStatus(status), clean(msg.message, 200, `device error ${status}`)), false);
        return;
      }
      case 'res': {
        const r = this.reqs.get(String(msg.rid));
        if (!r || r.conn !== conn || r.op !== 'receive' || r.state === 'done') return;
        const resolve = r.resolve;
        this.finishReq(r);
        if (msg.ok === false) r.reject(new HttpError(consumerStatus(Number(msg.status) || 500), clean(msg.message, 200, 'device error')));
        else resolve(msg.data ?? {});
        return;
      }
    }
  }

  // ───────────────────────── folders ─────────────────────────

  private folderRowsOf(deviceId: string): FolderRow[] {
    return this.ctx.db.all<FolderRow>('SELECT * FROM folders WHERE device_id = ? ORDER BY sort, id', deviceId);
  }

  /** hello / folders → one folders row per shared folder; vanished ones are dropped (cascade). */
  private syncFolders(conn: Conn): FolderRow[] {
    const existing = this.folderRowsOf(conn.id);
    const keep = new Set<number>();
    this.ctx.db.tx(() => {
      conn.folders.forEach((f, i) => {
        const cur = existing.find((r) => r.device_key === f.key);
        if (cur) {
          keep.add(cur.id);
          if (cur.name !== f.name || cur.sort !== i) this.ctx.db.run('UPDATE folders SET name = ?, sort = ? WHERE id = ?', f.name, i, cur.id);
        } else {
          const res = this.ctx.db.run(
            'INSERT INTO folders (library_id, path, name, temp, sort, device_id, device_key) VALUES (NULL, ?, ?, 0, ?, ?, ?)',
            `${DEVICE_SCHEME}${conn.id}/${f.key}`,
            f.name,
            i,
            conn.id,
            f.key,
          );
          keep.add(Number(res.lastInsertRowid));
        }
      });
      for (const r of existing) if (!keep.has(r.id)) this.ctx.db.run('DELETE FROM folders WHERE id = ?', r.id);
    });
    if (existing.some((r) => !keep.has(r.id))) this.ctx.events.bump();
    this.refreshOwners();
    return this.folderRowsOf(conn.id);
  }

  // ───────────────────────── requests ─────────────────────────

  /**
   * Send a request to a phone and resolve with its reply PUT (or the `res` frame for `receive`).
   * Rejects with 503 (offline), 504 (no first byte in time), the phone's err status, or AbortError.
   */
  request(deviceId: string, op: string, args: Record<string, unknown>, opts: { prio: Prio; signal?: AbortSignal; firstByteMs?: number }): Promise<any> {
    const conn = this.conns.get(deviceId);
    if (!conn) return Promise.reject(new HttpError(503, 'device offline'));
    if (opts.signal?.aborted) return Promise.reject(abortError());
    return new Promise((resolve, reject) => {
      const r: Req = {
        rid: crypto.randomBytes(16).toString('base64url'),
        conn,
        op,
        args,
        prio: opts.prio,
        state: 'queued',
        timer: null,
        firstByteMs: opts.firstByteMs ?? (op === 'file' || op === 'receive' ? FIRST_BYTE_MS.file : FIRST_BYTE_MS.short),
        resolve,
        reject,
        signal: opts.signal,
      };
      if (opts.signal) {
        const onAbort = () => this.abort(r);
        opts.signal.addEventListener('abort', onAbort, { once: true });
        r.offAbort = () => opts.signal!.removeEventListener('abort', onAbort);
      }
      this.reqs.set(r.rid, r);
      // stable insert by priority
      let i = conn.queue.length;
      while (i > 0 && conn.queue[i - 1].prio > r.prio) i--;
      conn.queue.splice(i, 0, r);
      this.pump(conn);
    });
  }

  /** in-flight / queued counts (tests, diagnostics) */
  stats(deviceId: string) {
    const c = this.conns.get(deviceId);
    return { inflight: c?.inflight.size ?? 0, queued: c?.queue.length ?? 0 };
  }

  private pump(conn: Conn) {
    while (conn.inflight.size < MAX_INFLIGHT && conn.queue.length && !conn.closed) {
      const low = [...conn.inflight.values()].filter((x) => x.prio === 2).length;
      const idx = conn.queue.findIndex((x) => x.prio < 2 || low < MAX_INFLIGHT_LOW);
      if (idx < 0) break;
      const r = conn.queue.splice(idx, 1)[0];
      r.state = 'sent';
      conn.inflight.set(r.rid, r);
      r.timer = setTimeout(() => this.fail(r, new HttpError(504, 'The device did not answer in time'), true), r.firstByteMs);
      r.timer.unref?.();
      this.send(conn, { t: 'req', rid: r.rid, op: r.op, args: r.args });
    }
  }

  private finishReq(r: Req) {
    if (r.state === 'done') return;
    r.state = 'done';
    if (r.timer) clearTimeout(r.timer);
    r.timer = null;
    r.offAbort?.();
    this.reqs.delete(r.rid);
    const q = r.conn.queue.indexOf(r);
    if (q >= 0) r.conn.queue.splice(q, 1);
    if (r.conn.inflight.delete(r.rid)) this.pump(r.conn);
  }

  private fail(r: Req, err: Error, cancel: boolean) {
    if (r.state === 'done') return;
    const wasSent = r.state === 'sent' || r.state === 'streaming';
    if (cancel && wasSent) this.send(r.conn, { t: 'cancel', rid: r.rid });
    if (r.state === 'streaming') {
      r.body?.destroy(err);
      r.finish?.(499);
    }
    this.finishReq(r);
    r.reject(err);
  }

  /** consumer went away (AbortSignal) */
  private abort(r: Req) {
    this.fail(r, abortError(), true);
  }

  /**
   * PUT /api/devices/reply/:rid — pipe the body into the consumer. Resolves with the HTTP status
   * for the PUT: 204 consumer finished, 499 consumer went away, 410 unknown/expired rid.
   */
  acceptReply(rid: string, deviceId: string, raw: import('node:http').IncomingMessage): Promise<number> | number {
    const r = this.reqs.get(rid);
    if (!r || r.conn.id !== deviceId || r.state !== 'sent' || r.op === 'receive') return 410;
    if (r.timer) clearTimeout(r.timer);
    r.timer = null;
    r.state = 'streaming';
    const headers: Record<string, string> = {};
    for (const [k, v] of Object.entries(raw.headers)) if (typeof v === 'string') headers[k.toLowerCase()] = v;
    const status = Number(headers['x-peek-status'] ?? 200) || 200;
    const body = new PassThrough();
    r.body = body;
    return new Promise<number>((resolve) => {
      let settled = false;
      const settle = (code: number) => {
        if (settled) return;
        settled = true;
        r.finish = undefined;
        this.finishReq(r);
        resolve(code);
      };
      r.finish = settle;
      raw.on('end', () => settle(204));
      raw.on('error', (e) => body.destroy(e));
      raw.on('close', () => {
        if (!raw.complete) {
          body.destroy(new Error('The device closed the connection'));
          settle(499);
        }
      });
      body.on('close', () => {
        if (raw.readableEnded || settled) return;
        // consumer went away before the phone finished sending
        raw.unpipe(body);
        raw.pause();
        this.send(r.conn, { t: 'cancel', rid: r.rid });
        settle(499);
      });
      raw.pipe(body);
      r.offAbort?.();
      if (r.signal) {
        const onAbort = () => body.destroy(abortError());
        if (r.signal.aborted) onAbort();
        else r.signal.addEventListener('abort', onAbort, { once: true });
      }
      r.resolve({ status, headers, body } satisfies TunnelReply);
    });
  }

  /** request + read the whole (small) body */
  async fetchBuffer(deviceId: string, op: string, args: Record<string, unknown>, prio: Prio, max = 64 * 1024 * 1024, signal?: AbortSignal): Promise<{ status: number; headers: Record<string, string>; data: Buffer }> {
    const tr: TunnelReply = await this.request(deviceId, op, args, { prio, signal });
    const chunks: Buffer[] = [];
    let n = 0;
    for await (const c of tr.body) {
      n += (c as Buffer).length;
      if (n > max) {
        tr.body.destroy();
        throw new HttpError(413, 'Device reply too large');
      }
      chunks.push(c as Buffer);
    }
    return { status: tr.status, headers: tr.headers, data: Buffer.concat(chunks) };
  }

  // ───────────────────────── device paths ─────────────────────────

  /** device://<id>/<key>/<rel> → parts (throws 400 for anything else) */
  parseAbs(abs: string): { deviceId: string; key: string; rel: string } {
    const m = /^device:\/\/([A-Za-z0-9._-]+)\/([A-Za-z0-9_-]+)\/(.+)$/.exec(abs);
    if (!m) throw new HttpError(400, 'Not a device path');
    return { deviceId: m[1], key: m[2], rel: normalizeRel(m[3]) };
  }

  folderOf(folder: FolderRow): { deviceId: string; key: string } {
    if (!folder.device_id || !folder.device_key) throw new HttpError(400, 'Not a device folder');
    return { deviceId: folder.device_id, key: folder.device_key };
  }

  caps(deviceId: string): { read: boolean; receive: boolean } | null {
    return this.conns.get(deviceId)?.caps ?? null;
  }

  // RemoteImageSource (ThumbService)
  async thumb(abs: string, size: 320 | 640 | 1600, prio: Priority): Promise<Buffer> {
    const { deviceId, key, rel } = this.parseAbs(abs);
    const r = await this.fetchBuffer(deviceId, 'thumb', { key, p: rel, s: size }, prio === 'high' ? 0 : 2, 16 * 1024 * 1024);
    if (r.status !== 200 || !r.data.length) throw new HttpError(502, 'No thumbnail from the device');
    return r.data;
  }

  async display(abs: string, max: number): Promise<Buffer> {
    const { deviceId, key, rel } = this.parseAbs(abs);
    const r = await this.fetchBuffer(deviceId, 'display', { key, p: rel, max }, 1);
    if (r.status !== 200 || !r.data.length) throw new HttpError(502, 'No image from the device');
    return r.data;
  }

  async strip(abs: string, n: number, h: number): Promise<Buffer> {
    const { deviceId, key, rel } = this.parseAbs(abs);
    const r = await this.fetchBuffer(deviceId, 'strip', { key, p: rel, n, h }, 1);
    if (r.status !== 200 || !r.data.length) throw new HttpError(502, 'No filmstrip from the device');
    return r.data;
  }

  // ───────────────────────── listing → media rows ─────────────────────────

  private listing = new Map<string, Promise<void>>();

  /** (Re)list a device folder (or a sub tree of it) into media rows. No-op while offline. */
  relist(folderId: number, subDir = ''): Promise<void> {
    if (this.closed) return Promise.resolve();
    const k = `${folderId}\u0000${subDir}`;
    const cur = this.listing.get(k);
    if (cur) return cur;
    const p = this.doList(folderId, subDir).finally(() => this.listing.delete(k));
    this.listing.set(k, p);
    return p;
  }

  private async doList(folderId: number, subDir: string) {
    const folder = this.ctx.db.get<FolderRow>('SELECT * FROM folders WHERE id = ?', folderId);
    if (!folder?.device_id || !folder.device_key) return;
    const conn = this.conns.get(folder.device_id);
    if (!conn || !conn.caps.read) return;
    const dir = normalizeRel(subDir);
    let tr: TunnelReply;
    try {
      tr = await this.request(conn.id, 'list', { key: folder.device_key, dir, recursive: true }, { prio: 2 });
    } catch (e) {
      // a reported directory that no longer exists on the phone → forget it
      if (dir && e instanceof HttpError && e.statusCode === 404) tr = { status: 200, headers: {}, body: Readable.from([]) };
      else throw e;
    }
    if (tr.status !== 200 || this.closed) {
      tr.body.resume();
      if (this.closed) return;
      throw new HttpError(502, `list failed (${tr.status})`);
    }
    const db = this.ctx.db;
    const scope = dir ? ' AND (dir = ? OR substr(dir, 1, ?) = ?)' : '';
    const scopeParams = dir ? [dir, dir.length + 1, `${dir}/`] : [];
    const inScope = (p: string) => !dir || p === dir || p.startsWith(`${dir}/`);
    const maxBefore = db.get<{ m: number | null }>('SELECT max(id) m FROM media')?.m ?? 0;
    const dirs = new Set<string>();
    let changed = 0;
    let batch: ListEntry[] = [];
    const tz = this.ctx.scanner.homeTimeZone;
    const flush = () => {
      if (!batch.length) return;
      if (this.closed) throw new HttpError(503, 'shutting down');
      const rows = batch;
      batch = [];
      db.tx(() => {
        for (const e of rows) if (this.upsertListed(folder.id, e, tz)) changed++;
      });
    };
    db.run(`UPDATE media SET seen = 0 WHERE folder_id = ?${scope}`, folder.id, ...scopeParams);
    let ok = false;
    try {
      for await (const line of ndjsonLines(tr.body)) {
        let o: any;
        try {
          o = JSON.parse(line);
        } catch {
          continue;
        }
        if (!o || typeof o !== 'object') continue;
        if (typeof o.dir === 'string' && o.p === undefined) {
          const d = safeRel(o.dir);
          if (d && inScope(d)) addDirChain(dirs, d);
          continue;
        }
        const p = typeof o.p === 'string' ? safeRel(o.p) : null;
        if (!p || !inScope(p)) continue;
        const segs = p.split('/');
        if (segs.some((s) => this.ctx.scanner.skipName(s))) continue;
        const name = segs.pop()!;
        const d = segs.join('/');
        const kind = kindOf(name);
        if (!kind) continue;
        if (d) addDirChain(dirs, d);
        batch.push({
          dir: d,
          name,
          kind,
          size: num(o.s) ?? 0,
          mtime: num(o.m) ?? 0,
          width: posInt(o.w),
          height: posInt(o.h),
          duration: num(o.d),
          taken: num(o.t),
        });
        if (batch.length >= 500) flush();
      }
      flush();
      ok = true;
    } finally {
      if (!ok && !this.closed) db.run(`UPDATE media SET seen = 1 WHERE folder_id = ?${scope}`, folder.id, ...scopeParams);
    }
    if (this.closed) return;
    const removed = Number(db.run(`DELETE FROM media WHERE folder_id = ? AND seen = 0${scope}`, folder.id, ...scopeParams).changes);
    db.tx(() => {
      db.run(`DELETE FROM device_dirs WHERE folder_id = ?${scope}`, folder.id, ...scopeParams);
      for (const d of dirs) db.run('INSERT OR IGNORE INTO device_dirs (folder_id, dir) VALUES (?, ?)', folder.id, d);
    });
    const added = db.get<{ c: number }>('SELECT count(*) c FROM media WHERE folder_id = ? AND id > ?', folder.id, maxBefore)?.c ?? 0;
    const fid = String(folder.id);
    if (added) this.ctx.events.mediaChanged(fid, '*', 'add');
    else if (removed) this.ctx.events.mediaChanged(fid, '*', 'remove');
    else if (changed) this.ctx.events.mediaChanged(fid, '*', 'change');
    if (added || removed || changed) this.ctx.scanner.onGroupsChanged?.();
    this.ctx.events.emit({ type: 'folders' });
    this.startThumbs(conn);
  }

  /** returns true when the row was inserted or changed */
  private upsertListed(folderId: number, e: ListEntry, tz: string): boolean {
    const db = this.ctx.db;
    const same = db.run('UPDATE media SET seen = 1 WHERE folder_id = ? AND dir = ? AND name = ? AND size = ? AND mtime = ?', folderId, e.dir, e.name, e.size, e.mtime);
    if (Number(same.changes)) return false;
    const ext = extOf(e.name);
    const m: ExtractedMeta = {
      width: e.width,
      height: e.height,
      duration: e.duration,
      playback: e.kind === 'image' ? (BROWSER_IMAGE_EXT.includes(ext) ? 'direct' : 'convert') : e.kind === 'video' ? (isBrowserPlayable(ext, null) ? 'direct' : 'transcode') : 'direct',
      extra: null,
      local: null,
      offset: null,
      instant: e.taken && e.taken > 0 ? e.taken : null,
      lat: null,
      lon: null,
      make: null,
      model: null,
      lens: null,
      gpano: false,
      pages: null,
    };
    const g = groupColumns({ name: e.name, dir: e.dir, ext, kind: e.kind, mtime: e.mtime }, m, this.ctx.geo, tz);
    db.run(
      `INSERT INTO media (folder_id, dir, name, ext, kind, size, mtime, width, height, duration, playback, meta_state, meta_v, thumb_state, seen,
                          taken_at, taken_local, tz_off, time_src, lat, lon, geo_id, cc, admin1, city, make, model, lens, flags)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, 0, 1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
       ON CONFLICT (folder_id, dir, name) DO UPDATE SET
         seen = 1, ext = excluded.ext, kind = excluded.kind, size = excluded.size, mtime = excluded.mtime,
         width = excluded.width, height = excluded.height, duration = excluded.duration, playback = excluded.playback,
         meta_state = 1, meta_v = excluded.meta_v, thumb_state = 0, thumbhash = NULL,
         ${GROUP_SET_SQL.replace(/(\w+) = \?/g, '$1 = excluded.$1')}`,
      folderId,
      e.dir,
      e.name,
      ext,
      e.kind,
      e.size,
      e.mtime,
      e.width,
      e.height,
      e.duration,
      m.playback,
      META_V,
      ...groupParams(g),
    );
    return true;
  }

  /** background loops that touch the database (awaited by close()) */
  private loops = new Set<Promise<void>>();

  private startThumbs(conn: Conn) {
    if (this.closed) return;
    const p: Promise<void> = this.thumbLoop(conn)
      .catch(() => {})
      .finally(() => this.loops.delete(p));
    this.loops.add(p);
  }

  /** Background thumbnails (+ thumbhash) of a device's media while it is online. */
  private async thumbLoop(conn: Conn) {
    if (conn.thumbRunning || conn.closed) return;
    conn.thumbRunning = true;
    try {
      const failed = new Set<number>();
      for (;;) {
        if (conn.closed || this.conns.get(conn.id) !== conn || this.closed) break;
        const rows = this.ctx.db.all<MediaRow & { device_key: string; fpath: string }>(
          `SELECT m.*, f.path fpath FROM media m JOIN folders f ON f.id = m.folder_id
           WHERE f.device_id = ? AND m.thumb_state = 0 AND m.kind IN ('image','video') ORDER BY m.taken_local DESC LIMIT 16`,
          conn.id,
        );
        const todo = rows.filter((r) => !failed.has(r.id));
        if (!todo.length) break;
        let offline = false;
        await pool(todo, 2, async (r) => {
          if (offline) return;
          const abs = `${r.fpath}/${r.dir ? `${r.dir}/` : ''}${r.name}`;
          try {
            const res = await this.ctx.thumbs.generateBase(abs, r, 'low');
            if (this.closed) return;
            this.ctx.db.run(
              'UPDATE media SET thumbhash = ?, thumb_state = ?, width = COALESCE(width, ?), height = COALESCE(height, ?) WHERE id = ?',
              res?.thumbhash ?? null,
              res ? 1 : 2,
              res && r.kind === 'image' ? res.width : null,
              res && r.kind === 'image' ? res.height : null,
              r.id,
            );
            // dimensions filled in by the thumbnail change the layout (skeleton aspects): new generation
            const dims = !!res && r.kind === 'image' && (!r.width || !r.height);
            this.ctx.events.mediaChanged(String(r.folder_id), r.dir, dims ? 'meta' : 'thumb');
          } catch (e) {
            if ((e instanceof HttpError && e.statusCode === 503) || this.closed || conn.closed) offline = true;
            else {
              failed.add(r.id);
              this.ctx.db.run('UPDATE media SET thumb_state = 2 WHERE id = ?', r.id);
            }
          }
        });
        if (offline) break;
      }
    } finally {
      conn.thumbRunning = false;
    }
  }

  // ───────────────────────── pull (PC → phone) ─────────────────────────

  createPull(deviceId: string, abs: string, size: number, mtime: number, hooks: Pick<Pull, 'onStart' | 'onBytes' | 'onEnd'> = {}): string {
    const token = crypto.randomBytes(24).toString('base64url');
    this.pulls.set(token, { deviceId, abs, size, mtime, expires: Date.now() + PULL_TTL_MS, ...hooks });
    return token;
  }

  revokePull(token: string) {
    this.pulls.delete(token);
  }

  /** one-time: the token is consumed by the first GET */
  takePull(token: string, deviceId: string): Pull {
    const p = this.pulls.get(token);
    if (!p || p.expires < Date.now()) throw new HttpError(404, 'Unknown or expired pull token');
    if (p.deviceId !== deviceId) throw new HttpError(403, 'Pull token belongs to another device');
    this.pulls.delete(token);
    return p;
  }

  /** a receive request whose pull started no longer needs its first-byte timer */
  holdReceive(rid: string) {
    const r = this.reqs.get(rid);
    if (r?.timer) clearTimeout(r.timer);
    if (r) r.timer = null;
  }

  /** after the pull finished the phone must confirm (`res`) within a minute */
  awaitReceiveResult(rid: string) {
    const r = this.reqs.get(rid);
    if (!r || r.state === 'done') return;
    if (r.timer) clearTimeout(r.timer);
    r.timer = setTimeout(() => this.fail(r, new HttpError(504, 'The device did not confirm the file'), true), 60_000);
    r.timer.unref?.();
  }

  /** rid of the request currently being sent for a receive (by its args.url) */
  ridOfPull(token: string): string | null {
    for (const r of this.reqs.values()) if (r.op === 'receive' && r.args.url === `/api/devices/pull/${token}`) return r.rid;
    return null;
  }

  // ───────────────────────── details ─────────────────────────

  /** `details` op → refine the row (GPS, camera, capture clock) and return the raw reply */
  async details(r: MediaRow, folder: FolderRow): Promise<DeviceDetails | null> {
    const { deviceId, key } = this.folderOf(folder);
    if (!this.conns.has(deviceId)) return null;
    const rel = r.dir ? `${r.dir}/${r.name}` : r.name;
    const res = await this.fetchBuffer(deviceId, 'details', { key, p: rel }, 1, 2 * 1024 * 1024);
    if (res.status !== 200) return null;
    let d: DeviceDetails;
    try {
      d = JSON.parse(res.data.toString('utf8'));
    } catch {
      return null;
    }
    return d && typeof d === 'object' ? d : null;
  }

  /** Shutdown: drop every phone (pending requests fail), then wait for listing / thumbnail work. */
  async close() {
    if (this.closed) return;
    for (const c of [...this.conns.values(), ...this.pending.values()]) {
      this.detach(c);
      try {
        c.socket.close(1001, 'server shutting down');
      } catch {
        /* ignore */
      }
    }
    this.closed = true;
    clearInterval(this.heartbeat);
    this.pulls.clear();
    await Promise.allSettled([...this.listing.values(), ...this.loops]);
  }
}

export interface DeviceDetails {
  exif?: Record<string, unknown> | null;
  gps?: { lat: number; lon: number } | null;
  make?: string | null;
  model?: string | null;
  lens?: string | null;
}

interface ListEntry {
  dir: string;
  name: string;
  kind: NonNullable<ReturnType<typeof kindOf>>;
  size: number;
  mtime: number;
  width: number | null;
  height: number | null;
  duration: number | null;
  taken: number | null;
}

function parseCaps(s: string | null): { read: boolean; receive: boolean } {
  try {
    const c = s ? JSON.parse(s) : null;
    return { read: c?.read !== false, receive: c?.receive === true };
  } catch {
    return { read: true, receive: false };
  }
}

function parseFolders(v: unknown): HelloFolder[] {
  if (!Array.isArray(v)) return [];
  const out: HelloFolder[] = [];
  const seen = new Set<string>();
  for (const f of v.slice(0, 64)) {
    const key = typeof f?.key === 'string' ? f.key : '';
    if (!KEY_RE.test(key) || seen.has(key)) continue;
    seen.add(key);
    out.push({ key, name: clean(f.name, 120, key), path: clean(f.path, 500) || undefined });
  }
  return out;
}

function safeEq(a: string, b: string): boolean {
  const x = Buffer.from(a);
  const y = Buffer.from(b);
  return x.length === y.length && crypto.timingSafeEqual(x, y);
}

function safeRel(p: string): string | null {
  try {
    return normalizeRel(p) || null;
  } catch {
    return null;
  }
}

function addDirChain(set: Set<string>, d: string) {
  let cur = d;
  while (cur && !set.has(cur)) {
    set.add(cur);
    const i = cur.lastIndexOf('/');
    cur = i < 0 ? '' : cur.slice(0, i);
  }
}

/** ["a", "a/b", "c"] → ["a", "c"]; "" swallows everything */
export function collapseDirs(dirs: string[]): string[] {
  if (dirs.includes('')) return [''];
  const sorted = [...new Set(dirs)].sort();
  const out: string[] = [];
  for (const d of sorted) if (!out.some((o) => d === o || d.startsWith(`${o}/`))) out.push(d);
  return out;
}

const num = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) ? v : null);
const posInt = (v: unknown): number | null => (typeof v === 'number' && Number.isFinite(v) && v > 0 ? Math.round(v) : null);

function abortError(): Error {
  const e = new Error('aborted');
  e.name = 'AbortError';
  return e;
}

/** Split a byte stream into lines (NDJSON) without buffering the whole body. */
export async function* ndjsonLines(stream: Readable, maxLine = 1024 * 1024): AsyncGenerator<string> {
  const dec = new StringDecoder('utf8');
  let buf = '';
  for await (const chunk of stream) {
    buf += dec.write(chunk as Buffer);
    let i: number;
    while ((i = buf.indexOf('\n')) >= 0) {
      const line = buf.slice(0, i).trim();
      buf = buf.slice(i + 1);
      if (line) yield line;
    }
    if (buf.length > maxLine) throw new HttpError(413, 'NDJSON line too long');
  }
  buf += dec.end();
  if (buf.trim()) yield buf.trim();
}

/** Readable that only starts the tunnel request when first read (zip entries are read one by one). */
export function lazyDeviceStream(open: (signal: AbortSignal) => Promise<TunnelReply>): Readable {
  const ctrl = new AbortController();
  let src: Readable | null = null;
  let started = false;
  const out = new Readable({
    read() {
      if (!started) {
        started = true;
        open(ctrl.signal).then(
          (tr) => {
            if (tr.status !== 200) {
              tr.body.destroy();
              out.destroy(new Error(`device answered ${tr.status}`));
              return;
            }
            src = tr.body;
            src.on('data', (c) => {
              if (!out.push(c)) src!.pause();
            });
            src.on('end', () => out.push(null));
            src.on('error', (e) => out.destroy(e));
          },
          (e) => out.destroy(e),
        );
      } else src?.resume();
    },
    destroy(err, cb) {
      ctrl.abort();
      src?.destroy();
      cb(err);
    },
  });
  return out;
}
