import { useEffect, useRef, useState, type ReactNode } from 'react';
import { NavLink, useLocation } from 'react-router';
import { AnimatePresence, motion } from 'motion/react';
import { useQueryClient } from '@tanstack/react-query';
import { BookImage, Compass, Heart, Images, Library as LibraryGlyph, Settings, WifiOff } from 'lucide-react';
import type { Album, Library } from '@peekaround/shared';
import { useBackend, useConnection, useInfo } from '@/backend';
import { useT, type MessageKey } from '@/i18n';
import { cn } from '@/lib/cn';
import { LibraryIcon } from '@/lib/icons';
import { useLibraries } from '@/lib/queries';
import { useScan } from '@/store/scan';
import { ProgressRing } from '@/components/ui';
import { LogoMark } from '@/components/Logo';
import { albumsSupported, useAlbums } from '@/features/library/albums';
import { addToAlbum } from '@/features/library/AlbumPicker';
import { isMediaDrag, isOwnNativeDrag, readMediaDrop } from '@/features/library/dnd';

type Match = (p: string) => boolean;

const photosMatch: Match = (p) => p === '/' || p.startsWith('/timeline') || p.startsWith('/search');
const albumsMatch: Match = (p) => p === '/albums' || p.startsWith('/album/');
const libraryMatch: Match = (p) => p === '/library' || p.startsWith('/library/') || p.startsWith('/folder/') || p.startsWith('/collage') || p.startsWith('/slideshow') || p.startsWith('/fonts');

/** what the drop flyout offers: albums for dragged tiles, libraries for files from the OS */
type DropKind = 'media' | 'files';

function dropKind(e: React.DragEvent): DropKind | null {
  if (isMediaDrag(e)) return 'media';
  const types = Array.from(e.dataTransfer?.types ?? []);
  return types.includes('Files') && !isOwnNativeDrag() ? 'files' : null;
}

function RailItem({
  to,
  icon: Icon,
  label,
  match,
  onDragEnter,
}: {
  to: string;
  icon: typeof Images;
  label: string;
  match: Match;
  onDragEnter?: (e: React.DragEvent<HTMLElement>) => void;
}) {
  const loc = useLocation();
  const active = match(loc.pathname);
  return (
    <NavLink
      to={to}
      title={label}
      aria-current={active ? 'page' : undefined}
      data-active={active ? '' : undefined}
      onDragEnter={onDragEnter}
      onDragOver={onDragEnter ? (e) => onDragEnter(e) : undefined}
      className="rail-item group"
    >
      <span className="relative grid h-8 w-14 place-items-center">
        {active && <motion.span layoutId="rail-active" className="absolute inset-0 rounded-full bg-[color-mix(in_oklab,var(--accent)_22%,transparent)]" transition={{ type: 'spring', stiffness: 520, damping: 40 }} />}
        <span className="absolute inset-0 rounded-full transition-colors group-hover:bg-surface-2 group-data-[active]:group-hover:bg-transparent" />
        <Icon className="relative size-[20px]" strokeWidth={active ? 2.25 : 1.8} />
      </span>
      <span className="max-w-full truncate px-1 text-[11.5px] leading-tight font-medium">{label}</span>
    </NavLink>
  );
}

/** an album row of the drop flyout (dragged tiles → added to the album) */
function DropAlbum({ album, onDone }: { album: Album; onDone: () => void }) {
  const t = useT();
  const backend = useBackend();
  const qc = useQueryClient();
  const [over, setOver] = useState(false);
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
        onDone();
      }}
      data-drop-album={album.id}
      className={cn('flex h-11 items-center gap-3 rounded-xl px-2 text-[13.5px] font-medium transition-colors', over ? 'album-drop text-fg' : 'text-dim')}
    >
      <span className="size-8 shrink-0 overflow-hidden rounded-lg bg-surface-2">
        {album.coverId ? <img src={backend.thumbUrl(album.coverId, 320)} alt="" draggable={false} className="size-full object-cover" /> : <BookImage className="m-1.5 size-5 text-faint" />}
      </span>
      <span className="min-w-0 flex-1 truncate">{album.name}</span>
      <span className="text-[11px] text-faint tabular-nums">{album.count || ''}</span>
    </div>
  );
}

/** a library row of the drop flyout (files from the OS → uploaded into it; see DropZone) */
function DropLibrary({ lib }: { lib: Library }) {
  const single = lib.folders.length === 1 ? lib.folders[0] : null;
  const writable = !!single && !single.remote && !single.device;
  return (
    <div
      data-drop-folder={writable ? single!.id : undefined}
      data-drop-dir={writable ? '' : undefined}
      data-drop-label={lib.name}
      className={cn('flex h-11 items-center gap-3 rounded-xl px-2 text-[13.5px] font-medium text-dim', !writable && 'opacity-45')}
    >
      <span className="grid size-8 shrink-0 place-items-center rounded-lg text-white" style={{ background: lib.color }}>
        <LibraryIcon name={lib.icon} className="size-4" />
      </span>
      <span className="min-w-0 flex-1 truncate">{lib.name}</span>
    </div>
  );
}

/**
 * Desktop navigation rail: the few main places (icon + short label) and Settings. Libraries,
 * albums, phones, MEGA and the tools live on the Library hub (/library). While tiles (or files
 * from the OS) are dragged over "Albums" / "Library", a flyout lists the drop targets.
 */
