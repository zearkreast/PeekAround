import type { Readable } from 'node:stream';
import { kindOf, type RemoteInspect, type RemoteNode } from '@peekaround/shared';
import { HttpError } from '../paths.js';

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
      const f = new File({ downloadId: link.id, key: link.key, directory: link.type === 'folder', loadedFile: link.sub?.id });
      let node: any;
      try {
        node = await f.loadAttributes();
      } catch (e) {
        throw megaError(e);
      }
      return wrapMega(node);
    },
  };
}

function wrapMega(f: any): MegaNode {
  let kids: MegaNode[] | null = null;
  return {
    id: String(Array.isArray(f.downloadId) ? f.downloadId[1] : (f.nodeId ?? f.downloadId)),
    name: typeof f.name === 'string' && f.name ? f.name : 'untitled',
    size: Number(f.size) || 0,
    directory: !!f.directory,
    timestamp: f.timestamp ? Number(f.timestamp) * 1000 : null,
    get children() {
      kids ??= (f.children ?? []).map(wrapMega);
      return kids!;
    },
    download({ signal }) {
      const s: Readable = f.download({ maxConnections: 4 });
      const stop = () => s.destroy(Object.assign(new Error('aborted'), { name: 'AbortError' }));
      if (signal.aborted) stop();
      else signal.addEventListener('abort', stop, { once: true });
      return s;
    },
  };
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
