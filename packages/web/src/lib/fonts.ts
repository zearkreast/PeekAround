import { useEffect, useMemo } from 'react';
import { useQuery } from '@tanstack/react-query';
import type { FontInfo } from '@peekaround/shared';
import { useBackend, type Backend } from '@/backend';

// Bundled, open-licensed fonts (Turkish glyphs included via latin-ext subsets).
import '@fontsource/playfair-display/400.css';
import '@fontsource/playfair-display/700.css';
import '@fontsource/playfair-display/400-italic.css';
import '@fontsource/montserrat/400.css';
import '@fontsource/montserrat/700.css';
import '@fontsource/montserrat/900.css';
import '@fontsource/bebas-neue/400.css';
import '@fontsource/oswald/400.css';
import '@fontsource/oswald/700.css';
import '@fontsource/anton/400.css';
import '@fontsource/caveat/400.css';
import '@fontsource/caveat/700.css';
import '@fontsource/dancing-script/400.css';
import '@fontsource/dancing-script/700.css';
import '@fontsource/pacifico/400.css';
import '@fontsource/lobster/400.css';
import '@fontsource/space-mono/400.css';
import '@fontsource/space-mono/700.css';

export interface FontFamily {
  family: string;
  label: string;
  bundled: boolean;
  weights: number[];
  italic: boolean;
  /** custom font ids belonging to this family */
  ids: string[];
  category: 'sans' | 'serif' | 'display' | 'script' | 'mono' | 'custom';
}

export const BUNDLED_FONTS: FontFamily[] = [
  { family: 'Inter Variable', label: 'Inter', bundled: true, weights: [300, 400, 500, 600, 700, 800, 900], italic: false, ids: [], category: 'sans' },
  { family: 'Bricolage Grotesque Variable', label: 'Bricolage Grotesque', bundled: true, weights: [400, 500, 600, 700, 800], italic: false, ids: [], category: 'sans' },
  { family: 'Montserrat', label: 'Montserrat', bundled: true, weights: [400, 700, 900], italic: false, ids: [], category: 'sans' },
  { family: 'Playfair Display', label: 'Playfair Display', bundled: true, weights: [400, 700], italic: true, ids: [], category: 'serif' },
  { family: 'Bebas Neue', label: 'Bebas Neue', bundled: true, weights: [400], italic: false, ids: [], category: 'display' },
  { family: 'Oswald', label: 'Oswald', bundled: true, weights: [400, 700], italic: false, ids: [], category: 'display' },
  { family: 'Anton', label: 'Anton', bundled: true, weights: [400], italic: false, ids: [], category: 'display' },
  { family: 'Lobster', label: 'Lobster', bundled: true, weights: [400], italic: false, ids: [], category: 'script' },
  { family: 'Pacifico', label: 'Pacifico', bundled: true, weights: [400], italic: false, ids: [], category: 'script' },
  { family: 'Caveat', label: 'Caveat', bundled: true, weights: [400, 700], italic: false, ids: [], category: 'script' },
  { family: 'Dancing Script', label: 'Dancing Script', bundled: true, weights: [400, 700], italic: false, ids: [], category: 'script' },
  { family: 'Space Mono', label: 'Space Mono', bundled: true, weights: [400, 700], italic: false, ids: [], category: 'mono' },
];

const registered = new Map<string, FontFace>();

/** Register uploaded fonts with the document so CSS and canvas can use them. */
export async function registerFonts(backend: Backend, fonts: FontInfo[]) {
  await Promise.all(
    fonts.map(async (f) => {
      const key = `${backend.id}:${f.id}`;
      if (registered.has(key)) return;
      const face = new FontFace(f.family, `url("${backend.fontUrl(f)}")`, {
        weight: String(f.weight),
        style: f.style,
        display: 'swap',
      });
      registered.set(key, face);
      try {
        await face.load();
        document.fonts.add(face);
      } catch {
        registered.delete(key);
      }
    }),
  );
}

function groupCustom(fonts: FontInfo[]): FontFamily[] {
  const map = new Map<string, FontFamily>();
  for (const f of fonts) {
    let g = map.get(f.family);
    if (!g) {
      g = { family: f.family, label: f.family, bundled: false, weights: [], italic: false, ids: [], category: 'custom' };
      map.set(f.family, g);
    }
    if (!g.weights.includes(f.weight)) g.weights.push(f.weight);
    if (f.style === 'italic') g.italic = true;
    g.ids.push(f.id);
  }
  for (const g of map.values()) g.weights.sort((a, b) => a - b);
  return [...map.values()];
}

/** All fonts usable in text layers: uploaded ones first, then the bundled set. */
export function useFonts() {
  const backend = useBackend();
  const q = useQuery({ queryKey: ['fonts', backend.id], queryFn: () => backend.fonts(), staleTime: 60_000 });
  useEffect(() => {
    if (q.data?.length) void registerFonts(backend, q.data);
  }, [backend, q.data]);
  const families = useMemo(() => [...groupCustom(q.data ?? []), ...BUNDLED_FONTS], [q.data]);
  return { raw: q.data ?? [], families, custom: groupCustom(q.data ?? []), loading: q.isLoading, refetch: q.refetch };
}

/** Make sure a font is ready before drawing it on a canvas. */
export async function ensureFont(family: string, weight = 400, italic = false): Promise<void> {
  try {
    await document.fonts.load(`${italic ? 'italic ' : ''}${weight} 48px "${family}"`, 'AaĞğŞşİıÇçÖöÜü');
  } catch {
    /* fall back to whatever the browser has */
  }
}

export function cssFont(family: string, weight: number, italic: boolean, size: number): string {
  return `${italic ? 'italic ' : ''}${weight} ${size}px "${family}", "Inter Variable", sans-serif`;
}