export function NavRail() {
  const t = useT();
  const backend = useBackend();
  const info = useInfo();
  const conn = useConnection();
  const scan = useScan();
  const libs = useLibraries();
  const albums = useAlbums();
  const showAlbums = albumsSupported(backend);
  const [flyout, setFlyout] = useState<{ kind: DropKind; top: number } | null>(null);
  const leave = useRef<ReturnType<typeof setTimeout> | null>(null);

  const keep = () => {
    if (leave.current) clearTimeout(leave.current);
    leave.current = null;
  };
  const close = () => {
    keep();
    setFlyout(null);
  };
  const closeSoon = () => {
    keep();
    leave.current = setTimeout(() => setFlyout(null), 450);
  };
  // the drag ended anywhere (dropped, cancelled, left the window)
  useEffect(() => {
    if (!flyout) return;
    const end = () => setTimeout(() => setFlyout(null), 0);
    window.addEventListener('dragend', end);
    window.addEventListener('drop', end);
    return () => {
      window.removeEventListener('dragend', end);
      window.removeEventListener('drop', end);
    };
  }, [flyout]);
  useEffect(() => keep, []);

  const openFor = (e: React.DragEvent<HTMLElement>) => {
    const kind = dropKind(e);
    if (!kind) return;
    if (kind === 'media' && !showAlbums) return;
    if (kind === 'files' && (!info || info.caps.readonly)) return;
    keep();
    const top = e.currentTarget.getBoundingClientRect().top;
    setFlyout((f) => (f && f.kind === kind ? f : { kind, top }));
  };

  const albumList = albums.data ?? [];
  const libraries = libs.data?.libraries ?? [];
  const items: { to: string; icon: typeof Images; label: MessageKey; match: Match; drop?: boolean; show?: boolean }[] = [
    { to: '/', icon: Images, label: 'nav2.photos', match: photosMatch },
    { to: '/explore', icon: Compass, label: 'mobile.nav.explore', match: (p) => p.startsWith('/explore') },
    { to: '/favorites', icon: Heart, label: 'nav.favorites', match: (p) => p.startsWith('/favorites') },
    { to: '/albums', icon: BookImage, label: 'nav2.albums', match: albumsMatch, drop: true, show: showAlbums },
    { to: '/library', icon: LibraryGlyph, label: 'nav2.library', match: libraryMatch, drop: true },
  ];

  let flyBody: ReactNode = null;
  if (flyout?.kind === 'media')
    flyBody = albumList.length ? (
      albumList.slice(0, 14).map((a) => <DropAlbum key={a.id} album={a} onDone={close} />)
    ) : (
      <p className="px-2 py-2 text-[13px] text-dim">{t('lib2.hub.noAlbums')}</p>
    );
  else if (flyout?.kind === 'files') flyBody = libraries.map((l) => <DropLibrary key={l.id} lib={l} />);

  return (
    <aside className="relative z-20 flex h-full w-[var(--rail-w)] shrink-0 flex-col border-r border-line bg-elev/40" data-testid="nav-rail">
      <div className="app-drag grid h-[var(--topbar-h)] shrink-0 place-items-center">
        <LogoMark size={30} />
      </div>
      <nav aria-label={t('mobile.nav.label')} className="flex flex-col items-stretch gap-1.5 px-1.5 pt-2" onDragLeave={closeSoon}>
        {items
          .filter((i) => i.show !== false)
          .map((i) => (
            <RailItem key={i.to} to={i.to} icon={i.icon} label={t(i.label)} match={i.match} onDragEnter={i.drop ? openFor : undefined} />
          ))}
      </nav>

      <div className="mt-auto flex flex-col items-stretch gap-1.5 px-1.5 pb-3">
        {conn === 'offline' && (
          <span className="mx-auto grid size-9 place-items-center rounded-full bg-danger/10 text-danger" title={t('nav.offline')} role="status">
            <WifiOff className="size-4" />
          </span>
        )}
        {scan.scanning && scan.total > 0 && (
          <span className="mx-auto grid place-items-center py-1" title={t('nav.scanning', { done: scan.done, total: scan.total })} role="status">
            <ProgressRing value={scan.done / Math.max(1, scan.total)} size={22} stroke={2.5} />
          </span>
        )}
        <RailItem to="/settings" icon={Settings} label={t('nav.settings')} match={(p) => p.startsWith('/settings')} />
      </div>

      <AnimatePresence>
        {flyout && (
          <motion.div
            key="flyout"
            initial={{ opacity: 0, x: -8 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: -6, transition: { duration: 0.12 } }}
            transition={{ type: 'spring', stiffness: 520, damping: 36 }}
            onDragEnter={keep}
            onDragOver={keep}
            onDragLeave={closeSoon}
            className="absolute left-[calc(100%+6px)] z-[75] w-64 rounded-2xl border border-line bg-elev p-1.5 shadow-soft"
            style={{ top: Math.max(8, Math.min(flyout.top - 6, window.innerHeight - 360)) }}
            data-testid="drop-flyout"
          >
            <div className="px-2 pt-1.5 pb-1 text-[11px] font-semibold tracking-wider text-faint uppercase">{t(flyout.kind === 'media' ? 'nav2.dropAlbums' : 'nav2.dropLibraries')}</div>
            <div className="no-scrollbar flex max-h-[320px] flex-col gap-0.5 overflow-y-auto">{flyBody}</div>
          </motion.div>
        )}
      </AnimatePresence>
    </aside>
  );
}
