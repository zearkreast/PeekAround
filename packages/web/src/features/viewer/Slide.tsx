import { memo, useEffect, useState } from 'react';
import { motion, type MotionValue } from 'motion/react';
import { Play } from 'lucide-react';
import type { MediaItem } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { cn } from '@/lib/cn';
import { thumbhashUrl } from '@/lib/thumbhash';
import { decodeImage, isDecoded } from './imageCache';
import { VideoPlayer } from './VideoPlayer';

export function fitContain(W: number, H: number, aspect: number) {
  if (!W || !H) return { w: 0, h: 0 };
  let w = W;
  let h = W / aspect;
  if (h > H) {
    h = H;
    w = H * aspect;
  }
  return { w, h };
}

export function aspectOfItem(item: Pick<MediaItem, 'width' | 'height'>) {
  return item.width && item.height ? item.width / item.height : 1.5;
}

export interface ZoomValues {
  scale: MotionValue<number>;
  x: MotionValue<number>;
  y: MotionValue<number>;
}

/**
 * One viewer slide. Image pipeline: thumbhash bg → 640 thumb (usually cached from the grid)
 * → 1600 rendition swapped in only after img.decode() → zoom rendition (≤4096, never the raw
 * original) only when `hiRes` (zoomed past ~1.5×). Videos: a <video> only on the active slide.
 * Gesture transforms live on [data-zoom-target]; the hero layoutId sits on an inner <img>.
 */
export const Slide = memo(function Slide({
  item,
  W,
  H,
  active,
  hero,
  hiRes,
  zoom,
  paused,
  barInset = false,
}: {
  item: MediaItem;
  W: number;
  H: number;
  active: boolean;
  hero: boolean;
  hiRes: boolean;
  zoom: ZoomValues;
  /** pause video (swiping) */
  paused: boolean;
  /** phones: keep the video controls clear of the viewer's top/bottom bars */
  barInset?: boolean;
}) {
  const backend = useBackend();
  const [natAspect, setNatAspect] = useState<number | null>(null);
  const aspect = natAspect ?? aspectOfItem(item);
  const fit = fitContain(W, H, aspect);
  const thumb = backend.thumbUrl(item.id, 640);
  const midUrl = backend.thumbUrl(item.id, 1600);
  const zoomUrl = item.kind === 'image' ? backend.zoomUrl(item) : null;
  const [mid, setMid] = useState(() => isDecoded(midUrl));
  const [hi, setHi] = useState(() => !!zoomUrl && isDecoded(zoomUrl));
  const placeholder = thumbhashUrl(item.thumbhash);

  useEffect(() => {
    let on = true;
    void decodeImage(midUrl).then((ok) => on && ok && setMid(true));
    return () => {
      on = false;
    };
  }, [midUrl]);

  useEffect(() => {
    if (!hiRes || !zoomUrl || hi) return;
    let on = true;
    void decodeImage(zoomUrl).then((ok) => on && ok && setHi(true));
    return () => {
      on = false;
    };
  }, [hiRes, zoomUrl, hi]);

  if (item.kind === 'video' && active) {
    return (
      <div className={cn('absolute inset-0 flex items-center justify-center', barInset && 'pt-16 pb-[calc(92px+env(safe-area-inset-bottom,0px))]')} data-viewer-video>
        <VideoPlayer key={item.id} item={item} poster={midUrl} autoPlay={!paused} />
      </div>
    );
  }

  return (
    <motion.div
      data-zoom-target
      className="absolute"
      style={{ width: fit.w, height: fit.h, left: (W - fit.w) / 2, top: (H - fit.h) / 2, scale: zoom.scale, x: zoom.x, y: zoom.y }}
    >
      {placeholder && <img src={placeholder} alt="" aria-hidden className="absolute inset-0 size-full rounded-sm object-cover" draggable={false} />}
      <motion.img
        layoutId={hero ? `media-${item.id}` : undefined}
        src={thumb}
        alt=""
        draggable={false}
        onLoad={(e) => {
          const im = e.currentTarget;
          if (!item.width && im.naturalWidth && im.naturalHeight) setNatAspect(im.naturalWidth / im.naturalHeight);
        }}
        className="absolute inset-0 size-full object-contain"
        transition={{ type: 'spring', stiffness: 320, damping: 34 }}
      />
      {mid && <img src={midUrl} alt={item.name} draggable={false} className="absolute inset-0 size-full object-contain" />}
      {hi && zoomUrl && <img src={zoomUrl} alt="" draggable={false} className="absolute inset-0 size-full object-contain" />}
      {item.kind === 'video' && (
        <span className="absolute inset-0 grid place-items-center">
          <span className="grid size-16 place-items-center rounded-full bg-black/45 text-white">
            <Play className="ml-1 size-8 fill-current" />
          </span>
        </span>
      )}
    </motion.div>
  );
});
