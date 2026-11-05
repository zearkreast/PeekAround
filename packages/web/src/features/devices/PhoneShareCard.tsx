import { useCallback, useEffect, useMemo, useState } from 'react';
import { motion } from 'motion/react';
import { Bell, CircleAlert, FolderPlus, Image, Link2, Loader2, MapPin, MonitorSmartphone, Radar, RotateCcw, ShieldCheck, Wifi, X } from 'lucide-react';
import { loadMode } from '@/backend/android/config';
import { PeekLink, type LinkStatus } from '@/backend/android/link-plugin';
import { PeekLocal, isNative, type MediaPermissions } from '@/backend/android/plugin';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { toast } from '@/store/toasts';
import { Button, Input, Switch } from '@/components/ui';
import { PhoneFolderPicker } from './PhoneFolderPicker';
import { baseName, prettyPath } from './phonePicker';

const DRAFT = 'peek.link.draft';

interface Draft {
  url: string;
  folders: { name: string; path: string }[];
}

function loadDraft(): Draft {
  try {
    return { url: '', folders: [], ...JSON.parse(localStorage.getItem(DRAFT) ?? '{}') };
  } catch {
    return { url: '', folders: [] };
  }
}
function saveDraft(d: Draft) {
  try {
    localStorage.setItem(DRAFT, JSON.stringify(d));
  } catch {
    /* ignore */
  }
}

/** Settings → "Telefon paylaşımı" (Android app, both modes): share phone folders with a PeekAround PC. */
export function PhoneShareCard() {
  return isNative() ? <PhoneShare /> : null;
}

