import { useEffect, useRef, useState } from 'react';
import { AnimatePresence, motion } from 'motion/react';
import { Archive, Clock3, Copy, FolderInput, Link2, Package, UploadCloud } from 'lucide-react';
import { useBackend, useInfo } from '@/backend';
import { desktop } from '@/backend/desktop';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { useLocation } from '@/store/location';
import { usePrompt } from '@/store/uploads';
import { Checkbox, Modal } from '@/components/ui';
import { FolderPicker } from '@/features/gallery/FolderPicker';
import { addFiles } from './actions';
import { collectFromDataTransfer } from './collect';
import { openMegaFrom } from '@/features/remote/RemoteHost';

function hasFiles(e: DragEvent) {
  return !!e.dataTransfer && Array.from(e.dataTransfer.types).includes('Files');
}

/** a dragged link / text without files */
function hasUrl(e: DragEvent) {
  const types = e.dataTransfer ? Array.from(e.dataTransfer.types) : [];
  return !types.includes('Files') && (types.includes('text/uri-list') || types.includes('text/plain'));
}

function editable(t: EventTarget | null) {
  const el = t as HTMLElement | null;
  return !!el?.tagName && (/^(input|textarea)$/i.test(el.tagName) || el.isContentEditable);
}

interface HoverTarget {
  folderId: string;
  dir: string;
  label: string;
  el: HTMLElement | null;
}

