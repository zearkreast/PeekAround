import { useEffect, useMemo, useRef, useState } from 'react';
import { AnimatePresence, motion } from 'motion/react';
import { useQueryClient } from '@tanstack/react-query';
import { RotateCcw, Sparkles, Trash2, Type, UploadCloud } from 'lucide-react';
import type { FontInfo } from '@peekaround/shared';
import { extOf, FONT_EXT } from '@peekaround/shared';
import { useBackend, useInfo } from '@/backend';
import { useT, type MessageKey } from '@/i18n';
import { cn } from '@/lib/cn';
import { BUNDLED_FONTS, useFonts, type FontFamily } from '@/lib/fonts';
import { useChrome } from '@/store/chrome';
import { toast } from '@/store/toasts';
import { Badge, Button, ConfirmDialog, IconButton, Input, Slider, Spinner } from '@/components/ui';

const CATEGORY: Record<FontFamily['category'], MessageKey> = {
  sans: 'fonts.cat.sans',
  serif: 'fonts.cat.serif',
  display: 'fonts.cat.display',
  script: 'fonts.cat.script',
  mono: 'fonts.cat.mono',
  custom: 'fonts.custom',
};

const hasFiles = (e: React.DragEvent) => Array.from(e.dataTransfer?.types ?? []).includes('Files');

const listItem = {
  hidden: { opacity: 0, y: 18, scale: 0.98 },
  show: { opacity: 1, y: 0, scale: 1, transition: { type: 'spring' as const, stiffness: 320, damping: 28 } },
};

function DropCard({ active, uploading, onFiles }: { active: boolean; uploading: string[]; onFiles: (f: File[]) => void }) {
  const t = useT();
  const input = useRef<HTMLInputElement>(null);
  return (
    <motion.div
      initial={{ opacity: 0, y: 16 }}
      animate={{ opacity: 1, y: 0, scale: active ? 1.015 : 1 }}
      transition={{ type: 'spring', stiffness: 300, damping: 26, delay: active ? 0 : 0.08 }}
      className={cn(
        'group relative overflow-hidden rounded-3xl border-2 border-dashed p-8 text-center transition-colors sm:p-10',
        active ? 'border-accent bg-[color-mix(in_oklab,var(--accent)_12%,transparent)]' : 'border-line-strong bg-elev hover:border-[color-mix(in_oklab,var(--accent)_50%,transparent)]',
      )}
    >
      <div className={cn('aurora transition-opacity duration-500', active ? 'opacity-90' : 'opacity-0 group-hover:opacity-50')} />
      <div className="relative flex flex-col items-center">
        <motion.div
          animate={active ? { y: [0, -10, 0], scale: 1.1 } : { y: 0, scale: 1 }}
          transition={active ? { duration: 1.1, repeat: Infinity, ease: 'easeInOut' } : { type: 'spring', stiffness: 300, damping: 20 }}
          className="mb-4 grid size-16 place-items-center rounded-2xl bg-accent-gradient text-white shadow-[0_14px_34px_-10px_var(--accent)]"
        >
          <UploadCloud className="size-8" />
        </motion.div>
        <AnimatePresence mode="wait" initial={false}>
          <motion.div key={active ? 'a' : 'b'} initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -6 }} className="font-display text-xl font-semibold">
            {active ? t('fonts.dropHere') : t('fonts.drop')}
          </motion.div>
        </AnimatePresence>
        <div className="mt-1 text-sm text-faint">.ttf · .otf · .woff · .woff2</div>
        <div className="mt-5 flex items-center gap-3 text-sm text-dim">
          <span className="h-px w-10 bg-line" />
          {t('fonts.or')}
          <span className="h-px w-10 bg-line" />
        </div>
        <Button variant="primary" className="mt-4" icon={<UploadCloud className="size-4" />} onClick={() => input.current?.click()}>
          {t('fonts.browse')}
        </Button>
        <input
          ref={input}
          type="file"
          hidden
          multiple
          accept=".ttf,.otf,.woff,.woff2"
          onChange={(e) => {
            onFiles(Array.from(e.target.files ?? []));
            e.target.value = '';
          }}
        />
        <AnimatePresence>
          {uploading.length > 0 && (
            <motion.div initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: 'auto' }} exit={{ opacity: 0, height: 0 }} className="mt-5 flex flex-wrap justify-center gap-2">
              {uploading.map((n) => (
                <motion.span key={n} layout initial={{ scale: 0.8, opacity: 0 }} animate={{ scale: 1, opacity: 1 }} exit={{ scale: 0.8, opacity: 0 }} className="inline-flex items-center gap-2 rounded-full border border-line bg-surface-2 px-3 py-1.5 text-xs">
                  <Spinner className="size-3.5" />
                  {n}
                </motion.span>
              ))}
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </motion.div>
  );
}

