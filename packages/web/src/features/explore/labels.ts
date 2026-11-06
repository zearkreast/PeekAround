/**
 * Explore labels — pure helpers (no React) so they can be unit tested.
 * Dates are localAt values (wall clock encoded as UTC) → always formatted with timeZone 'UTC'.
 */
import { DAY_MS, type BrowseQuery, type ColorBucket, type DayTime, type MediaFlagName, type Memory, type Orientation, type PlaceGroup, type VideoLength } from '@peekaround/shared';
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
  if (m.kind === 'year') {
    const year = new Date(m.from).getUTCFullYear();
    const months = m.months ? m.months.filter((n) => n > 0).length : 0;
    return {
      title: t('insights.yearReviewTitle', { year }),
      subtitle: m.count ? (months ? t('insights.yearReviewSub', { n: m.count, months }) : t('explore.count', { n: m.count })) : String(year),
    };
  }
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
  if (m.kind === 'year') return String(new Date(m.from).getUTCFullYear());
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

// ───────────────────────── insights ─────────────────────────

export const COLOR_KEYS: Record<ColorBucket, MessageKey> = {
  red: 'insights.color.red',
  orange: 'insights.color.orange',
  yellow: 'insights.color.yellow',
  green: 'insights.color.green',
  blue: 'insights.color.blue',
  purple: 'insights.color.purple',
  pink: 'insights.color.pink',
  bw: 'insights.color.bw',
};
export const ORIENT_KEYS: Record<Orientation, MessageKey> = {
  portrait: 'insights.orient.portrait',
  landscape: 'insights.orient.landscape',
  square: 'insights.orient.square',
  panorama: 'insights.orient.panorama',
};
export const VLEN_KEYS: Record<VideoLength, MessageKey> = { short: 'insights.vlen.short', long: 'insights.vlen.long' };
export const VLEN_SUB_KEYS: Record<VideoLength, MessageKey> = { short: 'insights.vlen.shortSub', long: 'insights.vlen.longSub' };
export const DAYTIME_KEYS: Record<DayTime, MessageKey> = { night: 'insights.daytime.night', golden: 'insights.daytime.golden' };
export const DAYTIME_SUB_KEYS: Record<DayTime, MessageKey> = { night: 'insights.daytime.nightSub', golden: 'insights.daytime.goldenSub' };

export const countryHref = (cc: string) => `/explore/country/${encodeURIComponent(cc.toUpperCase())}`;
export const cityHref = (key: string) => `/explore/city/${encodeURIComponent(key)}`;
export const colorHref = (c: ColorBucket) => `/explore/color/${c}`;
export const orientHref = (o: Orientation) => `/explore/shape/${o}`;
export const vlenHref = (v: VideoLength) => `/explore/length/${v}`;
export const daytimeHref = (d: DayTime) => `/explore/time/${d}`;
export const dayHref = (key: string) => `/explore/day/${key}`;
export const NO_GPS_HREF = '/explore/no-location';
export const LARGEST_HREF = '/explore/largest';
export const RECENT_HREF = '/explore/recent';
export const DUPLICATES_HREF = '/explore/duplicates';

/** "/explore/map?focus=<id>&lat=..&lon=.." — the map centred on one item */
export const mapFocusHref = (id: string, lat: number, lon: number) => `/explore/map?focus=${encodeURIComponent(id)}&lat=${lat.toFixed(6)}&lon=${lon.toFixed(6)}`;

/** "2024-03-05" → localAt range of that day */
export function dayRange(key: string): { from: number; to: number } | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(key);
  if (!m) return null;
  const from = Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
  if (!Number.isFinite(from) || new Date(from).toISOString().slice(0, 10) !== key) return null;
  return { from, to: from + DAY_MS };
}

/** "5 Mart 2024" for a 'YYYY-MM-DD' key */
export function dayLabel(key: string, lang: Lang, weekday = false): string {
  const r = dayRange(key);
  if (!r) return key;
  return fmt(lang, { day: 'numeric', month: 'long', year: 'numeric', ...(weekday ? { weekday: 'long' } : {}) }).format(new Date(r.from));
}

/** "12,3 GB" — locale-aware bytes */
export function bytesLabel(n: number, lang: Lang): string {
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let v = n;
  let u = 0;
  while (v >= 1024 && u < units.length - 1) {
    v /= 1024;
    u++;
  }
  return `${new Intl.NumberFormat(locale(lang), { maximumFractionDigits: v >= 100 || u === 0 ? 0 : 1 }).format(v)} ${units[u]}`;
}

/** "1.234" — locale-aware integer */
export function numLabel(n: number, lang: Lang): string {
  return new Intl.NumberFormat(locale(lang)).format(n);
}
