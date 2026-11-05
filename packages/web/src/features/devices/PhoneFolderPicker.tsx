import { useCallback, useEffect, useMemo, useState } from 'react';
import { ArrowLeft, Check, ChevronRight, Clock3, FileImage, FileVideo, Folder, FolderPlus, HardDrive, Smartphone } from 'lucide-react';
import { PeekLocal, type NativeFile } from '@/backend/android/plugin';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { formatBytes } from '@/lib/format';
import { Button, Input, Modal, Spinner } from '@/components/ui';
import { baseName, collapseNested, crumbsOf, loadRecent, parentOf, prettyPath, saveRecent, toggle } from './phonePicker';

export type PhonePickMode = 'folder' | 'folders' | 'files';

interface Listing {
  dirs: { name: string; path: string }[];
  files: NativeFile[];
}

/**
 * In-app browser for the phone's own storage (PeekLocal.roots / listDirs / list / mkdir).
 *  - mode 'folder'  → pick ONE destination folder (downloads); starts in the last used one
 *  - mode 'folders' → pick several folders (folders shared with a PC)
 *  - mode 'files'   → pick files and/or folders (upload to the PC)
 * Shows the recently used folders of `purpose` as chips; a new folder can be created anywhere.
 */
export function PhoneFolderPicker({
  open,
  mode,
  purpose,
  title,
  confirmLabel,
  initial,
  onClose,
  onPick,
}: {
  open: boolean;
  mode: PhonePickMode;
  purpose: string;
  title: string;
  confirmLabel?: string;
  /** preselected paths (mode 'folders') */
  initial?: string[];
  onClose: () => void;
  onPick: (paths: string[]) => void;
}) {
  const t = useT();
  const labels = useMemo(() => ({ internal: t('devices.picker.internal'), sd: t('devices.picker.sd') }), [t]);
  const [roots, setRoots] = useState<{ name: string; path: string }[] | null>(null);
  const [path, setPath] = useState<string | null>(null);
  const [list, setList] = useState<Listing | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [sel, setSel] = useState<string[]>([]);
  const [newName, setNewName] = useState<string | null>(null);
  const [recent, setRecent] = useState<string[]>([]);

  // (re)open: last used folder first, previous selection restored
  useEffect(() => {
    if (!open) return;
    const r = loadRecent(purpose);
    setRecent(r);
    setSel(initial ?? []);
    setNewName(null);
    setError('');
    setPath(mode === 'folder' && r[0] ? r[0] : null);
    PeekLocal.roots()
      .then((x) => setRoots(x.roots))
      .catch((e) => setError((e as Error).message));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, purpose, mode]);

  const load = useCallback(
    async (p: string) => {
      setLoading(true);
      setError('');
      try {
        if (mode === 'files') {
          const r = await PeekLocal.list({ path: p, recursive: false, limit: 2000 });
          setList({ dirs: r.dirs.map((d) => ({ name: d.name, path: d.path })), files: r.files });
        } else {
          const r = await PeekLocal.listDirs({ path: p });
          setList({ dirs: r.dirs, files: [] });
        }
      } catch (e) {
        setList({ dirs: [], files: [] });
        setError((e as Error).message || t('common.error'));
      } finally {
        setLoading(false);
      }
    },
    [mode, t],
  );

  useEffect(() => {
    if (open && path) void load(path);
    else setList(null);
  }, [open, path, load]);

  // volume roots (internal storage, SD cards) + the usual folders
  const top = useMemo(() => {
    const vols = new Map<string, string>();
    for (const r of roots ?? []) {
      const c = crumbsOf(r.path, labels)[0];
      if (!vols.has(c.path)) vols.set(c.path, c.label);
    }
    return { volumes: [...vols.entries()].map(([p, label]) => ({ path: p, label })), folders: roots ?? [] };
  }, [roots, labels]);

  const crumbs = path ? crumbsOf(path, labels) : [];
  const up = () => setPath(path ? parentOf(path) : null);
  const finish = (paths: string[]) => {
    if (mode === 'folder' && paths[0]) saveRecent(purpose, paths[0]);
    if (mode === 'files' && path) saveRecent(purpose, path);
    if (mode === 'folders') for (const p of paths.slice(-3)) saveRecent(purpose, p);
    onPick(mode === 'folder' ? paths : collapseNested(paths));
  };

  const multi = mode !== 'folder';
  const footer = (
    <>
      <div className="mr-auto min-w-0 truncate text-[13px] text-dim">
        {mode === 'folder'
          ? path
            ? t('devices.picker.target', { path: prettyPath(path, labels) })
            : t('devices.picker.chooseFolder')
          : t('devices.picker.selected', { n: sel.length })}
      </div>
      <Button variant="ghost" onClick={onClose}>
        {t('common.cancel')}
      </Button>
      <Button variant="primary" disabled={mode === 'folder' ? !path : !sel.length} onClick={() => finish(mode === 'folder' ? [path!] : sel)}>
        {confirmLabel ?? (mode === 'folder' ? t('devices.picker.saveHere') : t('common.done'))}
      </Button>
    </>
  );

  return (
    <Modal open={open} onClose={onClose} title={title} width={520} footer={footer}>
      <div className="flex flex-col gap-3" data-testid="phone-folder-picker">
        {recent.length > 0 && (
          <div className="no-scrollbar -mx-1 flex gap-2 overflow-x-auto px-1">
            {recent.slice(0, 4).map((r, i) => (
              <button
                key={r}
                type="button"
                onClick={() => setPath(r)}
                className={cn(
                  'inline-flex h-9 shrink-0 items-center gap-1.5 rounded-full border px-3 text-[13px] transition-colors',
                  path === r ? 'border-[color-mix(in_oklab,var(--accent)_60%,transparent)] bg-[color-mix(in_oklab,var(--accent)_14%,transparent)] text-fg' : 'border-line bg-surface text-dim hover:text-fg',
                )}
              >
                <Clock3 className="size-3.5" />
                {i === 0 && <span className="text-faint">{t('devices.picker.lastUsed')}:</span>}
                <span className="max-w-[160px] truncate">{baseName(r)}</span>
              </button>
            ))}
          </div>
        )}

        <div className="flex items-center gap-1.5">
          <button type="button" disabled={!path} onClick={up} aria-label={t('common.back')} className="grid size-10 shrink-0 place-items-center rounded-xl text-dim hover:bg-surface-2 disabled:opacity-30">
            <ArrowLeft className="size-4" />
          </button>
          <div className="no-scrollbar flex min-w-0 flex-1 items-center gap-0.5 overflow-x-auto text-sm">
            <button type="button" onClick={() => setPath(null)} className="flex shrink-0 items-center gap-1 rounded-md px-1.5 py-1 text-dim hover:bg-surface-2 hover:text-fg">
              <Smartphone className="size-4" />
              {!path && <span>{t('devices.picker.phone')}</span>}
            </button>
            {crumbs.map((c, i) => (
              <span key={c.path} className="flex shrink-0 items-center gap-0.5">
                <ChevronRight className="size-3.5 text-faint" />
                <button
                  type="button"
                  onClick={() => setPath(c.path)}
                  className={cn('max-w-[140px] truncate rounded-md px-1.5 py-1', i === crumbs.length - 1 ? 'font-medium text-fg' : 'text-dim hover:bg-surface-2 hover:text-fg')}
                >
                  {c.label}
                </button>
              </span>
            ))}
          </div>
          {path && mode !== 'files' && (
            <button type="button" title={t('gallery.newFolder')} aria-label={t('gallery.newFolder')} onClick={() => setNewName('')} className="grid size-10 shrink-0 place-items-center rounded-xl text-dim hover:bg-surface-2">
              <FolderPlus className="size-4" />
            </button>
          )}
        </div>

        {newName !== null && path && (
          <form
            className="flex gap-2"
            onSubmit={async (e) => {
              e.preventDefault();
              const n = newName.trim();
              if (!n) return;
              try {
                const r = await PeekLocal.mkdir({ path, name: n });
                setNewName(null);
                setPath(r.path);
              } catch (err) {
                setError((err as Error).message);
              }
            }}
          >
            <Input autoFocus value={newName} onChange={(e) => setNewName(e.target.value)} placeholder={t('gallery.newFolderName')} className="flex-1" />
            <Button type="submit" variant="primary" disabled={!newName.trim()}>
              {t('common.create')}
            </Button>
          </form>
        )}

        {multi && path && (
          <SelectRow checked={sel.includes(path)} label={t('devices.picker.selectThis', { name: baseName(path) })} onToggle={() => setSel((s) => toggle(s, path))} />
        )}

        <div className="h-[min(52dvh,360px)] overflow-auto rounded-2xl border border-line bg-surface p-1">
          {!path && (
            <>
              {top.volumes.map((v) => (
                <Row key={v.path} icon={<HardDrive className="size-5 text-accent" />} label={v.label} onOpen={() => setPath(v.path)} />
              ))}
              {top.folders.map((r) => (
                <Row
                  key={r.path}
                  icon={<Folder className="size-5 text-accent" />}
                  label={r.name}
                  sub={prettyPath(r.path, labels)}
                  onOpen={() => setPath(r.path)}
                  checked={multi ? sel.includes(r.path) : undefined}
                  onToggle={multi ? () => setSel((s) => toggle(s, r.path)) : undefined}
                />
              ))}
              {!roots && !error && (
                <div className="grid h-40 place-items-center">
                  <Spinner />
                </div>
              )}
            </>
          )}
          {path && loading && (
            <div className="grid h-40 place-items-center">
              <Spinner />
            </div>
          )}
          {path &&
            !loading &&
            list?.dirs.map((d) => (
              <Row
                key={d.path}
                icon={<Folder className="size-5 text-accent" />}
                label={d.name}
                onOpen={() => setPath(d.path)}
                checked={multi ? sel.includes(d.path) : undefined}
                onToggle={multi ? () => setSel((s) => toggle(s, d.path)) : undefined}
              />
            ))}
          {path &&
            !loading &&
            list?.files.map((f) => (
              <Row
                key={f.path}
                icon={f.kind === 'video' ? <FileVideo className="size-5 text-faint" /> : <FileImage className="size-5 text-faint" />}
                label={f.name}
                sub={formatBytes(f.size)}
                checked={sel.includes(f.path)}
                onToggle={() => setSel((s) => toggle(s, f.path))}
              />
            ))}
          {path && !loading && list && !list.dirs.length && !list.files.length && !error && <p className="p-4 text-center text-sm text-faint">{t('devices.picker.empty')}</p>}
          {error && <p className="p-4 text-center text-sm text-danger">{error}</p>}
        </div>
      </div>
    </Modal>
  );
}

