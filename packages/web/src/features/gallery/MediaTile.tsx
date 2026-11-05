import { memo, useEffect, useRef, useState, type CSSProperties, type MouseEvent } from 'react';
import { motion } from 'motion/react';
import { Check, Heart, Music, Play } from 'lucide-react';
import type { MediaItem } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { cn } from '@/lib/cn';
import { formatDuration } from '@/lib/format';
import { thumbhashUrl } from '@/lib/thumbhash';
import { usePrefs } from '@/store/prefs';
import { useViewer } from '@/store/viewer';

export interface TileProps {
  item: MediaItem;
  style: CSSProperties;
  selected: boolean;
  selecting: boolean;
  /** stagger index for the entrance animation */
  delay: number;
  onClick: (item: MediaItem, e: MouseEvent) => void;
  onToggle: (item: MediaItem, e: MouseEvent) => void;
  onContext: (item: MediaItem, e: MouseEvent) => void;
}

function TileImpl({ item, style, selected, selecting, delay, onClick, onToggle, onContext }: TileProps) {
  const backend = useBackend();
  const activeId = useViewer((s) => s.activeId);
  const hoverPreview = usePrefs((s) => s.hoverPreview);
  const [loaded, setLoaded] = useState(false);
  const [preview, setPreview] = useState(false);
  const [previewReady, setPreviewReady] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const press = useRef<{ t: ReturnType<typeof setTimeout>; x: number; y: number } | null>(null);
  const longPressed = useRef(false);
  const h = typeof style.height === 'number' ? style.height : 240;
  const dpr = typeof window !== 'undefined' ? Math.min(2, window.devicePixelRatio || 1) : 1;
  const size = h * dpr > 330 ? 640 : 320;
  const placeholder = thumbhashUrl(item.thumbhash);
  const isVideo = item.kind === 'video';
  const isAudio = item.kind === 'audio';

  useEffect(() => () => void (timer.current && clearTimeout(timer.current)), []);

  return (
    <div
      className={cn('group absolute select-none', delay >= 0 && 'tile-in')}
      style={delay >= 0 ? { ...style, animationDelay: `${Math.min(delay, 24) * 22}ms` } : style}
      data-tile={item.id}
      onContextMenu={(e) => {
        e.preventDefault();
        if (!longPressed.current) onContext(item, e);
      }}
      onMouseEnter={() => {
        if (!isVideo || !hoverPreview) return;
        timer.current = setTimeout(() => setPreview(true), 450);
      }}
      onMouseLeave={() => {
        if (timer.current) clearTimeout(timer.current);
        setPreview(false);
        setPreviewReady(false);
      }}
    >
      <button
        type="button"
        aria-label={item.name}
        onPointerDown={(e) => {
          if (e.pointerType !== 'touch') return;
          longPressed.current = false;
          const x = e.clientX;
          const y = e.clientY;
          press.current = {
            x,
            y,
            t: setTimeout(() => {
              longPressed.current = true;
              navigator.vibrate?.(12);
              onToggle(item, e);
            }, 450),
          };
        }}
        onPointerMove={(e) => {
          if (press.current && Math.hypot(e.clientX - press.current.x, e.clientY - press.current.y) > 10) {
            clearTimeout(press.current.t);
            press.current = null;
          }
        }}
        onPointerUp={() => {
          if (press.current) clearTimeout(press.current.t);
          press.current = null;
        }}
        onPointerCancel={() => {
          if (press.current) clearTimeout(press.current.t);
          press.current = null;
        }}
        onContextMenu={(e) => {
          // long-press on touch shouldn't open the context menu
          if (longPressed.current) e.preventDefault();
        }}
        onClick={(e) => {
          if (longPressed.current) {
            longPressed.current = false;
            return;
          }
          onClick(item, e);
        }}
        className={cn(
          'relative block size-full overflow-hidden rounded-[10px] bg-[var(--tile-bg)] outline-offset-2 transition-[transform,border-radius] duration-300 ease-[var(--ease-out)]',
          selected && 'scale-[0.9] rounded-[14px]',
        )}
        style={placeholder ? { backgroundImage: `url(${placeholder})`, backgroundSize: 'cover', backgroundPosition: 'center' } : undefined}
      >
        {isAudio ? (
          <div className="grid size-full place-items-center bg-gradient-to-br from-[color-mix(in_oklab,var(--accent)_30%,transparent)] to-transparent">
            <Music className="size-10 text-dim" />
          </div>
        ) : (
          <motion.img
            layoutId={activeId === item.id ? `media-${item.id}` : undefined}
            src={backend.thumbUrl(item.id, size)}
            alt=""
            draggable={false}
            loading="lazy"
            decoding="async"
            onLoad={() => setLoaded(true)}
            className={cn(
              'size-full object-cover transition-[opacity,transform,filter] duration-500 ease-[var(--ease-out)] group-hover:scale-[1.045]',
              loaded ? 'opacity-100 blur-0' : 'opacity-0 blur-sm',
            )}
          />
        )}
        {preview && (
          <video
            src={backend.previewUrl(item.id)}
            autoPlay
            muted
            loop
            playsInline
            onPlaying={() => setPreviewReady(true)}
            className={cn('absolute inset-0 size-full object-cover transition-opacity duration-300', previewReady ? 'opacity-100' : 'opacity-0')}
          />
        )}
        {/* gradient for legibility of badges */}
        <span className="pointer-events-none absolute inset-0 bg-gradient-to-b from-black/35 via-transparent to-black/40 opacity-0 transition-opacity duration-300 group-hover:opacity-100" />
        {(isVideo || isAudio) && (
          <span className="absolute right-2 bottom-2 inline-flex items-center gap-1 rounded-full bg-black/55 px-2 py-0.5 text-[11px] font-medium text-white backdrop-blur-md tabular-nums">
            {isVideo && <Play className="size-3 fill-current" />}
            {formatDuration(item.duration)}
          </span>
        )}
        {item.favorite && (
          <span className="absolute bottom-2 left-2 text-white drop-shadow">
            <Heart className="size-4 fill-[var(--accent-2)] text-[var(--accent-2)]" />
          </span>
        )}
      </button>
      {/* selection check */}
      <button
        type="button"
        aria-label="Select"
        onClick={(e) => {
          e.stopPropagation();
          onToggle(item, e);
        }}
        className={cn(
          'absolute top-2 left-2 grid size-6 place-items-center rounded-full border-2 transition-all duration-200',
          selected
            ? 'scale-100 border-transparent bg-accent text-white opacity-100 shadow-[0_2px_10px_-2px_var(--accent)]'
            : cn('border-white/90 bg-black/20 text-transparent backdrop-blur-sm hover:bg-black/40', selecting ? 'opacity-100' : 'scale-90 opacity-0 group-hover:scale-100 group-hover:opacity-100'),
        )}
      >
        <Check className="size-3.5" strokeWidth={3.5} />
      </button>
    </div>
  );
}

export const MediaTile = memo(TileImpl);
