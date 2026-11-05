import { describe, expect, it } from 'vitest';
import { collapseNested, crumbsOf, loadRecent, parentOf, prettyPath, saveRecent, toggle, volumeOf } from './phonePicker';

const L = { internal: 'Dahili', sd: 'SD' };

describe('PhoneFolderPicker state helpers', () => {
  it('builds breadcrumbs from the volume root', () => {
    expect(crumbsOf('/storage/emulated/0/DCIM/Camera/', L)).toEqual([
      { label: 'Dahili', path: '/storage/emulated/0' },
      { label: 'DCIM', path: '/storage/emulated/0/DCIM' },
      { label: 'Camera', path: '/storage/emulated/0/DCIM/Camera' },
    ]);
    expect(crumbsOf('/storage/1A2B-3C4D/Pictures', L).map((c) => c.label)).toEqual(['SD', 'Pictures']);
    expect(volumeOf('/storage/emulated/10/x').base).toBe('/storage/emulated/10');
    expect(prettyPath('/storage/emulated/0/Pictures/Tatil 2024', L)).toBe('Pictures/Tatil 2024');
    expect(prettyPath('/storage/emulated/0', L)).toBe('Dahili');
  });

  it('never goes above the volume root', () => {
    expect(parentOf('/storage/emulated/0/DCIM/Camera')).toBe('/storage/emulated/0/DCIM');
    expect(parentOf('/storage/emulated/0/DCIM')).toBe('/storage/emulated/0');
    expect(parentOf('/storage/emulated/0')).toBeNull();
    expect(parentOf('/storage/emulated/0/')).toBeNull();
  });

  it('toggles and collapses nested selections', () => {
    expect(toggle(['/a'], '/b')).toEqual(['/a', '/b']);
    expect(toggle(['/a', '/b'], '/a')).toEqual(['/b']);
    expect(collapseNested(['/s/DCIM/Camera/x.jpg', '/s/DCIM', '/s/DCIMX', '/s/DCIM'])).toEqual(['/s/DCIM', '/s/DCIMX']);
  });

  it('remembers the last used folders per purpose (most recent first, max 5)', () => {
    const m = new Map<string, string>();
    const s = { getItem: (k: string) => m.get(k) ?? null, setItem: (k: string, v: string) => void m.set(k, v) };
    expect(loadRecent('download', s)).toEqual([]);
    for (const p of ['/1', '/2', '/3', '/4', '/5', '/6', '/2']) saveRecent('download', p, s);
    expect(loadRecent('download', s)).toEqual(['/2', '/6', '/5', '/4', '/3']);
    expect(loadRecent('share', s)).toEqual([]);
    m.set('peek.phonePicker.last', '{bad');
    expect(loadRecent('download', s)).toEqual([]);
  });
});
