import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router';
import { useQueryClient } from '@tanstack/react-query';
import { AnimatePresence, motion } from 'motion/react';
import { ArrowUp, Cloud, Folder, FolderPlus, HardDrive, Plus, RefreshCw, Trash2, X } from 'lucide-react';
import { useRemote } from '@/features/remote/RemoteHost';
import type { Library } from '@peekaround/shared';
import { useBackend, useInfo } from '@/backend';
import { desktop } from '@/backend/desktop';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { LIBRARY_ICONS } from '@/lib/icons';
import { libraryHref } from '@/lib/routes';
import { ACCENTS } from '@/store/prefs';
import { toast } from '@/store/toasts';
import { Button, ColorSwatches, ConfirmDialog, Field, IconButton, Input, Modal, Spinner } from '@/components/ui';

/** Browse directories on the server to pick a folder (server/Docker mode). */
export function HostFolderPicker({ open, onClose, onPick }: { open: boolean; onClose: () => void; onPick: (path: string) => void }) {
  const t = useT();
  const backend = useBackend();
  const [path, setPath] = useState<string | undefined>(undefined);
  const [data, setData] = useState<{ path: string | null; parent: string | null; dirs: { name: string; path: string }[] } | null>(null);
  const [loading, setLoading] = useState(false);
  const [manual, setManual] = useState('');
  const [newName, setNewName] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setLoading(true);
    backend
      .hostDirs(path)
      .then(setData)
      .catch((e) => toast.error(t('common.error'), (e as Error).message))
      .finally(() => setLoading(false));
  }, [open, path, backend, t]);

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={t('lib.hostBrowser')}
      width={560}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" disabled={!data?.path} onClick={() => data?.path && onPick(data.path)}>
            {t('lib.useThis')}
          </Button>
        </>
      }
    >
      <div className="mb-3 flex items-center gap-2">
        <IconButton label={t('lib.hostUp')} size="sm" disabled={!data?.parent && !data?.path} onClick={() => setPath(data?.parent ?? undefined)}>
          <ArrowUp />
        </IconButton>
        <div className="min-w-0 flex-1 truncate rounded-lg bg-surface px-3 py-1.5 font-mono text-xs text-dim">{data?.path ?? '/'}</div>
        {data?.path && (
          <IconButton label={t('lib.newHostFolder')} size="sm" onClick={() => setNewName('')}>
            <FolderPlus />
          </IconButton>
        )}
      </div>
      {newName !== null && (
        <form
          className="mb-3 flex gap-2"
          onSubmit={async (e) => {
            e.preventDefault();
            if (!data?.path || !newName.trim()) return;
            const r = await backend.hostMkdir(data.path, newName.trim());
            setNewName(null);
            setPath(r.path);
          }}
        >
          <Input autoFocus value={newName} onChange={(e) => setNewName(e.target.value)} placeholder={t('gallery.newFolderName')} className="flex-1" />
          <Button type="submit" variant="primary">
            {t('common.create')}
          </Button>
        </form>
      )}
      <div className="h-72 overflow-auto rounded-2xl border border-line bg-surface p-1">
        {loading ? (
          <div className="grid h-full place-items-center">
            <Spinner />
          </div>
        ) : (
          data?.dirs.map((d) => (
            <button key={d.path} type="button" onDoubleClick={() => setPath(d.path)} onClick={() => setPath(d.path)} className="flex w-full items-center gap-2.5 rounded-xl px-3 py-2 text-left text-sm hover:bg-surface-2">
              {data.path ? <Folder className="size-4 text-accent" /> : <HardDrive className="size-4 text-accent" />}
              <span className="truncate">{d.name}</span>
            </button>
          ))
        )}
      </div>
      <form
        className="mt-3 flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          if (manual.trim()) onPick(manual.trim());
        }}
      >
        <Input value={manual} onChange={(e) => setManual(e.target.value)} placeholder={t('lib.pathHint')} className="flex-1" />
        <Button type="submit">{t('common.add')}</Button>
      </form>
    </Modal>
  );
}

