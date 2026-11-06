import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { motion } from 'motion/react';
import { Aperture, Calendar, Camera, Clock, FileImage, Film, FolderOpen, HardDrive, Map as MapIcon, MapPin, MapPinOff, Maximize, X } from 'lucide-react';
import { Link } from 'react-router';
import type { MediaItem } from '@peekaround/shared';
import { cameraKey, isMessagingName } from '@peekaround/shared';
import { cameraHref, cameraLabel, countryName, flagEmoji, mapFocusHref, placeHref } from '@/features/explore/labels';
import { MiniMap } from '@/features/explore/MiniMap';
import { useBackend } from '@/backend';
import { useLang, useT } from '@/i18n';
import { formatBytes, formatDate, formatDuration, formatLocal } from '@/lib/format';
import { IconButton, Spinner } from '@/components/ui';
import { usePrefs } from '@/store/prefs';

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

/** Details rows (shared by the desktop side panel and the mobile bottom sheet). */
export function InfoContent({ item }: { item: MediaItem }) {
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
  const lat = d?.gps?.lat ?? Number(ex.latitude);
  const lon = d?.gps?.lon ?? Number(ex.longitude);
  const local = typeof item.localAt === 'number' ? item.localAt : null;
  const make = d?.camera?.make ?? ex.Make;
  const model = d?.camera?.model ?? ex.Model;
  const lens = d?.camera?.lens ?? ex.LensModel;
  const place = d?.place ? [d.place.city, d.place.admin1 !== d.place.city ? d.place.admin1 : null].filter(Boolean).join(', ') : null;
  const country = d?.place ? countryName(d.place.cc, lang) : '';
  const camKey = d?.camera?.model ? cameraKey(d.camera.make, d.camera.model) : null;
  // Settings → privacy: location stays hidden until revealed (per item)
  const hideLocation = usePrefs((s) => s.hideLocation);
  const [revealed, setRevealed] = useState<string | null>(null);
  const locHidden = hideLocation && revealed !== item.id;
  return (
    <>
      <div className="mb-2 text-[15px] font-medium break-all">{item.name}</div>
      <div className="divide-y divide-line">
        <Row
          icon={<Calendar />}
          label={t('viewer.taken')}
          value={local != null ? formatLocal(local, lang, { dateStyle: 'full' }) : formatDate(item.takenAt, lang, { dateStyle: 'full' })}
          sub={local != null ? formatLocal(local, lang, { timeStyle: 'short' }) : formatDate(item.takenAt, lang, { timeStyle: 'short' })}
        />
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
        {(make || model) && (
          <Row
            icon={<Camera />}
            label={t('viewer.camera')}
            value={
              camKey ? (
                <Link to={cameraHref(camKey)} className="hover:text-accent hover:underline" title={t('explore.info.cameraLink')} data-info="camera">
                  {cameraLabel(make ? String(make) : null, model ? String(model) : null)}
                </Link>
              ) : (
                [make, model].filter(Boolean).join(' ')
              )
            }
            sub={lens ? String(lens) : undefined}
          />
        )}
        {exposure && <Row icon={<Aperture />} label={t('viewer.exposure')} value={exposure} />}
        {Number.isFinite(lat) && Number.isFinite(lon) && locHidden ? (
          <Row
            icon={<MapPinOff />}
            label={t('viewer.location')}
            value={
              <span className="flex items-center gap-2">
                <span className="text-dim">{t('pref.locationHidden')}</span>
                <button type="button" className="text-[13px] font-medium text-accent hover:underline" onClick={() => setRevealed(item.id)} data-info="reveal-location">
                  {t('pref.locationReveal')}
                </button>
              </span>
            }
          />
        ) : Number.isFinite(lat) && Number.isFinite(lon) ? (
          <Row
            icon={<MapPin />}
            label={t('viewer.location')}
            value={
              place && d?.place ? (
                <Link to={placeHref(d.place.key)} className="hover:text-accent hover:underline" title={t('explore.info.placeLink')} data-info="place">
                  {place}
                  {country && (
                    <span className="text-dim">
                      {' '}
                      · {flagEmoji(d.place.cc)} {country}
                    </span>
                  )}
                </Link>
              ) : (
                `${lat.toFixed(5)}, ${lon.toFixed(5)}`
              )
            }
            sub={
              <span className="flex flex-col gap-2">
                <span className="flex flex-wrap gap-x-3">
                  {place && (
                    <span className="tabular-nums" title={t('map.info.coords')} data-info="coords">
                      {`${lat.toFixed(5)}, ${lon.toFixed(5)}`}
                    </span>
                  )}
                  <a className="text-accent hover:underline" href={`https://www.openstreetmap.org/?mlat=${lat}&mlon=${lon}#map=15/${lat}/${lon}`} target="_blank" rel="noreferrer">
                    {t('viewer.openMap')}
                  </a>
                </span>
                <MiniMap lat={lat} lon={lon} to={mapFocusHref(item.id, lat, lon)} label={t('map.info.showOnMap')} className="mt-1" />
                <Link to={mapFocusHref(item.id, lat, lon)} className="inline-flex items-center gap-1.5 self-start text-[13px] font-medium text-accent hover:underline" data-info="show-on-map">
                  <MapIcon className="size-4" />
                  {t('map.info.showOnMap')}
                </Link>
              </span>
            }
          />
        ) : (
          d &&
          (item.kind === 'image' || item.kind === 'video') && (
            <Row
              icon={<MapPinOff />}
              label={t('viewer.location')}
              value={<span className="text-dim">{t('map.info.noLocation')}</span>}
              sub={isMessagingName(item.name, item.dir) ? t('map.info.noLocationMessaging') : undefined}
            />
          )
        )}
        <Row icon={<FolderOpen />} label={t('viewer.path')} value={<span className="font-mono text-xs break-all">{d?.path ?? '…'}</span>} />
        <Row icon={<FileImage />} label={t('viewer.modified')} value={formatDate(item.mtime, lang, { dateStyle: 'medium', timeStyle: 'short' })} />
      </div>
      {q.isLoading && (
        <div className="grid h-12 place-items-center">
          <Spinner />
        </div>
      )}
    </>
  );
}

/** Desktop side panel. */
export function InfoPanel({ item, onClose }: { item: MediaItem; onClose: () => void }) {
  const t = useT();
  return (
    <motion.aside
      initial={{ x: 380, opacity: 0 }}
      animate={{ x: 0, opacity: 1 }}
      exit={{ x: 380, opacity: 0 }}
      transition={{ type: 'spring', stiffness: 380, damping: 38 }}
      className="absolute top-0 right-0 bottom-0 z-20 flex w-[340px] max-w-[90vw] flex-col border-l border-white/10 bg-[var(--bg-elev)]/95 backdrop-blur-2xl"
      onClick={(e) => e.stopPropagation()}
      data-no-gesture
    >
      <div className="wco-r flex h-16 items-center justify-between pl-5">
        <h3 className="font-display text-lg font-semibold">{t('viewer.details')}</h3>
        <IconButton label={t('common.close')} size="sm" onClick={onClose}>
          <X />
        </IconButton>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-5 pb-6">
        <InfoContent item={item} />
      </div>
    </motion.aside>
  );
}
