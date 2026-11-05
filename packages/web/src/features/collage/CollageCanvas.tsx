import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { AnimatePresence, motion } from 'motion/react';
import type Konva from 'konva';
import { Group, Layer, Line, Stage } from 'react-konva';
import { Crosshair, ImagePlus, Move, Plus, RefreshCw, Trash2, ZoomIn } from 'lucide-react';
import type { MediaItem } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { useFonts } from '@/lib/fonts';
import type { TextLayer } from '@/lib/text';
import { Slider } from '@/components/ui';
import { LayerTransformer, TextNode, useAccent, useFontsVersion } from '@/features/photo-editor/konva/TextNode';
import { coverRect, layoutCells, placeImage, type CellGeom } from './geometry';
import { cssGradient, designSize, emptySlot, GRADIENTS, type Background, type Slot } from './model';
import { useCollage, useCollageDoc } from './store';

export const PHOTO_MIME = 'application/x-peek-photo';
const spring = { type: 'spring' as const, stiffness: 300, damping: 30 };
const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

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

function BackgroundView({ bg, s, W, H, firstId }: { bg: Background; s: number; W: number; H: number; firstId: string | null }) {
  const backend = useBackend();
  const dims = useCollage((st) => (firstId ? st.dims[firstId] : undefined));
  const key = bg.kind === 'solid' ? bg.color : bg.kind === 'gradient' ? bg.id : `blur-${firstId}`;
  let content: React.ReactNode = null;
  if (bg.kind === 'solid') content = <div className="absolute inset-0" style={{ background: bg.color }} />;
  else if (bg.kind === 'gradient') content = <div className="absolute inset-0" style={{ background: cssGradient(GRADIENTS.find((g) => g.id === bg.id) ?? GRADIENTS[0]) }} />;
  else {
    const r = dims ? coverRect(W * s, H * s, dims.w, dims.h, 1.12) : null;
    content = (
      <div className="absolute inset-0 overflow-hidden bg-[#111]">
        {firstId && r && (
          <img src={backend.thumbUrl(firstId, 1600)} alt="" draggable={false} className="absolute max-w-none" style={{ left: r.x, top: r.y, width: r.w, height: r.h, filter: `blur(${bg.amount * s}px)` }} />
        )}
        <div className="absolute inset-0 bg-black/[0.12]" />
      </div>
    );
  }
  return (
    <AnimatePresence initial={false}>
      <motion.div key={key} className="absolute inset-0" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} transition={{ duration: 0.35 }}>
        {content}
      </motion.div>
    </AnimatePresence>
  );
}

