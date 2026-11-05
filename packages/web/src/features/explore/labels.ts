/**
 * Explore labels — pure helpers (no React) so they can be unit tested.
 * Dates are localAt values (wall clock encoded as UTC) → always formatted with timeZone 'UTC'.
 */
import { DAY_MS, type BrowseQuery, type MediaFlagName, type Memory, type PlaceGroup } from '@peekaround/shared';
import { locale, translate, type MessageKey } from '@/i18n';
import type { Lang } from '@/store/prefs';

const regionNames = new Map<string, Intl.DisplayNames | null>();

/** ISO 3166-1 alpha-2 → localized country name ("TR" → "Türkiye"); falls back to the code. */
export function countryName(cc: string | null | undefined, lang: Lang): string {
  if (!cc) return '';
  let dn = regionNames.get(lang);
  if (dn === undefined) {
    try {
      dn = new Intl.DisplayNames([locale(lang)], { type: 'region' });
    } catch {
      dn = null;
    }
    regionNames.set(lang, dn);
  }
  try {
    return dn?.of(cc.toUpperCase()) ?? cc;
  } catch {
    return cc;
  }
}

/** regional indicator emoji for a country code */
export function flagEmoji(cc: string | null | undefined): string {
  if (!cc || !/^[A-Za-z]{2}$/.test(cc)) return '';
  return String.fromCodePoint(...[...cc.toUpperCase()].map((c) => 0x1f1e6 + c.charCodeAt(0) - 65));
}

const fmts = new Map<string, Intl.DateTimeFormat>();
function fmt(lang: Lang, opts: Intl.DateTimeFormatOptions): Intl.DateTimeFormat {
  const k = lang + JSON.stringify(opts);
  let f = fmts.get(k);
  if (!f) {
    f = new Intl.DateTimeFormat(locale(lang), { ...opts, timeZone: 'UTC' });
    fmts.set(k, f);
  }
  return f;
}

/** "Ağu 2024" for a localAt */
export function monthYear(localAt: number, lang: Lang, month: 'short' | 'long' = 'short'): string {
  return fmt(lang, { month, year: 'numeric' }).format(new Date(localAt));
}

/**
 * Human date range for a localAt range [from, to): "2–6 Tem 2024", "30 Haz – 2 Tem 2024",
 * single day → "5 Kas 2024".
 */
export function dateRange(from: number, to: number, lang: Lang): string {
  const last = Math.max(from, to - 1);
  const f = fmt(lang, { day: 'numeric', month: 'short', year: 'numeric' });
  if (Math.floor(from / DAY_MS) === Math.floor(last / DAY_MS)) return f.format(new Date(from));
  try {
    return f.formatRange(new Date(from), new Date(last));
  } catch {
    return `${f.format(new Date(from))} – ${f.format(new Date(last))}`;
  }
}

/** number of calendar days a [from, to) range touches */
export function spanDays(from: number, to: number): number {
  return Math.max(1, Math.round((Math.ceil(to / DAY_MS) * DAY_MS - Math.floor(from / DAY_MS) * DAY_MS) / DAY_MS));
}

export interface MemoryLabel {
  title: string;
  subtitle: string;
}

/** "2 yıl önce bugün" · "5 Kas 2023"  /  "Antalya gezisi" · "Türkiye · 2–6 Tem 2024" */
export function memoryLabel(m: Memory, lang: Lang): MemoryLabel {
  const t = (k: MessageKey, v?: Record<string, string | number>) => translate(lang, k, v);
  if (m.kind === 'onthisday') {
    // the window is ±3 days around the anniversary → its middle day is the actual date
    const mid = m.from + Math.floor((m.to - m.from) / 2 / DAY_MS) * DAY_MS;
    return { title: t('explore.memory.onthisday', { n: m.yearsAgo ?? 1 }), subtitle: fmt(lang, { day: 'numeric', month: 'long', year: 'numeric' }).format(new Date(mid)) };
  }
  const title = m.place ? t('explore.memory.trip', { place: m.place }) : t('explore.memory.tripNoPlace');
  const country = m.cc ? countryName(m.cc, lang) : '';
  const range = dateRange(m.from, m.to, lang);
  return { title, subtitle: [country && country !== m.place ? country : '', range].filter(Boolean).join(' · ') };
}

/** short title for small cards: "1 yıl önce" / "Antalya · Ağu 2024" */
export function memoryShort(m: Memory, lang: Lang): string {
  if (m.kind === 'onthisday') return translate(lang, 'explore.memory.onthisdayShort', { n: m.yearsAgo ?? 1 });
  return [m.place, monthYear(m.from, lang)].filter(Boolean).join(' · ');
}

export const TYPE_ORDER: MediaFlagName[] = ['video', 'selfie', 'screenshot', 'panorama', 'animated'];

export const TYPE_KEYS: Record<MediaFlagName, MessageKey> = {
  video: 'explore.type.video',
  screenshot: 'explore.type.screenshot',
  selfie: 'explore.type.selfie',
  panorama: 'explore.type.panorama',
  animated: 'explore.type.animated',
};

export function isFlagName(s: string | undefined): s is MediaFlagName {
  return !!s && s in TYPE_KEYS;
}

/** "İstanbul" + "Türkiye" */
export function placeLabel(p: Pick<PlaceGroup, 'name' | 'cc'>, lang: Lang): { name: string; country: string } {
  return { name: p.name, country: countryName(p.cc, lang) };
}

/** camera display name: "Nikon D750", "iPhone 13" (make omitted when the model already says it) */
export function cameraLabel(make: string | null | undefined, model: string | null | undefined): string {
  const md = (model ?? '').trim();
  const mk = (make ?? '').trim();
  if (!md) return mk;
  if (!mk || md.toLowerCase().startsWith(mk.toLowerCase())) return md;
  if (mk === 'Apple' && /^iphone|^ipad/i.test(md)) return md;
  return `${mk} ${md}`;
}

/** Explore result pages: the shared base query (images + videos, newest first, all folders). */
export function exploreQuery(extra: Partial<BrowseQuery>): BrowseQuery {
  // 'image-video' is understood by the server (Explore never lists audio); typed loosely on purpose
  return { recursive: true, sort: 'takenAt', order: 'desc', kind: 'image-video' as BrowseQuery['kind'], ...extra };
}

/** localAt range of a calendar year */
export function yearRange(year: number): { from: number; to: number } {
  return { from: Date.UTC(year, 0, 1), to: Date.UTC(year + 1, 0, 1) };
}

export const cameraHref = (key: string) => `/explore/camera/${encodeURIComponent(key)}`;
export const placeHref = (key: string) => `/explore/place/${encodeURIComponent(key)}`;
export const memoryHref = (id: string) => `/explore/memory/${encodeURIComponent(id)}`;
export const playHref = (id: string) => `/explore/memory/${encodeURIComponent(id)}/play`;
