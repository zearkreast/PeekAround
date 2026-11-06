/**
 * Settings → gallery → "Ask before deleting": replaces the built-in delete action (same id, so
 * the selection bar, viewer, context menu and the Delete key all use it). With the setting off,
 * files that go to a trash (OS trash / .peekaround-trash) are removed without the dialog; permanent
 * deletes (temporary sessions, no trash) always ask.
 */
import { BUILTIN_ACTIONS, refOf, registerMediaAction, type MediaAction } from '@/features/gallery/actions';
import { usePrefs } from '@/store/prefs';
import { useSelection } from '@/store/selection';

const base = BUILTIN_ACTIONS.find((a) => a.id === 'delete');

if (base) {
  const action: MediaAction = {
    ...base,
    run: async (items, c) => {
      const temp = items.some((i) => c.isTemp(i.folderId));
      const trash = c.info?.caps.trash;
      if (usePrefs.getState().confirmDelete || temp || !trash || trash === 'none') return base.run(items, c);
      const refs = items.map(refOf);
      await c.backend.remove(refs);
      useSelection.getState().clear();
      await Promise.all([c.qc.invalidateQueries({ queryKey: ['browse'] }), c.qc.invalidateQueries({ queryKey: ['libraries'] })]);
      c.toast.success(c.t('sel.deleted', { n: refs.length }));
    },
  };
  registerMediaAction(action);
}
