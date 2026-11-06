import { describe, expect, it } from 'vitest';
import { DEFAULT_SETTINGS } from '@peekaround/shared';
import { DEFAULT_PREFS } from '@/store/prefs';
import { DEFAULT_SETTINGS as SLIDE_DEFAULTS } from '@/features/slideshow/show';
import { makeBackup, parseBackup, sanitizePrefs, serverResetPatch } from './backup';
import { matchesQuery } from './search';

describe('settings backup', () => {
  it('round-trips prefs, slideshow and server settings', () => {
    const prefs = { ...DEFAULT_PREFS, theme: 'light' as const, tileNames: true, zoomStep: 2 };
    const b = makeBackup(prefs, { ...SLIDE_DEFAULTS, duration: 7 }, { ...DEFAULT_SETTINGS, hostBrowseRoots: ['/secret'], thumbQuality: 'high' }, new Date(0));
    const back = parseBackup(JSON.stringify(b))!;
    expect(back.prefs.theme).toBe('light');
    expect(back.prefs.tileNames).toBe(true);
    expect(back.prefs.zoomStep).toBe(2);
    expect(back.slideshow.duration).toBe(7);
    expect(back.server?.thumbQuality).toBe('high');
    // never exported: the server admin's browse roots
    expect(back.server && 'hostBrowseRoots' in back.server).toBe(false);
    // the slideshow music pick (a media item of this library) is not part of the backup
    expect('music' in back.slideshow).toBe(false);
  });

  it('rejects files that are not backups', () => {
    expect(parseBackup('nope')).toBeNull();
    expect(parseBackup('{"app":"other"}')).toBeNull();
    expect(parseBackup(JSON.stringify({ app: 'peekaround', kind: 'settings', version: 2 }))).toBeNull();
  });

  it('drops unknown keys, wrong types and values outside the allowed set', () => {
    const p = sanitizePrefs({ theme: 'neon', lang: 'en', rowHeight: 9999, accent: 'red', evil: 1, tileNames: 'yes', videoSpeed: Number.NaN, viewerBg: 'black' });
    expect(p).toEqual({ lang: 'en', rowHeight: 480, viewerBg: 'black' });
  });

  it('reset keeps the home time zone and host roots', () => {
    const r = serverResetPatch();
    expect('homeTimeZone' in r).toBe(false);
    expect('hostBrowseRoots' in r).toBe(false);
    expect(r.thumbQuality).toBe('normal');
  });
});

describe('settings search', () => {
  it('matches folded TR / EN words in any order', () => {
    const text = 'Küçük resim kalitesi Thumbnail quality webp';
    expect(matchesQuery(text, '')).toBe(true);
    expect(matchesQuery(text, 'kucuk')).toBe(true);
    expect(matchesQuery(text, 'KALİTE')).toBe(true);
    expect(matchesQuery(text, 'quality thumb')).toBe(true);
    expect(matchesQuery(text, 'video')).toBe(false);
  });
});
