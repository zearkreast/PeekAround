import { useMemo, useRef, useState } from 'react';
import { AnimatePresence, motion } from 'motion/react';
import { useQueryClient } from '@tanstack/react-query';
import { Download, FolderOpen } from 'lucide-react';
import { useBackend } from '@/backend';
import { useLang, useT } from '@/i18n';
import { formatDate } from '@/lib/format';
import { go } from '@/lib/nav';
import { useLibraries } from '@/lib/queries';
import { folderHref, withView } from '@/lib/routes';
import { toast } from '@/store/toasts';
import { Button, Input, Popover, ProgressBar, Segmented, Slider } from '@/components/ui';
import { FolderPicker } from '@/features/gallery/FolderPicker';
import { DESIGN_LONG, designSize } from './model';
import { renderCollage } from './render';
import { useCollage, useCollageDoc } from './store';

type Format = 'jpg' | 'png' | 'webp';

/** "Save collage" button + popover: name, destination folder, format/quality, progress. */
export function ExportControl() {
  const t = useT();
  const lang = useLang();
  const backend = useBackend();
  const qc = useQueryClient();
  const libs = useLibraries();
  const doc = useCollageDoc();
  const photos = useCollage((s) => s.photos);
  const target = useCollage((s) => s.target);
  const setTarget = useCollage((s) => s.setTarget);
  const anchor = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const [picking, setPicking] = useState(false);
  const [format, setFormat] = useState<Format>('jpg');
  const [quality, setQuality] = useState(92);
  const [name, setName] = useState('');
  const [progress, setProgress] = useState<number | null>(null);

  const defaultName = useMemo(() => t('collage.defaultName', { date: formatDate(Date.now(), lang, { dateStyle: 'medium' }) }), [t, lang]);
  const targetLabel = useMemo(() => {
    if (!target) return null;
    const all = [...(libs.data?.libraries.flatMap((l) => l.folders.map((f) => ({ f, lib: l.name, multi: l.folders.length > 1 }))) ?? []), ...(libs.data?.temp.map((f) => ({ f, lib: f.name, multi: false })) ?? [])];
    const hit = all.find((x) => x.f.id === target.folderId);
    const root = hit ? (hit.multi ? `${hit.lib} · ${hit.f.name}` : hit.lib) : '…';
    return target.dir ? `${root} / ${target.dir}` : root;
  }, [target, libs.data]);
  const { W, H } = designSize(doc.aspect);
  const k = doc.longSide / DESIGN_LONG;
  const filled = doc.slots.slice(0, doc.count).some((s) => s.id);

  const save = async () => {
    if (!target) {
      setPicking(true);
      return;
    }
    setProgress(0.02);
    try {
      const blob = await renderCollage(doc, new Map(photos.map((p) => [p.id, p])), backend, (p) => setProgress(p * 0.9));
      setProgress(0.92);
      const res = await backend.saveEdit(blob, { folderId: target.folderId, dir: target.dir, name: (name.trim() || defaultName).replace(/[\\/:*?"<>|]+/g, '-'), format, quality });
      setProgress(1);
      void qc.invalidateQueries({ queryKey: ['browse'] });
      toast.success(t('collage.saved'), res.item.name, {
        label: t('common.open'),
        run: () => go(folderHref(res.item.folderId, res.item.dir) + (res.id ? withView('', res.id) : '')),
      });
      setTimeout(() => setOpen(false), 350);
    } catch (e) {
      toast.error(t('common.error'), (e as Error).message);
    } finally {
      setTimeout(() => setProgress(null), 400);
    }
  };

  return (
    <>
      <Button ref={anchor} variant="primary" icon={<Download className="size-4" />} disabled={!filled} onClick={() => setOpen((o) => !o)}>
        {t('collage.export')}
      </Button>
      <Popover open={open} onClose={() => progress === null && !picking && setOpen(false)} anchor={anchor} align="end" className="w-80 p-4">
        <div className="flex flex-col gap-4">
          <label className="flex flex-col gap-1.5">
            <span className="text-[13px] font-medium text-dim">{t('collage.fileName')}</span>
            <Input value={name} placeholder={defaultName} onChange={(e) => setName(e.target.value)} />
          </label>
          <div>
            <div className="mb-1.5 text-[13px] font-medium text-dim">{t('collage.saveTo')}</div>
            <button type="button" onClick={() => setPicking(true)} className="flex w-full items-center gap-2.5 rounded-xl border border-line bg-surface px-3 py-2.5 text-left hover:bg-surface-2">
              <FolderOpen className="size-4 shrink-0 text-accent" />
              <span className="min-w-0 flex-1 truncate text-sm">{targetLabel ?? t('collage.noTarget')}</span>
              <span className="text-xs font-medium text-accent">{t('collage.change')}</span>
            </button>
          </div>
          <div>
            <div className="mb-1.5 text-[13px] font-medium text-dim">{t('photo.format')}</div>
            <Segmented
              value={format}
              onChange={setFormat}
              className="w-full [&>button]:flex-1"
              options={[
                { value: 'jpg', label: 'JPG' },
                { value: 'png', label: 'PNG' },
                { value: 'webp', label: 'WebP' },
              ]}
            />
          </div>
          <motion.div initial={false} animate={{ opacity: format === 'png' ? 0.35 : 1 }} className={format === 'png' ? 'pointer-events-none' : undefined}>
            <Slider label={t('photo.quality')} value={quality} min={40} max={100} center={92} onChange={setQuality} />
          </motion.div>
          <div className="text-xs text-faint tabular-nums">{t('collage.size', { w: Math.round(W * k), h: Math.round(H * k) })}</div>
          <AnimatePresence mode="wait" initial={false}>
            {progress !== null ? (
              <motion.div key="p" initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0 }} className="flex flex-col gap-2 py-1">
                <div className="flex items-center justify-between text-xs text-dim">
                  <span>{progress < 0.9 ? t('collage.rendering') : t('common.saving')}</span>
                  <span className="tabular-nums">{Math.round(progress * 100)}%</span>
                </div>
                <ProgressBar value={progress} />
              </motion.div>
            ) : (
              <motion.div key="b" initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0 }}>
                <Button variant="primary" className="w-full" icon={<Download className="size-4" />} onClick={() => void save()}>
                  {t('common.save')}
                </Button>
              </motion.div>
            )}
          </AnimatePresence>
        </div>
      </Popover>
      <FolderPicker
        open={picking}
        title={t('collage.saveTo')}
        onClose={() => setPicking(false)}
        onPick={(p) => {
          setTarget({ folderId: p.folderId, dir: p.dir });
          setPicking(false);
        }}
      />
    </>
  );
}
