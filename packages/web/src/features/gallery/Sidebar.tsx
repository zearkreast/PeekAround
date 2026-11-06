import { useEffect, useState, type ReactNode } from 'react';
import { NavLink, useLocation, useNavigate } from 'react-router';
import { AnimatePresence, motion } from 'motion/react';
import { useQueryClient } from '@tanstack/react-query';
import {
  BookImage,
  ChevronDown,
  ChevronRight,
  Clock3,
  Folder,
  FolderOpen,
  GripVertical,
  Heart,
  Images,
  LayoutGrid,
  Library as LibraryGlyph,
  PanelLeftClose,
  Plus,
  Presentation,
  Settings,
  Type,
  WifiOff,
  X,
  Compass,
  Cloud,
} from 'lucide-react';
import { useRemote } from '@/features/remote/RemoteHost';
import { RemoteBadge } from '@/features/remote/RemoteFolders';
import type { Album, FolderRef, Library } from '@peekaround/shared';
import { useBackend, useConnection, useInfo } from '@/backend';
import { useT, useLang } from '@/i18n';
import { cn } from '@/lib/cn';
import { durationUntil } from '@/lib/format';
import { LibraryIcon } from '@/lib/icons';
import { useBrowse, useLibraries } from '@/lib/queries';
import { folderHref, libraryHref } from '@/lib/routes';
import { usePrefs } from '@/store/prefs';
import { useChrome } from '@/store/chrome';
import { useIsMobile } from '@/lib/useMedia';
import { useScan } from '@/store/scan';
import { IconButton, ProgressRing } from '@/components/ui';
import { BACK, useBackHandler } from '@/lib/backStack';
import { LogoMark, LogoWordmark } from '@/components/Logo';
import { DevicesNav } from '@/features/devices/DevicesNav';
import { albumHref, albumsSupported, useAlbums } from '@/features/library/albums';
import { addToAlbum } from '@/features/library/AlbumPicker';
import { isMediaDrag, readMediaDrop } from '@/features/library/dnd';
import { useLibraryUi } from '@/features/library/LibraryHost';
import { NewAlbumButton } from '@/features/library/AlbumsPage';

/** the one moving "active" pill of the sidebar (shared layoutId → it glides between rows) */
function ActivePill() {
  return <motion.span layoutId="nav-active" className="absolute inset-0 rounded-xl bg-surface-2" transition={{ type: 'spring', stiffness: 500, damping: 40 }} />;
}

function NavItem({ to, icon, label, collapsed, end, match }: { to: string; icon: ReactNode; label: string; collapsed: boolean; end?: boolean; match?: (p: string) => boolean }) {
  const loc = useLocation();
  return (
    <NavLink
      to={to}
      end={end}
      title={collapsed ? label : undefined}
      className={({ isActive }) =>
        cn(
          'group relative flex h-9 items-center gap-3 rounded-xl px-2.5 text-[13.5px] font-medium transition-colors [&_svg]:size-[18px] [&_svg]:shrink-0',
          isActive || match?.(loc.pathname) ? 'text-fg' : 'text-dim hover:bg-surface hover:text-fg',
        )
      }
    >
      {({ isActive }) => (
        <>
          {(isActive || match?.(loc.pathname)) && <ActivePill />}
          <span className="relative z-10 flex min-w-0 items-center gap-3">
            {icon}
            {!collapsed && <span className="truncate">{label}</span>}
          </span>
        </>
      )}
    </NavLink>
  );
}

/** Lazily expanding directory tree under a library folder. Every row is a drop target. */
function DirTree({ folderId, dir, depth }: { folderId: string; dir: string; depth: number }) {
  const { first } = useBrowse({ folderId, dir, limit: 1 });
  const folders = first?.folders ?? [];
  if (!folders.length) return null;
  return (
    <div>
      {folders.map((f) => (
        <DirNode key={f.dir} folderId={folderId} dir={f.dir} name={f.name} count={f.mediaCount} depth={depth} />
      ))}
    </div>
  );
}

