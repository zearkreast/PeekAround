import { safeHref } from '@/lib/safeUrl';
/**
 * Media actions — ONE declarative registry used by the desktop selection bar, the mobile
 * selection sheet, the viewer bars and the right-click / long-press context menu.
 *
 * Registering an action (e.g. from features/devices):
 *
 *   import { registerMediaAction } from '@/features/gallery/actions';
 *   registerMediaAction({
 *     id: 'send-to-phone',
 *     icon: <Smartphone />,
 *     labelKey: 'devices.sendToPhone',          // any MessageKey (tr + en)
 *     group: 'share',                           // primary | share | create | organize | open | danger
 *     order: 25,                                // position inside the group (lower = earlier)
 *     surfaces: ['selection', 'viewer', 'menu'],// default: all three
 *     visible: (items, ctx) => ctx.backend.kind === 'http' && items.length > 0,
 *     run: async (items, ctx) => { … ctx.toast / ctx.nav / ctx.backend … },
 *   });
 *
 * Call it once at module load (side-effect import from the feature's entry) — it returns an
 * unregister function. Components re-render automatically when the registry changes.
 * `primary: true` keeps an action on the mobile bottom bar instead of the "More" sheet.
 */
import { useMemo, useState, type ReactNode } from 'react';
import { create } from 'zustand';
import { useQueryClient, type QueryClient } from '@tanstack/react-query';
import { useNavigate, type NavigateFunction } from 'react-router';
import {
  Copy,
  Download,
  Edit3,
  ExternalLink,
  FolderInput,
  FolderOpen,
  Heart,
  HeartOff,
  LayoutGrid,
  Maximize2,
  Pencil,
  Presentation,
  Share2,
  Trash2,
} from 'lucide-react';
import type { DeviceInfo, FileRef, FolderRef, MediaItem, ServerInfo } from '@peekaround/shared';
import { joinRel } from '@peekaround/shared';
import { useBackend, useInfo, type Backend } from '@/backend';
import { desktop } from '@/backend/desktop';
import { isNative } from '@/backend/android/plugin';
import { useT, type MessageKey } from '@/i18n';
import { folderHref } from '@/lib/routes';
import { useLibraries } from '@/lib/queries';
import { useLocation } from '@/store/location';
import { useSelection } from '@/store/selection';
import { toast } from '@/store/toasts';
import { useWorkbench } from '@/store/workbench';
import { Button, ConfirmDialog, Input, MenuItem, MenuSep, Modal } from '@/components/ui';
import { FolderPicker } from './FolderPicker';
import { liveSources, patchItems } from './grid/source';

export const refOf = (m: Pick<MediaItem, 'folderId' | 'dir' | 'name'>): FileRef => ({ folderId: m.folderId, rel: joinRel(m.dir, m.name) });

type Dialog =
  | { kind: 'delete'; refs: FileRef[]; temp: boolean }
  | { kind: 'move'; refs: FileRef[]; copy: boolean }
  | { kind: 'rename'; ref: FileRef; name: string };

interface ActionUi {
  dialog: Dialog | null;
  open(d: Dialog): void;
  close(): void;
}

export const useActionUi = create<ActionUi>((set) => ({
  dialog: null,
  open: (dialog) => set({ dialog }),
  close: () => set({ dialog: null }),
}));

export function download(url: string) {
  const a = document.createElement('a');
  a.href = safeHref(url);
  a.download = '';
  a.rel = 'noopener';
  document.body.appendChild(a);
  a.click();
  a.remove();
}

export function editHref(m: MediaItem) {
  return m.kind === 'video' ? `/edit/video/${m.id}` : `/edit/photo/${m.id}`;
}

// ───────────────────────── registry ─────────────────────────

export type ActionSurface = 'selection' | 'viewer' | 'menu';
export type ActionGroup = 'primary' | 'share' | 'create' | 'organize' | 'open' | 'danger';

