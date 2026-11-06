import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Cloud } from 'lucide-react';
import { useBackend, useInfo } from '@/backend';
import { useT } from '@/i18n';
import { formatBytes } from '@/lib/format';
import { useSettings } from '@/lib/queries';

const SIZES_MB = [0, 512, 1024, 2048, 5120, 10240, 20480];

/** Settings → cache: size cap of the MEGA stream cache (Settings.remoteCacheMb) */
export function RemoteCacheRow() {
  const t = useT();
  const backend = useBackend();
  const info = useInfo();
  const qc = useQueryClient();
  const settings = useSettings();
  const used = useQuery({ queryKey: ['remote-cache', backend.id], queryFn: () => backend.remoteCacheSize!(), enabled: !!backend.remoteCacheSize });
  const mb = settings.data?.remoteCacheMb;
  if (!backend.remoteCacheSize || mb === undefined) return null;
  const sizes = SIZES_MB.includes(mb) ? SIZES_MB : [...SIZES_MB, mb].sort((a, b) => a - b);
  return (
    <div className="flex flex-wrap items-center justify-between gap-x-6 gap-y-2 py-3.5 last:pb-0" data-testid="remote-cache">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 text-sm font-medium">
          <Cloud className="size-4 text-[#e5484d]" />
          {t('mega.cache.title')}
          <span className="text-xs font-normal text-faint">· {t('mega.cache.used', { size: formatBytes(used.data ?? 0) })}</span>
        </div>
        <div className="mt-0.5 text-xs text-faint">{t('mega.cache.hint')}</div>
      </div>
      <select
        aria-label={t('mega.cache.limit')}
        value={mb}
        disabled={!!info?.caps.readonly}
        onChange={async (e) => {
          const next = await backend.updateSettings({ remoteCacheMb: Number(e.target.value) });
          qc.setQueryData(['settings', backend.id], next);
          void used.refetch();
        }}
        className="h-9 rounded-xl border border-line bg-surface px-3 text-sm text-fg"
      >
        {sizes.map((s) => (
          <option key={s} value={s}>
            {s === 0 ? '0' : formatBytes(s * 1024 * 1024)}
          </option>
        ))}
      </select>
    </div>
  );
}
