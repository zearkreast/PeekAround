import { useState } from 'react';
import { ArrowLeft, ChevronRight, Folder, FolderPlus } from 'lucide-react';
import { motion, AnimatePresence } from 'motion/react';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { LibraryIcon } from '@/lib/icons';
import { useBrowse, useLibraries } from '@/lib/queries';
import { Button, Input, Modal, Spinner } from '@/components/ui';

export interface PickedTarget {
  folderId: string;
  dir: string;
  label: string;
}

/** Navigate libraries → folders → sub folders and pick a destination. */
export function FolderPicker({ open, title, onClose, onPick, confirmLabel }: { open: boolean; title: string; onClose: () => void; onPick: (t: PickedTarget) => void; confirmLabel?: string }) {
  const t = useT();
  const backend = useBackend();
  const libs = useLibraries();
  const [stack, setStack] = useState<PickedTarget[]>([]);
  const [newName, setNewName] = useState<string | null>(null);
  const cur = stack[stack.length - 1] ?? null;
  const browse = useBrowse({ folderId: cur?.folderId, dir: cur?.dir, limit: 1 }, !!cur);

  const reset = () => {
    setStack([]);
    setNewName(null);
  };

  return (
    <Modal
      open={open}
      onClose={() => {
        reset();
        onClose();
      }}
      title={title}
      width={480}
      footer={
        <>
          <Button variant="ghost" onClick={() => (reset(), onClose())}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" disabled={!cur} onClick={() => cur && (onPick(cur), reset())}>
            {confirmLabel ?? t('common.done')}
          </Button>
        </>
      }
    >
      <div className="mb-3 flex items-center gap-2">
        <button type="button" disabled={!stack.length} onClick={() => setStack((s) => s.slice(0, -1))} className="grid size-8 place-items-center rounded-lg text-dim hover:bg-surface-2 disabled:opacity-30">
          <ArrowLeft className="size-4" />
        </button>
        <div className="min-w-0 flex-1 truncate text-sm font-medium">{stack.map((s) => s.label).join(' / ') || t('nav.libraries')}</div>
        {cur && (
          <button type="button" title={t('gallery.newFolder')} onClick={() => setNewName('')} className="grid size-8 place-items-center rounded-lg text-dim hover:bg-surface-2">
            <FolderPlus className="size-4" />
          </button>
        )}
      </div>
      {newName !== null && cur && (
        <form
          className="mb-3 flex gap-2"
          onSubmit={async (e) => {
            e.preventDefault();
            if (!newName.trim()) return;
            const r = await backend.mkdir(cur.folderId, cur.dir, newName.trim());
            setNewName(null);
            setStack((s) => [...s, { folderId: r.folderId, dir: r.dir, label: newName.trim() }]);
          }}
        >
          <Input autoFocus value={newName} onChange={(e) => setNewName(e.target.value)} placeholder={t('gallery.newFolderName')} className="flex-1" />
          <Button type="submit" variant="primary">
            {t('common.create')}
          </Button>
        </form>
      )}
      <div className="h-80 overflow-auto rounded-2xl border border-line bg-surface p-1">
        <AnimatePresence mode="wait" initial={false}>
          <motion.div key={stack.length + (cur?.dir ?? '')} initial={{ opacity: 0, x: 12 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -12 }} transition={{ duration: 0.16 }}>
            {!cur &&
              libs.data?.libraries.flatMap((l) =>
                // remote (MEGA) folders are read-only: never a destination
                l.folders.filter((f) => !f.remote).map((f) => (
                  <Row
                    key={f.id}
                    icon={
                      <span className="grid size-6 place-items-center rounded-md text-white" style={{ background: l.color }}>
                        <LibraryIcon name={l.icon} className="size-3.5" />
                      </span>
                    }
                    label={l.folders.length > 1 ? `${l.name} · ${f.name}` : l.name}
                    onClick={() => setStack([{ folderId: f.id, dir: '', label: l.folders.length > 1 ? f.name : l.name }])}
                  />
                )),
              )}
            {cur && browse.isLoading && (
              <div className="grid h-40 place-items-center">
                <Spinner />
              </div>
            )}
            {cur &&
              browse.first?.folders.map((f) => (
                <Row key={f.dir} icon={<Folder className="size-4 text-accent" />} label={f.name} onClick={() => setStack((s) => [...s, { folderId: cur.folderId, dir: f.dir, label: f.name }])} />
              ))}
          </motion.div>
        </AnimatePresence>
      </div>
    </Modal>
  );
}

function Row({ icon, label, onClick }: { icon: React.ReactNode; label: string; onClick: () => void }) {
  return (
    <button type="button" onClick={onClick} className="flex w-full items-center gap-2.5 rounded-xl px-3 py-2.5 text-left text-sm hover:bg-surface-2">
      {icon}
      <span className="min-w-0 flex-1 truncate">{label}</span>
      <ChevronRight className="size-4 text-faint" />
    </button>
  );
}
