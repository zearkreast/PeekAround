import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { Link, useNavigate } from 'react-router';
import { useQuery } from '@tanstack/react-query';
import {
  CalendarDays,
  Clapperboard,
  Cloud,
  Copy,
  Dices,
  Film,
  Globe2,
  HardDrive,
  Images,
  MapPin,
  Moon,
  RectangleHorizontal,
  RectangleVertical,
  Square,
  Sunset,
  Timer,
  GalleryHorizontalEnd,
} from 'lucide-react';
import {
  calendarDayKey,
  COLOR_SWATCH,
  localDayKey,
  type CityGroup,
  type ColorBucket,
  type CountryGroup,
  type DayTime,
  type DuplicateSummary,
  type ExploreResult,
  type FacetGroup,
  type Memory,
  type Orientation,
  type RankedItem,
  type VideoLength,
} from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useLang, useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { usePrefs, WEEK_START_DAY } from '@/store/prefs';
import { toast } from '@/store/toasts';
import { Button } from '@/components/ui';
import { Thumb } from './cards';
import {
  bytesLabel,
  cityHref,
  COLOR_KEYS,
  colorHref,
  countryHref,
  countryName,
  DAYTIME_KEYS,
  DAYTIME_SUB_KEYS,
  dayHref,
  dayLabel,
  daytimeHref,
  DUPLICATES_HREF,
  exploreQuery,
  flagEmoji,
  LARGEST_HREF,
  memoryLabel,
  numLabel,
  ORIENT_KEYS,
  orientHref,
  playHref,
  RECENT_HREF,
  VLEN_KEYS,
  VLEN_SUB_KEYS,
  vlenHref,
} from './labels';

// ───────────────────────── stats header ─────────────────────────

function StatChip({ icon, children, to, title }: { icon: ReactNode; children: ReactNode; to?: string; title?: string }) {
  const cls =
    'inline-flex h-9 shrink-0 items-center gap-1.5 rounded-full border border-line bg-surface px-3 text-[13px] font-medium tabular-nums transition-colors [&>svg]:size-4 [&>svg]:text-accent';
  return to ? (
    <Link to={to} className={cn(cls, 'hover:bg-surface-2')} title={title}>
      {icon}
      {children}
    </Link>
  ) : (
    <span className={cls} title={title}>
      {icon}
      {children}
    </span>
  );
}

/** photos · videos · countries · cities · year span · total size */
export function StatsHeader({ r }: { r: ExploreResult }) {
  const t = useT();
  const lang = useLang();
  const s = r.stats;
  if (!s || !(s.photos + s.videos)) return null;
  const years =
    s.firstYear && s.lastYear
      ? s.firstYear === s.lastYear
        ? t('insights.stats.year', { year: s.firstYear })
        : t('insights.stats.years', { from: s.firstYear, to: s.lastYear })
      : null;
  return (
    <div className="no-scrollbar -mx-3 mb-7 flex gap-2 overflow-x-auto px-3 sm:-mx-7 sm:flex-wrap sm:px-7" data-stats>
      <StatChip icon={<Images />}>{t('insights.stats.photos', { n: numLabel(s.photos, lang) })}</StatChip>
      {s.videos > 0 && (
        <StatChip icon={<Film />} to="/explore/type/video">
          {t('insights.stats.videos', { n: numLabel(s.videos, lang) })}
        </StatChip>
      )}
      {s.countries > 0 && <StatChip icon={<Globe2 />}>{t('insights.stats.countries', { n: s.countries })}</StatChip>}
      {s.cities > 0 && <StatChip icon={<MapPin />}>{t('insights.stats.cities', { n: s.cities })}</StatChip>}
      {years && <StatChip icon={<CalendarDays />}>{years}</StatChip>}
      <SizeChip bytes={s.bytes} cloud={s.cloudBytes ?? 0} />
    </div>
  );
}

