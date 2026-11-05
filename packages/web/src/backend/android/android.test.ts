import { describe, expect, it, vi } from 'vitest';

vi.mock('@capacitor/core', () => ({ registerPlugin: () => ({}) }));

const { encId, decId } = await import('./local');
const { readFontInfo } = await import('./fontname');

describe('android local backend helpers', () => {
  it('round-trips Turkish paths through URL-safe ids', () => {
    const p = '/storage/emulated/0/DCIM/Düğün Albümü/İstanbul ş ğ ı.jpg';
    const id = encId(p);
    expect(id).not.toMatch(/[+/=]/);
    expect(decId(id)).toBe(p);
  });

  it('falls back to the file name for compressed fonts', () => {
    const r = readFontInfo(new ArrayBuffer(16), 'My-Brush_Font.woff2');
    expect(r.family).toBe('My Brush Font');
    expect(r.weight).toBe(400);
  });
});
