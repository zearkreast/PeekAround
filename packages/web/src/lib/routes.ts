/** URL helpers so every component builds gallery links the same way. */
export function folderHref(folderId: string, dir = ''): string {
  return `/folder/${folderId}${dir ? `/${dir.split('/').map(encodeURIComponent).join('/')}` : ''}`;
}

export function libraryHref(libraryId: string): string {
  return `/library/${libraryId}`;
}

export function withView(search: string, id: string | null): string {
  const p = new URLSearchParams(search);
  if (id) p.set('view', id);
  else p.delete('view');
  const s = p.toString();
  return s ? `?${s}` : '';
}
