import { useMemo, useState, type ReactNode } from 'react';
import { motion } from 'motion/react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Check, Clock, Compass, Search, Trash2 } from 'lucide-react';
import type { Settings } from '@peekaround/shared';
import { foldText, systemTimeZone, tileStyleOf, tzOffsetAt } from '@peekaround/shared';
import { useLang } from '@/i18n';
import { bytesLabel } from './labels';
import { useBackend, useInfo } from '@/backend';
import { useT } from '@/i18n';
import { useSettings } from '@/lib/queries';
import { toast } from '@/store/toasts';
import { Button, Input, Modal, Segmented } from '@/components/ui';

const offsets = new Map<string, number>();
/** "UTC+03:00" for a zone at the current instant */
export function offsetLabel(tz: string, now?: number): string {
  let m = now === undefined ? offsets.get(tz) : undefined;
  if (m === undefined) {
    m = tzOffsetAt(now ?? Date.now(), tz);
    if (now === undefined) offsets.set(tz, m);
  }
  const sign = m < 0 ? '-' : '+';
  const a = Math.abs(m);
  return `UTC${sign}${String(Math.floor(a / 60)).padStart(2, '0')}:${String(a % 60).padStart(2, '0')}`;
}

export function allTimeZones(): string[] {
  try {
    const list = (Intl as unknown as { supportedValuesOf?: (k: string) => string[] }).supportedValuesOf?.('timeZone');
    if (list?.length) return list.includes('UTC') ? list : ['UTC', ...list];
  } catch {
    /* old runtime */
  }
  return ['UTC', 'Europe/Istanbul', 'Europe/London', 'Europe/Berlin', 'America/New_York', 'America/Los_Angeles', 'Asia/Tokyo'];
}

/** search zones by folded name parts ("istanbul", "new york", "+03") */
export function filterZones(zones: string[], q: string): string[] {
  const f = foldText(q).replace(/[\s_]+/g, ' ').trim();
  if (!f) return zones;
  return zones.filter((z) => foldText(z).replace(/[/_]+/g, ' ').includes(f) || offsetLabel(z).includes(q.trim()));
}

function Row({ label, hint, children }: { label: ReactNode; hint?: ReactNode; children: ReactNode }) {
  return (
    <div className="flex flex-wrap items-center justify-between gap-x-6 gap-y-2 py-3.5 first:pt-0 last:pb-0">
      <div className="min-w-0 flex-1 basis-56">
        <div className="text-sm font-medium">{label}</div>
        {hint && <div className="mt-0.5 text-xs text-faint">{hint}</div>}
      </div>
      <div className="flex shrink-0 items-center gap-2">{children}</div>
    </div>
  );
}

/**
 * Settings card: map tiles + home time zone. Self-contained (own card chrome) so the settings page
 * only has to mount <ExploreSettings /> in its card column.
 */
