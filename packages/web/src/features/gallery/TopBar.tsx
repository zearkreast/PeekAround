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
import { useT, type MessageKey } from '@/i18n';
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
  // (not an <h1>: the page has its own big title; this one only shows once that scrolled away)
  if (!crumbs.length || mobile) return <div className="truncate font-display text-[17px] font-semibold">{crumbs.length ? crumbs[crumbs.length - 1].label : title}</div>;
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

/**
 * Desktop: the page title in the top bar shows only once the page's own big title (its first
 * <h1>) has scrolled out of view — no second title right above the first one. Pages without one
 * (settings, collage, map …) always show it.
 */
function useTitleReveal(enabled: boolean): boolean {
  const loc = useRouterLocation();
  const [show, setShow] = useState(true);
  useEffect(() => {
    if (!enabled) return setShow(true);
    const sc = document.getElementById('scroller');
    if (!sc) return;
    let raf = 0;
    const check = () => {
      raf = 0;
      // the page that animates out may still be there for a moment: take the last (= newest) one
      const all = sc.querySelectorAll('h1');
      const h1 = all[all.length - 1];
      setShow(!h1 || h1.getBoundingClientRect().bottom < sc.getBoundingClientRect().top + 8);
    };
    const on = () => {
      if (!raf) raf = requestAnimationFrame(check);
    };
    sc.addEventListener('scroll', on, { passive: true });
    // pages put their header up once their data is there
    const timers = [0, 250, 700, 1500].map((ms) => setTimeout(check, ms));
    return () => {
      sc.removeEventListener('scroll', on);
      if (raf) cancelAnimationFrame(raf);
      timers.forEach(clearTimeout);
    };
  }, [enabled, loc.pathname]);
  return show;
}

function SearchBox({ className }: { className?: string }) {
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
      role="search"
      className={className}
      onSubmit={(e) => {
        e.preventDefault();
        if (q.trim()) nav(`/search?q=${encodeURIComponent(q.trim())}`);
      }}
    >
      <Input ref={ref} value={q} onChange={(e) => setQ(e.target.value)} placeholder={t('nav.search')} title={`${t('nav.search')} (Ctrl+K)`} icon={<Search />} className="app-no-drag" />
    </form>
  );
}

