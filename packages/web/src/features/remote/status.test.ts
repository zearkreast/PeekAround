import { describe, expect, it } from 'vitest';
import type { RemoteFolderInfo } from '@peekaround/shared';
import { translate } from '@/i18n';
import { remoteStatusText } from './status';

const base: RemoteFolderInfo = {
  provider: 'mega',
  label: 'mega.nz/folder/AbCd1234',
  linkKind: 'folder',
  state: 'ok',
  error: null,
  code: null,
  retryAt: null,
  listedAt: null,
  indexing: { done: 10, total: 10 },
};
const tr = (k: Parameters<typeof translate>[1], v?: Record<string, string | number>) => translate('tr', k, v);
const en = (k: Parameters<typeof translate>[1], v?: Record<string, string | number>) => translate('en', k, v);

describe('remote folder status', () => {
  it('shows indexing progress, quota wait, offline and link errors', () => {
    expect(remoteStatusText({ ...base, indexing: { done: 3, total: 10 } }, tr, 'tr')).toBe('Hazırlanıyor 3/10');
    expect(remoteStatusText({ ...base, state: 'listing' }, en, 'en')).toBe('Listing…');
    expect(remoteStatusText({ ...base, state: 'offline' }, tr, 'tr')).toMatch(/ulaşılamıyor/);
    const now = Date.now();
    expect(remoteStatusText({ ...base, state: 'quota', retryAt: now + 90 * 60_000 + 30_000 }, tr, 'tr', now)).toBe('MEGA aktarım kotası doldu — 1 sa 30 dk sonra devam edecek');
    expect(remoteStatusText({ ...base, state: 'quota', retryAt: now - 1 }, en, 'en', now)).toMatch(/shortly/);
    expect(remoteStatusText({ ...base, state: 'error', code: 'not-found', error: 'MEGA link not found' }, en, 'en')).toBe('Cannot open the link: Link not found — it may have been removed or is wrong.');
    expect(remoteStatusText({ ...base, state: 'error', code: 'key', error: 'Invalid MEGA link key' }, en, 'en')).toMatch(/key/);
    expect(remoteStatusText(base, en, 'en')).toBe('Connected');
  });
});
