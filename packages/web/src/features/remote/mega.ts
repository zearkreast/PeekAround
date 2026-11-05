/** MEGA link detection + the inspect tree model of the import dialog (pure, unit tested). */
import type { RemoteInspect, RemoteNode } from '@peekaround/shared';
import { extOf, parentDir } from '@peekaround/shared';

const HOST = String.raw`(?:www\.)?mega(?:\.co)?\.nz`;
/** new style /file|folder/<id>#<key>[/file|folder/<id>] and legacy /#!id!key, /#F!id!key[!node] */
const LINK = new RegExp(String.raw`(?:https?:\/\/)?${HOST}\/(?:(?:file|folder)\/[A-Za-z0-9_-]{6,}#[A-Za-z0-9_-]{8,}(?:\/(?:file|folder)\/[A-Za-z0-9_-]{6,})?|#F?![A-Za-z0-9_-]{6,}![A-Za-z0-9_-]{8,}(?:![A-Za-z0-9_-]{6,})?)`, 'i');

/** First MEGA link inside arbitrary text (pasted text, a share intent, a dropped URL), normalised to https. */
export function findMegaUrl(text: string | null | undefined): string | null {
  if (!text) return null;
  const m = LINK.exec(text);
  if (!m) return null;
  const raw = m[0];
  return /^https?:\/\//i.test(raw) ? raw.replace(/^http:/i, 'https:') : `https://${raw}`;
}

/** Something that looks like a MEGA URL but is missing the key (user pasted half a link). */
export function looksLikeMega(text: string): boolean {
  return new RegExp(HOST, 'i').test(text);
}

// ── tree ──

export interface TreeNode {
  node: RemoteNode;
  children: TreeNode[];
  /** files below (this file = 1) */
  files: string[];
}

export interface Tree {
  root: TreeNode;
  byId: Map<string, TreeNode>;
  files: RemoteNode[];
}

export function buildTree(r: RemoteInspect): Tree {
  const byPath = new Map<string, TreeNode>();
  const byId = new Map<string, TreeNode>();
  let root: TreeNode | null = null;
  for (const n of r.nodes) {
    const t: TreeNode = { node: n, children: [], files: n.dir ? [] : [n.id] };
    byId.set(n.id, t);
    if (n.path === '') {
      root = t;
      continue;
    }
    byPath.set(n.path, t);
  }
  root ??= { node: { id: '', path: '', name: r.name, size: r.size, dir: true, kind: null }, children: [], files: [] };
  for (const n of r.nodes) {
    if (n.path === '') continue;
    const t = byPath.get(n.path)!;
    const parent = byPath.get(parentDir(n.path)) ?? root;
    parent.children.push(t);
  }
  // bottom-up file lists (nodes are breadth first: walk backwards)
  for (let i = r.nodes.length - 1; i >= 0; i--) {
    const n = r.nodes[i];
    const t = byId.get(n.id)!;
    if (n.path === '') continue;
    const parent = byPath.get(parentDir(n.path)) ?? root;
    parent.files.push(...t.files);
  }
  return { root, byId, files: r.nodes.filter((n) => !n.dir) };
}

export const isMediaNode = (n: RemoteNode) => n.kind === 'image' || n.kind === 'video';

/** media files (and zips, which may hold media) are preselected */
export function defaultSelection(r: RemoteInspect): Set<string> {
  const media = r.nodes.filter((n) => !n.dir && (isMediaNode(n) || n.kind === 'audio'));
  // a link without any media: select everything so the dialog is never stuck at 0
  return new Set((media.length ? media : r.nodes.filter((n) => !n.dir)).map((n) => n.id));
}

export function totals(tree: Tree, sel: Set<string>): { files: number; bytes: number; zips: string[] } {
  let files = 0;
  let bytes = 0;
  const zips: string[] = [];
  for (const f of tree.files) {
    if (!sel.has(f.id)) continue;
    files++;
    bytes += f.size;
    if (extOf(f.name) === 'zip') zips.push(f.name);
  }
  return { files, bytes, zips };
}

/** 'all' | 'none' | 'some' of the files below a folder */
export function checkState(t: TreeNode, sel: Set<string>): 'all' | 'none' | 'some' {
  let n = 0;
  for (const id of t.files) if (sel.has(id)) n++;
  return n === 0 ? 'none' : n === t.files.length ? 'all' : 'some';
}

export function toggleNode(t: TreeNode, sel: Set<string>): Set<string> {
  const next = new Set(sel);
  const on = checkState(t, sel) !== 'all';
  for (const id of t.files) {
    if (on) next.add(id);
    else next.delete(id);
  }
  return next;
}

/**
 * Node ids for RemoteImportSpec.nodes: undefined when everything is selected; otherwise
 * whole folders where every file below is selected, plus the remaining single files.
 */
export function importNodes(tree: Tree, sel: Set<string>): string[] | undefined {
  if (tree.files.every((f) => sel.has(f.id))) return undefined;
  const out: string[] = [];
  const walk = (t: TreeNode) => {
    for (const c of t.children) {
      if (!c.files.length) continue;
      const s = checkState(c, sel);
      if (s === 'none') continue;
      if (s === 'all' && c.node.dir) out.push(c.node.id);
      else if (!c.node.dir) out.push(c.node.id);
      else walk(c);
    }
  };
  walk(tree.root);
  return out;
}

/** API error → message key (+ vars) for the dialog. */
export function megaErrorKey(status: number, message: string): { key: 'remote.err.quota' | 'remote.err.quotaNoTime' | 'remote.err.notFound' | 'remote.err.badKey' | 'remote.err.notMega' | 'remote.err.unavailable' | 'remote.err.network' | 'remote.err.other'; vars?: Record<string, string | number> } {
  if (status === 429) {
    const m = /retry in about (\d+) min/i.exec(message);
    return m ? { key: 'remote.err.quota', vars: { min: Number(m[1]) } } : { key: 'remote.err.quotaNoTime' };
  }
  if (status === 404) return { key: 'remote.err.notFound' };
  if (status === 400) return /key/i.test(message) ? { key: 'remote.err.badKey' } : { key: 'remote.err.notMega' };
  if (status === 503) return { key: 'remote.err.unavailable' };
  if (status === 502 && /reach/i.test(message)) return { key: 'remote.err.network' };
  if (status === 0) return { key: 'remote.err.network' };
  return { key: 'remote.err.other', vars: { error: message } };
}
