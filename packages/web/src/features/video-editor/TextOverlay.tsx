import { useEffect, useMemo, useRef, useState } from 'react';
import { drawTextLayer, textBox, type TextLayer } from '@/lib/text';
import { ensureFont } from '@/lib/fonts';
import { cn } from '@/lib/cn';
import { outDuration, srcToOut, textWindow, type Size } from './model';
import { useEditor, usePlayhead } from './store';

let measureCtx: CanvasRenderingContext2D | null = null;
function mctx() {
  if (!measureCtx) measureCtx = document.createElement('canvas').getContext('2d');
  return measureCtx!;
}

/** Point (design px) inside a (rotated) text layer? */
function hit(l: TextLayer, x: number, y: number): boolean {
  const b = textBox(mctx(), l);
  const a = (-l.rotation * Math.PI) / 180;
  const dx = x - l.x;
  const dy = y - l.y;
  const rx = l.x + dx * Math.cos(a) - dy * Math.sin(a);
  const ry = l.y + dx * Math.sin(a) + dy * Math.cos(a);
  const pad = 6;
  return rx >= b.x - pad && rx <= b.x + b.width + pad && ry >= b.y - pad && ry <= b.y + b.height + pad;
}

type Drag = { kind: 'move' | 'scale'; id: string; px: number; py: number; start: TextLayer; d0: number };

/**
 * Text layers drawn on a canvas over the preview with the same drawing code that renders
 * the export overlays (lib/text). Click to select, drag to move, corner handle to resize.
 */
export function TextOverlay({ frame, design, ds, onBackgroundClick }: { frame: Size; design: Size; ds: number; onBackgroundClick: () => void }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const wrap = useRef<HTMLDivElement>(null);
  const texts = useEditor((s) => s.s.texts);
  const trim = useEditor((s) => s.s.trim);
  const speed = useEditor((s) => s.s.speed);
  const tool = useEditor((s) => s.tool);
  const selected = useEditor((s) => s.selectedText);
  const set = useEditor((s) => s.set);
  const select = useEditor((s) => s.select);
  const [fontTick, setFontTick] = useState(0);
  const drag = useRef<Drag | null>(null);
  const dur = outDuration({ trim, speed });

  // which layers are visible right now (re-render only when that set changes)
  const visibleKey = usePlayhead((p) => {
    const t = srcToOut({ trim, speed }, p.time);
    return texts
      .filter((l) => {
        const w = textWindow(l, dur);
        return t >= w.start - 1e-3 && t <= w.end + 1e-3;
      })
      .map((l) => l.id)
      .join('|');
  });
  const visible = useMemo(() => new Set(visibleKey.split('|').filter(Boolean)), [visibleKey]);
  // while editing text, the selected layer is always shown (dimmed when outside its time window)
  const shown = texts.filter((l) => visible.has(l.id) || (tool === 'text' && l.id === selected));

  useEffect(() => {
    let alive = true;
    void Promise.all(texts.map((l) => ensureFont(l.fontFamily, l.fontWeight, l.italic))).then(() => alive && setFontTick((n) => n + 1));
    return () => {
      alive = false;
    };
  }, [texts]);

  useEffect(() => {
    const c = canvas.current;
    if (!c) return;
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    const w = Math.max(1, Math.round(frame.w * dpr));
    const h = Math.max(1, Math.round(frame.h * dpr));
    if (c.width !== w) c.width = w;
    if (c.height !== h) c.height = h;
    const ctx = c.getContext('2d')!;
    ctx.clearRect(0, 0, w, h);
    for (const l of shown) {
      ctx.save();
      if (!visible.has(l.id)) ctx.globalAlpha = 0.45;
      drawTextLayer(ctx, l, ds * dpr);
      ctx.restore();
    }
  });

  const toDesign = (e: React.PointerEvent) => {
    const r = wrap.current!.getBoundingClientRect();
    return { x: (e.clientX - r.left) / ds, y: (e.clientY - r.top) / ds };
  };

  const onDown = (e: React.PointerEvent) => {
    if (e.button !== 0) return;
    const p = toDesign(e);
    const target = [...shown].reverse().find((l) => hit(l, p.x, p.y));
    if (!target) {
      select(null);
      onBackgroundClick();
      return;
    }
    e.preventDefault();
    select(target.id);
    wrap.current!.setPointerCapture(e.pointerId);
    drag.current = { kind: 'move', id: target.id, px: p.x, py: p.y, start: target, d0: 1 };
  };

  const onScaleDown = (e: React.PointerEvent, l: TextLayer) => {
    e.stopPropagation();
    e.preventDefault();
    const p = toDesign(e);
    wrap.current!.setPointerCapture(e.pointerId);
    drag.current = { kind: 'scale', id: l.id, px: p.x, py: p.y, start: l, d0: Math.max(4, Math.hypot(p.x - l.x, p.y - l.y)) };
  };

  const onMove = (e: React.PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    const p = toDesign(e);
    if (d.kind === 'move') {
      const x = Math.max(0, Math.min(design.w, d.start.x + p.x - d.px));
      const y = Math.max(0, Math.min(design.h, d.start.y + p.y - d.py));
      set((s) => ({ texts: s.texts.map((l) => (l.id === d.id ? { ...l, x, y } : l)) }));
    } else {
      const k = Math.max(0.1, Math.hypot(p.x - d.start.x, p.y - d.start.y) / d.d0);
      const fontSize = Math.max(6, Math.min(design.h * 1.5, d.start.fontSize * k));
      const f = fontSize / d.start.fontSize;
      set((s) => ({
        texts: s.texts.map((l) =>
          l.id === d.id
            ? {
                ...l,
                fontSize,
                stroke: d.start.stroke ? { ...d.start.stroke, width: d.start.stroke.width * f } : null,
                background: d.start.background ? { ...d.start.background, padding: d.start.background.padding * f, radius: d.start.background.radius * f } : null,
              }
            : l,
        ),
      }));
    }
  };
  const onUp = () => {
    drag.current = null;
  };

  const sel = shown.find((l) => l.id === selected);
  const box = sel ? textBox(mctx(), sel) : null;
  void fontTick;

  return (
    <div
      ref={wrap}
      className={cn('absolute inset-0 touch-none', shown.length ? 'cursor-default' : 'cursor-pointer')}
      onPointerDown={onDown}
      onPointerMove={onMove}
      onPointerUp={onUp}
      onPointerCancel={onUp}
      onDoubleClick={() => sel && useEditor.getState().setTool('text')}
      data-testid="text-overlay"
    >
      <canvas ref={canvas} className="pointer-events-none absolute inset-0 size-full" />
      {sel && box && (
        <div
          className="pointer-events-none absolute rounded-md border border-dashed border-white/90 shadow-[0_0_0_1px_rgb(0_0_0/0.35)]"
          style={{
            left: box.x * ds - 6,
            top: box.y * ds - 6,
            width: box.width * ds + 12,
            height: box.height * ds + 12,
            transform: `rotate(${sel.rotation}deg)`,
            transformOrigin: `${(sel.x - box.x) * ds + 6}px ${(sel.y - box.y) * ds + 6}px`,
          }}
        >
          <div
            className="pointer-events-auto absolute -right-2 -bottom-2 size-4 cursor-nwse-resize rounded-full border-2 border-white bg-accent shadow"
            onPointerDown={(e) => onScaleDown(e, sel)}
          />
        </div>
      )}
    </div>
  );
}
