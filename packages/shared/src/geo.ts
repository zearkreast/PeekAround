/**
 * Offline reverse geocoding (pure, environment agnostic).
 *
 * Data: GeoNames (https://www.geonames.org), CC BY 4.0 — built by scripts/build-geo.mjs into
 * packages/web/public/geo/cities.bin (gzip). Callers decompress (zlib.gunzipSync on Node,
 * DecompressionStream('gzip') in browsers) and pass the raw bytes to createGeoIndex().
 *
 * Binary layout (little endian):
 *   "PKG1" | u32 jsonLength | JSON header (UTF-8) | pad to 4 |
 *   i32 lat×1e5 [n] | i32 lon×1e5 [n] | u32 population [n] | u32 nameIndex [n] | u16 adminIndex [n] | u8 featureIndex [n]
 * Every column is byte-plane shuffled: byte 0 of all n values, then byte 1 of all values, …
 * JSON header: { v, n, attribution, cc: string[], admin: [ccIndex, code, name][], fcode: string[], names: string[] }
 */
import type { PlaceInfo } from './index';

export const GEO_ASSET_PATH = 'geo/cities.bin';
export const GEO_MAX_KM = 80;

export interface GeoHeader {
  v: number;
  n: number;
  attribution: string;
  cc: string[];
  admin: [number, string, string][];
  fcode: string[];
  names: string[];
}

export interface GeoMatch extends PlaceInfo {
  id: number;
  /** GeoNames admin1 code ('34' for İstanbul), null when unknown */
  admin1Code: string | null;
  lat: number;
  lon: number;
  km: number;
}

export interface GeoIndex {
  readonly size: number;
  readonly attribution: string;
  /** nearest populated place within maxKm (default 80), (0,0) and invalid coordinates → null */
  nearest(lat: number, lon: number, maxKm?: number): GeoMatch | null;
  /** place for an id returned by nearest() */
  place(id: number): PlaceInfo | null;
  /** admin1 display name for a place key `${cc}-${code}` (or null) */
  admin1Name(cc: string, code: string | null): string | null;
}

const R = 6371.0088;
const RAD = Math.PI / 180;

export function distanceKm(lat1: number, lon1: number, lat2: number, lon2: number): number {
  const dLat = (lat2 - lat1) * RAD;
  const dLon = (lon2 - lon1) * RAD;
  const a = Math.sin(dLat / 2) ** 2 + Math.cos(lat1 * RAD) * Math.cos(lat2 * RAD) * Math.sin(dLon / 2) ** 2;
  return 2 * R * Math.asin(Math.min(1, Math.sqrt(a)));
}

/** finite, in range and not the (0,0) "null island" placeholder */
export function isValidGps(lat: unknown, lon: unknown): lat is number {
  if (typeof lat !== 'number' || typeof lon !== 'number') return false;
  if (!Number.isFinite(lat) || !Number.isFinite(lon)) return false;
  if (Math.abs(lat) > 90 || Math.abs(lon) > 180) return false;
  return !(Math.abs(lat) < 1e-4 && Math.abs(lon) < 1e-4);
}

/** ISO 6709 string as written by phones into video metadata: "+41.0123+028.9744+050.000/" */
export function parseIso6709(s: unknown): { lat: number; lon: number } | null {
  if (typeof s !== 'string' || s.length > 256) return null;
  const m = /^\s*([+-]\d+(?:\.\d+)?)([+-]\d+(?:\.\d+)?)([+-]\d+(?:\.\d+)?)?(?:CRS[^/]*)?\/?\s*$/.exec(s);
  if (!m) return null;
  const lat = parseDeg(m[1], 2);
  const lon = parseDeg(m[2], 3);
  return isValidGps(lat, lon) ? { lat, lon } : null;
}

/** ±DD.DDD, ±DDMM.MMM, ±DDMMSS.SS (degree digits: 2 for lat, 3 for lon) */
function parseDeg(s: string, degDigits: number): number {
  const sign = s[0] === '-' ? -1 : 1;
  const body = s.slice(1);
  const intLen = body.split('.')[0].length;
  if (intLen <= degDigits) return sign * Number(body);
  const deg = Number(body.slice(0, degDigits));
  if (intLen <= degDigits + 2) return sign * (deg + Number(body.slice(degDigits)) / 60);
  return sign * (deg + Number(body.slice(degDigits, degDigits + 2)) / 60 + Number(body.slice(degDigits + 2)) / 3600);
}

/** Minimal UTF-8 decoder (no TextDecoder dependency). */
export function utf8Decode(b: Uint8Array, start = 0, end = b.length): string {
  let out = '';
  const chunk: number[] = [];
  for (let i = start; i < end; ) {
    const c = b[i++];
    let cp: number;
    if (c < 0x80) cp = c;
    else if (c < 0xe0) cp = ((c & 0x1f) << 6) | (b[i++] & 0x3f);
    else if (c < 0xf0) cp = ((c & 0x0f) << 12) | ((b[i++] & 0x3f) << 6) | (b[i++] & 0x3f);
    else cp = ((c & 0x07) << 18) | ((b[i++] & 0x3f) << 12) | ((b[i++] & 0x3f) << 6) | (b[i++] & 0x3f);
    if (cp > 0xffff) {
      cp -= 0x10000;
      chunk.push(0xd800 + (cp >> 10), 0xdc00 + (cp & 0x3ff));
    } else chunk.push(cp);
    if (chunk.length > 8192) {
      out += String.fromCharCode(...chunk);
      chunk.length = 0;
    }
  }
  return out + String.fromCharCode(...chunk);
}

