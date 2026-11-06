import { describe, expect, it } from 'vitest';
import type { MediaItem } from '@peekaround/shared';
import { translate } from '@/i18n';
import { firstPicture, hasPicture, mobileBarOrder, viewable } from './mediaKinds';

const it_ = (id: string, kind: MediaItem['kind']) => ({ id, kind }) as MediaItem;
const src = (items: (MediaItem | undefined)[]) => ({ total: items.length, getAt: (i: number) => items[i] });

describe('media kinds', () => {
  it('the viewer pages through photos, videos and audio — never archives', () => {
    expect(['image', 'video', 'audio', 'archive'].map((k) => viewable(it_('1', k as MediaItem['kind'])))).toEqual([true, true, true, false]);
    expect(viewable(undefined)).toBe(false);
    expect(hasPicture(it_('1', 'audio'))).toBe(false);
  });

  it('covers skip audio (no thumbnail) and wait for unloaded pages', () => {
    expect(firstPicture(src([it_('a', 'audio'), it_('v', 'video'), it_('p', 'image')]))).toBe('v');
    expect(firstPicture(src([it_('a', 'audio'), undefined, it_('p', 'image')]))).toBeNull();
    expect(firstPicture(src([it_('a', 'audio')]))).toBeNull();
    expect(firstPicture(null)).toBeNull();
  });

  it('phone viewer bar: delete once and last, also with few actions (audio: download + delete)', () => {
    const info = { id: 'info' };
    const ids = (l: { id: string }[]) => mobileBarOrder(l, info).map((a) => a.id);
    expect(ids([{ id: 'download' }, { id: 'delete' }])).toEqual(['download', 'info', 'delete']);
    expect(ids([{ id: 'share' }, { id: 'download' }, { id: 'edit' }, { id: 'favorite' }, { id: 'delete' }])).toEqual(['share', 'download', 'edit', 'favorite', 'info', 'delete']);
    expect(ids([{ id: 'download' }])).toEqual(['download', 'info']);
  });
});

describe('library hub subtitle', () => {
  it('singular / plural library count', () => {
    expect(translate('en', 'lib2.hub.subtitle', { n: 1, items: '3 items' })).toBe('1 library · 3 items');
    expect(translate('en', 'lib2.hub.subtitle', { n: 2, items: '3 items' })).toBe('2 libraries · 3 items');
    expect(translate('tr', 'lib2.hub.subtitle', { n: 2, items: '3 öğe' })).toBe('2 kütüphane · 3 öğe');
  });
});
