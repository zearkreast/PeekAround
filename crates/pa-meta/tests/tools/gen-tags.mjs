import { tagKeys, tagValues, tagRevivers } from '/home/user/peekaround/node_modules/exifr/src/tags.mjs';
import '/home/user/peekaround/node_modules/exifr/src/bundles/full.mjs';
const picked = ['Make', 'Model', 'LensModel', 'FNumber', 'ExposureTime', 'ISO', 'FocalLength', 'FocalLengthIn35mmFormat', 'Flash', 'WhiteBalance', 'DateTimeOriginal', 'Software', 'latitude', 'longitude', 'GPSAltitude'];
let out = '//! exifr 7.1.3 tag dictionaries (generated from node_modules/exifr/src/dicts by the parity tooling — do not edit).\n\n';
for (const b of ['ifd0', 'exif', 'gps', 'interop']) {
  const d = [...tagKeys.get(b).entries()].sort((a, c) => a[0] - c[0]);
  out += `/// ${b} tag names\npub(crate) const ${b.toUpperCase()}_KEYS: &[(u16, &str)] = &[\n${d.map(([k, v]) => `    (0x${k.toString(16).padStart(4, '0')}, ${JSON.stringify(v)}),`).join('\n')}\n];\n\n`;
}
const vals = [];
for (const b of ['ifd0', 'exif', 'gps', 'interop']) {
  const vd = tagValues.get(b);
  if (!vd) continue;
  for (const [tag, dict] of vd) {
    const name = tagKeys.get(b).get(tag);
    if (!picked.includes(name)) continue;
    for (const [k, v] of Object.entries(dict)) vals.push(`    (${JSON.stringify(b)}, 0x${tag.toString(16).padStart(4, '0')}, ${JSON.stringify(k)}, ${JSON.stringify(v)}),`);
  }
}
out += `/// (block, tag, raw value as JS property key, translation) for the tags readExif picks\npub(crate) const VALUES: &[(&str, u16, &str, &str)] = &[\n${vals.join('\n')}\n];\n`;
const rev = [];
for (const [b, m] of tagRevivers) for (const [tag] of m) rev.push(`${b}:0x${tag.toString(16)}`);
console.error('revivers', rev.join(' '));
process.stdout.write(out);