function CellView({
  c,
  slot,
  item,
  s,
  selected,
  dropping,
  panning,
  onPointerDown,
  onGripDown,
  onPick,
  onDropPhoto,
}: {
  c: CellGeom;
  slot: Slot;
  item: MediaItem | undefined;
  s: number;
  selected: boolean;
  dropping: boolean;
  panning: boolean;
  onPointerDown: (e: React.PointerEvent, c: CellGeom) => void;
  onGripDown: (e: React.PointerEvent, c: CellGeom) => void;
  onPick: (i: number) => void;
  onDropPhoto: (i: number, id: string) => void;
}) {
  const t = useT();
  const backend = useBackend();
  const dims = useCollage((st) => (item ? st.dims[item.id] : undefined));
  const setDims = useCollage((st) => st.setDims);
  const [dragOver, setDragOver] = useState(false);
  const img = item ? (dims ?? { w: item.width ?? 1000, h: item.height ?? 1000 }) : null;
  const r = img ? placeImage(c.inner, img, slot) : null;

  return (
    <motion.div
      data-cell={c.index}
      className={cn('group absolute', item ? 'cursor-grab active:cursor-grabbing' : 'cursor-pointer')}
      initial={{ left: c.x * s, top: c.y * s, width: c.w * s, height: c.h * s, rotate: c.rot, opacity: 0, scale: 0.85 }}
      animate={{ left: c.x * s, top: c.y * s, width: c.w * s, height: c.h * s, rotate: c.rot, opacity: 1, scale: 1 }}
      exit={{ opacity: 0, scale: 0.8 }}
      transition={spring}
      style={{
        zIndex: c.free ? 10 + c.index + (selected ? 20 : 0) : 1,
        borderRadius: c.radius * s,
        background: c.free ? '#fbfbf8' : undefined,
        boxShadow: c.free ? `0 ${8 * s}px ${24 * s}px rgba(0,0,0,0.35)` : undefined,
      }}
      onPointerDown={(e) => onPointerDown(e, c)}
      onDoubleClick={() => onPick(c.index)}
      onDragOver={(e) => {
        if (!e.dataTransfer.types.includes(PHOTO_MIME)) return;
        e.preventDefault();
        setDragOver(true);
      }}
      onDragLeave={() => setDragOver(false)}
      onDrop={(e) => {
        const id = e.dataTransfer.getData(PHOTO_MIME);
        setDragOver(false);
        if (!id) return;
        e.preventDefault();
        onDropPhoto(c.index, id);
      }}
    >
      <div
        className="absolute overflow-hidden"
        style={{ left: c.inner.x * s, top: c.inner.y * s, width: c.inner.w * s, height: c.inner.h * s, borderRadius: c.innerRadius * s, background: item ? '#1a1a22' : undefined }}
      >
        <AnimatePresence initial={false}>
          {item && r ? (
            <motion.img
              key={item.id}
              src={backend.thumbUrl(item.id, 1600)}
              alt=""
              draggable={false}
              className="absolute max-w-none select-none"
              initial={{ opacity: 0, scale: 1.18, left: r.x * s, top: r.y * s, width: r.w * s, height: r.h * s }}
              animate={{ opacity: 1, scale: 1, left: r.x * s, top: r.y * s, width: r.w * s, height: r.h * s }}
              exit={{ opacity: 0, scale: 0.92, transition: { duration: 0.25 } }}
              transition={panning ? { duration: 0 } : spring}
              onLoad={(e) => setDims(item.id, e.currentTarget.naturalWidth, e.currentTarget.naturalHeight)}
            />
          ) : (
            <motion.button
              key="empty"
              type="button"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              onClick={() => onPick(c.index)}
              className="absolute inset-0 flex flex-col items-center justify-center gap-1.5 border-2 border-dashed border-black/15 bg-black/[0.06] text-[11px] font-medium text-black/45 transition-colors hover:bg-black/10"
              style={{ borderRadius: c.innerRadius * s }}
            >
              <Plus className="size-6" />
              {c.inner.w * s > 110 && t('collage.cell.empty')}
            </motion.button>
          )}
        </AnimatePresence>
        <div
          className={cn(
            'pointer-events-none absolute inset-0 transition-[box-shadow,background] duration-200',
            selected && 'shadow-[inset_0_0_0_3px_var(--accent)]',
            (dropping || dragOver) && 'bg-[color-mix(in_oklab,var(--accent)_28%,transparent)] shadow-[inset_0_0_0_3px_var(--accent)]',
          )}
          style={{ borderRadius: c.innerRadius * s }}
        />
      </div>
      {item && (
        <button
          type="button"
          title={t('collage.swap')}
          aria-label={t('collage.swap')}
          onPointerDown={(e) => onGripDown(e, c)}
          className={cn(
            'absolute grid size-8 cursor-move touch-none place-items-center rounded-full bg-black/55 text-white opacity-0 shadow-lg backdrop-blur transition-[opacity,transform] group-hover:opacity-100 hover:scale-110',
            selected && 'opacity-100',
          )}
          style={{ left: c.inner.x * s + 8, top: c.inner.y * s + 8 }}
        >
          <Move className="size-4" />
        </button>
      )}
    </motion.div>
  );
}

