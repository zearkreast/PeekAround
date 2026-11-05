import { useEffect, useState, type ReactNode } from 'react';
import { motion } from 'motion/react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { renderSVG } from 'uqr';
import { Archive, Folder, HardDrive, Info, Monitor, Moon, Palette, Plus, Sun, Wifi } from 'lucide-react';
import type { Library, Settings, ZipAction } from '@peekaround/shared';
import { useBackend, useConnection, useInfo } from '@/backend';
import { desktop } from '@/backend/desktop';
import { useT } from '@/i18n';
import { formatBytes } from '@/lib/format';
import { LibraryIcon } from '@/lib/icons';
import { useLibraries, useSettings } from '@/lib/queries';
import { useChrome } from '@/store/chrome';
import { ACCENTS, usePrefs } from '@/store/prefs';
import { toast } from '@/store/toasts';
import { Badge, Button, ColorSwatches, Input, Segmented, Switch } from '@/components/ui';
import { LogoMark, LogoWordmark } from '@/components/Logo';
import { LibraryEditor } from './LibraryEditor';
import { AndroidConnectionCard } from '@/features/android/ConnectionCard';

function Card({ icon, title, children, delay = 0 }: { icon: ReactNode; title: string; children: ReactNode; delay?: number }) {
  return (
    <motion.section
      initial={{ opacity: 0, y: 16 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ type: 'spring', stiffness: 260, damping: 28, delay }}
      className="rounded-3xl border border-line bg-elev p-5 sm:p-6"
    >
      <h2 className="mb-4 flex items-center gap-2.5 font-display text-lg font-semibold">
        <span className="grid size-8 place-items-center rounded-xl bg-surface-2 text-accent [&>svg]:size-4">{icon}</span>
        {title}
      </h2>
      <div className="flex flex-col divide-y divide-line">{children}</div>
    </motion.section>
  );
}

function Row({ label, hint, children }: { label: ReactNode; hint?: ReactNode; children: ReactNode }) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-x-6 gap-y-2 py-3.5 first:pt-0 last:pb-0">
      <div className="min-w-0 flex-1">
        <div className="text-sm font-medium">{label}</div>
        {hint && <div className="mt-0.5 text-xs text-faint">{hint}</div>}
      </div>
      <div className="flex shrink-0 items-center gap-2">{children}</div>
    </div>
  );
}

function Qr({ text }: { text: string }) {
  return <div className="size-28 shrink-0 overflow-hidden rounded-xl bg-white p-1.5 [&>svg]:size-full" dangerouslySetInnerHTML={{ __html: renderSVG(text, { border: 1 }) }} />;
}

function LanCard() {
  const t = useT();
  const info = useInfo();
  const qc = useQueryClient();
  const backend = useBackend();
  const [lan, setLan] = useState<{ enabled: boolean; port: number; urls: string[] } | null>(null);
  const [port, setPort] = useState(8080);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    desktop?.getLan().then((l) => {
      setLan(l);
      setPort(l.port);
    });
  }, []);
  const urls = desktop ? (lan?.urls ?? []) : (info?.lan.urls ?? []);
  if (!desktop && info?.mode !== 'server') return null;
  return (
    <Card icon={<Wifi />} title={t('settings.network')} delay={0.12}>
      {desktop && (
        <Row label={t('settings.lanShare')} hint={t('settings.lanShareHint')}>
          <Input type="number" value={port} onChange={(e) => setPort(Number(e.target.value))} className="w-24" disabled={lan?.enabled} />
          <Switch
            checked={!!lan?.enabled}
            disabled={busy}
            onChange={async (v) => {
              setBusy(true);
              const r = await desktop!.setLan(v, port);
              if (r.error) toast.error(t('common.error'), r.error);
              setLan({ enabled: r.enabled, port, urls: r.urls });
              setBusy(false);
              void qc.invalidateQueries({ queryKey: ['info', backend.id] });
            }}
          />
        </Row>
      )}
      {urls.length > 0 && (
        <div className="flex flex-wrap items-center gap-5 py-4">
          <Qr text={urls[0]} />
          <div className="min-w-0 flex-1">
            <div className="text-xs text-faint">{t('settings.lanUrls')}</div>
            {urls.map((u) => (
              <a key={u} href={u} target="_blank" rel="noreferrer" className="block font-mono text-sm text-accent hover:underline">
                {u}
              </a>
            ))}
            <div className="mt-2 text-xs text-faint">{t('settings.scanQr')}</div>
          </div>
        </div>
      )}
    </Card>
  );
}

