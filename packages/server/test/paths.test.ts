import { describe, expect, it } from 'vitest';
import os from 'node:os';
import path from 'node:path';
import fs from 'node:fs';
import { normalizeRel, resolveInRoot, sanitizeName, uniqueEditPath, uniquePath } from '../src/paths';

describe('paths', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-paths-'));

  it('normalizes and rejects traversal', () => {
    expect(normalizeRel('/a//b/./c/')).toBe('a/b/c');
    expect(normalizeRel('a\\b')).toBe('a/b');
    expect(() => normalizeRel('../etc')).toThrow();
    expect(() => normalizeRel('a/../../b')).toThrow();
  });

  it('keeps resolved paths inside the root, also through symlinks', () => {
    expect(resolveInRoot(root, 'x/y.jpg')).toBe(path.join(root, 'x', 'y.jpg'));
    fs.symlinkSync(os.tmpdir(), path.join(root, 'escape'));
    expect(() => resolveInRoot(root, 'escape/whatever')).toThrow();
  });

  it('sanitizes names for every OS', () => {
    expect(sanitizeName('a/b:c*?.jpg')).toBe('a_b_c__.jpg');
    expect(sanitizeName('CON.txt')).toBe('_CON.txt');
    expect(sanitizeName('..')).toBe('untitled');
    expect(sanitizeName('İstanbul Boğazı.png')).toBe('İstanbul Boğazı.png');
  });

  it('finds free names', async () => {
    fs.writeFileSync(path.join(root, 'a.jpg'), '');
    expect(path.basename(await uniquePath(root, 'a.jpg'))).toBe('a (1).jpg');
    fs.writeFileSync(path.join(root, 'a_edit.jpg'), '');
    expect(path.basename(await uniqueEditPath(root, 'a', '_edit', 'jpg'))).toBe('a_edit-2.jpg');
  });
});
