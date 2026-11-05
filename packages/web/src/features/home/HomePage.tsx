import { useEffect, useMemo, useState } from 'react';
import { Link, useNavigate, useLocation as useRouterLocation } from 'react-router';
import { motion } from 'motion/react';
import { ArrowRight, FolderPlus, Images, Sparkles } from 'lucide-react';
import type { Library, MediaItem } from '@peekaround/shared';
import { useBackend, useInfo } from '@/backend';
import { useT } from '@/i18n';
import { LibraryIcon } from '@/lib/icons';
import { useBrowse, useLibraries } from '@/lib/queries';
import { libraryHref, withView } from '@/lib/routes';
import { useChrome } from '@/store/chrome';
import { useLocation } from '@/store/location';
import { useViewer } from '@/store/viewer';
import { Button, EmptyState } from '@/components/ui';
import { LibraryEditor } from '@/features/settings/LibraryEditor';
import { MediaGrid } from '@/features/gallery/MediaGrid';
import { useScrollRef } from '@/features/gallery/GalleryPage';
import { StaticSource } from '@/features/gallery/grid/source';
import { MemoriesStrip } from '@/features/explore/MemoriesStrip';

function greeting(t: ReturnType<typeof useT>) {
  const h = new Date().getHours();
  if (h < 5) return t('home.greeting.night');
  if (h < 12) return t('home.greeting.morning');
  if (h < 18) return t('home.greeting.day');
  if (h < 22) return t('home.greeting.evening');
  return t('home.greeting.night');
}

