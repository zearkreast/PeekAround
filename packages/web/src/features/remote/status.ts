/** Status line of a remote (MEGA streaming) folder — pure, unit tested. */
import type { RemoteFolderInfo } from '@peekaround/shared';
import type { MessageKey } from '@/i18n';
import { durationUntil, relativeTime } from '@/lib/format';
import { megaErrorKey } from './mega';

type T = (key: MessageKey, vars?: Record<string, string | number>) => string;

export function remoteStatusText(r: RemoteFolderInfo, t: T, lang: 'tr' | 'en', now = Date.now()): string {
  switch (r.state) {
    case 'listing':
      return t('mega.state.listing');
    case 'offline':
      return t('mega.state.offline');
    case 'quota':
      return r.retryAt && r.retryAt > now ? t('mega.state.quota', { time: durationUntil(r.retryAt, lang) }) : t('mega.state.quotaSoon');
    case 'error': {
      const status = r.code === 'not-found' || r.code === 'blocked' ? 404 : r.code === 'key' ? 400 : r.code === 'unavailable' ? 503 : r.code === 'network' ? 0 : 502;
      const m = megaErrorKey(status, r.error ?? '');
      return t('mega.state.error', { error: t(m.key, m.vars) });
    }
    default:
      if (r.indexing.total && r.indexing.done < r.indexing.total) return t('mega.state.indexing', { done: r.indexing.done, total: r.indexing.total });
      return r.listedAt ? t('mega.state.lastListed', { time: relativeTime(r.listedAt, lang) }) : t('mega.state.ok');
  }
}
