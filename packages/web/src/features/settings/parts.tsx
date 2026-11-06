import type { ReactNode } from 'react';
import type { MessageKey } from '@/i18n';
import { useT } from '@/i18n';
import { Monitor, Server } from 'lucide-react';
import { bothLangs, matchesQuery, SettingsQuery, useSettingsQuery } from './search';

/**
 * Building blocks of the settings page. Search: a Row renders nothing when it does not match the
 * query (labels are matched in both languages + keywords); a Card / Section whose title matches shows
 * everything inside. Cards / sections without a visible row are hidden by CSS (settings.css).
 */

export function Section({ id, title, icon, children }: { id: string; title: MessageKey; icon: ReactNode; children: ReactNode }) {
  const t = useT();
  const q = useSettingsQuery();
  const all = !!q && matchesQuery(bothLangs([title]), q);
  return (
    <section id={`settings-${id}`} data-section={id} className="settings-section flex scroll-mt-32 flex-col gap-4 @3xl:scroll-mt-6">
      <h2 className="flex items-center gap-2.5 px-1 font-display text-xl font-bold tracking-tight">
        <span className="grid size-8 place-items-center rounded-xl bg-[color-mix(in_oklab,var(--accent)_16%,transparent)] text-accent [&>svg]:size-4">{icon}</span>
        {t(title)}
      </h2>
      <SettingsQuery.Provider value={all ? '' : q}>{children}</SettingsQuery.Provider>
    </section>
  );
}

/** where a card's values live (per device vs on the server): a small badge with an explanation */
function ScopeBadge({ scope }: { scope: 'device' | 'server' }) {
  const t = useT();
  return (
    <span
      className="ml-auto inline-flex items-center gap-1 rounded-full border border-line px-2 py-0.5 text-[11px] font-medium text-faint [&>svg]:size-3"
      title={t(scope === 'device' ? 'pref.deviceOnlyHint' : 'pref.serverWideHint')}
    >
      {scope === 'device' ? <Monitor /> : <Server />}
      {t(scope === 'device' ? 'pref.deviceOnly' : 'pref.serverWide')}
    </span>
  );
}

export function Card({
  icon,
  title,
  titleKey,
  scope,
  children,
  testId,
}: {
  icon?: ReactNode;
  title: ReactNode;
  /** label key of the title (search matches it in both languages) */
  titleKey?: MessageKey;
  scope?: 'device' | 'server';
  children: ReactNode;
  testId?: string;
}) {
  const q = useSettingsQuery();
  const all = !!q && matchesQuery(`${bothLangs([titleKey])} ${typeof title === 'string' ? title : ''}`, q);
  return (
    <div data-card data-testid={testId} className="rounded-3xl border border-line bg-elev p-5 sm:p-6">
      <h3 className="mb-4 flex items-center gap-2.5 font-display text-[17px] font-semibold">
        {icon && <span className="grid size-8 place-items-center rounded-xl bg-surface-2 text-accent [&>svg]:size-4">{icon}</span>}
        {title}
        {scope && <ScopeBadge scope={scope} />}
      </h3>
      <div className="flex flex-col divide-y divide-line">
        <SettingsQuery.Provider value={all ? '' : q}>{children}</SettingsQuery.Provider>
      </div>
    </div>
  );
}

export function Row({
  label,
  k,
  hint,
  kw,
  children,
  setting,
  stack,
}: {
  label: ReactNode;
  /** label key (searched in TR + EN) */
  k?: MessageKey;
  hint?: ReactNode;
  /** extra search words */
  kw?: string;
  children?: ReactNode;
  /** data-setting attribute (tests / screenshots) */
  setting?: string;
  /** control below the label (wide controls on phones) */
  stack?: boolean;
}) {
  const q = useSettingsQuery();
  if (q) {
    const text = `${bothLangs([k])} ${typeof label === 'string' ? label : ''} ${typeof hint === 'string' ? hint : ''} ${kw ?? ''}`;
    if (!matchesQuery(text, q)) return null;
  }
  return (
    <div data-row data-setting={setting} className="flex flex-wrap items-center justify-between gap-x-6 gap-y-2 py-3.5 first:pt-0 last:pb-0">
      <div className={stack ? 'w-full' : 'min-w-0 flex-1 basis-52'}>
        <div className="text-sm font-medium">{label}</div>
        {hint && <div className="mt-0.5 text-xs text-faint">{hint}</div>}
      </div>
      {children !== undefined && <div className={stack ? 'flex w-full flex-wrap items-center gap-2' : 'flex shrink-0 flex-wrap items-center gap-2'}>{children}</div>}
    </div>
  );
}

/**
 * A self-contained card from another feature (explore, devices…): searchable only as a whole,
 * by its title keys + keywords. Renders nothing visible when the card itself renders nothing.
 */
export function Opaque({ keys, kw, children }: { keys: MessageKey[]; kw?: string; children: ReactNode }) {
  const q = useSettingsQuery();
  if (q && !matchesQuery(`${bothLangs(keys)} ${kw ?? ''}`, q)) return null;
  return (
    <div data-opaque className="contents">
      <SettingsQuery.Provider value="">{children}</SettingsQuery.Provider>
    </div>
  );
}

/** native <select> styled like the other controls (long option lists, numbers) */
export function Select<T extends string | number>({
  value,
  options,
  onChange,
  label,
  disabled,
  setting,
}: {
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
  label: string;
  disabled?: boolean;
  setting?: string;
}) {
  return (
    <select
      aria-label={label}
      value={String(value)}
      disabled={disabled}
      data-setting={setting}
      onChange={(e) => {
        const o = options.find((x) => String(x.value) === e.target.value);
        if (o) onChange(o.value);
      }}
      className="h-9 max-w-full rounded-xl border border-line bg-surface px-3 text-sm text-fg disabled:opacity-50"
    >
      {options.map((o) => (
        <option key={String(o.value)} value={String(o.value)}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

/** custom row content (lists, QR codes…): shown when its text or keys match the query; children carry data-row */
export function Searchable({ text = '', keys = [], children }: { text?: string; keys?: MessageKey[]; children: ReactNode }) {
  const q = useSettingsQuery();
  if (q && !matchesQuery(`${bothLangs(keys)} ${text}`, q)) return null;
  return <>{children}</>;
}
