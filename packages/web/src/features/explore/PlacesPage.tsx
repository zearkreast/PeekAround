import { useEffect, useMemo } from 'react';
import { motion } from 'motion/react';
import { MapPin, Map as MapIcon } from 'lucide-react';
import type { PlaceGroup } from '@peekaround/shared';
import { useLang, useT } from '@/i18n';
import { useChrome } from '@/store/chrome';
import { EmptyState, Spinner } from '@/components/ui';
import { IndexingBanner, MapCard, PlaceCard, SectionLink, useEnter } from './cards';
import { isIndexing, useExplore } from './data';
import { countryName, flagEmoji } from './labels';
import { BackLink } from './ResultPage';

/** /explore/places — every place, grouped by country (biggest first) */
export function PlacesPage() {
  const t = useT();
  const lang = useLang();
  const setChrome = useChrome((s) => s.set);
  const { result: r, isLoading } = useExplore();
  const enter = useEnter();
  useEffect(() => setChrome({ crumbs: [], title: t('explore.allPlaces'), viewControls: false }), [setChrome, t]);

  const byCountry = useMemo(() => {
    const m = new Map<string, PlaceGroup[]>();
    for (const p of r.places) {
      const l = m.get(p.cc);
      if (l) l.push(p);
      else m.set(p.cc, [p]);
    }
    return [...m.entries()].map(([cc, list]) => ({ cc, list, n: list.reduce((a, p) => a + p.count, 0) })).sort((a, b) => b.n - a.n);
  }, [r.places]);

  return (
    <div className="mx-auto w-full max-w-[1800px] px-3 pt-3 pb-28 sm:px-7 sm:pt-5" data-page="explore-places">
      <BackLink />
      <motion.div {...enter} className="mb-5 flex flex-wrap items-end gap-3">
        <div className="min-w-0 flex-1">
          <h1 className="flex items-center gap-2.5 font-display text-[22px] font-bold tracking-tight sm:text-[34px]">
            <MapPin className="size-6 text-accent sm:size-7" />
            {t('explore.allPlaces')}
          </h1>
          <p className="mt-1 text-[13px] text-dim sm:text-sm">{t('explore.placesCount', { n: r.places.length })}</p>
        </div>
        {r.places.length > 0 && (
          <SectionLink to="/explore/map" icon={<MapIcon />}>
            {t('explore.showOnMap')}
          </SectionLink>
        )}
      </motion.div>
      <IndexingBanner r={r} />
      {isLoading && (
        <div className="grid h-60 place-items-center">
          <Spinner className="size-7" />
        </div>
      )}
      {!isLoading && !r.places.length && <EmptyState icon={<MapPin />} title={t('explore.places')} body={isIndexing(r) ? t('explore.noPlacesIndexing') : t('explore.noPlaces')} />}
      {r.places.length > 0 && (
        <div className="mb-8 grid grid-cols-2 gap-x-3 gap-y-5 sm:grid-cols-4 lg:grid-cols-6 2xl:grid-cols-8">
          <MapCard places={r.places} wide />
        </div>
      )}
      {byCountry.map(({ cc, list, n }) => (
        <section key={cc} className="mb-8">
          <h2 className="mb-3 flex items-baseline gap-2 font-display text-lg font-semibold">
            <span aria-hidden>{flagEmoji(cc)}</span>
            {countryName(cc, lang)}
            <span className="text-xs font-normal text-faint tabular-nums">{t('explore.count', { n })}</span>
          </h2>
          <div className="grid grid-cols-2 gap-x-3 gap-y-5 sm:grid-cols-4 lg:grid-cols-6 2xl:grid-cols-8">
            {list.map((p) => (
              <PlaceCard key={p.key} p={p} wide />
            ))}
          </div>
        </section>
      ))}
    </div>
  );
}