/** total size, split into local (this computer / phone folders) and cloud (MEGA folders) when there is a cloud part */
export function SizeChip({ bytes, cloud }: { bytes: number; cloud: number }) {
  const t = useT();
  const lang = useLang();
  const local = Math.max(0, bytes - cloud);
  if (cloud <= 0) return <StatChip icon={<HardDrive />} title={t('insights.stats.localTip')}>{bytesLabel(bytes, lang)}</StatChip>;
  return (
    <span className="inline-flex h-9 shrink-0 items-center rounded-full border border-line bg-surface text-[13px] font-medium tabular-nums [&_svg]:size-4" data-stat="size">
      <span className="inline-flex h-full items-center gap-1.5 pr-2.5 pl-3" title={t('insights.stats.localTip')} data-stat="local">
        <HardDrive className="text-accent" />
        {t('insights.stats.local', { size: bytesLabel(local, lang) })}
      </span>
      <span className="h-4 w-px bg-line" aria-hidden />
      <span className="inline-flex h-full items-center gap-1.5 pr-3 pl-2.5" title={t('insights.stats.cloudTip')} data-stat="cloud">
        <Cloud className="text-[#e5484d]" />
        {t('insights.stats.cloud', { size: bytesLabel(cloud, lang) })}
      </span>
    </span>
  );
}

// ───────────────────────── cards ─────────────────────────

/** 2×2 collage (or one big cover) with a gradient label — the shared look of insight cards */
export function CollageCard({
  to,
  covers,
  title,
  sub,
  icon,
  className,
  badge,
  data,
}: {
  to: string;
  covers: string[];
  title: ReactNode;
  sub?: ReactNode;
  icon?: ReactNode;
  className?: string;
  badge?: ReactNode;
  data?: Record<string, string>;
}) {
  return (
    <Link
      to={to}
      {...data}
      className={cn('ins-card group relative block aspect-[4/5] w-[40vw] max-w-[190px] shrink-0 snap-start overflow-hidden rounded-2xl bg-[var(--tile-bg)] sm:w-[180px]', className)}
    >
      {covers.length >= 4 ? (
        <div className="absolute inset-0 grid grid-cols-2 grid-rows-2 gap-0.5">
          {covers.slice(0, 4).map((c) => (
            <Thumb key={c} id={c} className="size-full transition-transform duration-500 ease-[var(--ease-out)] group-hover:scale-105" />
          ))}
        </div>
      ) : (
        <Thumb id={covers[0]} size={640} className="absolute inset-0 size-full transition-transform duration-500 ease-[var(--ease-out)] group-hover:scale-105" />
      )}
      <div className="absolute inset-0 bg-gradient-to-t from-black/80 via-black/10 to-transparent" />
      {icon && <span className="absolute top-2 left-2 grid size-7 place-items-center rounded-full bg-black/35 text-white [&>svg]:size-3.5">{icon}</span>}
      {badge && <span className="absolute top-2 right-2">{badge}</span>}
      <div className="absolute inset-x-0 bottom-0 p-3 text-white">
        <div className="font-display text-[16px] leading-tight font-semibold [text-wrap:balance]">{title}</div>
        {sub && <div className="mt-0.5 truncate text-[11.5px] text-white/75">{sub}</div>}
      </div>
    </Link>
  );
}

export function YearReviewCard({ m }: { m: Memory }) {
  const lang = useLang();
  const { title, subtitle } = memoryLabel(m, lang);
  const year = new Date(m.from).getUTCFullYear();
  return (
    <Link
      to={playHref(m.id)}
      data-year-review={year}
      className="ins-card group relative block aspect-[3/4] w-[44vw] max-w-[220px] shrink-0 snap-start overflow-hidden rounded-2xl bg-[var(--tile-bg)] shadow-sm sm:w-[210px]"
    >
      <div className="absolute inset-0 grid grid-cols-3 grid-rows-4 gap-0.5">
        {m.coverIds.slice(0, 12).map((c, i) => (
          <Thumb key={c} id={c} className={cn('size-full', i === 0 && 'col-span-2 row-span-2')} size={i === 0 ? 640 : 320} />
        ))}
      </div>
      <div className="absolute inset-0 bg-gradient-to-t from-black/85 via-black/25 to-black/5" />
      <div className="absolute inset-x-0 bottom-0 p-3 text-white">
        <div className="font-display text-[34px] leading-none font-bold tracking-tight tabular-nums" title={title}>
          {year}
        </div>
        <div className="mt-1 truncate text-[11.5px] text-white/80">{subtitle}</div>
        {m.months && <MonthBars months={m.months} />}
      </div>
    </Link>
  );
}

