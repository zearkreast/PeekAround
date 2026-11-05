import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { Link } from 'react-router';
import { motion } from 'motion/react';
import { ChevronLeft, ChevronRight, Film, GalleryHorizontalEnd, Heart, ImagePlay, Map as MapIcon, ScanFace, Smartphone, Sparkles } from 'lucide-react';
import type { CameraGroup, ExploreResult, MediaFlagName, Memory, PlaceGroup, TypeGroup, YearGroup } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useLang, useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { usePrefs } from '@/store/prefs';
import { cameraHref, cameraLabel, countryName, memoryLabel, placeHref, playHref, TYPE_KEYS } from './labels';

/** entrance animation props honouring the motion preference */
export function useEnter(delay = 0) {
  const motionPref = usePrefs((s) => s.motion);
  if (motionPref === 'off') return {};
  const reduced = motionPref === 'reduced';
  return {
    initial: { opacity: 0, y: reduced ? 0 : 12 },
    animate: { opacity: 1, y: 0 },
    transition: { duration: reduced ? 0.15 : 0.35, ease: [0.22, 1, 0.36, 1] as const, delay: reduced ? 0 : delay },
  };
}

export function Thumb({ id, size = 320, className, alt = '' }: { id: string | undefined; size?: number; className?: string; alt?: string }) {
  const backend = useBackend();
  const [ok, setOk] = useState(false);
  if (!id) return <div className={cn('bg-[var(--tile-bg)]', className)} />;
  return (
    <img
      src={backend.thumbUrl(id, size)}
      alt={alt}
      loading="lazy"
      decoding="async"
      draggable={false}
      onLoad={() => setOk(true)}
      className={cn('bg-[var(--tile-bg)] object-cover transition-opacity duration-300', ok ? 'opacity-100' : 'opacity-0', className)}
    />
  );
}

export function Section({ title, count, actions, children, delay = 0, id }: { title: ReactNode; count?: number; actions?: ReactNode; children: ReactNode; delay?: number; id?: string }) {
  const enter = useEnter(delay);
  return (
    <motion.section {...enter} className="mb-9" data-section={id}>
      <div className="mb-3 flex items-center gap-2">
        <h2 className="font-display text-lg font-semibold sm:text-xl">{title}</h2>
        {count !== undefined && <span className="text-xs text-faint tabular-nums">{count}</span>}
        <div className="ml-auto flex items-center gap-1">{actions}</div>
      </div>
      {children}
    </motion.section>
  );
}

export function SectionLink({ to, children, icon, label }: { to: string; children: ReactNode; icon?: ReactNode; /** accessible name when the text is hidden on phones */ label?: string }) {
  return (
    <Link to={to} aria-label={label} className="touch-target inline-flex h-9 items-center justify-center gap-1.5 rounded-lg px-2.5 text-[13px] font-medium text-accent hover:bg-surface-2 [&>svg]:size-4">
      {icon}
      {children}
    </Link>
  );
}

/** horizontal scroller with snap; arrow buttons on pointer devices */
export function Carousel({ children, className }: { children: ReactNode; className?: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const [edges, setEdges] = useState({ left: false, right: false });
  const update = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    setEdges({ left: el.scrollLeft > 4, right: el.scrollLeft + el.clientWidth < el.scrollWidth - 4 });
  }, []);
  useEffect(() => {
    update();
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, [update, children]);
  const by = (d: 1 | -1) => ref.current?.scrollBy({ left: d * ref.current.clientWidth * 0.8, behavior: 'smooth' });
  return (
    <div className="group/car relative">
      <div ref={ref} onScroll={update} className={cn('no-scrollbar -mx-3 flex snap-x snap-mandatory gap-3 overflow-x-auto scroll-px-3 px-3 pb-1 sm:-mx-7 sm:scroll-px-7 sm:px-7', className)}>
        {children}
      </div>
      {edges.left && (
        <button type="button" aria-label="‹" onClick={() => by(-1)} className="carousel-arrow left-0 sm:-left-3">
          <ChevronLeft className="size-5" />
        </button>
      )}
      {edges.right && (
        <button type="button" aria-label="›" onClick={() => by(1)} className="carousel-arrow right-0 sm:-right-3">
          <ChevronRight className="size-5" />
        </button>
      )}
    </div>
  );
}

// ───────────────────────── Memories ─────────────────────────

