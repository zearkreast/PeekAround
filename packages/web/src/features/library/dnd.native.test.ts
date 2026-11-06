import { describe, expect, it, vi } from 'vitest';
import type { MediaItem } from '@peekaround/shared';

vi.mock('@capacitor/core', () => ({ registerPlugin: () => ({}) }));

// Electron window: the preload bridge exists before the modules load
const listeners = new Map<string, (e: unknown) => void>();
const startDrag = vi.fn();
vi.stubGlobal('window', {
  peekDesktop: { platform: 'win32', startDrag },
  addEventListener: (k: string, fn: (e: unknown) => void) => listeners.set(k, fn),
  removeEventListener: (k: string) => listeners.delete(k),
});

const { HttpBackend } = await import('@/backend/http');
const { MEDIA_MIME, isMediaDrag, isOwnNativeDrag, readMediaDrop, startMediaDrag, endMediaDrag } = await import('./dnd');

const item = (id: string): MediaItem => ({ id, name: `${id}.jpg`, ext: 'jpg', kind: 'image' }) as MediaItem;

describe('desktop native drag of several files', () => {
  const backend = new HttpBackend('http://127.0.0.1:47213');

  it('hands the ids to the shell, keeps album drops working, hides it from the upload zone', async () => {
    const prevent = vi.fn();
    const dt = { setData: vi.fn(), types: [] };
    expect(startMediaDrag({ dataTransfer: dt, preventDefault: prevent } as unknown as DragEvent, [item('1'), item('2')], backend)).toBe(true);
    expect(prevent).toHaveBeenCalled();
    expect(startDrag).toHaveBeenCalledWith(['1', '2']);
    expect(dt.setData).not.toHaveBeenCalled();
    expect(isOwnNativeDrag()).toBe(true);
    // the drag comes back into the window as 'Files' (no MEDIA_MIME): still ours
    const files = { dataTransfer: { types: ['Files'], getData: () => '' } } as unknown as DragEvent;
    expect(isMediaDrag(files)).toBe(true);
    expect(readMediaDrop(files)).toEqual(['1', '2']);
    listeners.get('drop')?.({});
    await new Promise((r) => setTimeout(r, 0));
    expect(isOwnNativeDrag()).toBe(false);
    expect(isMediaDrag(files)).toBe(false);
  });

  it('a single file keeps the HTML drag (DownloadURL)', () => {
    startDrag.mockClear();
    const data = new Map<string, string>();
    const dt = { setData: (k: string, v: string) => data.set(k, v), setDragImage() {}, types: [] };
    startMediaDrag({ dataTransfer: dt, preventDefault: vi.fn() } as unknown as DragEvent, [item('5')], backend);
    expect(startDrag).not.toHaveBeenCalled();
    expect(data.get(MEDIA_MIME)).toBe('["5"]');
    endMediaDrag();
  });
});
