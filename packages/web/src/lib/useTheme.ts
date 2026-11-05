import { useEffect } from 'react';
import { usePrefs } from '@/store/prefs';
import '@/backend/desktop';

function mix(hex: string, amount: number): string {
  // rotate hue a little for the gradient partner colors
  const n = parseInt(hex.slice(1), 16);
  let r = (n >> 16) & 255,
    g = (n >> 8) & 255,
    b = n & 255;
  const max = Math.max(r, g, b) / 255,
    min = Math.min(r, g, b) / 255;
  let h = 0;
  const l = (max + min) / 2;
  const d = max - min;
  const s = d === 0 ? 0 : d / (1 - Math.abs(2 * l - 1));
  if (d) {
    const rr = r / 255,
      gg = g / 255,
      bb = b / 255;
    if (max === rr) h = ((gg - bb) / d) % 6;
    else if (max === gg) h = (bb - rr) / d + 2;
    else h = (rr - gg) / d + 4;
    h *= 60;
  }
  h = (h + amount + 360) % 360;
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  const m = l - c / 2;
  [r, g, b] = h < 60 ? [c, x, 0] : h < 120 ? [x, c, 0] : h < 180 ? [0, c, x] : h < 240 ? [0, x, c] : h < 300 ? [x, 0, c] : [c, 0, x];
  return `#${[r, g, b].map((v) => Math.round((v + m) * 255).toString(16).padStart(2, '0')).join('')}`;
}

/** Applies theme, accent and motion preferences to <html>. */
export function useThemeEffect() {
  const { theme, accent, motion, lang } = usePrefs();
  useEffect(() => {
    document.documentElement.lang = lang;
  }, [lang]);
  useEffect(() => {
    const root = document.documentElement;
    const apply = () => {
      const dark = theme === 'dark' || (theme === 'system' && window.matchMedia('(prefers-color-scheme: dark)').matches);
      root.dataset.theme = dark ? 'dark' : 'light';
      window.peekDesktop?.setTheme?.(dark);
      document.querySelector('meta[name="theme-color"]')?.setAttribute('content', dark ? '#09090d' : '#f4f4f7');
    };
    apply();
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    mq.addEventListener('change', apply);
    return () => mq.removeEventListener('change', apply);
  }, [theme]);
  useEffect(() => {
    const root = document.documentElement;
    root.style.setProperty('--accent', accent);
    root.style.setProperty('--accent-2', mix(accent, 38));
    root.style.setProperty('--accent-3', mix(accent, 75));
  }, [accent]);
  useEffect(() => {
    document.documentElement.dataset.motion = motion;
  }, [motion]);
}
