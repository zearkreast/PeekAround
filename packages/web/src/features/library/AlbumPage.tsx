import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate, useParams, useLocation as useRouterLocation } from 'react-router';
import { useQueryClient } from '@tanstack/react-query';
import { motion } from 'motion/react';
import { ArrowDownWideNarrow, ArrowUpNarrowWide, BookImage, Download, GripVertical, ImageOff, MoreHorizontal, Pencil, Play, Share2, Trash2 } from 'lucide-react';
import type { AlbumOrder, BrowseQuery, MediaItem } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useLang, useT } from '@/i18n';
import { withView } from '@/lib/routes';
import { useChrome } from '@/store/chrome';
import { useLocation } from '@/store/location';
import { usePrefs } from '@/store/prefs';
import { toast } from '@/store/toasts';
import { useViewer } from '@/store/viewer';
import { Button, ConfirmDialog, EmptyState, IconButton, MenuItem, MenuLabel, MenuSep, Popover, Spinner } from '@/components/ui';
import { download, useMediaActions } from '@/features/gallery/actions';
import { useScrollRef } from '@/features/gallery/GalleryPage';
import { MediaGrid, type MediaGridHandle } from '@/features/gallery/MediaGrid';
import { SelectionBar } from '@/features/gallery/SelectionBar';
import { useGridSource } from '@/features/gallery/grid/source';
import { rangeLabel } from './AlbumCard';
import { albumsSupported, forgetAlbum, refreshAlbums, useAlbum, useAlbumContext } from './albums';
import { heroProps } from './vt';
import './library.css';

/** album name, editable in place (Enter / blur saves, Escape cancels) */
function AlbumTitle({ name, onSave }: { name: string; onSave: (n: string) => Promise<void> }) {
  const t = useT();
  const [edit, setEdit] = useState(false);
  const [value, setValue] = useState(name);
  useEffect(() => setValue(name), [name]);
  const commit = async () => {
    setEdit(false);
    const v = value.trim();
    if (v && v !== name) await onSave(v);
    else setValue(name);
  };
  if (edit)
    return (
      <input
        autoFocus
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onBlur={() => void commit()}
        onKeyDown={(e) => {
          if (e.key === 'Enter') void commit();
          if (e.key === 'Escape') {
            setValue(name);
            setEdit(false);
          }
        }}
        aria-label={t('album.rename')}
        className="w-full min-w-0 rounded-xl border border-[var(--accent)] bg-surface px-2 font-display text-[22px] font-bold tracking-tight outline-none sm:text-[32px]"
        data-testid="album-title-input"
      />
    );
  return (
    <button
      type="button"
      title={t('album.rename')}
      onClick={() => setEdit(true)}
      className="group/t -mx-2 flex max-w-full min-w-0 items-center gap-2 rounded-xl px-2 text-left transition-colors hover:bg-surface"
      data-testid="album-title"
    >
      <h1 className="truncate font-display text-[22px] font-bold tracking-tight sm:text-[32px]">{name}</h1>
      <Pencil className="size-4 shrink-0 text-faint opacity-0 transition-opacity group-hover/t:opacity-100" />
    </button>
  );
}

