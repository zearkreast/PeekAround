import { describe, expect, it } from 'vitest';
import tr from './tr';
import en from './en';

describe('translations', () => {
  it('Turkish and English have the same keys', () => {
    expect(Object.keys(en).sort()).toEqual(Object.keys(tr).sort());
  });

  it('placeholders match', () => {
    const vars = (s: string) => [...new Set(s.match(/\{[a-z]+\}/gi) ?? [])].sort();
    for (const k of Object.keys(tr) as (keyof typeof tr)[]) expect(vars(en[k]), k).toEqual(vars(tr[k]));
  });
});
