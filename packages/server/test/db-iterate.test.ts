import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { afterAll, describe, expect, it } from 'vitest';
import { Db } from '../src/db.js';

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'peek-iter-'));
const db = new Db(path.join(tmp, 'x.db'));
db.run('CREATE TABLE t (n INTEGER)');
for (let i = 0; i < 500; i++) db.run('INSERT INTO t (n) VALUES (?)', i);

afterAll(() => {
  db.close();
  fs.rmSync(tmp, { recursive: true, force: true });
});

describe('Db.iterateArrays', () => {
  it('two interleaved iterations of the same SQL do not reset each other', () => {
    const sql = 'SELECT n FROM t WHERE n >= ? ORDER BY n';
    const a = db.iterateArrays(sql, 0);
    const b = db.iterateArrays(sql, 400);
    const ra: number[] = [];
    const rb: number[] = [];
    // like two explore computations that yield every few thousand rows
    for (;;) {
      const x = a.next();
      const y = b.next();
      if (!x.done) ra.push((x.value as number[])[0]);
      if (!y.done) rb.push((y.value as number[])[0]);
      if (x.done && y.done) break;
    }
    expect(ra).toEqual(Array.from({ length: 500 }, (_, i) => i));
    expect(rb).toEqual(Array.from({ length: 100 }, (_, i) => 400 + i));
    // the cached statement is free again afterwards
    expect([...db.iterateArrays(sql, 498)].map((r) => r[0])).toEqual([498, 499]);
  });
});
