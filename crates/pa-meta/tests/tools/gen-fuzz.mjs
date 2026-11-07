// Fuzz parity fixtures for pa-meta's exifr emulation: random EXIF IFDs + XMP packets in JPEG / PNG / TIFF
// containers, parsed by exifr 7 with the two option sets of the TypeScript server.
// usage: TZ=UTC node gen-fuzz.mjs <out.json>
import fs from 'node:fs';
import { createRequire } from 'node:module';
const require = createRequire('/home/user/peekaround/package.json');
const exifr = require('exifr');

let seed = 12345;
const rnd = () => {
  seed = (seed * 1103515245 + 12345) & 0x7fffffff;
  return seed / 0x7fffffff;
};
const ri = (a, b) => Math.floor(a + rnd() * (b - a + 1));
const pick = (a) => a[Math.floor(rnd() * a.length)];

const GROUP = { tiff: true, exif: true, gps: true, xmp: true, icc: false, iptc: false, jfif: false, ihdr: false, interop: false, makerNote: false, userComment: false, reviveValues: false, translateValues: false, translateKeys: true, mergeOutput: true };
const PANEL = { tiff: true, exif: true, gps: true, xmp: false, icc: false, iptc: false };

// ── TIFF builder ──
const SIZES = { 1: 1, 2: 1, 3: 2, 4: 4, 5: 8, 6: 1, 7: 1, 8: 2, 9: 4, 10: 8, 11: 4, 12: 8, 13: 4 };
function tiff(le, ifds) {
  // ifds: { ifd0: entries, exif, gps } entries: [tag, type, count, bytes(Buffer of the value)]
  const order = ['ifd0', 'exif', 'gps', 'interop'].filter((k) => ifds[k]);
  const parts = [];
  let pos = 8;
  const offs = {};
  for (const k of order) {
    offs[k] = pos;
    const e = ifds[k];
    pos += 2 + e.length * 12 + 4 + e.reduce((a, x) => a + (x[3].length > 4 ? x[3].length + (x[3].length & 1) : 0), 0);
  }
  const buf = Buffer.alloc(pos + ri(0, 8));
  buf.write(le ? 'II' : 'MM', 0, 'latin1');
  const w16 = (v, o) => (le ? buf.writeUInt16LE(v & 0xffff, o) : buf.writeUInt16BE(v & 0xffff, o));
  const w32 = (v, o) => (le ? buf.writeUInt32LE(v >>> 0, o) : buf.writeUInt32BE(v >>> 0, o));
  w16(42, 2);
  w32(rnd() < 0.03 ? ri(0, 7) : 8, 4);
  for (const k of order) {
    let o = offs[k];
    const e = ifds[k];
    w16(e.length, o);
    let data = o + 2 + e.length * 12 + 4;
    for (let i = 0; i < e.length; i++) {
      let [tag, type, count, bytes] = e[i];
      if (tag === 0x8769 && ifds.exif && rnd() > 0.05) bytes = u32(le, offs.exif);
      if (tag === 0x8825 && ifds.gps && rnd() > 0.05) bytes = u32(le, offs.gps);
      if (tag === 0xa005 && ifds.interop && rnd() > 0.05) bytes = u32(le, offs.interop);
      const eo = o + 2 + i * 12;
      w16(tag, eo);
      w16(type, eo + 2);
      w32(count, eo + 4);
      if (bytes.length <= 4) bytes.copy(buf, eo + 8);
      else {
        w32(rnd() < 0.03 ? ri(0, 100000) : data, eo + 8);
        bytes.copy(buf, data);
        data += bytes.length + (bytes.length & 1);
      }
    }
  }
  return buf;
}
const u32 = (le, v) => {
  const b = Buffer.alloc(4);
  le ? b.writeUInt32LE(v >>> 0) : b.writeUInt32BE(v >>> 0);
  return b;
};
function value(le, type, n) {
  const sz = SIZES[type] ?? 1;
  const b = Buffer.alloc(Math.max(0, sz * n));
  for (let i = 0; i < n; i++) {
    const o = i * sz;
    const r = rnd();
    if (type === 1 || type === 7 || type === 6) b[o] = ri(0, 255);
    else if (type === 3 || type === 8) le ? b.writeUInt16LE(ri(0, 65535), o) : b.writeUInt16BE(ri(0, 65535), o);
    else if (type === 4 || type === 9 || type === 13) le ? b.writeUInt32LE(r < 0.5 ? ri(0, 9) : ri(0, 0x7fffffff), o) : b.writeUInt32BE(ri(0, 0x7fffffff), o);
    else if (type === 5 || type === 10) {
      const num = r < 0.2 ? 0 : ri(0, 100000);
      const den = r < 0.1 ? 0 : ri(1, 1000);
      le ? (b.writeUInt32LE(num, o), b.writeUInt32LE(den, o + 4)) : (b.writeUInt32BE(num, o), b.writeUInt32BE(den, o + 4));
      if (type === 10 && rnd() < 0.3) le ? b.writeInt32LE(-num, o) : b.writeInt32BE(-num, o);
    } else if (type === 11) le ? b.writeFloatLE(rnd() * 100 - 50, o) : b.writeFloatBE(rnd() * 100 - 50, o);
    else if (type === 12) le ? b.writeDoubleLE(rnd() * 360 - 180, o) : b.writeDoubleBE(rnd() * 360 - 180, o);
  }
  return b;
}
const strs = ['NIKON CORPORATION', 'Canon', '', '   ', 'Apple\0\0', 'x\0y', 'ÑIKON', '2024:07:01 23:30:00', '2024:07:01', '0000:00:00 00:00:00', '+03:00', '-0530', 'Z', 'N', 'S', 'E', 'W', '41.5', '8,30.0', '100', '123', 'equirectangular', '﻿BOM', '2024-07-03T21:00:00+0300', 'a'.repeat(200), '2024:13:45 99:99:99', '1999:12:31 23:59:59 ', '12:00:00'];
function asciiEntry(le, tag) {
  let s = Buffer.from(pick(strs) + (rnd() < 0.7 ? '\0' : ''), 'utf8');
  if (rnd() < 0.1) s = Buffer.from([0xff, 0xfe, 0x41, 0]);
  return [tag, 2, rnd() < 0.05 ? s.length + ri(1, 5) : s.length, s];
}
const IFD0_TAGS = [0x010f, 0x0110, 0x0112, 0x0131, 0x0132, 0x0100, 0x0101, 0x9003, 0x1234, 0xc4a5, 0x02bc];
const EXIF_TAGS = [0x9003, 0x9004, 0x9010, 0x9011, 0x9012, 0xa002, 0xa003, 0xa434, 0x829a, 0x829d, 0x8827, 0x9209, 0x920a, 0xa405, 0xa403, 0xa300, 0xa301, 0x927c, 0x9286, 0xa005, 0x9999];
const GPS_TAGS = [1, 2, 3, 4, 5, 6, 7, 0x1d, 0, 0x99];
const STRING_TAGS = new Set([0x010f, 0x0110, 0x0131, 0x0132, 0x9003, 0x9004, 0x9010, 0x9011, 0x9012, 0xa434, 1, 3, 0x1d]);
function entries(le, tags, n) {
  const out = [];
  for (let i = 0; i < n; i++) {
    const tag = pick(tags);
    if (STRING_TAGS.has(tag) && rnd() < 0.75) {
      out.push(asciiEntry(le, tag));
      continue;
    }
    let type = rnd() < 0.04 ? pick([0, 14, 99]) : pick([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13]);
    const count = pick([0, 1, 1, 1, 2, 3, 3, 4, 5]);
    if ((tag === 2 || tag === 4 || tag === 7) && rnd() < 0.6) type = pick([5, 10, 12]);
    out.push([tag, type, count, value(le, type, count)]);
  }
  return out;
}
function randomTiff() {
  const le = rnd() < 0.7;
  const ifds = { ifd0: entries(le, IFD0_TAGS, ri(0, 6)) };
  if (rnd() < 0.8) {
    ifds.exif = entries(le, EXIF_TAGS, ri(0, 8));
    ifds.ifd0.push([0x8769, 4, 1, u32(le, 0)]);
  }
  if (rnd() < 0.7) {
    ifds.gps = entries(le, GPS_TAGS, ri(0, 8));
    ifds.ifd0.push([0x8825, 4, 1, u32(le, 0)]);
  }
  if (rnd() < 0.2) {
    ifds.interop = entries(le, [1, 2], ri(1, 2));
    (ifds.exif ?? ifds.ifd0).push([0xa005, 4, 1, u32(le, 0)]);
  }
  for (const k of Object.keys(ifds)) if (rnd() < 0.3) ifds[k].sort((a, b) => a[0] - b[0]);
  return tiff(le, ifds);
}

