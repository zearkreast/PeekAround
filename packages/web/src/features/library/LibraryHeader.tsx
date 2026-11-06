import { useRef, useState, type ReactNode } from 'react';
import { Link } from 'react-router';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { motion } from 'motion/react';
import {
  ArrowDownAZ,
  CalendarRange,
  ChevronRight,
  Clock3,
  Filter,
  Folder,
  FolderTree,
  Hash,
  HardDrive,
  Images,
  ImageOff,
  LayoutGrid,
  MoreHorizontal,
  Pencil,
  Play,
  RefreshCw,
} from 'lucide-react';
import type { Crumb, Library, SubFolder } from '@peekaround/shared';
import { useBackend, useInfo } from '@/backend';
import { useLang, useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { formatBytes, relativeTime } from '@/lib/format';
import { LibraryIcon } from '@/lib/icons';
import { folderHref, libraryHref } from '@/lib/routes';
import { usePrefs, type FolderSort, type LibraryViewMode } from '@/store/prefs';
import { toast } from '@/store/toasts';
import { Button, IconButton, MenuItem, MenuSep, Popover, Segmented } from '@/components/ui';
import { rangeLabel } from './AlbumCard';
import { useLibraryUi } from './LibraryHost';
import { heroProps } from './vt';

/** "all" (every item, recursively) vs "folders" (folder browsing) — remembered per key */
export function useViewMode(key: string, fallback: LibraryViewMode): [LibraryViewMode, (v: LibraryViewMode) => void] {
  const mode = usePrefs((s) => s.libraryView[key]) ?? fallback;
  const set = (v: LibraryViewMode) => {
    const p = usePrefs.getState();
    p.set({ libraryView: { ...p.libraryView, [key]: v } });
  };
  return [mode, set];
}

export function ViewToggle({ value, onChange, className }: { value: LibraryViewMode; onChange: (v: LibraryViewMode) => void; className?: string }) {
  const t = useT();
  return (
    <Segmented<LibraryViewMode>
      value={value}
      onChange={onChange}
      className={className}
      options={[
        { value: 'all', label: t('lib2.view.all'), icon: <LayoutGrid className="size-4" /> },
        { value: 'folders', label: t('lib2.view.folders'), icon: <FolderTree className="size-4" /> },
      ]}
    />
  );
}

export function sortFolders(list: SubFolder[], sort: FolderSort): SubFolder[] {
  const byName = (a: SubFolder, b: SubFolder) => a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: 'base' });
  const out = [...list];
  if (sort === 'date') out.sort((a, b) => (b.latest ?? -Infinity) - (a.latest ?? -Infinity) || byName(a, b));
  else if (sort === 'count') out.sort((a, b) => b.mediaCount - a.mediaCount || byName(a, b));
  else out.sort(byName);
  return out;
}

export function FolderSortMenu() {
  const t = useT();
  const sort = usePrefs((s) => s.folderSort);
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLButtonElement>(null);
  const opts: { v: FolderSort; label: string; icon: ReactNode }[] = [
    { v: 'name', label: t('lib2.folders.byName'), icon: <ArrowDownAZ /> },
    { v: 'date', label: t('lib2.folders.byDate'), icon: <CalendarRange /> },
    { v: 'count', label: t('lib2.folders.byCount'), icon: <Hash /> },
  ];
  return (
    <>
      <button
        ref={anchor}
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="inline-flex h-7 items-center gap-1.5 rounded-lg px-2 text-xs text-dim transition-colors hover:bg-surface-2 hover:text-fg [&>svg]:size-3.5"
        aria-label={t('lib2.folders.sort')}
        title={t('lib2.folders.sort')}
      >
        {opts.find((o) => o.v === sort)?.icon}
        {opts.find((o) => o.v === sort)?.label}
      </button>
      <Popover open={open} onClose={() => setOpen(false)} anchor={anchor} align="end">
        {opts.map((o) => (
          <MenuItem
            key={o.v}
            icon={o.icon}
            checked={sort === o.v}
            onClick={() => {
              usePrefs.getState().set({ folderSort: o.v });
              setOpen(false);
            }}
          >
            {o.label}
          </MenuItem>
        ))}
      </Popover>
    </>
  );
}

