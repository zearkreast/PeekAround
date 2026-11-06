import { PassThrough, Readable } from 'node:stream';
import { pipeline } from 'node:stream/promises';
import { kindOf, type RemoteInspect, type RemoteNode } from '@peekaround/shared';
import { HttpError } from '../paths.js';
import { b64url, ctrRangeDecryptor, decryptFileAttr, parseFa, parseFileAttrResponse } from './crypto.js';

/**
 * MEGA (mega.nz / mega.co.nz) public links: parsing, an injectable client (megajs 1.3.10, Node
 * build) and the tree walk behind POST /api/remote/inspect.
 */

export interface MegaLink {
  type: 'file' | 'folder';
  id: string;
  key: string;
  /** node inside a folder link (".../folder/<id>#<key>/file/<sub>" or "/folder/<sub>", legacy "#F!id!key!sub") */
  sub?: { type: 'file' | 'folder' | 'node'; id: string };
}

const HOSTS = new Set(['mega.nz', 'www.mega.nz', 'mega.co.nz', 'www.mega.co.nz']);
const ID = /^[A-Za-z0-9_-]{8}$/;
const KEY = /^[A-Za-z0-9_-]{16,64}$/;
const NODE = /^[A-Za-z0-9_-]{8}$/;

/** Parse a MEGA link; anything else → HttpError 400. */
export function parseMegaUrl(input: string): MegaLink {
  const bad = (why = 'Not a MEGA link') => new HttpError(400, why);
  if (typeof input !== 'string' || input.length > 2048) throw bad();
  let url: URL;
  try {
    url = new URL(input.trim());
  } catch {
    throw bad();
  }
  if (url.protocol !== 'https:' && url.protocol !== 'http:') throw bad();
  if (!HOSTS.has(url.hostname.toLowerCase()) || url.username || url.password || (url.port && url.port !== '443')) throw bad();
  const hash = decodeURIComponent(url.hash.replace(/^#/, ''));
  const pathParts = url.pathname.split('/').filter(Boolean);

  // new style: /file/<id>#<key>  /folder/<id>#<key>[/file/<id>|/folder/<id>]
  if (pathParts.length === 2 && (pathParts[0] === 'file' || pathParts[0] === 'folder')) {
    const type = pathParts[0] as 'file' | 'folder';
    const id = pathParts[1];
    const [key, subType, subId, ...rest] = hash.split('/');
    if (!ID.test(id) || !KEY.test(key ?? '')) throw bad('Invalid MEGA link (id or key missing)');
    if (subType === undefined) return { type, id, key };
    if (type !== 'folder' || rest.length || (subType !== 'file' && subType !== 'folder') || !NODE.test(subId ?? '')) throw bad();
    return { type, id, key, sub: { type: subType, id: subId } };
  }

  // legacy: /#!<id>!<key>   /#F!<id>!<key>[!<node>]
  if (pathParts.length === 0 && hash.startsWith('!')) {
    const [, id, key, ...rest] = hash.split('!');
    if (rest.length || !ID.test(id ?? '') || !KEY.test(key ?? '')) throw bad('Invalid MEGA link (id or key missing)');
    return { type: 'file', id, key };
  }
  if (pathParts.length === 0 && hash.startsWith('F!')) {
    const [, id, key, sub, ...rest] = hash.split('!');
    if (rest.length || !ID.test(id ?? '') || !KEY.test(key ?? '')) throw bad('Invalid MEGA link (id or key missing)');
    if (sub === undefined || sub === '') return { type: 'folder', id, key };
    if (!NODE.test(sub)) throw bad();
    return { type: 'folder', id, key, sub: { type: 'node', id: sub } };
  }
  throw bad();
}

export interface MegaNode {
  id: string;
  name: string;
  size: number;
  directory: boolean;
  /** ms epoch (MEGA "ts") or null */
  timestamp: number | null;
  children: MegaNode[];
  /** decrypted content stream (files only); destroy it to cancel */
  download(opts: { signal: AbortSignal }): Readable;
  /** raw MEGA file attribute list ("<cluster>:<type>*<handle>/…"), when the listing had one */
  fa?: string | null;
  /** decrypted bytes [start, end] (inclusive) — random access, used for streaming (files only) */
  read?(opts: { start: number; end: number; signal: AbortSignal }): Readable;
  /** decrypted file attribute: 0 = 120px square thumbnail, 1 = preview; null when the node has none */
  attr?(type: 0 | 1, signal: AbortSignal): Promise<Buffer | null>;
}

export interface MegaClient {
  /** load the shared node of a link (for folder links: the whole tree; `sub` → that node) */
  load(link: MegaLink): Promise<MegaNode>;
}

/** Real client on top of megajs (dynamic import keeps it out of startup). */
export function megaClient(): MegaClient {
  return {
    async load(link) {
      const mod: any = await import('megajs');
      const File = mod.File ?? mod.default?.File;
      const API = mod.API ?? mod.default?.API;
      // own API instance: its raw responses carry the file attributes (`fa`) megajs drops
      const api = new API(false);
      const fa = new Map<string, string>();
      const orig = api.request.bind(api);
      api.request = (json: any, cb: any, retry?: number) =>
        orig(
          json,
          (err: unknown, resp: any) => {
            if (!err && resp && typeof resp === 'object') {
              if (Array.isArray(resp.f)) for (const n of resp.f) if (n && typeof n.fa === 'string') fa.set(String(n.h), n.fa);
              if (typeof resp.fa === 'string' && json?.p) fa.set(String(json.p), resp.fa);
            }
            cb?.(err, resp);
          },
          retry,
        );
      const f = new File({ downloadId: link.id, key: link.key, directory: link.type === 'folder', loadedFile: link.sub?.id, api });
      let node: any;
      try {
        node = await f.loadAttributes();
      } catch (e) {
        throw megaError(e);
      }
      return wrapMega(node, { api, link, fa });
    },
  };
}

interface WrapCtx {
  api: any;
  link: MegaLink;
  fa: Map<string, string>;
}

function wrapMega(f: any, c: WrapCtx): MegaNode {
  let kids: MegaNode[] | null = null;
  const id = String(Array.isArray(f.downloadId) ? f.downloadId[1] : (f.nodeId ?? f.downloadId));
  const key: Buffer | null = Buffer.isBuffer(f.key) ? f.key : null;
  return {
    id,
    name: typeof f.name === 'string' && f.name ? f.name : 'untitled',
    size: Number(f.size) || 0,
    directory: !!f.directory,
    timestamp: f.timestamp ? Number(f.timestamp) * 1000 : null,
    fa: c.fa.get(id) ?? null,
    get children() {
      kids ??= (f.children ?? []).map((k: any) => wrapMega(k, c));
      return kids!;
    },
    download({ signal }) {
      const s: Readable = f.download({ maxConnections: 4 });
      const stop = () => s.destroy(Object.assign(new Error('aborted'), { name: 'AbortError' }));
      if (signal.aborted) stop();
      else signal.addEventListener('abort', stop, { once: true });
      return s;
    },
    read({ start, end, signal }) {
      if (f.directory || !key) throw new HttpError(400, 'Not a file');
      return megaRead(c, id, key, Number(f.size) || 0, start, end, signal);
    },
    async attr(type, signal) {
      if (!key) return null;
      return megaFileAttr(c, key, c.fa.get(id) ?? null, type, signal);
    },
  };
}

// ── raw transfers (streaming) ──

function apiRequest(api: any, json: Record<string, unknown>): Promise<any> {
  return new Promise((resolve, reject) => {
    try {
      api.request(json, (err: unknown, resp: unknown) => (err ? reject(err) : resolve(resp)));
    } catch (e) {
      reject(e);
    }
  });
}

/** temporary download URLs per node (MEGA "g" command), reused for seeks */
const DL_URLS = new Map<string, { url: string; at: number }>();
const DL_URL_TTL = 10 * 60_000;

async function downloadUrl(c: WrapCtx, nodeId: string, fresh: boolean): Promise<string> {
  const k = `${c.link.id}/${nodeId}`;
  const hit = DL_URLS.get(k);
  if (!fresh && hit && Date.now() - hit.at < DL_URL_TTL) return hit.url;
  const req: Record<string, unknown> =
    c.link.type === 'folder' ? { a: 'g', g: 1, ssl: 2, n: nodeId, _querystring: { n: c.link.id } } : { a: 'g', g: 1, ssl: 2, p: c.link.id };
  const resp = await apiRequest(c.api, req);
  if (typeof resp?.g !== 'string' || !/^https?:/.test(resp.g)) {
    const e = Number(resp?.e);
    const tl = Number(resp?.tl);
    if (e === -17 || tl > 0) throw Object.assign(new Error(`EOVERQUOTA (-17): Bandwidth limit reached: ${tl || 0} seconds until it resets`), tl > 0 ? { timeLimit: tl } : {});
    throw new Error(e ? `MEGA error ${e}` : 'MEGA servers returned an invalid response, maybe caused by rate limit');
  }
  if (DL_URLS.size > 5000) DL_URLS.clear();
  DL_URLS.set(k, { url: resp.g, at: Date.now() });
  return resp.g;
}

function quotaError(res: Response): Error {
  const left = Number(res.headers.get('x-mega-time-left'));
  return Object.assign(new Error(`Bandwidth limit reached: ${left || 0} seconds until it resets`), left > 0 ? { timeLimit: left } : {});
}

/** ranged, decrypted read: one ciphertext request from the 16-byte aligned offset, retried before the first byte */
function megaRead(c: WrapCtx, nodeId: string, key: Buffer, size: number, start: number, end: number, signal: AbortSignal): Readable {
  const out = new PassThrough();
  if (end >= size) end = size - 1;
  if (start < 0 || start > end) {
    process.nextTick(() => out.destroy(new HttpError(416, 'Range not satisfiable')));
    return out;
  }
  const ctrl = new AbortController();
  const abort = () => ctrl.abort();
  if (signal.aborted) ctrl.abort();
  else signal.addEventListener('abort', abort, { once: true });
  out.once('close', abort);
  const aligned = start - (start % 16);
  void (async () => {
    let lastErr: unknown = null;
    for (let attempt = 0; attempt < 4; attempt++) {
      if (ctrl.signal.aborted) throw Object.assign(new Error('aborted'), { name: 'AbortError' });
      if (attempt) await new Promise((r) => setTimeout(r, 500 * 2 ** attempt));
      let res: Response;
      try {
        const url = await downloadUrl(c, nodeId, attempt > 0);
        res = await fetch(`${url}/${aligned}-${end}`, { signal: ctrl.signal });
      } catch (e) {
        lastErr = e;
        if ((e as Error)?.name === 'AbortError' || /OVERQUOTA|ENOENT|EBLOCKED|EACCESS|EKEY/.test(String((e as Error)?.message))) throw e;
        continue;
      }
      if (res.status === 509) throw quotaError(res);
      if (!res.ok || !res.body) {
        lastErr = new Error(`MEGA returned a ${res.status} status code`);
        await res.body?.cancel().catch(() => {});
        // 403/404: expired download URL → fresh one; 5xx: congestion → back off
        continue;
      }
      const body = Readable.fromWeb(res.body as any);
      await pipeline(body, ctrRangeDecryptor(key, aligned, start, end), out);
      return;
    }
    throw lastErr ?? new Error('MEGA download failed');
  })().catch((e) => {
    if (!out.destroyed) out.destroy(e?.name === 'AbortError' ? Object.assign(new Error('aborted'), { name: 'AbortError' }) : e);
  }).finally(() => signal.removeEventListener('abort', abort));
  return out;
}

/** cached file attribute host per handle: the POST url from "ufa" */
async function megaFileAttr(c: WrapCtx, key: Buffer, fa: string | null, type: 0 | 1, signal: AbortSignal): Promise<Buffer | null> {
  const ref = parseFa(fa).find((r) => r.type === type);
  if (!ref) return null;
  const req: Record<string, unknown> = { a: 'ufa', fah: b64url(ref.handle), ssl: 2, r: 1 };
  if (c.link.type === 'folder') req._querystring = { n: c.link.id };
  const resp = await apiRequest(c.api, req);
  const base = typeof resp?.p === 'string' ? resp.p : null;
  if (!base) throw new Error('MEGA returned no file attribute host');
  // the web client posts the handles to "<p>/<type>"; older hosts answer on "<p>" too
  for (const url of [`${base}/${type}`, base]) {
    const res = await fetch(url, { method: 'POST', body: new Uint8Array(ref.handle), signal });
    if (res.status === 509) throw quotaError(res);
    if (!res.ok) continue;
    const buf = Buffer.from(await res.arrayBuffer());
    const part = parseFileAttrResponse(buf).get(ref.handle.toString('hex'));
    if (part && part.length) return decryptFileAttr(key, part);
  }
  return null;
}

/** Friendly messages for MEGA failures. */
export function megaError(e: unknown): HttpError {
  if (e instanceof HttpError) return e;
  const msg = String((e as Error)?.message ?? e);
  const seconds = Number((e as { timeLimit?: unknown })?.timeLimit ?? /(\d+) seconds until it resets/.exec(msg)?.[1]);
  if (/EOVERQUOTA|EGOINGOVERQUOTA|ESHAREROVERQUOTA|Bandwidth limit|\b509\b|ETOOMANYCONNECTIONS|ERATELIMIT/.test(msg)) {
    const wait = Number.isFinite(seconds) && seconds > 0 ? ` (retry in about ${Math.max(1, Math.ceil(seconds / 60))} min)` : '';
    const err = new HttpError(429, `MEGA bandwidth quota exceeded, try again later${wait}`);
    if (Number.isFinite(seconds) && seconds > 0) (err as HttpError & { retryAfter?: number }).retryAfter = seconds;
    return err;
  }
  if (/EBLOCKED/.test(msg)) return new HttpError(404, 'This MEGA link has been taken down');
  if (/ENOENT|not found in folder|EACCESS/.test(msg)) return new HttpError(404, 'MEGA link not found (removed, or the link is wrong)');
  if (/EKEY|could not be decrypted|key isn't defined|wrong key length/.test(msg)) return new HttpError(400, 'Invalid MEGA link key');
  if (/ETEMPUNAVAIL|EAGAIN/.test(msg)) return new HttpError(503, 'MEGA is temporarily unavailable, try again later');
  if (/fetch failed|ENOTFOUND|ECONNRESET|ETIMEDOUT|EAI_AGAIN|network/i.test(msg)) return new HttpError(502, 'Cannot reach MEGA');
  return new HttpError(502, `MEGA error: ${msg.slice(0, 200)}`);
}

export const MAX_INSPECT_NODES = 20_000;

export interface FlatNode {
  node: MegaNode;
  /** path inside the shared tree ('' for the root) */
  path: string;
  /** ids of the node and all of its ancestors */
  chain: string[];
}

/** Breadth-first walk of a loaded tree (root first), capped at `max` nodes. */
export function flatten(root: MegaNode, max = Infinity): { nodes: FlatNode[]; truncated: boolean } {
  const out: FlatNode[] = [];
  const queue: FlatNode[] = [{ node: root, path: '', chain: [root.id] }];
  while (queue.length) {
    if (out.length >= max) return { nodes: out, truncated: true };
    const cur = queue.shift()!;
    out.push(cur);
    if (cur.node.directory) {
      const kids = [...cur.node.children].sort((a, b) => Number(b.directory) - Number(a.directory) || a.name.localeCompare(b.name, undefined, { numeric: true }));
      for (const k of kids) queue.push({ node: k, path: cur.path ? `${cur.path}/${k.name}` : k.name, chain: [...cur.chain, k.id] });
    }
  }
  return { nodes: out, truncated: false };
}

export function inspectTree(root: MegaNode): RemoteInspect {
  const { nodes, truncated } = flatten(root, MAX_INSPECT_NODES);
  // folders report the total size of the files below them
  const dirSize = new Map<string, number>();
  for (const { node, chain } of nodes) if (!node.directory) for (const id of chain.slice(0, -1)) dirSize.set(id, (dirSize.get(id) ?? 0) + node.size);
  let size = 0;
  const flat: RemoteNode[] = nodes.map(({ node, path }) => {
    if (!node.directory) size += node.size;
    const sz = node.directory ? (dirSize.get(node.id) ?? 0) : node.size;
    return { id: node.id, path, name: node.name, size: sz, dir: node.directory, kind: node.directory ? null : kindOf(node.name) };
  });
  return { provider: 'mega', kind: root.directory ? 'folder' : 'file', name: root.name, size, nodes: flat, truncated };
}
