/**
 * Albums (user-curated collections) and per-library options (1.2).
 * Types + the pure exclusion matcher shared by the server scanner and tests.
 */

/** how an album orders its items */
export type AlbumOrder = 'desc' | 'asc' | 'custom';

export interface Album {
  id: string;
  name: string;
  /** chosen cover, or the newest item when none was picked (null for an empty album) */
  coverId: string | null;
  /** true when coverId was picked by the user */
  coverPicked: boolean;
  count: number;
  /** photo / video counts */
  photos: number;
  videos: number;
  /** localAt range of the items (ms, wall clock as UTC) */
  from: number | null;
  to: number | null;
  order: AlbumOrder;
  created: number;
  updated: number;
  sort: number;
}

/** which albums contain some media (album picker check marks) */
export interface AlbumMembership {
  /** album id → how many of the asked items it contains */
  counts: Record<string, number>;
}

/**
 * Per-library scan options. Exclusions are applied by the scanner (walk, watcher, uploads):
 * files / folders that match are not indexed (nothing on disk is touched).
 */
export interface LibraryOptions {
  /**
   * Folder or file names / glob patterns to skip. A pattern without '/' matches any single
   * path segment (e.g. "Thumbnails", "*.tmp", "WhatsApp Stickers"); a pattern with '/' matches
   * the path relative to the library folder (e.g. "Telegram/*\/Cache", "**\/.thumbnails").
   * Case-insensitive.
   */
  exclude: string[];
  /** skip images smaller than this many bytes (icons, stickers); 0 = keep everything */
  minBytes: number;
  /** index hidden folders/files (dot names) of this library; null = global setting */
  showHidden: boolean | null;
}

export const DEFAULT_LIBRARY_OPTIONS: LibraryOptions = { exclude: [], minBytes: 0, showHidden: null };

export function normalizeLibraryOptions(o: Partial<LibraryOptions> | null | undefined): LibraryOptions {
  const exclude = Array.isArray(o?.exclude)
    ? [...new Set(o!.exclude.map((s) => String(s).trim().replace(/\\/g, '/').replace(/^\/+|\/+$/g, '')).filter(Boolean))].slice(0, 100)
    : [];
  const min = Number(o?.minBytes);
  return {
    exclude: exclude.map((s) => s.slice(0, 200)),
    minBytes: Number.isFinite(min) && min > 0 ? Math.min(Math.round(min), 1024 * 1024 * 1024) : 0,
    showHidden: typeof o?.showHidden === 'boolean' ? o.showHidden : null,
  };
}

export interface LibraryStats {
  items: number;
  photos: number;
  videos: number;
  audio: number;
  /** bytes of the indexed media */
  bytes: number;
  /** localAt range */
  from: number | null;
  to: number | null;
  /** distinct directories holding media */
  folders: number;
  /** last completed scan of any of the library's folders (ms), null = never */
  scannedAt: number | null;
  /** a scan / indexing pass is running for this library */
  scanning: boolean;
  /** items whose metadata / thumbnail is not ready yet */
  pending: number;
}

function globToRegExp(glob: string): RegExp {
  let re = '';
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i];
    if (c === '*') {
      if (glob[i + 1] === '*') {
        // '**/' = any number of directories (incl. none); '**' = anything
        if (glob[i + 2] === '/') {
          re += '(?:.*/)?';
          i += 2;
        } else {
          re += '.*';
          i += 1;
        }
      } else re += '[^/]*';
    } else if (c === '?') re += '[^/]';
    else re += c.replace(/[.+^${}()|[\]\\]/g, '\\$&');
  }
  return new RegExp(`^${re}$`, 'i');
}

export interface ExcludeMatcher {
  /** true when this relative path ('/' separated, file or dir) is excluded */
  path(rel: string): boolean;
  /** true when a single name (any segment) is excluded */
  name(name: string): boolean;
  readonly empty: boolean;
}

/** Compile exclusion patterns (see LibraryOptions.exclude). */
export function excludeMatcher(patterns: string[]): ExcludeMatcher {
  const names: RegExp[] = [];
  const paths: RegExp[] = [];
  for (const p of patterns) {
    const s = p.trim().replace(/\\/g, '/').replace(/^\/+|\/+$/g, '');
    if (!s) continue;
    try {
      if (s.includes('/')) paths.push(globToRegExp(s));
      else names.push(globToRegExp(s));
    } catch {
      /* invalid pattern — ignore */
    }
  }
  const name = (n: string) => names.some((r) => r.test(n));
  return {
    empty: !names.length && !paths.length,
    name,
    path(rel: string) {
      if (!rel) return false;
      const segs = rel.split('/').filter(Boolean);
      if (names.length && segs.some(name)) return true;
      if (!paths.length) return false;
      // a path pattern also excludes everything below a matching directory
      let acc = '';
      for (const s of segs) {
        acc = acc ? `${acc}/${s}` : s;
        if (paths.some((r) => r.test(acc))) return true;
      }
      return false;
    },
  };
}