function CustomFamily({ family, faces, sample, size, onDelete }: { family: string; faces: FontInfo[]; sample: string; size: number; onDelete: (f: FontInfo) => void }) {
  const t = useT();
  return (
    <motion.section variants={listItem} layout exit={{ opacity: 0, scale: 0.96, transition: { duration: 0.18 } }} className="overflow-hidden rounded-3xl border border-line bg-elev">
      <header className="flex items-center gap-3 border-b border-line px-5 py-3">
        <span className="grid size-8 place-items-center rounded-xl bg-surface-2 text-accent">
          <Type className="size-4" />
        </span>
        <h3 className="min-w-0 flex-1 truncate font-medium">{family}</h3>
        <Badge>{faces.length === 1 ? t('fonts.style1') : t('fonts.styles', { n: faces.length })}</Badge>
      </header>
      <AnimatePresence initial={false}>
        {faces.map((f) => (
          <motion.div
            key={f.id}
            layout
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: 'auto' }}
            exit={{ opacity: 0, height: 0 }}
            className="group flex items-center gap-4 border-b border-line px-5 py-4 last:border-b-0"
          >
            <div className="min-w-0 flex-1">
              <div className="mb-1.5 flex flex-wrap items-center gap-1.5 text-[11px] text-faint">
                <span className="font-medium text-dim">{f.subfamily}</span>
                <span>·</span>
                <span className="tabular-nums">{f.weight}</span>
                {f.style === 'italic' && <span>· italic</span>}
                <span className="rounded-md bg-surface-2 px-1.5 py-px font-mono uppercase">{f.format}</span>
              </div>
              <p className="break-words leading-tight text-fg" style={{ fontFamily: `"${f.family}", "Inter Variable", sans-serif`, fontWeight: f.weight, fontStyle: f.style, fontSize: size }}>
                {sample}
              </p>
            </div>
            <IconButton label={t('common.delete')} onClick={() => onDelete(f)} className="opacity-60 group-hover:opacity-100 hover:!text-danger">
              <Trash2 />
            </IconButton>
          </motion.div>
        ))}
      </AnimatePresence>
    </motion.section>
  );
}

