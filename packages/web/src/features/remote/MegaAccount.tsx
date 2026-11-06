/**
 * Settings → Ağ → "MEGA hesabı": the optional MEGA account login. With a session, MEGA transfers count
 * against the account's transfer quota instead of the one shared by the IP address. The server stores
 * only the session (never the password); LAN clients of the desktop app get 403 → the card hides.
 */
import { useState, type FormEvent } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { CircleUserRound, LogIn, LogOut, TriangleAlert } from 'lucide-react';
import type { MegaAccountStatus, MegaLoginRefusal } from '@peekaround/shared';
import { useBackend, useInfo } from '@/backend';
import { useT, type MessageKey } from '@/i18n';
import { toast } from '@/store/toasts';
import { Badge, Button, Input } from '@/components/ui';
import { Card, Row, Searchable } from '@/features/settings/parts';
import { planLabel, transferLabel } from './account';

/** deep link of the card (the quota banner's "Hesapla giriş yap") */
export const MEGA_ACCOUNT_HREF = '/settings#mega-account';

/** account status; undefined while loading, for backends without it and for LAN clients (403) */
export function useMegaAccount(enabled = true): MegaAccountStatus | undefined {
  const backend = useBackend();
  const info = useInfo();
  const q = useQuery({
    queryKey: ['mega-account', backend.id],
    queryFn: () => backend.megaAccount!(),
    enabled: enabled && !!backend.megaAccount && !!info && info.mode !== 'android-local',
    staleTime: 60_000,
    retry: false,
  });
  return q.data;
}

const REFUSAL: Record<MegaLoginRefusal, MessageKey | null> = {
  credentials: 'mega.account.err.credentials',
  'mfa-invalid': 'mega.account.err.mfaInvalid',
  blocked: 'mega.account.err.blocked',
  'mfa-required': null,
};

export function MegaAccountCard() {
  const t = useT();
  const backend = useBackend();
  const qc = useQueryClient();
  const account = useMegaAccount();
  // null: untouched (an expired session prefills its email)
  const [email, setEmail] = useState<string | null>(null);
  const [password, setPassword] = useState('');
  const [code, setCode] = useState('');
  const [needCode, setNeedCode] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  if (!account || !backend.megaLogin) return null;

  const emailValue = email ?? (account.expired ? (account.email ?? '') : '');
  const set = (a: MegaAccountStatus) => qc.setQueryData(['mega-account', backend.id], a);
  const refreshFolders = () => void qc.invalidateQueries({ queryKey: ['libraries'] });

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const r = await backend.megaLogin!({ email: emailValue, password, ...(needCode && code ? { code } : {}) });
      if (r.ok) {
        setPassword('');
        setCode('');
        setNeedCode(false);
        set(r.account);
        refreshFolders();
        toast.success(t('mega.account.loggedInToast'));
        return;
      }
      if (r.reason === 'mfa-required' || r.reason === 'mfa-invalid') setNeedCode(true);
      const key = REFUSAL[r.reason];
      if (key) setError(t(key));
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const logout = async () => {
    setBusy(true);
    try {
      set(await backend.megaLogout!());
      refreshFolders();
      toast.success(t('mega.account.loggedOutToast'));
    } catch (err) {
      toast.error(t('common.error'), (err as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const plan = planLabel(account, t);
  const transfer = transferLabel(account, t);

  return (
    <Card id="mega-account" title={t('mega.account.title')} titleKey="mega.account.title" icon={<CircleUserRound />} scope="server" testId="mega-account">
      {account.loggedIn ? (
        <Row
          label={
            <span className="flex flex-wrap items-center gap-2">
              <span className="break-all">{t('mega.account.loggedIn', { email: account.email ?? '' })}</span>
              {plan && <Badge>{plan}</Badge>}
            </span>
          }
          k="mega.account.title"
          hint={transfer ?? t('mega.account.hint')}
          kw="mega account hesap login giriş quota kota"
          setting="mega-account"
        >
          <Button variant="secondary" size="sm" icon={<LogOut className="size-4" />} loading={busy} onClick={() => void logout()} data-action="mega-logout">
            {t('mega.account.logout')}
          </Button>
        </Row>
      ) : (
        <Searchable keys={['mega.account.title', 'mega.account.login']} text="mega account hesap login giriş quota kota">
          <form className="flex flex-col gap-3 py-3.5 first:pt-0 last:pb-0" data-row data-setting="mega-account" onSubmit={(e) => void submit(e)}>
            {account.expired && (
              <div className="flex items-center gap-2 rounded-xl bg-[color-mix(in_oklab,var(--warn)_12%,transparent)] px-3 py-2 text-sm text-warn" role="status">
                <TriangleAlert className="size-4 shrink-0" />
                {t('mega.account.expired')}
              </div>
            )}
            <div className="text-xs text-faint">
              {t('mega.account.hint')} {t('mega.account.why')}
            </div>
            <div className="grid gap-2 sm:grid-cols-2">
              <label className="flex flex-col gap-1 text-xs text-dim">
                {t('mega.account.email')}
                <Input
                  type="email"
                  name="email"
                  autoComplete="username"
                  required
                  value={emailValue}
                  onChange={(e) => setEmail(e.target.value)}
                  disabled={busy}
                />
              </label>
              <label className="flex flex-col gap-1 text-xs text-dim">
                {t('mega.account.password')}
                <Input type="password" name="password" autoComplete="current-password" required value={password} onChange={(e) => setPassword(e.target.value)} disabled={busy} />
              </label>
            </div>
            {needCode && (
              <label className="flex flex-col gap-1 text-xs text-dim">
                {t('mega.account.code')}
                <Input
                  name="code"
                  inputMode="numeric"
                  autoComplete="one-time-code"
                  pattern="[0-9 ]{6,7}"
                  maxLength={7}
                  required
                  autoFocus
                  value={code}
                  onChange={(e) => setCode(e.target.value)}
                  disabled={busy}
                  className="w-40"
                />
                <span className="text-faint">{t('mega.account.codeHint')}</span>
              </label>
            )}
            {error && (
              <div className="text-sm text-danger" role="alert">
                {error}
              </div>
            )}
            <div className="flex flex-wrap items-center gap-3">
              <Button type="submit" variant="primary" size="sm" icon={<LogIn className="size-4" />} loading={busy} data-action="mega-login">
                {t('mega.account.login')}
              </Button>
              <span className="text-xs text-faint">{t('mega.account.privacy')}</span>
            </div>
          </form>
        </Searchable>
      )}
    </Card>
  );
}
