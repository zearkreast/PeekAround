import { useEffect, useMemo, useRef, useState } from 'react';
import { motion } from 'motion/react';
import { Archive, Check, ChevronRight, Clock3, Cloud, File, FileImage, FileVideo, Folder, FolderInput, Minus, Music, TriangleAlert } from 'lucide-react';
import type { RemoteInspect, UploadZipMode } from '@peekaround/shared';
import { ApiError, useBackend, useInfo } from '@/backend';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { formatBytes } from '@/lib/format';
import { useLibraries, useSettings } from '@/lib/queries';
import { useLocation } from '@/store/location';
import { toast } from '@/store/toasts';
import { Button, Input, Modal, Segmented, Spinner } from '@/components/ui';
import { FolderPicker, type PickedTarget } from '@/features/gallery/FolderPicker';
import { watchJob } from '@/features/devices/DevicesHost';
import { useRemote } from './RemoteHost';
import { buildTree, checkState, defaultSelection, findMegaUrl, importNodes, isMediaNode, looksLikeMega, megaErrorKey, toggleNode, totals, type Tree, type TreeNode } from './mega';

type Stage = { kind: 'input' } | { kind: 'loading'; url: string } | { kind: 'tree'; url: string; data: RemoteInspect } | { kind: 'error'; url: string; message: string };

/** "Add from link": MEGA file/folder link → inspect → choose files → import (transfer job). */
export function MegaDialog() {
  const t = useT();
  const backend = useBackend();
  const { open, url, seq, close } = useRemote();
  const [stage, setStage] = useState<Stage>({ kind: 'input' });
  const [text, setText] = useState('');
  const req = useRef(0);

  const inspect = async (u: string) => {
    const id = ++req.current;
    setStage({ kind: 'loading', url: u });
    try {
      const data = await backend.remoteInspect!(u);
      if (id === req.current) setStage({ kind: 'tree', url: u, data });
    } catch (e) {
      if (id !== req.current) return;
      const err = e as ApiError;
      const m = megaErrorKey(err.status ?? 0, err.message ?? '');
      setStage({ kind: 'error', url: u, message: t(m.key, m.vars) });
    }
  };

  useEffect(() => {
    if (!open) return;
    req.current++;
    setText(url ?? '');
    if (url && backend.remoteInspect) void inspect(url);
    else setStage({ kind: 'input' });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, url, seq]);

  const unsupported = !backend.remoteInspect || backend.kind !== 'http';
  const submitUrl = () => {
    const u = findMegaUrl(text);
    if (!u) {
      setStage({ kind: 'error', url: text, message: looksLikeMega(text) ? t('remote.err.badKey') : t('remote.err.notMega') });
      return;
    }
    void inspect(u);
  };

  return (
    <Modal open={open} onClose={close} title={t('remote.title')} width={640}>
      <div data-testid="mega-dialog" className="pb-2">
        {unsupported ? (
          <div className="flex flex-col items-center gap-3 py-6 text-center">
            <span className="grid size-14 place-items-center rounded-2xl bg-surface-2 text-dim">
              <Cloud className="size-7" />
            </span>
            <p className="max-w-sm text-sm text-dim">{t('remote.needServer')}</p>
            <Button variant="secondary" onClick={close}>
              {t('common.close')}
            </Button>
          </div>
        ) : stage.kind === 'tree' ? (
          <TreeStep key={stage.url} url={stage.url} data={stage.data} onDone={close} onBack={() => setStage({ kind: 'input' })} />
        ) : (
          <form
            className="flex flex-col gap-3"
            onSubmit={(e) => {
              e.preventDefault();
              submitUrl();
            }}
          >
            <p className="text-sm text-dim">{t('remote.intro')}</p>
            <div className="flex gap-2">
              <Input autoFocus={stage.kind === 'input'} value={text} onChange={(e) => setText(e.target.value)} inputMode="url" placeholder="https://mega.nz/folder/…#…" className="flex-1" icon={<Cloud />} />
              <Button type="submit" variant="primary" loading={stage.kind === 'loading'} disabled={!text.trim()}>
                {t('remote.inspect')}
              </Button>
            </div>
            {stage.kind === 'loading' && (
              <div className="flex items-center gap-3 rounded-2xl bg-surface p-4 text-sm text-dim">
                <Spinner />
                {t('remote.loading')}
              </div>
            )}
            {stage.kind === 'error' && (
              <motion.div initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} className="flex items-start gap-3 rounded-2xl bg-danger/10 p-4 text-sm text-danger" role="alert">
                <TriangleAlert className="mt-0.5 size-4 shrink-0" />
                <span className="flex-1">{stage.message}</span>
                {findMegaUrl(stage.url) && (
                  <button type="button" className="shrink-0 font-semibold underline-offset-2 hover:underline" onClick={() => void inspect(findMegaUrl(stage.url)!)}>
                    {t('common.retry')}
                  </button>
                )}
              </motion.div>
            )}
            <p className="text-xs text-faint">{t('remote.pasteHint')}</p>
          </form>
        )}
      </div>
    </Modal>
  );
}