export interface ActionCtx {
  backend: Backend;
  info: ServerInfo | undefined;
  /** server allows writes */
  writable: boolean;
  surface: ActionSurface;
  /** gallery location (save target for slideshow / collage) */
  target: { folderId: string; dir: string } | null;
  isTemp(folderId: string): boolean;
  /** the linked phone a folder lives on (view + copy only: no delete/rename/move/edit/favorite), or null */
  deviceOf(folderId: string): DeviceInfo | null;
  /** the remote (MEGA streaming) folder an item lives in (view + copy only), or null */
  remoteOf?(folderId: string): FolderRef | null;
  nav: NavigateFunction;
  qc: QueryClient;
  t: ReturnType<typeof useT>;
  toast: typeof toast;
  dialogs: ActionUi;
}

export interface MediaAction {
  id: string;
  /** icon element, or a function of the items (e.g. filled / empty heart) */
  icon: ReactNode | ((items: MediaItem[], ctx: ActionCtx) => ReactNode);
  labelKey: MessageKey;
  /** short label for labelled phone bars (falls back to the label) */
  shortLabelKey?: MessageKey;
  /** dynamic label override (e.g. favorite / unfavorite) */
  label?: (items: MediaItem[], ctx: ActionCtx) => string;
  run(items: MediaItem[], ctx: ActionCtx): void | Promise<void>;
  visible?(items: MediaItem[], ctx: ActionCtx): boolean;
  /** highlighted state (favorite) */
  active?(items: MediaItem[], ctx: ActionCtx): boolean;
  destructive?: boolean;
  group?: ActionGroup;
  order?: number;
  surfaces?: ActionSurface[];
  /** keep on the mobile bottom bar (otherwise in "More") */
  primary?: boolean;
  /** clear the selection after running from the selection bar (default: destructive/organize) */
  clearsSelection?: boolean;
}

const GROUP_ORDER: ActionGroup[] = ['primary', 'share', 'create', 'organize', 'open', 'danger'];

const useRegistry = create<{ actions: MediaAction[] }>(() => ({ actions: [] }));

/** Register (or replace by id) a media action. Returns an unregister function. */
export function registerMediaAction(a: MediaAction): () => void {
  useRegistry.setState((s) => ({ actions: [...s.actions.filter((x) => x.id !== a.id), a] }));
  return () => useRegistry.setState((s) => ({ actions: s.actions.filter((x) => x !== a) }));
}

function sortActions(list: MediaAction[]) {
  return [...list].sort((a, b) => GROUP_ORDER.indexOf(a.group ?? 'open') - GROUP_ORDER.indexOf(b.group ?? 'open') || (a.order ?? 50) - (b.order ?? 50));
}

export function iconOf(a: MediaAction, items: MediaItem[], ctx: ActionCtx): ReactNode {
  return typeof a.icon === 'function' ? (a.icon as (i: MediaItem[], c: ActionCtx) => ReactNode)(items, ctx) : a.icon;
}
export function labelOf(a: MediaAction, items: MediaItem[], ctx: ActionCtx): string {
  return a.label ? a.label(items, ctx) : ctx.t(a.labelKey);
}
/** compact label for the phone bottom bars */
export function shortLabelOf(a: MediaAction, items: MediaItem[], ctx: ActionCtx): string {
  return a.shortLabelKey ? ctx.t(a.shortLabelKey) : labelOf(a, items, ctx);
}

// ── built-in actions ──

async function refresh(qc: QueryClient) {
  await Promise.all([qc.invalidateQueries({ queryKey: ['browse'] }), qc.invalidateQueries({ queryKey: ['libraries'] })]);
}

const isMedia = (i: MediaItem) => i.kind === 'image' || i.kind === 'video';
/** some items live on a linked phone (read + copy only) */
export const onDevice = (items: MediaItem[], c: Pick<ActionCtx, 'deviceOf' | 'remoteOf'>) => items.some((i) => !!c.deviceOf(i.folderId)) || onRemote(items, c);
/** some items stream from a MEGA link (read-only, copy to this computer) */
export const onRemote = (items: MediaItem[], c: Pick<ActionCtx, 'remoteOf'>) => !!c.remoteOf && items.some((i) => !!c.remoteOf!(i.folderId));
/** Android app in client mode: "download" saves on the phone natively (features/devices) */
export const isPhoneClient = (c: Pick<ActionCtx, 'backend'>) => c.backend.kind === 'http' && isNative();

