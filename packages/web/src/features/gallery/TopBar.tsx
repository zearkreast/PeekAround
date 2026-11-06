import { useEffect, useRef, useState } from 'react';
import { Link, useLocation as useRouterLocation, useNavigate, useSearchParams } from 'react-router';
import { AnimatePresence, motion } from 'motion/react';
import {
  ArrowDownWideNarrow,
  ArrowLeft,
  ArrowUpNarrowWide,
  ChevronRight,
  Film,
  FolderPlus,
  FolderUp,
  Grid2x2,
  Heart,
  Images,
  LayoutDashboard,
  Menu,
  Loader2,
  Plus,
  Rows3,
  Search,
  SlidersHorizontal,
  Upload,
} from 'lucide-react';
import type { KindFilter, SortKey } from '@peekaround/shared';
import { useBackend, useInfo } from '@/backend';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { folderHref, libraryHref } from '@/lib/routes';
import { useChrome } from '@/store/chrome';
import { useJobs } from '@/store/jobs';
import { useLocation as useGalleryLocation } from '@/store/location';
import { usePrefs } from '@/store/prefs';
import { useIsMobile } from '@/lib/useMedia';
import { BottomSheet, Button, IconButton, Input, MenuItem, MenuLabel, MenuSep, Modal, Popover, ProgressRing, Segmented, Slider } from '@/components/ui';
import { canGoBackInHistory } from '@/lib/backStack';
import { pickAndUpload } from '@/features/upload/actions';
import { AddMenuExtras } from '@/features/upload/AddMenuExtras';
import { JobsPanel } from './JobsPanel';
import { useQueryClient } from '@tanstack/react-query';

function Crumbs() {
  const crumbs = useChrome((s) => s.crumbs);
  const title = useChrome((s) => s.title);
  const mobile = useIsMobile();
  if (!crumbs.length || mobile) return <h1 className="truncate font-display text-[17px] font-semibold">{crumbs.length ? crumbs[crumbs.length - 1].label : title}</h1>;
  return (
    <nav className="flex min-w-0 items-center gap-1 text-[14px]">
      <AnimatePresence initial={false} mode="popLayout">
        {crumbs.map((c, i) => {
          const last = i === crumbs.length - 1;
          const to = c.folderId ? folderHref(c.folderId, c.dir) : c.libraryId ? libraryHref(c.libraryId) : '/';
          return (
            <motion.span
              key={`${i}-${c.label}`}
              layout
              initial={{ opacity: 0, x: -8 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: -8 }}
              transition={{ type: 'spring', stiffness: 500, damping: 40 }}
              className={cn('flex min-w-0 items-center gap-1', !last && 'shrink-[3]')}
            >
              {i > 0 && <ChevronRight className="size-3.5 shrink-0 text-faint" />}
              {last ? (
                <span className="truncate font-display text-[17px] font-semibold text-fg">{c.label}</span>
              ) : (
                <Link to={to} data-drop-folder={c.folderId} data-drop-dir={c.dir ?? ''} data-drop-label={c.label} className="truncate rounded-md px-1 text-dim hover:bg-surface-2 hover:text-fg">
                  {c.label}
                </Link>
              )}
            </motion.span>
          );
        })}
      </AnimatePresence>
    </nav>
  );
}

function SearchBox() {
  const t = useT();
  const nav = useNavigate();
  const [params] = useSearchParams();
  const [q, setQ] = useState(params.get('q') ?? '');
  const ref = useRef<HTMLInputElement>(null);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.key === 'k' && (e.metaKey || e.ctrlKey)) || (e.key === '/' && !/input|textarea/i.test((e.target as HTMLElement).tagName))) {
        e.preventDefault();
        ref.current?.focus();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);
  return (
    <form
      className="hidden w-56 md:block lg:w-64"
      onSubmit={(e) => {
        e.preventDefault();
        if (q.trim()) nav(`/search?q=${encodeURIComponent(q.trim())}`);
      }}
    >
      <Input ref={ref} value={q} onChange={(e) => setQ(e.target.value)} placeholder={t('nav.search')} icon={<Search />} className="app-no-drag" />
    </form>
  );
}

