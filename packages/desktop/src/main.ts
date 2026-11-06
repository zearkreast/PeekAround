import { app, BrowserWindow, dialog, ipcMain, Menu, nativeImage, nativeTheme, net as enet, Notification, screen, shell, Tray, type IpcMainEvent, type IpcMainInvokeEvent } from 'electron';
import fs from 'node:fs';
import net from 'node:net';
import path from 'node:path';
import type { Job } from '@peekaround/shared';
import { createServer, type PeekServer } from '@peekaround/server/src/index';
import { checkForUpdate, L, loadPrefs, log, logDir, pickShellPatch, savePrefs, type DesktopPrefs, type UpdateInfo } from './shell';

const isDev = !app.isPackaged;
const here = __dirname;

/** Binaries inside app.asar must be read from app.asar.unpacked. */
function unpacked(p: string | null | undefined): string | undefined {
  if (!p) return undefined;
  return p.replace(`app.asar${path.sep}`, `app.asar.unpacked${path.sep}`);
}

/**
 * ffmpeg ships with the app (ffmpeg-static); ffprobe does not (−61 MB): the server uses an ffprobe
 * found on PATH (or PEEK_FFPROBE) when there is one, else it parses `ffmpeg -i` (see server ffmpeg.ts).
 */
function ffmpegBinaries() {
  let ffmpegPath: string | undefined;
  try {
    ffmpegPath = unpacked(require('ffmpeg-static') as string);
  } catch {
    /* fall back to PATH */
  }
  return {
    ffmpegPath: ffmpegPath && fs.existsSync(ffmpegPath) ? ffmpegPath : 'ffmpeg',
    ffprobePath: process.env.PEEK_FFPROBE || 'ffprobe',
  };
}

// a background tray app must not stop on a stray exception (Electron's default is a blocking error dialog)
process.on('uncaughtException', (e) => log('uncaught', e));
process.on('unhandledRejection', (e) => log('unhandled rejection', e));

// Shell preferences are read before `ready`: hardware acceleration can only be switched off now.
const prefs: DesktopPrefs = loadPrefs();
const gpuActive = prefs.gpu;
if (!prefs.gpu) app.disableHardwareAcceleration();
// Windows: notifications and taskbar grouping need the installer's AppUserModelID (dist.mjs appId)
if (process.platform === 'win32') app.setAppUserModelId('app.peekaround.desktop');

/** "open at login" works for installed Windows / macOS builds (Linux desktops differ too much) */
const loginSupported = app.isPackaged && (process.platform === 'win32' || process.platform === 'darwin');

let server: PeekServer | null = null;
let win: BrowserWindow | null = null;
let tray: Tray | null = null;
let quitting = false;
let lastUpdate: UpdateInfo | null = prefs.lastUpdate ?? null;
/** renderers that asked for update notices (they are subscribed from then on) */
const updateListeners = new Set<number>();

const lang = () => prefs.lang ?? (app.getLocale().toLowerCase().startsWith('tr') ? 'tr' : 'en');

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
  log('server starting');
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
    // map tiles through Chromium's network stack: honours the system / PAC proxy (Node fetch does not)
    fetch: ((input: string | URL | Request, init?: RequestInit) => enet.fetch(input instanceof URL ? input.href : input, init)) as typeof fetch,
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
    } catch (e) {
      log('LAN share could not start', e);
      prefs.lan.enabled = false;
      savePrefs(prefs);
    }
  }
  server.ctx.events.on((e) => {
    if (e.type === 'job') onJob(e.job);
    else if (e.type === 'devices') updateTray();
    else if (e.type === 'pair' && !windowFocused()) notify(L(lang(), `📱 ${e.device.name} bağlanmak istiyor`, `📱 ${e.device.name} wants to connect`), L(lang(), 'Onaylamak için PeekAround’u aç.', 'Open PeekAround to approve.'));
  });
}

