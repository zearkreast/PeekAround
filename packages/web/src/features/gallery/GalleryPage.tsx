import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type RefObject } from 'react';
import { useNavigate, useParams, useSearchParams, useLocation as useRouterLocation } from 'react-router';
import { AnimatePresence, motion } from 'motion/react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { CalendarDays, Clock3, Download, FolderOpen, Heart, ImageOff, Play, Presentation, Search, UploadCloud } from 'lucide-react';
import type { BrowseQuery, MediaItem } from '@peekaround/shared';
import { useBackend, useInfo } from '@/backend';
import { useLang, useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { durationUntil, formatMonth } from '@/lib/format';
import { useBrowse, useLibraries } from '@/lib/queries';
import { folderHref, withView } from '@/lib/routes';
import { useChrome } from '@/store/chrome';
import { useLocation } from '@/store/location';
import { usePrefs } from '@/store/prefs';
import { toast } from '@/store/toasts';
import { useViewer } from '@/store/viewer';
import { Button, EmptyState, Spinner } from '@/components/ui';
import { download, useMediaActions } from './actions';
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
      <div className="min-w-0 flex-1">
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

/** Year/month scrubber for the timeline. */
function DateRail({ onJump }: { onJump: (key: string) => void }) {
  const backend = useBackend();
  const lang = useLang();
  const q = useQuery({ queryKey: ['timeline', backend.id], queryFn: () => backend.timeline() });
  const buckets = q.data?.buckets ?? [];
  if (buckets.length < 2) return null;
  let lastYear = '';
  return (
    <div className="no-scrollbar fixed top-[calc(var(--topbar-h)+16px)] right-3 bottom-6 z-10 hidden w-20 flex-col items-end overflow-y-auto py-2 xl:flex">
      {buckets.map((b) => {
        const y = b.key.slice(0, 4);
        const showYear = y !== lastYear;
        lastYear = y;
        return (
          <div key={b.key} className="flex flex-col items-end">
            {showYear && <div className="mt-2 mb-0.5 font-display text-xs font-semibold text-dim">{y}</div>}
            <button type="button" onClick={() => onJump(b.key)} className="rounded-md px-1.5 py-0.5 text-[11px] text-faint capitalize transition-colors hover:bg-surface-2 hover:text-fg" title={`${formatMonth(b.key, lang)} · ${b.count}`}>
              {formatMonth(b.key, lang).split(' ')[0].slice(0, 3)}
            </button>
          </div>
        );
      })}
    </div>
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
  const prefs = usePrefs();
  const scrollRef = useScrollRef();
  const setChrome = useChrome((s) => s.set);
  const setView = useLocation((s) => s.setView);
  const actions = useMediaActions();
  const gridHandle = useRef<MediaGridHandle | null>(null);
  const [showAll, setShowAll] = useState(false);

  const dir = mode === 'folder' ? (params['*'] ?? '').split('/').map(decodeURIComponent).join('/') : '';
  const q = sp.get('q') ?? '';

  const query: BrowseQuery = useMemo(() => {
    const base = { sort: prefs.sort, order: prefs.order, kind: prefs.kind };
    switch (mode) {
      case 'folder':
        return { ...base, folderId: params.folderId, dir, recursive: prefs.recursive };
      case 'library':
        return { ...base, libraryId: params.libraryId, recursive: prefs.recursive || showAll };
      case 'timeline':
        return { ...base, sort: 'takenAt', order: 'desc', recursive: true };
      case 'favorites':
        return { ...base, kind: 'favorites', recursive: true };
      case 'search':
        return { ...base, q, recursive: true };
    }
  }, [mode, params.folderId, params.libraryId, dir, prefs.sort, prefs.order, prefs.kind, prefs.recursive, q, showAll]);

  const browse = useBrowse(query, mode !== 'search' || !!q);
  const { first, items, total } = browse;
  const crumbs = first?.crumbs ?? [];
  const title =
    mode === 'timeline' ? t('nav.timeline') : mode === 'favorites' ? t('nav.favorites') : mode === 'search' ? `“${q}”` : (crumbs[crumbs.length - 1]?.label ?? '');

  useEffect(() => {
    setChrome({ crumbs: mode === 'folder' || mode === 'library' ? crumbs : [], title, viewControls: true });
  }, [crumbs, title, mode, setChrome]);

  useEffect(() => {
    setView({ target: first?.target ?? null, targetLabel: crumbs[crumbs.length - 1]?.label ?? '', items });
  }, [first?.target, crumbs, items, setView]);

  useEffect(() => () => setView({ target: null, targetLabel: '', items: [] }), [setView]);

  // keep the grid scrolled to the item the viewer is showing (for the closing animation)
  const activeId = useViewer((s) => s.activeId);
  useEffect(() => {
    if (activeId) gridHandle.current?.scrollToItem(activeId);
  }, [activeId]);

  const open = useCallback(
    (item: MediaItem) => {
      useViewer.getState().setActive(item.id);
      requestAnimationFrame(() => nav({ search: withView(routerLoc.search, item.id) }));
    },
    [nav, routerLoc.search],
  );

  const loadMore = useCallback(() => {
    if (browse.hasNextPage && !browse.isFetchingNextPage) void browse.fetchNextPage();
  }, [browse]);

  const folders = first?.folders ?? [];
  const roots = first?.roots ?? [];
  const archives = first?.archives ?? [];
  const isTemp = mode === 'folder' && !!params.folderId;
  const photos = items.filter((i) => i.kind === 'image').length;
  const videos = items.filter((i) => i.kind === 'video').length;
  const empty = !browse.isLoading && !items.length && !folders.length && !roots.length && !archives.length;

  return (
    <div className="mx-auto w-full max-w-[1800px] px-2.5 pt-5 pb-28 sm:px-7 sm:pt-6">
      {isTemp && <TempBanner folderId={params.folderId!} />}

      {/* Page header */}
      <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={{ type: 'spring', stiffness: 260, damping: 30 }} className="mb-7 flex flex-wrap items-end gap-x-6 gap-y-3">
        <div className="min-w-0 flex-1">
          <h1 className="flex items-center gap-3 truncate font-display text-2xl font-bold tracking-tight sm:text-[34px]">
            {mode === 'timeline' && <CalendarDays className="size-7 text-accent" />}
            {mode === 'favorites' && <Heart className="size-7 text-[var(--accent-2)]" />}
            {mode === 'search' && <Search className="size-7 text-accent" />}
            <span className="truncate">{title || ' '}</span>
          </h1>
          <p className="mt-1.5 text-sm text-dim tabular-nums">
            {browse.isLoading ? t('common.loading') : `${t('common.items', { n: total })}${photos || videos ? ` · ${t('home.stats', { photos, videos })}` : ''}`}
          </p>
        </div>
        {items.length > 0 && (
          <div className="flex items-center gap-2">
            <Button variant="secondary" size="sm" icon={<Play className="size-4" />} onClick={() => actions.slideshow(items, first?.target)}>
              {t('nav.slideshow')}
            </Button>
            {mode === 'folder' && params.folderId && (
              <Button variant="ghost" size="sm" icon={<Download className="size-4" />} onClick={() => download(backend.folderDownloadUrl(params.folderId!, dir, title))}>
                <span className="hidden lg:inline">{t('gallery.downloadFolder')}</span>
              </Button>
            )}
          </div>
        )}
      </motion.div>

      {browse.isLoading && (
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
                folderId={params.folderId ?? first?.target?.folderId ?? ''}
                f={f}
                index={i}
                onSlideshow={async () => {
                  const r = await backend.browse({ folderId: params.folderId ?? first?.target?.folderId, dir: f.dir, recursive: true, limit: 2000 });
                  actions.slideshow(r.items.filter((x) => x.kind !== 'audio'), { folderId: params.folderId ?? first?.target?.folderId ?? '', dir: f.dir });
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

      {items.length > 0 && (
        <section className={cn(mode === 'timeline' && 'xl:pr-20')}>
          {(folders.length > 0 || archives.length > 0) && <SectionHeading count={total}>{t('gallery.media')}</SectionHeading>}
          <MediaGrid
            items={items}
            hasMore={!!browse.hasNextPage}
            loadMore={loadMore}
            scrollRef={scrollRef}
            groupBy={mode === 'timeline' ? 'month' : null}
            onOpen={open}
            handleRef={gridHandle}
          />
          {browse.isFetchingNextPage && (
            <div className="grid h-20 place-items-center">
              <Spinner />
            </div>
          )}
        </section>
      )}

      {mode === 'timeline' && <DateRail onJump={(k) => gridHandle.current?.scrollToGroup(k)} />}

      <AnimatePresence>
        {empty && (
          <EmptyState
            icon={mode === 'search' ? <Search /> : mode === 'favorites' ? <Heart /> : first?.target ? <UploadCloud /> : <ImageOff />}
            title={mode === 'search' ? t('gallery.noResults') : t('gallery.empty')}
            body={first?.target && info && !info.caps.readonly ? t('gallery.emptyHint') : undefined}
            action={
              mode === 'folder' && items.length === 0 && !first?.target ? undefined : mode === 'library' && roots.length ? (
                <Button variant="primary" icon={<Presentation className="size-4" />} onClick={() => setShowAll(true)}>
                  {t('gallery.showAll')}
                </Button>
              ) : undefined
            }
          />
        )}
      </AnimatePresence>

      <SelectionBar items={items} target={first?.target ?? null} />
    </div>
  );
}
