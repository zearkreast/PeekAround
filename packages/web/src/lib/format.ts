import type { Lang } from '@/store/prefs';
import { locale } from '@/i18n';

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
    return new Intl.DateTimeFormat(locale(lang), opts).format(new Date(ms));
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
