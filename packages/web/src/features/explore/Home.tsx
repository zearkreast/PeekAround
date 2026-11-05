import { useEffect } from 'react';
import { motion } from 'motion/react';
import { Compass, MapPin, Map as MapIcon } from 'lucide-react';
import { useT } from '@/i18n';
import { useChrome } from '@/store/chrome';
import { EmptyState, Spinner } from '@/components/ui';
import { CameraChip, Carousel, IndexingBanner, MapCard, MemoryCard, PlaceCard, Section, SectionLink, TypeTile, useEnter, YearCard } from './cards';
import { isIndexing, useExplore } from './data';
import { TYPE_ORDER } from './labels';

export function ExploreHome() {
  const t = useT();
  const setChrome = useChrome((s) => s.set);
  const { result: r, isLoading, isError } = useExplore();
  const enter = useEnter();
  useEffect(() => setChrome({ crumbs: [], title: t('explore.title'), viewControls: false }), [setChrome, t]);

  const types = TYPE_ORDER.map((f) => r.types.find((g) => g.flag === f)).filter((g) => !!g);
  const empty = !isLoading && !r.years.length && !r.memories.length && !r.places.length;

  return (
    <div className="mx-auto w-full max-w-[1800px] px-3 pt-4 pb-28 sm:px-7 sm:pt-6" data-page="explore">
      <motion.div {...enter} className="mb-5 sm:mb-7">
        <h1 className="flex items-center gap-2.5 font-display text-[22px] font-bold tracking-tight sm:text-[34px]">
          <Compass className="size-6 text-accent sm:size-7" />
          {t('explore.title')}
        </h1>
        <p className="mt-1 text-[13px] text-dim sm:text-sm">{t('explore.subtitle')}</p>
      </motion.div>

      <IndexingBanner r={r} />

      {isLoading && (
        <div className="grid h-60 place-items-center">
          <Spinner className="size-7" />
        </div>
      )}

      {empty && !isError && <EmptyState icon={<Compass />} title={t('explore.empty.title')} body={t('explore.empty.body')} />}

      {!isLoading && !empty && (
        <>
          <Section id="memories" title={t('explore.memories')} count={r.memories.length || undefined}>
            {r.memories.length ? (
              <Carousel>
                {r.memories.map((m) => (
                  <MemoryCard key={m.id} m={m} />
                ))}
              </Carousel>
            ) : (
              <p className="rounded-2xl border border-dashed border-line px-4 py-5 text-sm text-dim">{t('explore.noMemories')}</p>
            )}
          </Section>

          <Section
            id="places"
            title={t('explore.places')}
            count={r.places.length || undefined}
            delay={0.05}
            actions={
              r.places.length > 0 && (
                <>
                  <SectionLink to="/explore/map" icon={<MapIcon />} label={t('explore.showOnMap')}>
                    <span className="hidden sm:inline">{t('explore.showOnMap')}</span>
                  </SectionLink>
                  <SectionLink to="/explore/places">{t('explore.seeAll')}</SectionLink>
                </>
              )
            }
          >
            {r.places.length ? (
              <Carousel>
                <MapCard places={r.places} />
                {r.places.slice(0, 24).map((p) => (
                  <PlaceCard key={p.key} p={p} />
                ))}
              </Carousel>
            ) : (
              <div className="flex items-start gap-3 rounded-2xl border border-dashed border-line px-4 py-5 text-sm text-dim">
                <MapPin className="mt-0.5 size-5 shrink-0 text-faint" />
                <span>{isIndexing(r) ? t('explore.noPlacesIndexing') : t('explore.noPlaces')}</span>
              </div>
            )}
          </Section>

          <Section id="types" title={t('explore.types')} delay={0.1}>
            <div className="grid grid-cols-3 gap-2 sm:grid-cols-4 sm:gap-3 lg:grid-cols-6">
              {types.map((g) => (
                <TypeTile key={g.flag} g={g} />
              ))}
              <TypeTile favorites />
            </div>
          </Section>

          {r.cameras.length > 0 && (
            <Section id="cameras" title={t('explore.cameras')} count={r.cameras.length} delay={0.15}>
              <div className="flex flex-wrap gap-2">
                {r.cameras.slice(0, 30).map((c) => (
                  <CameraChip key={c.key} c={c} />
                ))}
              </div>
            </Section>
          )}

          {r.years.length > 0 && (
            <Section id="years" title={t('explore.years')} delay={0.2}>
              <Carousel>
                {r.years.map((y) => (
                  <YearCard key={y.year} y={y} />
                ))}
              </Carousel>
            </Section>
          )}
        </>
      )}
    </div>
  );
}
