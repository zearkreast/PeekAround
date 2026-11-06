/**
 * A tiny fake MEGA API gateway (the parts megajs 1.3.10 and remote/mega.ts use), served over HTTP on
 * loopback so the REAL clients run against it (ServerOptions.megaGateway):
 *  - "cs" commands: us0 / us (v2 login: PBKDF2 → uh, RSA-encrypted csid, AES-wrapped master + private key),
 *    ug, uq, sml, f (folder link listing), g (file link attributes / download URLs);
 *  - "dl/<handle>/<start>-<end>": ciphertext ranges (megajs encrypt → real MAC, real CTR).
 * Every cs request is logged with its query (`sid`, `n`) — the request shapes the tests check.
 */
import crypto from 'node:crypto';
import http from 'node:http';
import { Readable } from 'node:stream';

const e64 = (b: Buffer) => b.toString('base64').replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
const d64 = (s: string) => Buffer.from(s.replace(/-/g, '+').replace(/_/g, '/'), 'base64');

function ecb(key: Buffer, data: Buffer, decrypt = false): Buffer {
  const c = (decrypt ? crypto.createDecipheriv : crypto.createCipheriv)('aes-128-ecb', key, null).setAutoPadding(false);
  return Buffer.concat([c.update(data), c.final()]);
}

/** MEGA attribute block: AES-128-CBC, zero IV, folded key, zero padded */
function attrs(key: Buffer, obj: object): string {
  const k = key.length === 16 ? key : Buffer.from(Array.from({ length: 16 }, (_, i) => key[i] ^ key[i + 16]));
  const plain = Buffer.from(`MEGA${JSON.stringify(obj)}`);
  const padded = Buffer.alloc(Math.ceil(plain.length / 16) * 16);
  plain.copy(padded);
  const c = crypto.createCipheriv('aes-128-cbc', k, Buffer.alloc(16)).setAutoPadding(false);
  return e64(Buffer.concat([c.update(padded), c.final()]));
}

const big = (b64u: string) => BigInt(`0x${d64(b64u).toString('hex')}`);
const bytesOf = (n: bigint) => {
  let h = n.toString(16);
  if (h.length % 2) h = `0${h}`;
  return Buffer.from(h, 'hex');
};
/** MPI: 16-bit big-endian bit count + big-endian bytes */
const mpi = (n: bigint) => {
  const b = bytesOf(n);
  const head = Buffer.alloc(2);
  head.writeUInt16BE(n.toString(2).length);
  return Buffer.concat([head, b]);
};
function modPow(b: bigint, e: bigint, m: bigint): bigint {
  let r = 1n;
  b %= m;
  while (e > 0n) {
    if (e & 1n) r = (r * b) % m;
    b = (b * b) % m;
    e >>= 1n;
  }
  return r;
}
function modInv(a: bigint, m: bigint): bigint {
  let [r0, r1, s0, s1] = [a % m, m, 1n, 0n];
  while (r1) {
    const q = r0 / r1;
    [r0, r1] = [r1, r0 - q * r1];
    [s0, s1] = [s1, s0 - q * s1];
  }
  return ((s0 % m) + m) % m;
}

async function encryptMega(data: Buffer): Promise<{ key: Buffer; cipher: Buffer }> {
  const mod: any = await import('megajs');
  const enc = (mod.encrypt ?? mod.default.encrypt)();
  const out: Buffer[] = [];
  const done = new Promise<void>((resolve, reject) => {
    enc.on('data', (c: Buffer) => out.push(c));
    enc.on('end', resolve);
    enc.on('error', reject);
  });
  // megajs encrypts in place: never hand it the caller's buffer
  Readable.from([Buffer.from(data)]).pipe(enc);
  await done;
  return { key: Buffer.from(enc.key), cipher: Buffer.concat(out) };
}

export interface FakeAccount {
  email: string;
  password: string;
  /** two-factor code the account requires */
  mfa?: string;
  utype: number;
  mxfer?: number;
  caxfer?: number;
}

export interface CsLog {
  a: string;
  sid: string | null;
  n: string | null;
  cmd: Record<string, unknown>;
}

interface FakeNode {
  h: string;
  name: string;
  key: Buffer;
  cipher: Buffer;
  size: number;
}

export class FakeMega {
  log: CsLog[] = [];
  dl: { h: string; range: string }[] = [];
  /** valid session ids */
  sessions = new Set<string>();
  accounts: FakeAccount[] = [];
  /** answer every request that carries a sid with -15 (expired / killed session) */
  rejectSessions = false;
  private server: http.Server;
  private salt = crypto.randomBytes(16);
  private master = crypto.randomBytes(16);
  private rsa: { n: bigint; e: bigint; privk: Buffer };
  private files = new Map<string, FakeNode>(); // file links + folder children by handle
  private fileLinks = new Map<string, FakeNode>();
  private folders = new Map<string, { key: Buffer; root: string; name: string; kids: FakeNode[] }>();
  url = '';
  base = '';

  constructor() {
    const { privateKey } = crypto.generateKeyPairSync('rsa', { modulusLength: 2048 });
    const j = privateKey.export({ format: 'jwk' }) as Record<string, string>;
    const [n, e, d, p, q] = [big(j.n), big(j.e), big(j.d), big(j.p), big(j.q)];
    // megajs RSAdecrypt(m, d, p, q, u): Garner with u = p⁻¹ mod q
    const u = modInv(p, q);
    let privk = Buffer.concat([mpi(p), mpi(q), mpi(d), mpi(u)]);
    privk = Buffer.concat([privk, Buffer.alloc((16 - (privk.length % 16)) % 16)]);
    this.rsa = { n, e, privk };
    this.server = http.createServer((req, res) => void this.handle(req, res));
  }

