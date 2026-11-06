import crypto from 'node:crypto';
import { Transform, type TransformCallback } from 'node:stream';

/**
 * MEGA content crypto, just what streaming needs (verified against megajs 1.3.10 lib/crypto and
 * the MEGA web client):
 *  - a file node key is 32 bytes: k[0..16] ⊕ k[16..32] = AES-128 key ("folded"), k[16..24] = CTR nonce,
 *    k[24..32] = meta MAC (not checked for ranged reads);
 *  - file data = AES-128-CTR, IV = nonce(8) ‖ big-endian 64-bit block index (offset / 16) → random access;
 *  - file attributes (thumbnail type 0, preview type 1) = AES-128-CBC, zero IV, folded key,
 *    length padded to 16; the `fa` string lists them as "<cluster>:<type>*<handle>/…" (handle = 8 bytes, base64url).
 */

export function b64url(buf: Buffer): string {
  return buf.toString('base64').replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

export function fromB64url(s: string): Buffer {
  return Buffer.from(s.replace(/-/g, '+').replace(/_/g, '/'), 'base64');
}

/** AES key of a 32 byte file node key (16 byte folder keys are returned as they are) */
export function foldKey(key: Buffer): Buffer {
  if (key.length === 16) return Buffer.from(key);
  if (key.length < 32) throw new Error('wrong key length');
  const out = Buffer.alloc(16);
  for (let i = 0; i < 16; i++) out[i] = key[i] ^ key[i + 16];
  return out;
}

/** CTR IV for the 16-byte block that contains `offset` */
export function ctrIv(key: Buffer, offset: number): Buffer {
  const iv = Buffer.alloc(16);
  key.copy(iv, 0, 16, 24);
  iv.writeBigUInt64BE(BigInt(Math.floor(offset / 16)), 8);
  return iv;
}

/**
 * Decrypt a ciphertext stream that starts at file offset `alignedStart` (multiple of 16) and emit
 * exactly the plaintext bytes [start, end] (inclusive).
 */
export function ctrRangeDecryptor(key: Buffer, alignedStart: number, start: number, end: number): Transform {
  if (alignedStart % 16) throw new Error('alignedStart must be a multiple of 16');
  const decipher = crypto.createDecipheriv('aes-128-ctr', foldKey(key), ctrIv(key, alignedStart));
  let skip = start - alignedStart;
  let left = end - start + 1;
  return new Transform({
    transform(chunk: Buffer, _enc, cb: TransformCallback) {
      if (left <= 0) return cb();
      let out = decipher.update(chunk);
      if (skip > 0) {
        const n = Math.min(skip, out.length);
        out = out.subarray(n);
        skip -= n;
      }
      if (out.length > left) out = out.subarray(0, left);
      left -= out.length;
      cb(null, out.length ? out : undefined);
    },
  });
}

/** encrypt/decrypt a whole buffer with CTR starting at file offset 0 (tests, fakes) */
export function ctrXor(key: Buffer, data: Buffer, offset = 0): Buffer {
  const aligned = offset - (offset % 16);
  const c = crypto.createCipheriv('aes-128-ctr', foldKey(key), ctrIv(key, aligned));
  const pad = Buffer.alloc(offset - aligned);
  return c.update(Buffer.concat([pad, data])).subarray(pad.length);
}

export interface FileAttrRef {
  /** storage cluster id */
  cluster: number;
  /** 0 = 120px square thumbnail, 1 = preview (~1000px, aspect kept), 8/9 = media info */
  type: number;
  /** 8-byte attribute handle */
  handle: Buffer;
}

/** "123:0*AbCdEfGhIjK/456:1*…" → refs (malformed parts are skipped) */
export function parseFa(fa: string | null | undefined): FileAttrRef[] {
  if (typeof fa !== 'string' || !fa) return [];
  const out: FileAttrRef[] = [];
  for (const part of fa.split('/')) {
    const m = /^(\d+):(\d+)\*([A-Za-z0-9_-]{11})$/.exec(part.trim());
    if (!m) continue;
    const handle = fromB64url(m[3]);
    if (handle.length !== 8) continue;
    out.push({ cluster: Number(m[1]), type: Number(m[2]), handle });
  }
  return out;
}

/** decrypt a downloaded file attribute (thumbnail / preview JPEG) */
export function decryptFileAttr(key: Buffer, data: Buffer): Buffer {
  const len = data.length - (data.length % 16);
  if (!len) return Buffer.alloc(0);
  const d = crypto.createDecipheriv('aes-128-cbc', foldKey(key), Buffer.alloc(16)).setAutoPadding(false);
  const out = Buffer.concat([d.update(data.subarray(0, len)), d.final()]);
  return trimJpeg(out);
}

/** encrypt a file attribute like the MEGA uploaders do (zero padded to 16) — tests and fakes */
export function encryptFileAttr(key: Buffer, plain: Buffer): Buffer {
  const padded = Buffer.alloc(Math.ceil(plain.length / 16) * 16);
  plain.copy(padded);
  const c = crypto.createCipheriv('aes-128-cbc', foldKey(key), Buffer.alloc(16)).setAutoPadding(false);
  return Buffer.concat([c.update(padded), c.final()]);
}

/** cut the zero padding after a JPEG's EOI marker (other formats are returned unchanged) */
function trimJpeg(b: Buffer): Buffer {
  if (b.length < 4 || b[0] !== 0xff || b[1] !== 0xd8) return b;
  let end = b.length;
  while (end > 2 && b[end - 1] === 0) end--;
  return end >= 2 && b[end - 2] === 0xff && b[end - 1] === 0xd9 ? b.subarray(0, end) : b;
}

/** body of a file attribute POST: repeated [8-byte handle][uint32 LE length][data] */
export function parseFileAttrResponse(buf: Buffer): Map<string, Buffer> {
  const out = new Map<string, Buffer>();
  let p = 0;
  while (p + 12 <= buf.length) {
    const h = buf.subarray(p, p + 8).toString('hex');
    const len = buf.readUInt32LE(p + 8);
    p += 12;
    if (p + len > buf.length) break;
    out.set(h, buf.subarray(p, p + len));
    p += len;
  }
  return out;
}

export function buildFileAttrResponse(parts: { handle: Buffer; data: Buffer }[]): Buffer {
  return Buffer.concat(
    parts.flatMap(({ handle, data }) => {
      const len = Buffer.alloc(4);
      len.writeUInt32LE(data.length);
      return [handle, len, data];
    }),
  );
}
