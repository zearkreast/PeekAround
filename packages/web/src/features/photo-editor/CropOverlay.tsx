import { useRef, useState } from 'react';
import { motion } from 'motion/react';
import { cn } from '@/lib/cn';
import { frameToScreen, type View } from './geometry';
import type { Rect } from './types';

type Mode = 'move' | 'n' | 's' | 'e' | 'w' | 'nw' | 'ne' | 'sw' | 'se';

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/** New crop (frame px) after dragging a handle by (dx, dy) frame px. */
function resize(s: Rect, mode: Mode, dx: number, dy: number, ratio: number | null, W: number, H: number, min: number): Rect {
  if (mode === 'move') return { ...s, x: clamp(s.x + dx, 0, W - s.w), y: clamp(s.y + dy, 0, H - s.h) };
  let x0 = s.x;
  let y0 = s.y;
  let x1 = s.x + s.w;
  let y1 = s.y + s.h;
  if (mode.includes('w')) x0 = clamp(x0 + dx, 0, x1 - min);
  if (mode.includes('e')) x1 = clamp(x1 + dx, x0 + min, W);
  if (mode.includes('n')) y0 = clamp(y0 + dy, 0, y1 - min);
  if (mode.includes('s')) y1 = clamp(y1 + dy, y0 + min, H);
  if (!ratio) return { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };

  if (mode.length === 2) {
    // corner: keep the opposite corner fixed
    const ax = mode.includes('w') ? s.x + s.w : s.x;
    const ay = mode.includes('n') ? s.y + s.h : s.y;
    let w = x1 - x0;
    let h = y1 - y0;
    if (w / h > ratio) w = h * ratio;
    else h = w / ratio;
    const maxW = mode.includes('w') ? ax : W - ax;
    const maxH = mode.includes('n') ? ay : H - ay;
    if (w > maxW) (w = maxW), (h = w / ratio);
    if (h > maxH) (h = maxH), (w = h * ratio);
    return { x: mode.includes('w') ? ax - w : ax, y: mode.includes('n') ? ay - h : ay, w, h };
  }
  // edge: the other dimension follows, centred
  if (mode === 'n' || mode === 's') {
    let h = y1 - y0;
    let w = h * ratio;
    if (w > W) (w = W), (h = w / ratio);
    const cx = s.x + s.w / 2;
    return { x: clamp(cx - w / 2, 0, W - w), y: mode === 'n' ? s.y + s.h - h : s.y, w, h };
  }
  let w = x1 - x0;
  let h = w / ratio;
  if (h > H) (h = H), (w = h * ratio);
  const cy = s.y + s.h / 2;
  return { x: mode === 'w' ? s.x + s.w - w : s.x, y: clamp(cy - h / 2, 0, H - h), w, h };
}

const CORNERS: { m: Mode; cls: string; path: string }[] = [
  { m: 'nw', cls: '-left-1 -top-1 cursor-nwse-resize', path: 'M3 21V3h18' },
  { m: 'ne', cls: '-right-1 -top-1 cursor-nesw-resize', path: 'M3 3h18v18' },
  { m: 'sw', cls: '-left-1 -bottom-1 cursor-nesw-resize', path: 'M3 3v18h18' },
  { m: 'se', cls: '-right-1 -bottom-1 cursor-nwse-resize', path: 'M21 3v18H3' },
];

const EDGES: { m: Mode; cls: string; bar: string }[] = [
  { m: 'n', cls: 'left-1/2 -top-3 h-6 w-10 -translate-x-1/2 cursor-ns-resize', bar: 'h-1 w-7' },
  { m: 's', cls: 'left-1/2 -bottom-3 h-6 w-10 -translate-x-1/2 cursor-ns-resize', bar: 'h-1 w-7' },
  { m: 'w', cls: 'top-1/2 -left-3 h-10 w-6 -translate-y-1/2 cursor-ew-resize', bar: 'h-7 w-1' },
  { m: 'e', cls: 'top-1/2 -right-3 h-10 w-6 -translate-y-1/2 cursor-ew-resize', bar: 'h-7 w-1' },
];

