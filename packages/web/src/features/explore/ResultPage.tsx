import { useEffect, useMemo, useRef, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router';
import { motion } from 'motion/react';
import { ArrowLeft, Camera, CalendarDays, Clapperboard, Globe2, HardDrive, History, Map as MapIcon, MapPin, MapPinOff, Moon, Palette, Play, Presentation, Shapes, Sparkles, Sunset } from 'lucide-react';
import { isColorBucket, isDayTime, isOrientation, isVideoLength, type BrowseQuery, type Memory } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useLang, useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { useIsMobile } from '@/lib/useMedia';
import { useChrome } from '@/store/chrome';
import { useLocation } from '@/store/location';
import { useViewer } from '@/store/viewer';
import { Button, EmptyState, Spinner } from '@/components/ui';
import { useScrollRef } from '@/features/gallery/GalleryPage';
import { MediaGrid, type MediaGridHandle } from '@/features/gallery/MediaGrid';
import { SelectionBar } from '@/features/gallery/SelectionBar';
import { useGridSource } from '@/features/gallery/grid/source';
import { Carousel, PlaceCard, useEnter } from './cards';
import { memoryFromId } from './memoryId';
import { useExplore, useOpenViewer, usePlaySlideshow } from './data';
import { CityCard } from './insights';
import {
  cameraLabel,
  COLOR_KEYS,
  countryName,
  DAYTIME_KEYS,
  DAYTIME_SUB_KEYS,
  dayLabel,
  dayRange,
  exploreQuery,
  flagEmoji,
  isFlagName,
  memoryLabel,
  ORIENT_KEYS,
  playHref,
  TYPE_KEYS,
  VLEN_KEYS,
  VLEN_SUB_KEYS,
  yearRange,
} from './labels';

export type ResultKind =
  | 'place'
  | 'type'
  | 'camera'
  | 'year'
  | 'memory'
  | 'country'
  | 'city'
  | 'color'
  | 'daytime'
  | 'shape'
  | 'length'
  | 'largest'
  | 'recent'
  | 'day'
  | 'nogps';

interface Resolved {
  query: BrowseQuery | null;
  title: string;
  subtitle: string;
  icon: React.ReactNode;
  missing: boolean;
  memory?: Memory;
  placeKey?: string;
  /** grid sort (default: by date with day headers) */
  sortKey?: 'takenAt' | 'size' | 'mtime';
  /** extra content between the header and the grid (regions / cities, hints) */
  extra?: React.ReactNode;
}

/** route params → BrowseQuery + header texts (needs the Explore result for names / memory ranges) */
export function useResolved(kind: ResultKind): Resolved & { loading: boolean } {
  const params = useParams();
  const lang = useLang();
  const t = useT();
  const { result: r, isLoading, isSuccess } = useExplore();
  const raw = params.key ?? params.flag ?? params.year ?? params.id ?? params.value ?? '';
  const key = decodeURIComponent(raw);
  const res = useMemo((): Resolved => {
    switch (kind) {
      case 'place': {
        const g = r.places.find((p) => p.key === key);
        const cc = key.split('-')[0];
        const cities = (r.cities ?? []).filter((x) => x.placeKey === key);
        return {
          extra: cities.length > 1 ? <DrillDown regions={[]} cities={cities} /> : undefined,
          query: exploreQuery({ place: key }),
          title: g?.name ?? countryName(cc, lang),
          subtitle: [`${flagEmoji(g?.cc ?? cc)} ${g ? countryName(g.cc, lang) : ''}`.trim(), g?.city && g.city !== g.name ? g.city : ''].filter(Boolean).join(' · '),
          icon: <MapPin />,
          missing: false,
          placeKey: key,
        };
      }
      case 'type': {
        const flag = isFlagName(key) ? key : null;
        return { query: flag ? exploreQuery({ flag }) : null, title: flag ? t(TYPE_KEYS[flag]) : key, subtitle: '', icon: <Sparkles />, missing: !flag };
      }
      case 'camera': {
        const g = r.cameras.find((c) => c.key === key);
        const [mk, md] = key.split('|');
        return { query: exploreQuery({ camera: key }), title: g ? cameraLabel(g.make, g.model) : cameraLabel(mk, md), subtitle: '', icon: <Camera />, missing: false };
      }
      case 'year': {
        const y = Number(key);
        const ok = Number.isInteger(y) && y > 1900 && y < 3000;
        return { query: ok ? exploreQuery(yearRange(y)) : null, title: String(key), subtitle: '', icon: <CalendarDays />, missing: !ok };
      }
      case 'memory': {
        const m = r.memories.find((x) => x.id === key) ?? r.yearReviews?.find((x) => x.id === key) ?? (isSuccess || key.startsWith('year-') ? memoryFromId(key) : null);
        if (!m) return { query: null, title: '', subtitle: '', icon: <Sparkles />, missing: isSuccess };
        const l = memoryLabel(m, lang);
        return { query: exploreQuery({ from: m.from, to: m.to }), title: l.title, subtitle: l.subtitle, icon: <Sparkles />, missing: false, memory: m };
      }
      case 'country': {
        const cc = key.toUpperCase();
        const ok = /^[A-Z]{2}$/.test(cc);
        const c = r.countries?.find((x) => x.cc === cc);
        const regions = r.places.filter((p) => p.cc === cc);
        const cities = (r.cities ?? []).filter((x) => x.cc === cc);
        return {
          query: ok ? exploreQuery({ place: cc }) : null,
          title: countryName(cc, lang),
          subtitle: c ? t('insights.countrySub', { regions: c.regions, cities: c.cities }) : '',
          icon: flagEmoji(cc) ? <span className="shrink-0 leading-none">{flagEmoji(cc)}</span> : <Globe2 />,
          missing: !ok,
          placeKey: ok ? cc : undefined,
          extra: <DrillDown regions={regions} cities={cities} />,
        };
      }
      case 'city': {
        const [placeKey, ...rest] = key.split('|');
        const city = rest.join('|');
        const ok = !!placeKey && !!city;
        const cc = placeKey.split('-')[0];
        const g = r.places.find((p) => p.key === placeKey);
        return {
          query: ok ? exploreQuery({ place: placeKey, city }) : null,
          title: city,
          subtitle: [`${flagEmoji(cc)} ${countryName(cc, lang)}`.trim(), g && g.name !== city ? g.name : ''].filter(Boolean).join(' · '),
          icon: <MapPin />,
          missing: !ok,
          placeKey: ok ? placeKey : undefined,
        };
      }
      case 'color': {
        const ok = isColorBucket(key);
        return { query: ok ? exploreQuery({ color: key }) : null, title: ok ? t(COLOR_KEYS[key]) : key, subtitle: '', icon: <Palette />, missing: !ok };
      }
      case 'daytime': {
        const ok = isDayTime(key);
        return {
          query: ok ? exploreQuery({ daytime: key }) : null,
          title: ok ? t(DAYTIME_KEYS[key]) : key,
          subtitle: ok ? t(DAYTIME_SUB_KEYS[key]) : '',
          icon: key === 'night' ? <Moon /> : <Sunset />,
          missing: !ok,
        };
      }
      case 'shape': {
        const ok = isOrientation(key);
        return { query: ok ? exploreQuery({ orient: key }) : null, title: ok ? t(ORIENT_KEYS[key]) : key, subtitle: '', icon: <Shapes />, missing: !ok };
      }
      case 'length': {
        const ok = isVideoLength(key);
        return {
          query: ok ? exploreQuery({ vlen: key, kind: 'video' }) : null,
          title: ok ? t(VLEN_KEYS[key]) : key,
          subtitle: ok ? t(VLEN_SUB_KEYS[key]) : '',
          icon: <Clapperboard />,
          missing: !ok,
        };
      }
      case 'largest':
        return { query: exploreQuery({ sort: 'size', order: 'desc' }), title: t('insights.largest'), subtitle: t('insights.largestSub'), icon: <HardDrive />, missing: false, sortKey: 'size' };
      case 'recent':
        return { query: exploreQuery({ sort: 'mtime', order: 'desc', byAdded: true }), title: t('insights.recent'), subtitle: t('insights.recentSub'), icon: <History />, missing: false, sortKey: 'mtime' };
      case 'day': {
        const d = dayRange(key);
        return { query: d ? exploreQuery(d) : null, title: d ? dayLabel(key, lang, true) : key, subtitle: '', icon: <CalendarDays />, missing: !d };
      }
      case 'nogps':
        return {
          query: exploreQuery({ gps: 'none' }),
          title: t('map.noLocationTitle'),
          subtitle: '',
          icon: <MapPinOff />,
          missing: false,
          extra: (
            <p className="mb-5 flex max-w-3xl items-start gap-2.5 rounded-2xl border border-line bg-surface px-4 py-3 text-[13px] text-dim" data-nogps-hint>
              <MapPinOff className="mt-0.5 size-4 shrink-0 text-faint" />
              <span>
                {t('map.noLocationHint')} {t('meta.takeoutHint')}
              </span>
            </p>
          ),
        };
    }
  }, [kind, key, r, lang, t, isSuccess]);
  return { ...res, loading: isLoading };
}

export function ResultPage({ kind }: { kind: ResultKind }) {
  const t = useT();
  const nav = useNavigate();
  const backend = useBackend();
  const mobile = useIsMobile();
  const scrollRef = useScrollRef();
  const setChrome = useChrome((s) => s.set);
  const setView = useLocation((s) => s.setView);
  const open = useOpenViewer();
  const playSlides = usePlaySlideshow();
  const enter = useEnter();
  const gridHandle = useRef<MediaGridHandle | null>(null);
  const [busy, setBusy] = useState(false);
  const res = useResolved(kind);
  // Android local mode filters places / cameras with metadata loaded by explore() → wait for it there
  const gated = backend.kind === 'android-local' && res.loading;
  const source = useGridSource(gated ? null : res.query);
  const total = source?.total ?? 0;
  const loading = res.loading || (!!source && !source.ready && !source.error);

  useEffect(() => setChrome({ crumbs: [], title: res.title, viewControls: false }), [setChrome, res.title]);
  useEffect(() => setView({ target: null, targetLabel: res.title, source }), [source, res.title, setView]);
  useEffect(() => () => setView({ target: null, targetLabel: '', source: null }), [setView]);

  const activeId = useViewer((s) => s.activeId);
  useEffect(() => {
    if (activeId) gridHandle.current?.scrollToItem(activeId);
  }, [activeId]);

  const slideshow = async () => {
    if (!source) return;
    setBusy(true);
    try {
      playSlides(await source.itemsInRange(0, Math.min(source.total, 5000) - 1));
    } finally {
      setBusy(false);
    }
  };

  if (res.missing) {
    return (
      <div className="mx-auto w-full max-w-[1800px] px-3 pt-4 pb-28 sm:px-7">
        <BackLink />
        <EmptyState icon={<Sparkles />} title={t('explore.notFound')} body={t('explore.notFoundBody')} action={<Button onClick={() => nav('/explore')}>{t('explore.back')}</Button>} />
      </div>
    );
  }

  const stats = source && source.kindCounts.image && source.kindCounts.video ? t('home.stats', { photos: source.kindCounts.image, videos: source.kindCounts.video }) : '';

  return (
    <div className="mx-auto w-full max-w-[1800px] px-3 pt-3 pb-28 sm:px-7 sm:pt-5" data-page={`explore-${kind}`}>
      <BackLink />
      <motion.div {...enter} className="mb-4 flex flex-col gap-3 sm:mb-6 sm:flex-row sm:items-end sm:gap-6">
        <div className="min-w-0 sm:flex-1">
          <h1 className="flex items-center gap-2.5 truncate font-display text-[22px] font-bold tracking-tight sm:text-[34px] [&>svg]:size-6 [&>svg]:shrink-0 [&>svg]:text-accent sm:[&>svg]:size-7">
            {res.icon}
            <span className="truncate">{res.title || ' '}</span>
          </h1>
          <p className="mt-1 text-[13px] text-dim tabular-nums sm:text-sm">
            {[res.subtitle, loading ? t('common.loading') : t('explore.count', { n: total }), !loading && total ? stats : ''].filter(Boolean).join(' · ')}
          </p>
        </div>
        {total > 0 && (
          <div className="flex flex-wrap items-center gap-2">
            {res.memory && (
              <Button variant="primary" size="sm" icon={<Play className="size-4" />} onClick={() => nav(playHref(res.memory!.id))}>
                {t('explore.play')}
              </Button>
            )}
            <Button variant="secondary" size="sm" loading={busy} icon={<Presentation className="size-4" />} onClick={() => void slideshow()} data-action="slideshow">
              {t('explore.slideshow')}
            </Button>
            {res.placeKey && (
              <Button variant="ghost" size="sm" icon={<MapIcon className="size-4" />} onClick={() => nav(`/explore/map?place=${encodeURIComponent(res.placeKey!)}`)}>
                {t('explore.showOnMap')}
              </Button>
            )}
          </div>
        )}
      </motion.div>

      {res.extra}

      {loading && (
        <div className="grid h-60 place-items-center">
          <Spinner className="size-7" />
        </div>
      )}

      {source && total > 0 && (
        <section className={cn(mobile ? '-mx-2.5' : 'pr-3')}>
          <MediaGrid source={source} scrollRef={scrollRef} onOpen={open} handleRef={gridHandle} sortKey={res.sortKey ?? 'takenAt'} groups={!res.sortKey || res.sortKey === 'takenAt'} />
        </section>
      )}

      {source?.ready && !total && <EmptyState icon={res.icon} title={t('gallery.noResults')} />}

      <SelectionBar source={source} />
    </div>
  );
}

export function BackLink() {
  const t = useT();
  return (
    <Link to="/explore" className="-ml-1.5 mb-1 hidden h-9 items-center md:inline-flex gap-1 rounded-lg px-1.5 text-[13px] font-medium text-dim hover:text-fg">
      <ArrowLeft className="size-4" />
      {t('explore.back')}
    </Link>
  );
}

/** country → regions → cities rows above a result grid */
function DrillDown({ regions, cities }: { regions: import('@peekaround/shared').PlaceGroup[]; cities: import('@peekaround/shared').CityGroup[] }) {
  const t = useT();
  if (!regions.length && !cities.length) return null;
  return (
    <div className="mb-6 flex flex-col gap-5" data-drilldown>
      {regions.length > 0 && (
        <div>
          <h2 className="mb-2.5 font-display text-base font-semibold">
            {t('insights.regions')} <span className="ml-1 text-xs font-normal text-faint tabular-nums">{regions.length}</span>
          </h2>
          <Carousel>
            {regions.map((p) => (
              <PlaceCard key={p.key} p={p} />
            ))}
          </Carousel>
        </div>
      )}
      {cities.length > 0 && (
        <div>
          <h2 className="mb-2.5 font-display text-base font-semibold">
            {t('insights.cities')} <span className="ml-1 text-xs font-normal text-faint tabular-nums">{cities.length}</span>
          </h2>
          <Carousel>
            {cities.slice(0, 60).map((c) => (
              <CityCard key={c.key} c={c} />
            ))}
          </Carousel>
        </div>
      )}
    </div>
  );
}
