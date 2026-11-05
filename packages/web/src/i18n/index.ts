import { useCallback } from 'react';
import { usePrefs, type Lang } from '@/store/prefs';
import tr, { type MessageKey } from './tr';
import en from './en';

const dicts: Record<Lang, Record<string, string>> = { tr, en };

export type { MessageKey };

export function translate(lang: Lang, key: MessageKey, vars?: Record<string, string | number>): string {
  let s = dicts[lang]?.[key] ?? dicts.tr[key] ?? key;
  // "{n} item|{n} items" → singular / plural by n
  if (s.includes('|') && vars && 'n' in vars) {
    const [one, many] = s.split('|');
    s = Number(String(vars.n).replace(/[^\d.-]/g, '')) === 1 ? one : many;
  }
  if (vars) for (const [k, v] of Object.entries(vars)) s = s.replaceAll(`{${k}}`, String(v));
  return s;
}

/** const t = useT(); t('nav.home') */
export function useT() {
  const lang = usePrefs((s) => s.lang);
  return useCallback((key: MessageKey, vars?: Record<string, string | number>) => translate(lang, key, vars), [lang]);
}

export function useLang(): Lang {
  return usePrefs((s) => s.lang);
}

export function locale(lang: Lang) {
  return lang === 'tr' ? 'tr-TR' : 'en-US';
}