function DirNode({ folderId, dir, name, count, depth }: { folderId: string; dir: string; name: string; count: number; depth: number }) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const loc = useLocation();
  const href = folderHref(folderId, dir);
  const active = decodeURIComponent(loc.pathname) === decodeURIComponent(href);
  return (
    <div>
      <div
        data-drop-folder={folderId}
        data-drop-dir={dir}
        data-drop-label={name}
        className={cn('group relative flex h-8 items-center rounded-lg pr-2 text-[13px] transition-colors', active ? 'text-fg' : 'text-dim hover:bg-surface hover:text-fg')}
        style={{ paddingLeft: 6 + depth * 14 }}
      >
        {active && <ActivePill />}
        <button
          type="button"
          aria-label={t('mobile.expand')}
          aria-expanded={open}
          onClick={() => setOpen((o) => !o)}
          className="relative z-10 grid size-5 shrink-0 place-items-center rounded text-faint hover:text-fg"
        >
          <ChevronRight className={cn('size-3.5 transition-transform duration-200', open && 'rotate-90')} />
        </button>
        <NavLink to={href} className="relative z-10 flex min-w-0 flex-1 items-center gap-2">
          {open ? <FolderOpen className="size-4 shrink-0" /> : <Folder className="size-4 shrink-0" />}
          <span className="truncate">{name}</span>
          <span className="ml-auto text-[11px] text-faint tabular-nums opacity-0 transition-opacity group-hover:opacity-100">{count || ''}</span>
        </NavLink>
      </div>
      <AnimatePresence initial={false}>
        {open && (
          <motion.div initial={{ height: 0, opacity: 0 }} animate={{ height: 'auto', opacity: 1 }} exit={{ height: 0, opacity: 0 }} transition={{ duration: 0.22, ease: [0.22, 1, 0.36, 1] }} className="overflow-hidden">
            <DirTree folderId={folderId} dir={dir} depth={depth + 1} />
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

const LIB_MIME = 'application/x-peekaround-library';

function LibraryRow({
  lib,
  collapsed,
  onEdit,
  reorder,
}: {
  lib: Library;
  collapsed: boolean;
  onEdit: () => void;
  /** drag & drop reordering (writable servers) */
  reorder: { dragging: string | null; setDragging: (id: string | null) => void; drop: (id: string, before: boolean) => void } | null;
}) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const [insert, setInsert] = useState<'before' | 'after' | null>(null);
  const loc = useLocation();
  const active = loc.pathname === libraryHref(lib.id) || lib.folders.some((f) => loc.pathname.startsWith(`/folder/${f.id}`));
  const exact = loc.pathname === libraryHref(lib.id);
  const single = lib.folders.length === 1 ? lib.folders[0] : null;
  const remote = lib.folders.find((f) => f.remote)?.remote;
  return (
    <div
      className={cn('relative', reorder?.dragging === lib.id && 'nav-dragging', insert === 'before' && 'nav-insert-before', insert === 'after' && 'nav-insert-after')}
      onDragOver={(e) => {
        if (!reorder?.dragging || reorder.dragging === lib.id || !Array.from(e.dataTransfer.types).includes(LIB_MIME)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = 'move';
        const r = e.currentTarget.getBoundingClientRect();
        setInsert(e.clientY < r.top + Math.min(r.height, 36) / 2 ? 'before' : 'after');
      }}
      onDragLeave={() => setInsert(null)}
      onDrop={(e) => {
        if (!reorder?.dragging || !Array.from(e.dataTransfer.types).includes(LIB_MIME)) return;
        e.preventDefault();
        const where = insert;
        setInsert(null);
        if (where) reorder.drop(lib.id, where === 'before');
      }}
    >
      <div
        data-drop-folder={single && !single.remote && !single.device ? single.id : undefined}
        data-drop-dir={single ? '' : undefined}
        data-drop-label={lib.name}
        onContextMenu={(e) => {
          e.preventDefault();
          onEdit();
        }}
        className={cn('group relative flex h-9 items-center rounded-xl text-[13.5px] font-medium transition-colors', active ? 'text-fg' : 'text-dim hover:bg-surface hover:text-fg')}
      >
        {exact ? <ActivePill /> : active && <span className="absolute inset-0 rounded-xl bg-surface" />}
        {reorder && !collapsed && (
          <span
            draggable
            onDragStart={(e) => {
              e.dataTransfer.setData(LIB_MIME, lib.id);
              e.dataTransfer.effectAllowed = 'move';
              reorder.setDragging(lib.id);
            }}
            onDragEnd={() => reorder.setDragging(null)}
            title={t('nav2.reorderHint')}
            className="absolute top-1/2 left-0 z-20 grid h-6 w-3 -translate-y-1/2 cursor-grab place-items-center text-faint opacity-0 transition-opacity group-hover:opacity-100 active:cursor-grabbing"
          >
            <GripVertical className="size-3" />
          </span>
        )}
        <NavLink to={libraryHref(lib.id)} className="relative z-10 flex min-w-0 flex-1 items-center gap-3 px-2.5" title={collapsed ? lib.name : undefined}>
          <span className="grid size-[22px] shrink-0 place-items-center rounded-lg text-white transition-transform duration-200 group-hover:scale-110" style={{ background: lib.color }}>
            <LibraryIcon name={lib.icon} className="size-3.5" />
          </span>
          {!collapsed && <span className="truncate">{lib.name}</span>}
          {!collapsed && remote && <RemoteBadge remote={remote} />}
        </NavLink>
        {!collapsed && (
          <>
            <span className="relative z-10 mr-1 text-[11px] text-faint tabular-nums group-hover:hidden">{lib.mediaCount || ''}</span>
            <button
              type="button"
              aria-label={t('mobile.expand')}
              aria-expanded={open}
              onClick={() => setOpen((o) => !o)}
              className="relative z-10 mr-1 hidden size-6 place-items-center rounded-md text-faint hover:bg-surface-2 hover:text-fg group-hover:grid"
            >
              <ChevronRight className={cn('size-3.5 transition-transform duration-200', open && 'rotate-90')} />
            </button>
          </>
        )}
      </div>
      <AnimatePresence initial={false}>
        {open && !collapsed && (
          <motion.div initial={{ height: 0, opacity: 0 }} animate={{ height: 'auto', opacity: 1 }} exit={{ height: 0, opacity: 0 }} transition={{ duration: 0.22, ease: [0.22, 1, 0.36, 1] }} className="overflow-hidden pl-3">
            {single ? (
              <DirTree folderId={single.id} dir="" depth={0} />
            ) : (
              lib.folders.map((f) => <DirNode key={f.id} folderId={f.id} dir="" name={f.name} count={f.mediaCount} depth={0} />)
            )}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/** album row: link + drop target for dragged tiles */
function AlbumRow({ album, collapsed }: { album: Album; collapsed: boolean }) {
  const t = useT();
  const backend = useBackend();
  const qc = useQueryClient();
  const loc = useLocation();
  const [over, setOver] = useState(false);
  const active = loc.pathname === albumHref(album.id);
  return (
    <div
      onDragOver={(e) => {
        if (!isMediaDrag(e)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = 'copy';
        setOver(true);
      }}
      onDragLeave={() => setOver(false)}
      onDrop={(e) => {
        if (!isMediaDrag(e)) return;
        e.preventDefault();
        setOver(false);
        void addToAlbum(backend, qc, album, readMediaDrop(e), t);
      }}
      title={over ? t('nav2.dropToAlbum', { name: album.name }) : collapsed ? album.name : undefined}
      data-drop-album={album.id}
    >
      <NavLink
        to={albumHref(album.id)}
        className={cn(
          'group relative flex h-9 items-center gap-3 rounded-xl px-2.5 text-[13.5px] font-medium transition-colors',
          active ? 'text-fg' : 'text-dim hover:bg-surface hover:text-fg',
          over && 'album-drop',
        )}
      >
        {active && <ActivePill />}
        <span className="relative z-10 size-[22px] shrink-0 overflow-hidden rounded-lg bg-surface-2 transition-transform duration-200 group-hover:scale-110">
          {album.coverId ? (
            <img src={backend.thumbUrl(album.coverId, 320)} alt="" draggable={false} className="size-full object-cover" />
          ) : (
            <BookImage className="m-[3px] size-4 text-faint" />
          )}
        </span>
        {!collapsed && <span className="relative z-10 min-w-0 flex-1 truncate">{album.name}</span>}
        {!collapsed && <span className="relative z-10 text-[11px] text-faint tabular-nums">{album.count || ''}</span>}
      </NavLink>
    </div>
  );
}

function TempRow({ f }: { f: FolderRef }) {
  const backend = useBackend();
  const lang = useLang();
  const t = useT();
  const nav = useNavigate();
  const loc = useLocation();
  const active = loc.pathname.startsWith(`/folder/${f.id}`);
  return (
    <div className={cn('group relative flex h-9 items-center gap-2.5 rounded-xl px-2.5 text-[13px] transition-colors', active ? 'text-fg' : 'text-dim hover:bg-surface hover:text-fg')}>
      {active && <ActivePill />}
      <NavLink to={folderHref(f.id)} className="relative z-10 flex min-w-0 flex-1 items-center gap-2.5">
        <Clock3 className="size-4 shrink-0 text-warn" />
        <span className="truncate">{f.name}</span>
      </NavLink>
      {f.expiresAt && <span className="relative z-10 shrink-0 text-[11px] text-faint group-hover:hidden">{durationUntil(f.expiresAt, lang)}</span>}
      <button
        type="button"
        title={t('temp.close')}
        onClick={async () => {
          await backend.removeFolder(f.id);
          if (active) nav('/');
        }}
        className="relative z-10 hidden size-6 shrink-0 place-items-center rounded-md text-faint hover:bg-surface-2 hover:text-fg group-hover:grid"
      >
        <X className="size-3.5" />
      </button>
    </div>
  );
}

/** section header; with `id` its body collapses (remembered), with `to` the title is a link */
function SectionTitle({ children, action, collapsed, id, to }: { children: ReactNode; action?: ReactNode; collapsed: boolean; id?: string; to?: string }) {
  const t = useT();
  const closed = usePrefs((s) => (id ? !!s.navCollapsed[id] : false));
  if (collapsed) return <div className="mx-3 my-3 h-px bg-line" />;
  const toggle = () => {
    if (!id) return;
    const p = usePrefs.getState();
    p.set({ navCollapsed: { ...p.navCollapsed, [id]: !closed } });
  };
  const label = 'text-[11px] font-semibold tracking-[0.08em] uppercase';
  return (
    <div className="flex h-8 items-center justify-between pt-4 pb-1 pr-0.5 pl-1">
      <span className="flex min-w-0 items-center">
        {id && (
          <button
            type="button"
            onClick={toggle}
            aria-expanded={!closed}
            aria-label={t('nav2.section.toggle', { name: typeof children === 'string' ? children : id })}
            className="grid size-5 place-items-center rounded-md text-faint transition-colors hover:bg-surface-2 hover:text-fg"
          >
            <ChevronDown className={cn('size-3 transition-transform duration-200', closed && '-rotate-90')} />
          </button>
        )}
        {to ? (
          <NavLink to={to} end className={cn(label, 'rounded-md px-1 py-0.5 text-faint transition-colors hover:text-fg [&.active]:text-fg')}>
            {children}
          </NavLink>
        ) : (
          <span className={cn(label, 'px-1 text-faint', id && 'cursor-pointer select-none')} onClick={toggle}>
            {children}
          </span>
        )}
      </span>
      {action}
    </div>
  );
}

/** collapsible body of a section (height animation) */
function SectionBody({ id, children, collapsed }: { id: string; children: ReactNode; collapsed: boolean }) {
  const closed = usePrefs((s) => !!s.navCollapsed[id]);
  return (
    <AnimatePresence initial={false}>
      {(!closed || collapsed) && (
        <motion.div
          key="body"
          initial={{ height: 0, opacity: 0 }}
          animate={{ height: 'auto', opacity: 1 }}
          exit={{ height: 0, opacity: 0 }}
          transition={{ duration: 0.22, ease: [0.22, 1, 0.36, 1] }}
          className="flex flex-col gap-0.5 overflow-hidden"
        >
          {children}
        </motion.div>
      )}
    </AnimatePresence>
  );
}

const photosMatch = (p: string) => p === '/' || p.startsWith('/timeline') || p.startsWith('/search');

export function Sidebar() {
  const t = useT();
  const set = usePrefs((s) => s.set);
  const mobile = useIsMobile();
  const navOpen = useChrome((s) => s.navOpen);
  const setChrome = useChrome((s) => s.set);
  const loc = useLocation();
  // always expanded: the icons-only collapsed mode is the navigation rail (features/nav/NavRail)
  const collapsed = false;
  useBackHandler(mobile && navOpen, () => setChrome({ navOpen: false }), BACK.drawer, 'drawer');
  // close the drawer after navigating
  useEffect(() => {
    if (mobile) setChrome({ navOpen: false });
  }, [loc.pathname, mobile, setChrome]);
  const libs = useLibraries();
  const albums = useAlbums();
  const info = useInfo();
  const backend = useBackend();
  const qc = useQueryClient();
  const conn = useConnection();
  const scan = useScan();
  const [dragging, setDragging] = useState<string | null>(null);
  const canEdit = info ? !info.caps.readonly : false;
  const libraries = libs.data?.libraries ?? [];
  const albumList = albums.data ?? [];
  const showAlbums = albumsSupported(backend);
  const editLib = (l: Library | 'new') => canEdit && useLibraryUi.getState().edit(l);

  // drag & drop library order → server (optimistic)
  const reorder =
    canEdit && backend.orderLibraries && libraries.length > 1
      ? {
          dragging,
          setDragging,
          drop: (targetId: string, before: boolean) => {
            const moving = dragging;
            setDragging(null);
            if (!moving || moving === targetId) return;
            const ids = libraries.map((l) => l.id).filter((id) => id !== moving);
            const at = ids.indexOf(targetId) + (before ? 0 : 1);
            ids.splice(at, 0, moving);
            const key = ['libraries', backend.id];
            qc.setQueryData(key, (d: typeof libs.data) => (d ? { ...d, libraries: ids.map((id) => d.libraries.find((l) => l.id === id)!).filter(Boolean) } : d));
            void backend.orderLibraries!(ids).finally(() => qc.invalidateQueries({ queryKey: ['libraries'] }));
          },
        }
      : null;

  const body = (
    <>
      <div className="app-drag flex h-[var(--topbar-h)] items-center gap-2.5 px-4">
        <LogoMark size={30} />
        {!collapsed && <LogoWordmark />}
      </div>

      <nav className="no-scrollbar flex min-h-0 flex-1 flex-col overflow-y-auto px-2.5 pb-3" data-testid="sidebar">
        <NavItem to="/" end icon={<Images />} label={t('nav2.photos')} collapsed={collapsed} match={photosMatch} />
        <NavItem to="/explore" icon={<Compass />} label={t('mobile.nav.explore')} collapsed={collapsed} />
        <NavItem to="/favorites" icon={<Heart />} label={t('nav.favorites')} collapsed={collapsed} />
        <NavItem to="/library" end icon={<LibraryGlyph />} label={t('nav2.library')} collapsed={collapsed} />

        {showAlbums && (
          <>
            <SectionTitle collapsed={collapsed} id="albums" to="/albums" action={<NewAlbumButton compact />}>
              {t('nav2.albums')}
            </SectionTitle>
            <SectionBody id="albums" collapsed={collapsed}>
              {albumList.slice(0, 12).map((a) => (
                <AlbumRow key={a.id} album={a} collapsed={collapsed} />
              ))}
              {albumList.length > 12 && !collapsed && (
                <NavLink to="/albums" className="flex h-8 items-center rounded-lg px-2.5 text-[12.5px] text-faint hover:bg-surface hover:text-fg">
                  {t('nav2.allAlbums')} ({albumList.length})
                </NavLink>
              )}
              {!albumList.length && (collapsed ? <NavItem to="/albums" icon={<BookImage />} label={t('nav2.albums')} collapsed /> : albums.isSuccess && (
                <NavLink to="/albums" className="flex h-8 items-center gap-2.5 rounded-lg px-2.5 text-[12.5px] text-faint transition-colors hover:bg-surface hover:text-fg">
                  <BookImage className="size-4" />
                  {t('album.empty.title')}
                </NavLink>
              ))}
            </SectionBody>
          </>
        )}

        <SectionTitle
          collapsed={collapsed}
          id="libraries"
          action={
            canEdit && (
              <span className="flex items-center gap-0.5">
                {backend.remoteAddFolder && (
                  <button
                    type="button"
                    title={t('mega.addLink')}
                    aria-label={t('mega.addLink')}
                    onClick={() => useRemote.getState().show(null, { intent: 'stream' })}
                    className="grid size-6 place-items-center rounded-md text-faint hover:bg-surface-2 hover:text-[#e5484d]"
                  >
                    <Cloud className="size-3.5" />
                  </button>
                )}
                <button type="button" title={t('nav.addLibrary')} aria-label={t('nav.addLibrary')} onClick={() => editLib('new')} className="grid size-6 place-items-center rounded-md text-faint hover:bg-surface-2 hover:text-fg">
                  <Plus className="size-3.5" />
                </button>
              </span>
            )
          }
        >
          {t('nav.libraries')}
        </SectionTitle>
        <SectionBody id="libraries" collapsed={collapsed}>
          {libraries.map((l) => (
            <LibraryRow key={l.id} lib={l} collapsed={collapsed} onEdit={() => editLib(l)} reorder={reorder} />
          ))}
          {collapsed && canEdit && (
            <button type="button" title={t('nav.addLibrary')} onClick={() => editLib('new')} className="flex h-9 items-center justify-center rounded-xl text-faint hover:bg-surface hover:text-fg">
              <Plus className="size-4" />
            </button>
          )}
        </SectionBody>

        {!!libs.data?.temp.length && (
          <>
            <SectionTitle collapsed={collapsed}>{t('nav.temp')}</SectionTitle>
            <div className="flex flex-col gap-0.5">
              {libs.data.temp.map((f) => (collapsed ? <NavItem key={f.id} to={folderHref(f.id)} icon={<Clock3 />} label={f.name} collapsed /> : <TempRow key={f.id} f={f} />))}
            </div>
          </>
        )}

        <DevicesNav collapsed={collapsed} title={(c) => <SectionTitle collapsed={collapsed}>{c}</SectionTitle>} />

        <SectionTitle collapsed={collapsed} id="tools">
          {t('nav.tools')}
        </SectionTitle>
        <SectionBody id="tools" collapsed={collapsed}>
          <NavItem to="/collage" icon={<LayoutGrid />} label={t('nav.collage')} collapsed={collapsed} />
          <NavItem to="/slideshow" icon={<Presentation />} label={t('nav.slideshow')} collapsed={collapsed} />
          <NavItem to="/fonts" icon={<Type />} label={t('nav.fonts')} collapsed={collapsed} />
        </SectionBody>
      </nav>

      <div className="flex flex-col gap-1 border-t border-line px-2.5 py-2.5">
        <AnimatePresence>
          {conn === 'offline' && (
            <motion.div initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: 'auto' }} exit={{ opacity: 0, height: 0 }} className="flex items-center gap-2 rounded-xl bg-danger/10 px-2.5 py-2 text-xs text-danger">
              <WifiOff className="size-4 shrink-0" />
              {!collapsed && <span>{t('nav.offline')}</span>}
            </motion.div>
          )}
          {scan.scanning && scan.total > 0 && (
            <motion.div initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: 'auto' }} exit={{ opacity: 0, height: 0 }} className="flex items-center gap-2 px-2.5 py-1.5 text-xs text-dim">
              <ProgressRing value={scan.done / Math.max(1, scan.total)} size={18} stroke={2.5} />
              {!collapsed && <span className="tabular-nums">{t('nav.scanning', { done: scan.done, total: scan.total })}</span>}
            </motion.div>
          )}
        </AnimatePresence>
        <div className="flex items-center gap-1">
          <div className="min-w-0 flex-1">
            <NavItem to="/settings" icon={<Settings />} label={t('nav.settings')} collapsed={collapsed} />
          </div>
          {!mobile && (
            <IconButton label={t('nav.collapse')} size="sm" onClick={() => set({ wideSidebar: false })}>
              <PanelLeftClose />
            </IconButton>
          )}
        </div>
      </div>
    </>
  );

  if (mobile) {
    return (
      <AnimatePresence>
        {navOpen && (
          <>
            <motion.div
              key="scrim"
              className="fixed inset-0 z-40 bg-black/50 backdrop-blur-[2px]"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              onClick={() => setChrome({ navOpen: false })}
            />
            <motion.aside
              key="drawer"
              initial={{ x: -300 }}
              animate={{ x: 0 }}
              exit={{ x: -300 }}
              transition={{ type: 'spring', stiffness: 420, damping: 40 }}
              drag="x"
              dragConstraints={{ left: -300, right: 0 }}
              dragElastic={{ left: 0.2, right: 0 }}
              onDragEnd={(_, i) => i.offset.x < -80 && setChrome({ navOpen: false })}
              className="fixed inset-y-0 left-0 z-50 flex w-[280px] max-w-[85vw] flex-col border-r border-line bg-elev pt-[env(safe-area-inset-top)] pb-[env(safe-area-inset-bottom)] shadow-soft"
            >
              {body}
            </motion.aside>
          </>
        )}
      </AnimatePresence>
    );
  }

  return (
    <aside className="relative z-20 flex h-full w-[var(--sidebar-w)] shrink-0 flex-col border-r border-line bg-elev/60" data-testid="wide-sidebar">
      {body}
    </aside>
  );
}
