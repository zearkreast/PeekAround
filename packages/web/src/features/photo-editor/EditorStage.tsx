import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { animate, AnimatePresence, motion } from 'motion/react';
import type Konva from 'konva';
import { Group, Layer, Line, Shape, Stage } from 'react-konva';
import { useBackend } from '@/backend';
import { cn } from '@/lib/cn';
import { useFonts } from '@/lib/fonts';
import { thumbhashUrl } from '@/lib/thumbhash';
import type { TextLayer } from '@/lib/text';
import { usePrefs } from '@/store/prefs';
import { effectiveAdjust, NEUTRAL_RENDER } from './adjust';
import { grainCellFor, useEnv } from './context';
import { CropOverlay } from './CropOverlay';
import { aspectRatio, clampView, cropPx, fitView, frameSize, frameToScreen, lerpView, screenToFrame, type View } from './geometry';
import { LayerTransformer, TextNode, useAccent, useFontsVersion } from './konva/TextNode';
import { useEditor } from './store';
import { drawStroke } from './strokes';
import type { Rect, Stroke } from './types';

function useSize(ref: React.RefObject<HTMLElement | null>) {
  const [size, setSize] = useState({ w: 0, h: 0 });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setSize({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    setSize({ w: el.clientWidth, h: el.clientHeight });
    return () => ro.disconnect();
  }, [ref]);
  return size;
}

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