function PhoneShare() {
  const t = useT();
  const labels = useMemo(() => ({ internal: t('devices.picker.internal'), sd: t('devices.picker.sd') }), [t]);
  const mode = loadMode();
  const client = mode?.mode === 'server' ? mode.url : null;
  const [st, setSt] = useState<LinkStatus | null>(null);
  const [missing, setMissing] = useState(false);
  const [draft, setDraft] = useState<Draft>(loadDraft);
  const [name, setName] = useState('');
  const [picking, setPicking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [perms, setPerms] = useState<MediaPermissions | null>(null);
  const [servers, setServers] = useState<{ name: string; url: string }[] | null>(null);
  const [scanning, setScanning] = useState(false);

  useEffect(() => {
    let off: (() => void) | null = null;
    PeekLink.status()
      .then((s) => {
        setSt(s);
        setName(s.deviceName);
      })
      .catch(() => setMissing(true));
    PeekLink.addListener('link', (s) => setSt(s))
      .then((h) => (off = () => void h.remove()))
      .catch(() => {});
    PeekLocal.checkAccess()
      .then((r) => setPerms(r.media))
      .catch(() => {});
    return () => off?.();
  }, []);

  const enabled = !!st?.enabled;
  const folders = enabled || (st?.folders.length && !draft.folders.length) ? (st?.folders ?? []).map((f) => ({ name: f.name, path: f.path })) : draft.folders;
  const url = client ?? (st?.server && !draft.url ? st.server : draft.url);

  const patchDraft = (p: Partial<Draft>) => {
    const next = { ...draft, ...p };
    setDraft(next);
    saveDraft(next);
  };

  const run = useCallback(
    async (fn: () => Promise<LinkStatus>) => {
      setBusy(true);
      try {
        setSt(await fn());
      } catch (e) {
        toast.error(t('common.error'), (e as Error).message);
      } finally {
        setBusy(false);
      }
    },
    [t],
  );

  const start = (fs = folders) => {
    if (!url) return toast.error(t('devices.share.noPc'));
    if (!fs.length) return setPicking(true);
    void run(() => PeekLink.start({ url, folders: fs, name: name.trim() || undefined }));
  };

  const setFolders = (fs: { name: string; path: string }[]) => {
    patchDraft({ folders: fs });
    if (enabled) void run(() => (fs.length ? PeekLink.setFolders({ folders: fs }) : PeekLink.stop()));
  };

  const scan = async () => {
    setScanning(true);
    try {
      setServers((await PeekLocal.discover({ timeout: 4000 })).servers);
    } catch {
      setServers([]);
    } finally {
      setScanning(false);
    }
  };

  if (missing) return null;

  const askPerms = async () => {
    try {
      setPerms(await PeekLocal.requestMediaPermissions());
    } catch (e) {
      toast.error(t('common.error'), (e as Error).message);
    }
  };

  return (
    <motion.section initial={{ opacity: 0, y: 16 }} animate={{ opacity: 1, y: 0 }} className="rounded-3xl border border-line bg-elev p-5 sm:p-6" data-testid="phone-share-card">
      <div className="mb-1 flex items-center gap-3">
        <h2 className="flex flex-1 items-center gap-2.5 font-display text-lg font-semibold">
          <span className="grid size-8 place-items-center rounded-xl bg-surface-2 text-accent">
            <MonitorSmartphone className="size-4" />
          </span>
          {t('devices.share.title')}
        </h2>
        {busy && <Loader2 className="size-4 animate-spin text-faint" />}
        <Switch checked={enabled} disabled={busy || !st} label={t('devices.share.title')} onChange={(v) => (v ? start() : void run(() => PeekLink.stop()))} />
      </div>
      <p className="mb-4 text-xs text-faint">{t('devices.share.hint')}</p>

      {st && <StatusLine st={st} />}

      {/* PC */}
      <Block label={t('devices.share.pc')}>
        {client ? (
          <div className="flex items-center gap-2 rounded-2xl bg-surface px-3 py-2.5 text-sm">
            <Link2 className="size-4 shrink-0 text-accent" />
            <span className="min-w-0 flex-1 truncate font-mono text-[13px]">{client}</span>
            <span className="shrink-0 text-xs text-faint">{t('devices.share.connectedPc')}</span>
          </div>
        ) : (
          <div className="flex flex-col gap-2">
            <div className="flex gap-2">
              <Input
                value={draft.url || (st?.server ?? '')}
                onChange={(e) => patchDraft({ url: e.target.value })}
                inputMode="url"
                placeholder="http://192.168.1.10:47213"
                className="flex-1"
                disabled={enabled}
              />
              <Button variant="secondary" icon={scanning ? <Loader2 className="size-4 animate-spin" /> : <Radar className="size-4" />} onClick={scan} disabled={enabled || scanning}>
                {t('devices.share.find')}
              </Button>
            </div>
            {servers && !enabled && (
              <div className="flex flex-col gap-1.5">
                {servers.map((s) => (
                  <button
                    key={s.url}
                    type="button"
                    onClick={() => patchDraft({ url: s.url })}
                    className={cn('flex min-h-11 items-center gap-2.5 rounded-xl border px-3 text-left text-sm', draft.url === s.url ? 'border-[color-mix(in_oklab,var(--accent)_60%,transparent)] bg-[color-mix(in_oklab,var(--accent)_10%,transparent)]' : 'border-line bg-surface')}
                  >
                    <Wifi className="size-4 text-accent" />
                    <span className="min-w-0 flex-1 truncate">{s.name}</span>
                    <span className="font-mono text-xs text-faint">{s.url}</span>
                  </button>
                ))}
                {!servers.length && <p className="text-xs text-faint">{t('devices.share.noneFound')}</p>}
              </div>
            )}
          </div>
        )}
      </Block>

      {/* folders */}
      <Block
        label={t('devices.share.folders')}
        action={
          <Button size="sm" variant="ghost" icon={<FolderPlus className="size-4" />} onClick={() => setPicking(true)} className="min-h-10">
            {t('devices.share.addFolders')}
          </Button>
        }
      >
        {!folders.length && <p className="rounded-2xl bg-surface p-3 text-sm text-dim">{t('devices.share.noFolders')}</p>}
        <div className="flex flex-col gap-1.5">
          {folders.map((f) => (
            <div key={f.path} className="flex min-h-12 items-center gap-3 rounded-2xl bg-surface px-3">
              <Image className="size-4 shrink-0 text-accent" />
              <div className="min-w-0 flex-1">
                <div className="truncate text-sm font-medium">{f.name}</div>
                <div className="truncate text-xs text-faint">{prettyPath(f.path, labels)}</div>
              </div>
              <button
                type="button"
                aria-label={t('common.remove')}
                className="grid size-10 place-items-center rounded-xl text-faint hover:text-fg"
                onClick={() => setFolders(folders.filter((x) => x.path !== f.path))}
              >
                <X className="size-4" />
              </button>
            </div>
          ))}
        </div>
      </Block>

      {/* device name */}
      <Block label={t('devices.share.name')}>
        <Input
          value={name}
          maxLength={60}
          onChange={(e) => setName(e.target.value)}
          onBlur={() => {
            if (st && name.trim() && name.trim() !== st.deviceName) void run(() => PeekLink.setName({ name: name.trim() }));
          }}
          placeholder={st?.model ?? ''}
        />
      </Block>

      {/* permissions */}
      {perms && <Permissions perms={perms} onAsk={askPerms} />}

      {st && (st.paired || st.state === 'error') && (
        <div className="mt-4 flex justify-end">
          <Button size="sm" variant="ghost" icon={<RotateCcw className="size-4" />} onClick={() => void run(() => PeekLink.forget())}>
            {t('devices.share.reset')}
          </Button>
        </div>
      )}

      <PhoneFolderPicker
        open={picking}
        mode="folders"
        purpose="share"
        title={t('devices.share.pickTitle')}
        initial={folders.map((f) => f.path)}
        onClose={() => setPicking(false)}
        onPick={(paths) => {
          setPicking(false);
          const fs = paths.map((p) => folders.find((f) => f.path === p) ?? { name: baseName(p), path: p });
          if (!enabled && fs.length && url) {
            patchDraft({ folders: fs });
            start(fs);
          } else setFolders(fs);
        }}
      />
    </motion.section>
  );
}

function Block({ label, action, children }: { label: string; action?: React.ReactNode; children: React.ReactNode }) {
  return (
    <div className="mt-4">
      <div className="mb-1.5 flex min-h-8 items-center justify-between">
        <span className="text-[13px] font-medium text-dim">{label}</span>
        {action}
      </div>
      {children}
    </div>
  );
}

function StatusLine({ st }: { st: LinkStatus }) {
  const t = useT();
  if (st.state === 'pending' && st.code) {
    return (
      <div className="rounded-2xl border border-[color-mix(in_oklab,var(--accent)_40%,transparent)] bg-[color-mix(in_oklab,var(--accent)_10%,transparent)] p-4 text-center" data-testid="link-pending">
        <div className="text-sm font-medium">{t('devices.share.approveOnPc')}</div>
        <div className="mt-2 flex justify-center gap-1.5">
          {st.code.split('').map((c, i) => (
            <span key={i} className="grid h-12 w-10 place-items-center rounded-xl bg-elev font-display text-2xl font-bold tabular-nums">
              {c}
            </span>
          ))}
        </div>
        <div className="mt-2 text-xs text-faint">{t('devices.share.approveHint')}</div>
      </div>
    );
  }
  const map: Record<string, { tone: string; text: string; spin?: boolean }> = {
    off: { tone: 'text-faint', text: t('devices.share.state.off') },
    connecting: { tone: 'text-warn', text: st.error ? t('devices.share.state.retrying', { error: st.error }) : t('devices.share.state.connecting'), spin: true },
    pending: { tone: 'text-warn', text: t('devices.share.state.connecting'), spin: true },
    online: { tone: 'text-ok', text: t('devices.share.state.online', { pc: st.serverName ?? st.server ?? '' }) },
    rejected: { tone: 'text-danger', text: t('devices.share.state.rejected') },
    error: { tone: 'text-danger', text: st.error === 'forgotten' ? t('devices.share.state.forgotten') : t('devices.share.state.error', { error: st.error ?? '' }) },
  };
  const m = map[st.state] ?? map.off;
  return (
    <div className={cn('flex items-center gap-2 rounded-2xl bg-surface px-3 py-2.5 text-sm', m.tone)} data-testid="link-state" data-state={st.state}>
      {m.spin ? <Loader2 className="size-4 shrink-0 animate-spin" /> : st.state === 'online' ? <ShieldCheck className="size-4 shrink-0" /> : st.state === 'off' ? <MonitorSmartphone className="size-4 shrink-0" /> : <CircleAlert className="size-4 shrink-0" />}
      <span className="min-w-0 flex-1">{m.text}</span>
      {st.state === 'online' && st.active > 0 && <span className="text-xs text-faint tabular-nums">{t('devices.share.active', { n: st.active })}</span>}
    </div>
  );
}

function Permissions({ perms, onAsk }: { perms: MediaPermissions; onAsk: () => void }) {
  const t = useT();
  const rows = [
    { key: 'media', icon: <Image className="size-4" />, label: t('devices.perm.media'), hint: t('devices.perm.mediaHint'), state: perms.media },
    { key: 'location', icon: <MapPin className="size-4" />, label: t('devices.perm.location'), hint: t('devices.perm.locationHint'), state: perms.location },
    { key: 'notifications', icon: <Bell className="size-4" />, label: t('devices.perm.notifications'), hint: t('devices.perm.notificationsHint'), state: perms.notifications },
  ];
  const missing = rows.some((r) => r.state !== 'granted');
  return (
    <Block label={t('devices.perm.title')}>
      <div className="flex flex-col gap-1.5">
        {rows.map((r) => (
          <div key={r.key} className="flex items-center gap-3 rounded-2xl bg-surface px-3 py-2.5">
            <span className={cn('shrink-0', r.state === 'granted' ? 'text-ok' : 'text-warn')}>{r.icon}</span>
            <div className="min-w-0 flex-1">
              <div className="text-sm">{r.label}</div>
              <div className="text-xs text-faint">{r.hint}</div>
            </div>
            <span className={cn('shrink-0 text-xs', r.state === 'granted' ? 'text-ok' : r.state === 'denied' ? 'text-danger' : 'text-warn')}>
              {r.state === 'granted' ? t('devices.perm.granted') : r.state === 'denied' ? t('devices.perm.denied') : t('devices.perm.prompt')}
            </span>
          </div>
        ))}
      </div>
      {missing && (
        <div className="mt-2 flex flex-wrap items-center justify-between gap-2">
          <span className="text-xs text-faint">{rows.some((r) => r.state === 'denied') ? t('devices.perm.deniedHint') : ''}</span>
          <Button size="sm" variant="secondary" icon={<ShieldCheck className="size-4" />} onClick={onAsk}>
            {t('devices.perm.ask')}
          </Button>
        </div>
      )}
    </Block>
  );
}
