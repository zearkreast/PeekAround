import { describe, expect, it } from 'vitest';
import type { RemoteInspect } from '@peekaround/shared';
import { buildTree, checkState, defaultSelection, findMegaUrl, importNodes, looksLikeMega, toggleNode, totals } from './mega';

describe('MEGA link detection', () => {
  it('finds new-style file and folder links in any text', () => {
    expect(findMegaUrl('https://mega.nz/file/AbCdEf12#k3yK3yK3yK3yK3yK3yK3y')).toBe('https://mega.nz/file/AbCdEf12#k3yK3yK3yK3yK3yK3yK3y');
    expect(findMegaUrl('Bak: https://mega.nz/folder/XyZ12345#aaaaaaaaaaaaaaaaaaaaaa/file/QwErTy12 güzel değil mi?')).toBe(
      'https://mega.nz/folder/XyZ12345#aaaaaaaaaaaaaaaaaaaaaa/file/QwErTy12',
    );
    expect(findMegaUrl('mega.nz/folder/XyZ12345#aaaaaaaaaaaaaaaa')).toBe('https://mega.nz/folder/XyZ12345#aaaaaaaaaaaaaaaa');
    expect(findMegaUrl('http://www.mega.co.nz/file/AbCdEf12#keykeykeykey')).toBe('https://www.mega.co.nz/file/AbCdEf12#keykeykeykey');
  });

  it('finds legacy links', () => {
    expect(findMegaUrl('https://mega.nz/#!AbCdEf12!keykeykeykeykey')).toBe('https://mega.nz/#!AbCdEf12!keykeykeykeykey');
    expect(findMegaUrl('x https://mega.co.nz/#F!AbCdEf12!keykeykeykeykey!SubNod12 y')).toBe('https://mega.co.nz/#F!AbCdEf12!keykeykeykeykey!SubNod12');
  });

  it('ignores other hosts and links without a key', () => {
    expect(findMegaUrl('https://example.com/file/AbCdEf12#keykeykeykey')).toBeNull();
    expect(findMegaUrl('https://notmega.nz.evil.com/file/AbCdEf12#keykeykeykey')).toBeNull();
    expect(findMegaUrl('https://mega.nz/file/AbCdEf12')).toBeNull();
    expect(findMegaUrl('')).toBeNull();
    expect(findMegaUrl(null)).toBeNull();
    expect(looksLikeMega('https://mega.nz/file/AbCdEf12')).toBe(true);
  });
});

const INSPECT: RemoteInspect = {
  provider: 'mega',
  kind: 'folder',
  name: 'Tatil',
  size: 1000,
  truncated: false,
  nodes: [
    { id: 'r', path: '', name: 'Tatil', size: 1000, dir: true, kind: null },
    { id: 'd1', path: 'Gün 1', name: 'Gün 1', size: 300, dir: true, kind: null },
    { id: 'd2', path: 'Belgeler', name: 'Belgeler', size: 50, dir: true, kind: null },
    { id: 'a', path: 'kapak.jpg', name: 'kapak.jpg', size: 100, dir: false, kind: 'image' },
    { id: 'z', path: 'eski.zip', name: 'eski.zip', size: 550, dir: false, kind: 'archive' },
    { id: 'b', path: 'Gün 1/IMG_1.jpg', name: 'IMG_1.jpg', size: 100, dir: false, kind: 'image' },
    { id: 'c', path: 'Gün 1/VID_1.mp4', name: 'VID_1.mp4', size: 200, dir: false, kind: 'video' },
    { id: 'n', path: 'Belgeler/not.txt', name: 'not.txt', size: 50, dir: false, kind: null },
  ],
};

describe('MEGA tree model', () => {
  it('builds the tree and preselects media', () => {
    const tree = buildTree(INSPECT);
    expect(tree.root.children.map((c) => c.node.name)).toEqual(['Gün 1', 'Belgeler', 'kapak.jpg', 'eski.zip']);
    expect(tree.byId.get('d1')!.files.sort()).toEqual(['b', 'c']);
    expect(tree.root.files.sort()).toEqual(['a', 'b', 'c', 'n', 'z']);
    const sel = defaultSelection(INSPECT);
    expect([...sel].sort()).toEqual(['a', 'b', 'c']);
    expect(totals(tree, sel)).toEqual({ files: 3, bytes: 400, zips: [] });
    expect(checkState(tree.byId.get('d1')!, sel)).toBe('all');
    expect(checkState(tree.root, sel)).toBe('some');
    expect(checkState(tree.byId.get('d2')!, sel)).toBe('none');
  });

  it('toggles folders and compresses the import node list', () => {
    const tree = buildTree(INSPECT);
    let sel = defaultSelection(INSPECT);
    expect(importNodes(tree, sel)?.sort()).toEqual(['a', 'd1']);
    sel = toggleNode(tree.byId.get('d1')!, sel);
    expect(checkState(tree.byId.get('d1')!, sel)).toBe('none');
    sel = toggleNode(tree.byId.get('z')!, sel);
    expect(totals(tree, sel).zips).toEqual(['eski.zip']);
    expect(importNodes(tree, sel)?.sort()).toEqual(['a', 'z']);
    sel = toggleNode(tree.root, sel); // some → all
    expect(importNodes(tree, sel)).toBeUndefined();
  });

  it('single file links', () => {
    const r: RemoteInspect = { provider: 'mega', kind: 'file', name: 'x.mp4', size: 5, truncated: false, nodes: [{ id: 'f', path: '', name: 'x.mp4', size: 5, dir: false, kind: 'video' }] };
    const tree = buildTree(r);
    expect(tree.files).toHaveLength(1);
    expect(totals(tree, defaultSelection(r))).toEqual({ files: 1, bytes: 5, zips: [] });
    expect(importNodes(tree, defaultSelection(r))).toBeUndefined();
  });
});

describe('MEGA errors', () => {
  it('maps server errors to friendly messages', async () => {
    const { megaErrorKey } = await import('./mega');
    expect(megaErrorKey(429, 'MEGA bandwidth quota exceeded, try again later (retry in about 12 min)')).toEqual({ key: 'remote.err.quota', vars: { min: 12 } });
    expect(megaErrorKey(429, 'MEGA bandwidth quota exceeded, try again later').key).toBe('remote.err.quotaNoTime');
    expect(megaErrorKey(404, 'x').key).toBe('remote.err.notFound');
    expect(megaErrorKey(400, 'Invalid MEGA link key').key).toBe('remote.err.badKey');
    expect(megaErrorKey(400, 'Not a MEGA link').key).toBe('remote.err.notMega');
    expect(megaErrorKey(500, 'boom')).toEqual({ key: 'remote.err.other', vars: { error: 'boom' } });
  });
});
