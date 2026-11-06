import { useEffect } from 'react';
import { motion } from 'motion/react';
import { Compass, MapPin, MapPinOff, Map as MapIcon } from 'lucide-react';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { useChrome } from '@/store/chrome';
import { EmptyState } from '@/components/ui';
import { CameraChip, Carousel, IndexingBanner, MapCard, MemoryCard, PlaceCard, Section, SectionLink, TypeTile, useEnter, YearCard } from './cards';
import { isIndexing, useExplore } from './data';
import {
  CalendarHeatmap,
  ColorChip,
  CountryCard,
  DayTimeCard,
  DuplicatesCard,
  ExploreSkeleton,
  RankedStrip,
  ShapeTile,
  StatsHeader,
  SurpriseButton,
  YearReviewCard,
} from './insights';
import { LARGEST_HREF, NO_GPS_HREF, RECENT_HREF, TYPE_ORDER } from './labels';

export function ExploreHome() {
  const t = useT();
  const backend = useBackend();
  const setChrome = useChrome((s) => s.set);
  const { result: r, isLoading, isError } = useExplore();
  const enter = useEnter();
  useEffect(() => setChrome({ crumbs: [], title: t('explore.title'), viewControls: false }), [setChrome, t]);

  const types = TYPE_ORDER.map((f) => r.types.find((g) => g.flag === f)).filter((g) => !!g);
  const empty = !isLoading && !r.years.length && !r.memories.length && !r.places.length;
  const shapes = [...(r.orientations ?? []).map((g) => ({ g, kind: 'orient' as const })), ...(r.videoLengths ?? []).map((g) => ({ g, kind: 'vlen' as const }))];
  const years = r.yearReviews ?? [];
  const countries = r.countries ?? [];

  return (
    <div className="mx-auto w-full max-w-[1800px] px-3 pt-4 pb-28 sm:px-7 sm:pt-6" data-page="explore">
      <motion.div {...enter} className="mb-4 flex flex-wrap items-end gap-3 sm:mb-5">
        <div className="min-w-0 flex-1">
          <h1 className="flex items-center gap-2.5 font-display text-[22px] font-bold tracking-tight sm:text-[34px]">
            <Compass className="size-6 text-accent sm:size-7" />
            {t('explore.title')}
          </h1>
          <p className="mt-1 text-[13px] text-dim sm:text-sm">{t('explore.subtitle')}</p>
        </div>
        {!isLoading && !empty && <SurpriseButton r={r} />}
      </motion.div>

      <IndexingBanner r={r} />

      {isLoading && <ExploreSkeleton />}

      {empty && !isError && <EmptyState icon={<Compass />} title={t('explore.empty.title')} body={t('explore.empty.body')} />}

      {!isLoading && !empty && (
        <>
          <StatsHeader r={r} />

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

          {years.length > 0 && (
            <Section id="year-review" title={t('insights.yearReview')} count={years.length} delay={0.03}>
              <Carousel>
                {years.map((m) => (
                  <YearReviewCard key={m.id} m={m} />
                ))}
              </Carousel>
            </Section>
          )}

          <Section
            id="places"
            title={t('explore.places')}
            count={r.places.length || undefined}
            delay={0.05}
            actions={
              <>
                {!!r.noLocation && (
                  <SectionLink to={NO_GPS_HREF} icon={<MapPinOff />} label={t('map.noLocation', { n: r.noLocation })}>
                    <span className="hidden sm:inline">{t('map.noLocation', { n: r.noLocation })}</span>
                  </SectionLink>
                )}
                {r.places.length > 0 && (
                  <>
                    <SectionLink to="/explore/map" icon={<MapIcon />} label={t('explore.showOnMap')}>
                      <span className="hidden sm:inline">{t('explore.showOnMap')}</span>
                    </SectionLink>
                    <SectionLink to="/explore/places">{t('explore.seeAll')}</SectionLink>
                  </>
                )}
              </>
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

          {countries.length > 0 && (
            <Section id="countries" title={t('insights.countries')} count={countries.length} delay={0.07}>
              <Carousel>
                {countries.map((c) => (
                  <CountryCard key={c.cc} c={c} />
                ))}
              </Carousel>
            </Section>
          )}

          {!!r.calendarYears?.length && backend.calendar && (
            <Section id="calendar" title={t('insights.calendar')} delay={0.09}>
              <CalendarHeatmap years={r.calendarYears} counts={r.years} />
            </Section>
          )}

          {!!r.dayTimes?.length && (
            <Section id="daytime" title={t('insights.timeOfDay')} delay={0.1}>
              <Carousel>
                {r.dayTimes.map((g) => (
                  <DayTimeCard key={g.value} g={g} />
                ))}
              </Carousel>
            </Section>
          )}

          {!!r.colors?.length && (
            <Section id="colors" title={t('insights.colors')} delay={0.11}>
              <Carousel>
                {r.colors.map((g) => (
                  <ColorChip key={g.value} g={g} />
                ))}
              </Carousel>
            </Section>
          )}

          {shapes.length > 0 && (
            <Section id="shapes" title={t('insights.shapes')} delay={0.12}>
              <div className="grid grid-cols-2 gap-2 sm:grid-cols-4 sm:gap-3 lg:grid-cols-6">
                {shapes.map(({ g, kind }) => (
                  <ShapeTile key={`${kind}:${g.value}`} g={g} kind={kind} />
                ))}
              </div>
            </Section>
          )}

          <Section id="types" title={t('explore.types')} delay={0.13}>
            <div className="grid grid-cols-3 gap-2 sm:grid-cols-4 sm:gap-3 lg:grid-cols-6">
              {types.map((g) => (
                <TypeTile key={g.flag} g={g} />
              ))}
              <TypeTile favorites />
            </div>
          </Section>

          {!!r.recent?.length && (
            <Section id="recent" title={t('insights.recent')} delay={0.14} actions={<SectionLink to={RECENT_HREF}>{t('explore.seeAll')}</SectionLink>}>
              <Carousel>
                <RankedStrip list={r.recent} kind="recent" />
              </Carousel>
            </Section>
          )}

          {!!r.largest?.length && (
            <Section id="largest" title={t('insights.largest')} delay={0.15} actions={<SectionLink to={LARGEST_HREF}>{t('explore.seeAll')}</SectionLink>}>
              <Carousel>
                <RankedStrip list={r.largest} kind="largest" />
              </Carousel>
            </Section>
          )}

          {r.duplicates && (
            <Section id="duplicates" title={t('insights.duplicates')} delay={0.16}>
              <DuplicatesCard d={r.duplicates} />
            </Section>
          )}

          {r.cameras.length > 0 && (
            <Section id="cameras" title={t('explore.cameras')} count={r.cameras.length} delay={0.17}>
              <div className="flex flex-wrap gap-2">
                {r.cameras.slice(0, 30).map((c) => (
                  <CameraChip key={c.key} c={c} />
                ))}
              </div>
            </Section>
          )}

          {r.years.length > 0 && (
            <Section id="years" title={t('explore.years')} delay={0.18}>
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