// ── XMP builder ──
const NS = ['exif', 'tiff', 'xmp', 'GPano', 'aux', 'photoshop', 'dc', 'my_ns', 'drone-dji', 'xmlns', 'rdf'];
const NAMES = ['DateTimeOriginal', 'CreateDate', 'ModifyDate', 'GPSLatitude', 'GPSLongitude', 'GPSLatitudeRef', 'GPSLongitudeRef', 'Make', 'Model', 'Lens', 'LensModel', 'ProjectionType', 'UsePanoramaViewer', 'Orientation', 'PixelXDimension', 'ImageWidth', 'OffsetTimeOriginal', 'GPSDateStamp', 'GPSTimeStamp', 'latitude', 'value', 'subject', 'li', 'about'];
const VALS = ['41,0.738N', '28,58.476E', '2024-07-01T10:00:00+03:00', 'True', 'false', 'equirectangular', '8000', '0x10', 'Infinity', '1e3', ' 12 ', '', '  ', 'null', 'undefined', 'Pixel 8', 'a &amp; b', '6', '-12.5', '2007:07:07', '10:00:00', 'Ünïcödé', "it's"];
const attr = () => `${pick(NS)}:${pick(NAMES)}=${rnd() < 0.85 ? `"${pick(VALS).replace(/"/g, '')}"` : `'${pick(VALS).replace(/'/g, '')}'`}`;
function element(depth) {
  const ns = pick(NS.filter((n) => n !== 'xmlns'));
  const name = pick(NAMES);
  const r = rnd();
  if (r < 0.15) return `<${ns}:${name}/>`;
  if (r < 0.2) return `<${ns}:${name} ${attr()}/>`;
  if (r < 0.45 || depth > 2) return `<${ns}:${name}>${pick(VALS)}</${ns}:${name}>`;
  if (r < 0.75) {
    const list = pick(['Seq', 'Bag', 'Alt']);
    const items = Array.from({ length: ri(0, 3) }, () => (rnd() < 0.3 ? `<rdf:li xml:lang="x-default">${pick(VALS)}</rdf:li>` : rnd() < 0.2 ? `<rdf:li>${element(depth + 1)}</rdf:li>` : rnd() < 0.15 ? `<rdf:li><rdf:Description ${attr()}/></rdf:li>` : `<rdf:li>${pick(VALS)}</rdf:li>`)).join('');
    return `<${ns}:${name}><rdf:${list}>${items}</rdf:${list}></${ns}:${name}>`;
  }
  if (r < 0.85) return `<${ns}:${name} ${attr()}>${element(depth + 1)}${rnd() < 0.5 ? element(depth + 1) : ''}</${ns}:${name}>`;
  if (r < 0.92) return `<${ns}:${name}>${pick(VALS)}`; // unclosed
  return `<${ns}:${name} bad=x>${pick(VALS)}</${ns}:${name}>`;
}
function randomXmp() {
  const descs = Array.from({ length: ri(0, 3) }, () => {
    const attrs = Array.from({ length: ri(0, 5) }, attr).join(pick([' ', '\n  ', ' \t']));
    const kids = Array.from({ length: ri(0, 4) }, () => element(0)).join(pick(['', '\n']));
    return rnd() < 0.2 ? `<rdf:Description rdf:about="" ${attrs}/>` : `<rdf:Description rdf:about=""${attrs ? ' ' + attrs : ''}>${kids}</rdf:Description>`;
  });
  const body = descs.length ? descs.join('') : Array.from({ length: ri(1, 3) }, () => element(0)).join('');
  const wrap = rnd() < 0.8;
  return `${rnd() < 0.7 ? '<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>' : ''}${wrap ? '<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">' : ''}${body}${wrap ? '</rdf:RDF></x:xmpmeta>' : ''}`;
}