/** Crop box with dimmed surroundings, rule-of-thirds grid and draggable handles. */
export function CropOverlay({
  view,
  stageW,
  stageH,
  frameW,
  frameH,
  crop,
  ratio,
  onChange,
}: {
  view: View;
  stageW: number;
  stageH: number;
  frameW: number;
  frameH: number;
  crop: Rect; // normalized
  ratio: number | null;
  onChange: (crop: Rect) => void;
}) {
  const [drag, setDrag] = useState<Mode | null>(null);
  const r = { x: crop.x * frameW, y: crop.y * frameH, w: crop.w * frameW, h: crop.h * frameH };
  const [sx, sy] = frameToScreen(view, stageW, stageH, r.x, r.y);
  const sw = r.w * view.scale;
  const sh = r.h * view.scale;
  const latest = useRef(onChange);
  latest.current = onChange;

  const start = (mode: Mode) => (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const s0 = { ...r };
    const p0 = { x: e.clientX, y: e.clientY };
    const min = Math.max(16, 48 / view.scale);
    setDrag(mode);
    const move = (ev: PointerEvent) => {
      const n = resize(s0, mode, (ev.clientX - p0.x) / view.scale, (ev.clientY - p0.y) / view.scale, ratio, frameW, frameH, min);
      latest.current({ x: n.x / frameW, y: n.y / frameH, w: n.w / frameW, h: n.h / frameH });
    };
    const up = () => {
      setDrag(null);
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      window.removeEventListener('pointercancel', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
    window.addEventListener('pointercancel', up);
  };

  return (
    <div className="pointer-events-none absolute inset-0 overflow-hidden">
      <motion.div
        className="pointer-events-auto absolute cursor-move touch-none"
        initial={false}
        animate={{ left: sx, top: sy, width: sw, height: sh }}
        transition={drag ? { duration: 0 } : { type: 'spring', stiffness: 700, damping: 48 }}
        style={{ boxShadow: '0 0 0 9999px rgb(5 5 8 / 0.62)' }}
        onPointerDown={start('move')}
      >
        <div className="absolute inset-0 outline outline-1 outline-white/90" />
        {/* rule of thirds */}
        <div className={cn('pointer-events-none absolute inset-0 transition-opacity duration-200', drag ? 'opacity-100' : 'opacity-55')}>
          {[1, 2].map((i) => (
            <div key={`v${i}`} className="absolute inset-y-0 w-px bg-white/55 shadow-[0_0_2px_rgb(0_0_0/0.4)]" style={{ left: `${(i * 100) / 3}%` }} />
          ))}
          {[1, 2].map((i) => (
            <div key={`h${i}`} className="absolute inset-x-0 h-px bg-white/55 shadow-[0_0_2px_rgb(0_0_0/0.4)]" style={{ top: `${(i * 100) / 3}%` }} />
          ))}
        </div>
        {CORNERS.map((c) => (
          <div key={c.m} className={cn('absolute size-6 touch-none', c.cls)} onPointerDown={start(c.m)}>
            <svg viewBox="0 0 24 24" className="size-full overflow-visible drop-shadow-[0_1px_3px_rgb(0_0_0/0.6)]">
              <path d={c.path} fill="none" stroke="white" strokeWidth={3.5} strokeLinecap="round" />
            </svg>
          </div>
        ))}
        {EDGES.map((ed) => (
          <div key={ed.m} className={cn('absolute grid touch-none place-items-center', ed.cls)} onPointerDown={start(ed.m)}>
            <span className={cn('rounded-full bg-white shadow-[0_1px_3px_rgb(0_0_0/0.6)]', ed.bar)} />
          </div>
        ))}
      </motion.div>
    </div>
  );
}
