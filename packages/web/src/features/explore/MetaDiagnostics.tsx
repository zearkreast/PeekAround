import { useState } from 'react';
import { Link } from 'react-router';
import { motion } from 'motion/react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { MapPinned, MapPinOff, MessageCircle, RefreshCw } from 'lucide-react';
import type { MetaLibraryStatus } from '@peekaround/shared';
import { useBackend, useInfo } from '@/backend';
import { useLang, useT } from '@/i18n';
import { toast } from '@/store/toasts';
import { Button, Spinner } from '@/components/ui';
import { NO_GPS_HREF, numLabel } from './labels';

const SEGMENTS = [
  { key: 'withLocation', cls: 'bg-accent', label: 'meta.withLocation' },
  { key: 'withoutLocation', cls: 'bg-[color-mix(in_oklab,var(--warn,#f5a524)_70%,transparent)]', label: 'meta.withoutLocation' },
  { key: 'pending', cls: 'shimmer bg-surface-3', label: 'meta.pending' },
  { key: 'unreadable', cls: 'bg-danger/70', label: 'meta.unreadable' },
] as const;

function LibraryRow({ l, onReread, busy, ro }: { l: MetaLibraryStatus; onReread: () => void; busy: boolean; ro: boolean }) {
  const t = useT();
  const lang = useLang();
  const parts = { withLocation: l.withLocation, withoutLocation: l.withoutLocation, pending: l.pending, unreadable: l.unreadable };
  const total = Math.max(1, l.total);
  return (
    <div className="py-3.5 first:pt-0 last:pb-0" data-meta-library={l.id}>
      <div className="mb-2 flex items-center gap-2">
        <span className="min-w-0 flex-1 truncate text-sm font-medium">{l.name}</span>
        <span className="text-xs text-faint tabular-nums">{numLabel(l.total, lang)}</span>
      </div>
      <div className="flex h-2.5 overflow-hidden rounded-full bg-surface-2" role="img" aria-label={`${t('meta.withLocation')}: ${l.withLocation} / ${l.total}`}>
        {SEGMENTS.map((s) =>
          parts[s.key] ? <span key={s.key} className={`${s.cls} h-full transition-[width] duration-500`} style={{ width: `${(parts[s.key] / total) * 100}%` }} /> : null,
        )}
      </div>
      <div className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-xs text-dim tabular-nums">
        {SEGMENTS.map((s) =>
          parts[s.key] || s.key === 'withLocation' ? (
            <span key={s.key} className="inline-flex items-center gap-1.5" data-meta-count={s.key}>
              <span className={`size-2 rounded-full ${s.cls}`} />
              {t(s.label)}: {numLabel(parts[s.key], lang)}
            </span>
          ) : null,
        )}
      </div>
      {l.messaging > 0 && (
        <p className="mt-2 flex items-start gap-1.5 text-xs text-faint">
          <MessageCircle className="mt-px size-3.5 shrink-0" />
          {t('meta.messaging', { n: numLabel(l.messaging, lang) })}
        </p>
      )}
      <div className="mt-2.5 flex flex-wrap gap-2">
        <Button variant="secondary" size="sm" icon={<RefreshCw className="size-4" />} loading={busy} disabled={ro || !l.total} onClick={onReread} data-action="meta-reread">
          {t('meta.reread')}
        </Button>
        {parts.withoutLocation > 0 && (
          <Link to={NO_GPS_HREF} className="inline-flex h-8 items-center gap-1.5 rounded-lg px-2.5 text-[13px] font-medium text-accent hover:bg-surface-2">
            <MapPinOff className="size-4" />
            {t('meta.showMissing')}
          </Link>
        )}
      </div>
    </div>
  );
}

/**
 * Settings card "Location & metadata": per library how many files carry a location, how many wait
 * to be read / could not be read, a hint for messaging-app files, and "re-read metadata".
 */
export function MetaDiagnostics({ delay = 0.15 }: { delay?: number }) {
  const t = useT();
  const backend = useBackend();
  const info = useInfo();
  const qc = useQueryClient();
  const ro = !!info?.caps.readonly;
  const [busy, setBusy] = useState<string | null>(null);
  const q = useQuery({
    queryKey: ['meta-status', backend.id],
    queryFn: () => backend.metaStatus!(),
    enabled: !!backend.metaStatus,
    refetchInterval: (query) => {
      const d = query.state.data;
      return d && (d.enriching || d.libraries.some((l) => l.pending > 0)) ? 2000 : 15_000;
    },
  });
  if (!backend.metaStatus) return null;
  const d = q.data;
  const reread = async (id?: string) => {
    setBusy(id ?? '*');
    try {
      await backend.rereadMeta?.(id);
      toast.success(t('meta.rereadStarted'));
      await q.refetch();
      void qc.invalidateQueries({ queryKey: ['explore', backend.id] });
    } catch (e) {
      toast.error(t('common.error'), (e as Error).message);
    } finally {
      setBusy(null);
    }
  };
  const pending = d?.libraries.reduce((a, l) => a + l.pending, 0) ?? 0;
  return (
    <motion.section
      initial={{ opacity: 0, y: 16 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ type: 'spring', stiffness: 260, damping: 28, delay }}
      className="rounded-3xl border border-line bg-elev p-5 sm:p-6"
      data-settings="meta"
    >
      <h2 className="mb-1.5 flex items-center gap-2.5 font-display text-lg font-semibold">
        <span className="grid size-8 place-items-center rounded-xl bg-surface-2 text-accent [&>svg]:size-4">
          <MapPinned />
        </span>
        <span className="flex-1">{t('meta.title')}</span>
        {d && (
          <span className="text-xs font-normal text-faint tabular-nums" data-meta-progress>
            {pending > 0 || d.enriching ? t('meta.progress', { done: Math.max(0, d.indexing.done), total: d.indexing.total }) : t('meta.idle')}
          </span>
        )}
      </h2>
      <p className="mb-4 text-xs text-faint">
        {t('meta.intro')} {t('meta.takeoutHint')}
      </p>
      {!d && (
        <div className="grid h-16 place-items-center">
          <Spinner />
        </div>
      )}
      {d && !d.libraries.length && <p className="text-sm text-dim">{t('meta.empty')}</p>}
      {d && d.libraries.length > 0 && (
        <div className="flex flex-col divide-y divide-line">
          {d.libraries.map((l) => (
            <LibraryRow key={l.id} l={l} ro={ro} busy={busy === l.id} onReread={() => void reread(l.id)} />
          ))}
        </div>
      )}
      {d && d.libraries.length > 1 && (
        <div className="mt-4 border-t border-line pt-4">
          <Button variant="ghost" size="sm" icon={<RefreshCw className="size-4" />} loading={busy === '*'} disabled={ro} onClick={() => void reread()}>
            {t('meta.rereadAll')}
          </Button>
        </div>
      )}
    </motion.section>
  );
}

export default MetaDiagnostics;
