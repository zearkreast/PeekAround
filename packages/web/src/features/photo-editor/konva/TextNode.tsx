import { useEffect, useMemo, useRef, useState } from 'react';
import type Konva from 'konva';
import { Shape, Transformer } from 'react-konva';
import { drawTextLayer, measureTextLayer, type TextLayer } from '@/lib/text';
import { ensureFont } from '@/lib/fonts';

let measureCtx: CanvasRenderingContext2D | null = null;
export function getMeasureCtx() {
  if (!measureCtx) measureCtx = document.createElement('canvas').getContext('2d')!;
  return measureCtx;
}

/** Size of a text layer's box (incl. background padding) in layer units. */
export function layerBox(l: TextLayer) {
  const m = measureTextLayer(getMeasureCtx(), l, 1);
  const p = l.background ? l.background.padding : 0;
  return { w: Math.max(8, m.width + p * 2), h: Math.max(8, m.height + p * 2) };
}

/**
 * Loads every font used by the layers and bumps a counter when fonts finish loading,
 * so text boxes get re-measured and redrawn with the real glyphs.
 */
export function useFontsVersion(layers: Pick<TextLayer, 'fontFamily' | 'fontWeight' | 'italic'>[]) {
  const [v, setV] = useState(0);
  const key = [...new Set(layers.map((l) => `${l.italic ? 'i' : 'n'}|${l.fontWeight}|${l.fontFamily}`))].sort().join(',');
  useEffect(() => {
    let alive = true;
    const list = key ? key.split(',') : [];
    void Promise.all(
      list.map((k) => {
        const [st, w, ...fam] = k.split('|');
        return ensureFont(fam.join('|'), Number(w), st === 'i');
      }),
    ).then(() => alive && setV((x) => x + 1));
    return () => {
      alive = false;
    };
  }, [key]);
  useEffect(() => {
    const on = () => setV((x) => x + 1);
    document.fonts.addEventListener('loadingdone', on);
    return () => document.fonts.removeEventListener('loadingdone', on);
  }, []);
  return v;
}

/** Scale every size-like property of a layer (used after a Transformer resize). */
export function scaleLayer(l: TextLayer, k: number): Partial<TextLayer> {
  const r = (n: number) => Math.round(n * k * 100) / 100;
  return {
    fontSize: Math.max(4, r(l.fontSize)),
    letterSpacing: r(l.letterSpacing),
    stroke: l.stroke ? { ...l.stroke, width: r(l.stroke.width) } : null,
    shadow: l.shadow ? { ...l.shadow, blur: r(l.shadow.blur), x: r(l.shadow.x), y: r(l.shadow.y) } : null,
    background: l.background ? { ...l.background, padding: r(l.background.padding), radius: r(l.background.radius) } : null,
  };
}

export interface SnapTarget {
  x: number;
  y: number;
  /** tolerance in layer units */
  tol: number;
}

/**
 * A text layer drawn by the shared `drawTextLayer` (pixel-identical with the export),
 * draggable and transformable with a Konva Transformer.
 */
export function TextNode({
  layer,
  interactive,
  fontsVersion,
  snap,
  onSelect,
  onChange,
  onDblClick,
  onGuides,
  nodeRef,
}: {
  layer: TextLayer;
  interactive: boolean;
  fontsVersion: number;
  snap?: SnapTarget;
  onSelect: (id: string) => void;
  onChange: (id: string, patch: Partial<TextLayer>, key?: string) => void;
  onDblClick?: (id: string) => void;
  onGuides?: (g: { x: boolean; y: boolean }) => void;
  nodeRef?: (id: string, node: Konva.Shape | null) => void;
}) {
  const box = useMemo(() => layerBox(layer), [layer, fontsVersion]);
  const ref = useRef<Konva.Shape>(null);
  useEffect(() => {
    nodeRef?.(layer.id, ref.current);
    return () => nodeRef?.(layer.id, null);
  }, [layer.id, nodeRef]);

  return (
    <Shape
      ref={ref}
      x={layer.x}
      y={layer.y}
      rotation={layer.rotation}
      width={box.w}
      height={box.h}
      offsetX={box.w / 2}
      offsetY={box.h / 2}
      listening={interactive}
      draggable={interactive}
      sceneFunc={(ctx) => {
        const native = ctx._context;
        const m = native.getTransform();
        const k = Math.hypot(m.a, m.b) || 1;
        native.save();
        // draw in device pixels so shadow blur/offsets (which ignore transforms) match the export
        native.scale(1 / k, 1 / k);
        drawTextLayer(native, { ...layer, x: box.w / 2, y: box.h / 2, rotation: 0 }, k);
        native.restore();
      }}
      hitFunc={(ctx, shape) => {
        ctx.beginPath();
        ctx.rect(0, 0, box.w, box.h);
        ctx.closePath();
        ctx.fillStrokeShape(shape);
      }}
      onPointerDown={(e) => {
        e.cancelBubble = true;
        onSelect(layer.id);
      }}
      onDblClick={() => onDblClick?.(layer.id)}
      onDblTap={() => onDblClick?.(layer.id)}
      onDragStart={() => onSelect(layer.id)}
      onDragMove={(e) => {
        if (!snap) return;
        const n = e.target;
        const sx = Math.abs(n.x() - snap.x) < snap.tol;
        const sy = Math.abs(n.y() - snap.y) < snap.tol;
        if (sx) n.x(snap.x);
        if (sy) n.y(snap.y);
        onGuides?.({ x: sx, y: sy });
      }}
      onDragEnd={(e) => {
        onGuides?.({ x: false, y: false });
        onChange(layer.id, { x: Math.round(e.target.x() * 10) / 10, y: Math.round(e.target.y() * 10) / 10 });
      }}
      onTransformEnd={(e) => {
        const n = e.target;
        const k = n.scaleX();
        n.scaleX(1);
        n.scaleY(1);
        onChange(layer.id, { ...scaleLayer(layer, k), x: n.x(), y: n.y(), rotation: Math.round(n.rotation() * 10) / 10 });
      }}
    />
  );
}

/** Accent-coloured transformer bound to one node. */
export function LayerTransformer({ node, accent }: { node: Konva.Node | null; accent: string }) {
  const ref = useRef<Konva.Transformer>(null);
  useEffect(() => {
    const tr = ref.current;
    if (!tr) return;
    tr.nodes(node ? [node] : []);
    tr.getLayer()?.batchDraw();
  }, [node]);
  return (
    <Transformer
      ref={ref}
      rotateEnabled
      keepRatio
      flipEnabled={false}
      enabledAnchors={['top-left', 'top-right', 'bottom-left', 'bottom-right']}
      anchorSize={11}
      anchorCornerRadius={6}
      anchorStroke={accent}
      anchorFill="#ffffff"
      anchorStrokeWidth={2}
      borderStroke={accent}
      borderStrokeWidth={1.5}
      borderDash={[5, 4]}
      rotateAnchorOffset={26}
      rotationSnaps={[0, 45, 90, 135, 180, 225, 270, 315]}
      rotationSnapTolerance={4}
      padding={6}
      boundBoxFunc={(oldBox, newBox) => (Math.abs(newBox.width) < 14 || Math.abs(newBox.height) < 10 ? oldBox : newBox)}
    />
  );
}

/** Current accent colour from the CSS variables (Konva can't read CSS vars). */
export function useAccent() {
  const [c, setC] = useState('#8b5cf6');
  useEffect(() => {
    const v = getComputedStyle(document.documentElement).getPropertyValue('--accent').trim();
    if (v) setC(v);
  }, []);
  return c;
}
