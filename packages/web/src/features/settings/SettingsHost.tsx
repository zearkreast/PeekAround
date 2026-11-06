import { useEffect } from 'react';
import { desktop, type UpdateInfo } from '@/backend/desktop';
import { useT } from '@/i18n';
import { usePrefs } from '@/store/prefs';
import { useToasts } from '@/store/toasts';
import { ShortcutsOverlay } from './Shortcuts';
// "Ask before deleting" (replaces the built-in delete action)
import './deleteAction';

/** non-intrusive "new version" toast with a link to the release page (opened by the desktop shell) */
export function showUpdateToast(u: UpdateInfo, t: ReturnType<typeof useT>) {
  if (!u.newer || !u.url) return;
  const url = u.url;
  useToasts.getState().push({
    kind: 'info',
    title: t('pref.desk.updateAvailable', { v: u.latest ?? '' }),
    body: t('pref.desk.updateBody', { cur: u.current }),
    action: { label: t('pref.desk.download'), run: () => void desktop?.openExternal(url) },
    duration: 20_000,
  });
}

/** Global side of the settings: "?" shortcut overlay, desktop update notices, tray language. Mount once. */
export function SettingsHost() {
  const t = useT();
  const lang = usePrefs((s) => s.lang);
  useEffect(() => desktop?.setLang?.(lang), [lang]);
  useEffect(() => desktop?.onUpdate?.((u) => showUpdateToast(u, t)), [t]);
  return <ShortcutsOverlay />;
}
