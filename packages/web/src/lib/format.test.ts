import { afterEach, describe, expect, it } from 'vitest';
import { usePrefs } from '@/store/prefs';
import { formatDate, formatLocal } from './format';

describe('date format settings', () => {
  afterEach(() => usePrefs.setState({ dateFormat: 'locale', hourCycle: 'auto' }));
  const at = Date.UTC(2024, 6, 5, 14, 30);

  it('localized by default', () => {
    expect(formatLocal(at, 'en', { dateStyle: 'medium' })).toBe('Jul 5, 2024');
  });

  it('ISO dates for dateStyle formats, headers stay localized', () => {
    usePrefs.setState({ dateFormat: 'iso' });
    expect(formatLocal(at, 'tr', { dateStyle: 'long', timeStyle: 'short' })).toBe('2024-07-05 14:30');
    expect(formatLocal(at, 'en', { dateStyle: 'full' })).toBe('2024-07-05');
    expect(formatLocal(at, 'en', { month: 'long', year: 'numeric' })).toBe('July 2024');
    usePrefs.setState({ hourCycle: 'h12' });
    expect(formatLocal(at, 'en', { dateStyle: 'medium', timeStyle: 'short' })).toBe('2024-07-05 2:30 PM');
  });

  it('24 / 12 hour clock override', () => {
    usePrefs.setState({ hourCycle: 'h23' });
    expect(formatLocal(at, 'en', { timeStyle: 'short' })).toBe('14:30');
    usePrefs.setState({ hourCycle: 'h12' });
    expect(formatLocal(at, 'tr', { timeStyle: 'short' })).toMatch(/2:30/);
    expect(formatDate(at, 'en', { dateStyle: 'medium' })).toMatch(/2024/);
  });
});
