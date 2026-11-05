import { Cloud, Smartphone } from 'lucide-react';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { MenuItem } from '@/components/ui';
import { isPhoneClient, useDeviceUi } from '@/features/devices/useDevices';
import { useRemote } from '@/features/remote/RemoteHost';

/** Extra entries of the gallery "Add" menu: "Telefondan seç" (Android client) and "Linkten ekle" (MEGA). */
export function AddMenuExtras({ target, close }: { target: { folderId: string; dir: string }; close: () => void }) {
  const t = useT();
  const backend = useBackend();
  return (
    <>
      {isPhoneClient(backend) && (
        <MenuItem
          icon={<Smartphone />}
          onClick={() => {
            close();
            useDeviceUi.getState().open({ kind: 'uploadFromPhone', target });
          }}
        >
          {t('devices.pickFromPhone')}
        </MenuItem>
      )}
      {backend.remoteInspect && (
        <MenuItem
          icon={<Cloud />}
          onClick={() => {
            close();
            useRemote.getState().show(null);
          }}
        >
          {t('remote.addFromLink')}
        </MenuItem>
      )}
    </>
  );
}