export function ExploreSettings({ delay = 0.1 }: { delay?: number }) {
  const t = useT();
  const backend = useBackend();
  const info = useInfo();
  const qc = useQueryClient();
  const settings = useSettings();
  const s = settings.data;
  const ro = !!info?.caps.readonly;
  const [pick, setPick] = useState(false);
  const [q, setQ] = useState('');
  const zones = useMemo(() => (pick ? allTimeZones() : []), [pick]);
  const shown = useMemo(() => filterZones(zones, q).slice(0, 400), [zones, q]);
  const device = systemTimeZone();
  const lang = useLang();
  const tileCache = useQuery({ queryKey: ['tile-cache', backend.id], queryFn: () => backend.tileCache!(), enabled: !!backend.tileCache, staleTime: 10_000 });
  const [clearing, setClearing] = useState(false);
  const clearTiles = async () => {
    setClearing(true);
    try {
      await backend.clearTileCache?.();
      await tileCache.refetch();
      toast.success(t('map.settings.cleared'));
    } catch (e) {
      toast.error(t('common.error'), (e as Error).message);
    } finally {
      setClearing(false);
    }
  };

  const update = async (patch: Partial<Settings>) => {
    try {
      const next = await backend.updateSettings(patch);
      qc.setQueryData(['settings', backend.id], next);
      void qc.invalidateQueries({ queryKey: ['explore', backend.id] });
      toast.success(t('explore.settings.saved'));
    } catch (e) {
      toast.error(t('common.error'), (e as Error).message);
    }
  };

  if (!s) return null;
  const tz = s.homeTimeZone || 'UTC';
  return (
    <motion.section
      initial={{ opacity: 0, y: 16 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ type: 'spring', stiffness: 260, damping: 28, delay }}
      className="rounded-3xl border border-line bg-elev p-5 sm:p-6"
      data-settings="explore"
    >
      <h2 className="mb-4 flex items-center gap-2.5 font-display text-lg font-semibold">
        <span className="grid size-8 place-items-center rounded-xl bg-surface-2 text-accent [&>svg]:size-4">
          <Compass />
        </span>
        {t('explore.settings.title')}
      </h2>
      <div className="flex flex-col divide-y divide-line">
        <Row label={t('map.settings.style')} hint={backend.tileUrl ? t('map.settings.styleHint') : t('map.settings.styleHintDirect')}>
          <Segmented
            size="sm"
            value={tileStyleOf(s.mapTiles) ?? 'off'}
            onChange={(mapTiles) => !ro && void update({ mapTiles })}
            options={[
              { value: 'osm', label: t('map.style.osm') },
              { value: 'topo', label: t('map.style.topo') },
              { value: 'satellite', label: t('map.style.satellite') },
              { value: 'off', label: t('map.style.off') },
            ]}
          />
        </Row>
        {backend.tileCache && (
          <Row
            label={t('map.settings.cache')}
            hint={
              tileCache.data
                ? t('map.settings.cacheUsage', { size: bytesLabel(tileCache.data.bytes, lang), limit: bytesLabel((s.tileCacheMb ?? tileCache.data.limitMb) * 1024 * 1024, lang), n: tileCache.data.files })
                : undefined
            }
          >
            <select
              aria-label={t('map.settings.cacheLimit')}
              title={t('map.settings.cacheLimit')}
              value={String(s.tileCacheMb ?? 512)}
              disabled={ro}
              onChange={(e) => void update({ tileCacheMb: Number(e.target.value) }).then(() => tileCache.refetch())}
              className="h-9 rounded-lg border border-line bg-surface-2 px-2 text-sm"
              data-setting="tileCacheMb"
            >
              {[128, 256, 512, 1024, 2048, 4096].map((mb) => (
                <option key={mb} value={mb}>
                  {mb >= 1024 ? `${mb / 1024} GB` : `${mb} MB`}
                </option>
              ))}
            </select>
            <Button variant="secondary" size="sm" icon={<Trash2 className="size-4" />} loading={clearing} onClick={() => void clearTiles()} data-action="clear-tiles">
              {t('map.settings.clear')}
            </Button>
          </Row>
        )}
        <Row label={t('explore.settings.homeTz')} hint={t('explore.settings.homeTzHint')}>
          <Button variant="secondary" size="sm" icon={<Clock className="size-4" />} disabled={ro} onClick={() => setPick(true)} data-action="pick-tz">
            <span className="max-w-[180px] truncate">{tz.replace(/_/g, ' ')}</span>
            <span className="text-xs text-faint tabular-nums">{offsetLabel(tz)}</span>
          </Button>
        </Row>
      </div>

      <Modal open={pick} onClose={() => setPick(false)} title={t('explore.settings.homeTz')} width={460}>
        <Input autoFocus value={q} onChange={(e) => setQ(e.target.value)} placeholder={t('explore.settings.tzSearch')} icon={<Search />} />
        {device !== tz && (
          <button
            type="button"
            onClick={() => {
              setPick(false);
              void update({ homeTimeZone: device });
            }}
            className="mt-3 flex w-full items-center gap-2 rounded-xl border border-dashed border-line px-3 py-2.5 text-left text-sm hover:bg-surface-2"
          >
            <Clock className="size-4 text-accent" />
            {t('explore.settings.tzDevice', { tz: device.replace(/_/g, ' ') })}
          </button>
        )}
        <div className="mt-3 max-h-[50vh] overflow-y-auto rounded-xl border border-line">
          {shown.length === 0 && <div className="px-3 py-6 text-center text-sm text-dim">{t('explore.settings.tzNone')}</div>}
          {shown.map((z) => (
            <button
              key={z}
              type="button"
              onClick={() => {
                setPick(false);
                if (z !== tz) void update({ homeTimeZone: z });
              }}
              className="flex min-h-11 w-full items-center gap-2 border-b border-line px-3 py-2 text-left text-sm last:border-0 hover:bg-surface-2"
            >
              <span className="min-w-0 flex-1 truncate">{z.replace(/_/g, ' ')}</span>
              <span className="text-xs text-faint tabular-nums">{offsetLabel(z)}</span>
              {z === tz && <Check className="size-4 text-accent" />}
            </button>
          ))}
        </div>
      </Modal>
    </motion.section>
  );
}

export default ExploreSettings;
