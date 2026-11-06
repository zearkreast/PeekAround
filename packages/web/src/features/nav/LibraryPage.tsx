import { useEffect, type ReactNode } from 'react';
import { Link, useNavigate } from 'react-router';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { motion } from 'motion/react';
import {
  ArrowRight,
  CalendarRange,
  ChevronRight,
  Clock3,
  Cloud,
  Copy,
  FolderPlus,
  HardDrive,
  Heart,
  Images,
  LayoutGrid,
  MapPin,
  Pencil,
  Plus,
  Presentation,
  Smartphone,
  Sparkles,
  Type,
  X,
} from 'lucide-react';
import type { FolderRef, Library } from '@peekaround/shared';
import { useBackend, useInfo } from '@/backend';
import { useLang, useT } from '@/i18n';
import { durationUntil, formatBytes } from '@/lib/format';
import { LibraryIcon } from '@/lib/icons';
import { useBrowse, useLibraries } from '@/lib/queries';
import { folderHref, libraryHref } from '@/lib/routes';
import { useChrome } from '@/store/chrome';
import { Button, IconButton } from '@/components/ui';
import { OnlineDot } from '@/features/devices/DevicesNav';
import { useLinkedDevices } from '@/features/devices/useDevices';
import { useRemote } from '@/features/remote/RemoteHost';
import { RemoteBadge } from '@/features/remote/RemoteFolders';
import { AlbumCard, rangeLabel } from '@/features/library/AlbumCard';
import { albumsSupported, refreshAlbums, useAlbums } from '@/features/library/albums';
import { addToAlbum } from '@/features/library/AlbumPicker';
import { useLibraryUi } from '@/features/library/LibraryHost';
import { stagger, useEnterOnce } from '@/features/library/motion';
import { heroNavigate } from '@/features/library/vt';
import { NewAlbumButton } from '@/features/library/AlbumsPage';

function Section({ title, action, children, id }: { title: ReactNode; action?: ReactNode; children: ReactNode; id?: string }) {
  return (
    <section className="mb-9" data-section={id}>
      <div className="mb-3 flex min-h-9 items-center justify-between gap-3 px-1">
        <h2 className="font-display text-[19px] font-semibold tracking-tight sm:text-xl">{title}</h2>
        {action}
      </div>
      {children}
    </section>
  );
}

function Row({ to, icon, title, sub, onClick, trailing }: { to?: string; icon: ReactNode; title: ReactNode; sub?: ReactNode; onClick?: () => void; trailing?: ReactNode }) {
  const body = (
    <>
      <span className="grid size-11 shrink-0 place-items-center rounded-2xl bg-surface-2 text-dim [&>svg]:size-[22px]">{icon}</span>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[15px] font-medium text-fg">{title}</span>
        {sub && <span className="block truncate text-[13px] text-dim">{sub}</span>}
      </span>
      {trailing ?? <ChevronRight className="size-5 shrink-0 text-faint" />}
    </>
  );
  const cls = 'flex min-h-[60px] w-full items-center gap-3.5 rounded-2xl px-3 py-2 text-left transition-colors active:bg-surface-2 hover:bg-surface';
  return to ? (
    <Link to={to} className={cls} onClick={onClick}>
      {body}
    </Link>
  ) : (
    <button type="button" className={cls} onClick={onClick}>
      {body}
    </button>
  );
}

