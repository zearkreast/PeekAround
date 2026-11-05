import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';
import { createGeoIndex, foldText, GEO_ASSET_PATH, type GeoIndex, type GeoMatch, type PlaceInfo } from '@peekaround/shared';

/**
 * Offline reverse geocoder backed by the GeoNames asset that ships with the web UI
 * (packages/web/public/geo/cities.bin → copied by Vite into web/dist/geo/).
 * Looked up in: $PEEK_GEO_FILE, <webDir>/geo/, then packages/web/public/geo/ relative to this module (dev/tests).
 */
export class GeoService {
  private index: GeoIndex | null = null;
  private loaded = false;
  readonly file: string | null;
  private placeTextCache = new Map<string, string>();
  private countryNames = new Map<string, string[]>();

  constructor(webDir: string | null) {
    this.file = GeoService.locate(webDir);
  }

  static locate(webDir: string | null): string | null {
    let here: string | null = null;
    try {
      here = path.dirname(fileURLToPath(import.meta.url));
    } catch {
      here = null;
    }
    const candidates = [
      process.env.PEEK_GEO_FILE,
      webDir ? path.join(webDir, GEO_ASSET_PATH) : null,
      here ? path.resolve(here, '../../web/public', GEO_ASSET_PATH) : null,
      here ? path.resolve(here, '../web', GEO_ASSET_PATH) : null,
      here ? path.resolve(here, 'web', GEO_ASSET_PATH) : null,
    ];
    return candidates.find((f): f is string => !!f && fs.existsSync(f)) ?? null;
  }

  get available(): boolean {
    return !!this.get();
  }

  private get(): GeoIndex | null {
    if (this.loaded) return this.index;
    this.loaded = true;
    if (!this.file) return null;
    try {
      let bytes: Uint8Array = fs.readFileSync(this.file);
      if (bytes[0] === 0x1f && bytes[1] === 0x8b) bytes = zlib.gunzipSync(bytes);
      this.index = createGeoIndex(bytes);
    } catch {
      this.index = null;
    }
    return this.index;
  }

  nearest(lat: number | null, lon: number | null): GeoMatch | null {
    if (lat == null || lon == null) return null;
    return this.get()?.nearest(lat, lon) ?? null;
  }

  /** PlaceInfo from stored columns (city label + cc + admin1 code). */
  placeInfo(city: string | null, cc: string | null, admin1: string | null): PlaceInfo | null {
    if (!cc) return null;
    return { city: city ?? '', admin1: this.admin1Name(cc, admin1), cc, key: placeKey(cc, admin1) };
  }

  admin1Name(cc: string, code: string | null): string | null {
    return this.get()?.admin1Name(cc, code ?? '') ?? null;
  }

  /** country display names (en, tr + native) for search */
  countryNamesOf(cc: string): string[] {
    let n = this.countryNames.get(cc);
    if (!n) {
      n = [];
      for (const lang of ['en', 'tr', 'de', 'fr', 'es']) {
        try {
          const name = new Intl.DisplayNames([lang], { type: 'region' }).of(cc);
          if (name && name !== cc && !n.includes(name)) n.push(name);
        } catch {
          /* ignore */
        }
      }
      this.countryNames.set(cc, n);
    }
    return n;
  }

  /** folded "admin1 country names" text for search matching (cached per place group) */
  placeText(cc: string | null, admin1: string | null): string {
    if (!cc) return '';
    const k = `${cc}-${admin1 ?? ''}`;
    let t = this.placeTextCache.get(k);
    if (t === undefined) {
      t = foldText([this.admin1Name(cc, admin1) ?? '', ...this.countryNamesOf(cc)].join(' '));
      this.placeTextCache.set(k, t);
    }
    return t;
  }
}

export function placeKey(cc: string, admin1: string | null): string {
  return `${cc}-${admin1 || '00'}`;
}
