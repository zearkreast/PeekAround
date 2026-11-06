import { useEffect, useState } from 'react';
import { FolderOpen, Laptop, Power, RefreshCw, RotateCw, ScrollText, Sparkles } from 'lucide-react';
import { desktop, type DesktopShellPrefs, type DesktopShellState, type UpdateInfo } from '@/backend/desktop';
import { useLang, useT } from '@/i18n';
import { relativeTime } from '@/lib/format';
import { toast } from '@/store/toasts';
import { Badge, Button, Segmented, Switch } from '@/components/ui';
import { Card, Row, Section } from './parts';
import { showUpdateToast } from './SettingsHost';

/** Settings → Desktop (Electron shell only; values live in the app's desktop.json, validated in main). */
export function DesktopSection() {
  const t = useT();
  const lang = useLang();
  const [st, setSt] = useState<DesktopShellState | null>(null);
  const [checking, setChecking] = useState(false);
  const [startGpu, setStartGpu] = useState<boolean | null>(null);
  useEffect(() => {
    void desktop?.getShell?.().then((s) => {
      setSt(s);
      setStartGpu(s.gpuActive);
    });
  }, []);
  if (!desktop?.getShell || !st) return null;

  const set = async (patch: Partial<DesktopShellPrefs>) => {
    try {
      setSt(await desktop!.setShell!(patch));
    } catch (e) {
      toast.error(t('common.error'), (e as Error).message);
    }
  };
  const check = async () => {
    setChecking(true);
    try {
      const u: UpdateInfo = await desktop!.checkUpdate!();
      setSt((s) => (s ? { ...s, lastUpdate: u } : s));
      if (u.error) toast.error(t('pref.desk.checkFailed'), u.error);
      else if (u.newer) showUpdateToast(u, t);
      else toast.success(t('pref.desk.upToDate', { v: u.current }));
    } finally {
      setChecking(false);
    }
  };
  const open = async (which: 'data' | 'logs') => {
    const err = await desktop!.openPath!(which);
    if (err) toast.error(t('common.error'), err);
  };
  const restartNeeded = startGpu !== null && startGpu !== st.gpu;
  const last = st.lastUpdate;

  return (
    <Section id="desktop" title="pref.sec.desktop" icon={<Laptop />}>
      <Card title={t('pref.desk.startup')} titleKey="pref.desk.startup" icon={<Power />} scope="device">
        {st.loginSupported && (
          <Row label={t('pref.desk.loginStart')} k="pref.desk.loginStart" kw="autostart login windows startup" setting="startAtLogin">
            <Switch checked={st.startAtLogin} label={t('pref.desk.loginStart')} onChange={(startAtLogin) => void set({ startAtLogin })} />
          </Row>
        )}
        {st.loginSupported && (
          <Row label={t('pref.desk.startHidden')} k="pref.desk.startHidden" hint={t('pref.desk.startHiddenHint')} kw="tray tepsi minimized" setting="startHidden">
            <Switch checked={st.startHidden} disabled={!st.startAtLogin} label={t('pref.desk.startHidden')} onChange={(startHidden) => void set({ startHidden })} />
          </Row>
        )}
        <Row label={t('pref.desk.close')} k="pref.desk.close" hint={t('pref.desk.closeHint')} kw="tray tepsi quit çık close kapat" setting="closeAction" stack>
          <Segmented
            size="sm"
            value={st.closeAction}
            onChange={(closeAction) => void set({ closeAction })}
            options={[
              { value: 'auto', label: t('pref.desk.close.auto') },
              { value: 'tray', label: t('pref.desk.close.tray') },
              { value: 'quit', label: t('pref.desk.close.quit') },
            ]}
          />
        </Row>
        <Row
          label={t('pref.desk.gpu')}
          k="pref.desk.gpu"
          hint={
            <>
              {t('pref.desk.gpuHint')}
              {restartNeeded && <Badge className="ml-2 text-warn">{t('pref.desk.restartNeeded')}</Badge>}
            </>
          }
          kw="gpu hardware acceleration donanım ekran kartı"
          setting="gpu"
        >
          {restartNeeded && (
            <Button variant="secondary" size="sm" icon={<RotateCw className="size-4" />} onClick={() => desktop!.restart?.()} data-action="restart">
              {t('pref.desk.restart')}
            </Button>
          )}
          <Switch checked={st.gpu} label={t('pref.desk.gpu')} onChange={(gpu) => void set({ gpu })} />
        </Row>
        <Row label={t('pref.desk.notify')} k="pref.desk.notify" hint={t('pref.desk.notifyHint')} kw="notification bildirim" setting="notify">
          <Switch checked={st.notify} label={t('pref.desk.notify')} onChange={(notify) => void set({ notify })} />
        </Row>
      </Card>

      <Card title={t('pref.desk.updates')} titleKey="pref.desk.updates" icon={<Sparkles />} scope="device">
        <Row label={t('pref.desk.autoUpdate')} k="pref.desk.autoUpdate" hint={t('pref.desk.autoUpdateHint')} kw="update güncelleme github release" setting="autoUpdate">
          <Switch checked={st.autoUpdate} label={t('pref.desk.autoUpdate')} onChange={(autoUpdate) => void set({ autoUpdate })} />
        </Row>
        <Row
          label={t('settings.version', { v: st.version })}
          k="pref.desk.checkNow"
          hint={last ? (last.error ? t('pref.desk.checkFailed') : t('pref.desk.lastCheck', { time: relativeTime(last.checkedAt, lang) })) : undefined}
          kw="update güncelleme version sürüm"
          setting="checkUpdate"
        >
          {last?.newer && last.url && (
            <Button variant="primary" size="sm" onClick={() => void desktop!.openExternal(last.url!)}>
              {t('pref.desk.updateAvailable', { v: last.latest ?? '' })}
            </Button>
          )}
          <Button variant="secondary" size="sm" icon={<RefreshCw className="size-4" />} loading={checking} onClick={() => void check()} data-action="check-update">
            {t('pref.desk.checkNow')}
          </Button>
        </Row>
      </Card>

      <Card title={t('pref.desk.folders')} titleKey="pref.desk.folders" icon={<FolderOpen />}>
        <Row label={t('pref.desk.openData')} k="pref.desk.openData" hint={t('pref.desk.openDataHint')} kw="data folder veri klasör database" setting="openData">
          <Button variant="secondary" size="sm" icon={<FolderOpen className="size-4" />} onClick={() => void open('data')}>
            {t('pref.desk.open')}
          </Button>
        </Row>
        <Row label={t('pref.desk.openLogs')} k="pref.desk.openLogs" kw="log günlük debug" setting="openLogs">
          <Button variant="secondary" size="sm" icon={<ScrollText className="size-4" />} onClick={() => void open('logs')}>
            {t('pref.desk.open')}
          </Button>
        </Row>
      </Card>
    </Section>
  );
}
