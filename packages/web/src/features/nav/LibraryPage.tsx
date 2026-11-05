import { useEffect, useState, type ReactNode } from 'react';
import { Link, useNavigate } from 'react-router';
import { ChevronRight, Clock3, FolderOpen, FolderPlus, Heart, Home, LayoutGrid, Presentation, Smartphone, Type, X } from 'lucide-react';
import type { FolderRef, Library } from '@peekaround/shared';
import { useBackend, useInfo } from '@/backend';
import { useLang, useT } from '@/i18n';
import { durationUntil } from '@/lib/format';
import { LibraryIcon } from '@/lib/icons';
import { useBrowse, useLibraries } from '@/lib/queries';
import { folderHref, libraryHref } from '@/lib/routes';
import { useChrome } from '@/store/chrome';
import { Button, IconButton } from '@/components/ui';
import { LibraryEditor } from '@/features/settings/LibraryEditor';
import { OnlineDot } from '@/features/devices/DevicesNav';
import { useLinkedDevices } from '@/features/devices/useDevices';

function Section({ title, action, children }: { title: string; action?: ReactNode; children: ReactNode }) {
  return (
    <section className="mb-7">
      <div className="mb-2.5 flex min-h-9 items-center justify-between px-1">
        <h2 className="text-[13px] font-semibold tracking-[0.06em] text-faint uppercase">{title}</h2>
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

function LibraryCard({ lib }: { lib: Library }) {
  const t = useT();
  const backend = useBackend();
  const { first } = useBrowse({ libraryId: lib.id, recursive: true, limit: 4, sort: 'takenAt', order: 'desc', kind: 'all' });
  const covers = (first?.items ?? []).filter((i) => i.kind !== 'audio').slice(0, 4);
  return (
    <Link
      to={libraryHref(lib.id)}
      className="group relative block aspect-[4/3] overflow-hidden rounded-3xl border border-line bg-elev"
      data-drop-folder={lib.folders.length === 1 ? lib.folders[0].id : undefined}
      data-drop-dir=""
      data-drop-label={lib.name}
    >
      <div className="absolute inset-0 grid grid-cols-2 grid-rows-2 gap-0.5">
        {covers.length ? (
          covers.map((c, i) => (
            <img
              key={c.id}
              src={backend.thumbUrl(c.id, 320)}
              alt=""
              draggable={false}
              className={`size-full object-cover ${covers.length === 1 ? 'col-span-2 row-span-2' : covers.length === 2 ? 'row-span-2' : covers.length === 3 && i === 0 ? 'row-span-2' : ''}`}
            />
          ))
        ) : (
          <div className="col-span-2 row-span-2" style={{ background: `linear-gradient(135deg, ${lib.color}, transparent)` }} />
        )}
      </div>
      <div className="absolute inset-0 bg-gradient-to-t from-black/80 via-black/10 to-transparent" />
      <div className="absolute inset-x-0 bottom-0 flex items-center gap-2.5 p-3 text-white">
        <span className="grid size-8 shrink-0 place-items-center rounded-xl" style={{ background: lib.color }}>
          <LibraryIcon name={lib.icon} className="size-4" />
        </span>
        <div className="min-w-0">
          <div className="truncate text-[15px] font-semibold">{lib.name}</div>
          <div className="text-xs text-white/70 tabular-nums">{t('common.items', { n: lib.mediaCount.toLocaleString() })}</div>
        </div>
      </div>
    </Link>
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

/**
 * Mobile "Library" tab: libraries, their folders, temporary sessions and tools in big,
 * thumb-friendly rows. (Also works as a page on desktop.)
 */
export default function LibraryPage() {
  const t = useT();
  const info = useInfo();
  const nav = useNavigate();
  const libs = useLibraries();
  const setChrome = useChrome((s) => s.set);
  const [adding, setAdding] = useState(false);
  const [openLib, setOpenLib] = useState<string | null>(null);
  const canEdit = !!info && !info.caps.readonly;
  const libraries = libs.data?.libraries ?? [];
  const temp = libs.data?.temp ?? [];
  const devices = useLinkedDevices();

  useEffect(() => setChrome({ crumbs: [], title: t('mobile.library.title'), viewControls: false }), [setChrome, t]);

  return (
    <div className="mx-auto w-full max-w-3xl px-3 pt-4 pb-28 sm:px-6">
      <Section title={t('mobile.library.quick')}>
        <div className="grid grid-cols-2 gap-2.5">
          {[
            { to: '/favorites', icon: <Heart className="text-[var(--accent-2)]" />, label: t('nav.favorites') },
            { to: '/', icon: <Home className="text-accent" />, label: t('nav.home') },
          ].map((q) => (
            <Link key={q.to} to={q.to} className="flex min-h-[56px] items-center gap-3 rounded-2xl border border-line bg-surface px-4 text-[15px] font-medium active:bg-surface-2 [&_svg]:size-[22px]">
              {q.icon}
              {q.label}
            </Link>
          ))}
        </div>
      </Section>

      <Section
        title={t('mobile.library.libraries')}
        action={
          canEdit && (
            <Button size="sm" variant="ghost" icon={<FolderPlus className="size-4" />} onClick={() => setAdding(true)} className="min-h-10">
              {t('nav.addLibrary')}
            </Button>
          )
        }
      >
        {libs.isSuccess && !libraries.length && <p className="px-1 text-sm text-dim">{t('mobile.library.empty')}</p>}
        <div className="grid grid-cols-2 gap-2.5 sm:grid-cols-3">
          {libraries.map((l) => (
            <LibraryCard key={l.id} lib={l} />
          ))}
        </div>
      </Section>

      {libraries.some((l) => l.folders.length > 0) && (
        <Section title={t('mobile.library.folders')}>
          <div className="flex flex-col">
            {libraries.map((l) =>
              l.folders.length === 1 ? (
                <Row key={l.id} to={folderHref(l.folders[0].id)} icon={<FolderOpen />} title={l.folders[0].name} sub={`${l.name} · ${t('common.items', { n: l.folders[0].mediaCount })}`} />
              ) : (
                <div key={l.id}>
                  <Row
                    icon={<LibraryIcon name={l.icon} />}
                    title={l.name}
                    sub={t('common.items', { n: l.mediaCount })}
                    onClick={() => setOpenLib((o) => (o === l.id ? null : l.id))}
                    trailing={<ChevronRight className={`size-5 shrink-0 text-faint transition-transform ${openLib === l.id ? 'rotate-90' : ''}`} />}
                  />
                  {openLib === l.id && (
                    <div className="ml-6 border-l border-line pl-2">
                      {l.folders.map((f) => (
                        <Row key={f.id} to={folderHref(f.id)} icon={<FolderOpen />} title={f.name} sub={t('common.items', { n: f.mediaCount })} />
                      ))}
                    </div>
                  )}
                </div>
              ),
            )}
          </div>
        </Section>
      )}

      {devices.length > 0 && (
        <Section title={t('devices.title')}>
          <div className="flex flex-col" data-testid="devices-library">
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
        <Section title={t('mobile.library.temp')}>
          <div className="flex flex-col">
            {temp.map((f) => (
              <TempRow key={f.id} f={f} />
            ))}
          </div>
        </Section>
      )}

      <Section title={t('mobile.library.tools')}>
        <div className="flex flex-col">
          <Row icon={<LayoutGrid />} title={t('nav.collage')} onClick={() => nav('/collage')} />
          <Row icon={<Presentation />} title={t('nav.slideshow')} onClick={() => nav('/slideshow')} />
          <Row icon={<Type />} title={t('nav.fonts')} onClick={() => nav('/fonts')} />
        </div>
      </Section>

      <LibraryEditor value={adding ? 'new' : null} onClose={() => setAdding(false)} />
    </div>
  );
}
