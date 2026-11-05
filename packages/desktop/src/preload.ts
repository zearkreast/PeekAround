import { contextBridge, ipcRenderer, webUtils } from 'electron';

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
});