/** library card: cover (picked or newest), name, numbers; cover morphs into the library page banner */
function LibraryCard({ lib, index, animate }: { lib: Library; index: number; animate: boolean }) {
  const t = useT();
  const lang = useLang();
  const backend = useBackend();
  const info = useInfo();
  const nav = useNavigate();
  const { first } = useBrowse({ libraryId: lib.id, recursive: true, limit: 4, sort: 'takenAt', order: 'desc', kind: 'all' }, !lib.coverId);
  const cover = lib.coverId ?? first?.items.find((i) => i.kind === 'image' || i.kind === 'video')?.id ?? null;
  const stats = useQuery({ queryKey: ['libraries', backend.id, 'stats', lib.id], queryFn: () => backend.libraryStats!(lib.id), enabled: !!backend.libraryStats, staleTime: 30_000 });
  const s = stats.data;
  const range = s ? rangeLabel(s.from, s.to, lang) : null;
  const remote = lib.folders.find((f) => f.remote)?.remote;
  return (
    <motion.div
      initial={animate ? { opacity: 0, y: 18, scale: 0.98 } : false}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      transition={{ type: 'spring', stiffness: 300, damping: 30, delay: animate ? stagger(index, 0.05) : 0 }}
      className="group relative"
    >
      <Link
        to={libraryHref(lib.id)}
        onClick={(e) => heroNavigate(e, nav, libraryHref(lib.id), `lib-${lib.id}`)}
        className="block"
        data-drop-folder={lib.folders.length === 1 && !lib.folders[0].remote && !lib.folders[0].device ? lib.folders[0].id : undefined}
        data-drop-dir=""
        data-drop-label={lib.name}
        data-testid="hub-library"
      >
        <div className="card-lift relative aspect-[16/9] overflow-hidden rounded-3xl border border-line bg-elev">
          <div className="absolute inset-0" data-hero>
            {cover ? (
              <img
                src={backend.thumbUrl(cover, 640)}
                alt=""
                draggable={false}
                decoding="async"
                onLoad={(e) => (e.currentTarget.dataset.loaded = '1')}
                className="fade-img size-full object-cover transition-transform duration-700 ease-[var(--ease-out)] group-hover:scale-[1.04]"
              />
            ) : (
              <div className="size-full" style={{ background: `linear-gradient(135deg, ${lib.color}, color-mix(in oklab, ${lib.color} 20%, var(--bg-elev)))` }} />
            )}
          </div>
          <div className="absolute inset-0 bg-gradient-to-t from-black/85 via-black/20 to-transparent" />
          <div className="absolute inset-x-0 bottom-0 flex items-end gap-3 p-3.5 text-white sm:p-4">
            <span className="grid size-9 shrink-0 place-items-center rounded-xl shadow-lg ring-1 ring-white/20 sm:size-10" style={{ background: lib.color }}>
              <LibraryIcon name={lib.icon} className="size-4 sm:size-5" />
            </span>
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-1.5">
                <span className="truncate font-display text-base font-semibold sm:text-lg">{lib.name}</span>
                {remote && <RemoteBadge remote={remote} />}
              </div>
              <div className="flex flex-wrap items-center gap-x-2.5 text-xs text-white/75 tabular-nums">
                <span>{t('lib2.stats.items', { n: (s?.items ?? lib.mediaCount).toLocaleString() })}</span>
                {!!s?.bytes && (
                  <span className="hidden items-center gap-1 sm:inline-flex">
                    <HardDrive className="size-3" />
                    {formatBytes(s.bytes)}
                  </span>
                )}
                {range && (
                  <span className="hidden items-center gap-1 md:inline-flex">
                    <CalendarRange className="size-3" />
                    {range}
                  </span>
                )}
              </div>
            </div>
            <ArrowRight className="mb-1 size-5 shrink-0 -translate-x-2 opacity-0 transition-all duration-300 group-hover:translate-x-0 group-hover:opacity-100" />
          </div>
        </div>
      </Link>
      {info && !info.caps.readonly && (
        <IconButton
          label={t('lib2.edit')}
          size="sm"
          tone="glass"
          className="absolute top-2.5 right-2.5 opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100 max-md:opacity-100"
          onClick={() => useLibraryUi.getState().edit(lib)}
        >
          <Pencil />
        </IconButton>
      )}
    </motion.div>
  );
}

