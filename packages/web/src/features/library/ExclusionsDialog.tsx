import { useEffect, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { Plus } from 'lucide-react';
import type { Library } from '@peekaround/shared';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { formatBytes } from '@/lib/format';
import { toast } from '@/store/toasts';
import { Button, Field, Modal, Segmented, Switch } from '@/components/ui';

const SIZES = [0, 10 * 1024, 30 * 1024, 100 * 1024];
/** common junk folders of phones / messengers / OS caches */
const SUGGESTIONS = ['Thumbnails', '.thumbnails', 'WhatsApp Stickers', 'Telegram Stickers', 'cache', '*.tmp', '@eaDir'];

/** Per-library exclusions: folder names / glob patterns, minimum photo size, hidden folders. */
export function ExclusionsDialog({ lib, onClose }: { lib: Library | null; onClose: () => void }) {
  const t = useT();
  const backend = useBackend();
  const qc = useQueryClient();
  const [text, setText] = useState('');
  const [minBytes, setMinBytes] = useState(0);
  const [hidden, setHidden] = useState<boolean | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!lib) return;
    setText((lib.options?.exclude ?? []).join('\n'));
    setMinBytes(lib.options?.minBytes ?? 0);
    setHidden(lib.options?.showHidden ?? null);
  }, [lib]);

  const lines = text
    .split('\n')
    .map((s) => s.trim())
    .filter(Boolean);
  const sizeOptions = SIZES.includes(minBytes) ? SIZES : [...SIZES, minBytes].sort((a, b) => a - b);

  const save = async () => {
    if (!lib) return;
    setBusy(true);
    try {
      await backend.updateLibrary(lib.id, { options: { exclude: lines, minBytes, showHidden: hidden } });
      await qc.invalidateQueries({ queryKey: ['libraries'] });
      toast.success(t('lib2.ex.saved'));
      onClose();
    } catch (e) {
      toast.error(t('common.error'), (e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open={!!lib}
      onClose={onClose}
      title={lib ? t('lib2.ex.title', { name: lib.name }) : ''}
      width={540}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" loading={busy} onClick={() => void save()}>
            {t('common.save')}
          </Button>
        </>
      }
    >
      <p className="mb-4 text-sm text-dim">{t('lib2.ex.body')}</p>
      <Field label={t('lib2.ex.patterns')} hint={t('lib2.ex.patternsHint')}>
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          rows={5}
          spellCheck={false}
          placeholder={t('lib2.ex.placeholder')}
          className="w-full resize-y rounded-xl border border-line bg-surface px-3 py-2 font-mono text-[13px] text-fg outline-none placeholder:text-faint focus:border-[var(--accent)]"
          data-testid="exclusions-text"
        />
      </Field>
      <div className="mt-2 flex flex-wrap items-center gap-1.5">
        <span className="mr-1 text-xs text-faint">{t('lib2.ex.suggest')}</span>
        {SUGGESTIONS.filter((s) => !lines.includes(s)).map((s) => (
          <button
            key={s}
            type="button"
            onClick={() => setText((v) => (v.trim() ? `${v.trim()}\n${s}` : s))}
            className="inline-flex h-7 items-center gap-1 rounded-full border border-line bg-surface px-2.5 font-mono text-[11.5px] text-dim transition-colors hover:border-[var(--accent)] hover:text-fg"
          >
            <Plus className="size-3" />
            {s}
          </button>
        ))}
      </div>
      <div className="mt-5">
        <Field label={t('lib2.ex.minSize')} hint={t('lib2.ex.minSizeHint')}>
          <Segmented<string>
            size="sm"
            value={String(minBytes)}
            onChange={(v) => setMinBytes(Number(v))}
            options={sizeOptions.map((b) => ({ value: String(b), label: b ? formatBytes(b) : t('lib2.ex.off') }))}
          />
        </Field>
      </div>
      <div className="mt-5 flex items-start justify-between gap-4 rounded-xl border border-line bg-surface px-3.5 py-3">
        <div className="min-w-0">
          <div className="text-sm font-medium">{t('lib2.ex.hidden')}</div>
          <div className="text-xs text-dim">{hidden === null ? t('lib2.ex.hiddenDefault') : t('lib2.ex.hiddenHint')}</div>
        </div>
        <div className="flex items-center gap-2">
          {hidden !== null && (
            <button type="button" className="text-xs text-faint hover:text-fg" onClick={() => setHidden(null)}>
              {t('common.reset')}
            </button>
          )}
          <Switch checked={!!hidden} onChange={(v) => setHidden(v)} />
        </div>
      </div>
    </Modal>
  );
}