/** our own pages: the embedded server origin (and the Vite dev server when not packaged) */
function isAppUrl(url: string): boolean {
  try {
    const o = new URL(url).origin;
    return (!!server && o === new URL(server.url).origin) || (!app.isPackaged && o === 'http://localhost:5173');
  } catch {
    return false;
  }
}

/** only plain web links may leave the app — never file:, javascript:, custom schemes */
function isExternalWebUrl(url: string): boolean {
  try {
    const u = new URL(url);
    return (u.protocol === 'https:' || u.protocol === 'http:') && !isAppUrl(url);
  } catch {
    return false;
  }
}

/** IPC only from our own pages (never from a page that managed to navigate elsewhere) */
function trusted(e: IpcMainEvent | IpcMainInvokeEvent): boolean {
  return isAppUrl(e.senderFrame?.url ?? '');
}

const lanOn = () => !!server?.ctx.lan.enabled;
const phoneActive = () => !!server?.ctx.devices.list().some((d) => d.paired && d.online);
/** closing the window keeps the app (tray) instead of quitting */
const keepInTray = () => prefs.closeAction === 'tray' || (prefs.closeAction === 'auto' && (lanOn() || phoneActive()));
const windowFocused = () => !!win && win.isVisible() && !win.isMinimized() && win.isFocused();

function showWindow() {
  if (!win) {
    createWindow(true);
    return;
  }
  if (win.isMinimized()) win.restore();
  win.show();
  win.focus();
}

/** saved size / position, the position only while it is still on a connected display */
function restoredBounds(): { width: number; height: number; x?: number; y?: number } {
  const b = prefs.bounds;
  const size = { width: Math.max(880, b?.width ?? 1400), height: Math.max(560, b?.height ?? 900) };
  if (b?.x === undefined || b.y === undefined) return size;
  const wa = screen.getDisplayMatching({ x: b.x, y: b.y, width: size.width, height: size.height }).workArea;
  const onScreen = b.x < wa.x + wa.width - 80 && b.x + size.width > wa.x + 80 && b.y >= wa.y - 40 && b.y < wa.y + wa.height - 80;
  return onScreen ? { ...size, x: b.x, y: b.y } : size;
}

function saveBounds() {
  if (!win || win.isDestroyed()) return;
  prefs.bounds = { ...win.getNormalBounds(), maximized: win.isMaximized(), fullscreen: win.isFullScreen() };
  savePrefs(prefs);
}

const ZOOMS = [0.5, 0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2];
function stepZoom(dir: 1 | -1 | 0) {
  if (!win) return;
  const wc = win.webContents;
  const cur = wc.getZoomFactor();
  let next = 1;
  if (dir > 0) next = ZOOMS.find((z) => z > cur + 0.01) ?? ZOOMS[ZOOMS.length - 1];
  else if (dir < 0) next = [...ZOOMS].reverse().find((z) => z < cur - 0.01) ?? ZOOMS[0];
  wc.setZoomFactor(next);
  prefs.zoom = next;
  savePrefs(prefs);
}

