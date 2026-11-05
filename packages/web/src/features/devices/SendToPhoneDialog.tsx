import { useState } from 'react';
import { ArrowLeft, ChevronRight, Folder, Smartphone } from 'lucide-react';
import { useT } from '@/i18n';
import { cn } from '@/lib/cn';
import { useBrowse } from '@/lib/queries';
import { Button, Modal, Spinner } from '@/components/ui';
import { useLinkedDevices } from './useDevices';

interface Step {
  folderId: string;
  dir: string;
  label: string;
}

/** Pick a linked phone → one of its shared folders → (optional) sub folder. */
export function SendToPhoneDialog({ open, label, onClose, onPick }: { open: boolean; label: string; onClose: () => void; onPick: (t: { folderId: string; dir: string; label: string }) => void }) {
  const t = useT();
  const devices = useLinkedDevices();
  const [stack, setStack] = useState<Step[]>([]);
  const cur = stack[stack.length - 1] ?? null;
  const browse = useBrowse({ folderId: cur?.folderId, dir: cur?.dir, limit: 1 }, !!cur);
  const close = () => {
    setStack([]);
    onClose();
  };
  return (
    <Modal
      open={open}
      onClose={close}
      title={t('devices.send.title', { what: label })}
      width={480}
      footer={
        <>
          <Button variant="ghost" onClick={close}>
            {t('common.cancel')}
          </Button>
          <Button
            variant="primary"
            disabled={!cur}
            onClick={() => {
              if (!cur) return;
              onPick(cur);
              setStack([]);
            }}
          >
            {t('devices.send.confirm')}
          </Button>
        </>
      }
    >
      <div className="mb-3 flex items-center gap-2">
        <button type="button" disabled={!stack.length} onClick={() => setStack((s) => s.slice(0, -1))} className="grid size-9 place-items-center rounded-lg text-dim hover:bg-surface-2 disabled:opacity-30" aria-label={t('common.back')}>
          <ArrowLeft className="size-4" />
        </button>
        <div className="min-w-0 flex-1 truncate text-sm font-medium">{stack.map((s) => s.label).join(' / ') || t('devices.send.pick')}</div>
      </div>
      <div className="h-80 overflow-auto rounded-2xl border border-line bg-surface p-1">
        {!cur &&
          devices.map((d) => {
            const usable = d.online && d.caps?.receive !== false;
            return (
              <div key={d.id} className="mb-1">
                <div className="flex items-center gap-2 px-3 pt-2 pb-1 text-xs font-semibold tracking-wide text-faint uppercase">
                  <Smartphone className="size-3.5" />
                  <span className="truncate">{d.name}</span>
                  <span className={cn('size-1.5 rounded-full', d.online ? 'bg-ok' : 'bg-faint')} />
                  {!d.online && <span className="font-normal normal-case">{t('devices.offline')}</span>}
                  {d.online && d.caps?.receive === false && <span className="font-normal normal-case">{t('devices.noReceive')}</span>}
                </div>
                {d.folders.map((f) => (
                  <Row key={f.id} disabled={!usable} label={f.name} onClick={() => setStack([{ folderId: f.id, dir: '', label: `${d.name} · ${f.name}` }])} />
                ))}
              </div>
            );
          })}
        {!cur && !devices.length && <p className="p-4 text-center text-sm text-faint">{t('devices.none')}</p>}
        {cur && browse.isLoading && (
          <div className="grid h-40 place-items-center">
            <Spinner />
          </div>
        )}
        {cur && browse.first?.folders.map((f) => <Row key={f.dir} label={f.name} onClick={() => setStack((s) => [...s, { folderId: cur.folderId, dir: f.dir, label: f.name }])} />)}
        {cur && !browse.isLoading && !browse.first?.folders.length && <p className="p-4 text-center text-sm text-faint">{t('devices.send.here')}</p>}
      </div>
    </Modal>
  );
}

function Row({ label, onClick, disabled }: { label: string; onClick: () => void; disabled?: boolean }) {
  return (
    <button type="button" disabled={disabled} onClick={onClick} className="flex min-h-11 w-full items-center gap-2.5 rounded-xl px-3 py-2 text-left text-sm hover:bg-surface-2 disabled:opacity-40">
      <Folder className="size-4 text-accent" />
      <span className="min-w-0 flex-1 truncate">{label}</span>
      <ChevronRight className="size-4 text-faint" />
    </button>
  );
}