export function SettingsPage() {
  const t = useT();
  const prefs = usePrefs();
  const backend = useBackend();
  const info = useInfo();
  const conn = useConnection();
  const qc = useQueryClient();
  const libs = useLibraries();
  const settingsQ = useSettings();
  const [editing, setEditing] = useState<Library | 'new' | null>(null);
  const setChrome = useChrome((s) => s.set);
  const cache = useQuery({ queryKey: ['cache', backend.id], queryFn: () => backend.cacheSize() });
  const s = settingsQ.data;
  const ro = !!info?.caps.readonly;
  const [suffix, setSuffix] = useState('');
  useEffect(() => setChrome({ crumbs: [], title: t('settings.title'), viewControls: false }), [setChrome, t]);
  useEffect(() => {
    if (s) setSuffix(s.editSuffix);
  }, [s]);

  const update = async (patch: Partial<Settings>) => {
    const next = await backend.updateSettings(patch);
    qc.setQueryData(['settings', backend.id], next);
  };

  return (
    <div className="mx-auto w-full max-w-3xl px-4 pt-8 pb-24 sm:px-7">
      <motion.h1 initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} className="mb-7 font-display text-3xl font-bold tracking-tight">
        {t('settings.title')}
      </motion.h1>
      <div className="flex flex-col gap-5">
        <Card icon={<Palette />} title={t('settings.appearance')}>
          <Row label={t('settings.language')}>
            <Segmented
              value={prefs.lang}
              onChange={(lang) => prefs.set({ lang })}
              options={[
                { value: 'tr', label: 'Türkçe' },
                { value: 'en', label: 'English' },
              ]}
            />
          </Row>
          <Row label={t('settings.theme')}>
            <Segmented
              value={prefs.theme}
              onChange={(theme) => prefs.set({ theme })}
              options={[
                { value: 'dark', label: t('settings.theme.dark'), icon: <Moon /> },
                { value: 'light', label: t('settings.theme.light'), icon: <Sun /> },
                { value: 'system', label: t('settings.theme.system'), icon: <Monitor /> },
              ]}
            />
          </Row>
          <Row label={t('settings.accent')}>
            <ColorSwatches value={prefs.accent} onChange={(accent) => prefs.set({ accent })} colors={ACCENTS} />
          </Row>
          <Row label={t('settings.motion')}>
            <Segmented
              value={prefs.motion}
              onChange={(motion) => prefs.set({ motion })}
              options={[
                { value: 'full', label: t('settings.motion.full') },
                { value: 'reduced', label: t('settings.motion.reduced') },
                { value: 'off', label: t('settings.motion.off') },
              ]}
            />
          </Row>
          <Row label={t('settings.hoverPreview')}>
            <Switch checked={prefs.hoverPreview} onChange={(hoverPreview) => prefs.set({ hoverPreview })} />
          </Row>
        </Card>

        <Card icon={<Folder />} title={t('settings.libraries')} delay={0.04}>
          {libs.data?.libraries.map((l) => (
            <button key={l.id} type="button" disabled={ro} onClick={() => setEditing(l)} className="flex items-center gap-3 py-3 text-left first:pt-0 hover:opacity-80">
              <span className="grid size-9 place-items-center rounded-xl text-white" style={{ background: l.color }}>
                <LibraryIcon name={l.icon} className="size-4" />
              </span>
              <span className="min-w-0 flex-1">
                <span className="block text-sm font-medium">{l.name}</span>
                <span className="block truncate font-mono text-[11px] text-faint">{l.folders.map((f) => f.path).join(' · ')}</span>
              </span>
              <Badge>{l.mediaCount}</Badge>
            </button>
          ))}
          {!ro && (
            <div className="pt-3">
              <Button variant="secondary" size="sm" icon={<Plus className="size-4" />} onClick={() => setEditing('new')}>
                {t('nav.addLibrary')}
              </Button>
            </div>
          )}
        </Card>

        {s && !ro && (
          <Card icon={<Archive />} title={t('settings.files')} delay={0.08}>
            <Row label={t('settings.zipAction')}>
              <Segmented<ZipAction>
                size="sm"
                value={s.zipAction}
                onChange={(zipAction) => update({ zipAction })}
                options={[
                  { value: 'ask', label: t('settings.zip.ask') },
                  { value: 'extract', label: t('settings.zip.extract') },
                  { value: 'temp', label: t('settings.zip.temp') },
                ]}
              />
            </Row>
            <Row label={t('settings.editSuffix')} hint={t('settings.editSuffixHint', { suffix: suffix || '_edit' })}>
              <Input value={suffix} onChange={(e) => setSuffix(e.target.value)} onBlur={() => suffix !== s.editSuffix && update({ editSuffix: suffix })} className="w-32" />
            </Row>
            <Row label={t('settings.tempHours')}>
              <Input type="number" min={1} max={720} defaultValue={s.tempSessionHours} onBlur={(e) => update({ tempSessionHours: Number(e.target.value) })} className="w-24" />
            </Row>
            <Row label={t('settings.showHidden')}>
              <Switch checked={s.showHidden} onChange={(showHidden) => update({ showHidden })} />
            </Row>
          </Card>
        )}

        <AndroidConnectionCard />
        <LanCard />

        <Card icon={<HardDrive />} title={t('settings.cache')} delay={0.16}>
          <Row label={t('settings.cacheSize', { size: formatBytes(cache.data ?? 0) })}>
            <Button
              variant="secondary"
              size="sm"
              onClick={async () => {
                await backend.clearCache();
                await cache.refetch();
                toast.success(t('settings.cacheCleared'));
              }}
            >
              {t('settings.clearCache')}
            </Button>
          </Row>
        </Card>

        <Card icon={<Info />} title={t('settings.about')} delay={0.2}>
          <div className="flex items-center gap-4 py-1">
            <LogoMark size={48} />
            <div>
              <LogoWordmark className="text-2xl" />
              <div className="text-xs text-faint">
                {t('settings.version', { v: info?.version ?? '…' })} · {info ? t(`settings.mode.${info.mode}` as const) : ''} · {info?.hostname}
              </div>
            </div>
          </div>
          <Row label={t('settings.connection')}>
            <Badge className={conn === 'online' ? 'text-ok' : 'text-danger'}>● {conn}</Badge>
            {info?.caps.readonly && <Badge>{t('settings.readonly')}</Badge>}
          </Row>
        </Card>
      </div>
      <LibraryEditor value={editing} onClose={() => setEditing(null)} />
    </div>
  );
}
