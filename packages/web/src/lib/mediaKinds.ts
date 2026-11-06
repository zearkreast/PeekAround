import type { MediaItem } from '@peekaround/shared';

/** photos / videos have a picture (thumbnails, covers, slideshows); audio and archives do not */
export const hasPicture = (i: MediaItem | undefined): i is MediaItem => !!i && (i.kind === 'image' || i.kind === 'video');

/** what the viewer pages through: pictures and audio (an audio slide with a player), never archives */
export const viewable = (i: MediaItem | undefined): i is MediaItem => !!i && (i.kind === 'image' || i.kind === 'video' || i.kind === 'audio');

/** newest loaded photo / video among the first items of a grid (a cover); null when none / not loaded */
export function firstPicture(src: { total: number; getAt(i: number): MediaItem | undefined } | null, scan = 24): string | null {
  if (!src) return null;
  for (let i = 0; i < Math.min(src.total, scan); i++) {
    const it = src.getAt(i);
    if (!it) return null;
    if (hasPicture(it)) return it.id;
  }
  return null;
}

/** phone viewer bottom bar: the actions in order, "info" next, delete always last — each once */
export function mobileBarOrder<A extends { id: string }>(bottom: A[], info: A): A[] {
  return [...bottom.filter((a) => a.id !== 'delete'), info, ...bottom.filter((a) => a.id === 'delete')];
}
