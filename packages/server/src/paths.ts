import fs from 'node:fs';
import fsp from 'node:fs/promises';
import path from 'node:path';

export class HttpError extends Error {
  constructor(
    public statusCode: number,
    message: string,
  ) {
    super(message);
  }
}

/** Normalize a client supplied relative path ("a/b", "a\\b", "/a/b/") → "a/b". Rejects traversal. */
export function normalizeRel(rel: string | undefined | null): string {
  if (!rel) return '';
  const parts = String(rel)
    .replace(/\\/g, '/')
    .split('/')
    .filter((p) => p !== '' && p !== '.');
  for (const p of parts) {
    if (p === '..') throw new HttpError(400, 'Invalid path');
    if (p.includes('\0')) throw new HttpError(400, 'Invalid path');
  }
  return parts.join('/');
}

/**
 * Resolve `rel` inside `root` and guarantee the result stays inside it,
 * also after following symlinks of the existing part of the path.
 */
export function resolveInRoot(root: string, rel: string): string {
  const clean = normalizeRel(rel);
  const rootAbs = path.resolve(root);
  const abs = path.resolve(rootAbs, ...clean.split('/').filter(Boolean));
  if (!isInside(rootAbs, abs)) throw new HttpError(400, 'Path escapes root');
  let probe = abs;
  while (!fs.existsSync(probe) && probe !== rootAbs) probe = path.dirname(probe);
  try {
    const realRoot = fs.realpathSync(rootAbs);
    const realProbe = fs.realpathSync(probe);
    if (!isInside(realRoot, realProbe)) throw new HttpError(400, 'Path escapes root');
  } catch (e) {
    if (e instanceof HttpError) throw e;
  }
  return abs;
}

export function isInside(root: string, p: string): boolean {
  const r = path.resolve(root);
  const x = path.resolve(p);
  if (x === r) return true;
  const rel = path.relative(r, x);
  return !!rel && !rel.startsWith('..') && !path.isAbsolute(rel);
}

const WIN_RESERVED = /^(con|prn|aux|nul|com\d|lpt\d)$/i;

/** Make a single path segment safe for every OS. */
export function sanitizeName(name: string): string {
  let n = String(name)
    .replace(/[\u0000-\u001f<>:"/\\|?*]/g, '_')
    .replace(/[. ]+$/g, '')
    .trim();
  if (!n || n === '.' || n === '..') n = 'untitled';
  const base = n.split('.')[0];
  if (WIN_RESERVED.test(base)) n = `_${n}`;
  if (n.length > 200) {
    const ext = path.extname(n);
    n = n.slice(0, 200 - ext.length) + ext;
  }
  return n;
}

/** Sanitize every segment of a relative path. */
export function sanitizeRel(rel: string): string {
  return normalizeRel(rel)
    .split('/')
    .filter(Boolean)
    .map(sanitizeName)
    .join('/');
}

/** "a.jpg" → "a (1).jpg" … until free. */
export async function uniquePath(dir: string, name: string): Promise<string> {
  const ext = path.extname(name);
  const base = name.slice(0, name.length - ext.length);
  let candidate = path.join(dir, name);
  for (let i = 1; await exists(candidate); i++) {
    candidate = path.join(dir, `${base} (${i})${ext}`);
  }
  return candidate;
}

/** "a.jpg" + "_edit" → "a_edit.jpg", "a_edit-2.jpg" … */
export async function uniqueEditPath(dir: string, base: string, suffix: string, ext: string): Promise<string> {
  let candidate = path.join(dir, `${base}${suffix}.${ext}`);
  for (let i = 2; await exists(candidate); i++) {
    candidate = path.join(dir, `${base}${suffix}-${i}.${ext}`);
  }
  return candidate;
}

export async function exists(p: string): Promise<boolean> {
  try {
    await fsp.lstat(p);
    return true;
  } catch {
    return false;
  }
}

export function toPosix(p: string): string {
  return p.split(path.sep).join('/');
}

export function isHiddenName(name: string): boolean {
  return name.startsWith('.') || name === '@eaDir' || name === '$RECYCLE.BIN' || name === 'System Volume Information' || name === '#recycle';
}