/** Global drag & drop: animated full-screen overlay + per-folder drop targets (data-drop-folder). */
export function DropZone() {
  const t = useT();
  const backend = useBackend();
  const info = useInfo();
  const { target, targetLabel } = useLocation();
  const [active, setActive] = useState(false);
  const [hover, setHover] = useState<HoverTarget | null>(null);
  const [pos, setPos] = useState({ x: 0, y: 0 });
  const depth = useRef(0);
  const lastEl = useRef<HTMLElement | null>(null);
  const boxRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const findTarget = (e: DragEvent): HoverTarget | null => {
      const el = (e.target as HTMLElement)?.closest?.('[data-drop-folder]') as HTMLElement | null;
      if (!el || !el.dataset.dropFolder) return null;
      return { folderId: el.dataset.dropFolder, dir: el.dataset.dropDir ?? '', label: el.dataset.dropLabel ?? '', el };
    };
    const onEnter = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      depth.current++;
      setActive(true);
    };
    const onOver = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      e.dataTransfer!.dropEffect = 'copy';
      setPos({ x: e.clientX, y: e.clientY });
      const h = findTarget(e);
      if (lastEl.current !== h?.el) {
        lastEl.current?.classList.remove('drop-hover');
        h?.el?.classList.add('drop-hover');
        lastEl.current = h?.el ?? null;
        setHover(h);
      }
    };
    const onLeave = (e: DragEvent) => {
      if (!hasFiles(e)) return;
      depth.current = Math.max(0, depth.current - 1);
      if (depth.current === 0) {
        setActive(false);
        lastEl.current?.classList.remove('drop-hover');
        lastEl.current = null;
        setHover(null);
      }
    };
    const onDrop = async (e: DragEvent) => {
      if (!hasFiles(e)) return;
      e.preventDefault();
      depth.current = 0;
      setActive(false);
      const h = findTarget(e);
      lastEl.current?.classList.remove('drop-hover');
      lastEl.current = null;
      setHover(null);
      const collected = await collectFromDataTransfer(e.dataTransfer!, desktop ? (f) => desktop!.getPathForFile(f) : undefined);
      const dest = h ? { folderId: h.folderId, dir: h.dir } : useLocation.getState().target;
      void addFiles(backend, collected, dest);
    };
    // a dragged link (e.g. a mega.nz URL from the browser's address bar) → MEGA import dialog
    const onUrlOver = (e: DragEvent) => {
      if (hasUrl(e) && !editable(e.target)) e.preventDefault();
    };
    const onUrlDrop = (e: DragEvent) => {
      if (!hasUrl(e) || editable(e.target)) return;
      const dt = e.dataTransfer!;
      const text = dt.getData('text/uri-list') || dt.getData('text/plain');
      e.preventDefault();
      openMegaFrom(text);
    };
    window.addEventListener('dragenter', onEnter);
    window.addEventListener('dragover', onOver);
    window.addEventListener('dragover', onUrlOver);
    window.addEventListener('dragleave', onLeave);
    window.addEventListener('drop', onDrop);
    window.addEventListener('drop', onUrlDrop);
    return () => {
      window.removeEventListener('dragover', onUrlOver);
      window.removeEventListener('drop', onUrlDrop);
      window.removeEventListener('dragenter', onEnter);
      window.removeEventListener('dragover', onOver);
      window.removeEventListener('dragleave', onLeave);
      window.removeEventListener('drop', onDrop);
    };
  }, [backend]);

  const readonly = info?.caps.readonly;
  const origin = boxRef.current?.getBoundingClientRect() ?? { x: 0, y: 0 };
  const label = hover?.label || (target ? targetLabel : '');

  return (
    <>
      <div ref={boxRef} className="pointer-events-none absolute inset-0" />
      <AnimatePresence>
        {active && (
          <motion.div
            className="pointer-events-none absolute inset-0 z-[70] overflow-hidden"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0, transition: { duration: 0.2 } }}
          >
            <div className="absolute inset-0 bg-[var(--overlay)] opacity-70" />
            {/* ripple that follows the pointer */}
            <motion.div
              className="absolute size-[520px] rounded-full"
              style={{ background: 'radial-gradient(circle, color-mix(in oklab, var(--accent) 35%, transparent), transparent 65%)' }}
              animate={{ x: pos.x - 260 - origin.x, y: pos.y - 260 - origin.y }}
              transition={{ type: 'spring', stiffness: 180, damping: 22, mass: 0.6 }}
            />
            <motion.div
              className={cn('absolute inset-4 rounded-[28px] border-2 border-dashed', hover ? 'border-transparent' : 'border-[color-mix(in_oklab,var(--accent)_70%,transparent)]')}
              initial={{ scale: 1.03 }}
              animate={{ scale: 1 }}
              transition={{ type: 'spring', stiffness: 260, damping: 24 }}
            />
            <motion.div
              initial={{ y: 30, opacity: 0, scale: 0.9 }}
              animate={{ y: 0, opacity: 1, scale: 1 }}
              transition={{ type: 'spring', stiffness: 380, damping: 26, delay: 0.04 }}
              className="absolute inset-x-0 bottom-16 mx-auto flex w-fit flex-col items-center gap-3 rounded-3xl border border-line bg-elev/90 px-8 py-6 text-center shadow-soft"
            >
              <motion.div
                animate={{ y: [0, -8, 0] }}
                transition={{ duration: 1.6, repeat: Infinity, ease: 'easeInOut' }}
                className="grid size-14 place-items-center rounded-2xl bg-accent-gradient text-white shadow-[0_10px_30px_-8px_var(--accent)]"
              >
                <UploadCloud className="size-7" />
              </motion.div>
              <div className="font-display text-xl font-semibold">{readonly ? t('drop.readonly') : t('drop.title')}</div>
              {!readonly && (
                <AnimatePresence mode="wait">
                  <motion.div key={label} initial={{ opacity: 0, y: 4 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -4 }} className="text-sm text-dim">
                    {label ? t('drop.into', { name: label }) : t('drop.noTarget')}
                  </motion.div>
                </AnimatePresence>
              )}
              <div className="text-xs text-faint">{t('drop.hint')}</div>
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>
      <Prompts />
    </>
  );
}