const MAX_ROWS = 400;

function TreeStep({ url, data, onDone, onBack }: { url: string; data: RemoteInspect; onDone: () => void; onBack: () => void }) {
  const t = useT();
  const backend = useBackend();
  const info = useInfo();
  const settings = useSettings();
  const libs = useLibraries();
  const here = useLocation((s) => s.target);
  const hereLabel = useLocation((s) => s.targetLabel);
  const tree = useMemo(() => buildTree(data), [data]);
  const [sel, setSel] = useState(() => defaultSelection(data));
  const [open, setOpen] = useState<Set<string>>(() => new Set(data.nodes.length <= 60 ? data.nodes.filter((n) => n.dir).map((n) => n.id) : [tree.root.node.id]));
  const hasPc = !!libs.data?.libraries.some((l) => l.folders.length);
  const [mode, setMode] = useState<'extract' | 'temp'>(here || hasPc ? 'extract' : 'temp');
  const [target, setTarget] = useState<PickedTarget | null>(here ? { ...here, label: hereLabel } : null);
  const [picking, setPicking] = useState(false);
  const [zip, setZip] = useState<UploadZipMode>('extract');
  const [busy, setBusy] = useState(false);
  const sum = totals(tree, sel);
  const askZip = sum.zips.length > 0 && settings.data?.zipAction === 'ask';
  const ro = !!info?.caps.readonly;

  const start = async () => {
    if (mode === 'extract' && !target) {
      setPicking(true);
      return;
    }
    setBusy(true);
    try {
      const job = await backend.remoteImport!({
        url,
        nodes: importNodes(tree, sel),
        mode,
        folderId: mode === 'extract' ? target!.folderId : undefined,
        dir: mode === 'extract' ? target!.dir : undefined,
        zip: askZip ? zip : undefined,
      });
      watchJob(job);
      toast.info(t('remote.started'), data.name);
      onDone();
    } catch (e) {
      const err = e as ApiError;
      const m = megaErrorKey(err.status ?? 0, err.message ?? '');
      toast.error(t('common.error'), t(m.key, m.vars));
    } finally {
      setBusy(false);
    }
  };

  const rows: { t: TreeNode; depth: number }[] = [];
  const walk = (n: TreeNode, depth: number) => {
    for (const c of n.children) {
      if (rows.length >= MAX_ROWS) return;
      rows.push({ t: c, depth });
      if (c.node.dir && open.has(c.node.id)) walk(c, depth + 1);
    }
  };
  if (data.kind === 'file') rows.push({ t: tree.root, depth: 0 });
  else walk(tree.root, 0);
  const rootState = checkState(tree.root, sel);

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center gap-3">
        <span className="grid size-11 shrink-0 place-items-center rounded-2xl bg-[#d9272e]/15 text-[#e5484d]">
          <Cloud className="size-5" />
        </span>
        <div className="min-w-0 flex-1">
          <div className="truncate font-medium">{data.name}</div>
          <div className="text-xs text-faint">
            {t('remote.summary', { n: tree.files.length, size: formatBytes(data.size) })}
            {data.truncated && <span className="ml-1 text-warn">· {t('remote.truncated')}</span>}
          </div>
        </div>
        <button type="button" onClick={onBack} className="shrink-0 text-xs text-faint hover:text-fg">
          {t('remote.otherLink')}
        </button>
      </div>

      <div className="rounded-2xl border border-line bg-surface">
        {data.kind === 'folder' && (
          <button type="button" onClick={() => setSel(toggleNode(tree.root, sel))} className="flex min-h-11 w-full items-center gap-3 border-b border-line px-3 text-left text-sm font-medium">
            <Tri state={rootState} />
            {t('remote.selectAll')}
            <span className="ml-auto text-xs font-normal text-faint">{t('remote.mediaPreselected')}</span>
          </button>
        )}
        <div className="max-h-[min(42dvh,340px)] overflow-auto p-1" data-testid="mega-tree">
          {rows.map(({ t: n, depth }) => (
            <Row
              key={n.node.id}
              n={n}
              depth={depth}
              state={checkState(n, sel)}
              expanded={open.has(n.node.id)}
              onToggle={() => setSel(toggleNode(n, sel))}
              onExpand={() =>
                setOpen((o) => {
                  const x = new Set(o);
                  if (x.has(n.node.id)) x.delete(n.node.id);
                  else x.add(n.node.id);
                  return x;
                })
              }
            />
          ))}
          {rows.length >= MAX_ROWS && <p className="p-2 text-center text-xs text-faint">{t('remote.moreRows')}</p>}
        </div>
      </div>

      {/* destination */}
      <div className="flex flex-col gap-2.5">
        <Segmented
          value={mode}
          onChange={(m) => setMode(m)}
          options={[
            { value: 'extract', label: t('remote.mode.extract'), icon: <FolderInput /> },
            { value: 'temp', label: t('remote.mode.temp'), icon: <Clock3 /> },
          ]}
        />
        <p className="text-xs text-faint">{mode === 'extract' ? t('remote.mode.extractHint') : t('remote.mode.tempHint')}</p>
        {mode === 'extract' && (
          <div className="flex items-center gap-2 rounded-2xl bg-surface px-3 py-2">
            <Folder className="size-4 shrink-0 text-accent" />
            <span className={cn('min-w-0 flex-1 truncate text-sm', !target && 'text-faint')}>{target ? target.label : t('remote.noTarget')}</span>
            <Button size="sm" variant="ghost" onClick={() => setPicking(true)}>
              {target ? t('slide.change') : t('remote.pickFolder')}
            </Button>
          </div>
        )}
        {askZip && (
          <div className="rounded-2xl border border-line p-3">
            <div className="mb-2 flex items-center gap-2 text-sm">
              <Archive className="size-4 text-warn" />
              {t('remote.zipAsk', { n: sum.zips.length })}
            </div>
            <Segmented
              size="sm"
              value={zip}
              onChange={setZip}
              options={[
                { value: 'extract', label: t('zip.extract') },
                { value: 'temp', label: t('zip.temp') },
                { value: 'keep', label: t('zip.keep') },
              ]}
            />
          </div>
        )}
      </div>

      <div className="flex flex-wrap items-center justify-end gap-2 border-t border-line pt-4">
        <span className="mr-auto text-sm text-dim tabular-nums">{t('remote.selected', { n: sum.files, size: formatBytes(sum.bytes) })}</span>
        <Button variant="primary" loading={busy} disabled={!sum.files || ro} onClick={start}>
          {t('remote.import')}
        </Button>
      </div>

      <FolderPicker
        open={picking}
        title={t('remote.pickFolder')}
        onClose={() => setPicking(false)}
        onPick={(tg) => {
          setPicking(false);
          setTarget(tg);
        }}
      />
    </div>
  );
}

