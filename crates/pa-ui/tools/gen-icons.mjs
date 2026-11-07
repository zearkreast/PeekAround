// Generates ui/icons/*.svg from lucide-react's icon nodes (ISC, see ui/icons/LICENSE-lucide.txt).
// usage: node tools/gen-icons.mjs <path to node_modules/lucide-react>
import fs from 'node:fs';
import path from 'node:path';
import url from 'node:url';

const lucide = process.argv[2];
const here = path.dirname(url.fileURLToPath(import.meta.url));
const out = path.join(here, '..', 'ui', 'icons');
// lucide name → stroke widths to emit (2 = default; rail icons also get thin/bold variants)
const ICONS = {
  images: [2, 1.8, 2.25], compass: [2, 1.8, 2.25], heart: [2, 1.8, 2.25], 'book-image': [2, 1.8, 2.25], library: [2, 1.8, 2.25],
  settings: [2, 1.8, 2.25], 'wifi-off': [2], search: [2], 'sliders-horizontal': [2], plus: [2], x: [2], check: [2, 3],
  'chevron-down': [2], 'chevron-right': [2], 'chevron-left': [2], 'chevron-up': [2], 'loader-circle': [2], moon: [2], sun: [2],
  monitor: [2], palette: [2], play: [2], film: [2], music: [2], 'circle-check': [2], 'circle-alert': [2], info: [2],
  'grid-2x2': [2], 'rows-3': [2], 'layout-grid': [2], eye: [2], folder: [2], gauge: [2], network: [2], 'shield-check': [2],
  keyboard: [2], 'arrow-left': [2], upload: [2], 'folder-plus': [2], 'layout-dashboard': [2], sparkles: [2],
  'calendar-days': [2], server: [2], minus: [2], 'trash-2': [2], image: [2], video: [2], 'map-pin': [2], star: [2],
  'ellipsis': [2], 'refresh-cw': [2], 'triangle-alert': [2], 'arrow-down-wide-narrow': [2], 'arrow-up-narrow-wide': [2],
  'grid-3x3': [2], 'folder-up': [2], 'clock-3': [2], download: [2], 'external-link': [2], 'square': [2], 'maximize-2': [2],
};
fs.mkdirSync(out, { recursive: true });
const esc = (s) => String(s).replace(/&/g, '&amp;').replace(/"/g, '&quot;');
for (const [name, widths] of Object.entries(ICONS)) {
  // some names are aliases (`export { default } from './other.mjs'`): follow them
  let file = path.join(lucide, 'dist/esm/icons', `${name}.mjs`);
  for (;;) {
    const m = fs.readFileSync(file, 'utf8').match(/export \{ default \} from '\.\/([^']+)'/);
    if (!m) break;
    file = path.join(lucide, 'dist/esm/icons', m[1]);
  }
  const node = (await import(file)).__iconData.node;
  const body = node
    .map(([tag, attrs]) => `<${tag} ${Object.entries(attrs).filter(([k]) => k !== 'key').map(([k, v]) => `${k}="${esc(v === 'currentColor' ? '#000' : v)}"`).join(' ')}/>`)
    .join('');
  for (const w of widths) {
    const suffix = w === 2 ? '' : w < 2 ? '-thin' : w > 2.5 ? '-heavy' : '-bold';
    const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="${w}" stroke-linecap="round" stroke-linejoin="round">${body}</svg>\n`;
    fs.writeFileSync(path.join(out, `${name}${suffix}.svg`), svg);
  }
}
// filled variants (fill-current in the web UI: tile badges)
for (const name of ['heart', 'play']) {
  const svg = fs.readFileSync(path.join(out, `${name}.svg`), 'utf8').replace('fill="none"', 'fill="#000"');
  fs.writeFileSync(path.join(out, `${name}-fill.svg`), svg);
}
fs.copyFileSync(path.join(lucide, 'LICENSE'), path.join(out, 'LICENSE-lucide.txt'));
console.log('icons:', Object.keys(ICONS).length);