function Choice({ icon, title, hint, onClick, primary }: { icon: React.ReactNode; title: string; hint: string; onClick: () => void; primary?: boolean }) {
  return (
    <motion.button
      type="button"
      whileHover={{ y: -2 }}
      whileTap={{ scale: 0.98 }}
      onClick={onClick}
      className={cn(
        'group flex w-full items-start gap-4 rounded-2xl border p-4 text-left transition-colors',
        primary ? 'border-[color-mix(in_oklab,var(--accent)_45%,transparent)] bg-[color-mix(in_oklab,var(--accent)_10%,transparent)] hover:bg-[color-mix(in_oklab,var(--accent)_16%,transparent)]' : 'border-line bg-surface hover:bg-surface-2',
      )}
    >
      <span className={cn('grid size-11 shrink-0 place-items-center rounded-xl [&>svg]:size-5', primary ? 'bg-accent-gradient text-white' : 'bg-surface-3 text-fg')}>{icon}</span>
      <span className="flex flex-col gap-0.5">
        <span className="font-medium text-fg">{title}</span>
        <span className="text-[13px] text-dim">{hint}</span>
      </span>
    </motion.button>
  );
}

/** Zip / folder-link / destination prompts raised by the upload flow. */
function Prompts() {
  const t = useT();
  const { prompt, done } = usePrompt();
  const [remember, setRemember] = useState(false);
  if (!prompt) return null;
  const finish = (fn: () => void) => {
    fn();
    done();
    setRemember(false);
  };

  if (prompt.kind === 'target') {
    return <FolderPicker open title={t('drop.pickTitle')} onClose={() => finish(() => prompt.resolve(null))} onPick={(tg) => finish(() => prompt.resolve(tg))} />;
  }

  if (prompt.kind === 'link') {
    return (
      <Modal open onClose={() => finish(() => prompt.resolve(null))} title={t('link.title')} width={480}>
        <p className="mb-4 text-sm text-dim">{t('link.body', { name: prompt.names.join(', ') })}</p>
        <div className="flex flex-col gap-2.5 pb-3">
          <Choice primary icon={<Link2 />} title={t('link.link')} hint={t('link.linkHint')} onClick={() => finish(() => prompt.resolve('link'))} />
          <Choice icon={<Copy />} title={t('link.copy')} hint={t('link.copyHint')} onClick={() => finish(() => prompt.resolve('copy'))} />
        </div>
      </Modal>
    );
  }

  return (
    <Modal open onClose={() => finish(() => prompt.resolve(null))} width={480}>
      <div className="flex flex-col items-center pt-4 pb-2 text-center">
        <motion.div
          initial={{ rotate: -12, scale: 0.6, opacity: 0 }}
          animate={{ rotate: 0, scale: 1, opacity: 1 }}
          transition={{ type: 'spring', stiffness: 300, damping: 16 }}
          className="mb-3 grid size-16 place-items-center rounded-2xl bg-accent-gradient text-white shadow-[0_12px_30px_-10px_var(--accent)]"
        >
          <Package className="size-8" />
        </motion.div>
        <h2 className="font-display text-xl font-semibold">{t('zip.title')}</h2>
        <p className="mt-1 text-sm text-dim">{prompt.names.length === 1 ? t('zip.body', { name: prompt.names[0] }) : t('zip.bodyMany', { n: prompt.names.length })}</p>
      </div>
      <div className="flex flex-col gap-2.5 py-3">
        <Choice primary icon={<FolderInput />} title={t('zip.extract')} hint={t('zip.extractHint')} onClick={() => finish(() => prompt.resolve({ mode: 'extract', remember }))} />
        <Choice icon={<Clock3 />} title={t('zip.temp')} hint={t('zip.tempHint')} onClick={() => finish(() => prompt.resolve({ mode: 'temp', remember }))} />
      </div>
      <div className="flex items-center justify-between gap-3 pt-1 pb-4">
        <Checkbox checked={remember} onChange={setRemember} label={t('zip.remember')} hint={t('zip.rememberHint')} />
        <button type="button" onClick={() => finish(() => prompt.resolve({ mode: 'keep', remember: false }))} className="inline-flex shrink-0 items-center gap-1.5 text-xs text-faint hover:text-fg">
          <Archive className="size-3.5" />
          {t('zip.keep')}
        </button>
      </div>
    </Modal>
  );
}