export async function setFavorite(items: MediaItem[], fav: boolean, ctx: Pick<ActionCtx, 'backend' | 'qc'>) {
  const ids = items.map((i) => i.id);
  // optimistic: patch every loaded copy in place (no relayout, no refetch storm)
  patchItems(ids, { favorite: fav });
  const sel = useSelection.getState();
  if (ids.some((id) => sel.items.has(id))) {
    const n = new Map(sel.items);
    for (const id of ids) {
      const it = n.get(id);
      if (it) n.set(id, { ...it, favorite: fav });
    }
    useSelection.setState({ items: n });
  }
  for (const id of ids) ctx.qc.setQueryData(['media', ctx.backend.id, id], (d: MediaItem | undefined) => (d ? { ...d, favorite: fav } : d));
  try {
    await ctx.backend.setFavorite(ids, fav);
  } catch (e) {
    patchItems(ids, { favorite: !fav });
    toast.error((e as Error).message);
    return;
  }
  for (const s of liveSources()) if (s.query.kind === 'favorites') s.refresh();
}

export const BUILTIN_ACTIONS: MediaAction[] = [
  {
    id: 'share',
    icon: <Share2 />,
    labelKey: 'mobile.action.share',
    group: 'primary',
    order: 10,
    primary: true,
    visible: (_i, c) => !!c.backend.share,
    run: (items, c) => void c.backend.share?.(items.map((i) => i.id)),
  },
  {
    id: 'download',
    icon: <Download />,
    labelKey: 'common.download',
    group: 'primary',
    order: 11,
    primary: true,
    visible: (_i, c) => !c.backend.share && !isPhoneClient(c),
    run: (items, c) => {
      if (items.length === 1) download(c.backend.fileUrl(items[0].id, true));
      else download(c.backend.downloadUrl(items.map((i) => i.id)));
    },
  },
  {
    id: 'edit',
    icon: <Edit3 />,
    labelKey: 'viewer.edit',
    group: 'primary',
    order: 20,
    primary: true,
    visible: (items, c) => c.writable && items.length === 1 && isMedia(items[0]) && !onDevice(items, c),
    run: (items, c) => c.nav(editHref(items[0])),
  },
  {
    id: 'favorite',
    icon: (items) => (items.length && items.every((i) => i.favorite) ? <HeartOff /> : <Heart />),
    labelKey: 'sel.favorite',
    shortLabelKey: 'viewer.favorite',
    label: (items, c) => (items.length && items.every((i) => i.favorite) ? c.t('sel.unfavorite') : c.t('sel.favorite')),
    active: (items) => items.length > 0 && items.every((i) => i.favorite),
    group: 'primary',
    order: 30,
    primary: true,
    // favorites live in our database: fine for MEGA items, not for phone items
    visible: (items, c) => items.some(isMedia) && !items.some((i) => !!c.deviceOf(i.folderId)),
    run: (items, c) => setFavorite(items, !items.every((i) => i.favorite), c),
  },
  {
    id: 'slideshow',
    icon: <Presentation />,
    labelKey: 'sel.slideshow',
    group: 'create',
    order: 10,
    visible: (items) => items.some(isMedia),
    run: (items, c) => {
      useWorkbench.getState().set(items.filter(isMedia), c.target);
      c.nav('/slideshow');
    },
  },
  {
    id: 'collage',
    icon: <LayoutGrid />,
    labelKey: 'sel.collage',
    group: 'create',
    order: 20,
    visible: (items, c) => c.writable && items.some((i) => i.kind === 'image') && !onDevice(items, c),
    run: (items, c) => {
      useWorkbench.getState().set(
        items.filter((i) => i.kind === 'image'),
        c.target ?? (items[0] ? { folderId: items[0].folderId, dir: items[0].dir } : null),
      );
      c.nav('/collage');
    },
  },
  {
    id: 'move',
    icon: <FolderInput />,
    labelKey: 'sel.move',
    group: 'organize',
    order: 10,
    visible: (items, c) => c.writable && items.length > 0 && !onDevice(items, c),
    run: (items, c) => c.dialogs.open({ kind: 'move', refs: items.map(refOf), copy: false }),
  },
  {
    id: 'copy',
    icon: <Copy />,
    labelKey: 'sel.copy',
    group: 'organize',
    order: 20,
    visible: (items, c) => c.writable && items.length > 0 && !onDevice(items, c),
    run: (items, c) => c.dialogs.open({ kind: 'move', refs: items.map(refOf), copy: true }),
  },
  {
    id: 'rename',
    icon: <Pencil />,
    labelKey: 'common.rename',
    group: 'organize',
    order: 30,
    surfaces: ['menu', 'viewer'],
    visible: (items, c) => c.writable && items.length === 1 && !onDevice(items, c),
    run: (items, c) => c.dialogs.open({ kind: 'rename', ref: refOf(items[0]), name: items[0].name }),
  },
  {
    id: 'show-in-folder',
    icon: <FolderOpen />,
    labelKey: 'viewer.showInFolder',
    group: 'open',
    order: 10,
    surfaces: ['menu', 'viewer'],
    visible: (items) => items.length === 1,
    run: (items, c) => c.nav(folderHref(items[0].folderId, items[0].dir)),
  },
  {
    id: 'reveal',
    icon: <ExternalLink />,
    labelKey: 'mobile.action.reveal',
    label: (_i, c) => (desktop?.platform === 'win32' ? 'Explorer' : desktop?.platform === 'darwin' ? 'Finder' : c.t('mobile.action.reveal')),
    group: 'open',
    order: 20,
    surfaces: ['menu', 'viewer'],
    visible: (items, c) => items.length === 1 && !!desktop && c.info?.mode === 'desktop' && !onDevice(items, c),
    run: async (items, c) => {
      const d = await c.backend.media(items[0].id);
      await desktop?.showInFolder(d.path);
    },
  },
  {
    id: 'delete',
    icon: <Trash2 />,
    labelKey: 'common.delete',
    group: 'danger',
    order: 10,
    primary: true,
    destructive: true,
    visible: (items, c) => c.writable && items.length > 0 && !onDevice(items, c),
    run: (items, c) => c.dialogs.open({ kind: 'delete', refs: items.map(refOf), temp: items.some((i) => c.isTemp(i.folderId)) }),
  },
];
for (const a of BUILTIN_ACTIONS) registerMediaAction(a);

