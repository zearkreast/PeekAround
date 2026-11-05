import { useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { AnimatePresence, motion } from 'motion/react';
import { Play } from 'lucide-react';
import type { MediaDetails } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { Spinner } from '@/components/ui';
import { CropOverlay } from './CropOverlay';
import { TextOverlay } from './TextOverlay';
import { containSize, eqMatrix, isAdjusted, normalizeCrop, rotated, vignetteGradient, warmthGains, type Rotation, type Size } from './model';
import { useEditor, usePlayhead } from './store';
import type { Playback } from './usePlayback';

const SPRING = { type: 'spring', stiffness: 260, damping: 30 } as const;
const SCRUB_FRAMES = 30;

function useElementSize<T extends HTMLElement>() {
  const ref = useRef<T>(null);
  const [size, setSize] = useState<Size>({ w: 0, h: 0 });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => setSize({ w: e.contentRect.width, h: e.contentRect.height }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, size] as const;
}

/** Cumulative rotation so 270° → 0° animates a quarter turn instead of spinning back. */
function useTurns(rotate: Rotation): number {
  const prev = useRef({ r: rotate as number, deg: rotate as number });
  if (prev.current.r !== rotate) {
    const d = ((((rotate - prev.current.r) % 360) + 540) % 360) - 180;
    prev.current = { r: rotate, deg: prev.current.deg + d };
  }
  return prev.current.deg;
}

/** SVG colour filter approximating the ffmpeg eq → hue → colortemperature chain. */
function ColorFilter({ id }: { id: string }) {
  const adjust = useEditor((s) => s.s.adjust);
  const [r, g, b] = warmthGains(adjust.warmth);
  return (
    <svg aria-hidden className="pointer-events-none absolute size-0">
      <filter id={id} colorInterpolationFilters="sRGB" x="0" y="0" width="100%" height="100%">
        <feColorMatrix type="matrix" values={eqMatrix(adjust).join(' ')} />
        <feColorMatrix type="hueRotate" values={String(adjust.hue)} />
        <feColorMatrix type="matrix" values={`${r} 0 0 0 0  0 ${g} 0 0 0  0 0 ${b} 0 0  0 0 0 1 0`} />
      </filter>
    </svg>
  );
}

/** The preview: cropped / rotated / flipped video with colour, vignette and text — or the crop tool. */
export function Stage({ item, pb }: { item: MediaDetails; pb: Playback }) {
  const t = useT();
  const backend = useBackend();
  const [ref, avail] = useElementSize<HTMLDivElement>();
  const filterId = `vef${useId().replace(/[^a-zA-Z0-9]/g, '')}`;
  const src = useEditor((s) => s.src);
  const duration = useEditor((s) => s.duration);
  const st = useEditor((s) => s.s);
  const tool = useEditor((s) => s.tool);
  const playing = usePlayhead((s) => s.playing);
  const scrubbing = usePlayhead((s) => s.scrubbing);
  const scrubIdx = usePlayhead((s) => (pb.transcode && s.scrubbing ? Math.min(SCRUB_FRAMES - 1, Math.floor((s.time / Math.max(0.01, duration)) * SCRUB_FRAMES)) : -1));
  const [waiting, setWaiting] = useState(false);
  const turns = useTurns(st.rotate);

  const cropMode = tool === 'crop';
  const crop = cropMode ? null : normalizeCrop(st.crop, src.w, src.h);
  const region = crop ?? { x: 0, y: 0, w: src.w, h: src.h };
  const design: Size = rotated(st.rotate) ? { w: region.h, h: region.w } : { w: region.w, h: region.h };
  const pad = cropMode ? 44 : 20;
  const F = containSize({ w: avail.w - pad * 2, h: avail.h - pad * 2 }, design.w / design.h);
  const ds = design.w ? F.w / design.w : 0;
  const adjusted = isAdjusted(st.adjust);
  const vignette = vignetteGradient(st.adjust.vignette);

  useEffect(() => {
    if (cropMode) pb.pause();
  }, [cropMode, pb]);

  return (
    <div ref={ref} className="relative min-h-0 flex-1 overflow-hidden" data-testid="stage">
      <div className="pointer-events-none absolute inset-0 opacity-50">
        <div className="aurora" />
      </div>
      <ColorFilter id={filterId} />
      {F.w > 0 && (
        <div className="absolute inset-0 grid place-items-center">
          <motion.div
            className="relative overflow-hidden rounded-xl bg-black shadow-[0_30px_80px_-20px_rgb(0_0_0/0.8)]"
            initial={false}
            animate={{ width: F.w, height: F.h }}
            transition={SPRING}
          >
            <motion.div
              className="absolute top-1/2 left-1/2 overflow-hidden"
              style={{ x: '-50%', y: '-50%' }}
              initial={false}
              animate={{ width: region.w * ds, height: region.h * ds, rotate: turns, scaleX: st.flipH ? -1 : 1, scaleY: st.flipV ? -1 : 1 }}
              transition={SPRING}
            >
              <motion.video
                ref={pb.videoRef}
                src={pb.src}
                autoPlay={pb.autoPlay}
                playsInline
                preload="auto"
                className="absolute max-w-none select-none"
                style={{ filter: adjusted ? `url(#${filterId})` : undefined }}
                initial={false}
                animate={{ left: -region.x * ds, top: -region.y * ds, width: src.w * ds, height: src.h * ds }}
                transition={SPRING}
                {...pb.onVideoEvent}
                onWaiting={() => setWaiting(true)}
                onPlaying={() => setWaiting(false)}
                onCanPlay={() => setWaiting(false)}
                data-testid="preview-video"
              />
              {scrubIdx >= 0 && (
                <div
                  className="absolute bg-no-repeat"
                  style={{
                    left: -region.x * ds,
                    top: -region.y * ds,
                    width: src.w * ds,
                    height: src.h * ds,
                    backgroundImage: `url(${backend.filmstripUrl(item.id, SCRUB_FRAMES, 180)})`,
                    backgroundSize: `${SCRUB_FRAMES * 100}% 100%`,
                    backgroundPosition: `${(scrubIdx / (SCRUB_FRAMES - 1)) * 100}% 0`,
                    filter: adjusted ? `url(#${filterId})` : undefined,
                  }}
                />
              )}
            </motion.div>
            {!cropMode && vignette && <div className="pointer-events-none absolute inset-0" style={{ background: vignette }} />}
            {!cropMode && <TextOverlay frame={F} design={design} ds={ds} onBackgroundClick={pb.toggle} />}
            {cropMode && <CropOverlay ds={ds} />}
          </motion.div>
        </div>
      )}

      <AnimatePresence>
        {!playing && !cropMode && !scrubbing && F.w > 0 && (
          <motion.button
            type="button"
            initial={{ opacity: 0, scale: 0.7 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 1.3 }}
            transition={{ type: 'spring', stiffness: 400, damping: 25 }}
            onClick={pb.play}
            aria-label={t('viewer.play')}
            className="absolute top-1/2 left-1/2 grid size-16 -translate-x-1/2 -translate-y-1/2 place-items-center rounded-full bg-white/15 text-white shadow-xl backdrop-blur-xl hover:bg-white/25"
          >
            <Play className="ml-1 size-7 fill-current" />
          </motion.button>
        )}
      </AnimatePresence>
      {waiting && playing && (
        <div className="absolute top-4 right-4 rounded-full bg-black/50 p-2 backdrop-blur">
          <Spinner className="size-4 text-white" />
        </div>
      )}
      {pb.transcode && (
        <div className="absolute top-3 left-1/2 -translate-x-1/2 rounded-full bg-black/45 px-3 py-1 text-[11px] text-white/80 backdrop-blur">{t('viewer.transcoding')}</div>
      )}
    </div>
  );
}
