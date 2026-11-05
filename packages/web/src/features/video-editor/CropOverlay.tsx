import { useRef, useState } from 'react';
import { motion } from 'motion/react';
import { cn } from '@/lib/cn';
import { ASPECT_RATIOS, normalizeCrop, orientRect, resizeRect, rotated, unorientRect, type Handle, type Rect, type Size } from './model';
import { useEditor } from './store';

const HANDLES: { h: Handle; cls: string; cursor: string }[] = [
  { h: 'nw', cls: '-left-1 -top-1', cursor: 'nwse-resize' },
  { h: 'ne', cls: '-right-1 -top-1', cursor: 'nesw-resize' },
  { h: 'sw', cls: '-left-1 -bottom-1', cursor: 'nesw-resize' },
  { h: 'se', cls: '-right-1 -bottom-1', cursor: 'nwse-resize' },
  { h: 'n', cls: 'left-1/2 -top-1 -translate-x-1/2', cursor: 'ns-resize' },
  { h: 's', cls: 'left-1/2 -bottom-1 -translate-x-1/2', cursor: 'ns-resize' },
  { h: 'w', cls: '-left-1 top-1/2 -translate-y-1/2', cursor: 'ew-resize' },
  { h: 'e', cls: '-right-1 top-1/2 -translate-y-1/2', cursor: 'ew-resize' },
];

/**
 * Crop box drawn over the full (rotated / flipped) frame. The rect is edited in
 * oriented coordinates and stored back in source pixels.
 */
export function CropOverlay({ ds }: { ds: number }) {
  const src = useEditor((s) => s.src);
  const crop = useEditor((s) => s.s.crop);
  const rot = useEditor((s) => s.s.rotate);
  const flipH = useEditor((s) => s.s.flipH);
  const flipV = useEditor((s) => s.s.flipV);
  const aspect = useEditor((s) => s.s.aspect);
  const set = useEditor((s) => s.set);
  const drag = useRef<{ handle: Handle; start: Rect; px: number; py: number } | null>(null);
  const [dragging, setDragging] = useState(false);

  const O: Size = rotated(rot) ? { w: src.h, h: src.w } : src;
  const c = normalizeCrop(crop, src.w, src.h);
  const R = c ? orientRect(c, src, rot, flipH, flipV) : { x: 0, y: 0, w: O.w, h: O.h };
  const ratio = aspect === 'free' ? null : ASPECT_RATIOS[aspect];
  const minPx = Math.max(16, Math.min(O.w, O.h) * 0.04);

  const begin = (handle: Handle) => (e: React.PointerEvent) => {
    e.stopPropagation();
    e.preventDefault();
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    drag.current = { handle, start: R, px: e.clientX, py: e.clientY };
    setDragging(true);
  };
  const move = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d || !ds) return;
    const next = resizeRect(d.start, d.handle, (e.clientX - d.px) / ds, (e.clientY - d.py) / ds, O, ratio, minPx);
    set({ crop: unorientRect(next, src, rot, flipH, flipV) });
  };
  const end = () => {
    drag.current = null;
    setDragging(false);
  };

  return (
    <motion.div
      className={cn('absolute touch-none border-2 border-white/90', dragging ? 'cursor-grabbing' : 'cursor-grab')}
      style={{ left: R.x * ds, top: R.y * ds, width: R.w * ds, height: R.h * ds, boxShadow: '0 0 0 9999px rgb(0 0 0 / 0.58)' }}
      onPointerDown={begin('move')}
      onPointerMove={move}
      onPointerUp={end}
      onPointerCancel={end}
      initial={{ opacity: 0, scale: 0.96 }}
      animate={{ opacity: 1, scale: 1 }}
      transition={{ type: 'spring', stiffness: 420, damping: 32 }}
      data-testid="crop-box"
    >
      {/* rule of thirds */}
      <div className="pointer-events-none absolute inset-0">
        {[1, 2].map((i) => (
          <div key={`v${i}`} className="absolute inset-y-0 w-px bg-white/35" style={{ left: `${(i * 100) / 3}%` }} />
        ))}
        {[1, 2].map((i) => (
          <div key={`h${i}`} className="absolute inset-x-0 h-px bg-white/35" style={{ top: `${(i * 100) / 3}%` }} />
        ))}
      </div>
      {HANDLES.map(({ h, cls, cursor }) => (
        <div
          key={h}
          onPointerDown={begin(h)}
          onPointerMove={move}
          onPointerUp={end}
          onPointerCancel={end}
          style={{ cursor }}
          className={cn(
            'absolute z-10 touch-none rounded-full bg-white shadow-[0_0_0_1px_rgb(0_0_0/0.25),0_2px_8px_rgb(0_0_0/0.4)] transition-transform hover:scale-125',
            h.length === 2 ? 'size-4' : h === 'n' || h === 's' ? 'h-2 w-7' : 'h-7 w-2',
            cls,
          )}
        />
      ))}
      <div className="pointer-events-none absolute -top-8 left-1/2 -translate-x-1/2 rounded-full bg-black/60 px-2.5 py-1 text-[11px] font-medium text-white tabular-nums backdrop-blur">
        {Math.round(R.w)} × {Math.round(R.h)}
      </div>
    </motion.div>
  );
}