function Dot() {
  return <span className="text-faint">·</span>;
}

/** compact numbers row: items · photos · videos | size · range · folders · last scan */
function StatsLine({ libId, fallbackCount }: { libId: string; fallbackCount: number }) {
  const t = useT();
  const lang = useLang();
  const backend = useBackend();
  const stats = useQuery({
    queryKey: ['libraries', backend.id, 'stats', libId],
    queryFn: () => backend.libraryStats!(libId),
    enabled: !!backend.libraryStats,
    staleTime: 5_000,
    // poll while the library is being scanned / indexed
    refetchInterval: (q) => (q.state.data && (q.state.data.scanning || q.state.data.pending > 0) ? 4000 : false),
  });
  const s = stats.data;
  if (!s) return <p className="mt-1 text-sm text-white/75 tabular-nums">{t('lib2.stats.items', { n: fallbackCount.toLocaleString() })}</p>;
  const range = rangeLabel(s.from, s.to, lang);
  return (
    <div className="mt-1.5 flex flex-col gap-1 text-[13px] text-white/80 tabular-nums sm:text-sm" data-testid="library-stats">
      <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5">
        <span className="font-medium text-white">{t('lib2.stats.items', { n: s.items.toLocaleString() })}</span>
        {s.photos > 0 && (
          <>
            <Dot />
            <span>{t('lib2.stats.photos', { n: s.photos.toLocaleString() })}</span>
          </>
        )}
        {s.videos > 0 && (
          <>
            <Dot />
            <span>{t('lib2.stats.videos', { n: s.videos.toLocaleString() })}</span>
          </>
        )}
        {s.audio > 0 && (
          <>
            <Dot />
            <span>{t('lib2.stats.audio', { n: s.audio.toLocaleString() })}</span>
          </>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-0.5 text-white/65 [&_svg]:size-3.5 [&_svg]:shrink-0">
        {s.bytes > 0 && (
          <span className="inline-flex items-center gap-1.5">
            <HardDrive />
            {t('lib2.stats.size', { size: formatBytes(s.bytes) })}
          </span>
        )}
        {range && (
          <span className="inline-flex items-center gap-1.5">
            <CalendarRange />
            {range}
          </span>
        )}
        {s.folders > 0 && (
          <span className="inline-flex items-center gap-1.5">
            <Folder />
            {t('lib2.stats.folders', { n: s.folders })}
          </span>
        )}
        <span className="inline-flex items-center gap-1.5">
          {s.scanning ? <RefreshCw className="animate-spin" /> : <Clock3 />}
          {s.scanning ? t('lib2.stats.scanning') : s.scannedAt ? t('lib2.stats.scanned', { time: relativeTime(s.scannedAt, lang) }) : t('lib2.stats.never')}
        </span>
        {!s.scanning && s.pending > 0 && (
          <span className="inline-flex items-center gap-1.5 text-white/80">
            <span className="size-1.5 animate-pulse rounded-full bg-[var(--accent)]" />
            {t('lib2.stats.indexing', { n: s.pending.toLocaleString() })}
          </span>
        )}
      </div>
    </div>
  );
}

/** "⋯" menu of a library: edit, exclusions, automatic cover */
function LibraryMenu({ lib }: { lib: Library }) {
  const t = useT();
  const backend = useBackend();
  const qc = useQueryClient();
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLButtonElement>(null);
  const ui = useLibraryUi.getState();
  const rules = lib.options?.exclude.length ?? 0;
  return (
    <>
      <IconButton ref={anchor} label={t('lib2.more')} active={open} className="border border-white/20 bg-black/35 !text-white backdrop-blur-md hover:bg-black/55" onClick={() => setOpen((o) => !o)}>
        <MoreHorizontal />
      </IconButton>
      <Popover open={open} onClose={() => setOpen(false)} anchor={anchor} align="end">
        <MenuItem
          icon={<Pencil />}
          onClick={() => {
            setOpen(false);
            ui.edit(lib);
          }}
        >
          {t('lib2.edit')}
        </MenuItem>
        {backend.libraryStats && (
        <MenuItem
          icon={<Filter />}
          hint={rules ? t('lib2.excluded', { n: rules }) : undefined}
          onClick={() => {
            setOpen(false);
            ui.exclude(lib);
          }}
        >
          {t('lib2.exclusions')}
        </MenuItem>
        )}
        {lib.coverId && (
          <>
            <MenuSep />
            <MenuItem
              icon={<ImageOff />}
              onClick={async () => {
                setOpen(false);
                await backend.updateLibrary(lib.id, { coverId: null });
                await qc.invalidateQueries({ queryKey: ['libraries'] });
              }}
            >
              {t('lib2.resetCover')}
            </MenuItem>
          </>
        )}
      </Popover>
    </>
  );
}

/**
 * Library page banner: cover photo (picked or newest), icon, name, numbers, actions and the
 * All / Folders switch. The cover is the morph target of the library card (View Transitions).
 */
export function LibraryHeader({
  lib,
  coverId,
  view,
  onView,
  onSlideshow,
  canSlideshow,
}: {
  lib: Library;
  coverId: string | null;
  view: LibraryViewMode;
  onView: (v: LibraryViewMode) => void;
  onSlideshow: () => void;
  canSlideshow: boolean;
}) {
  const t = useT();
  const backend = useBackend();
  const info = useInfo();
  const writable = !!info && !info.caps.readonly;
  const [scanning, setScanning] = useState(false);
  const hero = heroProps(`lib-${lib.id}`);
  const morph = !!hero.style.viewTransitionName;
  return (
    <div className="mb-5 sm:mb-7">
      <motion.div
        initial={morph ? false : { opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ type: 'spring', stiffness: 260, damping: 30 }}
        className="relative isolate overflow-hidden rounded-3xl border border-line bg-neutral-900"
        data-testid="library-header"
      >
        <div className="absolute inset-0 -z-10" {...hero}>
          {coverId ? (
            <>
              {/* the card's 640 thumb is cached → instant; the sharp one fades in over it */}
              <img src={backend.thumbUrl(coverId, 640)} alt="" draggable={false} className="absolute inset-0 size-full scale-105 object-cover blur-[2px]" />
              <img
                src={backend.thumbUrl(coverId, 1600)}
                alt=""
                draggable={false}
                decoding="async"
                onLoad={(e) => (e.currentTarget.dataset.loaded = '1')}
                className="fade-img lib-cover absolute inset-0 size-full object-cover"
              />
            </>
          ) : (
            <div className="absolute inset-0" style={{ background: `linear-gradient(135deg, ${lib.color}, color-mix(in oklab, ${lib.color} 25%, var(--bg-elev)))` }} />
          )}
          <div className="absolute inset-0 bg-gradient-to-t from-black/85 via-black/45 to-black/10" />
          <div className="absolute inset-0 bg-gradient-to-r from-black/50 to-transparent" />
        </div>
        {/* while the cover morphs in (it is drawn on top), the text waits and fades in after it */}
        <motion.div
          initial={morph ? { opacity: 0 } : false}
          animate={{ opacity: 1 }}
          transition={{ duration: 0.25, delay: morph ? 0.32 : 0 }}
          className="flex min-h-[188px] flex-col justify-end gap-4 p-4 text-white sm:min-h-[232px] sm:flex-row sm:items-end sm:p-6"
        >
          <div className="flex min-w-0 flex-1 items-end gap-3.5 sm:gap-4">
            <span className="grid size-11 shrink-0 place-items-center rounded-2xl shadow-lg ring-1 ring-white/20 sm:size-14" style={{ background: lib.color }}>
              <LibraryIcon name={lib.icon} className="size-5 sm:size-7" />
            </span>
            <div className="min-w-0">
              <h1 className="truncate font-display text-[24px] leading-tight font-bold tracking-tight sm:text-[34px]">{lib.name}</h1>
              <StatsLine libId={lib.id} fallbackCount={lib.mediaCount} />
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-1.5">
            {canSlideshow && (
              <Button variant="ghost" size="sm" icon={<Play className="size-4" />} onClick={onSlideshow} className="border border-white/20 bg-black/35 !text-white backdrop-blur-md hover:bg-black/55">
                {t('nav.slideshow')}
              </Button>
            )}
            {backend.rescanLibrary && (
              <IconButton
                label={t('lib2.rescan')}
                className="border border-white/20 bg-black/35 !text-white backdrop-blur-md hover:bg-black/55"
                onClick={async () => {
                  setScanning(true);
                  try {
                    await backend.rescanLibrary!(lib.id);
                    toast.info(t('lib2.rescanStarted'));
                  } finally {
                    setTimeout(() => setScanning(false), 1200);
                  }
                }}
              >
                <RefreshCw className={cn(scanning && 'animate-spin')} />
              </IconButton>
            )}
            {writable && <LibraryMenu lib={lib} />}
          </div>
        </motion.div>
      </motion.div>
      <div className="mt-4 flex flex-wrap items-center justify-between gap-3">
        <ViewToggle value={view} onChange={onView} />
      </div>
    </div>
  );
}

/** Folder page header: breadcrumb (drop targets), title, counts, All / Folders switch, actions. */
export function FolderHeader({
  crumbs,
  title,
  sub,
  view,
  onView,
  actions,
  heroKey,
  coverId,
}: {
  crumbs: Crumb[];
  title: string;
  sub: ReactNode;
  view: LibraryViewMode;
  onView: (v: LibraryViewMode) => void;
  actions?: ReactNode;
  heroKey?: string;
  coverId?: string | null;
}) {
  const backend = useBackend();
  const parents = crumbs.slice(0, -1);
  const hero = heroKey ? heroProps(heroKey) : undefined;
  return (
    <motion.div
      initial={hero?.style.viewTransitionName ? false : { opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ type: 'spring', stiffness: 260, damping: 30 }}
      className="mb-5 sm:mb-7"
    >
      {parents.length > 0 && (
        <nav className="no-scrollbar -mx-1 mb-2 flex items-center gap-0.5 overflow-x-auto text-[13px] text-dim" aria-label="breadcrumb" data-testid="folder-crumbs">
          {parents.map((c, i) => (
            <span key={`${i}-${c.label}`} className="flex shrink-0 items-center gap-0.5">
              {i > 0 && <ChevronRight className="size-3.5 text-faint" />}
              <Link
                to={c.folderId ? folderHref(c.folderId, c.dir) : c.libraryId ? libraryHref(c.libraryId) : '/'}
                data-drop-folder={c.folderId}
                data-drop-dir={c.dir ?? ''}
                data-drop-label={c.label}
                className="rounded-md px-1 py-0.5 transition-colors hover:bg-surface-2 hover:text-fg"
              >
                {c.label}
              </Link>
            </span>
          ))}
          <ChevronRight className="size-3.5 shrink-0 text-faint" />
        </nav>
      )}
      <div className="flex flex-wrap items-end gap-x-6 gap-y-3">
        {/* phones: a full row (the actions wrap below); flex-1 alone would squeeze the title to nothing */}
        <div className="flex min-w-0 grow basis-full items-center gap-3 sm:basis-0">
          {coverId !== undefined && (
            <span className="size-11 shrink-0 overflow-hidden rounded-xl border border-line bg-surface sm:size-14" {...hero}>
              {coverId ? (
                <img src={backend.thumbUrl(coverId, 320)} alt="" draggable={false} className="size-full object-cover" />
              ) : (
                <span className="grid size-full place-items-center text-accent">
                  <Images className="size-5" />
                </span>
              )}
            </span>
          )}
          <div className="min-w-0">
            <h1 className="truncate font-display text-[22px] font-bold tracking-tight sm:text-[32px]">{title || ' '}</h1>
            <p className="mt-0.5 text-[13px] text-dim tabular-nums sm:text-sm">{sub}</p>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <ViewToggle value={view} onChange={onView} />
          {actions}
        </div>
      </div>
    </motion.div>
  );
}
