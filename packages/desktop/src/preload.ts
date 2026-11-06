import { contextBridge, ipcRenderer, webUtils, type IpcRendererEvent } from 'electron';

/** Bridge for the web UI (types: packages/web/src/backend/desktop.ts). Every call is validated in main. */
contextBridge.exposeInMainWorld('peekDesktop', {
  platform: process.platform,
  version: process.versions.electron,
  pickFolders: (): Promise<string[]> => ipcRenderer.invoke('peek:pick-folders'),
  getPathForFile: (file: File): string => {
    try {
      return webUtils.getPathForFile(file);
    } catch {
      return '';
    }
  },
  showInFolder: (p: string) => ipcRenderer.invoke('peek:show-in-folder', p),
  openExternal: (url: string) => ipcRenderer.invoke('peek:open-external', url),
  setLan: (enabled: boolean, port: number) => ipcRenderer.invoke('peek:set-lan', enabled, port),
  getLan: () => ipcRenderer.invoke('peek:get-lan'),
  setTheme: (dark: boolean) => ipcRenderer.send('peek:theme', dark),
  window: {
    minimize: () => ipcRenderer.send('peek:window', 'minimize'),
    toggleMaximize: () => ipcRenderer.send('peek:window', 'maximize'),
    close: () => ipcRenderer.send('peek:window', 'close'),
  },
  // ── shell settings ──
  getShell: () => ipcRenderer.invoke('peek:shell-get'),
  setShell: (patch: Record<string, unknown>) => ipcRenderer.invoke('peek:shell-set', patch),
  restart: () => ipcRenderer.send('peek:restart'),
  checkUpdate: () => ipcRenderer.invoke('peek:update-check'),
  onUpdate: (cb: (u: unknown) => void) => {
    const on = (_e: IpcRendererEvent, u: unknown) => cb(u);
    ipcRenderer.on('peek:update', on);
    // a release found before this page subscribed (daily check at startup)
    void ipcRenderer.invoke('peek:update-pending').then((u) => u && cb(u));
    return () => {
      ipcRenderer.removeListener('peek:update', on);
    };
  },
  openPath: (which: 'data' | 'logs') => ipcRenderer.invoke('peek:open-path', which),
  setLang: (lang: 'tr' | 'en') => ipcRenderer.send('peek:lang', lang),
  /** native OS drag of local media files (ids are resolved to paths in main) */
  startDrag: (ids: string[]) => ipcRenderer.send('peek:start-drag', ids),
});
