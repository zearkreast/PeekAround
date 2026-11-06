import { usePrefs, type Lang } from '@/store/prefs';
import { locale } from '@/i18n';

/** Settings → language: 24/12 h clock override */
function withClock(opts: Intl.DateTimeFormatOptions): Intl.DateTimeFormatOptions {
  const hc = usePrefs.getState().hourCycle;
  if (hc === 'auto' || (!opts.timeStyle && opts.hour === undefined)) return opts;
  return { ...opts, hourCycle: hc };
}

const pad = (n: number) => String(n).padStart(2, '0');

/**
 * Settings → language: ISO dates ("2024-07-05 14:30"). Only for dateStyle / timeStyle formats
 * (headers like "Sat, 5 Jul" stay localized); null = use Intl.
 */
export function isoDate(d: Date, opts: Intl.DateTimeFormatOptions, utc: boolean): string | null {
  const p = usePrefs.getState();
  if (p.dateFormat !== 'iso' || !opts.dateStyle) return null;
  const [y, mo, da, h, mi] = utc
    ? [d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate(), d.getUTCHours(), d.getUTCMinutes()]
    : [d.getFullYear(), d.getMonth(), d.getDate(), d.getHours(), d.getMinutes()];
  if (!Number.isFinite(y)) return null;
  const date = `${y}-${pad(mo + 1)}-${pad(da)}`;
  if (!opts.timeStyle) return date;
  const time = p.hourCycle === 'h12' ? `${((h + 11) % 12) + 1}:${pad(mi)} ${h < 12 ? 'AM' : 'PM'}` : `${pad(h)}:${pad(mi)}`;
  return `${date} ${time}`;
}

export function formatBytes(n: number): string {
  if (!n) return '0 B';
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.min(u.length - 1, Math.floor(Math.log(n) / Math.log(1024)));
  return `${(n / 1024 ** i).toFixed(i === 0 ? 0 : n / 1024 ** i >= 100 ? 0 : 1)} ${u[i]}`;
}

export function formatDuration(s: number | null | undefined): string {
  if (s == null || !Number.isFinite(s)) return '';
  const t = Math.max(0, Math.round(s));
  const h = Math.floor(t / 3600);
  const m = Math.floor((t % 3600) / 60);
  const sec = t % 60;
  return h ? `${h}:${String(m).padStart(2, '0')}:${String(sec).padStart(2, '0')}` : `${m}:${String(sec).padStart(2, '0')}`;
}

export function formatTime(s: number): string {
  if (!Number.isFinite(s)) return '0:00';
  const m = Math.floor(s / 60);
  const sec = Math.floor(s % 60);
  return `${m}:${String(sec).padStart(2, '0')}`;
}

export function formatDate(ms: number, lang: Lang, opts: Intl.DateTimeFormatOptions = { dateStyle: 'medium' }): string {
  try {
    const iso = isoDate(new Date(ms), opts, false);
    if (iso) return iso;
    return new Intl.DateTimeFormat(locale(lang), withClock(opts)).format(new Date(ms));
  } catch {
    return new Date(ms).toLocaleDateString();
  }
}

export function formatMonth(key: string, lang: Lang): string {
  const [y, m] = key.split('-').map(Number);
  return formatDate(new Date(y, (m || 1) - 1, 1).getTime(), lang, { month: 'long', year: 'numeric' });
}

export function relativeTime(ms: number, lang: Lang): string {
  const diff = ms - Date.now();
  const rtf = new Intl.RelativeTimeFormat(locale(lang), { numeric: 'auto' });
  const abs = Math.abs(diff);
  if (abs < 3600_000) return rtf.format(Math.round(diff / 60_000), 'minute');
  if (abs < 86400_000) return rtf.format(Math.round(diff / 3600_000), 'hour');
  return rtf.format(Math.round(diff / 86400_000), 'day');
}

export function durationUntil(ms: number, lang: Lang): string {
  const diff = Math.max(0, ms - Date.now());
  const h = Math.floor(diff / 3600_000);
  const m = Math.floor((diff % 3600_000) / 60_000);
  if (lang === 'tr') return h ? `${h} sa ${m} dk` : `${m} dk`;
  return h ? `${h}h ${m}m` : `${m}m`;
}

const dtfCache = new Map<string, Intl.DateTimeFormat>();
function dtf(lang: Lang, opts: Intl.DateTimeFormatOptions): Intl.DateTimeFormat {
  const o = withClock(opts);
  const k = lang + JSON.stringify(o);
  let f = dtfCache.get(k);
  if (!f) {
    f = new Intl.DateTimeFormat(locale(lang), { ...o, timeZone: 'UTC' });
    dtfCache.set(k, f);
  }
  return f;
}

/** Format a wall-clock-as-UTC timestamp (MediaItem.localAt) — never shifts the day. */
export function formatLocal(localAt: number, lang: Lang, opts: Intl.DateTimeFormatOptions = { dateStyle: 'medium' }): string {
  try {
    return isoDate(new Date(localAt), opts, true) ?? dtf(lang, opts).format(new Date(localAt));
  } catch {
    return new Date(localAt).toISOString().slice(0, 10);
  }
}

/** 'YYYY-MM-DD' (skeleton day key) → "Cmt, 5 Tem" / "Cmt, 5 Tem 2023" */
export function formatDayKey(key: string, lang: Lang): string {
  const [y, m, d] = key.split('-').map(Number);
  const ms = Date.UTC(y, (m || 1) - 1, d || 1);
  const sameYear = y === new Date().getFullYear();
  return formatLocal(ms, lang, sameYear ? { weekday: 'short', day: 'numeric', month: 'short' } : { weekday: 'short', day: 'numeric', month: 'short', year: 'numeric' });
}

/** 'YYYY-MM' or 'YYYY-MM-DD' → "Temmuz 2024" */
export function formatMonthKey(key: string, lang: Lang, short = false): string {
  const [y, m] = key.split('-').map(Number);
  return formatLocal(Date.UTC(y, (m || 1) - 1, 1), lang, { month: short ? 'short' : 'long', year: 'numeric' });
}