function LibraryCard({ lib, index }: { lib: Library; index: number }) {
  const backend = useBackend();
  const { items } = useBrowse({ libraryId: lib.id, recursive: true, limit: 4, sort: 'takenAt', order: 'desc', kind: 'all' });
  const covers = items.filter((i) => i.kind !== 'audio').slice(0, 4);
  return (
    <motion.div
      initial={{ opacity: 0, y: 24, scale: 0.97 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      transition={{ type: 'spring', stiffness: 260, damping: 26, delay: 0.08 + index * 0.06 }}
      whileHover={{ y: -6 }}
    >
      <Link to={libraryHref(lib.id)} className="group relative block aspect-[5/4] overflow-hidden rounded-3xl border border-line bg-elev shadow-sm transition-shadow hover:shadow-soft" data-drop-folder={lib.folders.length === 1 ? lib.folders[0].id : undefined} data-drop-dir="" data-drop-label={lib.name}>
        <div className="absolute inset-0 grid grid-cols-2 grid-rows-2 gap-0.5">
          {covers.length ? (
            covers.map((c, i) => (
              <img
                key={c.id}
                src={backend.thumbUrl(c.id, 320)}
                alt=""
                draggable={false}
                className={`size-full object-cover transition-transform duration-[900ms] ease-[var(--ease-out)] group-hover:scale-110 ${covers.length === 1 ? 'col-span-2 row-span-2' : covers.length === 2 ? 'row-span-2' : covers.length === 3 && i === 0 ? 'row-span-2' : ''}`}
              />
            ))
          ) : (
            <div className="col-span-2 row-span-2" style={{ background: `linear-gradient(135deg, ${lib.color}, transparent)` }} />
          )}
        </div>
        <div className="absolute inset-0 bg-gradient-to-t from-black/80 via-black/10 to-transparent" />
        <div className="absolute inset-x-0 bottom-0 flex items-center gap-3 p-4 text-white">
          <span className="grid size-10 place-items-center rounded-xl shadow-lg" style={{ background: lib.color }}>
            <LibraryIcon name={lib.icon} className="size-5" />
          </span>
          <div className="min-w-0 flex-1">
            <div className="truncate font-display text-lg font-semibold">{lib.name}</div>
            <div className="text-xs text-white/70 tabular-nums">{lib.mediaCount}</div>
          </div>
          <ArrowRight className="size-5 -translate-x-2 opacity-0 transition-all duration-300 group-hover:translate-x-0 group-hover:opacity-100" />
        </div>
      </Link>
    </motion.div>
  );
}

export function HomePage() {
  const t = useT();
  const info = useInfo();
  const libs = useLibraries();
  const nav = useNavigate();
  const routerLoc = useRouterLocation();
  const scrollRef = useScrollRef();
  const setChrome = useChrome((s) => s.set);
  const setView = useLocation((s) => s.setView);
  const [adding, setAdding] = useState(false);
  const recent = useBrowse({ recursive: true, sort: 'mtime', order: 'desc', limit: 60, kind: 'all' });
  const recentItems = useMemo(
    () => (recent.data?.pages.flatMap((p) => p.items) ?? []).filter((i: MediaItem) => i.kind !== 'audio').slice(0, 48),
    [recent.data],
  );

  useEffect(() => setChrome({ crumbs: [], title: t('nav.home'), viewControls: false }), [setChrome, t]);
  const recentSource = useMemo(() => new StaticSource(`recent:${recentItems.map((i) => i.id).join(',')}`, recentItems), [recentItems]);
  useEffect(() => setView({ target: null, targetLabel: '', source: recentSource }), [recentSource, setView]);
  useEffect(() => () => setView({ source: null }), [setView]);

  const libraries = libs.data?.libraries ?? [];
  const total = libraries.reduce((a, l) => a + l.mediaCount, 0);
  const canEdit = info && !info.caps.readonly;

  return (
    <div className="relative mx-auto w-full max-w-[1800px] px-4 pb-28 sm:px-7">
      <section className="relative -mx-4 mb-10 overflow-hidden px-4 pt-12 pb-10 sm:-mx-7 sm:px-7">
        <div className="aurora opacity-70" />
        <motion.div initial={{ opacity: 0, y: 18 }} animate={{ opacity: 1, y: 0 }} transition={{ type: 'spring', stiffness: 200, damping: 24 }} className="relative">
          <div className="mb-3 inline-flex items-center gap-2 rounded-full border border-line bg-surface px-3 py-1 text-xs text-dim backdrop-blur">
            <Sparkles className="size-3.5 text-accent" />
            {t('app.tagline')}
          </div>
          <h1 className="font-display text-4xl font-bold tracking-tight sm:text-5xl">
            {greeting(t)}
            <span className="text-gradient">.</span>
          </h1>
          {total > 0 && <p className="mt-3 text-dim tabular-nums">{t('common.items', { n: total.toLocaleString() })}</p>}
        </motion.div>
      </section>

      <MemoriesStrip className="mb-10" />

      {libs.isSuccess && libraries.length === 0 && (
        <EmptyState
          icon={<Images />}
          title={t('home.empty.title')}
          body={t('home.empty.body')}
          action={
            canEdit && (
              <Button variant="primary" size="lg" icon={<FolderPlus className="size-5" />} onClick={() => setAdding(true)}>
                {t('home.empty.cta')}
              </Button>
            )
          }
        />
      )}

      {libraries.length > 0 && (
        <section className="mb-12">
          <div className="mb-4 flex items-center justify-between">
            <h2 className="font-display text-xl font-semibold">{t('home.libraries')}</h2>
            {canEdit && (
              <Button variant="ghost" size="sm" icon={<FolderPlus className="size-4" />} onClick={() => setAdding(true)}>
                {t('nav.addLibrary')}
              </Button>
            )}
          </div>
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3 2xl:grid-cols-4">
            {libraries.map((l, i) => (
              <LibraryCard key={l.id} lib={l} index={i} />
            ))}
          </div>
        </section>
      )}

      {recentItems.length > 0 && (
        <section>
          <div className="mb-4 flex items-center justify-between">
            <h2 className="font-display text-xl font-semibold">{t('home.recent')}</h2>
            <Button variant="ghost" size="sm" onClick={() => nav('/timeline')}>
              {t('home.seeAll')} <ArrowRight className="size-4" />
            </Button>
          </div>
          <MediaGrid
            source={recentSource}
            scrollRef={scrollRef}
            groups={false}
            fastScroller={false}
            zoomable={false}
            onOpen={(item) => {
              useViewer.getState().setActive(item.id);
              requestAnimationFrame(() => nav({ search: withView(routerLoc.search, item.id) }, { state: { viewer: true } }));
            }}
          />
        </section>
      )}
      <LibraryEditor value={adding ? 'new' : null} onClose={() => setAdding(false)} />
    </div>
  );
}