export function MemoryCard({ m, className }: { m: Memory; className?: string }) {
  const lang = useLang();
  const { title, subtitle } = memoryLabel(m, lang);
  const c = m.coverIds;
  return (
    <Link
      to={playHref(m.id)}
      data-memory={m.id}
      className={cn(
        'group relative block aspect-[3/4] w-[42vw] max-w-[210px] shrink-0 snap-start overflow-hidden rounded-2xl bg-[var(--tile-bg)] shadow-sm sm:w-[200px]',
        className,
      )}
    >
      {c.length >= 3 ? (
        <div className="absolute inset-0 grid grid-cols-2 grid-rows-3 gap-0.5">
          <Thumb id={c[0]} size={640} className="col-span-2 row-span-2 size-full" />
          <Thumb id={c[1]} className="size-full" />
          <Thumb id={c[2]} className="size-full" />
        </div>
      ) : (
        <Thumb id={c[0]} size={640} className="absolute inset-0 size-full" />
      )}
      <div className="absolute inset-0 bg-gradient-to-t from-black/80 via-black/15 to-transparent transition-opacity group-hover:opacity-90" />
      <div className="absolute inset-x-0 bottom-0 p-3 text-white">
        <div className="font-display text-[17px] leading-tight font-semibold [text-wrap:balance]">{title}</div>
        <div className="mt-1 truncate text-[11.5px] text-white/75">{subtitle}</div>
      </div>
      <span className="absolute top-2 left-2 grid size-7 place-items-center rounded-full bg-black/35 text-white">
        <Sparkles className="size-3.5" />
      </span>
    </Link>
  );
}

// ───────────────────────── Places ─────────────────────────

export function PlaceCard({ p, className, wide }: { p: PlaceGroup; className?: string; wide?: boolean }) {
  const lang = useLang();
  const t = useT();
  return (
    <Link to={placeHref(p.key)} data-place={p.key} className={cn('group block shrink-0 snap-start', wide ? 'w-full' : 'w-[36vw] max-w-[170px] sm:w-[168px]', className)}>
      <div className="relative aspect-square overflow-hidden rounded-2xl bg-[var(--tile-bg)]">
        <Thumb id={p.coverId} className="size-full transition-transform duration-500 ease-[var(--ease-out)] group-hover:scale-105" />
      </div>
      <div className="mt-2 truncate text-sm font-medium">{p.name}</div>
      <div className="truncate text-xs text-dim">
        {countryName(p.cc, lang)} · {t('explore.count', { n: p.count })}
      </div>
    </Link>
  );
}

/** decorative "mini map": every place as a dot, projected into the places' bounding box */
export function MapCard({ places, className, wide }: { places: PlaceGroup[]; className?: string; wide?: boolean }) {
  const t = useT();
  const pts = places.slice(0, 200);
  let [minLat, maxLat, minLon, maxLon] = [90, -90, 180, -180];
  for (const p of pts) {
    minLat = Math.min(minLat, p.lat);
    maxLat = Math.max(maxLat, p.lat);
    minLon = Math.min(minLon, p.lon);
    maxLon = Math.max(maxLon, p.lon);
  }
  const padLon = Math.max(2, (maxLon - minLon) * 0.18);
  const padLat = Math.max(2, (maxLat - minLat) * 0.18);
  const w = maxLon - minLon + padLon * 2;
  const h = maxLat - minLat + padLat * 2;
  const max = Math.max(1, ...pts.map((p) => p.count));
  return (
    <Link to="/explore/map" data-map-card className={cn('group block shrink-0 snap-start', wide ? 'w-full' : 'w-[36vw] max-w-[170px] sm:w-[168px]', className)}>
      <div className="relative aspect-square overflow-hidden rounded-2xl border border-line bg-[radial-gradient(120%_90%_at_30%_20%,color-mix(in_oklab,var(--accent)_22%,var(--bg-elev-2)),var(--bg-elev-2))]">
        <svg viewBox="0 0 100 100" className="absolute inset-0 size-full" preserveAspectRatio="none" aria-hidden>
          {[20, 40, 60, 80].map((v) => (
            <g key={v} stroke="currentColor" className="text-line" strokeWidth="0.4">
              <line x1={v} y1="0" x2={v} y2="100" />
              <line x1="0" y1={v} x2="100" y2={v} />
            </g>
          ))}
        </svg>
        {pts.map((p) => {
          const x = ((p.lon - minLon + padLon) / w) * 100;
          const y = ((maxLat + padLat - p.lat) / h) * 100;
          const r = 5 + Math.sqrt(p.count / max) * 9;
          return (
            <span
              key={p.key}
              className="absolute rounded-full bg-accent shadow-[0_0_0_3px_color-mix(in_oklab,var(--accent)_28%,transparent)] transition-transform duration-500 group-hover:scale-110"
              style={{ left: `${x}%`, top: `${y}%`, width: r, height: r, marginLeft: -r / 2, marginTop: -r / 2 }}
            />
          );
        })}
        <span className="absolute right-2 bottom-2 grid size-9 place-items-center rounded-full bg-accent text-white shadow-lg">
          <MapIcon className="size-4" />
        </span>
      </div>
      <div className="mt-2 truncate text-sm font-medium">{t('explore.map')}</div>
      <div className="truncate text-xs text-dim">{t('explore.placesCount', { n: places.length })}</div>
    </Link>
  );
}