export default function FontsPage() {
  const t = useT();
  const backend = useBackend();
  const info = useInfo();
  const qc = useQueryClient();
  const setChrome = useChrome((s) => s.set);
  const { raw, loading } = useFonts();
  const [sample, setSample] = useState('');
  const [size, setSize] = useState(40);
  const [uploading, setUploading] = useState<string[]>([]);
  const [toDelete, setToDelete] = useState<FontInfo | null>(null);
  const [dragging, setDragging] = useState(false);
  const depth = useRef(0);
  const text = sample || t('fonts.sample');
  const readonly = !!info?.caps.readonly;

  useEffect(() => setChrome({ crumbs: [], title: t('fonts.title'), viewControls: false }), [setChrome, t]);

  const families = useMemo(() => {
    const map = new Map<string, FontInfo[]>();
    for (const f of raw.filter((x) => !x.bundled)) {
      const list = map.get(f.family) ?? [];
      list.push(f);
      map.set(f.family, list);
    }
    for (const l of map.values()) l.sort((a, b) => a.weight - b.weight || a.style.localeCompare(b.style));
    return [...map.entries()].sort((a, b) => a[0].localeCompare(b[0]));
  }, [raw]);

  const upload = async (files: File[]) => {
    if (!files.length) return;
    const ok = files.filter((f) => FONT_EXT.includes(extOf(f.name)));
    for (const f of files.filter((x) => !ok.includes(x))) toast.error(t('fonts.badFile', { name: f.name }));
    if (!ok.length) return;
    setUploading(ok.map((f) => f.name));
    let added = 0;
    for (const f of ok) {
      try {
        await backend.uploadFont(f);
        added++;
      } catch (e) {
        toast.error(t('common.error'), `${f.name}: ${(e as Error).message}`);
      }
      setUploading((u) => u.filter((n) => n !== f.name));
    }
    await qc.invalidateQueries({ queryKey: ['fonts'] });
    if (added) toast.success(added === 1 ? t('fonts.added') : t('fonts.addedMany', { n: added }));
  };

  // the whole page is a font drop target; stop the gallery's global media drop handler
  const dnd = readonly
    ? {}
    : {
        onDragEnter: (e: React.DragEvent) => {
          if (!hasFiles(e)) return;
          e.preventDefault();
          e.stopPropagation();
          depth.current++;
          setDragging(true);
        },
        onDragOver: (e: React.DragEvent) => {
          if (!hasFiles(e)) return;
          e.preventDefault();
          e.stopPropagation();
          e.dataTransfer.dropEffect = 'copy';
        },
        onDragLeave: (e: React.DragEvent) => {
          if (!hasFiles(e)) return;
          e.stopPropagation();
          depth.current = Math.max(0, depth.current - 1);
          if (!depth.current) setDragging(false);
        },
        onDrop: (e: React.DragEvent) => {
          if (!hasFiles(e)) return;
          e.preventDefault();
          e.stopPropagation();
          depth.current = 0;
          setDragging(false);
          void upload(Array.from(e.dataTransfer.files));
        },
      };

  return (
    <div className="relative min-h-[calc(100dvh-var(--topbar-h))]" {...dnd}>
      <div className="mx-auto w-full max-w-4xl px-4 pt-8 pb-24 sm:px-7">
        <motion.header initial={{ opacity: 0, y: 10 }} animate={{ opacity: 1, y: 0 }} className="mb-7 flex items-start gap-4">
          <motion.span
            initial={{ rotate: -20, scale: 0.6 }}
            animate={{ rotate: 0, scale: 1 }}
            transition={{ type: 'spring', stiffness: 300, damping: 14 }}
            className="grid size-14 shrink-0 place-items-center rounded-2xl bg-accent-gradient text-white shadow-[0_12px_30px_-10px_var(--accent)]"
          >
            <Type className="size-7" />
          </motion.span>
          <div className="min-w-0">
            <h1 className="font-display text-3xl font-bold tracking-tight">{t('fonts.title')}</h1>
            <p className="mt-1 text-sm text-dim">{t('fonts.subtitle')}</p>
            <p className="mt-1 flex items-center gap-1.5 text-xs text-faint">
              <Sparkles className="size-3.5 text-accent" />
              {t('fonts.usage')}
            </p>
          </div>
        </motion.header>

        {!readonly && <DropCard active={dragging} uploading={uploading} onFiles={(f) => void upload(f)} />}

        {/* preview controls */}
        <motion.div
          initial={{ opacity: 0, y: 16 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ type: 'spring', stiffness: 300, damping: 28, delay: 0.14 }}
          className="glass sticky top-3 z-20 mt-6 flex flex-col gap-3 rounded-2xl border border-line p-3 shadow-soft sm:flex-row sm:items-center"
        >
          <div className="flex min-w-0 flex-1 items-center gap-1.5">
            <Input value={sample} onChange={(e) => setSample(e.target.value)} placeholder={t('fonts.sample')} aria-label={t('fonts.preview')} className="flex-1" />
            <IconButton label={t('fonts.resetSample')} disabled={!sample} onClick={() => setSample('')}>
              <RotateCcw />
            </IconButton>
          </div>
          <div className="flex items-center gap-3 sm:w-56">
            <span className="shrink-0 text-[13px] text-dim">{t('fonts.size')}</span>
            <Slider value={size} min={16} max={120} center={40} onChange={setSize} className="flex-1" />
            <span className="w-8 text-right text-xs text-faint tabular-nums">{size}</span>
          </div>
        </motion.div>

        {/* custom fonts */}
        <section className="mt-9">
          <div className="mb-3 flex items-center gap-2">
            <h2 className="font-display text-lg font-semibold">{t('fonts.custom')}</h2>
            {families.length > 0 && <Badge>{raw.filter((x) => !x.bundled).length}</Badge>}
          </div>
          {loading ? (
            <div className="grid h-28 place-items-center">
              <Spinner />
            </div>
          ) : families.length === 0 ? (
            <motion.div initial={{ opacity: 0 }} animate={{ opacity: 1 }} className="rounded-3xl border border-dashed border-line-strong px-6 py-10 text-center text-sm text-faint">
              {t('fonts.empty')}
            </motion.div>
          ) : (
            <motion.div className="flex flex-col gap-4" initial="hidden" animate="show" variants={{ show: { transition: { staggerChildren: 0.06 } } }}>
              <AnimatePresence initial={false} mode="popLayout">
                {families.map(([family, faces]) => (
                  <CustomFamily key={family} family={family} faces={faces} sample={text} size={size} onDelete={setToDelete} />
                ))}
              </AnimatePresence>
            </motion.div>
          )}
        </section>

        {/* bundled fonts */}
        <section className="mt-10">
          <div className="mb-3 flex items-center gap-2">
            <h2 className="font-display text-lg font-semibold">{t('fonts.bundled')}</h2>
            <Badge>{BUNDLED_FONTS.length}</Badge>
          </div>
          <motion.div className="grid gap-3 sm:grid-cols-2" initial="hidden" animate="show" variants={{ show: { transition: { staggerChildren: 0.04, delayChildren: 0.15 } } }}>
            {BUNDLED_FONTS.map((f) => (
              <motion.div key={f.family} variants={listItem} whileHover={{ y: -3 }} className="flex flex-col gap-2 overflow-hidden rounded-3xl border border-line bg-elev px-5 py-4">
                <div className="flex items-center gap-2">
                  <span className="min-w-0 flex-1 truncate text-sm font-medium">{f.label}</span>
                  <Badge>{t(CATEGORY[f.category])}</Badge>
                </div>
                <p className="line-clamp-2 break-words leading-tight" style={{ fontFamily: `"${f.family}"`, fontSize: Math.round(size * 0.8) }}>
                  {text}
                </p>
                <div className="flex flex-wrap gap-1 text-[11px] text-faint tabular-nums">
                  {f.weights.map((w) => (
                    <span key={w} className="rounded-md bg-surface-2 px-1.5 py-px">
                      {w}
                    </span>
                  ))}
                  {f.italic && <span className="rounded-md bg-surface-2 px-1.5 py-px italic">italic</span>}
                </div>
              </motion.div>
            ))}
          </motion.div>
        </section>
      </div>

      {/* page-wide drop highlight */}
      <AnimatePresence>
        {dragging && (
          <motion.div
            className="pointer-events-none absolute inset-0 z-40 m-3 rounded-[28px] border-2 border-dashed border-accent bg-[color-mix(in_oklab,var(--accent)_8%,transparent)]"
            initial={{ opacity: 0, scale: 1.02 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0 }}
            transition={{ type: 'spring', stiffness: 300, damping: 26 }}
          />
        )}
      </AnimatePresence>

      <ConfirmDialog
        open={!!toDelete}
        title={t('fonts.deleteConfirm', { name: toDelete ? `${toDelete.family} ${toDelete.subfamily}` : '' })}
        confirmLabel={t('common.delete')}
        cancelLabel={t('common.cancel')}
        danger
        onConfirm={async () => {
          if (!toDelete) return;
          try {
            await backend.deleteFont(toDelete.id);
            await qc.invalidateQueries({ queryKey: ['fonts'] });
            toast.success(t('fonts.deleted'));
          } catch (e) {
            toast.error(t('common.error'), (e as Error).message);
          }
        }}
        onClose={() => setToDelete(null)}
      />
    </div>
  );
}