function createWindow(show = true) {
  const b = prefs.bounds;
  const dark = nativeTheme.shouldUseDarkColors;
  win = new BrowserWindow({
    ...restoredBounds(),
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
      // packaged builds: no DevTools (and their memory); timers of a background window are throttled
      devTools: isDev,
      backgroundThrottling: true,
    },
  });
  const w = win;
  // webContents is gone once the window is destroyed: keep its id for the 'closed' handler
  const wcId = w.webContents.id;
  if (b?.maximized) w.maximize();
  if (b?.fullscreen) w.setFullScreen(true);
  w.once('ready-to-show', () => show && w.show());
  w.webContents.setWindowOpenHandler(({ url }) => {
    if (isExternalWebUrl(url)) void shell.openExternal(url);
    return { action: 'deny' };
  });
  w.webContents.on('will-navigate', (e, url) => {
    if (isAppUrl(url)) return;
    e.preventDefault();
    if (isExternalWebUrl(url)) void shell.openExternal(url);
  });
  // no <webview>, no permission prompts (camera, mic, geolocation, notifications…)
  w.webContents.on('will-attach-webview', (e) => e.preventDefault());
  w.webContents.session.setPermissionRequestHandler((_wc, permission, cb) => cb(permission === 'clipboard-sanitized-write' || permission === 'fullscreen'));
  w.webContents.on('did-finish-load', () => {
    if (prefs.zoom && prefs.zoom !== 1) w.webContents.setZoomFactor(prefs.zoom);
  });
  // the app has no menu on Windows / Linux: the standard window shortcuts are handled here
  if (process.platform !== 'darwin') {
    w.webContents.on('before-input-event', (e, input) => {
      if (input.type !== 'keyDown') return;
      const mod = input.control || input.meta;
      if (input.key === 'F11') w.setFullScreen(!w.isFullScreen());
      else if (mod && !input.alt && (input.key === '+' || input.key === '=' || input.code === 'NumpadAdd')) stepZoom(1);
      else if (mod && !input.alt && (input.key === '-' || input.code === 'NumpadSubtract')) stepZoom(-1);
      else if (mod && !input.alt && !input.shift && (input.key === '0' || input.code === 'Numpad0')) stepZoom(0);
      else if (isDev && ((mod && input.key.toLowerCase() === 'r') || input.key === 'F5')) w.webContents.reload();
      else if (isDev && (input.key === 'F12' || (mod && input.shift && input.key.toLowerCase() === 'i'))) w.webContents.toggleDevTools();
      else return;
      e.preventDefault();
    });
  }
  w.webContents.on('render-process-gone', (_e, d) => {
    log('renderer gone', d.reason, d.exitCode);
    if (d.reason !== 'clean-exit' && d.reason !== 'killed' && !w.isDestroyed()) w.reload();
  });
  w.on('close', () => {
    saveBounds();
    // first close into the tray: say where the app went
    if (!quitting && keepInTray() && !prefs.trayHintShown) {
      prefs.trayHintShown = true;
      savePrefs(prefs);
      notify('PeekAround', L(lang(), 'Tepside çalışmaya devam ediyor.', 'Still running in the tray.'), true);
    }
  });
  w.on('closed', () => {
    updateListeners.delete(wcId);
    if (win === w) win = null;
    updateTray();
  });
  const url = isDev && process.env.PEEK_DEV_URL ? process.env.PEEK_DEV_URL : server!.url;
  void w.loadURL(url);
  updateTray();
}

function updateTray() {
  if (!server || quitting) return;
  const need = lanOn() || phoneActive() || !win || prefs.closeAction === 'tray';
  if (!need) {
    tray?.destroy();
    tray = null;
    return;
  }
  if (!tray) {
    const ic = iconPath();
    tray = new Tray(ic ? nativeImage.createFromPath(ic).resize({ width: 18, height: 18 }) : nativeImage.createEmpty());
    tray.setToolTip('PeekAround');
    tray.on('click', showWindow);
  }
  const lg = lang();
  const urls = lanOn() ? (require('@peekaround/server/src/config') as typeof import('@peekaround/server/src/config')).lanUrls(server.ctx.lan.port) : [];
  tray.setContextMenu(
    Menu.buildFromTemplate([
      { label: 'PeekAround', enabled: false },
      ...urls.map((u) => ({ label: u, click: () => void shell.openExternal(u) })),
      { type: 'separator' as const },
      { label: L(lg, 'Göster', 'Show'), click: showWindow },
      ...(lanOn()
        ? [
            {
              label: L(lg, 'Ağ paylaşımını durdur', 'Stop LAN sharing'),
              click: () => {
                void server?.setLan(false).then(() => {
                  prefs.lan.enabled = false;
                  savePrefs(prefs);
                  updateTray();
                });
              },
            },
          ]
        : []),
      { label: L(lg, 'Çık', 'Quit'), click: () => app.quit() },
    ]),
  );
}

// ── notifications + taskbar progress (server job events, in-process) ──

