// Stages a self-contained Electron app in ./stage:
//   main.js + preload.js (esbuild bundles incl. the PeekAround server), web/ (built UI),
//   package.json with only the native/binary runtime deps, and their node_modules.
// Usage: node scripts/stage.mjs [--no-install] [--platform=win32|linux|darwin] [--arch=x64|arm64]
import { build } from 'esbuild';
import { execSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const pkgDir = path.resolve(here, '..');
const root = path.resolve(pkgDir, '../..');
const stage = path.join(pkgDir, 'stage');
const args = Object.fromEntries(
  process.argv.slice(2).map((a) => {
    const [k, v] = a.replace(/^--/, '').split('=');
    return [k, v ?? true];
  }),
);
const platform = args.platform ?? process.platform;
const arch = args.arch ?? process.arch;
const version = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8')).version;

// no ffprobe-static (61 MB per platform): the server parses `ffmpeg -i` output instead (ffmpeg.ts probe)
const NATIVE = {
  sharp: '^0.35.5',
  'ffmpeg-static': '^5.3.0',
  'heic-decode': '^2.1.0',
};

// 1) web UI
const webDist = path.join(root, 'packages/web/dist');
if (!fs.existsSync(path.join(webDist, 'index.html')) || args['build-web']) {
  console.log('› building web UI');
  execSync('npm run build -w @peekaround/web', { cwd: root, stdio: 'inherit' });
}

// 2) bundles
fs.mkdirSync(stage, { recursive: true });
for (const f of ['main.js', 'main.js.map', 'preload.js', 'preload.js.map']) fs.rmSync(path.join(stage, f), { force: true });
const common = {
  bundle: true,
  platform: 'node',
  format: 'cjs',
  target: 'node22',
  sourcemap: true,
  logLevel: 'info',
  external: ['electron', ...Object.keys(NATIVE)],
  define: { 'import.meta.url': '__importMetaUrl' },
  banner: { js: "const __importMetaUrl = require('url').pathToFileURL(__filename).href;" },
};
await build({ ...common, entryPoints: [path.join(pkgDir, 'src/main.ts')], outfile: path.join(stage, 'main.js') });
await build({ ...common, entryPoints: [path.join(pkgDir, 'src/preload.ts')], outfile: path.join(stage, 'preload.js'), sourcemap: false, banner: {}, define: {}, external: ['electron'] });

// 3) web + icons
fs.rmSync(path.join(stage, 'web'), { recursive: true, force: true });
fs.cpSync(webDist, path.join(stage, 'web'), { recursive: true });
const icon = path.join(pkgDir, 'build/icon.png');
if (fs.existsSync(icon)) fs.copyFileSync(icon, path.join(stage, 'icon.png'));

// 4) package.json for the packaged app
fs.writeFileSync(
  path.join(stage, 'package.json'),
  JSON.stringify(
    {
      name: 'peekaround',
      productName: 'PeekAround',
      version,
      description: 'PeekAround — your folders, beautifully.',
      author: { name: 'PeekAround', email: 'hello@peekaround.app' },
      homepage: 'https://github.com/peekaround/peekaround',
      license: 'MIT',
      main: 'main.js',
      dependencies: NATIVE,
    },
    null,
    2,
  ),
);

// 5) install native deps for the target platform
if (!args['no-install']) {
  console.log(`› installing runtime deps for ${platform}-${arch}`);
  fs.rmSync(path.join(stage, 'node_modules'), { recursive: true, force: true });
  fs.rmSync(path.join(stage, 'package-lock.json'), { force: true });
  execSync(`npm install --omit=dev --no-audit --no-fund --os=${platform} --cpu=${arch} --platform=${platform} --arch=${arch}`, {
    cwd: stage,
    stdio: 'inherit',
    env: { ...process.env, npm_config_platform: platform, npm_config_arch: arch },
  });
}
console.log(`✓ staged in ${stage}`);
