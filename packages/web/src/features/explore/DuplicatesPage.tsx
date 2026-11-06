import { useEffect, useMemo, useState } from 'react';
import { motion } from 'motion/react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Check, CheckCheck, Copy, Crown } from 'lucide-react';
import type { MediaItem } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useLang, useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { useChrome } from '@/store/chrome';
import { useLocation } from '@/store/location';
import { useSelection } from '@/store/selection';
import { Button, EmptyState, Spinner } from '@/components/ui';
import { SelectionBar } from '@/features/gallery/SelectionBar';
import { StaticSource } from '@/features/gallery/grid/source';
import { Thumb, useEnter } from './cards';
import { useOpenViewer } from './data';
import { bytesLabel } from './labels';
import { BackLink } from './ResultPage';

const PAGE = 120;

/**
 * Exact duplicates, one card per group (copies side by side with their folders). Selection works like
 * in the gallery (SelectionBar: delete / move / download …); "select extra copies" keeps the oldest
 * file of every group and selects the rest.
 */
export function DuplicatesPage() {
  const t = useT();
  const lang = useLang();
  const backend = useBackend();
  const qc = useQueryClient();
  const setChrome = useChrome((s) => s.set);
  const setView = useLocation((s) => s.setView);
  const open = useOpenViewer();
  const enter = useEnter();
  const [limit, setLimit] = useState(PAGE);
  const selected = useSelection((s) => s.ids);
  const q = useQuery({
    queryKey: ['explore', backend.id, 'duplicates', limit],
    queryFn: () => backend.duplicates!(0, limit),
    enabled: !!backend.duplicates,
    placeholderData: (prev) => prev,
    refetchInterval: (query) => (query.state.data?.pending ? 5000 : false),
  });
  const data = q.data;

  // deleted / moved files leave their group
  useEffect(
    () =>
      backend.subscribe((e) => {
        if (e.type === 'media' && (e.reason === 'remove' || e.reason === 'add')) void qc.invalidateQueries({ queryKey: ['explore', backend.id, 'duplicates'] });
      }),
    [backend, qc],
  );

  const byId = useMemo(() => new Map((data?.items ?? []).map((i) => [i.id, i])), [data]);
  const groups = useMemo(
    () =>
      (data?.groups ?? [])
        // oldest file (modification time) first: that is the one "select extra copies" keeps
        .map((g) => ({ ...g, items: g.ids.map((id) => byId.get(id)).filter((x): x is MediaItem => !!x).sort((a, b) => a.mtime - b.mtime || Number(a.id) - Number(b.id)) }))
        .filter((g) => g.items.length > 1),
    [data, byId],
  );
  const ordered = useMemo(() => groups.flatMap((g) => g.items), [groups]);
  const source = useMemo(() => new StaticSource(`dups:${ordered.length}:${ordered.slice(0, 40).map((i) => i.id).join(',')}`, ordered), [ordered]);
  const title = t('insights.dup.title');
  useEffect(() => setChrome({ crumbs: [], title, viewControls: false }), [setChrome, title]);
  useEffect(() => setView({ target: null, targetLabel: title, source }), [source, title, setView]);
  useEffect(() => () => setView({ target: null, targetLabel: '', source: null }), [setView]);

  const wasted = groups.reduce((a, g) => a + g.size * (g.items.length - 1), 0);
  const selectExtra = () => useSelection.getState().set(groups.flatMap((g) => g.items.slice(1)));

  return (
    <div className="mx-auto w-full max-w-[1800px] px-3 pt-3 pb-28 sm:px-7 sm:pt-5" data-page="explore-duplicates">
      <BackLink />
      <motion.div {...enter} className="mb-5 flex flex-col gap-3 sm:mb-7 sm:flex-row sm:items-end sm:gap-6">
        <div className="min-w-0 sm:flex-1">
          <h1 className="flex items-center gap-2.5 truncate font-display text-[22px] font-bold tracking-tight sm:text-[34px]">
            <Copy className="size-6 shrink-0 text-accent sm:size-7" />
            <span className="truncate">{title}</span>
          </h1>
          <p className="mt-1 text-[13px] text-dim tabular-nums sm:text-sm">
            {data
              ? [t('insights.dup.summary', { groups: data.total, files: ordered.length }), wasted ? t('insights.dup.wasted', { size: bytesLabel(wasted, lang) }) : '', data.pending ? t('insights.dup.pending', { n: data.pending }) : '']
                  .filter(Boolean)
                  .join(' · ')
              : t('common.loading')}
          </p>
        </div>
        {groups.length > 0 && (
          <Button variant="secondary" size="sm" icon={<CheckCheck className="size-4" />} onClick={selectExtra} title={t('insights.dup.selectExtraHint')} data-action="select-extra">
            {t('insights.dup.selectExtra')}
          </Button>
        )}
      </motion.div>

      {q.isLoading && (
        <div className="grid h-60 place-items-center">
          <Spinner className="size-7" />
        </div>
      )}
      {data && !groups.length && <EmptyState icon={<Copy />} title={data.pending ? t('insights.dup.pending', { n: data.pending }) : t('insights.dup.none')} />}

      <div className="flex flex-col gap-3" data-dup-groups={groups.length}>
        {groups.map((g) => (
          <section key={g.hash} className="rounded-2xl border border-line bg-surface p-3 sm:p-4" data-dup-group={g.hash}>
            <div className="mb-2.5 flex items-center gap-2 text-[13px]">
              <span className="font-semibold tabular-nums">{t('insights.dup.group', { n: g.items.length, size: bytesLabel(g.size, lang) })}</span>
              <span className="truncate text-faint">{g.items[0].name}</span>
            </div>
            <div className="no-scrollbar -mx-1 flex gap-2.5 overflow-x-auto px-1 pb-1">
              {g.items.map((it, i) => {
                const sel = selected.has(it.id);
                return (
                  <div key={it.id} className="w-[44vw] max-w-[200px] shrink-0 sm:w-[184px]">
                    <div className={cn('group relative aspect-[4/3] overflow-hidden rounded-xl bg-[var(--tile-bg)] ring-2 transition-shadow', sel ? 'ring-accent' : 'ring-transparent')}>
                      <button type="button" className="absolute inset-0" onClick={() => open(it)} aria-label={it.name}>
                        <Thumb id={it.id} className="size-full transition-transform duration-500 ease-[var(--ease-out)] group-hover:scale-105" />
                      </button>
                      <button
                        type="button"
                        onClick={() => useSelection.getState().toggle(it)}
                        aria-pressed={sel}
                        aria-label={t('insights.dup.select')}
                        className={cn(
                          'absolute top-1.5 left-1.5 grid size-7 place-items-center rounded-full border-2 transition-colors',
                          sel ? 'border-accent bg-accent text-white' : 'border-white/80 bg-black/30 text-transparent hover:text-white/80',
                        )}
                        data-dup-select={it.id}
                      >
                        <Check className="size-4" />
                      </button>
                      {i === 0 && (
                        <span className="absolute top-1.5 right-1.5 inline-flex items-center gap-1 rounded-full bg-black/55 px-2 py-0.5 text-[10.5px] font-medium text-white backdrop-blur-sm">
                          <Crown className="size-3" />
                          {t('insights.dup.original')}
                        </span>
                      )}
                    </div>
                    <div className="mt-1.5 truncate text-[12.5px] font-medium" title={it.name}>
                      {it.name}
                    </div>
                    <div className="truncate text-[11.5px] text-faint" title={data?.paths[it.id]}>
                      {data?.paths[it.id] || '—'}
                    </div>
                  </div>
                );
              })}
            </div>
          </section>
        ))}
      </div>

      {data && data.total > groups.length && groups.length >= limit && (
        <div className="mt-5 flex justify-center">
          <Button variant="secondary" loading={q.isFetching} onClick={() => setLimit((l) => l + PAGE)}>
            {t('insights.dup.more')}
          </Button>
        </div>
      )}

      <SelectionBar source={source} />
    </div>
  );
}
