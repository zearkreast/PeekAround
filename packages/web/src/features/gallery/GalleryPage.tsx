import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type RefObject } from 'react';
import { useNavigate, useParams, useSearchParams, useLocation as useRouterLocation } from 'react-router';
import { AnimatePresence, motion } from 'motion/react';
import { useQueryClient } from '@tanstack/react-query';
import { MemoriesStrip } from '@/features/explore/MemoriesStrip';
import { CalendarDays, Clock3, FolderOpen, Heart, ImageOff, Play, Presentation, Search, UploadCloud } from 'lucide-react';
import type { BrowseQuery, MediaItem } from '@peekaround/shared';
import { useBackend, useInfo } from '@/backend';
import { useLang, useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { durationUntil } from '@/lib/format';
import { useLibraries } from '@/lib/queries';
import { folderHref, withView } from '@/lib/routes';
import { useChrome } from '@/store/chrome';
import { useLocation } from '@/store/location';
import { usePrefs } from '@/store/prefs';
import { toast } from '@/store/toasts';
import { useViewer } from '@/store/viewer';
import { Button, EmptyState, Spinner } from '@/components/ui';
import { useIsMobile } from '@/lib/useMedia';
import { useMediaActions } from './actions';
import { FolderHeaderActions } from '@/features/devices/FolderActions';
import { useGridSource } from './grid/source';
import { ArchiveCard, FolderCard, RootCard } from './FolderCards';
import { FolderPicker } from './FolderPicker';
import { MediaGrid, type MediaGridHandle } from './MediaGrid';
import { SelectionBar } from './SelectionBar';

export const ScrollContext = createContext<RefObject<HTMLDivElement | null>>({ current: null });
export const useScrollRef = () => useContext(ScrollContext);

export type GalleryMode = 'folder' | 'library' | 'timeline' | 'favorites' | 'search';

function SectionHeading({ children, count }: { children: React.ReactNode; count?: number }) {
  return (
    <div className="mb-3 flex items-baseline gap-2">
      <h2 className="text-[13px] font-semibold tracking-[0.06em] text-faint uppercase">{children}</h2>
      {count !== undefined && <span className="text-xs text-faint">{count}</span>}
    </div>
  );
}

function TempBanner({ folderId }: { folderId: string }) {
  const t = useT();
  const lang = useLang();
  const backend = useBackend();
  const qc = useQueryClient();
  const nav = useNavigate();
  const libs = useLibraries();
  const temp = libs.data?.temp.find((f) => f.id === folderId);
  const [pick, setPick] = useState(false);
  if (!temp) return null;
  return (
    <motion.div
      initial={{ opacity: 0, y: -10 }}
      animate={{ opacity: 1, y: 0 }}
      className="mb-6 flex flex-wrap items-center gap-3 rounded-2xl border border-[color-mix(in_oklab,var(--warn)_35%,transparent)] bg-[color-mix(in_oklab,var(--warn)_9%,transparent)] px-4 py-3"
    >
      <Clock3 className="size-5 shrink-0 text-warn" />
      <div className="min-w-[min(calc(100%-2.5rem),16rem)] flex-1">
        <div className="text-sm font-medium">{t('temp.banner', { name: temp.sourceName ?? temp.name })}</div>
        {temp.expiresAt && <div className="text-xs text-dim">{t('temp.expires', { time: durationUntil(temp.expiresAt, lang) })}</div>}
      </div>
      <Button size="sm" variant="ghost" onClick={() => backend.extendTemp(temp.id, 24).then(() => qc.invalidateQueries({ queryKey: ['libraries'] }))}>
        {t('temp.extend')}
      </Button>
      <Button size="sm" variant="ghost" onClick={async () => (await backend.removeFolder(temp.id), nav('/'))}>
        {t('temp.close')}
      </Button>
      <Button size="sm" variant="primary" onClick={() => setPick(true)}>
        {t('temp.keep')}
      </Button>
      <FolderPicker
        open={pick}
        title={t('temp.keepTitle')}
        confirmLabel={t('temp.keep')}
        onClose={() => setPick(false)}
        onPick={async (target) => {
          setPick(false);
          try {
            const r = await backend.keepTemp(temp.id, target);
            await qc.invalidateQueries({ queryKey: ['libraries'] });
            toast.success(t('temp.kept'));
            nav(folderHref(r.folderId, r.dir));
          } catch (e) {
            toast.error(t('common.error'), (e as Error).message);
          }
        }}
      />
    </motion.div>
  );
}

export function GalleryPage({ mode }: { mode: GalleryMode }) {
  const t = useT();
  const backend = useBackend();
  const info = useInfo();
  const params = useParams();
  const [sp] = useSearchParams();
  const routerLoc = useRouterLocation();
  const nav = useNavigate();
  const sort = usePrefs((s) => s.sort);
  const order = usePrefs((s) => s.order);
  const kind = usePrefs((s) => s.kind);
  const recursive = usePrefs((s) => s.recursive);
  const scrollRef = useScrollRef();
  const setChrome = useChrome((s) => s.set);
  const setView = useLocation((s) => s.setView);
  const actions = useMediaActions('selection');
  const gridHandle = useRef<MediaGridHandle | null>(null);
  const [showAll, setShowAll] = useState(false);
  const mobile = useIsMobile();

  const dir = mode === 'folder' ? (params['*'] ?? '').split('/').map(decodeURIComponent).join('/') : '';
  const q = sp.get('q') ?? '';

  const query: BrowseQuery = useMemo(() => {
    const base = { sort, order, kind };
    switch (mode) {
      case 'folder':
        return { ...base, folderId: params.folderId, dir, recursive };
      case 'library':
        return { ...base, libraryId: params.libraryId, recursive: recursive || showAll };
      case 'timeline':
        return { ...base, sort: 'takenAt', order: 'desc', recursive: true };
      case 'favorites':
        return { ...base, kind: 'favorites', recursive: true };
      case 'search':
        return { ...base, q, recursive: true };
    }
  }, [mode, params.folderId, params.libraryId, dir, sort, order, kind, recursive, q, showAll]);

  const source = useGridSource(mode !== 'search' || !!q ? query : null);
  const meta = source?.meta ?? null;
  const total = source?.total ?? 0;
  const loading = !!source && (!source.ready || !meta) && !source.error;
  const crumbs = useMemo(() => meta?.crumbs ?? [], [meta?.crumbs]);
  const target = meta?.target ?? null;
  const title =
    mode === 'timeline' ? t('nav.timeline') : mode === 'favorites' ? t('nav.favorites') : mode === 'search' ? `“${q}”` : (crumbs[crumbs.length - 1]?.label ?? '');

  useEffect(() => {
    setChrome({ crumbs: mode === 'folder' || mode === 'library' ? crumbs : [], title, viewControls: true });
  }, [crumbs, title, mode, setChrome]);

  useEffect(() => {
    setView({ target, targetLabel: crumbs[crumbs.length - 1]?.label ?? '', source });
  }, [target, crumbs, source, setView]);

  useEffect(() => () => setView({ target: null, targetLabel: '', source: null }), [setView]);

  // keep the grid scrolled to the item the viewer is showing (for the closing animation)
  const activeId = useViewer((s) => s.activeId);
  useEffect(() => {
    if (activeId) gridHandle.current?.scrollToItem(activeId);
  }, [activeId]);

  const open = useCallback(
    (item: MediaItem) => {
      useViewer.getState().setActive(item.id);
      requestAnimationFrame(() => nav({ search: withView(routerLoc.search, item.id) }, { state: { viewer: true } }));
    },
    [nav, routerLoc.search],
  );

  const slideshowAll = async () => {
    if (!source) return;
    const list = await source.itemsInRange(0, Math.min(source.total, 5000) - 1);
    void actions.run('slideshow', list.filter((x) => x.kind === 'image' || x.kind === 'video'));
  };

  const folders = meta?.folders ?? [];
  const roots = meta?.roots ?? [];
  const archives = meta?.archives ?? [];
  const isTemp = mode === 'folder' && !!params.folderId;
  const photos = source?.kindCounts.image ?? 0;
  const videos = source?.kindCounts.video ?? 0;
  const empty = !!source && source.ready && !!meta && !total && !folders.length && !roots.length && !archives.length;

  return (
    <div className="mx-auto w-full max-w-[1800px] px-3 pt-4 pb-28 sm:px-7 sm:pt-6">
      {isTemp && <TempBanner folderId={params.folderId!} />}

      {/* Page header */}
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ type: 'spring', stiffness: 260, damping: 30 }}
        className="mb-4 flex flex-wrap items-end gap-x-6 gap-y-3 sm:mb-7"
      >
        <div className="min-w-0 flex-1">
          <h1 className="flex items-center gap-2.5 truncate font-display text-[22px] font-bold tracking-tight sm:gap-3 sm:text-[34px]">
            {mode === 'timeline' && <CalendarDays className="size-6 text-accent sm:size-7" />}
            {mode === 'favorites' && <Heart className="size-6 text-[var(--accent-2)] sm:size-7" />}
            {mode === 'search' && <Search className="size-6 text-accent sm:size-7" />}
            <span className="truncate">{title || ' '}</span>
          </h1>
          <p className="mt-1 text-[13px] text-dim tabular-nums sm:mt-1.5 sm:text-sm">
            {loading ? t('common.loading') : `${t('common.items', { n: total })}${photos || videos ? ` · ${t('home.stats', { photos, videos })}` : ''}`}
          </p>
        </div>
        {total > 0 && (
          <div className="flex items-center gap-2">
            <Button variant="secondary" size="sm" icon={<Play className="size-4" />} onClick={() => void slideshowAll()}>
              {t('nav.slideshow')}
            </Button>
            {mode === 'folder' && params.folderId && <FolderHeaderActions folderId={params.folderId} dir={dir} name={title} />}
          </div>
        )}
      </motion.div>
      {mode === 'timeline' && <MemoriesStrip className="mb-5" />}

      {loading && (
        <div className="grid h-60 place-items-center">
          <Spinner className="size-7" />
        </div>
      )}

      {roots.length > 0 && (
        <section className="mb-8">
          <SectionHeading count={roots.length}>{t('gallery.libraryRoots')}</SectionHeading>
          <div className="grid grid-cols-1 gap-3 md:grid-cols-2 2xl:grid-cols-3">
            {roots.map((f, i) => (
              <RootCard key={f.id} f={f} index={i} />
            ))}
          </div>
          {!query.recursive && (
            <Button variant="ghost" size="sm" className="mt-3" icon={<FolderOpen className="size-4" />} onClick={() => setShowAll(true)}>
              {t('gallery.showAll')}
            </Button>
          )}
        </section>
      )}

      {folders.length > 0 && (
        <section className="mb-9">
          <SectionHeading count={folders.length}>{t('gallery.folders')}</SectionHeading>
          <div className="grid grid-cols-2 gap-3.5 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5 2xl:grid-cols-6">
            {folders.map((f, i) => (
              <FolderCard
                key={f.dir}
                folderId={params.folderId ?? target?.folderId ?? ''}
                f={f}
                index={i}
                onSlideshow={async () => {
                  const r = await backend.browse({ folderId: params.folderId ?? target?.folderId, dir: f.dir, recursive: true, limit: 2000 });
                  useLocation.getState().setView({ target: { folderId: params.folderId ?? target?.folderId ?? '', dir: f.dir } });
                  void actions.run('slideshow', r.items.filter((x) => x.kind !== 'audio'));
                }}
              />
            ))}
          </div>
        </section>
      )}

      {archives.length > 0 && (
        <section className="mb-9">
          <SectionHeading count={archives.length}>{t('gallery.archives')}</SectionHeading>
          <div className="grid grid-cols-1 gap-3 md:grid-cols-2 2xl:grid-cols-3">
            {archives.map((a, i) => (
              <ArchiveCard key={a.id} item={a} index={i} />
            ))}
          </div>
        </section>
      )}

      {source && total > 0 && (
        <section className={cn(mobile ? '-mx-2.5' : 'pr-3')}>
          {(folders.length > 0 || archives.length > 0) && (
            <div className={cn(mobile && 'px-2.5')}>
              <SectionHeading count={total}>{t('gallery.media')}</SectionHeading>
            </div>
          )}
          <MediaGrid source={source} scrollRef={scrollRef} onOpen={open} handleRef={gridHandle} sortKey={query.sort} groups={query.sort === 'takenAt'} />
        </section>
      )}

      <AnimatePresence>
        {empty && (
          <EmptyState
            icon={mode === 'search' ? <Search /> : mode === 'favorites' ? <Heart /> : target ? <UploadCloud /> : <ImageOff />}
            title={mode === 'search' ? t('gallery.noResults') : t('gallery.empty')}
            body={target && info && !info.caps.readonly ? t('gallery.emptyHint') : undefined}
            action={
              mode === 'library' && roots.length ? (
                <Button variant="primary" icon={<Presentation className="size-4" />} onClick={() => setShowAll(true)}>
                  {t('gallery.showAll')}
                </Button>
              ) : undefined
            }
          />
        )}
      </AnimatePresence>

      <SelectionBar source={source} />
    </div>
  );
}
