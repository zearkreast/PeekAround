import { useState } from 'react';
import { motion } from 'motion/react';
import { useQueryClient } from '@tanstack/react-query';
import { Check, Smartphone, X } from 'lucide-react';
import type { DeviceInfo } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { toast } from '@/store/toasts';
import { Button, Modal } from '@/components/ui';
import { useDeviceUi } from './useDevices';

/** "📱 Pixel 8 wants to connect — code 4821": approve / reject (PC window). */
export function PairingDialog() {
  const t = useT();
  const backend = useBackend();
  const qc = useQueryClient();
  const pairs = useDeviceUi((s) => s.pairs);
  const drop = useDeviceUi((s) => s.dropPair);
  const [busy, setBusy] = useState<'approve' | 'reject' | null>(null);
  const d: DeviceInfo | undefined = pairs[0];

  const decide = async (approve: boolean) => {
    if (!d || !backend.pairDevice) return;
    setBusy(approve ? 'approve' : 'reject');
    try {
      await backend.pairDevice(d.id, approve);
      drop(d.id);
      if (approve) toast.success(t('devices.pair.approved', { name: d.name }), t('devices.pair.approvedBody'));
      void qc.invalidateQueries({ queryKey: ['devices'] });
      void qc.invalidateQueries({ queryKey: ['libraries'] });
    } catch (e) {
      toast.error(t('common.error'), (e as Error).message);
      drop(d.id);
    } finally {
      setBusy(null);
    }
  };

  return (
    <Modal open={!!d} onClose={() => d && drop(d.id, true)} width={440}>
      {d && (
        <div className="flex flex-col items-center pt-4 pb-2 text-center" data-testid="pairing-dialog">
          <motion.div
            initial={{ scale: 0.6, rotate: -10, opacity: 0 }}
            animate={{ scale: 1, rotate: 0, opacity: 1 }}
            transition={{ type: 'spring', stiffness: 300, damping: 16 }}
            className="mb-4 grid size-16 place-items-center rounded-2xl bg-accent-gradient text-white shadow-[0_12px_30px_-10px_var(--accent)]"
          >
            <Smartphone className="size-8" />
          </motion.div>
          <h2 className="font-display text-xl font-semibold">{t('devices.pair.title', { name: d.name })}</h2>
          {d.model && d.model !== d.name && <p className="mt-0.5 text-sm text-faint">{d.model}</p>}
          <p className="mt-3 max-w-sm text-sm text-dim">{t('devices.pair.body')}</p>
          {d.code && (
            <div className="mt-5 flex gap-2" aria-label={t('devices.pair.code')}>
              {d.code.split('').map((c, i) => (
                <motion.span
                  key={i}
                  initial={{ y: 10, opacity: 0 }}
                  animate={{ y: 0, opacity: 1 }}
                  transition={{ delay: 0.1 + i * 0.05 }}
                  className="grid h-14 w-11 place-items-center rounded-xl border border-line bg-surface font-display text-3xl font-bold tabular-nums"
                >
                  {c}
                </motion.span>
              ))}
            </div>
          )}
          <p className="mt-4 text-xs text-faint">{t('devices.pair.hint')}</p>
          <div className="mt-6 flex w-full gap-2.5 pb-2">
            <Button variant="secondary" size="lg" className="flex-1" icon={<X className="size-4" />} loading={busy === 'reject'} disabled={!!busy} onClick={() => decide(false)}>
              {t('devices.pair.reject')}
            </Button>
            <Button variant="primary" size="lg" className="flex-1" icon={<Check className="size-4" />} loading={busy === 'approve'} disabled={!!busy} onClick={() => decide(true)}>
              {t('devices.pair.approve')}
            </Button>
          </div>
        </div>
      )}
    </Modal>
  );
}
