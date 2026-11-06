import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';
import Fastify, { type FastifyInstance } from 'fastify';
import cors from '@fastify/cors';
import fastifyStatic from '@fastify/static';
import websocket from '@fastify/websocket';
import { resolveConfig, type ServerOptions } from './config.js';
import { getSettings, type Ctx } from './context.js';
import { Db } from './db.js';
import { EventHub } from './events.js';
import { foldText, isValidTimeZone, systemTimeZone } from '@peekaround/shared';
import { ExploreService } from './explore.js';
import { Ffmpeg } from './ffmpeg.js';
import { GeoService } from './geo.js';
import { JobService } from './jobs/index.js';
import { LibraryService } from './library.js';
import { Scanner } from './media/scanner.js';
import { ThumbService } from './media/thumbs.js';
import { WatcherService } from './media/watcher.js';
import { HttpError } from './paths.js';
import { browseRoutes } from './routes/browse.js';
import { coreRoutes, serverInfo } from './routes/core.js';
import { fileRoutes } from './routes/files.js';
import { jobRoutes } from './routes/jobs.js';
import { mediaRoutes } from './routes/media.js';
import { exploreRoutes } from './routes/explore.js';
import { tileRoutes } from './routes/tiles.js';
import { corsDelegator, registerSecurity, securityOptionsFromEnv, type SecurityOptions } from './security.js';
import { deviceRoutes } from './routes/devices.js';
import { remoteRoutes } from './routes/remote.js';
import { cacheRoutes } from './routes/cache.js';
import { albumRoutes } from './routes/albums.js';
import { applyRuntimeSettings } from './runtime-settings.js';
import { DeviceHub } from './devices/hub.js';
import { deviceAccessHook } from './devices/access.js';
import { megaClient } from './remote/mega.js';
import { MegaAccount, megaAuthClient } from './remote/account.js';
import { RemoteService } from './remote/folders.js';
import type { Duplex } from 'node:stream';

export interface PeekServer {
  app: FastifyInstance;
  ctx: Ctx;
  url: string;
  port: number;
  /** Desktop: expose the gallery on the local network on an extra port. */
  setLan(enabled: boolean, port?: number): Promise<{ enabled: boolean; urls: string[] }>;
  /** the LAN http.Server while sharing is on (tests / diagnostics) */
  readonly lanServer: http.Server | null;
  close(): Promise<void>;
}

