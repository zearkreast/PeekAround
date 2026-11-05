import { Download, HardDriveDownload, MonitorDown, Smartphone } from 'lucide-react';
import { useBackend, useInfo } from '@/backend';
import { useT } from '@/i18n';
import { download } from '@/features/gallery/actions';
import { Button, MenuItem } from '@/components/ui';
import { OnlineDot } from './DevicesNav';
import { isPhoneClient, useDeviceFolder, useDeviceUi } from './useDevices';

/**
 * Gallery header buttons for the open folder:
 *  - browser / desktop: zip download (+ "PC'ye kopyala" when the folder lives on a linked phone)
 *  - Android app (client mode): "Telefona kaydet" — every file below, sub folders kept
 */
export function FolderHeaderActions({ folderId, dir, name }: { folderId: string; dir: string; name: string }) {
  const t = useT();
  const backend = useBackend();
  const info = useInfo();
  const dev = useDeviceFolder(folderId);
  const open = useDeviceUi((s) => s.open);
  const phone = isPhoneClient(backend);
  return (
    <>
      {dev && (
        <span className="hidden items-center gap-1.5 rounded-full border border-line bg-surface px-2.5 py-1 text-xs text-dim sm:inline-flex" title={dev.device.online ? undefined : t('devices.offlineTip')}>
          <Smartphone className="size-3.5" />
          {dev.device.name}
          <OnlineDot online={dev.device.online} />
        </span>
      )}
      {dev && !phone && info && !info.caps.readonly && (
        <Button
          variant="secondary"
          size="sm"
          icon={<MonitorDown className="size-4" />}
          className="touch-target"
          aria-label={t('devices.copyToPc')}
          disabled={!dev.device.online}
          title={dev.device.online ? undefined : t('devices.offlineTip')}
          onClick={() => open({ kind: 'copyToPc', refs: [{ folderId, rel: dir }], label: name })}
        >
          <span className="hidden sm:inline">{t('devices.copyToPc')}</span>
        </Button>
      )}
      {phone ? (
        <Button variant="ghost" size="sm" className="touch-target" aria-label={t('devices.saveToPhone')} icon={<HardDriveDownload className="size-4" />} onClick={() => open({ kind: 'saveFolderToPhone', folderId, dir, name })}>
          <span className="hidden sm:inline">{t('devices.saveToPhone')}</span>
        </Button>
      ) : (
        <Button variant="ghost" size="sm" className="touch-target" aria-label={t('gallery.downloadFolder')} icon={<Download className="size-4" />} onClick={() => download(backend.folderDownloadUrl(folderId, dir, name))}>
          <span className="hidden lg:inline">{t('gallery.downloadFolder')}</span>
        </Button>
      )}
    </>
  );
}

/** Context menu entries for a sub folder card (download / save on the phone / copy to the PC). */
export function FolderMenuItems({ folderId, dir, name }: { folderId: string; dir: string; name: string }) {
  const t = useT();
  const backend = useBackend();
  const info = useInfo();
  const dev = useDeviceFolder(folderId);
  const open = useDeviceUi((s) => s.open);
  const phone = isPhoneClient(backend);
  return (
    <>
      {phone ? (
        <MenuItem icon={<HardDriveDownload />} onClick={() => open({ kind: 'saveFolderToPhone', folderId, dir, name })}>
          {t('devices.saveToPhone')}
        </MenuItem>
      ) : (
        <MenuItem icon={<Download />} onClick={() => download(backend.folderDownloadUrl(folderId, dir, name))}>
          {t('gallery.downloadFolder')}
        </MenuItem>
      )}
      {dev && !phone && info && !info.caps.readonly && dev.device.online && (
        <MenuItem icon={<MonitorDown />} onClick={() => open({ kind: 'copyToPc', refs: [{ folderId, rel: dir }], label: name })}>
          {t('devices.copyToPc')}
        </MenuItem>
      )}
    </>
  );
}
