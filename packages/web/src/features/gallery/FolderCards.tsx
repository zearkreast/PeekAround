import { useState } from 'react';
import { Link } from 'react-router';
import { motion } from 'motion/react';
import { Archive, Eye, Folder, FolderInput, Pencil, Presentation, Trash2 } from 'lucide-react';
import type { FolderRef, MediaItem, SubFolder } from '@peekaround/shared';
import { useQueryClient } from '@tanstack/react-query';
import { useBackend, useInfo } from '@/backend';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { formatBytes } from '@/lib/format';
import { go } from '@/lib/nav';
import { folderHref } from '@/lib/routes';
import { toast } from '@/store/toasts';
import { Button, ContextMenu, MenuItem, MenuSep } from '@/components/ui';
import { useActionUi } from './actions';
import { FolderMenuItems } from '@/features/devices/FolderActions';
import { useDeviceFolder } from '@/features/devices/useDevices';

function Covers({ ids, className }: { ids: string[]; className?: string }) {
  const backend = useBackend();
  if (!ids.length)
    return (
      <div className={cn('grid place-items-center bg-surface', className)}>
        <Folder className="size-10 text-faint" />
      </div>
    );
  return (
    <div className={cn('grid gap-0.5', ids.length === 1 ? 'grid-cols-1' : 'grid-cols-[2fr_1fr] grid-rows-2', className)}>
      {ids.slice(0, 3).map((id, i) => (
        <img
          key={id}
          src={backend.thumbUrl(id, 320)}
          alt=""
          loading="lazy"
          draggable={false}
          className={cn('size-full object-cover transition-transform duration-700 ease-[var(--ease-out)] group-hover:scale-105', i === 0 && ids.length > 1 && 'row-span-2')}
        />
      ))}
    </div>
  );
}

export function FolderCard({ folderId, f, index, onSlideshow }: { folderId: string; f: SubFolder; index: number; onSlideshow: () => void }) {
  const t = useT();
  const backend = useBackend();
  const info = useInfo();
  const ui = useActionUi();
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const writable = info && !info.caps.readonly;
  // folders on a linked phone: view + copy only
  const device = useDeviceFolder(folderId);
  return (
    <>
      <motion.div
        initial={{ opacity: 0, y: 16 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ type: 'spring', stiffness: 300, damping: 28, delay: Math.min(index, 12) * 0.035 }}
        whileHover={{ y: -4 }}
        onContextMenu={(e) => {
          e.preventDefault();
          setMenu({ x: e.clientX, y: e.clientY });
        }}
      >
        <Link
          to={folderHref(folderId, f.dir)}
          data-drop-folder={folderId}
          data-drop-dir={f.dir}
          data-drop-label={f.name}
          className="group block overflow-hidden rounded-2xl border border-line bg-elev shadow-sm transition-shadow hover:shadow-soft"
        >
          <Covers ids={f.coverIds} className="aspect-[4/3] overflow-hidden" />
          <div className="flex items-center gap-2 px-3.5 py-3">
            <Folder className="size-4 shrink-0 text-accent" />
            <span className="min-w-0 flex-1 truncate text-sm font-medium">{f.name}</span>
            <span className="text-xs text-faint tabular-nums">{f.mediaCount}</span>
          </div>
        </Link>
      </motion.div>
      <ContextMenu at={menu} onClose={() => setMenu(null)}>
        <MenuItem icon={<Presentation />} onClick={onSlideshow}>
          {t('gallery.slideshowHere')}
        </MenuItem>
        <FolderMenuItems folderId={folderId} dir={f.dir} name={f.name} />
        {writable && !device && (
          <>
            <MenuSep />
            <MenuItem icon={<Pencil />} onClick={() => ui.open({ kind: 'rename', ref: { folderId, rel: f.dir }, name: f.name })}>
              {t('common.rename')}
            </MenuItem>
            <MenuItem icon={<FolderInput />} onClick={() => ui.open({ kind: 'move', refs: [{ folderId, rel: f.dir }], copy: false })}>
              {t('sel.move')}
            </MenuItem>
            <MenuItem icon={<Trash2 />} danger onClick={() => ui.open({ kind: 'delete', refs: [{ folderId, rel: f.dir }], temp: false })}>
              {t('common.delete')}
            </MenuItem>
          </>
        )}
      </ContextMenu>
    </>
  );
}

export function RootCard({ f, index }: { f: FolderRef; index: number }) {
  return (
    <motion.div initial={{ opacity: 0, y: 16 }} animate={{ opacity: 1, y: 0 }} transition={{ type: 'spring', stiffness: 300, damping: 28, delay: index * 0.04 }} whileHover={{ y: -4 }}>
      <Link to={folderHref(f.id)} data-drop-folder={f.id} data-drop-dir="" data-drop-label={f.name} className="group flex items-center gap-3 rounded-2xl border border-line bg-elev p-4 transition-shadow hover:shadow-soft">
        <span className="grid size-11 place-items-center rounded-xl bg-surface-2 text-accent">
          <Folder className="size-5" />
        </span>
        <span className="min-w-0 flex-1">
          <span className="block truncate font-medium">{f.name}</span>
          <span className="block truncate font-mono text-[11px] text-faint">{f.path}</span>
        </span>
        <span className="text-sm text-dim tabular-nums">{f.mediaCount}</span>
      </Link>
    </motion.div>
  );
}

export function ArchiveCard({ item, index }: { item: MediaItem; index: number }) {
  const t = useT();
  const backend = useBackend();
  const info = useInfo();
  const qc = useQueryClient();
  const [busy, setBusy] = useState<'peek' | 'extract' | null>(null);
  return (
    <motion.div
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ delay: index * 0.04 }}
      className="flex items-center gap-3 rounded-2xl border border-line bg-elev p-3 pr-3.5"
    >
      <span className="grid size-11 shrink-0 place-items-center rounded-xl bg-[color-mix(in_oklab,var(--warn)_18%,transparent)] text-warn">
        <Archive className="size-5" />
      </span>
      <div className="min-w-0 flex-1">
        <div className="truncate text-sm font-medium">{item.name}</div>
        <div className="text-xs text-faint">{formatBytes(item.size)}</div>
      </div>
      <Button
        size="sm"
        variant="secondary"
        icon={<Eye className="size-4" />}
        loading={busy === 'peek'}
        onClick={async () => {
          setBusy('peek');
          try {
            const r = await backend.peekZip(item.id);
            await qc.invalidateQueries({ queryKey: ['libraries'] });
            go(folderHref(r.tempFolder.id));
          } catch (e) {
            toast.error(t('common.error'), (e as Error).message);
          } finally {
            setBusy(null);
          }
        }}
      >
        {t('gallery.peek')}
      </Button>
      {info && !info.caps.readonly && (
        <Button
          size="sm"
          variant="ghost"
          loading={busy === 'extract'}
          onClick={async () => {
            setBusy('extract');
            try {
              const r = await backend.extractZip(item.id);
              toast.success(t('upload.extracted'), r.dir, { label: t('common.open'), run: () => go(folderHref(r.folderId, r.dir)) });
              await qc.invalidateQueries({ queryKey: ['browse'] });
            } catch (e) {
              toast.error(t('common.error'), (e as Error).message);
            } finally {
              setBusy(null);
            }
          }}
        >
          {t('gallery.extract')}
        </Button>
      )}
    </motion.div>
  );
}