/** tiny bar chart of items per month */
function MonthBars({ months }: { months: number[] }) {
  const max = Math.max(1, ...months);
  return (
    <div className="mt-2 flex h-5 items-end gap-[3px]" aria-hidden>
      {months.map((n, i) => (
        <span key={i} className="w-full rounded-sm bg-white/80" style={{ height: `${Math.max(n ? 12 : 4, (n / max) * 100)}%`, opacity: n ? 0.9 : 0.25 }} />
      ))}
    </div>
  );
}

export function CountryCard({ c }: { c: CountryGroup }) {
  const lang = useLang();
  const t = useT();
  return (
    <Link to={countryHref(c.cc)} data-country={c.cc} className="ins-card group block w-[36vw] max-w-[170px] shrink-0 snap-start sm:w-[168px]">
      <div className="relative aspect-square overflow-hidden rounded-2xl bg-[var(--tile-bg)]">
        <Thumb id={c.coverId} className="size-full transition-transform duration-500 ease-[var(--ease-out)] group-hover:scale-105" />
        <span className="absolute top-2 left-2 grid size-9 place-items-center rounded-full bg-black/35 text-xl leading-none backdrop-blur-sm" aria-hidden>
          {flagEmoji(c.cc)}
        </span>
      </div>
      <div className="mt-2 truncate text-sm font-medium">{countryName(c.cc, lang)}</div>
      <div className="truncate text-xs text-dim">
        {t('explore.count', { n: c.count })} · {t('insights.countrySub', { regions: c.regions, cities: c.cities })}
      </div>
    </Link>
  );
}

export function CityCard({ c }: { c: CityGroup }) {
  const t = useT();
  return (
    <Link to={cityHref(c.key)} data-city={c.key} className="ins-card group block w-[32vw] max-w-[150px] shrink-0 snap-start sm:w-[148px]">
      <div className="relative aspect-square overflow-hidden rounded-2xl bg-[var(--tile-bg)]">
        <Thumb id={c.coverId} className="size-full transition-transform duration-500 ease-[var(--ease-out)] group-hover:scale-105" />
      </div>
      <div className="mt-2 truncate text-sm font-medium">{c.name}</div>
      <div className="truncate text-xs text-dim">{t('explore.count', { n: c.count })}</div>
    </Link>
  );
}

export function DayTimeCard({ g }: { g: FacetGroup<DayTime> }) {
  const t = useT();
  return (
    <CollageCard
      to={daytimeHref(g.value)}
      covers={g.coverIds ?? [g.coverId]}
      title={t(DAYTIME_KEYS[g.value])}
      sub={`${t(DAYTIME_SUB_KEYS[g.value])} · ${g.count}`}
      icon={g.value === 'night' ? <Moon /> : <Sunset />}
      className="aspect-[5/4] w-[58vw] max-w-[280px] sm:w-[260px]"
      data={{ 'data-daytime': g.value }}
    />
  );
}

export function ColorChip({ g }: { g: FacetGroup<ColorBucket> }) {
  const t = useT();
  const sw = COLOR_SWATCH[g.value];
  return (
    <Link
      to={colorHref(g.value)}
      data-color={g.value}
      className="ins-card group relative block h-[104px] w-[38vw] max-w-[176px] shrink-0 snap-start overflow-hidden rounded-2xl sm:w-[168px]"
      style={{ background: g.value === 'bw' ? 'linear-gradient(135deg,#f1f1f4,#2a2a33)' : sw }}
    >
      <div className="absolute inset-y-2 right-2 flex gap-1">
        {(g.coverIds ?? [g.coverId]).slice(0, 2).map((c, i) => (
          <span key={c} className={cn('w-12 overflow-hidden rounded-xl shadow-lg ring-2 ring-white/40 transition-transform duration-500 ease-[var(--ease-out)]', i === 0 ? 'group-hover:-rotate-3' : 'group-hover:rotate-3')}>
            <Thumb id={c} className="size-full" />
          </span>
        ))}
      </div>
      <div className="absolute bottom-2.5 left-3 text-white drop-shadow-[0_1px_6px_rgb(0_0_0/0.45)]">
        <div className="text-sm font-semibold">{t(COLOR_KEYS[g.value])}</div>
        <div className="text-[11px] text-white/85 tabular-nums">{g.count}</div>
      </div>
    </Link>
  );
}

const ORIENT_ICONS: Record<Orientation, ReactNode> = {
  portrait: <RectangleVertical />,
  landscape: <RectangleHorizontal />,
  square: <Square />,
  panorama: <GalleryHorizontalEnd />,
};

