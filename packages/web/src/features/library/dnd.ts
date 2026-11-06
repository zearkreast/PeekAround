/**
 * Dragging media tiles (desktop, mouse): onto an album in the sidebar, or out of the window.
 *  - our own drop targets read MEDIA_MIME (JSON list of ids) — no 'Files' type, so the upload
 *    drop zone ignores these drags
 *  - Chromium (browser + Electron) also gets `DownloadURL` → dropping one tile on the desktop /
 *    a file manager saves the original file; other apps get the file URL as text/uri-list
 */
import type { MediaItem } from '@peekaround/shared';
import type { Backend } from '@/backend';
import { desktop } from '@/backend/desktop';

export const MEDIA_MIME = 'application/x-peekaround-media';

/** ids being dragged (dragover cannot read the data, only the types) */
let current: string[] = [];
/** desktop: a multi-file drag runs as a native OS drag (real files for Explorer / Finder) */
let native = false;

/** our own native drag is in progress (the upload drop zone must ignore its 'Files') */
export function isOwnNativeDrag(): boolean {
  return native && current.length > 0;
}

function armNativeEnd() {
  const end = () => {
    window.removeEventListener('drop', end);
    window.removeEventListener('dragend', end);
    window.removeEventListener('mousemove', move);
    // after the drop handlers of our own targets (they bubble before window)
    setTimeout(endMediaDrag, 0);
  };
  const move = (e: MouseEvent) => e.buttons === 0 && end();
  window.addEventListener('drop', end);
  window.addEventListener('dragend', end);
  window.addEventListener('mousemove', move);
}

export function draggedIds(): string[] {
  return current;
}

export function isMediaDrag(e: { dataTransfer: DataTransfer | null }): boolean {
  if (!e.dataTransfer) return false;
  const types = Array.from(e.dataTransfer.types);
  return types.includes(MEDIA_MIME) || (isOwnNativeDrag() && types.includes('Files'));
}

const MIME: Record<string, string> = {
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  png: 'image/png',
  gif: 'image/gif',
  webp: 'image/webp',
  heic: 'image/heic',
  mp4: 'video/mp4',
  mov: 'video/quicktime',
  webm: 'video/webm',
  mkv: 'video/x-matroska',
};

function absolute(url: string): string {
  try {
    return new URL(url, location.href).href;
  } catch {
    return url;
  }
}

/** fill a dragstart event for these items; returns false when nothing can be dragged */
export function startMediaDrag(e: DragEvent | React.DragEvent, items: MediaItem[], backend: Backend, ghost?: HTMLElement | null): boolean {
  const dt = e.dataTransfer;
  if (!dt || !items.length) return false;
  current = items.map((i) => i.id);
  // Electron: several files → native drag with the real files (the HTML drag can carry only one DownloadURL)
  if (items.length > 1 && desktop?.startDrag && backend.kind === 'http') {
    e.preventDefault();
    native = true;
    desktop.startDrag(current);
    armNativeEnd();
    return true;
  }
  dt.effectAllowed = 'copyMove';
  dt.setData(MEDIA_MIME, JSON.stringify(current));
  const urls = items.map((i) => absolute(backend.fileUrl(i.id, true)));
  dt.setData('text/uri-list', urls.join('\r\n'));
  dt.setData('text/plain', items.length === 1 ? items[0].name : items.map((i) => i.name).join('\n'));
  // Chromium: drag a single file out to the OS (saved as the original)
  if (items.length === 1) {
    const it = items[0];
    dt.setData('DownloadURL', `${MIME[it.ext] ?? 'application/octet-stream'}:${it.name.replace(/:/g, '_')}:${urls[0]}`);
  }
  if (ghost) dt.setDragImage(ghost, 24, 24);
  return true;
}

export function endMediaDrag() {
  current = [];
  native = false;
}

export function readMediaDrop(e: DragEvent | React.DragEvent): string[] {
  try {
    const raw = e.dataTransfer?.getData(MEDIA_MIME);
    const ids = raw ? (JSON.parse(raw) as unknown) : null;
    return Array.isArray(ids) ? ids.map(String) : current;
  } catch {
    return current;
  }
}

/** small stacked-thumbnail drag image (count badge) — kept in the DOM only during the drag */
export function dragGhost(thumb: string, count: number): HTMLElement {
  const el = document.createElement('div');
  el.style.cssText =
    'position:fixed;top:-200px;left:-200px;width:64px;height:64px;border-radius:14px;overflow:visible;pointer-events:none;z-index:9999';
  const img = document.createElement('div');
  img.style.cssText = `width:64px;height:64px;border-radius:14px;background:#222 center/cover url("${thumb}");box-shadow:0 8px 24px rgba(0,0,0,.45);border:2px solid #fff`;
  el.appendChild(img);
  if (count > 1) {
    const b = document.createElement('div');
    b.textContent = String(count);
    b.style.cssText =
      'position:absolute;top:-8px;right:-8px;min-width:22px;height:22px;padding:0 6px;border-radius:11px;background:var(--accent,#8b5cf6);color:#fff;font:600 12px/22px Inter,system-ui,sans-serif;text-align:center';
    el.appendChild(b);
  }
  document.body.appendChild(el);
  setTimeout(() => el.remove(), 0);
  return el;
}
