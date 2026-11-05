import { useState } from 'react';
import { create } from 'zustand';
import { useQueryClient } from '@tanstack/react-query';
import { useNavigate } from 'react-router';
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
  Trash2,
} from 'lucide-react';
import type { FileRef, MediaItem } from '@peekaround/shared';
import { joinRel } from '@peekaround/shared';
import { useBackend, useInfo, type Backend } from '@/backend';
import { desktop } from '@/backend/desktop';
import { useT } from '@/i18n';
import { folderHref } from '@/lib/routes';
import { useLibraries } from '@/lib/queries';
import { useSelection } from '@/store/selection';
import { toast } from '@/store/toasts';
import { useWorkbench } from '@/store/workbench';
import { Button, ConfirmDialog, Input, MenuItem, MenuSep, Modal } from '@/components/ui';
import { FolderPicker } from './FolderPicker';

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
  a.href = url;
  a.download = '';
  a.rel = 'noopener';
  document.body.appendChild(a);
  a.click();
  a.remove();
}

export function editHref(m: MediaItem) {
  return m.kind === 'video' ? `/edit/video/${m.id}` : `/edit/photo/${m.id}`;
}

/** Shared actions for one or more media items (context menu, viewer, selection bar). */
export function useMediaActions() {
  const backend = useBackend();
  const qc = useQueryClient();
  const nav = useNavigate();
  const t = useT();
  const ui = useActionUi();
  const libs = useLibraries();
  const isTemp = (folderId: string) => !!libs.data?.temp.some((f) => f.id === folderId);

  return {
    async favorite(items: MediaItem[], fav: boolean) {
      await backend.setFavorite(
        items.map((i) => i.id),
        fav,
      );
      await qc.invalidateQueries({ queryKey: ['browse'] });
    },
    download(items: MediaItem[]) {
      if (backend.share) {
        void backend.share(items.map((i) => i.id));
        return;
      }
      if (items.length === 1) download(backend.fileUrl(items[0].id, true));
      else download(backend.downloadUrl(items.map((i) => i.id)));
    },
    remove(items: MediaItem[]) {
      ui.open({ kind: 'delete', refs: items.map(refOf), temp: items.some((i) => isTemp(i.folderId)) });
    },
    move(items: MediaItem[], copy = false) {
      ui.open({ kind: 'move', refs: items.map(refOf), copy });
    },
    rename(item: MediaItem) {
      ui.open({ kind: 'rename', ref: refOf(item), name: item.name });
    },
    edit(item: MediaItem) {
      nav(editHref(item));
    },
    slideshow(items: MediaItem[], target?: { folderId: string; dir: string } | null) {
      useWorkbench.getState().set(items, target ?? null);
      nav('/slideshow');
    },
    collage(items: MediaItem[], target?: { folderId: string; dir: string } | null) {
      useWorkbench.getState().set(
        items.filter((i) => i.kind === 'image'),
        target ?? (items[0] ? { folderId: items[0].folderId, dir: items[0].dir } : null),
      );
      nav('/collage');
    },
    showInFolder(item: MediaItem) {
      nav(folderHref(item.folderId, item.dir));
    },
    async reveal(item: MediaItem) {
      const d = await backend.media(item.id);
      await desktop?.showInFolder(d.path);
    },
    t,
  };
}

/** Context menu body for media items. */
export function MediaMenu({ items, onOpen }: { items: MediaItem[]; onOpen?: () => void }) {
  const a = useMediaActions();
  const info = useInfo();
  const t = a.t;
  const one = items.length === 1 ? items[0] : null;
  const writable = info && !info.caps.readonly;
  const allFav = items.every((i) => i.favorite);
  return (
    <>
      {one && onOpen && (
        <MenuItem icon={<Maximize2 />} onClick={onOpen}>
          {t('common.open')}
        </MenuItem>
      )}
      {one && writable && one.kind !== 'audio' && (
        <MenuItem icon={<Edit3 />} onClick={() => a.edit(one)}>
          {one.kind === 'video' ? t('tools.editVideo') : t('tools.editPhoto')}
        </MenuItem>
      )}
      <MenuItem icon={allFav ? <HeartOff /> : <Heart />} onClick={() => a.favorite(items, !allFav)}>
        {allFav ? t('sel.unfavorite') : t('sel.favorite')}
      </MenuItem>
      <MenuItem icon={<Download />} onClick={() => a.download(items)}>
        {t('common.download')}
      </MenuItem>
      <MenuSep />
      <MenuItem icon={<Presentation />} onClick={() => a.slideshow(items)}>
        {t('sel.slideshow')}
      </MenuItem>
      {writable && items.some((i) => i.kind === 'image') && (
        <MenuItem icon={<LayoutGrid />} onClick={() => a.collage(items)}>
          {t('sel.collage')}
        </MenuItem>
      )}
      {one && (
        <MenuItem icon={<FolderOpen />} onClick={() => a.showInFolder(one)}>
          {t('viewer.showInFolder')}
        </MenuItem>
      )}
      {one && desktop && info?.mode === 'desktop' && (
        <MenuItem icon={<ExternalLink />} onClick={() => a.reveal(one)}>
          {desktop.platform === 'win32' ? 'Explorer' : desktop.platform === 'darwin' ? 'Finder' : t('viewer.showInFolder')}
        </MenuItem>
      )}
      {writable && (
        <>
          <MenuSep />
          {one && (
            <MenuItem icon={<Pencil />} onClick={() => a.rename(one)}>
              {t('common.rename')}
            </MenuItem>
          )}
          <MenuItem icon={<FolderInput />} onClick={() => a.move(items)}>
            {t('sel.move')}
          </MenuItem>
          <MenuItem icon={<Copy />} onClick={() => a.move(items, true)}>
            {t('sel.copy')}
          </MenuItem>
          <MenuItem icon={<Trash2 />} danger onClick={() => a.remove(items)}>
            {t('common.delete')}
          </MenuItem>
        </>
      )}
    </>
  );
}

async function refresh(qc: ReturnType<typeof useQueryClient>) {
  await Promise.all([qc.invalidateQueries({ queryKey: ['browse'] }), qc.invalidateQueries({ queryKey: ['libraries'] })]);
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
