import { useEffect, useState, type ReactNode } from 'react';
import { NavLink, useLocation, useNavigate } from 'react-router';
import { AnimatePresence, motion } from 'motion/react';
import {
  CalendarDays,
  ChevronRight,
  Clock3,
  Folder,
  FolderOpen,
  Heart,
  Home,
  LayoutGrid,
  PanelLeftClose,
  PanelLeftOpen,
  Plus,
  Presentation,
  Settings,
  Type,
  WifiOff,
  X,
} from 'lucide-react';
import type { FolderRef, Library } from '@peekaround/shared';
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
import { LogoMark, LogoWordmark } from '@/components/Logo';
import { LibraryEditor } from '@/features/settings/LibraryEditor';

function NavItem({ to, icon, label, collapsed, end }: { to: string; icon: ReactNode; label: string; collapsed: boolean; end?: boolean }) {
  return (
    <NavLink
      to={to}
      end={end}
      title={collapsed ? label : undefined}
      className={({ isActive }) =>
        cn(
          'group relative flex h-9 items-center gap-3 rounded-xl px-2.5 text-[13.5px] font-medium transition-colors [&_svg]:size-[18px] [&_svg]:shrink-0',
          isActive ? 'text-fg' : 'text-dim hover:bg-surface hover:text-fg',
        )
      }
    >
      {({ isActive }) => (
        <>
          {isActive && (
            <motion.span
              layoutId="nav-active"
              className="absolute inset-0 rounded-xl bg-surface-2"
              transition={{ type: 'spring', stiffness: 500, damping: 40 }}
            />
          )}
          <span className="relative z-10 flex items-center gap-3">
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
        className={cn('group flex h-8 items-center rounded-lg pr-2 text-[13px] transition-colors', active ? 'bg-surface-2 text-fg' : 'text-dim hover:bg-surface hover:text-fg')}
        style={{ paddingLeft: 6 + depth * 14 }}
      >
        <button
          type="button"
          aria-label="Expand"
          onClick={() => setOpen((o) => !o)}
          className="grid size-5 shrink-0 place-items-center rounded text-faint hover:text-fg"
        >
          <ChevronRight className={cn('size-3.5 transition-transform duration-200', open && 'rotate-90')} />
        </button>
        <NavLink to={href} className="flex min-w-0 flex-1 items-center gap-2">
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

function LibraryRow({ lib, collapsed, onEdit }: { lib: Library; collapsed: boolean; onEdit: () => void }) {
  const [open, setOpen] = useState(false);
  const loc = useLocation();
  const active = loc.pathname === libraryHref(lib.id) || lib.folders.some((f) => loc.pathname.startsWith(`/folder/${f.id}`));
  const single = lib.folders.length === 1 ? lib.folders[0] : null;
  return (
    <div>
      <div
        data-drop-folder={single?.id}
        data-drop-dir={single ? '' : undefined}
        data-drop-label={lib.name}
        onContextMenu={(e) => {
          e.preventDefault();
          onEdit();
        }}
        className={cn('group relative flex h-9 items-center rounded-xl text-[13.5px] font-medium transition-colors', active ? 'bg-surface-2 text-fg' : 'text-dim hover:bg-surface hover:text-fg')}
      >
        <NavLink to={libraryHref(lib.id)} className="flex min-w-0 flex-1 items-center gap-3 px-2.5" title={collapsed ? lib.name : undefined}>
          <span className="grid size-[22px] shrink-0 place-items-center rounded-lg text-white" style={{ background: lib.color }}>
            <LibraryIcon name={lib.icon} className="size-3.5" />
          </span>
          {!collapsed && <span className="truncate">{lib.name}</span>}
        </NavLink>
        {!collapsed && (
          <>
            <span className="mr-1 text-[11px] text-faint tabular-nums group-hover:hidden">{lib.mediaCount || ''}</span>
            <button
              type="button"
              aria-label="Expand"
              onClick={() => setOpen((o) => !o)}
              className="mr-1 hidden size-6 place-items-center rounded-md text-faint hover:bg-surface-2 hover:text-fg group-hover:grid"
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

function TempRow({ f }: { f: FolderRef }) {
  const backend = useBackend();
  const lang = useLang();
  const t = useT();
  const nav = useNavigate();
  const loc = useLocation();
  const active = loc.pathname.startsWith(`/folder/${f.id}`);
  return (
    <div
      className={cn('group flex h-9 items-center gap-2.5 rounded-xl px-2.5 text-[13px] transition-colors', active ? 'bg-surface-2 text-fg' : 'text-dim hover:bg-surface hover:text-fg')}
    >
      <NavLink to={folderHref(f.id)} className="flex min-w-0 flex-1 items-center gap-2.5">
        <Clock3 className="size-4 shrink-0 text-warn" />
        <span className="truncate">{f.name}</span>
      </NavLink>
      {f.expiresAt && <span className="shrink-0 text-[11px] text-faint group-hover:hidden">{durationUntil(f.expiresAt, lang)}</span>}
      <button
        type="button"
        title={t('temp.close')}
        onClick={async () => {
          await backend.removeFolder(f.id);
          if (active) nav('/');
        }}
        className="hidden size-6 shrink-0 place-items-center rounded-md text-faint hover:bg-surface-2 hover:text-fg group-hover:grid"
      >
        <X className="size-3.5" />
      </button>
    </div>
  );
}

function SectionTitle({ children, action, collapsed }: { children: ReactNode; action?: ReactNode; collapsed: boolean }) {
  if (collapsed) return <div className="mx-3 my-3 h-px bg-line" />;
  return (
    <div className="flex h-8 items-center justify-between px-2.5 pt-4 pb-1">
      <span className="text-[11px] font-semibold tracking-[0.08em] text-faint uppercase">{children}</span>
      {action}
    </div>
  );
}

export function Sidebar() {
  const t = useT();
  const { sidebarCollapsed: prefCollapsed, set } = usePrefs();
  const mobile = useIsMobile();
  const navOpen = useChrome((s) => s.navOpen);
  const setChrome = useChrome((s) => s.set);
  const loc = useLocation();
  const collapsed = mobile ? false : prefCollapsed;
  // close the drawer after navigating
  useEffect(() => {
    if (mobile) setChrome({ navOpen: false });
  }, [loc.pathname, mobile, setChrome]);
  const libs = useLibraries();
  const info = useInfo();
  const conn = useConnection();
  const scan = useScan();
  const [editing, setEditing] = useState<Library | 'new' | null>(null);
  const canEdit = info ? !info.caps.readonly : false;

  const body = (
    <>
      <div className="app-drag flex h-[var(--topbar-h)] items-center gap-2.5 px-4">
        <LogoMark size={30} />
        {!collapsed && <LogoWordmark />}
      </div>

      <nav className="no-scrollbar flex min-h-0 flex-1 flex-col overflow-y-auto px-2.5 pb-3">
        <NavItem to="/" end icon={<Home />} label={t('nav.home')} collapsed={collapsed} />
        <NavItem to="/timeline" icon={<CalendarDays />} label={t('nav.timeline')} collapsed={collapsed} />
        <NavItem to="/favorites" icon={<Heart />} label={t('nav.favorites')} collapsed={collapsed} />

        <SectionTitle
          collapsed={collapsed}
          action={
            canEdit && (
              <button type="button" title={t('nav.addLibrary')} onClick={() => setEditing('new')} className="grid size-6 place-items-center rounded-md text-faint hover:bg-surface-2 hover:text-fg">
                <Plus className="size-3.5" />
              </button>
            )
          }
        >
          {t('nav.libraries')}
        </SectionTitle>
        <div className="flex flex-col gap-0.5">
          {libs.data?.libraries.map((l) => (
            <LibraryRow key={l.id} lib={l} collapsed={collapsed} onEdit={() => canEdit && setEditing(l)} />
          ))}
          {collapsed && canEdit && (
            <button type="button" title={t('nav.addLibrary')} onClick={() => setEditing('new')} className="flex h-9 items-center justify-center rounded-xl text-faint hover:bg-surface hover:text-fg">
              <Plus className="size-4" />
            </button>
          )}
        </div>

        {!!libs.data?.temp.length && (
          <>
            <SectionTitle collapsed={collapsed}>{t('nav.temp')}</SectionTitle>
            <div className="flex flex-col gap-0.5">
              {libs.data.temp.map((f) => (collapsed ? <NavItem key={f.id} to={folderHref(f.id)} icon={<Clock3 />} label={f.name} collapsed /> : <TempRow key={f.id} f={f} />))}
            </div>
          </>
        )}

        <SectionTitle collapsed={collapsed}>{t('nav.tools')}</SectionTitle>
        <NavItem to="/collage" icon={<LayoutGrid />} label={t('nav.collage')} collapsed={collapsed} />
        <NavItem to="/slideshow" icon={<Presentation />} label={t('nav.slideshow')} collapsed={collapsed} />
        <NavItem to="/fonts" icon={<Type />} label={t('nav.fonts')} collapsed={collapsed} />
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
          {!collapsed && !mobile && (
            <IconButton label={t('nav.collapse')} size="sm" onClick={() => set({ sidebarCollapsed: true })}>
              <PanelLeftClose />
            </IconButton>
          )}
        </div>
        {collapsed && (
          <IconButton label={t('nav.expand')} size="sm" className="mx-auto" onClick={() => set({ sidebarCollapsed: false })}>
            <PanelLeftOpen />
          </IconButton>
        )}
      </div>
      <LibraryEditor value={editing} onClose={() => setEditing(null)} />
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
    <motion.aside
      animate={{ width: collapsed ? 68 : 268 }}
      transition={{ type: 'spring', stiffness: 420, damping: 40 }}
      className="relative z-20 flex h-full shrink-0 flex-col border-r border-line bg-elev/60"
    >
      {body}
    </motion.aside>
  );
}
