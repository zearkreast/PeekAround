import { memo } from 'react';
import { motion } from 'motion/react';
import { Check, Heart, Music, Play } from 'lucide-react';
import type { MediaItem } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { formatDuration } from '@/lib/format';
import { thumbhashUrl } from '@/lib/thumbhash';
import { useSelection } from '@/store/selection';
import { useViewer } from '@/store/viewer';

declare global {
  interface Window {
    /** perf harness: number of tile renders */
    __peekTileRenders?: number;
  }
}

export interface TileProps {
  index: number;
  /** undefined while its page is not loaded → neutral placeholder */
  item: MediaItem | undefined;
  x: number;
  y: number;
  w: number;
  h: number;
  /** thumbnail size bucket (320 | 640) */
  size: number;
  /** while flying over the grid (fast scroller) only placeholders are drawn */
  fast: boolean;
  /** hover video preview (mouse only) */
  preview: boolean;
  selecting: boolean;
}

function markLoaded(el: HTMLImageElement | null) {
  if (el && el.complete && el.naturalWidth) el.dataset.loaded = '1';
}

/**
 * One grid cell. Pure display: every interaction is handled by the grid through event
 * delegation (data-index), so tiles have no per-tile callbacks and memo always holds.
 * No backdrop-filter, no filter animations — just an opacity fade.
 */
function TileImpl({ index, item, x, y, w, h, size, fast, preview, selecting }: TileProps) {
  if (typeof window !== 'undefined') window.__peekTileRenders = (window.__peekTileRenders ?? 0) + 1;
  const t = useT();
  const backend = useBackend();
  const id = item?.id ?? '';
  const selected = useSelection((s) => (id ? s.ids.has(id) : false));
  const hero = useViewer((s) => (id ? s.activeId === id : false));

  const pos = { left: x, top: y, width: w, height: h };
  if (!item) return <div className="tile tile-ph" data-index={index} style={pos} aria-hidden />;

  const placeholder = thumbhashUrl(item.thumbhash);
  const isVideo = item.kind === 'video';
  const isAudio = item.kind === 'audio';
  const src = backend.thumbUrl(item.id, size);

  return (
    <div
      className="tile group"
      data-tile={item.id}
      data-index={index}
      data-selected={selected ? '1' : undefined}
      role="button"
      tabIndex={-1}
      aria-label={t('mobile.tile.open', { name: item.name })}
      aria-pressed={selecting ? selected : undefined}
      style={pos}
    >
      <div className="tile-inner" style={placeholder ? { backgroundImage: `url(${placeholder})` } : undefined}>
        {isAudio ? (
          <div className="grid size-full place-items-center bg-gradient-to-br from-[color-mix(in_oklab,var(--accent)_30%,transparent)] to-transparent">
            <Music className="size-10 text-dim" />
          </div>
        ) : fast && !hero ? null : hero ? (
          <motion.img key={item.id} layoutId={`media-${item.id}`} src={src} alt="" draggable={false} data-loaded="1" className="tile-img" />
        ) : (
          <img
            key={item.id}
            ref={markLoaded}
            src={src}
            alt=""
            draggable={false}
            decoding="async"
            onLoad={(e) => (e.currentTarget.dataset.loaded = '1')}
            onError={(e) => (e.currentTarget.style.visibility = 'hidden')}
            className="tile-img"
          />
        )}
        {preview && isVideo && (
          <video
            src={backend.previewUrl(item.id)}
            autoPlay
            muted
            loop
            playsInline
            onPlaying={(e) => (e.currentTarget.dataset.loaded = '1')}
            className="tile-img absolute inset-0"
          />
        )}
        {(isVideo || isAudio) && (
          <span className="tile-badge">
            {isVideo && <Play className="size-3 fill-current" />}
            {formatDuration(item.duration)}
          </span>
        )}
        {item.favorite && (
          <span className="absolute bottom-1.5 left-1.5 text-white drop-shadow">
            <Heart className="size-4 fill-[var(--accent-2)] text-[var(--accent-2)]" />
          </span>
        )}
      </div>
      <span data-check aria-hidden className="tile-check" data-on={selected ? '1' : undefined} data-show={selecting ? '1' : undefined}>
        <Check className="size-3.5" strokeWidth={3.5} />
      </span>
    </div>
  );
}

export const MediaTile = memo(TileImpl);