export function isGzip(b: Uint8Array): boolean {
  return b.length > 2 && b[0] === 0x1f && b[1] === 0x8b;
}

const CELL = 0.25; // degrees
const COLS = 360 / CELL;
const ROWS = 180 / CELL;

function cellOf(lat: number, lon: number): [number, number] {
  const r = Math.min(ROWS - 1, Math.max(0, Math.floor((lat + 90) / CELL)));
  const c = (((Math.floor((lon + 180) / CELL) % COLS) + COLS) % COLS);
  return [r, c];
}

export function createGeoIndex(bytes: Uint8Array): GeoIndex {
  if (isGzip(bytes)) throw new Error('geo asset is gzip compressed — decompress it first');
  const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (bytes[0] !== 0x50 || bytes[1] !== 0x4b || bytes[2] !== 0x47 || bytes[3] !== 0x31) throw new Error('not a PeekAround geo asset');
  const jl = dv.getUint32(4, true);
  const header = JSON.parse(utf8Decode(bytes, 8, 8 + jl)) as GeoHeader;
  const n = header.n;
  let off = 8 + jl;
  off = (off + 3) & ~3;
  const lat = new Float64Array(n);
  const lon = new Float64Array(n);
  const nameIdx = new Uint32Array(n);
  const adminIdx = new Uint16Array(n);
  // un-shuffle a byte-plane column into little-endian values
  const col = (size: number, signed: boolean, into: { [i: number]: number }) => {
    for (let i = 0; i < n; i++) {
      let v = 0;
      for (let k = size - 1; k >= 0; k--) v = v * 256 + bytes[off + k * n + i];
      if (signed && size === 4 && v >= 0x80000000) v -= 0x100000000;
      into[i] = v;
    }
    off += n * size;
  };
  col(4, true, lat);
  col(4, true, lon);
  for (let i = 0; i < n; i++) {
    lat[i] /= 1e5;
    lon[i] /= 1e5;
  }
  off += n * 4; // population (not needed for lookups)
  col(4, false, nameIdx);
  col(2, false, adminIdx);

  // grid (CSR): cellStart[cell] .. cellStart[cell+1] in cellItems
  const cells = ROWS * COLS;
  const cellStart = new Uint32Array(cells + 1);
  const cellOfItem = new Uint32Array(n);
  for (let i = 0; i < n; i++) {
    const [r, c] = cellOf(lat[i], lon[i]);
    const k = r * COLS + c;
    cellOfItem[i] = k;
    cellStart[k + 1]++;
  }
  for (let k = 0; k < cells; k++) cellStart[k + 1] += cellStart[k];
  const fill = cellStart.slice(0, cells);
  const cellItems = new Uint32Array(n);
  for (let i = 0; i < n; i++) cellItems[fill[cellOfItem[i]]++] = i;

  const adminByKey = new Map<string, string>();
  for (const [ci, code, name] of header.admin) adminByKey.set(`${header.cc[ci]}-${code}`, name);

  const place = (id: number): PlaceInfo | null => {
    if (id < 0 || id >= n) return null;
    const [ci, code, name] = header.admin[adminIdx[id]];
    const cc = header.cc[ci];
    return { city: header.names[nameIdx[id]], admin1: name || null, cc, key: `${cc}-${code || '00'}` };
  };

  return {
    size: n,
    attribution: header.attribution,
    place,
    admin1Name(cc, code) {
      return adminByKey.get(`${cc}-${code ?? ''}`) || null;
    },
    nearest(qLat, qLon, maxKm = GEO_MAX_KM) {
      if (!isValidGps(qLat, qLon)) return null;
      const cos = Math.max(0.01, Math.cos(qLat * RAD));
      // ring search around the query cell; stop once a ring can't hold anything closer
      const [r0, c0] = cellOf(qLat, qLon);
      const cellKm = CELL * 111.2 * Math.min(1, cos);
      const maxRing = Math.min(Math.floor(COLS / 2), Math.ceil(maxKm / cellKm) + 1);
      let best = -1;
      let bestKm = maxKm;
      const visit = (r: number, c: number) => {
        if (r < 0 || r >= ROWS) return;
        const k = r * COLS + ((((c % COLS) + COLS) % COLS));
        for (let j = cellStart[k]; j < cellStart[k + 1]; j++) {
          const i = cellItems[j];
          let dLon = Math.abs(lon[i] - qLon);
          if (dLon > 180) dLon = 360 - dLon;
          // equirectangular approximation — accurate to well under 1% at < 100 km
          const x = dLon * cos;
          const y = lat[i] - qLat;
          const km = Math.sqrt(x * x + y * y) * RAD * R;
          if (km < bestKm) {
            bestKm = km;
            best = i;
          }
        }
      };
      for (let ring = 0; ring <= maxRing; ring++) {
        if (best >= 0 && (ring - 1) * cellKm > bestKm) break;
        if (ring === 0) visit(r0, c0);
        else {
          for (let dc = -ring; dc <= ring; dc++) {
            visit(r0 - ring, c0 + dc);
            visit(r0 + ring, c0 + dc);
          }
          for (let dr = -ring + 1; dr <= ring - 1; dr++) {
            visit(r0 + dr, c0 - ring);
            visit(r0 + dr, c0 + ring);
          }
        }
      }
      if (best < 0) return null;
      const p = place(best)!;
      const code = header.admin[adminIdx[best]][1];
      return { ...p, admin1Code: code && code !== '00' ? code : null, id: best, lat: lat[best], lon: lon[best], km: bestKm };
    },
  };
}
