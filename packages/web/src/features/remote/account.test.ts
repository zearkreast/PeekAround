import { describe, expect, it } from 'vitest';
import type { MegaAccountStatus } from '@peekaround/shared';
import { translate } from '@/i18n';
import { planLabel, transferLabel } from './account';

const tr = (k: Parameters<typeof translate>[1], v?: Record<string, string | number>) => translate('tr', k, v);
const en = (k: Parameters<typeof translate>[1], v?: Record<string, string | number>) => translate('en', k, v);
const base: MegaAccountStatus = { loggedIn: true, email: 'a@b.c', type: null, plan: null, transfer: null, expired: false, since: 1 };

describe('MEGA account labels', () => {
  it('names the plan', () => {
    expect(planLabel({ ...base, type: 'free' }, tr)).toBe('Ücretsiz');
    expect(planLabel({ ...base, type: 'pro', plan: 'Pro II' }, tr)).toBe('Pro II');
    expect(planLabel({ ...base, type: 'pro' }, en)).toBe('Pro');
    expect(planLabel(base, tr)).toBeNull();
  });

  it('shows the transfer quota when MEGA reports one', () => {
    const gib = 1024 ** 3;
    expect(transferLabel({ ...base, transfer: { used: 5 * gib, total: 2048 * gib } }, tr)).toBe('Aktarım kotası: 5.0 GB / 2.0 TB');
    expect(transferLabel({ ...base, transfer: { used: 3 * gib, total: null } }, en)).toBe('Transfer: 3.0 GB used');
    expect(transferLabel({ ...base, transfer: { used: 0, total: null } }, tr)).toBeNull();
    expect(transferLabel(base, tr)).toBeNull();
  });
});