/** Interactive collage preview: pan/zoom inside cells, drag-to-swap, replace, text layers on top. */
export function CollageCanvas({ onPick }: { onPick: (slot: number) => void }) {
  const t = useT();
  const backend = useBackend();
  const accent = useAccent();
  const doc = useCollageDoc();
  const photos = useCollage((st) => st.photos);
  const cellSel = useCollage((st) => st.cell);
  const textId = useCollage((st) => st.textId);
  const outerRef = useRef<HTMLDivElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const size = useSize(outerRef);
  const { W, H } = designSize(doc.aspect);
  const narrow = size.w < 640;
  const padX = narrow ? 12 : 32;
  const padTop = narrow ? 12 : 28;
  const padBottom = narrow ? 64 : 76;
  const s = size.w ? Math.max(0.05, Math.min((size.w - padX * 2) / W, (size.h - padTop - padBottom) / H)) : 0;
  const cells = useMemo(() => layoutCells(doc), [doc]);
  const byId = useMemo(() => new Map(photos.map((p) => [p.id, p])), [photos]);
  const rootLeft = (size.w - W * s) / 2;
  const rootTop = padTop + (size.h - padTop - padBottom - H * s) / 2;

  const [panning, setPanning] = useState<number | null>(null);
  const [drag, setDrag] = useState<{ from: number; id: string; x: number; y: number; over: number | null; w: number } | null>(null);

  const change = useCollage((st) => st.change);
  const selectCell = useCollage((st) => st.selectCell);
  const selectText = useCollage((st) => st.selectText);

  const setSlot = useCallback((i: number, patch: Partial<Slot>, key?: string) => {
    useCollage.getState().change((d) => ({ ...d, slots: d.slots.map((sl, j) => (j === i ? { ...sl, ...patch } : sl)) }), key);
  }, []);

  // ───────── pan inside a cell ─────────
  const onCellDown = (e: React.PointerEvent, c: CellGeom) => {
    if (e.button !== 0) return;
    selectCell(c.index);
    const slot = doc.slots[c.index];
    const item = slot?.id ? byId.get(slot.id) : undefined;
    if (!item) return;
    const st = useCollage.getState();
    const img = st.dims[item.id] ?? { w: item.width ?? 1000, h: item.height ?? 1000 };
    const r = placeImage(c.inner, img, slot);
    const ovx = r.w - c.inner.w;
    const ovy = r.h - c.inner.h;
    const p0 = { x: e.clientX, y: e.clientY };
    const rad = (-c.rot * Math.PI) / 180;
    setPanning(c.index);
    const move = (ev: PointerEvent) => {
      // pointer delta in the cell's (possibly rotated) frame, in design units
      const dx0 = (ev.clientX - p0.x) / s;
      const dy0 = (ev.clientY - p0.y) / s;
      const dx = dx0 * Math.cos(rad) - dy0 * Math.sin(rad);
      const dy = dx0 * Math.sin(rad) + dy0 * Math.cos(rad);
      setSlot(c.index, { px: ovx > 0.5 ? clamp(slot.px + (2 * dx) / ovx, -1, 1) : 0, py: ovy > 0.5 ? clamp(slot.py + (2 * dy) / ovy, -1, 1) : 0 }, `pan.${c.index}`);
    };
    const up = () => {
      setPanning(null);
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      window.removeEventListener('pointercancel', up);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
    window.addEventListener('pointercancel', up);
  };

  // ───────── drag a photo onto another cell to swap ─────────
  const cellAt = (x: number, y: number) => {
    const el = document.elementFromPoint(x, y)?.closest('[data-cell]') as HTMLElement | null;
    return el ? Number(el.dataset.cell) : null;
  };
  const onGripDown = (e: React.PointerEvent, c: CellGeom) => {
    e.preventDefault();
    e.stopPropagation();
    const id = doc.slots[c.index]?.id;
    if (!id) return;
    selectCell(c.index);
    const w = Math.min(160, Math.max(90, c.w * s * 0.5));
    setDrag({ from: c.index, id, x: e.clientX, y: e.clientY, over: null, w });
    const move = (ev: PointerEvent) => {
      const over = cellAt(ev.clientX, ev.clientY);
      setDrag((d) => (d ? { ...d, x: ev.clientX, y: ev.clientY, over: over === d.from ? null : over } : d));
    };
    const up = (ev: PointerEvent) => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      const over = cellAt(ev.clientX, ev.clientY);
      setDrag(null);
      if (over === null || over === c.index) return;
      change((d) => {
        const slots = [...d.slots];
        [slots[c.index], slots[over]] = [slots[over], slots[c.index]];
        return { ...d, slots };
      });
      selectCell(over);
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  };

  const onDropPhoto = (i: number, id: string) => {
    change((d) => {
      const slots = [...d.slots];
      const prev = slots.findIndex((sl, j) => j < d.count && sl.id === id);
      if (prev >= 0 && prev !== i) slots[prev] = { ...slots[i] }; // swap when the photo is already used
      slots[i] = emptySlot(id);
      return { ...d, slots };
    });
    selectCell(i);
  };

  // wheel → zoom the photo under the cursor
  useEffect(() => {
    const el = rootRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      const cellEl = (e.target as HTMLElement).closest?.('[data-cell]') as HTMLElement | null;
      if (!cellEl) return;
      const i = Number(cellEl.dataset.cell);
      const d = useCollage.getState().h.doc;
      const slot = d.slots[i];
      if (!slot?.id) return;
      e.preventDefault();
      const zoom = clamp(slot.zoom * Math.exp(-e.deltaY * 0.0022), 1, 4);
      setSlot(i, { zoom }, `zoom.${i}`);
      useCollage.getState().selectCell(i);
    };
    el.addEventListener('wheel', onWheel, { passive: false });
    return () => el.removeEventListener('wheel', onWheel);
  }, [setSlot, s > 0]);

  // ───────── text layers (Konva) ─────────
  const stageRef = useRef<Konva.Stage>(null);
  useFonts(); // registers uploaded fonts (FontFace) so canvas text can use them
  const fontsVersion = useFontsVersion(doc.layers);
  const nodes = useRef(new Map<string, Konva.Shape>());
  const [selNode, setSelNode] = useState<Konva.Shape | null>(null);
  const [hot, setHot] = useState(false);
  const [guides, setGuides] = useState({ x: false, y: false });
  const registerNode = useCallback((id: string, n: Konva.Shape | null) => {
    if (n) nodes.current.set(id, n);
    else nodes.current.delete(id);
  }, []);
  useEffect(() => setSelNode(textId ? (nodes.current.get(textId) ?? null) : null), [textId, doc.layers]);
  const onLayerChange = useCallback((id: string, patch: Partial<TextLayer>) => {
    useCollage.getState().change((d) => ({ ...d, layers: d.layers.map((l) => (l.id === id ? { ...l, ...patch } : l)) }));
  }, []);
  const onTextSelect = useCallback((id: string) => {
    const st = useCollage.getState();
    st.selectText(id);
    if (st.tab !== 'text') st.setTab('text');
  }, []);
  const onTextDbl = useCallback(
    (id: string) => {
      onTextSelect(id);
      requestAnimationFrame(() => {
        const ta = document.querySelector<HTMLTextAreaElement>('[data-text-panel] textarea');
        ta?.focus();
        ta?.select();
      });
    },
    [onTextSelect],
  );

  // Konva only receives pointer events while hovering text or its transformer
  const trackHot = (e: React.PointerEvent) => {
    const st = stageRef.current;
    if (!st || e.buttons) return;
    const rect = st.container().getBoundingClientRect();
    setHot(!!st.getIntersection({ x: e.clientX - rect.left, y: e.clientY - rect.top }));
  };

  const selCell = cellSel !== null ? cells.find((c) => c.index === cellSel) : undefined;
  const selSlot = selCell ? doc.slots[selCell.index] : undefined;
  const firstId = cells.map((c) => doc.slots[c.index]?.id).find(Boolean) ?? photos[0]?.id ?? null;

  return (
    <div
      ref={outerRef}
      className="relative min-h-0 flex-1 overflow-hidden"
      onPointerDown={(e) => {
        if (e.target === e.currentTarget) {
          selectCell(null);
          selectText(null);
        }
      }}
    >
      {s > 0 && (
        <motion.div
          ref={rootRef}
          className="absolute overflow-hidden shadow-[0_30px_80px_-24px_rgb(0_0_0/0.8),0_0_0_1px_rgb(255_255_255/0.06)]"
          initial={false}
          animate={{ left: rootLeft, top: rootTop, width: W * s, height: H * s }}
          transition={spring}
          onPointerMove={trackHot}
          onPointerDown={(e) => {
            if (!(e.target as HTMLElement).closest('[data-cell]') && !hot) selectCell(null);
            if (!hot) selectText(null);
          }}
        >
          <BackgroundView bg={doc.bg} s={s} W={W} H={H} firstId={firstId} />
          <AnimatePresence>
            {cells.map((c) => {
              const slot = doc.slots[c.index] ?? emptySlot();
              return (
                <CellView
                  key={c.index}
                  c={c}
                  slot={slot}
                  item={slot.id ? byId.get(slot.id) : undefined}
                  s={s}
                  selected={cellSel === c.index}
                  dropping={drag?.over === c.index}
                  panning={panning === c.index}
                  onPointerDown={onCellDown}
                  onGripDown={onGripDown}
                  onPick={onPick}
                  onDropPhoto={onDropPhoto}
                />
              );
            })}
          </AnimatePresence>
          <Stage
            ref={stageRef}
            width={W * s}
            height={H * s}
            className="absolute inset-0"
            style={{ zIndex: 60, pointerEvents: hot ? 'auto' : 'none' }}
            onPointerLeave={() => setHot(false)}
          >
            <Layer>
              <Group scaleX={s} scaleY={s}>
                {doc.layers.map((l) => (
                  <TextNode
                    key={l.id}
                    layer={l}
                    interactive
                    fontsVersion={fontsVersion}
                    snap={{ x: W / 2, y: H / 2, tol: 8 / s }}
                    onSelect={onTextSelect}
                    onChange={onLayerChange}
                    onDblClick={onTextDbl}
                    onGuides={setGuides}
                    nodeRef={registerNode}
                  />
                ))}
                {guides.x && <Line points={[W / 2, 0, W / 2, H]} stroke={accent} strokeWidth={1.25 / s} dash={[6 / s, 5 / s]} listening={false} />}
                {guides.y && <Line points={[0, H / 2, W, H / 2]} stroke={accent} strokeWidth={1.25 / s} dash={[6 / s, 5 / s]} listening={false} />}
              </Group>
              <LayerTransformer node={selNode} accent={accent} />
            </Layer>
          </Stage>
        </motion.div>
      )}

      {/* floating toolbar for the selected cell */}
      <AnimatePresence>
        {selCell && selSlot?.id && s > 0 && !drag && (
          <motion.div
            key="celltools"
            initial={{ opacity: 0, y: 8, scale: 0.94 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 6, scale: 0.96 }}
            transition={{ type: 'spring', stiffness: 520, damping: 34 }}
            className="glass absolute z-[70] flex items-center gap-1 rounded-2xl border border-line p-1.5 shadow-soft"
            style={{
              left: clamp(rootLeft + (selCell.x + selCell.w / 2) * s - 170, 8, Math.max(8, size.w - 348)),
              top: Math.min(size.h - 56, rootTop + H * s + 12),
            }}
          >
            <span className="grid size-8 place-items-center text-faint">
              <ZoomIn className="size-4" />
            </span>
            <Slider className="w-28" value={Math.round(selSlot.zoom * 100)} min={100} max={400} center={100} onChange={(v) => setSlot(selCell.index, { zoom: v / 100 }, `zoom.${selCell.index}`)} />
            <div className="mx-1 h-5 w-px bg-line" />
            <ToolBtn label={t('collage.cell.reset')} onClick={() => setSlot(selCell.index, { zoom: 1, px: 0, py: 0 })}>
              <Crosshair />
            </ToolBtn>
            <ToolBtn label={t('collage.cell.replace')} onClick={() => onPick(selCell.index)}>
              <RefreshCw />
            </ToolBtn>
            <ToolBtn label={t('collage.cell.clear')} onClick={() => setSlot(selCell.index, emptySlot())} danger>
              <Trash2 />
            </ToolBtn>
          </motion.div>
        )}
      </AnimatePresence>

      {!selCell && s > 0 && photos.length > 0 && (
        <div className="pointer-events-none absolute inset-x-0 hidden justify-center text-[11px] text-faint md:flex" style={{ top: rootTop + H * s + 18 }}>
          {t('collage.cell.hint')}
        </div>
      )}

      {/* swap ghost */}
      <AnimatePresence>
        {drag && (
          <motion.div
            key="ghost"
            className="pointer-events-none fixed z-[95] overflow-hidden rounded-2xl border-2 border-white shadow-[0_24px_60px_-12px_rgb(0_0_0/0.7)]"
            initial={{ opacity: 0, scale: 0.6, rotate: 0 }}
            animate={{ opacity: 0.95, scale: 1, rotate: drag.over !== null ? 0 : 4 }}
            exit={{ opacity: 0, scale: 0.5 }}
            transition={{ type: 'spring', stiffness: 500, damping: 30 }}
            style={{ left: drag.x - drag.w / 2, top: drag.y - drag.w / 2, width: drag.w, height: drag.w }}
          >
            <img src={backend.thumbUrl(drag.id, 320)} alt="" className="size-full object-cover" />
            <div className="absolute inset-0 grid place-items-center bg-black/20 text-white">
              <ImagePlus className="size-6 drop-shadow" />
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

function ToolBtn({ label, onClick, children, danger }: { label: string; onClick: () => void; children: React.ReactNode; danger?: boolean }) {
  return (
    <motion.button
      type="button"
      title={label}
      aria-label={label}
      whileTap={{ scale: 0.88 }}
      onClick={onClick}
      className={cn('grid size-8 place-items-center rounded-xl text-dim transition-colors hover:bg-surface-3 hover:text-fg [&>svg]:size-4', danger && 'hover:!text-danger')}
    >
      {children}
    </motion.button>
  );
}
