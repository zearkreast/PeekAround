/**
 * Desktop shell helpers without window logic: persisted shell preferences (desktop.json), a tiny
 * rotating log file, and the GitHub release check. Kept free of heavy imports so the main process
 * starts fast.
 */
import { app } from 'electron';
import fs from 'node:fs';
import path from 'node:path';

export type CloseAction = 'auto' | 'tray' | 'quit';

export interface DesktopPrefs {
  lan: { enabled: boolean; port: number };
  /** fixed local port so the UI origin (and its saved preferences) stays the same between launches */
  port?: number;
  bounds?: { width: number; height: number; x?: number; y?: number; maximized?: boolean; fullscreen?: boolean };
  /** page zoom of the window (Ctrl + / −) */
  zoom?: number;
  startAtLogin: boolean;
  /** at a login launch: tray only, no window */
  startHidden: boolean;
  closeAction: CloseAction;
  /** GPU acceleration of the interface (read before app ready → restart to apply) */
  gpu: boolean;
  notify: boolean;
  autoUpdate: boolean;
  lastUpdateCheck?: number;
  /** newest version the user was already told about (one notice per version) */
  notifiedVersion?: string;
  /** tray / notification language (set by the renderer) */
  lang?: 'tr' | 'en';
  /** result of the last release check (settings page: "last checked") */
  lastUpdate?: UpdateInfo;
  /** the "still running in the tray" notice was shown once */
  trayHintShown?: boolean;
}

export const DEFAULT_DESKTOP_PREFS: DesktopPrefs = {
  lan: { enabled: false, port: 8080 },
  startAtLogin: false,
  startHidden: false,
  closeAction: 'auto',
  gpu: true,
  notify: true,
  autoUpdate: true,
};

const prefsFile = () => path.join(app.getPath('userData'), 'desktop.json');

export function loadPrefs(): DesktopPrefs {
  try {
    const raw = JSON.parse(fs.readFileSync(prefsFile(), 'utf8')) as Partial<DesktopPrefs>;
    return { ...DEFAULT_DESKTOP_PREFS, ...raw, lan: { ...DEFAULT_DESKTOP_PREFS.lan, ...(raw.lan ?? {}) } };
  } catch {
    return { ...DEFAULT_DESKTOP_PREFS, lan: { ...DEFAULT_DESKTOP_PREFS.lan } };
  }
}

let saveTimer: NodeJS.Timeout | null = null;
export function savePrefs(p: DesktopPrefs, now = false) {
  const write = () => {
    saveTimer = null;
    try {
      fs.mkdirSync(path.dirname(prefsFile()), { recursive: true });
      const tmp = `${prefsFile()}.tmp`;
      fs.writeFileSync(tmp, JSON.stringify(p, null, 2));
      fs.renameSync(tmp, prefsFile());
    } catch {
      /* read-only profile: keep running with in-memory prefs */
    }
  };
  if (saveTimer) clearTimeout(saveTimer);
  if (now) write();
  else saveTimer = setTimeout(write, 300);
}

/** shell options the renderer may change (validated: unknown keys / wrong types are dropped) */
export function pickShellPatch(raw: unknown): Partial<Pick<DesktopPrefs, 'startAtLogin' | 'startHidden' | 'closeAction' | 'gpu' | 'notify' | 'autoUpdate'>> {
  if (!raw || typeof raw !== 'object') return {};
  const r = raw as Record<string, unknown>;
  const out: ReturnType<typeof pickShellPatch> = {};
  for (const k of ['startAtLogin', 'startHidden', 'gpu', 'notify', 'autoUpdate'] as const) if (typeof r[k] === 'boolean') out[k] = r[k] as boolean;
  if (r.closeAction === 'auto' || r.closeAction === 'tray' || r.closeAction === 'quit') out.closeAction = r.closeAction;
  return out;
}

// ───────────────────────── log ─────────────────────────

export function logDir(): string {
  return app.getPath('logs');
}

const MAX_LOG = 1024 * 1024;
export function log(...parts: unknown[]) {
  try {
    const dir = logDir();
    fs.mkdirSync(dir, { recursive: true });
    const f = path.join(dir, 'main.log');
    try {
      if (fs.statSync(f).size > MAX_LOG) fs.renameSync(f, path.join(dir, 'main.old.log'));
    } catch {
      /* no log yet */
    }
    const line = parts.map((p) => (p instanceof Error ? (p.stack ?? p.message) : typeof p === 'string' ? p : JSON.stringify(p))).join(' ');
    fs.appendFileSync(f, `${new Date().toISOString()} ${line}\n`);
  } catch {
    /* logging must never break the app */
  }
}

// ───────────────────────── updates ─────────────────────────

export const RELEASES_API = 'https://api.github.com/repos/zearkreast/PeakAround/releases/latest';
export const RELEASES_PAGE = 'https://github.com/zearkreast/PeakAround/releases/latest';

export interface UpdateInfo {
  current: string;
  latest: string | null;
  newer: boolean;
  url: string | null;
  checkedAt: number;
  error?: string;
}

/** compare "1.2.0", "v1.10.1", "1.2.0-beta.1" (a pre-release sorts before its release); <0, 0, >0 */
export function compareSemver(a: string, b: string): number {
  const parse = (v: string) => {
    const m = /^v?(\d+)(?:\.(\d+))?(?:\.(\d+))?(?:-([0-9a-z.-]+))?/i.exec(v.trim());
    return m ? { n: [Number(m[1]), Number(m[2] ?? 0), Number(m[3] ?? 0)], pre: m[4] ?? '' } : { n: [0, 0, 0], pre: '' };
  };
  const x = parse(a);
  const y = parse(b);
  for (let i = 0; i < 3; i++) if (x.n[i] !== y.n[i]) return x.n[i] - y.n[i];
  if (x.pre === y.pre) return 0;
  if (!x.pre) return 1;
  if (!y.pre) return -1;
  return x.pre.localeCompare(y.pre, 'en', { numeric: true });
}

/** latest GitHub release vs the running version; never downloads anything */
export async function checkForUpdate(fetchFn: typeof fetch, current: string): Promise<UpdateInfo> {
  const checkedAt = Date.now();
  try {
    const ctl = new AbortController();
    const tm = setTimeout(() => ctl.abort(), 15_000);
    let res: Response;
    try {
      res = await fetchFn(RELEASES_API, {
        headers: { Accept: 'application/vnd.github+json', 'User-Agent': `PeekAround/${current}` },
        signal: ctl.signal,
      });
    } finally {
      clearTimeout(tm);
    }
    if (res.status === 404) return { current, latest: null, newer: false, url: null, checkedAt };
    if (!res.ok) throw new Error(`GitHub ${res.status}`);
    const j = (await res.json()) as { tag_name?: string; html_url?: string; draft?: boolean; prerelease?: boolean };
    const latest = typeof j.tag_name === 'string' ? j.tag_name.replace(/^v/i, '') : null;
    const url = typeof j.html_url === 'string' && j.html_url.startsWith('https://github.com/') ? j.html_url : RELEASES_PAGE;
    const newer = !!latest && !j.draft && !j.prerelease && compareSemver(latest, current) > 0;
    return { current, latest, newer, url, checkedAt };
  } catch (e) {
    return { current, latest: null, newer: false, url: null, checkedAt, error: (e as Error).message };
  }
}

/** two-language strings of the shell (tray menu, notifications) */
export function L(lang: 'tr' | 'en' | undefined, tr: string, en: string): string {
  return lang === 'en' ? en : tr;
}
