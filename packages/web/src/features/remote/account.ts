/** MEGA account labels (Settings → MEGA hesabı) — pure, unit tested. */
import type { MegaAccountStatus } from '@peekaround/shared';
import type { MessageKey } from '@/i18n';
import { formatBytes } from '@/lib/format';

type T = (key: MessageKey, vars?: Record<string, string | number>) => string;

export function planLabel(a: MegaAccountStatus, t: T): string | null {
  if (a.type === 'free') return t('mega.account.free');
  if (a.type === 'business') return a.plan ?? t('mega.account.business');
  if (a.type === 'pro') return a.plan ?? t('mega.account.pro');
  return null;
}

export function transferLabel(a: MegaAccountStatus, t: T): string | null {
  if (!a.transfer) return null;
  if (a.transfer.total) return t('mega.account.transfer', { used: formatBytes(a.transfer.used), total: formatBytes(a.transfer.total) });
  return a.transfer.used > 0 ? t('mega.account.transferUsed', { used: formatBytes(a.transfer.used) }) : null;
}
