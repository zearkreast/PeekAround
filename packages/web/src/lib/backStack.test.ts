import { afterEach, describe, expect, it, vi } from 'vitest';
import { BACK, _resetBackStack, backLabels, handleBack, onBackStackChange, pushBack } from './backStack';

describe('back stack', () => {
  afterEach(() => _resetBackStack());

  it('closes the highest priority, most recent entry first', () => {
    const calls: string[] = [];
    pushBack(() => calls.push('selection'), { prio: BACK.selection, label: 'selection' });
    pushBack(() => calls.push('viewer'), { prio: BACK.viewer, label: 'viewer' });
    pushBack(() => calls.push('sheet'), { prio: BACK.sheet, label: 'sheet' });
    pushBack(() => calls.push('drawer'), { prio: BACK.drawer, label: 'drawer' });
    expect(backLabels()).toEqual(['sheet', 'viewer', 'selection', 'drawer']);
    expect(handleBack()).toBe(true);
    expect(handleBack()).toBe(true);
    expect(calls).toEqual(['sheet', 'viewer']);
    expect(backLabels()).toEqual(['selection', 'drawer']);
  });

  it('unregistering removes the entry and notifies', () => {
    const fn = vi.fn();
    const off = onBackStackChange(fn);
    const remove = pushBack(() => {}, { label: 'x' });
    remove();
    expect(backLabels()).toEqual([]);
    expect(handleBack()).toBe(false);
    expect(fn).toHaveBeenCalledTimes(2);
    off();
  });

  it('same priority → last opened closes first', () => {
    const calls: number[] = [];
    pushBack(() => calls.push(1), { prio: BACK.modal });
    pushBack(() => calls.push(2), { prio: BACK.modal });
    handleBack();
    expect(calls).toEqual([2]);
  });
});