function ViewOptions() {
  const t = useT();
  const p = usePrefs();
  const mobile = useIsMobile();
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLButtonElement>(null);
  const sorts: SortKey[] = ['takenAt', 'name', 'mtime', 'size'];
  const body = (
    <>
      <MenuLabel>{t('gallery.filter.all')}</MenuLabel>
      <div className="px-1 pb-2">
        <Segmented<KindFilter>
          size="sm"
          value={p.kind}
          onChange={(kind) => p.set({ kind })}
          className="w-full [&>button]:flex-1"
          options={[
            { value: 'all', label: t('gallery.filter.all') },
            { value: 'image', label: '', icon: <Images />, title: t('gallery.filter.image') },
            { value: 'video', label: '', icon: <Film />, title: t('gallery.filter.video') },
            { value: 'favorites', label: '', icon: <Heart />, title: t('gallery.filter.favorites') },
          ]}
        />
      </div>
      <MenuSep />
      <MenuLabel>{t('gallery.sort')}</MenuLabel>
      {sorts.map((s) => (
        <MenuItem key={s} checked={p.sort === s} onClick={() => p.set({ sort: s })}>
          {t(`gallery.sort.${s}` as const)}
        </MenuItem>
      ))}
      <MenuSep />
      <MenuItem icon={<ArrowDownWideNarrow />} checked={p.order === 'desc'} onClick={() => p.set({ order: 'desc' })}>
        {t('gallery.order.desc')}
      </MenuItem>
      <MenuItem icon={<ArrowUpNarrowWide />} checked={p.order === 'asc'} onClick={() => p.set({ order: 'asc' })}>
        {t('gallery.order.asc')}
      </MenuItem>
      <MenuSep />
      <div className="flex flex-col gap-3 px-2.5 py-2">
        <Segmented
          size="sm"
          value={p.gridMode}
          onChange={(gridMode) => p.set({ gridMode })}
          className="w-full [&>button]:flex-1"
          options={[
            { value: 'justified', label: t('gallery.layout.justified'), icon: <Rows3 /> },
            { value: 'square', label: t('gallery.layout.square'), icon: <Grid2x2 /> },
          ]}
        />
        {mobile ? (
          <Segmented<string>
            size="md"
            value={String(p.gridZoomMobile)}
            onChange={(v) => p.set({ gridZoomMobile: Number(v) })}
            className="w-full [&>button]:flex-1"
            options={['0', '1', '2', '3'].map((v) => ({ value: v, label: '', icon: <span className="grid place-items-center">{densityIcon(Number(v))}</span>, title: t('gallery.size') }))}
          />
        ) : (
          <Slider label={t('gallery.size')} value={p.rowHeight} min={110} max={420} step={10} onChange={(rowHeight) => p.set({ rowHeight })} format={(v) => `${v}px`} />
        )}
      </div>
    </>
  );
  return (
    <>
      <IconButton ref={anchor} label={t('gallery.sort')} active={open} onClick={() => setOpen((o) => !o)}>
        <SlidersHorizontal />
      </IconButton>
      {mobile ? (
        <BottomSheet open={open} onClose={() => setOpen(false)} title={t('gallery.sort')}>
          <div className="px-3 [&>button]:min-h-11">{body}</div>
        </BottomSheet>
      ) : (
        <Popover open={open} onClose={() => setOpen(false)} anchor={anchor} align="end" className="w-72 p-2">
          {body}
        </Popover>
      )}
    </>
  );
}

/** tiny grid glyph for a density level (2 … 8 columns) */
function densityIcon(level: number) {
  const n = [2, 3, 5, 8][level] ?? 3;
  const cell = 16 / n;
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden>
      {Array.from({ length: n * Math.min(n, 3) }, (_, k) => (
        <rect key={k} x={(k % n) * cell + 0.5} y={Math.floor(k / n) * cell + 0.5} width={cell - 1} height={cell - 1} rx={0.6} fill="currentColor" />
      ))}
    </svg>
  );
}

function AddMenu() {
  const t = useT();
  const backend = useBackend();
  const qc = useQueryClient();
  const target = useGalleryLocation((s) => s.target);
  const [open, setOpen] = useState(false);
  const [mk, setMk] = useState(false);
  const [name, setName] = useState('');
  const anchor = useRef<HTMLButtonElement>(null);
  const nav = useNavigate();
  if (!target) return null;
  return (
    <>
      <Button ref={anchor} variant="primary" size="sm" icon={<Plus className="size-4" />} onClick={() => setOpen((o) => !o)} className="app-no-drag touch-target" aria-label={t('common.add')}>
        <span className="hidden sm:inline">{t('common.add')}</span>
      </Button>
      <Popover open={open} onClose={() => setOpen(false)} anchor={anchor} align="end">
        <MenuItem
          icon={<Upload />}
          onClick={() => {
            setOpen(false);
            pickAndUpload(backend, target, false);
          }}
        >
          {t('gallery.upload')}
        </MenuItem>
        <MenuItem
          icon={<FolderUp />}
          onClick={() => {
            setOpen(false);
            pickAndUpload(backend, target, true);
          }}
        >
          {t('gallery.uploadFolder')}
        </MenuItem>
        <AddMenuExtras target={target} close={() => setOpen(false)} />
        <MenuSep />
        <MenuItem
          icon={<FolderPlus />}
          onClick={() => {
            setOpen(false);
            setName('');
            setMk(true);
          }}
        >
          {t('gallery.newFolder')}
        </MenuItem>
      </Popover>
      <Modal
        open={mk}
        onClose={() => setMk(false)}
        title={t('gallery.newFolder')}
        width={400}
        footer={
          <>
            <Button variant="ghost" onClick={() => setMk(false)}>
              {t('common.cancel')}
            </Button>
            <Button
              variant="primary"
              disabled={!name.trim()}
              onClick={async () => {
                const r = await backend.mkdir(target.folderId, target.dir, name.trim());
                setMk(false);
                await qc.invalidateQueries({ queryKey: ['browse'] });
                nav(folderHref(r.folderId, r.dir));
              }}
            >
              {t('common.create')}
            </Button>
          </>
        }
      >
        <Input autoFocus value={name} onChange={(e) => setName(e.target.value)} placeholder={t('gallery.newFolderName')} />
      </Modal>
    </>
  );
}