function TempRow({ f }: { f: FolderRef }) {
  const t = useT();
  const lang = useLang();
  const backend = useBackend();
  return (
    <Row
      to={folderHref(f.id)}
      icon={<Clock3 className="text-warn" />}
      title={f.sourceName ?? f.name}
      sub={f.expiresAt ? t('mobile.library.expires', { time: durationUntil(f.expiresAt, lang) }) : undefined}
      trailing={
        <IconButton
          label={t('temp.close')}
          onClick={(e) => {
            e.preventDefault();
            e.stopPropagation();
            void backend.removeFolder(f.id);
          }}
        >
          <X />
        </IconButton>
      }
    />
  );
}

function QuickTile({ to, icon, label, tint }: { to: string; icon: ReactNode; label: string; tint: string }) {
  return (
    <Link
      to={to}
      className="group flex min-h-[56px] items-center gap-3 rounded-2xl border border-line bg-surface px-4 text-[15px] font-medium transition-[background,border-color,transform] duration-200 hover:border-line-strong hover:bg-surface-2 active:scale-[0.98] [&_svg]:size-[20px]"
    >
      <span className="grid size-9 place-items-center rounded-xl transition-transform duration-300 group-hover:scale-110" style={{ background: `color-mix(in oklab, ${tint} 18%, transparent)`, color: tint }}>
        {icon}
      </span>
      {label}
    </Link>
  );
}

/**
 * Library hub ("Kitaplık"): albums, libraries (with numbers), quick collections, phones,
 * temporary sessions and tools. The mobile Library tab and the desktop "Library" page.
 */
