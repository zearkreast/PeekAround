/**
 * URL hygiene for values that end up in src/href attributes.
 * Server base URLs come from storage / user input and media ids from the address bar,
 * so they are validated before being turned into URLs.
 */

/** '' (same origin) or an http(s) origin + path, without trailing slashes. Throws on anything else. */
export function normalizeBase(base: string): string {
  const raw = String(base ?? '').trim();
  if (!raw) return '';
  let u: URL;
  try {
    u = new URL(raw);
  } catch {
    throw new Error('Invalid server address');
  }
  if (u.protocol !== 'http:' && u.protocol !== 'https:') throw new Error('Server address must start with http:// or https://');
  if (u.username || u.password) throw new Error('Server address must not contain credentials');
  let path = u.pathname;
  let end = path.length;
  while (end > 0 && path[end - 1] === '/') end--;
  path = path.slice(0, end);
  return `${u.origin}${path}`;
}

/** Only same-origin relative paths, http(s) and blob: URLs may be used as link targets. */
export function safeHref(url: string): string {
  const s = String(url ?? '');
  if (s.startsWith('/') && !s.startsWith('//')) return s;
  try {
    const u = new URL(s, location.href);
    if (u.protocol === 'http:' || u.protocol === 'https:' || u.protocol === 'blob:') return u.href;
  } catch {
    /* fall through */
  }
  return 'about:blank';
}

/** a media / folder id as a single URL path segment */
export function seg(id: string | number): string {
  return encodeURIComponent(String(id));
}