/** Action context + registry for the current page. */
export function useMediaActions(surface: ActionSurface = 'menu') {
  const backend = useBackend();
  const info = useInfo();
  const qc = useQueryClient();
  const nav = useNavigate();
  const t = useT();
  const dialogs = useActionUi();
  const libs = useLibraries();
  const target = useLocation((s) => s.target);
  const registry = useRegistry((s) => s.actions);
  const temp = libs.data?.temp;
  const devices = libs.data?.devices;
  const libraries = libs.data?.libraries;

  const ctx: ActionCtx = useMemo(
    () => ({
      backend,
      info,
      writable: !!info && !info.caps.readonly,
      surface,
      target,
      isTemp: (folderId: string) => !!temp?.some((f) => f.id === folderId),
      deviceOf: (folderId: string) => devices?.find((d) => d.folders.some((f) => f.id === folderId)) ?? null,
      remoteOf: (folderId: string) => libraries?.flatMap((l) => l.folders).find((f) => f.id === folderId && !!f.remote) ?? null,
      nav,
      qc,
      t,
      toast,
      dialogs,
    }),
    [backend, info, surface, target, temp, devices, libraries, nav, qc, t, dialogs],
  );

  return useMemo(() => {
    const all = sortActions(registry);
    const forItems = (items: MediaItem[], s: ActionSurface = surface) => {
      const c = s === ctx.surface ? ctx : { ...ctx, surface: s };
      return all.filter((a) => (!a.surfaces || a.surfaces.includes(s)) && (!a.visible || a.visible(items, c)));
    };
    const run = async (a: MediaAction | string, items: MediaItem[]) => {
      const act = typeof a === 'string' ? all.find((x) => x.id === a) : a;
      if (!act || !items.length) return;
      try {
        await act.run(items, ctx);
        if (ctx.surface === 'selection' && act.clearsSelection) useSelection.getState().clear();
      } catch (e) {
        toast.error(t('common.error'), (e as Error).message);
      }
    };
    return { all, ctx, for: forItems, run, t };
  }, [registry, ctx, surface, t]);
}

