import { useEffect, useRef, useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { renderSVG } from 'uqr';
import { Archive, Cpu, DatabaseBackup, Download, Folder, Gauge, HardDrive, Info, Network, Plus, RotateCcw, Trash2, Upload, Wifi } from 'lucide-react';
import type { IndexIntensity, Library, Settings, ThumbQuality, ZipAction } from '@peekaround/shared';
import { useBackend, useConnection, useInfo, type CacheKind } from '@/backend';
import { desktop } from '@/backend/desktop';
import { useT } from '@/i18n';
import { formatBytes } from '@/lib/format';
import { LibraryIcon } from '@/lib/icons';
import { useLibraries, useSettings } from '@/lib/queries';
import { DEFAULT_PREFS, usePrefs } from '@/store/prefs';
import { toast } from '@/store/toasts';
import { Badge, Button, ConfirmDialog, Input, Segmented, Switch } from '@/components/ui';
import { LogoMark, LogoWordmark } from '@/components/Logo';
import { useSlideshow } from '@/features/slideshow/store';
import { DEFAULT_SETTINGS as SLIDE_DEFAULTS } from '@/features/slideshow/show';
import { RemoteCacheRow } from '@/features/remote/RemoteSettings';
import { MegaAccountCard } from '@/features/remote/MegaAccount';
import { AndroidConnectionCard } from '@/features/android/ConnectionCard';
import { DevicesCard } from '@/features/devices/DevicesCard';
import { PhoneShareCard } from '@/features/devices/PhoneShareCard';
import { LibraryEditor } from './LibraryEditor';
import { makeBackup, parseBackup, sanitizeServer, serverResetPatch } from './backup';
import { Card, Opaque, Row, Searchable, Section } from './parts';

/** server settings + a patch helper that keeps the query cache in sync */
export function useServerSettings() {
  const backend = useBackend();
  const info = useInfo();
  const qc = useQueryClient();
  const q = useSettings();
  const update = async (patch: Partial<Settings>) => {
    try {
      const next = await backend.updateSettings(patch);
      qc.setQueryData(['settings', backend.id], next);
      return next;
    } catch (e) {
      toast.error((e as Error).message);
      return undefined;
    }
  };
  return { s: q.data, ro: !!info?.caps.readonly, update };
}

// ───────────────────────── libraries & files ─────────────────────────

export function LibrariesSection() {
  const t = useT();
  const libs = useLibraries();
  const { s, ro, update } = useServerSettings();
  const [editing, setEditing] = useState<Library | 'new' | null>(null);
  const [suffix, setSuffix] = useState('');
  useEffect(() => {
    if (s) setSuffix(s.editSuffix);
  }, [s]);
  return (
    <Section id="libraries" title="pref.sec.libraries" icon={<Folder />}>
      <Card title={t('settings.libraries')} titleKey="settings.libraries" icon={<Folder />}>
        {libs.data?.libraries.map((l) => (
          <Searchable key={l.id} text={`${l.name} ${l.folders.map((f) => f.path).join(' ')}`} keys={['settings.libraries']}>
          <button type="button" data-row disabled={ro} onClick={() => setEditing(l)} className="flex items-center gap-3 py-3 text-left first:pt-0 hover:opacity-80">
            <span className="grid size-9 place-items-center rounded-xl text-white" style={{ background: l.color }}>
              <LibraryIcon name={l.icon} className="size-4" />
            </span>
            <span className="min-w-0 flex-1">
              <span className="block text-sm font-medium">{l.name}</span>
              <span className="block truncate font-mono text-[11px] text-faint">{l.folders.map((f) => f.path).join(' · ')}</span>
            </span>
            <Badge>{l.mediaCount}</Badge>
          </button>
          </Searchable>
        ))}
        {!ro && (
          <Searchable keys={['settings.libraries', 'nav.addLibrary']} text="folder klasör">
            <div className="pt-3" data-row>
              <Button variant="secondary" size="sm" icon={<Plus className="size-4" />} onClick={() => setEditing('new')}>
                {t('nav.addLibrary')}
              </Button>
            </div>
          </Searchable>
        )}
      </Card>
      {s && !ro && (
        <Card title={t('settings.files')} titleKey="settings.files" icon={<Archive />} scope="server">
          <Row label={t('settings.zipAction')} k="settings.zipAction" kw="zip archive arşiv" setting="zipAction">
            <Segmented<ZipAction>
              size="sm"
              value={s.zipAction}
              onChange={(zipAction) => void update({ zipAction })}
              options={[
                { value: 'ask', label: t('settings.zip.ask') },
                { value: 'extract', label: t('settings.zip.extract') },
                { value: 'temp', label: t('settings.zip.temp') },
              ]}
            />
          </Row>
          <Row label={t('settings.editSuffix')} k="settings.editSuffix" hint={t('settings.editSuffixHint', { suffix: suffix || '_edit' })} kw="edit copy kopya" setting="editSuffix">
            <Input value={suffix} onChange={(e) => setSuffix(e.target.value)} onBlur={() => suffix !== s.editSuffix && void update({ editSuffix: suffix })} className="w-32" />
          </Row>
          <Row label={t('settings.tempHours')} k="settings.tempHours" kw="temporary geçici" setting="tempSessionHours">
            <Input type="number" min={1} max={720} defaultValue={s.tempSessionHours} onBlur={(e) => void update({ tempSessionHours: Number(e.target.value) })} className="w-24" />
          </Row>
          <Row label={t('settings.showHidden')} k="settings.showHidden" kw="hidden dotfiles gizli" setting="showHidden">
            <Switch checked={s.showHidden} label={t('settings.showHidden')} onChange={(showHidden) => void update({ showHidden })} />
          </Row>
        </Card>
      )}
      <LibraryEditor value={editing} onClose={() => setEditing(null)} />
    </Section>
  );
}

// ───────────────────────── performance & cache ─────────────────────────

export function PerformanceSection() {
  const t = useT();
  const backend = useBackend();
  const info = useInfo();
  const { s, ro, update } = useServerSettings();
  const details = useQuery({ queryKey: ['cache-details', backend.id], queryFn: () => backend.cacheDetails!(), enabled: !!backend.cacheDetails, staleTime: 5_000 });
  const legacy = useQuery({ queryKey: ['cache', backend.id], queryFn: () => backend.cacheSize(), enabled: !backend.cacheDetails });
  const [busy, setBusy] = useState<string | null>(null);
  const tuning = backend.kind === 'http' && !!s && s.thumbQuality !== undefined;

  const clear = async (kind: CacheKind | 'all', name: string) => {
    setBusy(kind);
    try {
      if (kind === 'all') await backend.clearCache();
      else await backend.clearCacheKind!(kind);
      toast.success(t('pref.cache.cleared', { name }));
    } catch (e) {
      toast.error(t('common.error'), (e as Error).message);
    } finally {
      setBusy(null);
      void details.refetch();
      void legacy.refetch();
    }
  };
  const d = details.data;

  return (
    <Section id="performance" title="pref.sec.performance" icon={<Gauge />}>
      {tuning && !ro && (
        <Card title={t('pref.processing')} titleKey="pref.processing" icon={<Cpu />} scope="server">
          <Row label={t('pref.thumbQuality')} k="pref.thumbQuality" hint={t('pref.thumbQualityHint')} kw="webp jpeg thumbnail" setting="thumbQuality">
            <Segmented<ThumbQuality>
              size="sm"
              value={s!.thumbQuality}
              onChange={(thumbQuality) => void update({ thumbQuality })}
              options={[
                { value: 'low', label: t('pref.q.low') },
                { value: 'normal', label: t('pref.q.normal') },
                { value: 'high', label: t('pref.q.high') },
              ]}
            />
          </Row>
          <Row label={t('pref.intensity')} k="pref.intensity" hint={t('pref.intensityHint')} kw="cpu index scan tarama fan" setting="indexIntensity">
            <Segmented<IndexIntensity>
              size="sm"
              value={s!.indexIntensity}
              onChange={(indexIntensity) => void update({ indexIntensity })}
              options={[
                { value: 'low', label: t('pref.q.low') },
                { value: 'normal', label: t('pref.q.normal') },
                { value: 'high', label: t('pref.q.high') },
              ]}
            />
          </Row>
          {info?.caps.videoJobs && (
            <Row label={t('pref.hwAccel')} k="pref.hwAccel" hint={t('pref.hwAccelHint')} kw="gpu ffmpeg transcode nvenc vaapi" setting="hwAccel">
              <Switch checked={s!.hwAccel} label={t('pref.hwAccel')} onChange={(hwAccel) => void update({ hwAccel })} />
            </Row>
          )}
        </Card>
      )}
      <Card title={t('pref.cacheUsage')} titleKey="pref.cacheUsage" icon={<HardDrive />} scope={backend.kind === 'http' ? 'server' : undefined} testId="cache-card">
        {d ? (
          <>
            <Row label={t('pref.cache.thumbs')} k="pref.cache.thumbs" hint={`${formatBytes(d.thumbs.bytes)} · ${t('pref.cache.thumbsHint')}`} kw="cache önbellek thumbnail" setting="cache-thumbs">
              <Button variant="secondary" size="sm" icon={<Trash2 className="size-4" />} loading={busy === 'thumbs'} disabled={ro} onClick={() => void clear('thumbs', t('pref.cache.thumbs'))}>
                {t('pref.cache.clear')}
              </Button>
            </Row>
            <Row
              label={t('pref.cache.tiles')}
              k="pref.cache.tiles"
              hint={`${formatBytes(d.tiles.bytes)} · ${t('pref.cache.limit', { size: formatBytes((s?.tileCacheMb ?? d.tiles.limitMb) * 1024 * 1024) })}`}
              kw="cache map harita"
              setting="cache-tiles"
            >
              <Button variant="secondary" size="sm" icon={<Trash2 className="size-4" />} loading={busy === 'tiles'} disabled={ro} onClick={() => void clear('tiles', t('pref.cache.tiles'))}>
                {t('pref.cache.clear')}
              </Button>
            </Row>
            {/* MEGA stream cache: size cap (remote folders) + clear */}
            <Searchable keys={['pref.cache.remote', 'mega.cache.title']} text="mega cache önbellek">
              <div data-row>
                <RemoteCacheRow />
              </div>
            </Searchable>
            {d.remote.bytes > 0 && (
              <Row label={t('pref.cache.remote')} k="pref.cache.remote" hint={formatBytes(d.remote.bytes)} kw="mega cache" setting="cache-remote">
                <Button variant="secondary" size="sm" icon={<Trash2 className="size-4" />} loading={busy === 'remote'} disabled={ro} onClick={() => void clear('remote', t('pref.cache.remote'))}>
                  {t('pref.cache.clear')}
                </Button>
              </Row>
            )}
          </>
        ) : (
          <Row label={t('settings.cacheSize', { size: formatBytes(legacy.data ?? 0) })} k="settings.cache" kw="cache önbellek thumbnail">
            <Button variant="secondary" size="sm" loading={busy === 'all'} onClick={() => void clear('all', t('settings.cache'))}>
              {t('settings.clearCache')}
            </Button>
          </Row>
        )}
      </Card>
    </Section>
  );
}

// ───────────────────────── network & devices ─────────────────────────

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
    <Card icon={<Wifi />} title={t('settings.network')} titleKey="settings.network">
      {desktop && (
        <Row label={t('settings.lanShare')} k="settings.lanShare" hint={t('settings.lanShareHint')} kw="lan wifi port qr" setting="lan">
          <Input type="number" aria-label={t('settings.lanPort')} value={port} onChange={(e) => setPort(Number(e.target.value))} className="w-24" disabled={lan?.enabled} />
          <Switch
            checked={!!lan?.enabled}
            disabled={busy}
            label={t('settings.lanShare')}
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
        <Searchable keys={['settings.network', 'settings.lanUrls', 'settings.lanShare']} text="lan qr wifi">
        <div className="flex flex-wrap items-center gap-5 py-4" data-row>
          <Qr text={urls[0]} />
          <div className="min-w-0 flex-1">
            <div className="text-xs text-faint">{t('settings.lanUrls')}</div>
            {urls.map((u) => (
              <a key={u} href={u} target="_blank" rel="noreferrer" className="block font-mono text-sm break-all text-accent hover:underline">
                {u}
              </a>
            ))}
            <div className="mt-2 text-xs text-faint">{t('settings.scanQr')}</div>
          </div>
        </div>
        </Searchable>
      )}
    </Card>
  );
}

