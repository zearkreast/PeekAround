/** Bridge exposed by the Electron preload script (packages/desktop/src/preload.ts). */
export interface DesktopBridge {
  platform: 'win32' | 'linux' | 'darwin' | string;
  version: string;
  pickFolders(): Promise<string[]>;
  getPathForFile(file: File): string;
  showInFolder(path: string): Promise<void>;
  openExternal(url: string): Promise<void>;
  setLan(enabled: boolean, port: number): Promise<{ enabled: boolean; urls: string[]; error?: string }>;
  setTheme?(dark: boolean): void;
  getLan(): Promise<{ enabled: boolean; port: number; urls: string[] }>;
  window: {
    minimize(): void;
    toggleMaximize(): void;
    close(): void;
  };
  // ── shell settings (1.2; older shells lack them → their UI is hidden) ──
  getShell?(): Promise<DesktopShellState>;
  setShell?(patch: Partial<DesktopShellPrefs>): Promise<DesktopShellState>;
  /** relaunch the app (hardware acceleration change) */
  restart?(): void;
  checkUpdate?(): Promise<UpdateInfo>;
  /** main process found a newer release (daily check); returns an unsubscribe function */
  onUpdate?(cb: (u: UpdateInfo) => void): () => void;
  /** open the data or log folder in the file manager; resolves with an error message or '' */
  openPath?(which: 'data' | 'logs'): Promise<string>;
  /** UI language for tray menu / notifications */
  setLang?(lang: 'tr' | 'en'): void;
  /** native drag of local media files out of the window (main resolves the ids to paths) */
  startDrag?(ids: string[]): void;
}

export type CloseAction = 'auto' | 'tray' | 'quit';

export interface DesktopShellPrefs {
  startAtLogin: boolean;
  startHidden: boolean;
  closeAction: CloseAction;
  /** GPU acceleration of the interface (applies after a restart) */
  gpu: boolean;
  /** native notification when a long job ends while the window is unfocused */
  notify: boolean;
  /** check GitHub releases once a day */
  autoUpdate: boolean;
}

export interface UpdateInfo {
  current: string;
  latest: string | null;
  newer: boolean;
  url: string | null;
  checkedAt: number;
  error?: string;
}

export interface DesktopShellState extends DesktopShellPrefs {
  /** the OS supports "open at login" for this build (not on Linux) */
  loginSupported: boolean;
  /** GPU acceleration state of the running process */
  gpuActive: boolean;
  version: string;
  lastUpdate: UpdateInfo | null;
}

declare global {
  interface Window {
    peekDesktop?: DesktopBridge;
  }
}

export const desktop: DesktopBridge | null = typeof window !== 'undefined' ? (window.peekDesktop ?? null) : null;