const shown = new Set<Notification>();
function notify(title: string, body: string, force = false) {
  if ((!prefs.notify && !force) || !Notification.isSupported()) return;
  const n = new Notification({ title, body, icon: iconPath(), silent: false });
  shown.add(n);
  n.on('click', () => {
    shown.delete(n);
    showWindow();
  });
  n.on('close', () => shown.delete(n));
  n.show();
}

const jobs = new Map<string, Job>();
let progressTimer: NodeJS.Timeout | null = null;

function updateProgress() {
  if (progressTimer) return;
  progressTimer = setTimeout(() => {
    progressTimer = null;
    if (!win || win.isDestroyed()) return;
    const active = [...jobs.values()].filter((j) => j.status === 'running' || j.status === 'queued');
    if (!active.length) win.setProgressBar(-1);
    else win.setProgressBar(Math.max(0.02, Math.min(1, active.reduce((a, j) => a + (j.status === 'running' ? j.progress : 0), 0) / active.length)));
  }, 250);
}

function onJob(j: Job) {
  const prev = jobs.get(j.id);
  const done = j.status === 'done' || j.status === 'error' || j.status === 'canceled';
  if (done) {
    jobs.delete(j.id);
    // long jobs that end while the window is in the background (or closed to the tray)
    if (prev && j.status !== 'canceled' && Date.now() - j.createdAt > 8000 && !windowFocused()) {
      const lg = lang();
      if (j.status === 'done') notify(L(lg, 'İş tamamlandı', 'Job finished'), j.title);
      else notify(L(lg, 'İş başarısız', 'Job failed'), `${j.title}${j.error ? ` — ${j.error}` : ''}`);
    }
  } else jobs.set(j.id, j);
  updateProgress();
}

// ── updates (GitHub releases; never downloads) ──

async function runUpdateCheck(manual: boolean): Promise<UpdateInfo> {
  const u = await checkForUpdate(((input: string | URL | Request, init?: RequestInit) => enet.fetch(input instanceof URL ? input.href : input, init)) as typeof fetch, app.getVersion());
  if (u.error) log('update check failed', u.error);
  else prefs.lastUpdateCheck = u.checkedAt;
  lastUpdate = u;
  prefs.lastUpdate = u;
  savePrefs(prefs);
  if (!manual) offerUpdate();
  else if (u.newer && u.latest) {
    prefs.notifiedVersion = u.latest;
    savePrefs(prefs);
  }
  return u;
}

/** a newer release the user has not been told about yet */
function pendingUpdate(): UpdateInfo | null {
  const u = lastUpdate;
  if (!u?.newer || !u.latest || prefs.notifiedVersion === u.latest || u.current !== app.getVersion()) return null;
  return u;
}

function offerUpdate() {
  const u = pendingUpdate();
  if (!u || !win || !updateListeners.has(win.webContents.id)) return;
  win.webContents.send('peek:update', u);
  prefs.notifiedVersion = u.latest!;
  savePrefs(prefs);
}

function maybeAutoCheck() {
  if (prefs.autoUpdate && !quitting && Date.now() - (prefs.lastUpdateCheck ?? 0) > 24 * 3600_000) void runUpdateCheck(false);
}

// ── shell settings (renderer: Settings → Desktop) ──

function applyLoginItem() {
  if (!loginSupported) return;
  try {
    app.setLoginItemSettings({
      openAtLogin: prefs.startAtLogin,
      // Windows: the flag tells a login launch apart (startHidden is checked when it starts)
      args: ['--hidden'],
      ...(process.env.PORTABLE_EXECUTABLE_FILE ? { path: process.env.PORTABLE_EXECUTABLE_FILE } : {}),
    });
  } catch (e) {
    log('login item', e);
  }
}

function launchedAtLogin(): boolean {
  if (process.argv.includes('--hidden')) return true;
  try {
    return process.platform === 'darwin' && app.getLoginItemSettings().wasOpenedAtLogin;
  } catch {
    return false;
  }
}

function shellState() {
  return {
    startAtLogin: loginSupported && prefs.startAtLogin,
    startHidden: prefs.startHidden,
    closeAction: prefs.closeAction,
    gpu: prefs.gpu,
    notify: prefs.notify,
    autoUpdate: prefs.autoUpdate,
    loginSupported,
    gpuActive,
    version: app.getVersion(),
    lastUpdate,
  };
}