export function NetworkSection() {
  return (
    <Section id="network" title="pref.sec.network" icon={<Network />}>
      <Opaque keys={['devices.title']} kw="android phone telefon connection bağlantı">
        <AndroidConnectionCard />
      </Opaque>
      <Opaque keys={['devices.share.title']} kw="phone telefon share paylaş">
        <PhoneShareCard />
      </Opaque>
      <LanCard />
      <Opaque keys={['devices.title']} kw="phone telefon devices cihaz pair eşleştir">
        <DevicesCard />
      </Opaque>
      {/* optional MEGA account: transfers count against its quota instead of the per-IP one */}
      <MegaAccountCard />
    </Section>
  );
}

// ───────────────────────── about + backup ─────────────────────────

function BackupCard() {
  const t = useT();
  const backend = useBackend();
  const qc = useQueryClient();
  const { s, ro } = useServerSettings();
  const file = useRef<HTMLInputElement>(null);
  const [confirm, setConfirm] = useState(false);
  const serverWritable = backend.kind === 'http' && !ro;

  const pushServer = async (patch: Partial<Settings>) => {
    if (!serverWritable || !Object.keys(patch).length) return;
    const next = await backend.updateSettings(patch);
    qc.setQueryData(['settings', backend.id], next);
  };

  const exportNow = () => {
    const b = makeBackup(usePrefs.getState(), useSlideshow.getState().settings, s);
    const blob = new Blob([JSON.stringify(b, null, 2)], { type: 'application/json' });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = `peekaround-settings-${new Date().toISOString().slice(0, 10)}.json`;
    document.body.appendChild(a);
    a.click();
    a.remove();
    setTimeout(() => URL.revokeObjectURL(a.href), 2000);
  };

  const importFile = async (f: File) => {
    const b = parseBackup(await f.text());
    if (!b) {
      toast.error(t('pref.importFailed'));
      return;
    }
    usePrefs.getState().set(b.prefs);
    useSlideshow.getState().setSettings(b.slideshow);
    try {
      if (b.server) await pushServer(sanitizeServer(b.server));
      toast.success(t('pref.imported'));
    } catch (e) {
      toast.error(t('common.error'), (e as Error).message);
    }
  };

  return (
    <Card title={t('pref.backup')} titleKey="pref.backup" icon={<DatabaseBackup />} testId="backup-card">
      <Row label={t('pref.backup')} k="pref.backup" hint={t('pref.backupHint')} kw="export import json reset sıfırla yedek" stack>
        <Button variant="secondary" size="sm" icon={<Download className="size-4" />} onClick={exportNow} data-action="export-settings">
          {t('pref.export')}
        </Button>
        <Button variant="secondary" size="sm" icon={<Upload className="size-4" />} onClick={() => file.current?.click()} data-action="import-settings">
          {t('pref.import')}
        </Button>
        <Button variant="ghost" size="sm" icon={<RotateCcw className="size-4" />} onClick={() => setConfirm(true)} data-action="reset-settings">
          {t('pref.reset')}
        </Button>
        <input
          ref={file}
          type="file"
          accept="application/json,.json"
          hidden
          onChange={(e) => {
            const f = e.target.files?.[0];
            e.target.value = '';
            if (f) void importFile(f);
          }}
        />
      </Row>
      <ConfirmDialog
        open={confirm}
        danger
        title={t('pref.resetTitle')}
        body={t('pref.resetBody')}
        confirmLabel={t('pref.reset')}
        cancelLabel={t('common.cancel')}
        onClose={() => setConfirm(false)}
        onConfirm={async () => {
          const { music: _m, ...slide } = SLIDE_DEFAULTS;
          usePrefs.getState().set({ ...DEFAULT_PREFS, lang: usePrefs.getState().lang });
          useSlideshow.getState().setSettings(slide);
          try {
            await pushServer(serverResetPatch());
          } catch (e) {
            toast.error(t('common.error'), (e as Error).message);
            return;
          }
          toast.success(t('pref.resetDone'));
        }}
      />
    </Card>
  );
}

export function AboutSection() {
  const t = useT();
  const info = useInfo();
  const conn = useConnection();
  return (
    <Section id="about" title="pref.sec.about" icon={<Info />}>
      <Card title={t('settings.about')} titleKey="settings.about" icon={<Info />}>
        <Searchable keys={['settings.about', 'settings.version']} text="peekaround version">
        <div className="flex items-center gap-4 py-1 pb-3" data-row>
          <LogoMark size={48} />
          <div>
            <LogoWordmark className="text-2xl" />
            <div className="text-xs text-faint">
              {t('settings.version', { v: info?.version ?? '…' })} · {info ? t(`settings.mode.${info.mode}` as const) : ''} · {info?.hostname}
            </div>
          </div>
        </div>
        </Searchable>
        <Row label={t('settings.connection')} k="settings.connection" kw="version sürüm online">
          <Badge className={conn === 'online' ? 'text-ok' : 'text-danger'}>● {conn}</Badge>
          {info?.caps.readonly && <Badge>{t('settings.readonly')}</Badge>}
        </Row>
      </Card>
      <BackupCard />
    </Section>
  );
}
