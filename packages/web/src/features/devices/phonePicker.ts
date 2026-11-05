/** Pure helpers for PhoneFolderPicker (breadcrumbs, last used folder, selection). */

export interface PhoneCrumb {
  label: string;
  path: string;
}

/** Storage volume of an absolute Android path: internal (/storage/emulated/N) or an SD card (/storage/XXXX-XXXX). */
export function volumeOf(path: string): { base: string; kind: 'internal' | 'sd' | 'other' } {
  const m = /^\/storage\/emulated\/\d+/.exec(path);
  if (m) return { base: m[0], kind: 'internal' };
  const sd = /^\/storage\/[^/]+/.exec(path);
  if (sd && sd[0] !== '/storage/self') return { base: sd[0], kind: 'sd' };
  return { base: '/', kind: 'other' };
}

/** Breadcrumbs from the volume root down to `path`. */
export function crumbsOf(path: string, labels: { internal: string; sd: string }): PhoneCrumb[] {
  const clean = path.replace(/\/+$/, '') || '/';
  const v = volumeOf(clean);
  const out: PhoneCrumb[] = [{ label: v.kind === 'internal' ? labels.internal : v.kind === 'sd' ? labels.sd : '/', path: v.base }];
  const rest = clean.slice(v.base.length).split('/').filter(Boolean);
  let acc = v.base === '/' ? '' : v.base;
  for (const seg of rest) {
    acc = `${acc}/${seg}`;
    out.push({ label: seg, path: acc });
  }
  return out;
}

/** Parent folder, never above the volume root (null there). */
export function parentOf(path: string): string | null {
  const clean = path.replace(/\/+$/, '');
  const v = volumeOf(clean);
  if (clean === v.base || clean.length <= v.base.length) return null;
  const p = clean.slice(0, clean.lastIndexOf('/'));
  return p.length < v.base.length ? v.base : p || '/';
}

export function baseName(path: string): string {
  const clean = path.replace(/\/+$/, '');
  return clean.slice(clean.lastIndexOf('/') + 1) || clean;
}

/** Display a phone path relative to its volume: "DCIM/Camera" (or the volume label). */
export function prettyPath(path: string, labels: { internal: string; sd: string }): string {
  const c = crumbsOf(path, labels);
  return c.length > 1 ? c.slice(1).map((x) => x.label).join('/') : c[0].label;
}

export function toggle(sel: readonly string[], path: string): string[] {
  return sel.includes(path) ? sel.filter((p) => p !== path) : [...sel, path];
}

/** Drop paths that are inside another selected path (a selected folder already includes them). */
export function collapseNested(paths: readonly string[]): string[] {
  const sorted = [...new Set(paths)].sort((a, b) => a.length - b.length);
  const out: string[] = [];
  for (const p of sorted) if (!out.some((q) => p === q || p.startsWith(q + '/'))) out.push(p);
  return out;
}

// ── last used folders (per purpose: "download", "share", "upload") ──

const KEY = 'peek.phonePicker.last';

type Store = { getItem(k: string): string | null; setItem(k: string, v: string): void };

function store(): Store | null {
  try {
    return typeof localStorage === 'undefined' ? null : localStorage;
  } catch {
    return null;
  }
}

/** most recent first, at most 5 */
export function loadRecent(purpose: string, s: Store | null = store()): string[] {
  try {
    const all = JSON.parse(s?.getItem(KEY) ?? '{}') as Record<string, string[]>;
    return Array.isArray(all[purpose]) ? all[purpose].filter((x) => typeof x === 'string') : [];
  } catch {
    return [];
  }
}

export function saveRecent(purpose: string, path: string, s: Store | null = store()): string[] {
  let all: Record<string, string[]> = {};
  try {
    all = JSON.parse(s?.getItem(KEY) ?? '{}');
  } catch {
    /* reset */
  }
  const next = [path, ...(all[purpose] ?? []).filter((p) => p !== path)].slice(0, 5);
  all[purpose] = next;
  try {
    s?.setItem(KEY, JSON.stringify(all));
  } catch {
    /* ignore */
  }
  return next;
}