export function ShapeTile({ g, kind }: { g: FacetGroup<Orientation> | FacetGroup<VideoLength>; kind: 'orient' | 'vlen' }) {
  const t = useT();
  const label = kind === 'orient' ? t(ORIENT_KEYS[g.value as Orientation]) : t(VLEN_KEYS[g.value as VideoLength]);
  const sub = kind === 'vlen' ? t(VLEN_SUB_KEYS[g.value as VideoLength]) : null;
  const to = kind === 'orient' ? orientHref(g.value as Orientation) : vlenHref(g.value as VideoLength);
  const icon = kind === 'orient' ? ORIENT_ICONS[g.value as Orientation] : g.value === 'short' ? <Timer /> : <Clapperboard />;
  return (
    <Link to={to} data-shape={`${kind}:${g.value}`} className="ins-card group relative block aspect-[4/3] overflow-hidden rounded-2xl bg-[var(--tile-bg)] sm:aspect-square">
      <Thumb id={g.coverId} className="absolute inset-0 size-full transition-transform duration-500 ease-[var(--ease-out)] group-hover:scale-105" />
      <div className="absolute inset-0 bg-gradient-to-t from-black/75 via-black/10 to-transparent" />
      <div className="absolute inset-x-0 bottom-0 flex items-center gap-2 p-2.5 text-white">
        <span className="grid size-7 shrink-0 place-items-center rounded-full bg-white/20 [&>svg]:size-3.5">{icon}</span>
        <div className="min-w-0">
          <div className="truncate text-[13px] leading-tight font-semibold">{label}</div>
          <div className="truncate text-[11px] text-white/70 tabular-nums">{sub ? `${sub} · ${g.count}` : g.count}</div>
        </div>
      </div>
    </Link>
  );
}

/** largest / recently added: a strip of thumbnails with a value label */
export function RankedStrip({ list, kind }: { list: RankedItem[]; kind: 'largest' | 'recent' }) {
  const lang = useLang();
  const href = kind === 'largest' ? LARGEST_HREF : RECENT_HREF;
  const fmtDay = useMemo(() => new Intl.DateTimeFormat(lang === 'tr' ? 'tr-TR' : 'en-US', { day: 'numeric', month: 'short' }), [lang]);
  return (
    <>
      {list.slice(0, 16).map((it) => (
        <Link key={it.id} to={`${href}?view=${encodeURIComponent(it.id)}`} data-ranked={it.id} className="ins-card group relative block aspect-square w-[30vw] max-w-[150px] shrink-0 snap-start overflow-hidden rounded-2xl bg-[var(--tile-bg)] sm:w-[144px]">
          <Thumb id={it.id} className="size-full transition-transform duration-500 ease-[var(--ease-out)] group-hover:scale-105" />
          <span className="absolute bottom-1.5 left-1.5 rounded-full bg-black/55 px-2 py-0.5 text-[11px] font-medium text-white tabular-nums backdrop-blur-sm">
            {kind === 'largest' ? bytesLabel(it.value, lang) : fmtDay.format(new Date(it.value))}
          </span>
        </Link>
      ))}
    </>
  );
}

export function DuplicatesCard({ d }: { d: DuplicateSummary }) {
  const t = useT();
  const lang = useLang();
  return (
    <Link
      to={DUPLICATES_HREF}
      data-duplicates={d.groups}
      className="ins-card group flex items-center gap-4 rounded-2xl border border-line bg-surface p-3 pr-5 transition-colors hover:bg-surface-2 sm:max-w-xl"
    >
      <div className="relative size-20 shrink-0">
        {d.coverIds.slice(0, 2).map((c, i) => (
          <span
            key={c}
            className={cn(
              'absolute size-16 overflow-hidden rounded-xl shadow-lg ring-2 ring-[var(--bg-elev)] transition-transform duration-500 ease-[var(--ease-out)]',
              i === 0 ? 'top-0 left-0 group-hover:-translate-x-1 group-hover:-rotate-6' : 'right-0 bottom-0 group-hover:translate-x-1 group-hover:rotate-6',
            )}
          >
            <Thumb id={c} className="size-full" />
          </span>
        ))}
        {!d.coverIds.length && (
          <span className="grid size-16 place-items-center rounded-xl bg-surface-2 text-faint">
            <Copy className="size-6" />
          </span>
        )}
      </div>
      <div className="min-w-0 flex-1">
        <div className="text-[15px] font-semibold">{t('insights.dup.title')}</div>
        {d.groups > 0 ? (
          <>
            <div className="text-[13px] text-dim tabular-nums">{t('insights.dup.summary', { groups: d.groups, files: d.files })}</div>
            <div className="text-[13px] font-medium text-accent tabular-nums">{t('insights.dup.wasted', { size: bytesLabel(d.wasted, lang) })}</div>
          </>
        ) : (
          <div className="text-[13px] text-dim">{d.pending ? t('insights.dup.pending', { n: d.pending }) : t('insights.dup.none')}</div>
        )}
        {d.groups > 0 && d.pending > 0 && <div className="mt-0.5 text-[11.5px] text-faint">{t('insights.dup.pending', { n: d.pending })}</div>}
      </div>
    </Link>
  );
}

