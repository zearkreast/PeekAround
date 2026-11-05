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
}

declare global {
  interface Window {
    peekDesktop?: DesktopBridge;
  }
}

export const desktop: DesktopBridge | null = typeof window !== 'undefined' ? (window.peekDesktop ?? null) : null;