function Tri({ state }: { state: 'all' | 'none' | 'some' }) {
  return (
    <span className={cn('grid size-5 shrink-0 place-items-center rounded-md border transition-colors', state === 'none' ? 'border-line-strong' : 'border-transparent bg-accent text-white')}>
      {state === 'all' && <Check className="size-3.5" strokeWidth={3} />}
      {state === 'some' && <Minus className="size-3.5" strokeWidth={3} />}
    </span>
  );
}

function iconFor(n: TreeNode) {
  if (n.node.dir) return <Folder className="size-4 text-accent" />;
  switch (n.node.kind) {
    case 'image':
      return <FileImage className="size-4 text-faint" />;
    case 'video':
      return <FileVideo className="size-4 text-faint" />;
    case 'audio':
      return <Music className="size-4 text-faint" />;
    case 'archive':
      return <Archive className="size-4 text-warn" />;
    default:
      return <File className="size-4 text-faint" />;
  }
}

function Row({ n, depth, state, expanded, onToggle, onExpand }: { n: TreeNode; depth: number; state: 'all' | 'none' | 'some'; expanded: boolean; onToggle: () => void; onExpand: () => void }) {
  const dim = !n.node.dir && !isMediaNode(n.node) && n.node.kind !== 'archive';
  const empty = n.node.dir && !n.files.length;
  return (
    <div className={cn('flex min-h-10 items-center rounded-lg pr-2 text-sm hover:bg-surface-2', (dim || empty) && 'opacity-55')} style={{ paddingLeft: 4 + depth * 18 }}>
      {n.node.dir ? (
        <button type="button" onClick={onExpand} aria-label={n.node.name} aria-expanded={expanded} className="grid size-8 shrink-0 place-items-center text-faint hover:text-fg">
          <ChevronRight className={cn('size-4 transition-transform', expanded && 'rotate-90')} />
        </button>
      ) : (
        <span className="w-8 shrink-0" />
      )}
      <button type="button" disabled={empty} onClick={onToggle} className="flex min-h-10 min-w-0 flex-1 items-center gap-2.5 text-left" data-node={n.node.id}>
        <Tri state={state} />
        {iconFor(n)}
        <span className="min-w-0 flex-1 truncate">{n.node.name}</span>
        <span className="shrink-0 text-xs text-faint tabular-nums">{n.node.dir ? `${n.files.length} · ${formatBytes(n.node.size)}` : formatBytes(n.node.size)}</span>
      </button>
    </div>
  );
}
