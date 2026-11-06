import { useEffect, useState } from 'react';
import { Link } from 'react-router';
import { MapPin } from 'lucide-react';
import { TILE_STYLES, tileOf } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useSettings } from '@/lib/queries';
import { cn } from '@/lib/cn';
import { tileConfig } from './mapUtil';

/** current theme of <html> (dark unless 'light') */
function isDark() {
  return typeof document === 'undefined' || document.documentElement.dataset.theme !== 'light';
}

/**
 * Static mini map: the 3×3 tiles around a point (through the server's tile cache when there is a
 * server), the point centred with a pin. No Leaflet — it is a handful of <img>s.
 */
export function MiniMap({ lat, lon, to, zoom = 14, className, label }: { lat: number; lon: number; to?: string; zoom?: number; className?: string; label?: string }) {
  const backend = useBackend();
  const settings = useSettings();
  const cfg = tileConfig(settings.data?.mapTiles ?? 'osm', isDark(), backend.tileUrl ? (s) => backend.tileUrl!(s) : undefined);
  const [failed, setFailed] = useState(0);
  useEffect(() => setFailed(0), [lat, lon, cfg?.url]);
  if (!cfg) return null;
  const z = Math.min(zoom, cfg.maxZoom);
  const { x, y, px, py } = tileOf(lat, lon, z);
  const n = 2 ** z;
  const subs = (cfg.subdomains ?? '').split('').filter(Boolean);
  const url = (tx: number, ty: number) => {
    const wx = ((tx % n) + n) % n;
    const s = subs.length ? subs[(wx + ty) % subs.length] : '';
    return cfg.url.replace('{s}', s).replace('{z}', String(z)).replace('{x}', String(wx)).replace('{y}', String(ty));
  };
  const tiles: { k: string; src: string; col: number; row: number }[] = [];
  for (let dy = -1; dy <= 1; dy++)
    for (let dx = -1; dx <= 1; dx++) {
      const ty = y + dy;
      if (ty < 0 || ty >= n) continue;
      tiles.push({ k: `${dx}:${dy}`, src: url(x + dx, ty), col: dx + 1, row: dy + 1 });
    }
  const body = (
    <>
      <div className="absolute" style={{ width: 768, height: 768, left: `calc(50% - ${256 + px}px)`, top: `calc(50% - ${256 + py}px)` }}>
        {tiles.map((t) => (
          <img
            key={t.k}
            src={t.src}
            alt=""
            draggable={false}
            loading="lazy"
            decoding="async"
            onError={() => setFailed((f) => f + 1)}
            className={cn('absolute size-[256px] max-w-none select-none', cfg.className && 'pm-tiles-dark')}
            style={{ left: t.col * 256, top: t.row * 256 }}
          />
        ))}
      </div>
      {failed >= 3 && <span className="absolute inset-0 grid place-items-center bg-[var(--bg-elev-2)] text-faint" aria-hidden />}
      <span className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-full text-accent drop-shadow-[0_2px_4px_rgb(0_0_0/0.45)]">
        <MapPin className="size-8 fill-[var(--accent)] stroke-white" strokeWidth={1.6} />
      </span>
      <span className="absolute right-1 bottom-0.5 max-w-[95%] truncate rounded bg-black/40 px-1 text-[9px] text-white/80">{TILE_STYLES[cfg.style].attributionText}</span>
    </>
  );
  const cls = cn('relative block h-36 w-full overflow-hidden rounded-xl border border-line bg-[var(--bg-elev-2)]', className);
  return to ? (
    <Link to={to} className={cn(cls, 'transition-opacity hover:opacity-90')} aria-label={label} data-minimap>
      {body}
    </Link>
  ) : (
    <div className={cls} data-minimap>
      {body}
    </div>
  );
}
