// Package the staged app with electron-builder.
// Usage: node scripts/dist.mjs [--win] [--linux] [--mac] [--dir]
import { Arch, build, Platform } from 'electron-builder';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const pkgDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const argv = new Set(process.argv.slice(2));
const dirOnly = argv.has('--dir');

const targets = new Map();
if (argv.has('--win')) targets.set(Platform.WINDOWS, dirOnly ? ['dir'] : ['nsis', 'portable']);
if (argv.has('--linux')) targets.set(Platform.LINUX, dirOnly ? ['dir'] : ['AppImage', 'pacman', 'deb']);
if (argv.has('--mac')) targets.set(Platform.MAC, dirOnly ? ['dir'] : ['dmg']);
if (!targets.size) {
  const p = process.platform === 'win32' ? Platform.WINDOWS : process.platform === 'darwin' ? Platform.MAC : Platform.LINUX;
  targets.set(p, dirOnly ? ['dir'] : p === Platform.LINUX ? ['AppImage', 'pacman', 'deb'] : p === Platform.WINDOWS ? ['nsis', 'portable'] : ['dmg']);
}

const targetMap = new Map([...targets].map(([p, t]) => [p, p.createTarget(t, Arch.x64)]));

await build({
  projectDir: pkgDir,
  targets: new Map([...targetMap].flatMap(([, m]) => [...m])),
  config: {
    appId: 'app.peekaround.desktop',
    productName: 'PeekAround',
    copyright: 'Copyright © PeekAround',
    electronVersion: '45.2.3',
    directories: { app: 'stage', output: 'release', buildResources: 'build' },
    npmRebuild: false,
    publish: null,
    nodeGypRebuild: false,
    asar: true,
    asarUnpack: ['node_modules/sharp/**', 'node_modules/@img/**', 'node_modules/ffmpeg-static/**'],
    // only the UI languages we ship (≈ 7.5 MB of Chromium locales less)
    electronLanguages: ['en-US', 'tr'],
    compression: 'maximum',
    files: [
      '**/*',
      '!**/*.map',
      '!**/*.d.ts',
      '!**/{test,tests,__tests__,docs,doc,example,examples,.github}/**',
      '!**/{README,CHANGELOG,HISTORY}*',
      '!**/example.js',
      // heic-decode loads libheif-js/wasm-bundle (wasm inlined); the asm.js build, the ESM copy and the loose wasm are unused
      '!node_modules/libheif-js/libheif/**',
      '!node_modules/libheif-js/libheif-wasm/{libheif-bundle.mjs,libheif.js,libheif.wasm}',
      // sharp's WebAssembly fallback (+ its runtime): the native build for the target platform is always installed
      '!node_modules/@img/sharp-wasm32/**',
      '!node_modules/@emnapi/**',
      '!node_modules/@types/**',
      '!node_modules/ffmpeg-static/{*.README,install.js}',
    ],
    artifactName: '${productName}-${version}-${arch}.${ext}',
    win: {
      icon: 'build/icon.ico',
      executableName: 'PeekAround',
    },
    nsis: {
      oneClick: false,
      perMachine: false,
      allowToChangeInstallationDirectory: true,
      createDesktopShortcut: true,
      createStartMenuShortcut: true,
      shortcutName: 'PeekAround',
      artifactName: '${productName}-Setup-${version}.${ext}',
    },
    portable: { artifactName: '${productName}-${version}-portable.${ext}' },
    linux: {
      icon: 'build/icons',
      category: 'Graphics',
      executableName: 'peekaround',
      synopsis: 'Modern, animated gallery for your folders',
      description: 'PeekAround — browse, watch, edit and share the photos and videos in your folders.',
      maintainer: 'PeekAround <hello@peekaround.app>',
      vendor: 'PeekAround',
      desktop: { entry: { StartupWMClass: 'PeekAround', Keywords: 'photo;video;gallery;slideshow;collage;' } },
    },
    pacman: { depends: ['gtk3', 'nss', 'alsa-lib', 'libxss', 'libxtst'] },
    deb: { depends: ['libgtk-3-0', 'libnss3', 'libxss1', 'libxtst6', 'libasound2 | libasound2t64'] },
    mac: { category: 'public.app-category.photography', icon: 'build/icon.icns' },
  },
});