// ───────────────────────── Types ─────────────────────────

const TYPE_ICONS: Record<MediaFlagName | 'favorites', ReactNode> = {
  video: <Film />,
  screenshot: <Smartphone />,
  selfie: <ScanFace />,
  panorama: <GalleryHorizontalEnd />,
  animated: <ImagePlay />,
  favorites: <Heart />,
};

export function TypeTile({ g, favorites }: { g?: TypeGroup; favorites?: boolean }) {
  const t = useT();
  const label = favorites ? t('explore.type.favorites') : t(TYPE_KEYS[g!.flag]);
  const to = favorites ? '/favorites' : `/explore/type/${g!.flag}`;
  return (
    <Link to={to} data-type={favorites ? 'favorites' : g!.flag} className="group relative block aspect-[4/3] overflow-hidden rounded-2xl bg-[var(--tile-bg)] sm:aspect-square">
      {g ? (
        <Thumb id={g.coverId} className="absolute inset-0 size-full transition-transform duration-500 ease-[var(--ease-out)] group-hover:scale-105" />
      ) : (
        <div className="absolute inset-0 bg-[linear-gradient(135deg,var(--accent),var(--accent-2))] opacity-80" />
      )}
      <div className="absolute inset-0 bg-gradient-to-t from-black/75 via-black/10 to-transparent" />
      <div className="absolute inset-x-0 bottom-0 flex items-center gap-2 p-2.5 text-white">
        <span className="grid size-7 shrink-0 place-items-center rounded-full bg-white/20 [&>svg]:size-3.5">{TYPE_ICONS[favorites ? 'favorites' : g!.flag]}</span>
        <div className="min-w-0">
          <div className="truncate text-[13px] leading-tight font-semibold">{label}</div>
          {g && <div className="text-[11px] text-white/70 tabular-nums">{g.count}</div>}
        </div>
      </div>
    </Link>
  );
}

// ───────────────────────── Cameras / years ─────────────────────────

export function CameraChip({ c }: { c: CameraGroup }) {
  return (
    <Link
      to={cameraHref(c.key)}
      data-camera={c.key}
      className="group inline-flex h-12 max-w-full items-center gap-2.5 rounded-full border border-line bg-surface py-1 pr-4 pl-1 transition-colors hover:bg-surface-2"
    >
      <span className="size-10 shrink-0 overflow-hidden rounded-full">
        <Thumb id={c.coverId} className="size-full" />
      </span>
      <span className="min-w-0 truncate text-sm font-medium">{cameraLabel(c.make, c.model)}</span>
      <span className="text-xs text-faint tabular-nums">{c.count}</span>
    </Link>
  );
}

export function YearCard({ y }: { y: YearGroup }) {
  const t = useT();
  return (
    <Link to={`/explore/year/${y.year}`} data-year={y.year} className="group relative block aspect-[4/5] w-[34vw] max-w-[160px] shrink-0 snap-start overflow-hidden rounded-2xl bg-[var(--tile-bg)] sm:w-[160px]">
      <Thumb id={y.coverId} className="absolute inset-0 size-full transition-transform duration-500 ease-[var(--ease-out)] group-hover:scale-105" />
      <div className="absolute inset-0 bg-gradient-to-t from-black/75 via-black/5 to-transparent" />
      <div className="absolute inset-x-0 bottom-0 p-3 text-white">
        <div className="font-display text-2xl font-bold tabular-nums">{y.year}</div>
        <div className="text-[11px] text-white/70">{t('explore.count', { n: y.count })}</div>
      </div>
    </Link>
  );
}

export function IndexingBanner({ r }: { r: ExploreResult }) {
  const t = useT();
  const { done, total } = r.indexing;
  if (!total || done >= total) return null;
  const p = Math.floor((done / total) * 100);
  return (
    <div role="status" className="mb-6 flex items-center gap-3 rounded-2xl border border-line bg-surface px-4 py-3" data-indexing>
      <span className="relative grid size-8 shrink-0 place-items-center">
        <svg viewBox="0 0 36 36" className="absolute inset-0 -rotate-90">
          <circle cx="18" cy="18" r="15" fill="none" stroke="var(--surface-3)" strokeWidth="3" />
          <circle cx="18" cy="18" r="15" fill="none" stroke="var(--accent)" strokeWidth="3" strokeLinecap="round" strokeDasharray={`${(p / 100) * 94.2} 94.2`} />
        </svg>
        <MapIcon className="size-3.5 text-accent" />
      </span>
      <div className="min-w-0 flex-1 text-sm">{t('explore.indexing', { p })}</div>
    </div>
  );
}