/** Create or edit a library (a named group of folders). */
export function LibraryEditor({ value, onClose }: { value: Library | 'new' | null; onClose: () => void }) {
  const t = useT();
  const backend = useBackend();
  const info = useInfo();
  const qc = useQueryClient();
  const nav = useNavigate();
  const isNew = value === 'new';
  const lib = value && value !== 'new' ? value : null;
  const [name, setName] = useState('');
  const [color, setColor] = useState(ACCENTS[0]);
  const [icon, setIcon] = useState('images');
  const [paths, setPaths] = useState<string[]>([]);
  const [picker, setPicker] = useState(false);
  const [busy, setBusy] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);

  useEffect(() => {
    if (!value) return;
    if (lib) {
      setName(lib.name);
      setColor(lib.color);
      setIcon(lib.icon);
      setPaths([]);
    } else {
      setName('');
      setColor(ACCENTS[Math.floor(Math.random() * ACCENTS.length)]);
      setIcon('images');
      setPaths([]);
    }
  }, [value, lib]);

  const addPaths = (ps: string[]) => {
    setPaths((cur) => [...cur, ...ps.filter((p) => !cur.includes(p))]);
    if (!name && ps[0]) setName(ps[0].split(/[\\/]/).filter(Boolean).pop() ?? '');
  };

  const pickFolder = async () => {
    if (desktop && info?.caps.nativeFolders) addPaths(await desktop.pickFolders());
    else setPicker(true);
  };

  const refresh = () => qc.invalidateQueries({ queryKey: ['libraries'] });

  const save = async () => {
    setBusy(true);
    try {
      if (isNew) {
        const created = await backend.createLibrary({ name: name || 'Library', color, icon, folders: paths });
        toast.success(t('lib.created'));
        nav(libraryHref(created.id));
      } else if (lib) {
        await backend.updateLibrary(lib.id, { name, color, icon });
        for (const p of paths) await backend.addFolder(lib.id, p);
      }
      await refresh();
      onClose();
    } catch (e) {
      toast.error(t('common.error'), (e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <Modal
        open={!!value}
        onClose={onClose}
        title={isNew ? t('lib.new') : t('lib.edit')}
        width={520}
        footer={
          <>
            {lib && (
              <Button variant="ghost" className="mr-auto text-danger" icon={<Trash2 className="size-4" />} onClick={() => setConfirmDelete(true)}>
                {t('lib.delete')}
              </Button>
            )}
            <Button variant="ghost" onClick={onClose}>
              {t('common.cancel')}
            </Button>
            <Button variant="primary" loading={busy} onClick={save} disabled={isNew && !paths.length}>
              {isNew ? t('common.create') : t('common.save')}
            </Button>
          </>
        }
      >
        <div className="flex flex-col gap-5 pb-2">
          <div className="flex items-center gap-4">
            <motion.div layout className="grid size-14 shrink-0 place-items-center rounded-2xl text-white shadow-lg" style={{ background: color }}>
              {(() => {
                const I = LIBRARY_ICONS[icon];
                return I ? <I className="size-7" /> : null;
              })()}
            </motion.div>
            <Field label={t('lib.name')} className="flex-1">
              <Input autoFocus value={name} onChange={(e) => setName(e.target.value)} />
            </Field>
          </div>
          <Field label={t('lib.color')}>
            <ColorSwatches value={color} onChange={setColor} colors={ACCENTS} />
          </Field>
          <Field label={t('lib.icon')}>
            <div className="flex flex-wrap gap-1.5">
              {Object.entries(LIBRARY_ICONS).map(([k, I]) => (
                <button
                  key={k}
                  type="button"
                  onClick={() => setIcon(k)}
                  className={cn('grid size-9 place-items-center rounded-xl border transition-colors', icon === k ? 'border-transparent bg-surface-3 text-fg' : 'border-line text-faint hover:text-fg')}
                >
                  <I className="size-4" />
                </button>
              ))}
            </div>
          </Field>
          <Field label={t('lib.folders')}>
            <div className="flex flex-col gap-1.5">
              {lib?.folders.map((f) => (
                <div key={f.id} className="flex items-center gap-2.5 rounded-xl border border-line bg-surface px-3 py-2">
                  {f.remote ? <Cloud className="size-4 shrink-0 text-[#e5484d]" /> : <Folder className="size-4 shrink-0 text-accent" />}
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-sm">{f.name}</div>
                    {/* MEGA folders: the link without its key */}
                    <div className="truncate font-mono text-[11px] text-faint">{f.remote ? f.remote.label : f.path}</div>
                  </div>
                  <IconButton label={f.remote ? t('mega.refresh') : t('gallery.rescan')} size="sm" onClick={() => (f.remote && backend.remoteRefresh ? backend.remoteRefresh(f.id).catch(() => {}) : backend.rescanFolder(f.id))}>
                    <RefreshCw />
                  </IconButton>
                  <IconButton
                    label={t('lib.removeFolder')}
                    size="sm"
                    onClick={async () => {
                      await backend.removeFolder(f.id);
                      await refresh();
                      toast.info(t('lib.removeFolder'), t('lib.removeFolderHint'));
                    }}
                  >
                    <X />
                  </IconButton>
                </div>
              ))}
              <AnimatePresence>
                {paths.map((p) => (
                  <motion.div key={p} initial={{ opacity: 0, y: -6 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0 }} className="flex items-center gap-2.5 rounded-xl border border-dashed border-line-strong px-3 py-2">
                    <Folder className="size-4 shrink-0 text-ok" />
                    <span className="min-w-0 flex-1 truncate font-mono text-xs">{p}</span>
                    <IconButton label={t('common.remove')} size="sm" onClick={() => setPaths((c) => c.filter((x) => x !== p))}>
                      <X />
                    </IconButton>
                  </motion.div>
                ))}
              </AnimatePresence>
              <div className="flex flex-wrap gap-2">
                <Button variant="secondary" icon={<Plus className="size-4" />} onClick={pickFolder}>
                  {t('lib.addFolder')}
                </Button>
                {/* a MEGA link as a streamed folder of this (or a new) library — nothing is downloaded */}
                {backend.remoteAddFolder && (
                  <Button
                    variant="ghost"
                    icon={<Cloud className="size-4 text-[#e5484d]" />}
                    title={t('mega.addLinkHint')}
                    onClick={() => {
                      onClose();
                      useRemote.getState().show(null, { intent: 'stream', libraryId: lib?.id });
                    }}
                  >
                    {t('mega.addLink')}
                  </Button>
                )}
              </div>
            </div>
          </Field>
        </div>
      </Modal>
      <HostFolderPicker
        open={picker}
        onClose={() => setPicker(false)}
        onPick={(p) => {
          addPaths([p]);
          setPicker(false);
        }}
      />
      {lib && (
        <ConfirmDialog
          open={confirmDelete}
          danger
          title={t('lib.delete')}
          body={t('lib.deleteConfirm', { name: lib.name })}
          confirmLabel={t('common.remove')}
          cancelLabel={t('common.cancel')}
          onClose={() => setConfirmDelete(false)}
          onConfirm={async () => {
            await backend.deleteLibrary(lib.id);
            await refresh();
            onClose();
            nav('/');
          }}
        />
      )}
    </>
  );
}
