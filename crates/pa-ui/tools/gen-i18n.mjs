// Ports the web UI string tables (packages/web/src/i18n/*.ts) to i18n/{tr,en}.json.
// usage: node crates/pa-ui/tools/gen-i18n.mjs   (from the repo root, or any cwd)
// The keys, `{var}` placeholders and the "one|many" plural syntax are kept as they are, so both UIs
// stay in sync while the React app is the reference; rerun after editing the TS tables.
import fs from 'node:fs';
import path from 'node:path';
import url from 'node:url';

const here = path.dirname(url.fileURLToPath(import.meta.url));
const src = path.join(here, '../../../packages/web/src/i18n');
const out = path.join(here, '../i18n');

/** the object literal of `const x… = { … }` (spreads dropped), evaluated as plain JS */
function table(file) {
  const text = fs.readFileSync(path.join(src, file), 'utf8');
  const start = text.indexOf('= {', text.search(/^const \w+/m));
  const end = text.search(/^\}( as const)?;/m);
  const body = text.slice(start + 2, end + 1).replace(/^\s*\.\.\.\w+,\s*$/gm, '');
  return new Function(`return (${body});`)();
}

const subs = ['mobile', 'devices', 'explore', 'remote', 'map', 'mega', 'settings', 'library'];
const tr = Object.assign({}, table('tr.ts'), ...subs.map((s) => table(`${s}.tr.ts`)));
const en = Object.assign({}, ...subs.map((s) => table(`${s}.en.ts`)), table('en.ts'));
const sort = (o) => Object.fromEntries(Object.entries(o).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)));
fs.mkdirSync(out, { recursive: true });
fs.writeFileSync(path.join(out, 'tr.json'), JSON.stringify(sort(tr), null, 0).replace(/","/g, '",\n"') + '\n');
fs.writeFileSync(path.join(out, 'en.json'), JSON.stringify(sort(en), null, 0).replace(/","/g, '",\n"') + '\n');
const missing = Object.keys(tr).filter((k) => !(k in en));
console.log(`tr: ${Object.keys(tr).length} keys, en: ${Object.keys(en).length} keys, missing in en: ${missing.length}`);