/** absolute paths of local media files by media id (unknown / device / remote / missing ids are skipped) */
function localMediaFiles(ids: unknown[]): string[] {
  const { mediaRow, mediaAbs } = require('@peekaround/server/src/routes/util') as typeof import('@peekaround/server/src/routes/util');
  const out: string[] = [];
  for (const id of ids) {
    if ((typeof id !== 'string' && typeof id !== 'number') || !/^\d{1,12}$/.test(String(id))) continue;
    try {
      const abs = mediaAbs(server!.ctx, mediaRow(server!.ctx, String(id))).abs;
      if (path.isAbsolute(abs) && fs.existsSync(abs)) out.push(abs);
    } catch {
      /* not a local file */
    }
  }
  return out;
}

let dragIconCache: Electron.NativeImage | null = null;
function dragIcon(): Electron.NativeImage {
  if (!dragIconCache) {
    const ic = iconPath();
    dragIconCache = ic ? nativeImage.createFromPath(ic).resize({ width: 48, height: 48 }) : nativeImage.createEmpty();
  }
  return dragIconCache;
}

function registerIpc() {
  ipcMain.handle('peek:pick-folders', async (e) => {
    if (!trusted(e) || !win) return [];
    const r = await dialog.showOpenDialog(win, { properties: ['openDirectory', 'multiSelections', 'createDirectory'] });
    return r.canceled ? [] : r.filePaths;
  });
  ipcMain.handle('peek:show-in-folder', (e, p: unknown) => {
    if (trusted(e) && typeof p === 'string' && path.isAbsolute(p)) shell.showItemInFolder(p);
  });
  ipcMain.handle('peek:open-external', (e, url: unknown) => {
    if (trusted(e) && typeof url === 'string' && isExternalWebUrl(url)) return shell.openExternal(url);
  });
  ipcMain.handle('peek:get-lan', (e) => {
    if (!trusted(e)) return { enabled: false, port: prefs.lan.port, urls: [] };
    const lan = server!.ctx.lan;
    const { lanUrls } = require('@peekaround/server/src/config') as typeof import('@peekaround/server/src/config');
    return { enabled: lan.enabled, port: prefs.lan.port, urls: lan.enabled ? lanUrls(lan.port) : [] };
  });
  ipcMain.handle('peek:set-lan', async (e, enabled: unknown, port: unknown) => {
    if (!trusted(e)) return { enabled: false, urls: [], error: 'Not allowed' };
    const p = Number(port);
    if (typeof enabled !== 'boolean' || !Number.isInteger(p) || p < 1024 || p > 65535) return { enabled: lanOn(), urls: [], error: 'Invalid port' };
    try {
      const r = await server!.setLan(enabled, p);
      prefs.lan = { enabled: r.enabled, port: p };
      savePrefs(prefs);
      updateTray();
      return r;
    } catch (err) {
      return { enabled: false, urls: [], error: (err as Error).message };
    }
  });
  ipcMain.on('peek:window', (e, action: unknown) => {
    if (!win || !trusted(e)) return;
    if (action === 'minimize') win.minimize();
    else if (action === 'maximize') (win.isMaximized() ? win.unmaximize() : win.maximize());
    else if (action === 'close') win.close();
  });
  ipcMain.on('peek:theme', (e, dark: unknown) => {
    if (process.platform !== 'darwin' && win && trusted(e)) {
      try {
        win.setTitleBarOverlay({ color: '#00000000', symbolColor: dark ? '#a3a3b0' : '#555564', height: 59 });
      } catch {
        /* not supported */
      }
    }
  });
  ipcMain.on('peek:lang', (e, l: unknown) => {
    if (!trusted(e) || (l !== 'tr' && l !== 'en') || prefs.lang === l) return;
    prefs.lang = l;
    savePrefs(prefs);
    updateTray();
  });
  ipcMain.handle('peek:shell-get', (e) => (trusted(e) ? shellState() : null));
  ipcMain.handle('peek:shell-set', (e, raw: unknown) => {
    if (!trusted(e)) return shellState();
    const patch = pickShellPatch(raw);
    Object.assign(prefs, patch);
    savePrefs(prefs);
    if (patch.startAtLogin !== undefined || patch.startHidden !== undefined) applyLoginItem();
    if (patch.closeAction !== undefined) updateTray();
    if (patch.autoUpdate) maybeAutoCheck();
    return shellState();
  });
  ipcMain.on('peek:restart', (e) => {
    if (!trusted(e)) return;
    app.relaunch({ args: process.argv.slice(1).filter((a) => a !== '--hidden') });
    app.quit();
  });
  ipcMain.handle('peek:update-check', (e) => (trusted(e) ? runUpdateCheck(true) : null));
  ipcMain.handle('peek:update-pending', (e) => {
    if (!trusted(e)) return null;
    updateListeners.add(e.sender.id);
    const u = pendingUpdate();
    if (u) {
      prefs.notifiedVersion = u.latest!;
      savePrefs(prefs);
    }
    return u;
  });
  // drag media out to the OS as real files: the renderer only names media ids, the paths come from
  // the server's index (local files only — never phone / MEGA media, never arbitrary paths)
  ipcMain.on('peek:start-drag', (e, ids: unknown) => {
    if (!trusted(e) || !server || !Array.isArray(ids) || !ids.length) return;
    const files = localMediaFiles(ids.slice(0, 1000));
    if (!files.length) return;
    try {
      e.sender.startDrag({ file: files[0], files, icon: dragIcon() });
    } catch (err) {
      log('startDrag', err);
    }
  });
  ipcMain.handle('peek:open-path', async (e, which: unknown) => {
    if (!trusted(e)) return 'Not allowed';
    const dir = which === 'data' ? app.getPath('userData') : which === 'logs' ? logDir() : null;
    if (!dir) return 'Unknown folder';
    fs.mkdirSync(dir, { recursive: true });
    return shell.openPath(dir);
  });
}

