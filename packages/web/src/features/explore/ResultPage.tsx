import { useEffect, useMemo, useRef, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router';
import { motion } from 'motion/react';
import { ArrowLeft, Camera, CalendarDays, Map as MapIcon, MapPin, Play, Presentation, Sparkles } from 'lucide-react';
import type { BrowseQuery, Memory } from '@peekaround/shared';
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
import { useEnter } from './cards';
import { memoryFromId } from './memoryId';
import { useExplore, useOpenViewer, usePlaySlideshow } from './data';
import { cameraLabel, countryName, exploreQuery, flagEmoji, isFlagName, memoryLabel, playHref, TYPE_KEYS, yearRange } from './labels';

export type ResultKind = 'place' | 'type' | 'camera' | 'year' | 'memory';

interface Resolved {
  query: BrowseQuery | null;
  title: string;
  subtitle: string;
  icon: React.ReactNode;
  missing: boolean;
  memory?: Memory;
  placeKey?: string;
}

/** route params → BrowseQuery + header texts (needs the Explore result for names / memory ranges) */
export function useResolved(kind: ResultKind): Resolved & { loading: boolean } {
  const params = useParams();
  const lang = useLang();
  const t = useT();
  const { result: r, isLoading, isSuccess } = useExplore();
  const raw = params.key ?? params.flag ?? params.year ?? params.id ?? '';
  const key = decodeURIComponent(raw);
  const res = useMemo((): Resolved => {
    switch (kind) {
      case 'place': {
        const g = r.places.find((p) => p.key === key);
        const cc = key.split('-')[0];
        return {
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
        const m = r.memories.find((x) => x.id === key) ?? (isSuccess ? memoryFromId(key) : null);
        if (!m) return { query: null, title: '', subtitle: '', icon: <Sparkles />, missing: isSuccess };
        const l = memoryLabel(m, lang);
        return { query: exploreQuery({ from: m.from, to: m.to }), title: l.title, subtitle: l.subtitle, icon: <Sparkles />, missing: false, memory: m };
      }
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

      {loading && (
        <div className="grid h-60 place-items-center">
          <Spinner className="size-7" />
        </div>
      )}

      {source && total > 0 && (
        <section className={cn(mobile ? '-mx-2.5' : 'pr-3')}>
          <MediaGrid source={source} scrollRef={scrollRef} onOpen={open} handleRef={gridHandle} sortKey="takenAt" groups />
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
