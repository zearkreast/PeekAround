import { createContext, useContext } from 'react';
import { foldText } from '@peekaround/shared';
import { translate, type MessageKey } from '@/i18n';

/** every word of the query appears (folded: case, Turkish letters, accents) in the text */
export function matchesQuery(text: string, query: string): boolean {
  const words = foldText(query).split(/\s+/).filter(Boolean);
  if (!words.length) return true;
  const hay = foldText(text);
  return words.every((w) => hay.includes(w));
}

/** label text of keys in both languages, so a Turkish UI finds "thumbnail" and vice versa */
export function bothLangs(keys: (MessageKey | undefined)[]): string {
  return keys
    .filter((k): k is MessageKey => !!k)
    .map((k) => `${translate('tr', k)} ${translate('en', k)}`)
    .join(' ');
}

/** current settings search query ('' = no filter) */
export const SettingsQuery = createContext('');
export const useSettingsQuery = () => useContext(SettingsQuery);