if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  // a second launch (shortcut, login item…) brings the running window back
  app.on('second-instance', () => showWindow());

  app.whenReady().then(async () => {
    log(`start ${app.getVersion()} electron ${process.versions.electron} gpu=${gpuActive}`);
    Menu.setApplicationMenu(
      process.platform === 'darwin'
        ? Menu.buildFromTemplate([{ role: 'appMenu' }, { role: 'editMenu' }, { role: 'viewMenu' }, { role: 'windowMenu' }])
        : null,
    );
    registerIpc();
    try {
      await startServer();
      log(`server ready ${server?.url ?? ''}`);
    } catch (e) {
      log('server start failed', e);
      dialog.showErrorBox('PeekAround', `The gallery server could not start:\n${(e as Error).stack ?? e}`);
      app.quit();
      return;
    }
    // login launch with "start hidden": tray only — the window (and its renderer) is created on demand
    if (prefs.startHidden && prefs.startAtLogin && launchedAtLogin()) updateTray();
    else createWindow();
    app.on('activate', () => {
      if (!BrowserWindow.getAllWindows().length) createWindow();
    });
    setTimeout(maybeAutoCheck, 30_000).unref();
    setInterval(maybeAutoCheck, 3 * 3600_000).unref();
  });

  app.on('window-all-closed', () => {
    if (process.platform === 'darwin') return;
    // keep serving in the tray (LAN sharing / phone link / "minimize to tray"); the renderer is gone → memory is freed
    if (keepInTray()) {
      updateTray();
      return;
    }
    app.quit();
  });

  app.on('before-quit', (e) => {
    saveBounds();
    savePrefs(prefs, true);
    if (quitting || !server) return;
    e.preventDefault();
    quitting = true;
    const s = server;
    server = null;
    tray?.destroy();
    tray = null;
    void s
      .close()
      .catch(() => {})
      .finally(() => app.exit(0));
  });
}