function SelectRow({ checked, label, onToggle }: { checked: boolean; label: string; onToggle: () => void }) {
  return (
    <button type="button" onClick={onToggle} className="flex min-h-11 items-center gap-3 rounded-xl border border-line bg-surface px-3 text-left text-sm hover:bg-surface-2">
      <Tick checked={checked} />
      <span className="min-w-0 flex-1 truncate">{label}</span>
    </button>
  );
}

function Tick({ checked }: { checked: boolean }) {
  return (
    <span className={cn('grid size-5 shrink-0 place-items-center rounded-md border transition-colors', checked ? 'border-transparent bg-accent text-white' : 'border-line-strong')}>
      {checked && <Check className="size-3.5" strokeWidth={3} />}
    </span>
  );
}

function Row({ icon, label, sub, onOpen, checked, onToggle }: { icon: React.ReactNode; label: string; sub?: string; onOpen?: () => void; checked?: boolean; onToggle?: () => void }) {
  return (
    <div className="flex min-h-12 items-center rounded-xl hover:bg-surface-2">
      {onToggle && (
        <button type="button" aria-label={label} aria-pressed={checked} onClick={onToggle} className="grid size-12 shrink-0 place-items-center">
          <Tick checked={!!checked} />
        </button>
      )}
      <button type="button" onClick={onOpen ?? onToggle} className={cn('flex min-h-12 min-w-0 flex-1 items-center gap-3 py-1.5 pr-3 text-left', !onToggle && 'pl-3')}>
        {icon}
        <span className="min-w-0 flex-1">
          <span className="block truncate text-sm">{label}</span>
          {sub && <span className="block truncate text-xs text-faint">{sub}</span>}
        </span>
        {onOpen && <ChevronRight className="size-4 shrink-0 text-faint" />}
      </button>
    </div>
  );
}
