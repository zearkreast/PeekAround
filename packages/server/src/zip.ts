import fs from 'node:fs';
import fsp from 'node:fs/promises';
import path from 'node:path';
import { pipeline } from 'node:stream/promises';
import { Transform } from 'node:stream';
import yauzl from 'yauzl';
import { normalizeRel, sanitizeName } from './paths.js';

// Code page 857 (Turkish DOS) — what Windows' built-in zip uses on Turkish systems
// when it doesn't set the UTF-8 flag. ASCII range is identical.
const CP857_HIGH =
  'ÇüéâäàåçêëèïîıÄÅ' +
  'ÉæÆôöòûùİÖÜø£ØŞş' +
  'áíóúñÑĞğ¿®¬½¼¡«»' +
  '░▒▓│┤ÁÂÀ©╣║╗╝¢¥┐' +
  '└┴┬├─┼ãÃ╚╔╩╦╠═╬¤' +
  'ºªÊËÈ€ÍÎÏ┘┌█▄¦Ì▀' +
  'ÓßÔÒõÕµ_×ÚÛÙìÿ¯´' +
  '­±_¾¶§÷¸°¨·¹³²■ ';

function decodeCp857(buf: Buffer): string {
  let s = '';
  for (const b of buf) s += b < 0x80 ? String.fromCharCode(b) : CP857_HIGH[b - 0x80];
  return s;
}

const utf8 = new TextDecoder('utf-8', { fatal: true });

export function decodeZipName(raw: Buffer, flags: number, extraFields: { id: number; data: Buffer }[]): string {
  // Info-ZIP Unicode Path Extra Field
  const up = extraFields.find((f) => f.id === 0x7075);
  if (up && up.data.length > 5 && up.data[0] === 1) return up.data.subarray(5).toString('utf8');
  if (flags & 0x800) return raw.toString('utf8');
  try {
    return utf8.decode(raw);
  } catch {
    return decodeCp857(raw);
  }
}

export interface ExtractResult {
  files: string[]; // posix rel paths of written files
  bytes: number;
}

const SKIP = (rel: string) => rel.startsWith('__MACOSX/') || /(^|\/)\.DS_Store$/.test(rel) || /(^|\/)Thumbs\.db$/i.test(rel);

/**
 * Extract a zip into destDir with zip-slip protection and a size guard.
 * Entries with absolute or ../ paths are skipped.
 */
export async function extractZip(file: string, destDir: string, maxBytes = 200 * 1024 ** 3): Promise<ExtractResult> {
  const zip = await new Promise<yauzl.ZipFile>((res, rej) =>
    yauzl.open(file, { lazyEntries: true, decodeStrings: false, autoClose: true }, (e, z) => (e ? rej(e) : res(z!))),
  );
  await fsp.mkdir(destDir, { recursive: true });
  const result: ExtractResult = { files: [], bytes: 0 };
  const root = path.resolve(destDir);

  await new Promise<void>((resolve, reject) => {
    zip.on('error', reject);
    zip.on('end', () => resolve());
    zip.on('entry', async (entry: yauzl.Entry) => {
      try {
        const rawName = entry.fileName as unknown as Buffer;
        const name = decodeZipName(rawName, entry.generalPurposeBitFlag, entry.extraFields);
        let rel: string;
        try {
          rel = normalizeRel(name);
        } catch {
          zip.readEntry();
          return; // traversal attempt → skip
        }
        if (!rel || SKIP(rel) || /^[a-zA-Z]:/.test(name) || name.startsWith('/')) {
          zip.readEntry();
          return;
        }
        rel = rel.split('/').map(sanitizeName).join('/');
        const target = path.resolve(root, ...rel.split('/'));
        if (!target.startsWith(root + path.sep)) {
          zip.readEntry();
          return;
        }
        if (/\/$/.test(name)) {
          await fsp.mkdir(target, { recursive: true });
          zip.readEntry();
          return;
        }
        if (result.bytes + entry.uncompressedSize > maxBytes) throw new Error('Archive too large');
        await fsp.mkdir(path.dirname(target), { recursive: true });
        const rs = await new Promise<NodeJS.ReadableStream>((res, rej) =>
          zip.openReadStream(entry, (e, s) => (e ? rej(e) : res(s!))),
        );
        let written = 0;
        const counter = new Transform({
          transform(chunk: Buffer, _enc, cb) {
            written += chunk.length;
            result.bytes += chunk.length;
            if (result.bytes > maxBytes || written > entry.uncompressedSize + 1024) cb(new Error('Archive too large or corrupt'));
            else cb(null, chunk);
          },
        });
        await pipeline(rs, counter, fs.createWriteStream(target));
        result.files.push(rel);
        zip.readEntry();
      } catch (e) {
        zip.close();
        reject(e);
      }
    });
    zip.readEntry();
  });
  return result;
}
