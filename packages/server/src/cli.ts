import os from 'node:os';
import path from 'node:path';
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { createServer } from './app.js';

const here = path.dirname(fileURLToPath(import.meta.url));

function env(name: string): string | undefined {
  const v = process.env[name];
  return v === undefined || v === '' ? undefined : v;
}

function bool(v: string | undefined, def: boolean): boolean {
  if (v === undefined) return def;
  return ['1', 'true', 'yes', 'on'].includes(v.toLowerCase());
}

/** "Photos=/media/photos,/media/videos" → [{name:'Photos', path:'/media/photos'}, {path:'/media/videos'}] */
function parseRoots(list: string[]): { path: string; name?: string }[] {
  return list
    .flatMap((s) => s.split(/[,;\n]/))
    .map((s) => s.trim())
    .filter(Boolean)
    .map((s) => {
      const i = s.indexOf('=');
      return i > 0 && !/^[a-zA-Z]$/.test(s.slice(0, i)) ? { name: s.slice(0, i), path: s.slice(i + 1) } : { path: s };
    });
}

const { values } = parseArgs({
  options: {
    root: { type: 'string', multiple: true, short: 'r' },
    data: { type: 'string', short: 'd' },
    port: { type: 'string', short: 'p' },
    host: { type: 'string' },
    web: { type: 'string' },
    readonly: { type: 'boolean' },
    poll: { type: 'boolean' },
    'no-mdns': { type: 'boolean' },
    'split-roots': { type: 'boolean' },
    'browse-root': { type: 'string', multiple: true },
    verbose: { type: 'boolean', short: 'v' },
    help: { type: 'boolean', short: 'h' },
  },
  allowPositionals: true,
});

if (values.help) {
  console.log(`PeekAround — media gallery server

Usage: peekaround [--root <dir>]... [options]

  -r, --root <dir>        library folder (repeatable, "Name=dir" allowed)   env PEEK_ROOTS
  -d, --data <dir>        data/cache directory (default ~/.peekaround)     env PEEK_DATA_DIR
  -p, --port <port>       port (default 8080)                             env PEEK_PORT
      --host <host>       bind address (default 0.0.0.0)                  env PEEK_HOST
      --readonly          disable uploads/edits/deletes                   env PEEK_READONLY
      --poll              poll for file changes (network shares)           env PEEK_WATCH_POLL
      --split-roots       one library per sub folder of each root          env PEEK_SPLIT_ROOTS
      --browse-root <dir> limit the "add folder" browser to these dirs      env PEEK_BROWSE_ROOTS
      --no-mdns           don't announce on the local network              env PEEK_MDNS=false
`);
  process.exit(0);
}

const webCandidates = [values.web, env('PEEK_WEB_DIR'), path.resolve(here, '../../web/dist'), path.resolve(here, '../web'), path.resolve(here, 'web')];
const webDir = webCandidates.find((d) => d && fs.existsSync(path.join(d, 'index.html'))) ?? null;
const split = values['split-roots'] ?? bool(env('PEEK_SPLIT_ROOTS'), false);
const roots = parseRoots([...(values.root ?? []), ...(env('PEEK_ROOTS') ? [env('PEEK_ROOTS')!] : [])]).map((r) => ({ ...r, split }));

const server = await createServer({
  mode: 'server',
  dataDir: values.data ?? env('PEEK_DATA_DIR') ?? path.join(os.homedir(), '.peekaround'),
  port: Number(values.port ?? env('PEEK_PORT') ?? 8080),
  host: values.host ?? env('PEEK_HOST') ?? '0.0.0.0',
  roots,
  webDir,
  readonly: values.readonly ?? bool(env('PEEK_READONLY'), false),
  watchPoll: values.poll ?? bool(env('PEEK_WATCH_POLL'), false),
  mdns: values['no-mdns'] ? false : bool(env('PEEK_MDNS'), true),
  // never expose the whole host by default: library roots + home unless configured
  hostBrowseRoots: (() => {
    const given = [...(values['browse-root'] ?? []), ...(env('PEEK_BROWSE_ROOTS')?.split(/[,;]/).filter(Boolean) ?? [])];
    return given.length ? given : [...roots.map((r) => r.path), os.homedir()];
  })(),
  logger: values.verbose ?? bool(env('PEEK_VERBOSE'), false),
  version: process.env.npm_package_version ?? '1.3.1',
});

const lan = (await import('./config.js')).lanUrls(server.port);
console.log(`\n  ✦ PeekAround is running\n\n    Local:   ${server.url}\n${lan.map((u) => `    Network: ${u}`).join('\n')}\n${webDir ? '' : '\n    (web UI not built — run "npm run build" or use the Vite dev server)\n'}`);

const stop = async () => {
  await server.close().catch(() => {});
  process.exit(0);
};
process.on('SIGINT', stop);
process.on('SIGTERM', stop);