export default function LibraryPage() {
  const t = useT();
  const info = useInfo();
  const nav = useNavigate();
  const backend = useBackend();
  const qc = useQueryClient();
  const libs = useLibraries();
  const albums = useAlbums();
  const setChrome = useChrome((s) => s.set);
  const canEdit = !!info && !info.caps.readonly;
  const libraries = libs.data?.libraries ?? [];
  const temp = libs.data?.temp ?? [];
  const devices = useLinkedDevices();
  const total = libraries.reduce((a, l) => a + l.mediaCount, 0);
  const animateLibs = useEnterOnce('hub-libraries', libs.isSuccess);
  const animateAlbums = useEnterOnce('hub-albums', albums.isSuccess);
  const showAlbums = albumsSupported(backend);
  const albumList = albums.data ?? [];

  useEffect(() => setChrome({ crumbs: [], title: t('lib2.hub.title'), viewControls: false }), [setChrome, t]);

  return (
    <div className="mx-auto w-full max-w-[1500px] px-3 pt-4 pb-28 sm:px-7 sm:pt-6">
      <motion.header initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={{ type: 'spring', stiffness: 260, damping: 30 }} className="mb-6 px-1">
        <h1 className="font-display text-[26px] font-bold tracking-tight sm:text-[34px]">{t('lib2.hub.title')}</h1>
        {libraries.length > 0 && (
          <p className="mt-1 text-sm text-dim tabular-nums">{t('lib2.hub.subtitle', { n: libraries.length, items: t('common.items', { n: total.toLocaleString() }) })}</p>
        )}
      </motion.header>

      <div className="mb-9 grid grid-cols-2 gap-2.5 md:grid-cols-4">
        <QuickTile to="/favorites" icon={<Heart />} label={t('nav.favorites')} tint="var(--accent-2)" />
        <QuickTile to="/explore/recent" icon={<Sparkles />} label={t('lib2.hub.recent')} tint="var(--accent)" />
        <QuickTile to="/explore/places" icon={<MapPin />} label={t('lib2.hub.places')} tint="#10b981" />
        {backend.duplicates ? (
          <QuickTile to="/explore/duplicates" icon={<Copy />} label={t('lib2.hub.duplicates')} tint="#f97316" />
        ) : (
          <QuickTile to="/explore" icon={<Images />} label={t('mobile.nav.explore')} tint="#f97316" />
        )}
      </div>

      {showAlbums && (
        <Section
          id="albums"
          title={
            <Link to="/albums" className="inline-flex items-center gap-1 hover:text-accent">
              {t('lib2.hub.albums')}
              <ChevronRight className="size-5 text-faint" />
            </Link>
          }
          action={<NewAlbumButton />}
        >
          {albums.isSuccess && !albumList.length ? (
            <p className="px-1 text-sm text-dim">{t('lib2.hub.noAlbums')}</p>
          ) : (
            <div className="grid grid-cols-2 gap-x-3 gap-y-5 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 2xl:grid-cols-6" data-testid="hub-albums">
              {albumList.slice(0, 12).map((a, i) => (
                <AlbumCard
                  key={a.id}
                  album={a}
                  index={i}
                  animate={animateAlbums}
                  onDropIds={(ids) => void addToAlbum(backend, qc, a, ids, t).then(() => refreshAlbums(qc, [a.id]))}
                />
              ))}
            </div>
          )}
        </Section>
      )}

      <Section
        id="libraries"
        title={t('lib2.hub.libraries')}
        action={
          canEdit && (
            <div className="flex items-center gap-1">
              {backend.remoteAddFolder && (
                <Button variant="ghost" size="sm" icon={<Cloud className="size-4 text-[#e5484d]" />} title={t('mega.addLinkHint')} aria-label={t('mega.addLink')} onClick={() => useRemote.getState().show(null, { intent: 'stream' })}>
                  <span className="hidden sm:inline">{t('mega.addLink')}</span>
                </Button>
              )}
              <Button variant="ghost" size="sm" icon={<FolderPlus className="size-4" />} aria-label={t('nav.addLibrary')} onClick={() => useLibraryUi.getState().edit('new')}>
                <span className="hidden sm:inline">{t('nav.addLibrary')}</span>
              </Button>
            </div>
          )
        }
      >
        {libs.isSuccess && !libraries.length && (
          <div className="flex flex-col items-start gap-3 rounded-3xl border border-dashed border-line px-5 py-6">
            <p className="text-sm text-dim">{t('home.empty.body')}</p>
            {canEdit && (
              <Button variant="primary" icon={<Plus className="size-4" />} onClick={() => useLibraryUi.getState().edit('new')}>
                {t('home.empty.cta')}
              </Button>
            )}
          </div>
        )}
        <div className="grid grid-cols-1 gap-3.5 sm:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-4">
          {libraries.map((l, i) => (
            <LibraryCard key={l.id} lib={l} index={i} animate={animateLibs} />
          ))}
        </div>
      </Section>

      {devices.length > 0 && (
        <Section id="devices" title={t('lib2.hub.devices')}>
          <div className="grid grid-cols-1 gap-1 md:grid-cols-2" data-testid="devices-library">
            {devices.flatMap((d) =>
              d.folders.map((f) => (
                <div key={f.id} className={d.online ? '' : 'opacity-60'} title={d.online ? undefined : t('devices.offlineTip')}>
                  <Row
                    to={folderHref(f.id)}
                    icon={<Smartphone />}
                    title={f.name}
                    sub={
                      <span className="flex items-center gap-1.5">
                        <OnlineDot online={d.online} />
                        <span className="truncate">
                          {d.name} · {d.online ? t('common.items', { n: f.mediaCount }) : t('devices.offlineTip')}
                        </span>
                      </span>
                    }
                  />
                </div>
              )),
            )}
          </div>
        </Section>
      )}

      {temp.length > 0 && (
        <Section id="temp" title={t('lib2.hub.temp')}>
          <div className="grid grid-cols-1 gap-1 md:grid-cols-2">
            {temp.map((f) => (
              <TempRow key={f.id} f={f} />
            ))}
          </div>
        </Section>
      )}

      <Section id="tools" title={t('lib2.hub.tools')}>
        <div className="grid grid-cols-1 gap-1 md:grid-cols-3">
          <Row icon={<LayoutGrid />} title={t('nav.collage')} onClick={() => nav('/collage')} />
          <Row icon={<Presentation />} title={t('nav.slideshow')} onClick={() => nav('/slideshow')} />
          <Row icon={<Type />} title={t('nav.fonts')} onClick={() => nav('/fonts')} />
        </div>
      </Section>
    </div>
  );
}