function ViewOptions() {
  const t = useT();
  const p = usePrefs();
  const mobile = useIsMobile();
  // what this page honours (the timeline / albums keep their own order, Favorites is a filter itself)
  const canSort = useChrome((s) => s.viewSort);
  const canKind = useChrome((s) => s.viewKind);
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLButtonElement>(null);
  const sorts: SortKey[] = ['takenAt', 'name', 'mtime', 'size'];
  // the levels of the grid on screen (a narrow grid uses the phone ones, see MediaGrid)
  const gridPhone = useChrome((s) => s.gridPhone);
  const phoneLevels = mobile || gridPhone;
  const level = phoneLevels ? p.gridZoomMobile : p.gridZoomDesktop;
  // the two dense levels are always square tiles of a fixed size: the layout / size options do nothing there
  const freeLayout = level <= 1;
  const filtered = canKind && p.kind !== 'all';
  const body = (
    <>
      {canKind && (
        <>
          <MenuLabel>{t('gallery.filter')}</MenuLabel>
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
        </>
      )}
      {canSort && (
        <>
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
        </>
      )}
      <MenuLabel>{t('gallery.density')}</MenuLabel>
      <div className="flex flex-col gap-3 px-1 pt-0.5 pb-2">
        <Segmented<string>
          size={mobile ? 'md' : 'sm'}
          value={String(level)}
          onChange={(v) => p.set(phoneLevels ? { gridZoomMobile: Number(v) } : { gridZoomDesktop: Number(v) })}
          className="w-full [&>button]:flex-1"
          options={['0', '1', '2', '3'].map((v) => ({ value: v, label: '', icon: <span className="grid place-items-center">{densityIcon(Number(v))}</span>, title: t(`gallery.density.${v}` as MessageKey) }))}
        />
        {freeLayout ? (
          <>
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
            {!phoneLevels && (
              <div className="px-1.5">
                <Slider label={t('gallery.size')} value={p.rowHeight} min={110} max={420} step={10} onChange={(rowHeight) => p.set({ rowHeight })} format={(v) => `${v}px`} />
              </div>
            )}
          </>
        ) : (
          <p className="px-1.5 text-xs text-faint">{t('gallery.density.fixedHint')}</p>
        )}
        {!mobile && <p className="px-1.5 text-[11px] text-faint">{t('gallery.density.wheelHint')}</p>}
      </div>
    </>
  );
  return (
    <>
      <IconButton ref={anchor} label={filtered ? `${t('gallery.viewOptions')} · ${t(`gallery.filter.${p.kind}` as MessageKey)}` : t('gallery.viewOptions')} active={open} onClick={() => setOpen((o) => !o)} className="relative" data-action="view-options">
        <SlidersHorizontal />
        {filtered && <span className="absolute top-2 right-2 size-2 rounded-full bg-accent ring-2 ring-[var(--bg)]" />}
      </IconButton>
      {mobile ? (
        <BottomSheet open={open} onClose={() => setOpen(false)} title={t('gallery.viewOptions')}>
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

/**
 * The page header. Desktop: title / breadcrumb (once the page's own title scrolled away) ·
 * centered search · page actions; the window buttons of the desktop app (titleBarOverlay) sit in
 * the right padding, which stays a window drag region like the rest of the empty bar.
 * Phones: menu / back · title · search · actions.
 */
export function TopBar() {
  const t = useT();
  const viewControls = useChrome((s) => s.viewControls);
  const setChrome = useChrome((s) => s.set);
  const mobile = useIsMobile();
  const info = useInfo();
  const loc = useRouterLocation();
  const nav = useNavigate();
  const root = ROOT_TABS.includes(loc.pathname);
  const showTitle = useTitleReveal(!mobile);
  const actions = (
    <div className="-mr-1 flex shrink-0 items-center justify-end gap-0.5 sm:mr-0 sm:gap-1.5">
      {mobile && <MobileSearch />}
      {viewControls && <ViewOptions />}
      <JobsButton />
      {info && !info.caps.readonly && viewControls && <AddMenu />}
    </div>
  );
  if (!mobile)
    return (
      <header
        className="app-drag wco-r relative z-10 grid shrink-0 grid-cols-[minmax(0,1fr)_auto_minmax(max-content,1fr)] items-center gap-4 border-b border-line bg-bg/70 pl-5 backdrop-blur-xl [--wco-base:1rem]"
        style={{ height: 'var(--topbar-h)' }}
        data-testid="topbar"
      >
        <div className={cn('flex min-w-0 items-center transition-opacity duration-200', showTitle ? 'opacity-100' : 'pointer-events-none opacity-0')} aria-hidden={!showTitle}>
          <Crumbs />
        </div>
        <SearchBox className="w-[clamp(200px,34vw,520px)]" />
        {actions}
      </header>
    );
  return (
    <header
      className="app-drag relative z-10 flex shrink-0 items-center gap-2 border-b border-line bg-bg px-3 sm:gap-3 sm:px-5"
      style={{ height: 'calc(56px + env(safe-area-inset-top, 0px))', paddingTop: 'env(safe-area-inset-top, 0px)' }}
    >
      {!root && (
        <IconButton label={t('common.back')} onClick={() => (canGoBackInHistory() ? nav(-1) : nav('/library'))} className="-ml-1.5">
          <ArrowLeft />
        </IconButton>
      )}
      {root && (
        <IconButton label={t('mobile.menu')} onClick={() => setChrome({ navOpen: true })} className="-ml-1.5">
          <Menu />
        </IconButton>
      )}
      <div className="flex min-w-0 flex-1 items-center">
        <Crumbs />
      </div>
      {actions}
    </header>
  );
}
