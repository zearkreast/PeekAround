import { useQuery } from '@tanstack/react-query';
import { motion } from 'motion/react';
import { Aperture, Calendar, Camera, Clock, FileImage, Film, FolderOpen, HardDrive, MapPin, Maximize, X } from 'lucide-react';
import type { MediaItem } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useLang, useT } from '@/i18n';
import { formatBytes, formatDate, formatDuration } from '@/lib/format';
import { IconButton, Spinner } from '@/components/ui';

function Row({ icon, label, value, sub }: { icon: React.ReactNode; label: string; value: React.ReactNode; sub?: React.ReactNode }) {
  return (
    <div className="flex gap-3 py-2.5">
      <span className="mt-0.5 text-faint [&>svg]:size-[18px]">{icon}</span>
      <div className="min-w-0 flex-1">
        <div className="text-[11px] font-medium tracking-wide text-faint uppercase">{label}</div>
        <div className="mt-0.5 text-sm break-words text-fg">{value}</div>
        {sub && <div className="text-xs text-dim">{sub}</div>}
      </div>
    </div>
  );
}

export function InfoPanel({ item, onClose }: { item: MediaItem; onClose: () => void }) {
  const t = useT();
  const lang = useLang();
  const backend = useBackend();
  const q = useQuery({ queryKey: ['media', backend.id, item.id], queryFn: () => backend.media(item.id) });
  const d = q.data;
  const ex = d?.exif ?? {};
  const exposure = [
    ex.FNumber ? `ƒ/${ex.FNumber}` : null,
    ex.ExposureTime ? (Number(ex.ExposureTime) < 1 ? `1/${Math.round(1 / Number(ex.ExposureTime))}s` : `${ex.ExposureTime}s`) : null,
    ex.ISO ? `ISO ${ex.ISO}` : null,
    ex.FocalLength ? `${ex.FocalLength}mm` : null,
  ]
    .filter(Boolean)
    .join(' · ');
  const lat = Number(ex.latitude);
  const lon = Number(ex.longitude);
  return (
    <motion.aside
      initial={{ x: 380, opacity: 0 }}
      animate={{ x: 0, opacity: 1 }}
      exit={{ x: 380, opacity: 0 }}
      transition={{ type: 'spring', stiffness: 380, damping: 38 }}
      className="absolute top-0 right-0 bottom-0 z-20 flex w-[340px] max-w-[90vw] flex-col border-l border-white/10 bg-[var(--bg-elev)]/95 backdrop-blur-2xl"
      onClick={(e) => e.stopPropagation()}
    >
      <div className="flex h-16 items-center justify-between px-5">
        <h3 className="font-display text-lg font-semibold">{t('viewer.details')}</h3>
        <IconButton label={t('common.close')} size="sm" onClick={onClose}>
          <X />
        </IconButton>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-5 pb-6">
        <div className="mb-2 text-[15px] font-medium break-all">{item.name}</div>
        <div className="divide-y divide-line">
          <Row icon={<Calendar />} label={t('viewer.taken')} value={formatDate(item.takenAt, lang, { dateStyle: 'full' })} sub={formatDate(item.takenAt, lang, { timeStyle: 'short' })} />
          {item.width && item.height && (
            <Row icon={<Maximize />} label={t('viewer.dimensions')} value={`${item.width} × ${item.height}`} sub={`${((item.width * item.height) / 1e6).toFixed(1)} MP`} />
          )}
          <Row icon={<HardDrive />} label={t('viewer.size')} value={formatBytes(item.size)} sub={item.ext.toUpperCase()} />
          {item.duration != null && <Row icon={<Clock />} label={t('viewer.duration')} value={formatDuration(item.duration)} />}
          {d?.video && (
            <Row
              icon={<Film />}
              label={t('viewer.codec')}
              value={[d.video.codec, d.video.audioCodec].filter(Boolean).join(' / ').toUpperCase()}
              sub={[d.video.fps ? `${d.video.fps} fps` : null, d.video.bitrate ? `${(d.video.bitrate / 1e6).toFixed(1)} Mbps` : null].filter(Boolean).join(' · ')}
            />
          )}
          {(ex.Make || ex.Model) && <Row icon={<Camera />} label={t('viewer.camera')} value={[ex.Make, ex.Model].filter(Boolean).join(' ')} sub={ex.LensModel ? String(ex.LensModel) : undefined} />}
          {exposure && <Row icon={<Aperture />} label={t('viewer.exposure')} value={exposure} />}
          {Number.isFinite(lat) && Number.isFinite(lon) && (
            <Row
              icon={<MapPin />}
              label={t('viewer.location')}
              value={`${lat.toFixed(5)}, ${lon.toFixed(5)}`}
              sub={
                <a className="text-accent hover:underline" href={`https://www.openstreetmap.org/?mlat=${lat}&mlon=${lon}#map=15/${lat}/${lon}`} target="_blank" rel="noreferrer">
                  {t('viewer.openMap')}
                </a>
              }
            />
          )}
          <Row icon={<FolderOpen />} label={t('viewer.path')} value={<span className="font-mono text-xs">{d?.path ?? '…'}</span>} />
          <Row icon={<FileImage />} label={t('viewer.modified')} value={formatDate(item.mtime, lang, { dateStyle: 'medium', timeStyle: 'short' })} />
        </div>
        {q.isLoading && (
          <div className="grid h-12 place-items-center">
            <Spinner />
          </div>
        )}
      </div>
    </motion.aside>
  );
}