export async function createServer(opts: ServerOptions): Promise<PeekServer> {
  const cfg = resolveConfig(opts);
  for (const d of [cfg.dataDir, cfg.cacheDir, cfg.thumbDir, cfg.tempDir, cfg.fontDir]) fs.mkdirSync(d, { recursive: true });

  const db = new Db(cfg.dbPath);
  const events = new EventHub();
  const ffmpeg = new Ffmpeg(cfg.ffmpegPath, cfg.ffprobePath);
  await ffmpeg.check();
  const lib = new LibraryService(db, events);
  const thumbs = new ThumbService(cfg.thumbDir, ffmpeg);
  const geo = new GeoService(cfg.webDir);
  registerSearch(db, geo);
  const scanner = new Scanner(db, lib, ffmpeg, thumbs, events, geo);
  const watcher = new WatcherService(lib, scanner, cfg.watchPoll);
  const jobs = new JobService(events, cfg.tempDir);
  const ctx = { cfg, db, events, lib, ffmpeg, thumbs, scanner, watcher, jobs, geo, lan: { enabled: false, port: 8080 } } as Omit<Ctx, 'explore'> as Ctx;
  ctx.explore = new ExploreService(ctx);
  // optional MEGA account: its session id rides on every MEGA API request (links, streaming, imports)
  ctx.megaAccount = new MegaAccount(ctx, opts.megaAuth ?? megaAuthClient({ gateway: opts.megaGateway }));
  ctx.mega = opts.mega ?? megaClient({ session: ctx.megaAccount, gateway: opts.megaGateway });
  ctx.devices = new DeviceHub(ctx);
  ctx.remote = new RemoteService(ctx);
  // home time zone: persisted on first run (server zone), used for videos / mtime
  const stored = db.getSetting<{ homeTimeZone?: string }>('settings', {});
  if (!isValidTimeZone(stored.homeTimeZone)) db.setSetting('settings', { ...getSettings(ctx), homeTimeZone: systemTimeZone() });
  scanner.showHidden = getSettings(ctx).showHidden;
  scanner.homeTimeZone = getSettings(ctx).homeTimeZone;
  // thumbnail quality + indexing concurrency (runtime-settings.ts)
  applyRuntimeSettings(ctx, getSettings(ctx));

  const app = Fastify({
    logger: cfg.logger ? { level: 'info' } : false,
    bodyLimit: 32 * 1024 * 1024,
    routerOptions: { maxParamLength: 500 },
    forceCloseConnections: true,
  });

  const sec = opts.security ?? securityOptionsFromEnv();
  await registerSecurity(app, ctx, sec);
  await app.register(cors, { delegator: corsDelegator(sec) } as never);
  await app.register(websocket, { options: { maxPayload: 1024 * 1024 } });
  const webDir = cfg.webDir && fs.existsSync(path.join(cfg.webDir, 'index.html')) ? cfg.webDir : null;
  await app.register(fastifyStatic, {
    root: webDir ?? cfg.dataDir,
    serve: !!webDir,
    wildcard: false,
    index: ['index.html'],
    setHeaders(reply, p) {
      if (/[\\/]assets[\\/]/.test(p)) reply.header('Cache-Control', 'public, max-age=31536000, immutable');
      else reply.header('Cache-Control', 'no-cache');
    },
  });

  app.setErrorHandler((err: any, _req, reply) => {
    const code = err instanceof HttpError ? err.statusCode : ((err as any).statusCode ?? 500);
    if (code >= 500) app.log.error(err);
    reply.code(code).send({ error: err.message });
  });

  // device folders/media are invisible (404) to viewers that may not see them (see devices/access.ts)
  app.addHook('preHandler', deviceAccessHook(ctx));

  await app.register(async (scope) => {
    coreRoutes(scope, ctx);
    browseRoutes(scope, ctx);
    mediaRoutes(scope, ctx);
    jobRoutes(scope, ctx);
    exploreRoutes(scope, ctx);
    // map tiles (proxy + cache, own rate limit)
    tileRoutes(scope, ctx);
    remoteRoutes(scope, ctx);
    // per-kind cache usage + clearing (settings page)
    cacheRoutes(scope, ctx);
    albumRoutes(scope, ctx);
  });
  // file routes add raw-body content type parsers → keep them in their own encapsulated scope
  await app.register(async (scope) => fileRoutes(scope, ctx));
  await app.register(async (scope) => deviceRoutes(scope, ctx));

  // SPA fallback
  app.setNotFoundHandler((req, reply) => {
    if (webDir && req.method === 'GET' && !req.url.startsWith('/api/')) {
      return reply.type('text/html').header('Cache-Control', 'no-cache').sendFile('index.html', webDir);
    }
    reply.code(404).send({ error: 'Not found' });
  });

  lib.ensureRoots(cfg.roots);
  await lib.cleanupTemp(cfg.tempDir);
  // drop sessions whose files vanished
  for (const t of lib.listTemp()) if (!fs.existsSync(t.path)) await lib.deleteTemp(lib.folderRow(t.id), cfg.tempDir);
  const cleanupTimer = setInterval(() => void lib.cleanupTemp(cfg.tempDir).catch(() => {}), 10 * 60_000);
  cleanupTimer.unref();

  await app.listen({ host: cfg.host, port: cfg.port });
  const addr = app.server.address();
  const port = typeof addr === 'object' && addr ? addr.port : cfg.port;
  const shownHost = cfg.host === '0.0.0.0' || cfg.host === '::' ? '127.0.0.1' : cfg.host;
  ctx.devices.selfUrl = `http://${shownHost.includes(':') ? `[${shownHost}]` : shownHost}:${port}`;

  watcher.sync();
  void scanner.scanAll();
  // remote (MEGA) folders: re-list in the background (offline → old rows stay, status shows it)
  ctx.remote.start();

  // mDNS so the Android app can discover servers on the LAN
  let mdns: { stop(): void } | null = null;
  const publishMdns = async (p: number) => {
    try {
      const { Bonjour } = await import('bonjour-service');
      const b = new Bonjour();
      const svc = b.publish({ name: `PeekAround ${serverInfo(ctx).hostname}`.slice(0, 60), type: 'peekaround', port: p, txt: { v: cfg.version } });
      svc.on?.('error', () => {});
      mdns = { stop: () => b.unpublishAll(() => b.destroy()) };
    } catch {
      mdns = null;
    }
  };
  if (cfg.mdns && cfg.mode === 'server') void publishMdns(port);

  let lanServer: http.Server | null = null;
  /** upgraded (WebSocket) sockets that came in through the LAN server: http.Server no longer tracks them */
  const lanUpgraded = new Set<Duplex>();

  const stopLan = async () => {
    if (!lanServer) return;
    const srv = lanServer;
    lanServer = null;
    const closed = new Promise<void>((r) => srv.close(() => r()));
    // "stop sharing" must cut everything: phone tunnels, event sockets, in-flight replies/downloads
    for (const s of lanUpgraded) s.destroy();
    lanUpgraded.clear();
    srv.closeAllConnections?.();
    await closed;
  };

  const setLan = async (enabled: boolean, lanPort = 8080) => {
    if (lanServer) {
      await stopLan();
      mdns?.stop();
      mdns = null;
    }
    ctx.lan = { enabled: false, port: lanPort };
    if (enabled) {
      const srv = http.createServer((req, res) => {
        (req as http.IncomingMessage & { peekLan?: boolean }).peekLan = true;
        app.routing(req, res);
      });
      // long uploads / replies / downloads must never be cut by a request timeout
      srv.requestTimeout = 0;
      srv.keepAliveTimeout = 72_000;
      srv.headersTimeout = 60_000;
      srv.on('upgrade', (req, socket, head) => {
        (req as http.IncomingMessage & { peekLan?: boolean }).peekLan = true;
        lanUpgraded.add(socket);
        socket.once('close', () => lanUpgraded.delete(socket));
        app.server.emit('upgrade', req, socket, head);
      });
      await new Promise<void>((resolve, reject) => {
        srv.once('error', reject);
        srv.listen(lanPort, '0.0.0.0', () => resolve());
      });
      lanServer = srv;
      ctx.lan = { enabled: true, port: lanPort };
      if (cfg.mdns) void publishMdns(lanPort);
    }
    events.emit({ type: 'hello', info: serverInfo(ctx) });
    return serverInfo(ctx).lan;
  };

  return {
    app,
    ctx,
    port,
    url: `http://${shownHost}:${port}`,
    setLan,
    get lanServer() {
      return lanServer;
    },
    async close() {
      clearInterval(cleanupTimer);
      mdns?.stop();
      await stopLan();
      await ctx.devices.close();
      await ctx.remote.close();
      await jobs.close();
      await watcher.close();
      await scanner.stop();
      ctx.explore.close();
      await app.close();
      events.close();
      db.close();
    },
  };
}

/** SQL function behind free-text search: folded name + place (city, province, country names) + camera. */
function registerSearch(db: Db, geo: GeoService) {
  db.raw.function('peek_search', { deterministic: true }, (name: unknown, city: unknown, cc: unknown, admin1: unknown, make: unknown, model: unknown) => {
    const str = (v: unknown) => (v == null ? '' : String(v));
    return `${foldText(str(name))} ${foldText(str(city))} ${geo.placeText(cc == null ? null : String(cc), admin1 == null ? null : String(admin1))} ${foldText(`${str(make)} ${str(model)}`)}`;
  });
}
