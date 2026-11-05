import { useMemo, useRef, useState } from 'react';
import { ChevronDown, Search, Upload } from 'lucide-react';
import { useQueryClient } from '@tanstack/react-query';
import { useBackend } from '@/backend';
import { useT } from '@/i18n';
import { useFonts, type FontFamily } from '@/lib/fonts';
import { toast } from '@/store/toasts';
import { cn } from '@/lib/cn';
import { Input, MenuLabel, Popover, Spinner } from './ui';

/** Font family picker with live previews, search and inline upload of custom fonts. */
export function FontPicker({ value, onChange, className }: { value: string; onChange: (family: string, f: FontFamily) => void; className?: string }) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState('');
  const anchor = useRef<HTMLButtonElement>(null);
  const { families, custom } = useFonts();
  const backend = useBackend();
  const qc = useQueryClient();
  const [uploading, setUploading] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);

  const current = families.find((f) => f.family === value);
  const filtered = useMemo(() => {
    const s = q.trim().toLowerCase();
    return s ? families.filter((f) => f.label.toLowerCase().includes(s)) : families;
  }, [families, q]);
  const customSet = new Set(custom.map((c) => c.family));

  const upload = async (files: FileList | null) => {
    if (!files?.length) return;
    setUploading(true);
    try {
      for (const f of Array.from(files)) await backend.uploadFont(f);
      await qc.invalidateQueries({ queryKey: ['fonts'] });
      toast.success(t('fonts.added'));
    } catch (e) {
      toast.error(t('common.error'), (e as Error).message);
    } finally {
      setUploading(false);
    }
  };

  const row = (f: FontFamily) => (
    <button
      key={f.family}
      type="button"
      onClick={() => {
        onChange(f.family, f);
        setOpen(false);
      }}
      className={cn('flex w-full items-center justify-between gap-3 rounded-xl px-3 py-2 text-left hover:bg-surface-2', f.family === value && 'bg-surface-2')}
    >
      <span className="truncate text-[17px] leading-tight" style={{ fontFamily: `"${f.family}"` }}>
        {f.label}
      </span>
      <span className="shrink-0 text-[11px] text-faint">{f.weights.length > 1 ? `${f.weights.length}` : ''}</span>
    </button>
  );

  return (
    <>
      <button
        ref={anchor}
        type="button"
        onClick={() => setOpen((o) => !o)}
        className={cn('flex h-10 w-full items-center justify-between gap-2 rounded-xl border border-line bg-surface px-3 text-left hover:bg-surface-2', className)}
      >
        <span className="truncate text-[15px]" style={{ fontFamily: `"${value}"` }}>
          {current?.label ?? value}
        </span>
        <ChevronDown className="size-4 shrink-0 text-faint" />
      </button>
      <Popover open={open} onClose={() => setOpen(false)} anchor={anchor} className="w-72">
        <div className="flex items-center gap-1.5 p-1">
          <Input autoFocus value={q} onChange={(e) => setQ(e.target.value)} placeholder={t('fonts.search')} icon={<Search />} className="flex-1" />
          <button
            type="button"
            title={t('fonts.upload')}
            onClick={() => fileRef.current?.click()}
            className="grid size-10 shrink-0 place-items-center rounded-xl border border-line bg-surface text-dim hover:bg-surface-2 hover:text-fg"
          >
            {uploading ? <Spinner className="size-4" /> : <Upload className="size-4" />}
          </button>
          <input ref={fileRef} type="file" hidden multiple accept=".ttf,.otf,.woff,.woff2" onChange={(e) => void upload(e.target.files)} />
        </div>
        <div className="max-h-80 overflow-auto p-0.5">
          {filtered.some((f) => customSet.has(f.family)) && <MenuLabel>{t('fonts.custom')}</MenuLabel>}
          {filtered.filter((f) => customSet.has(f.family)).map(row)}
          <MenuLabel>{t('fonts.bundled')}</MenuLabel>
          {filtered.filter((f) => !customSet.has(f.family)).map(row)}
        </div>
      </Popover>
    </>
  );
}