// ── containers ──
const seg = (marker, payload) => Buffer.concat([Buffer.from([0xff, marker, ((payload.length + 2) >> 8) & 0xff, (payload.length + 2) & 0xff]), payload]);
function jpeg(t, x) {
  const segs = [];
  if (rnd() < 0.5) segs.push(seg(0xe0, Buffer.from('JFIF\0\x01\x01\0\0\x01\0\x01\0\0', 'latin1')));
  const ex = t && seg(0xe1, Buffer.concat([Buffer.from('Exif\0\0', 'latin1'), t]));
  const xm = x && seg(0xe1, Buffer.concat([Buffer.from('http://ns.adobe.com/xap/1.0/\0', 'latin1'), Buffer.from(x, 'utf8')]));
  const list = [ex, xm].filter(Boolean);
  if (rnd() < 0.5) list.reverse();
  segs.push(...list);
  if (rnd() < 0.2) segs.push(seg(0xe2, Buffer.from('ICC_PROFILE\0\x01\x01xxxx', 'latin1')));
  if (rnd() < 0.15 && t) segs.push(seg(0xe1, Buffer.concat([Buffer.from('Exif\0\0', 'latin1'), randomTiff()])));
  const sof = Buffer.from([0xff, 0xc0, 0, 11, 8, 0, ri(1, 255), 0, ri(1, 255), 1, 1, 0x11, 0]);
  const sos = Buffer.from([0xff, 0xda, 0, 8, 1, 1, 0, 0, 0x3f, 0, 1, 2, 3, 0xff, 0xd9]);
  return Buffer.concat([Buffer.from([0xff, 0xd8]), ...segs, sof, sos]);
}
function crc32(buf) {
  let c = ~0;
  for (const b of buf) {
    c ^= b;
    for (let k = 0; k < 8; k++) c = (c >>> 1) ^ (0xedb88320 & -(c & 1));
  }
  return ~c >>> 0;
}
const chunk = (type, data) => {
  const t = Buffer.from(type, 'latin1');
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([t, data])));
  return Buffer.concat([len, t, data, crc]);
};
function png(t, x) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(ri(1, 4000), 0);
  ihdr.writeUInt32BE(ri(1, 4000), 4);
  ihdr[8] = 8;
  ihdr[9] = 2;
  const parts = [Buffer.from('\x89PNG\r\n\x1a\n', 'latin1'), chunk('IHDR', ihdr)];
  if (rnd() < 0.5) parts.push(chunk('tEXt', Buffer.from(`${pick(['Software', 'Make', 'Comment', 'Model'])}\0${pick(VALS)}`, 'utf8')));
  if (t) parts.push(chunk('eXIf', t));
  if (x) parts.push(chunk('iTXt', Buffer.concat([Buffer.from('XML:com.adobe.xmp\0\0\0\0\0', 'latin1'), Buffer.from(x, 'utf8')])));
  parts.push(chunk('IDAT', Buffer.from([0x78, 0x9c, 3, 0, 0, 0, 0, 1])), chunk('IEND', Buffer.alloc(0)));
  return Buffer.concat(parts);
}