/** Context menu body for media items (registry, grouped). */
export function MediaMenu({ items, onOpen }: { items: MediaItem[]; onOpen?: () => void }) {
  const a = useMediaActions('menu');
  const list = a.for(items);
  const out: ReactNode[] = [];
  if (onOpen && items.length === 1) {
    out.push(
      <MenuItem key="open" icon={<Maximize2 />} onClick={onOpen}>
        {a.t('common.open')}
      </MenuItem>,
    );
  }
  let group: string | undefined;
  for (const act of list) {
    if (out.length && act.group !== group) out.push(<MenuSep key={`sep-${act.id}`} />);
    group = act.group;
    out.push(
      <MenuItem key={act.id} icon={iconOf(act, items, a.ctx)} danger={act.destructive} onClick={() => void a.run(act, items)}>
        {labelOf(act, items, a.ctx)}
      </MenuItem>,
    );
  }
  return <>{out}</>;
}

/** Renders the dialogs requested through useActionUi. Mount once. */
export function ActionsHost() {
  const t = useT();
  const backend = useBackend();
  const info = useInfo();
  const qc = useQueryClient();
  const { dialog, close } = useActionUi();
  const [busy, setBusy] = useState(false);

  if (!dialog) return null;

  if (dialog.kind === 'delete') {
    return (
      <ConfirmDialog
        open
        danger
        title={t('sel.deleteConfirm', { n: dialog.refs.length })}
        body={dialog.temp ? t('sel.deleteBodyTemp') : info?.caps.trash === 'os' ? t('sel.deleteBodyTrash') : t('sel.deleteBodyFolder')}
        confirmLabel={t('common.delete')}
        cancelLabel={t('common.cancel')}
        onClose={close}
        onConfirm={async () => {
          await backend.remove(dialog.refs);
          useSelection.getState().clear();
          await refresh(qc);
          toast.success(t('sel.deleted', { n: dialog.refs.length }));
        }}
      />
    );
  }

  if (dialog.kind === 'move') {
    return (
      <FolderPicker
        open
        title={dialog.copy ? t('sel.copy') : t('sel.move')}
        confirmLabel={dialog.copy ? t('sel.copy') : t('sel.move')}
        onClose={close}
        onPick={async (target) => {
          close();
          try {
            await backend.move(dialog.refs, target, dialog.copy);
            useSelection.getState().clear();
            await refresh(qc);
            toast.success(dialog.copy ? t('sel.copied', { n: dialog.refs.length }) : t('sel.moved', { n: dialog.refs.length }));
          } catch (e) {
            toast.error(t('common.error'), (e as Error).message);
          }
        }}
      />
    );
  }

  return (
    <RenameDialog
      initial={dialog.name}
      busy={busy}
      onClose={close}
      onSubmit={async (n) => {
        setBusy(true);
        try {
          await backend.rename(dialog.ref, n);
          await refresh(qc);
          close();
        } catch (e) {
          toast.error(t('common.error'), (e as Error).message);
        } finally {
          setBusy(false);
        }
      }}
    />
  );
}

function RenameDialog({ initial, onClose, onSubmit, busy }: { initial: string; onClose: () => void; onSubmit: (n: string) => void; busy: boolean }) {
  const t = useT();
  const [name, setName] = useState(initial);
  return (
    <Modal
      open
      onClose={onClose}
      title={t('common.rename')}
      width={420}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" loading={busy} disabled={!name.trim()} onClick={() => onSubmit(name.trim())}>
            {t('common.save')}
          </Button>
        </>
      }
    >
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (name.trim()) onSubmit(name.trim());
        }}
      >
        <Input
          autoFocus
          value={name}
          onChange={(e) => setName(e.target.value)}
          onFocus={(e) => {
            const dot = e.target.value.lastIndexOf('.');
            e.target.setSelectionRange(0, dot > 0 ? dot : e.target.value.length);
          }}
        />
      </form>
    </Modal>
  );
}

export type { Backend };