  async start(): Promise<this> {
    await new Promise<void>((r) => this.server.listen(0, '127.0.0.1', r));
    const port = (this.server.address() as { port: number }).port;
    this.base = `http://127.0.0.1:${port}/`;
    this.url = this.base;
    return this;
  }

  close() {
    this.server.closeAllConnections?.();
    return new Promise<void>((r) => this.server.close(() => r()));
  }

  private uh(password: string): { aes: Buffer; uh: string } {
    const r = crypto.pbkdf2Sync(Buffer.from(password), this.salt, 100_000, 32, 'sha512');
    return { aes: r.subarray(0, 16), uh: e64(r.subarray(16)) };
  }

  async addFileLink(id: string, name: string, data: Buffer): Promise<string> {
    const { key, cipher } = await encryptMega(data);
    const node = { h: id, name, key, cipher, size: data.length };
    this.fileLinks.set(id, node);
    this.files.set(id, node);
    return `https://mega.nz/file/${id}#${e64(key)}`;
  }

  async addFolderLink(id: string, name: string, files: { h: string; name: string; data: Buffer }[]): Promise<string> {
    const key = crypto.randomBytes(16);
    const kids: FakeNode[] = [];
    for (const f of files) {
      const enc = await encryptMega(f.data);
      const node = { h: f.h, name: f.name, key: enc.key, cipher: enc.cipher, size: f.data.length };
      kids.push(node);
      this.files.set(f.h, node);
    }
    this.folders.set(id, { key, root: `R${id.slice(1)}`, name, kids });
    return `https://mega.nz/folder/${id}#${e64(key)}`;
  }

  /** cs requests that carried this sid */
  withSid(sid: string) {
    return this.log.filter((l) => l.sid === sid);
  }

  private async handle(req: http.IncomingMessage, res: http.ServerResponse) {
    const u = new URL(req.url ?? '/', this.base);
    if (u.pathname.startsWith('/dl/')) {
      const [, , h, range] = u.pathname.split('/');
      const f = this.files.get(h);
      const m = /^(\d+)-(\d+)$/.exec(range ?? '');
      if (!f || !m) return void res.writeHead(404).end();
      this.dl.push({ h, range });
      res.writeHead(200, { 'content-type': 'application/octet-stream' });
      return void res.end(f.cipher.subarray(Number(m[1]), Number(m[2]) + 1));
    }
    if (u.pathname !== '/cs') return void res.writeHead(404).end();
    const chunks: Buffer[] = [];
    for await (const c of req) chunks.push(c as Buffer);
    const cmds = JSON.parse(Buffer.concat(chunks).toString()) as Record<string, unknown>[];
    const sid = u.searchParams.get('sid');
    const n = u.searchParams.get('n');
    const out = cmds.map((cmd) => {
      this.log.push({ a: String(cmd.a), sid, n, cmd });
      if (sid && (this.rejectSessions || !this.sessions.has(sid))) return -15;
      return this.command(cmd, sid, n);
    });
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify(out));
  }

  private command(cmd: Record<string, any>, sid: string | null, n: string | null): unknown {
    switch (cmd.a) {
      case 'us0':
        return { v: 2, s: this.salt.toString('base64') };
      case 'us': {
        const acc = this.accounts.find((a) => a.email === cmd.user);
        if (!acc) return -9;
        const { aes, uh } = this.uh(acc.password);
        if (cmd.uh !== uh) return -9;
        if (acc.mfa && cmd.mfa !== acc.mfa) return -26;
        // session id: 43 random bytes (first one non-zero) + filler → RSA with the account's public key
        const raw = crypto.randomBytes(43);
        raw[0] |= 1;
        const s = e64(raw);
        this.sessions.add(s);
        const m = BigInt(`0x${Buffer.concat([raw, crypto.randomBytes(212)]).toString('hex')}`);
        const c = modPow(m, this.rsa.e, this.rsa.n);
        return {
          k: e64(ecb(aes, this.master)),
          csid: e64(mpi(c)),
          privk: e64(ecb(this.master, this.rsa.privk)),
          u: 'UsErHnd1',
        };
      }
      case 'ug': {
        if (!sid) return -15;
        const acc = this.accounts[0];
        return { u: 'UsErHnd1', email: acc?.email, name: 'Test' };
      }
      case 'uq': {
        if (!sid) return -15;
        const acc = this.accounts[0];
        return { utype: acc?.utype ?? 0, ...(acc?.mxfer ? { mxfer: acc.mxfer } : {}), ...(acc?.caxfer !== undefined ? { caxfer: acc.caxfer } : {}) };
      }
      case 'sml':
        if (sid) this.sessions.delete(sid);
        return 0;
      case 'f': {
        const f = n ? this.folders.get(n) : undefined;
        if (!f) return -9;
        const ts = 1_700_000_000;
        return {
          f: [
            { h: f.root, p: 'PaReNt00', t: 1, ts, a: attrs(f.key, { n: f.name }), k: `${f.root}:${e64(ecb(f.key, f.key))}` },
            ...f.kids.map((k) => ({ h: k.h, p: f.root, t: 0, ts, s: k.size, a: attrs(k.key, { n: k.name }), k: `${k.h}:${e64(ecb(f.key, k.key))}` })),
          ],
          ok: [],
        };
      }
      case 'g': {
        const node = cmd.p ? this.fileLinks.get(String(cmd.p)) : n && this.folders.get(n)?.kids.find((k) => k.h === cmd.n);
        if (!node) return -9;
        if (cmd.g) return { g: `${this.base}dl/${node.h}`, s: node.size };
        return { s: node.size, at: attrs(node.key, { n: node.name }) };
      }
      default:
        return -2;
    }
  }
}