const enc = (v) => {
  if (v === undefined) return { $u: 1 };
  if (typeof v === 'number' && !Number.isFinite(v)) return { $n: String(v) };
  if (v instanceof Date) return Number.isNaN(+v) ? { $n: 'NaN' } : v.getTime();
  if (ArrayBuffer.isView(v)) return { $t: [...v].map(enc) };
  if (Array.isArray(v)) return v.map(enc);
  if (v && typeof v === 'object') {
    const o = {};
    for (const [k, x] of Object.entries(v)) if (k !== 'errors') o[k] = enc(x);
    return o;
  }
  return v;
};
const TMP = new URL('./fuzz.tmp', import.meta.url).pathname;
const run = async (buf, opts) => {
  try {
    // by path, like the server (exifr's chunked file reader)
    fs.writeFileSync(TMP, buf);
    return enc((await exifr.parse(TMP, opts)) ?? null);
  } catch (e) {
    return { $err: String(e.message) };
  }
};

const cases = [];
for (let i = 0; i < 700; i++) {
  const t = rnd() < 0.85 ? randomTiff() : null;
  const x = rnd() < 0.6 ? randomXmp() : null;
  const kind = i % 7 === 6 ? 'png' : i % 11 === 10 && t ? 'tiff' : 'jpeg';
  const buf = kind === 'png' ? png(t, x) : kind === 'tiff' ? t : jpeg(t, x);
  cases.push({ kind, b64: buf.toString('base64'), group: await run(buf, GROUP), panel: await run(buf, PANEL) });
}
fs.writeFileSync(process.argv[2], JSON.stringify(cases));
console.log(cases.length, 'cases', cases.filter((c) => c.group && !c.group.$err).length, 'parsed');