// ───────────────────────── calendar heatmap ─────────────────────────

const LEVELS = 4;

/** quantile-ish level of a day count (0 = empty) */
export function heatLevel(n: number, max: number): number {
  if (!n || !max) return 0;
  return Math.min(LEVELS, Math.max(1, Math.ceil((Math.log1p(n) / Math.log1p(max)) * LEVELS)));
}

/** newest year that holds a good share of the items (a few stray files in a new year don't win) */
export function defaultCalendarYear(years: { year: number; count: number }[]): number | undefined {
  const max = Math.max(0, ...years.map((y) => y.count));
  return years.find((y) => y.count >= Math.max(1, max * 0.25))?.year ?? years[0]?.year;
}

export function CalendarHeatmap({ years, counts }: { years: number[]; counts?: { year: number; count: number }[] }) {
  const t = useT();
  const lang = useLang();
  const backend = useBackend();
  const nav = useNavigate();
  const initial = (counts && defaultCalendarYear(counts.filter((c) => years.includes(c.year)))) ?? years[0];
  const [year, setYear] = useState(initial);
  useEffect(() => {
    if (!years.includes(year)) setYear(initial);
  }, [years, year, initial]);
  const q = useQuery({
    queryKey: ['explore', backend.id, 'calendar', year],
    queryFn: () => backend.calendar!(year),
    enabled: !!backend.calendar && !!year,
    staleTime: 60_000,
  });
  const cal = q.data;
  // week columns start on Settings → first day of the week (Monday by default)
  const weekStart = usePrefs((s) => WEEK_START_DAY[s.weekStart] ?? 1);
  const lead = (new Date(Date.UTC(year, 0, 1)).getUTCDay() - weekStart + 7) % 7;
  const monthFmt = useMemo(() => new Intl.DateTimeFormat(lang === 'tr' ? 'tr-TR' : 'en-US', { month: 'short', timeZone: 'UTC' }), [lang]);
  const months = useMemo(
    () =>
      Array.from({ length: 12 }, (_, m) => {
        const d = Math.round((Date.UTC(year, m, 1) - Date.UTC(year, 0, 1)) / 86_400_000);
        return { label: monthFmt.format(new Date(Date.UTC(year, m, 1))), col: Math.floor((d + lead) / 7) };
      }),
    [year, lead, monthFmt],
  );
  if (!backend.calendar || !years.length) return null;
  const n = cal?.days.length ?? (new Date(Date.UTC(year, 1, 29)).getUTCMonth() === 1 ? 366 : 365);
  return (
    <div className="rounded-2xl border border-line bg-surface p-3 sm:p-4" data-calendar={year}>
      <div className="mb-3 flex flex-wrap items-center gap-2">
        <div className="no-scrollbar flex gap-1 overflow-x-auto">
          {years.slice(0, 30).map((y) => (
            <button
              key={y}
              type="button"
              onClick={() => setYear(y)}
              aria-pressed={y === year}
              className={cn('h-8 shrink-0 rounded-lg px-2.5 text-[13px] font-medium tabular-nums transition-colors', y === year ? 'bg-accent text-white' : 'text-dim hover:bg-surface-2 hover:text-fg')}
            >
              {y}
            </button>
          ))}
        </div>
        {cal && <span className="ml-auto text-xs text-faint tabular-nums">{t('insights.calendar.total', { year, n: cal.total })}</span>}
      </div>
      <div className="no-scrollbar overflow-x-auto pb-1">
        <div className="cal-wrap inline-block">
          <div className="relative mb-1 h-4 text-[10.5px] text-faint" style={{ width: `calc(${Math.ceil((n + lead) / 7)} * (var(--cal-cell) + 3px) - 3px)` }}>
            {months.map((m) => (
              <span key={m.label + m.col} className="absolute" style={{ left: `calc(${m.col} * (var(--cal-cell) + 3px))` }}>
                {m.label}
              </span>
            ))}
          </div>
          <div className={cn('cal-grid', !cal && 'opacity-60')}>
            {Array.from({ length: lead }, (_, i) => (
              <span key={`l${i}`} />
            ))}
            {Array.from({ length: n }, (_, d) => {
              const c = cal?.days[d] ?? 0;
              const key = calendarDayKey(year, d);
              const label = c ? t('insights.calendar.day', { date: dayLabel(key, lang), n: c }) : t('insights.calendar.dayEmpty', { date: dayLabel(key, lang) });
              return c ? (
                <button key={d} type="button" className="cal-day" data-l={heatLevel(c, cal!.max)} title={label} aria-label={label} onClick={() => nav(dayHref(key))} />
              ) : (
                <span key={d} className={cn('cal-day', !cal && 'shimmer')} title={label} />
              );
            })}
          </div>
        </div>
      </div>
      <div className="mt-2 flex items-center justify-end gap-1.5 text-[10.5px] text-faint">
        {t('insights.calendar.less')}
        {[0, 1, 2, 3, 4].map((l) => (
          <span key={l} className="cal-day inline-block size-[11px]" data-l={l} />
        ))}
        {t('insights.calendar.more')}
      </div>
    </div>
  );
}