/** /album/:albumId — the album's items in the regular grid (viewer, selection, actions …) */
export default function AlbumPage() {
  const t = useT();
  const lang = useLang();
  const { albumId } = useParams();
  const backend = useBackend();
  const qc = useQueryClient();
  const nav = useNavigate();
  const routerLoc = useRouterLocation();
  const scrollRef = useScrollRef();
  const setChrome = useChrome((s) => s.set);
  const setView = useLocation((s) => s.setView);
  const kind = usePrefs((s) => s.kind);
  const actions = useMediaActions('selection');
  const album = useAlbum(albumId);
  const a = album.data;
  const gridHandle = useRef<MediaGridHandle | null>(null);
  const [menu, setMenu] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const anchor = useRef<HTMLButtonElement>(null);
  const order: AlbumOrder = a?.order ?? 'desc';

  const query: BrowseQuery | null = useMemo(
    () => (albumId && a ? { albumId, kind, sort: 'takenAt', order: order === 'asc' ? 'asc' : 'desc', byPosition: order === 'custom' || undefined } : null),
    [albumId, a, kind, order],
  );
  const source = useGridSource(query);
  const total = source?.total ?? 0;

  useEffect(() => setChrome({ crumbs: [], title: a?.name ?? t('album.title'), viewControls: true }), [setChrome, a?.name, t]);
  useEffect(() => setView({ target: null, targetLabel: a?.name ?? '', source }), [source, a?.name, setView]);
  useEffect(() => () => setView({ target: null, targetLabel: '', source: null }), [setView]);
  // album actions (remove from album, make cover, move) know the open album
  useEffect(() => {
    useAlbumContext.getState().set(a ?? null);
  }, [a]);
  useEffect(() => () => useAlbumContext.getState().set(null), []);

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

  if (!albumsSupported(backend)) return <EmptyState icon={<BookImage />} title={t('album.unsupported')} />;
  if (album.isError) return <EmptyState icon={<ImageOff />} title={t('common.error')} body={(album.error as Error).message} />;
  if (!a)
    return (
      <div className="grid h-[60vh] place-items-center">
        <Spinner className="size-7" />
      </div>
    );

  const save = async (patch: Parameters<NonNullable<typeof backend.updateAlbum>>[1]) => {
    try {
      await backend.updateAlbum!(a.id, patch);
      refreshAlbums(qc, [a.id]);
    } catch (e) {
      toast.error(t('common.error'), (e as Error).message);
    }
  };
  const slideshow = async () => {
    if (!source) return;
    const list = await source.itemsInRange(0, Math.min(source.total, 5000) - 1);
    void actions.run('slideshow', list.filter((x) => x.kind === 'image' || x.kind === 'video'));
  };
  const range = rangeLabel(a.from, a.to, lang);
  const hero = heroProps(`album-${a.id}`);

  return (
    <div className="mx-auto w-full max-w-[1800px] px-3 pt-4 pb-28 sm:px-7 sm:pt-6">
      <motion.div
        initial={hero.style.viewTransitionName ? false : { opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ type: 'spring', stiffness: 260, damping: 30 }}
        className="mb-5 flex flex-wrap items-end gap-x-6 gap-y-3 sm:mb-7"
        data-testid="album-header"
      >
        {/* phones: a full row (the actions wrap below); flex-1 alone would squeeze the title to nothing */}
        <div className="flex min-w-0 grow basis-full items-center gap-3.5 sm:basis-0 sm:gap-4">
          <span className="size-14 shrink-0 overflow-hidden rounded-2xl border border-line bg-surface shadow-sm sm:size-20" {...hero}>
            {a.coverId ? (
              <img src={backend.thumbUrl(a.coverId, 640)} alt="" draggable={false} className="size-full object-cover" />
            ) : (
              <span className="grid size-full place-items-center text-faint">
                <BookImage className="size-6" />
              </span>
            )}
          </span>
          <div className="min-w-0 flex-1">
            <AlbumTitle name={a.name} onSave={(name) => save({ name })} />
            <p className="mt-0.5 text-[13px] text-dim tabular-nums sm:text-sm">
              {t('album.items', { n: a.count })}
              {a.photos || a.videos ? ` · ${t('home.stats', { photos: a.photos, videos: a.videos })}` : ''}
              {range ? ` · ${range}` : ''}
            </p>
          </div>
        </div>
        <div className="flex items-center gap-2">
          {total > 0 && (
            <Button variant="secondary" size="sm" icon={<Play className="size-4" />} onClick={() => void slideshow()}>
              {t('nav.slideshow')}
            </Button>
          )}
          {total > 0 && backend.share && (
            <IconButton
              label={t('album.share')}
              onClick={async () => source && backend.share!((await source.itemsInRange(0, Math.min(source.total, 500) - 1)).map((i) => i.id))}
            >
              <Share2 />
            </IconButton>
          )}
          {total > 0 && backend.albumDownloadUrl && !backend.share && (
            <IconButton label={t('album.download')} onClick={() => download(backend.albumDownloadUrl!(a.id, a.name))}>
              <Download />
            </IconButton>
          )}
          <IconButton ref={anchor} label={t('lib2.more')} active={menu} onClick={() => setMenu((o) => !o)}>
            <MoreHorizontal />
          </IconButton>
          <Popover open={menu} onClose={() => setMenu(false)} anchor={anchor} align="end" className="w-60">
            <MenuLabel>{t('album.order')}</MenuLabel>
            {(
              [
                ['desc', <ArrowDownWideNarrow key="d" />],
                ['asc', <ArrowUpNarrowWide key="a" />],
                ['custom', <GripVertical key="c" />],
              ] as const
            ).map(([o, icon]) => (
              <MenuItem
                key={o}
                icon={icon}
                checked={order === o}
                onClick={() => {
                  setMenu(false);
                  void save({ order: o });
                }}
              >
                {t(`album.order.${o}` as const)}
              </MenuItem>
            ))}
            <MenuSep />
            {a.coverPicked && (
              <MenuItem
                icon={<ImageOff />}
                onClick={() => {
                  setMenu(false);
                  void save({ coverId: null });
                }}
              >
                {t('lib2.resetCover')}
              </MenuItem>
            )}
            <MenuItem
              icon={<Trash2 />}
              danger
              onClick={() => {
                setMenu(false);
                setConfirm(true);
              }}
            >
              {t('album.delete')}
            </MenuItem>
          </Popover>
        </div>
      </motion.div>

      {source && !source.ready && (
        <div className="grid h-60 place-items-center">
          <Spinner className="size-7" />
        </div>
      )}
      {source && source.ready && total === 0 && <EmptyState icon={<BookImage />} title={t('album.noItems')} body={t('album.noItemsHint')} />}
      {source && total > 0 && (
        <section className="pr-0 sm:pr-3">
          <MediaGrid source={source} scrollRef={scrollRef} onOpen={open} handleRef={gridHandle} sortKey="takenAt" groups={order !== 'custom'} />
        </section>
      )}

      <ConfirmDialog
        open={confirm}
        danger
        title={t('album.deleteConfirm', { name: a.name })}
        body={t('album.deleteBody')}
        confirmLabel={t('album.delete')}
        cancelLabel={t('common.cancel')}
        onClose={() => setConfirm(false)}
        onConfirm={async () => {
          const undo = forgetAlbum(a.id);
          try {
            await backend.deleteAlbum!(a.id);
          } catch (e) {
            undo();
            toast.error(t('common.error'), (e as Error).message);
            return;
          }
          refreshAlbums(qc);
          toast.success(t('album.deleted'));
          nav('/albums', { replace: true });
        }}
      />
      <SelectionBar source={source} />
    </div>
  );
}
