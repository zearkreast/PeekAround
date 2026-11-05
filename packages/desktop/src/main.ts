import { app, BrowserWindow, dialog, ipcMain, Menu, nativeImage, nativeTheme, shell, Tray } from 'electron';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import { createServer, type PeekServer } from '@peekaround/server/src/index';

const isDev = !app.isPackaged;
const here = __dirname;

/** Binaries inside app.asar must be read from app.asar.unpacked. */
function unpacked(p: string | null | undefined): string | undefined {
  if (!p) return undefined;
  return p.replace(`app.asar${path.sep}`, `app.asar.unpacked${path.sep}`);
}

function ffmpegBinaries() {
  let ffmpegPath: string | undefined;
  let ffprobePath: string | undefined;
  try {
    ffmpegPath = unpacked(require('ffmpeg-static') as string);
  } catch {
    /* fall back to PATH */
  }
  try {
    ffprobePath = unpacked((require('ffprobe-static') as { path: string }).path);
  } catch {
    /* fall back to PATH */
  }
  return {
    ffmpegPath: ffmpegPath && fs.existsSync(ffmpegPath) ? ffmpegPath : 'ffmpeg',
    ffprobePath: ffprobePath && fs.existsSync(ffprobePath) ? ffprobePath : 'ffprobe',
  };
}

interface DesktopPrefs {
  lan: { enabled: boolean; port: number };
  /** fixed local port so the UI origin (and its saved preferences) stays the same between launches */
  port?: number;
  bounds?: { width: number; height: number; x?: number; y?: number; maximized?: boolean };
}

const prefsFile = () => path.join(app.getPath('userData'), 'desktop.json');

function loadPrefs(): DesktopPrefs {
  try {
    return { lan: { enabled: false, port: 8080 }, ...JSON.parse(fs.readFileSync(prefsFile(), 'utf8')) };
  } catch {
    return { lan: { enabled: false, port: 8080 } };
  }
}

function savePrefs(p: DesktopPrefs) {
  try {
    fs.mkdirSync(path.dirname(prefsFile()), { recursive: true });
    fs.writeFileSync(prefsFile(), JSON.stringify(p, null, 2));
  } catch {
    /* ignore */
  }
}

let server: PeekServer | null = null;
let win: BrowserWindow | null = null;
let tray: Tray | null = null;
let prefs: DesktopPrefs;

function iconPath(): string | undefined {
  const candidates = [path.join(here, 'icon.png'), path.join(here, '..', 'build', 'icon.png')];
  return candidates.find((c) => fs.existsSync(c));
}

function portFree(port: number): Promise<boolean> {
  return new Promise((resolve) => {
    const s = net.createServer();
    s.once('error', () => resolve(false));
    s.listen(port, '127.0.0.1', () => s.close(() => resolve(true)));
  });
}

async function pickPort(): Promise<number> {
  for (const p of [prefs.port, 47213, 47214, 47215, 47216].filter((x): x is number => !!x)) {
    if (await portFree(p)) return p;
  }
  return 0;
}

async function startServer() {
  const { ffmpegPath, ffprobePath } = ffmpegBinaries();
  const dataDir = path.join(app.getPath('userData'), 'data');
  const firstRun = !fs.existsSync(path.join(dataDir, 'peekaround.db'));
  const roots: { path: string; name?: string }[] = [];
  if (firstRun) {
    for (const k of ['pictures', 'videos'] as const) {
      try {
        const p = app.getPath(k);
        if (p && fs.existsSync(p)) roots.push({ path: p });
      } catch {
        /* not available on this OS */
      }
    }
  }
  server = await createServer({
    mode: 'desktop',
    host: '127.0.0.1',
    port: await pickPort(),
    dataDir,
    roots,
    webDir: isDev && process.env.PEEK_WEB_DIR ? process.env.PEEK_WEB_DIR : path.join(here, 'web'),
    ffmpegPath,
    ffprobePath,
    mdns: true,
    version: app.getVersion(),
    hooks: {
      trash: async (p) => {
        await shell.trashItem(p);
      },
    },
  });
  if (prefs.port !== server.port) {
    prefs.port = server.port;
    savePrefs(prefs);
  }
  if (prefs.lan.enabled) {
    try {
      await server.setLan(true, prefs.lan.port);
    } catch {
      prefs.lan.enabled = false;
      savePrefs(prefs);
    }
  }
  updateTray();
}

