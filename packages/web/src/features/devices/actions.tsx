/**
 * Media actions for linked phones (registered into the gallery action registry):
 *  - "PC'ye kopyala"  on items that live on a phone (desktop window / server web UI)
 *  - "Telefona gönder" on PC items when a paired phone that accepts files is online
 *  - "Telefona kaydet" in the Android app (client mode): native download into a phone folder
 */
import { HardDriveDownload, MonitorDown, Smartphone } from 'lucide-react';
import type { MediaItem } from '@peekaround/shared';
import { isPhoneClient, onDevice, refOf, registerMediaAction, type ActionCtx } from '@/features/gallery/actions';
import { queryClient } from '@/lib/queryClient';
import type { DeviceInfo } from '@peekaround/shared';
import { useDeviceUi } from './useDevices';

const label = (items: MediaItem[], c: ActionCtx) => (items.length === 1 ? items[0].name : c.t('devices.nItems', { n: items.length }));

/** paired phones with receive caps that are online right now (from the libraries cache) */
function receivers(c: ActionCtx): DeviceInfo[] {
  const libs = queryClient.getQueryData<{ devices?: DeviceInfo[] }>(['libraries', c.backend.id]);
  return (libs?.devices ?? []).filter((d) => d.online && d.paired !== false && d.caps?.receive !== false && d.folders.length > 0);
}

registerMediaAction({
  id: 'copy-to-pc',
  icon: <MonitorDown />,
  labelKey: 'devices.copyToPc',
  shortLabelKey: 'devices.copyToPcShort',
  group: 'primary',
  order: 12,
  primary: true,
  clearsSelection: false,
  visible: (items, c) => c.writable && items.length > 0 && items.every((i) => !!c.deviceOf(i.folderId)) && !isPhoneClient(c),
  run: (items, c) => {
    if (items.some((i) => c.deviceOf(i.folderId)?.online === false)) {
      c.toast.error(c.t('devices.offlineTip'));
      return;
    }
    useDeviceUi.getState().open({ kind: 'copyToPc', refs: items.map(refOf), label: label(items, c) });
  },
});

registerMediaAction({
  id: 'send-to-phone',
  icon: <Smartphone />,
  labelKey: 'devices.sendToPhone',
  group: 'share',
  order: 30,
  visible: (items, c) => c.backend.kind === 'http' && c.writable && items.length > 0 && !onDevice(items, c) && !c.isTemp(items[0].folderId) && receivers(c).length > 0,
  run: (items, c) => useDeviceUi.getState().open({ kind: 'sendToPhone', refs: items.map(refOf), label: label(items, c) }),
});

registerMediaAction({
  id: 'save-to-phone',
  icon: <HardDriveDownload />,
  labelKey: 'devices.saveToPhone',
  shortLabelKey: 'devices.saveToPhoneShort',
  group: 'primary',
  order: 11,
  primary: true,
  visible: (items, c) => isPhoneClient(c) && items.length > 0,
  run: (items) => useDeviceUi.getState().open({ kind: 'saveToPhone', items }),
});