/** The dark stage: WebGL preview + Konva layers (text, drawing) + crop overlay, with animated zoom/pan. */
export function EditorStage() {
  const env = useEnv();
  const backend = useBackend();
  const accent = useAccent();
  const doc = useEditor((s) => s.h.doc);
  const tab = useEditor((s) => s.tab);
  const compare = useEditor((s) => s.compare);
  const zoom = useEditor((s) => s.zoom);
  const selectedId = useEditor((s) => s.selectedId);
  const brush = useEditor((s) => s.brush);
  const fx = useEditor((s) => s.fx);
  const motionPref = usePrefs((s) => s.motion);

  const boxRef = useRef<HTMLDivElement>(null);
  const glHost = useRef<HTMLDivElement>(null);
  const spinRef = useRef<HTMLDivElement>(null);
  const size = useSize(boxRef);
  const ready = !!env.renderer?.hasSource() && !!env.source;

  const frame = frameSize(env.srcW, env.srcH, doc.geo.rot);
  const crop = cropPx(doc.geo, frame.w, frame.h);
  const cropMode = tab === 'crop';
  const drawMode = tab === 'draw';
  const fullFrame: Rect = { x: 0, y: 0, w: frame.w, h: frame.h };
  const region = cropMode ? fullFrame : crop;
  const pad = size.w < 640 ? 14 : cropMode ? 44 : 32;

  // ───────── view (zoom / pan), animated between targets ─────────
  const [view, setViewState] = useState<View | null>(null);
  const viewRef = useRef<View | null>(null);
  const anim = useRef<{ stop(): void } | null>(null);
  const setView = useCallback((v: View) => {
    viewRef.current = v;
    setViewState(v);
  }, []);
  const prevSize = useRef(size);
  const live = useRef({ region, size, pad });
  live.current = { region, size, pad };

  useLayoutEffect(() => {
    if (!size.w || !size.h) return;
    const fit = fitView(region, size.w, size.h, pad);
    const cur = viewRef.current;
    const target = zoom === 'fit' ? fit : clampView({ scale: zoom, cx: cur?.cx ?? fit.cx, cy: cur?.cy ?? fit.cy }, region, size.w, size.h);
    const resized = prevSize.current.w !== size.w || prevSize.current.h !== size.h;
    prevSize.current = size;
    anim.current?.stop();
    if (!cur || resized || motionPref === 'off') return setView(target);
    if (Math.abs(cur.scale - target.scale) < 1e-6 && Math.abs(cur.cx - target.cx) < 0.01 && Math.abs(cur.cy - target.cy) < 0.01) return;
    const from = cur;
    anim.current = animate(0, 1, {
      type: 'spring',
      stiffness: 230,
      damping: 30,
      restDelta: 0.0005,
      onUpdate: (t) => setView(lerpView(from, target, t)),
    });
  }, [region.x, region.y, region.w, region.h, size.w, size.h, zoom, pad, motionPref, setView]);

  // rotate / flip flourish
  useEffect(() => {
    if (!fx || !spinRef.current || motionPref === 'off') return;
    const el = spinRef.current;
    const spring = { type: 'spring' as const, stiffness: 240, damping: 22 };
    if (fx.kind === 'rotate') void animate(el, { rotate: [-90 * fx.dir, 0] }, spring);
    else if (fx.kind === 'flipH') void animate(el, { scaleX: [-1, 1] }, spring);
    else void animate(el, { scaleY: [-1, 1] }, spring);
  }, [fx, motionPref]);

  // ───────── WebGL preview ─────────
  useEffect(() => {
    const c = env.renderer?.canvas;
    const host = glHost.current;
    if (!c || !host) return;
    c.className = 'absolute inset-0 size-full';
    host.appendChild(c);
    return () => c.remove();
  }, [env.renderer]);

  const eff = useMemo(() => (compare ? NEUTRAL_RENDER : effectiveAdjust(doc.adjust, doc.filter)), [compare, doc.adjust, doc.filter]);
  const dpr = Math.min(2, window.devicePixelRatio || 1);
  useLayoutEffect(() => {
    const r = env.renderer;
    if (!r || !view || !size.w || !ready) return;
    const k = 1 / (view.scale * dpr);
    r.render(
      {
        adjust: eff,
        srcW: env.srcW,
        srcH: env.srcH,
        rot: doc.geo.rot,
        flipH: doc.geo.flipH,
        flipV: doc.geo.flipV,
        straighten: doc.geo.straighten,
        frameW: frame.w,
        frameH: frame.h,
        ox: view.cx - size.w / 2 / view.scale,
        oy: view.cy - size.h / 2 / view.scale,
        k,
        clip: cropMode ? fullFrame : crop,
        crop,
        grainCell: Math.max(grainCellFor(crop), k),
        sharpenStep: Math.max(1, k),
      },
      size.w * dpr,
      size.h * dpr,
    );
  }, [env.renderer, env.srcW, env.srcH, view, size.w, size.h, ready, eff, doc.geo, cropMode, dpr, frame.w, frame.h, crop.x, crop.y, crop.w, crop.h]);

  // ───────── Konva layers ─────────
  useFonts(); // registers uploaded fonts (FontFace) so canvas text can use them
  const fontsVersion = useFontsVersion(doc.layers);
  const nodes = useRef(new Map<string, Konva.Shape>());
  const [selNode, setSelNode] = useState<Konva.Shape | null>(null);
  const registerNode = useCallback((id: string, n: Konva.Shape | null) => {
    if (n) nodes.current.set(id, n);
    else nodes.current.delete(id);
  }, []);
  useEffect(() => setSelNode(selectedId ? (nodes.current.get(selectedId) ?? null) : null), [selectedId, doc.layers, tab, compare]);
  const [guides, setGuides] = useState({ x: false, y: false });
  const interactive = !cropMode && !drawMode && !compare;

  const onLayerChange = useCallback((id: string, patch: Partial<TextLayer>) => {
    useEditor.getState().change((d) => ({ ...d, layers: d.layers.map((l) => (l.id === id ? { ...l, ...patch } : l)) }));
  }, []);
  const onSelect = useCallback((id: string) => {
    const s = useEditor.getState();
    s.select(id);
    if (s.tab !== 'text') s.setTab('text');
  }, []);
  const onDblClick = useCallback((id: string) => {
    onSelect(id);
    requestAnimationFrame(() => {
      const ta = document.querySelector<HTMLTextAreaElement>('[data-text-panel] textarea');
      ta?.focus();
      ta?.select();
    });
  }, [onSelect]);

  // ───────── pointer: draw / pan ─────────
  const [liveStroke, setLiveStroke] = useState<Stroke | null>(null);
  const [cursor, setCursor] = useState<{ x: number; y: number } | null>(null);
  const toFrame = (clientX: number, clientY: number) => {
    const r = boxRef.current!.getBoundingClientRect();
    return screenToFrame(viewRef.current!, live.current.size.w, live.current.size.h, clientX - r.left, clientY - r.top);
  };
  const strokeSize = Math.max(1, (brush.size / 1000) * Math.min(crop.w, crop.h));

  const startDraw = (ev: PointerEvent) => {
    const [x, y] = toFrame(ev.clientX, ev.clientY);
    const s: Stroke = { id: `s${Date.now().toString(36)}`, color: brush.color, size: strokeSize, opacity: brush.opacity, points: [x, y] };
    setLiveStroke(s);
    let lx = ev.clientX;
    let ly = ev.clientY;
    const move = (e: PointerEvent) => {
      if (Math.hypot(e.clientX - lx, e.clientY - ly) < 1.5) return;
      lx = e.clientX;
      ly = e.clientY;
      const [fx2, fy2] = toFrame(e.clientX, e.clientY);
      s.points.push(fx2, fy2);
      setLiveStroke({ ...s });
    };
    const up = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      window.removeEventListener('pointercancel', up);
      setLiveStroke(null);
      useEditor.getState().change((d) => ({ ...d, strokes: [...d.strokes, { ...s, points: [...s.points] }] }));
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
    window.addEventListener('pointercancel', up);
  };

  const startPan = (ev: PointerEvent) => {
    const v0 = viewRef.current;
    if (!v0) return;
    const p0 = { x: ev.clientX, y: ev.clientY };
    const el = boxRef.current;
    el?.classList.add('cursor-grabbing');
    const move = (e: PointerEvent) => {
      const { region: rg, size: sz } = live.current;
      setView(clampView({ scale: v0.scale, cx: v0.cx - (e.clientX - p0.x) / v0.scale, cy: v0.cy - (e.clientY - p0.y) / v0.scale }, rg, sz.w, sz.h));
    };
    const up = () => {
      el?.classList.remove('cursor-grabbing');
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };

  // wheel / pinch zoom around the cursor
  useEffect(() => {
    const el = boxRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      const st = useEditor.getState();
      if (st.tab === 'crop' || !viewRef.current) return;
      e.preventDefault();
      const { region: rg, size: sz, pad: pd } = live.current;
      const v = viewRef.current;
      const fit = fitView(rg, sz.w, sz.h, pd).scale;
      const r = el.getBoundingClientRect();
      const mx = e.clientX - r.left;
      const my = e.clientY - r.top;
      const s = clamp(v.scale * Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.0018)), fit, Math.max(fit, 8));
      const [fx2, fy2] = screenToFrame(v, sz.w, sz.h, mx, my);
      anim.current?.stop();
      setView(clampView({ scale: s, cx: fx2 - (mx - sz.w / 2) / s, cy: fy2 - (my - sz.h / 2) / s }, rg, sz.w, sz.h));
      st.setZoom(s <= fit * 1.0001 ? 'fit' : s);
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  }, [setView]);

  const ratio = aspectRatio(doc.geo.aspect, frame.w, frame.h);
  const shadowRect = view && size.w ? frameToScreen(view, size.w, size.h, region.x, region.y) : null;
  const thumb = thumbhashUrl(env.item.thumbhash);
  const showLayers = !compare;
  const snapTol = view ? 8 / view.scale : 0;

  return (
    <div
      ref={boxRef}
      className={cn('relative min-h-0 flex-1 touch-none overflow-hidden select-none', drawMode ? 'cursor-none' : zoom !== 'fit' && !cropMode && 'cursor-grab')}
      onPointerMove={(e) => {
        if (!drawMode) return;
        const r = e.currentTarget.getBoundingClientRect();
        setCursor({ x: e.clientX - r.left, y: e.clientY - r.top });
      }}
      onPointerLeave={() => setCursor(null)}
    >
      {/* soft stage lighting */}
      <div className="pointer-events-none absolute inset-0 bg-[radial-gradient(60%_60%_at_50%_45%,rgb(255_255_255/0.035),transparent_70%)]" />
      <div ref={spinRef} className="absolute inset-0">
        {shadowRect && view && (
          <div
            className="pointer-events-none absolute shadow-[0_30px_90px_-20px_rgb(0_0_0/0.85),0_0_0_1px_rgb(255_255_255/0.04)]"
            style={{ left: shadowRect[0], top: shadowRect[1], width: region.w * view.scale, height: region.h * view.scale }}
          />
        )}
        <motion.div ref={glHost} className="absolute inset-0" initial={false} animate={{ opacity: ready ? 1 : 0 }} transition={{ duration: 0.5 }} />

        {/* placeholder while the full-size image loads */}
        <AnimatePresence>
          {!ready && view && shadowRect && (
            <motion.div
              key="ph"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0, transition: { duration: 0.6 } }}
              className="pointer-events-none absolute overflow-hidden"
              style={{ left: shadowRect[0], top: shadowRect[1], width: region.w * view.scale, height: region.h * view.scale }}
            >
              {thumb && <img src={thumb} alt="" className="absolute inset-0 size-full object-cover" />}
              <img src={backend.thumbUrl(env.item.id, 1600)} alt="" className="absolute inset-0 size-full object-cover" />
              <div className="shimmer absolute inset-0 opacity-25 mix-blend-overlay" />
            </motion.div>
          )}
        </AnimatePresence>

        {view && size.w > 0 && (
          <Stage
            width={size.w}
            height={size.h}
            className="absolute inset-0"
            onPointerDown={(e) => {
              if (cropMode || compare) return;
              if (drawMode) {
                if (e.evt.button === 0) startDraw(e.evt);
                return;
              }
              if (e.target === e.target.getStage()) {
                useEditor.getState().select(null);
                if (zoom !== 'fit') startPan(e.evt);
              }
            }}
          >
            <Layer>
              <Group
                x={size.w / 2 - view.cx * view.scale}
                y={size.h / 2 - view.cy * view.scale}
                scaleX={view.scale}
                scaleY={view.scale}
                clipX={(cropMode ? fullFrame : crop).x}
                clipY={(cropMode ? fullFrame : crop).y}
                clipWidth={(cropMode ? fullFrame : crop).w}
                clipHeight={(cropMode ? fullFrame : crop).h}
                visible={showLayers && ready}
              >
                <Shape
                  listening={false}
                  sceneFunc={(ctx) => {
                    for (const s of doc.strokes) drawStroke(ctx._context, s);
                    if (liveStroke) drawStroke(ctx._context, liveStroke);
                  }}
                />
                {doc.layers.map((l) => (
                  <TextNode
                    key={l.id}
                    layer={l}
                    interactive={interactive}
                    fontsVersion={fontsVersion}
                    snap={{ x: crop.x + crop.w / 2, y: crop.y + crop.h / 2, tol: snapTol }}
                    onSelect={onSelect}
                    onChange={onLayerChange}
                    onDblClick={onDblClick}
                    onGuides={setGuides}
                    nodeRef={registerNode}
                  />
                ))}
                {guides.x && <Line points={[crop.x + crop.w / 2, crop.y, crop.x + crop.w / 2, crop.y + crop.h]} stroke={accent} strokeWidth={1.25 / view.scale} dash={[6 / view.scale, 5 / view.scale]} listening={false} />}
                {guides.y && <Line points={[crop.x, crop.y + crop.h / 2, crop.x + crop.w, crop.y + crop.h / 2]} stroke={accent} strokeWidth={1.25 / view.scale} dash={[6 / view.scale, 5 / view.scale]} listening={false} />}
              </Group>
              {interactive && <LayerTransformer node={selNode} accent={accent} />}
            </Layer>
          </Stage>
        )}

        {cropMode && view && ready && (
          <CropOverlay
            view={view}
            stageW={size.w}
            stageH={size.h}
            frameW={frame.w}
            frameH={frame.h}
            crop={doc.geo.crop}
            ratio={ratio}
            onChange={(c) => useEditor.getState().change((d) => ({ ...d, geo: { ...d.geo, crop: c } }), 'crop.rect')}
          />
        )}
      </div>

      {/* brush cursor */}
      {drawMode && cursor && view && (
        <div
          className="pointer-events-none absolute rounded-full border-2 border-white mix-blend-difference"
          style={{
            left: cursor.x,
            top: cursor.y,
            width: Math.max(6, strokeSize * view.scale),
            height: Math.max(6, strokeSize * view.scale),
            transform: 'translate(-50%,-50%)',
          }}
        />
      )}
    </div>
  );
}