function createWindow() {
  const b = prefs.bounds;
  const dark = nativeTheme.shouldUseDarkColors;
  win = new BrowserWindow({
    width: b?.width ?? 1400,
    height: b?.height ?? 900,
    x: b?.x,
    y: b?.y,
    minWidth: 880,
    minHeight: 560,
    show: false,
    backgroundColor: '#09090d',
    title: 'PeekAround',
    icon: iconPath(),
    titleBarStyle: process.platform === 'darwin' ? 'hiddenInset' : 'hidden',
    titleBarOverlay: process.platform === 'darwin' ? undefined : { color: '#00000000', symbolColor: dark ? '#a3a3b0' : '#555564', height: 59 },
    trafficLightPosition: { x: 18, y: 20 },
    webPreferences: {
      preload: path.join(here, 'preload.js'),
      contextIsolation: true,
      sandbox: true,
      nodeIntegration: false,
      spellcheck: false,
    },
  });
  if (b?.maximized) win.maximize();
  win.once('ready-to-show', () => win?.show());
  win.webContents.setWindowOpenHandler(({ url }) => {
    if (/^https?:/.test(url) && !url.startsWith(server!.url)) void shell.openExternal(url);
    return { action: 'deny' };
  });
  win.webContents.on('will-navigate', (e, url) => {
    if (!url.startsWith(server!.url) && !url.startsWith('http://localhost:5173')) {
      e.preventDefault();
      void shell.openExternal(url);
    }
  });
  const saveBounds = () => {
    if (!win) return;
    prefs.bounds = { ...win.getNormalBounds(), maximized: win.isMaximized() };
    savePrefs(prefs);
  };
  win.on('close', saveBounds);
  win.on('closed', () => (win = null));
  const url = isDev && process.env.PEEK_DEV_URL ? process.env.PEEK_DEV_URL : server!.url;
  void win.loadURL(url);
}

function updateTray() {
  if (!server) return;
  const lanOn = server.ctx.lan.enabled;
  if (!lanOn) {
    tray?.destroy();
    tray = null;
    return;
  }
  if (!tray) {
    const ic = iconPath();
    tray = new Tray(ic ? nativeImage.createFromPath(ic).resize({ width: 18, height: 18 }) : nativeImage.createEmpty());
    tray.setToolTip('PeekAround');
    tray.on('click', () => (win ? win.show() : createWindow()));
  }
  const urls = server.ctx.lan.enabled ? (require('@peekaround/server/src/config') as typeof import('@peekaround/server/src/config')).lanUrls(server.ctx.lan.port) : [];
  tray.setContextMenu(
    Menu.buildFromTemplate([
      { label: 'PeekAround', enabled: false },
      ...urls.map((u) => ({ label: u, click: () => void shell.openExternal(u) })),
      { type: 'separator' as const },
      { label: 'Show', click: () => (win ? win.show() : createWindow()) },
      { label: 'Quit', click: () => app.quit() },
    ]),
  );
}

function registerIpc() {
  ipcMain.handle('peek:pick-folders', async () => {
    const r = await dialog.showOpenDialog(win!, { properties: ['openDirectory', 'multiSelections', 'createDirectory'] });
    return r.canceled ? [] : r.filePaths;
  });
  ipcMain.handle('peek:show-in-folder', (_e, p: string) => shell.showItemInFolder(p));
  ipcMain.handle('peek:open-external', (_e, url: string) => {
    if (/^https?:\/\//.test(url)) return shell.openExternal(url);
  });
  ipcMain.handle('peek:get-lan', () => {
    const lan = server!.ctx.lan;
    const { lanUrls } = require('@peekaround/server/src/config') as typeof import('@peekaround/server/src/config');
    return { enabled: lan.enabled, port: prefs.lan.port, urls: lan.enabled ? lanUrls(lan.port) : [] };
  });
  ipcMain.handle('peek:set-lan', async (_e, enabled: boolean, port: number) => {
    try {
      const r = await server!.setLan(enabled, port);
      prefs.lan = { enabled: r.enabled, port };
      savePrefs(prefs);
      updateTray();
      return r;
    } catch (e) {
      return { enabled: false, urls: [], error: (e as Error).message };
    }
  });
  ipcMain.on('peek:window', (_e, action: 'minimize' | 'maximize' | 'close') => {
    if (!win) return;
    if (action === 'minimize') win.minimize();
    else if (action === 'maximize') (win.isMaximized() ? win.unmaximize() : win.maximize());
    else win.close();
  });
  ipcMain.on('peek:theme', (_e, dark: boolean) => {
    if (process.platform !== 'darwin' && win) {
      try {
        win.setTitleBarOverlay({ color: '#00000000', symbolColor: dark ? '#a3a3b0' : '#555564', height: 59 });
      } catch {
        /* not supported */
      }
    }
  });
}

if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  app.on('second-instance', () => {
    if (win) {
      if (win.isMinimized()) win.restore();
      win.show();
      win.focus();
    }
  });

  app.whenReady().then(async () => {
    prefs = loadPrefs();
    Menu.setApplicationMenu(
      process.platform === 'darwin'
        ? Menu.buildFromTemplate([{ role: 'appMenu' }, { role: 'editMenu' }, { role: 'viewMenu' }, { role: 'windowMenu' }])
        : null,
    );
    registerIpc();
    try {
      await startServer();
    } catch (e) {
      dialog.showErrorBox('PeekAround', `The gallery server could not start:\n${(e as Error).stack ?? e}`);
      app.quit();
      return;
    }
    createWindow();
    app.on('activate', () => {
      if (!BrowserWindow.getAllWindows().length) createWindow();
    });
  });

  app.on('window-all-closed', () => {
    // keep serving the LAN in the tray if sharing is on
    if (server?.ctx.lan.enabled && process.platform !== 'darwin') return;
    if (process.platform !== 'darwin') app.quit();
  });

  let quitting = false;
  app.on('before-quit', (e) => {
    if (quitting || !server) return;
    e.preventDefault();
    quitting = true;
    const s = server;
    server = null;
    void s
      .close()
      .catch(() => {})
      .finally(() => app.exit(0));
  });
}