function JobsButton() {
  const t = useT();
  const jobs = Object.values(useJobs((s) => s.jobs));
  const active = jobs.filter((j) => j.status === 'running' || j.status === 'queued');
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLButtonElement>(null);
  if (!jobs.length) return null;
  const running = active.find((j) => j.status === 'running');
  return (
    <>
      <IconButton ref={anchor} label={t('jobs.title')} active={open} onClick={() => setOpen((o) => !o)} className="relative">
        {running ? <ProgressRing value={running.progress} size={20} stroke={2.5} /> : active.length ? <Loader2 className="animate-spin" /> : <LayoutDashboard />}
        {!active.length && <span className="absolute top-2 right-2 size-2 rounded-full bg-ok" />}
      </IconButton>
      <Popover open={open} onClose={() => setOpen(false)} anchor={anchor} align="end" className="w-80 p-2">
        <JobsPanel onNavigate={() => setOpen(false)} />
      </Popover>
    </>
  );
}

function MobileSearch() {
  const t = useT();
  const nav = useNavigate();
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState('');
  return (
    <>
      <IconButton label={t('nav.search')} onClick={() => setOpen(true)} className="md:hidden">
        <Search />
      </IconButton>
      <AnimatePresence>
        {open && (
          <motion.form
            initial={{ opacity: 0, y: -8 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -8 }}
            className="absolute inset-x-0 top-0 z-20 flex h-full items-center gap-2 bg-bg px-3"
            onSubmit={(e) => {
              e.preventDefault();
              if (q.trim()) nav(`/search?q=${encodeURIComponent(q.trim())}`);
              setOpen(false);
            }}
          >
            <Input autoFocus value={q} onChange={(e) => setQ(e.target.value)} placeholder={t('nav.search')} icon={<Search />} className="flex-1" onBlur={() => !q && setOpen(false)} />
          </motion.form>
        )}
      </AnimatePresence>
    </>
  );
}

const ROOT_TABS = ['/', '/explore', '/library', '/settings'];

export function TopBar() {
  const t = useT();
  const viewControls = useChrome((s) => s.viewControls);
  const setChrome = useChrome((s) => s.set);
  const mobile = useIsMobile();
  const info = useInfo();
  const loc = useRouterLocation();
  const nav = useNavigate();
  const root = ROOT_TABS.includes(loc.pathname);
  return (
    <header
      className={cn(
        'app-drag relative z-10 flex shrink-0 items-center gap-2 border-b border-line px-3 sm:gap-3 sm:px-5',
        mobile ? 'bg-bg' : 'bg-bg/70 backdrop-blur-xl',
      )}
      style={{
        height: mobile ? 'calc(56px + env(safe-area-inset-top, 0px))' : 'calc(var(--topbar-h) + env(safe-area-inset-top, 0px))',
        paddingTop: 'env(safe-area-inset-top, 0px)',
        paddingRight: mobile ? undefined : 'max(20px, calc(100vw - env(titlebar-area-x, 0px) - env(titlebar-area-width, 100vw) + 12px))',
      }}
    >
      {mobile && !root && (
        <IconButton label={t('common.back')} onClick={() => (canGoBackInHistory() ? nav(-1) : nav('/library'))} className="app-no-drag -ml-1.5">
          <ArrowLeft />
        </IconButton>
      )}
      {mobile && root && (
        <IconButton label={t('mobile.menu')} onClick={() => setChrome({ navOpen: true })} className="app-no-drag -ml-1.5">
          <Menu />
        </IconButton>
      )}
      <div className="app-no-drag flex min-w-0 flex-1 items-center">
        <Crumbs />
      </div>
      <div className="app-no-drag -mr-1 flex items-center gap-0.5 sm:mr-0 sm:gap-1.5">
        <SearchBox />
        <MobileSearch />
        {viewControls && <ViewOptions />}
        <JobsButton />
        {info && !info.caps.readonly && viewControls && <AddMenu />}
      </div>
    </header>
  );
}
