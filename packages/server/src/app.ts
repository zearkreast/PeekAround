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
import { Ffmpeg } from './ffmpeg.js';
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

export interface PeekServer {
  app: FastifyInstance;
  ctx: Ctx;
  url: string;
  port: number;
  /** Desktop: expose the gallery on the local network on an extra port. */
  setLan(enabled: boolean, port?: number): Promise<{ enabled: boolean; urls: string[] }>;
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
  const scanner = new Scanner(db, lib, ffmpeg, thumbs, events);
  const watcher = new WatcherService(lib, scanner, cfg.watchPoll);
  const jobs = new JobService(events, cfg.tempDir);
  const ctx: Ctx = { cfg, db, events, lib, ffmpeg, thumbs, scanner, watcher, jobs, lan: { enabled: false, port: 8080 } };
  scanner.showHidden = getSettings(ctx).showHidden;

  const app = Fastify({
    logger: cfg.logger ? { level: 'info' } : false,
    bodyLimit: 32 * 1024 * 1024,
    routerOptions: { maxParamLength: 500 },
    forceCloseConnections: true,
  });

  await app.register(cors, { origin: true, credentials: false, methods: ['GET', 'HEAD', 'PUT', 'POST', 'PATCH', 'DELETE', 'OPTIONS'] });
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

  await app.register(async (scope) => {
    coreRoutes(scope, ctx);
    browseRoutes(scope, ctx);
    mediaRoutes(scope, ctx);
    jobRoutes(scope, ctx);
  });
  // file routes add raw-body content type parsers → keep them in their own encapsulated scope
  await app.register(async (scope) => fileRoutes(scope, ctx));

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

  watcher.sync();
  void scanner.scanAll();

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

  const setLan = async (enabled: boolean, lanPort = 8080) => {
    if (lanServer) {
      await new Promise<void>((r) => lanServer!.close(() => r()));
      lanServer.closeAllConnections?.();
      lanServer = null;
      mdns?.stop();
      mdns = null;
    }
    ctx.lan = { enabled: false, port: lanPort };
    if (enabled) {
      const srv = http.createServer((req, res) => {
        (req as http.IncomingMessage & { peekLan?: boolean }).peekLan = true;
        app.routing(req, res);
      });
      srv.on('upgrade', (req, socket, head) => {
        (req as http.IncomingMessage & { peekLan?: boolean }).peekLan = true;
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
    async close() {
      clearInterval(cleanupTimer);
      mdns?.stop();
      if (lanServer) lanServer.close();
      await watcher.close();
      await scanner.stop();
      await app.close();
      db.close();
    },
  };
}
