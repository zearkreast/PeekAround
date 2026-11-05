import type { Library, Settings } from '@peekaround/shared';
import { DEFAULT_SETTINGS } from '@peekaround/shared';

/** How the Android app is connected. Persisted in localStorage. */
export type AndroidMode = { mode: 'local' } | { mode: 'server'; url: string; name?: string };

const MODE_KEY = 'peek.android.mode';
const LOCAL_KEY = 'peek.android.local';

export function loadMode(): AndroidMode | null {
  try {
    const raw = localStorage.getItem(MODE_KEY);
    return raw ? (JSON.parse(raw) as AndroidMode) : null;
  } catch {
    return null;
  }
}

export function saveMode(m: AndroidMode | null) {
  try {
    if (m) localStorage.setItem(MODE_KEY, JSON.stringify(m));
    else localStorage.removeItem(MODE_KEY);
  } catch {
    /* ignore */
  }
}

export interface LocalFolder {
  id: string;
  path: string;
  name: string;
}

export interface LocalTemp extends LocalFolder {
  sourceName: string;
  expiresAt: number;
}

export interface LocalFont {
  id: string;
  family: string;
  subfamily: string;
  weight: number;
  style: 'normal' | 'italic';
  format: 'truetype' | 'opentype' | 'woff' | 'woff2';
  path: string;
}

export interface LocalConfig {
  libraries: (Omit<Library, 'folders' | 'mediaCount'> & { folders: LocalFolder[] })[];
  temp: LocalTemp[];
  favorites: string[];
  fonts: LocalFont[];
  counts: Record<string, number>;
  settings: Settings;
  seq: number;
}

export function loadLocal(): LocalConfig {
  const empty: LocalConfig = { libraries: [], temp: [], favorites: [], fonts: [], counts: {}, settings: { ...DEFAULT_SETTINGS }, seq: 1 };
  try {
    const raw = localStorage.getItem(LOCAL_KEY);
    return raw ? { ...empty, ...JSON.parse(raw) } : empty;
  } catch {
    return empty;
  }
}

export function saveLocal(c: LocalConfig) {
  try {
    localStorage.setItem(LOCAL_KEY, JSON.stringify(c));
  } catch {
    /* ignore */
  }
}