// ───────────────────────── surprise me ─────────────────────────

export function SurpriseButton({ r }: { r: ExploreResult }) {
  const t = useT();
  const nav = useNavigate();
  const backend = useBackend();
  const [busy, setBusy] = useState(false);
  const total = (r.stats?.photos ?? 0) + (r.stats?.videos ?? 0) || r.years.reduce((a, y) => a + y.count, 0);
  const memories = [...r.memories, ...(r.yearReviews ?? [])];
  if (!total && !memories.length) return null;
  const go = async () => {
    setBusy(true);
    try {
      // half the time a memory (story player), otherwise one random photo on its day
      if (memories.length && (Math.random() < 0.5 || !total)) {
        nav(playHref(memories[Math.floor(Math.random() * memories.length)].id));
        return;
      }
      const res = await backend.browse({ ...exploreQuery({}), offset: Math.floor(Math.random() * total), limit: 1 });
      const it = res.items[0];
      if (!it) return toast.info(t('insights.surpriseNone'));
      nav(`${dayHref(localDayKey(it.localAt))}?view=${encodeURIComponent(it.id)}`);
    } catch (e) {
      toast.error(t('common.error'), (e as Error).message);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Button variant="secondary" size="sm" loading={busy} icon={<Dices className="size-4" />} onClick={() => void go()} data-action="surprise">
      {t('insights.surprise')}
    </Button>
  );
}

// ───────────────────────── loading skeletons ─────────────────────────

export function SkeletonRow({ n = 6, className }: { n?: number; className?: string }) {
  return (
    <div className="no-scrollbar -mx-3 flex gap-3 overflow-hidden px-3 sm:-mx-7 sm:px-7" aria-hidden>
      {Array.from({ length: n }, (_, i) => (
        <div key={i} className={cn('shimmer aspect-[4/5] w-[40vw] max-w-[190px] shrink-0 rounded-2xl sm:w-[180px]', className)} />
      ))}
    </div>
  );
}

export function ExploreSkeleton() {
  const motion = usePrefs((s) => s.motion);
  return (
    <div data-skeleton className={motion === 'off' ? '[&_.shimmer]:animate-none' : undefined}>
      <div className="mb-7 flex gap-2">
        {[88, 70, 64, 72, 90, 60].map((w, i) => (
          <div key={i} className="shimmer h-9 rounded-full" style={{ width: w }} />
        ))}
      </div>
      {[0, 1, 2].map((k) => (
        <div key={k} className="mb-9">
          <div className="shimmer mb-3 h-6 w-40 rounded-lg" />
          <SkeletonRow className={k === 1 ? 'aspect-square' : undefined} />
        </div>
      ))}
    </div>
  );
}

