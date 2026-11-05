import type { BrowseQuery, BrowseSkeleton, MediaItem } from '@peekaround/shared';
import { aspectToByte } from '@peekaround/shared';

function b64(bytes: Uint8Array): string {
  let s = '';
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

export function dayKey(localAt: number): string {
  return new Date(localAt).toISOString().slice(0, 10);
}

/** Skeleton for an already filtered + sorted item list (Android local mode). */
export function buildSkeleton(items: MediaItem[], q: BrowseQuery): BrowseSkeleton {
  const aspects = new Uint8Array(items.length);
  const kindCounts = { image: 0, video: 0, audio: 0 };
  let days: BrowseSkeleton['days'] = (q.sort ?? 'takenAt') === 'takenAt' ? [] : null;
  items.forEach((it, i) => {
    aspects[i] = aspectToByte(it.width && it.height ? it.width / it.height : 1);
    if (it.kind === 'image' || it.kind === 'video' || it.kind === 'audio') kindCounts[it.kind]++;
    if (days) {
      const k = dayKey(it.localAt);
      const last = days[days.length - 1];
      if (last && last.key === k) last.count++;
      else days.push({ key: k, count: 1, place: null });
    }
  });
  let h = 0;
  for (const it of items) h = (h * 31 + it.mtime + it.id.length) | 0;
  return { gen: `${items.length}:${h}`, total: items.length, kindCounts, aspects: b64(aspects), days };
}
