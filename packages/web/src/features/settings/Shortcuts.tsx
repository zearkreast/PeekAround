import { useEffect } from 'react';
import { create } from 'zustand';
import { Keyboard } from 'lucide-react';
import { desktop } from '@/backend/desktop';
import { useT, type MessageKey } from '@/i18n';
import { Button, Kbd, Modal } from '@/components/ui';
import { Card, Row, Searchable, Section } from './parts';

/** The shortcuts the app really handles (grep: keydown handlers in gallery, viewer, editors, slideshow). */
type Group = { title: MessageKey; items: [MessageKey, string[]][]; desktopOnly?: boolean };

const mod = typeof navigator !== 'undefined' && /Mac|iPhone|iPad/.test(navigator.platform) ? '⌘' : 'Ctrl';

export const SHORTCUTS: Group[] = [
  {
    title: 'pref.kb.general',
    items: [
      ['pref.kb.search', ['/', `${mod} K`]],
      ['pref.kb.help', ['?']],
    ],
  },
  {
    title: 'pref.kb.grid',
    items: [
      ['pref.kb.move', ['← ↑ → ↓']],
      ['pref.kb.open', ['Enter']],
      ['pref.kb.select', ['Space']],
      ['pref.kb.selectAll', [`${mod} A`]],
      ['pref.kb.clearSel', ['Esc']],
      ['pref.kb.delete', ['Delete']],
      ['pref.kb.density', [`${mod} + 🖱`]],
    ],
  },
  {
    title: 'pref.kb.viewer',
    items: [
      ['pref.kb.prevNext', ['←', '→']],
      ['pref.kb.close', ['Esc']],
      ['pref.kb.info', ['I']],
      ['pref.kb.fullscreen', ['F']],
      ['pref.kb.zoom', ['+', '−']],
      ['pref.kb.zoomReset', ['0']],
    ],
  },
  {
    title: 'pref.kb.video',
    items: [
      ['pref.kb.playPause', ['Space', 'K']],
      ['pref.kb.seek', ['J', 'L']],
      ['pref.kb.mute', ['M']],
      ['pref.kb.fullscreen', ['F']],
    ],
  },
  {
    title: 'pref.kb.slideshow',
    items: [
      ['pref.kb.playPause', ['Space', 'K']],
      ['pref.kb.prevNext', ['←', '→']],
      ['pref.kb.fullscreen', ['F']],
      ['pref.kb.captions', ['C']],
      ['pref.kb.mute', ['M']],
      ['pref.kb.exit', ['Esc']],
    ],
  },
  {
    title: 'pref.kb.photoEditor',
    items: [
      ['pref.kb.save', [`${mod} S`]],
      ['pref.kb.compare', ['\\']],
      ['pref.kb.fit', ['0', '1']],
      ['pref.kb.deleteLayer', ['Delete']],
      ['pref.kb.deselect', ['Esc']],
    ],
  },
  {
    title: 'pref.kb.videoEditor',
    items: [
      ['pref.kb.playPause', ['Space', 'K']],
      ['pref.kb.trim', ['I', 'O']],
      ['pref.kb.frame', ['←', '→']],
      ['pref.kb.undo', [`${mod} Z`, `${mod} Y`]],
      ['pref.kb.deleteLayer', ['Delete']],
    ],
  },
  {
    title: 'pref.kb.desktop',
    desktopOnly: true,
    items: [
      ['pref.kb.fullscreen', ['F11']],
      ['pref.kb.winZoom', [`${mod} +`, `${mod} −`, `${mod} 0`]],
    ],
  },
];

const groups = () => SHORTCUTS.filter((g) => !g.desktopOnly || !!desktop);

export const useShortcutsOverlay = create<{ open: boolean; set(open: boolean): void }>((set) => ({ open: false, set: (open) => set({ open }) }));

function Keys({ keys }: { keys: string[] }) {
  return (
    <span className="flex flex-wrap justify-end gap-1">
      {keys.map((k) => (
        <Kbd key={k}>{k}</Kbd>
      ))}
    </span>
  );
}

export function ShortcutsSection() {
  const t = useT();
  const open = useShortcutsOverlay((s) => s.set);
  return (
    <Section id="shortcuts" title="pref.sec.shortcuts" icon={<Keyboard />}>
      <Card title={t('pref.sec.shortcuts')} titleKey="pref.sec.shortcuts" icon={<Keyboard />}>
        <Row label={t('pref.shortcutsOpen')} k="pref.shortcutsOpen" hint={t('pref.shortcutsHint')} kw="keyboard klavye hotkey">
          <Button variant="secondary" size="sm" icon={<Keyboard className="size-4" />} onClick={() => open(true)} data-action="shortcuts">
            ?
          </Button>
        </Row>
        <div className="grid gap-x-8 gap-y-4 pt-4 @2xl:grid-cols-2">
          {groups().map((g) => (
            <Searchable key={g.title} keys={[g.title, ...g.items.map(([k]) => k)]} text="keyboard klavye kısayol shortcut">
              <div data-row>
                <div className="mb-1 text-[11px] font-semibold tracking-wide text-faint uppercase">{t(g.title)}</div>
                {g.items.map(([k, keys]) => (
                  <div key={k} className="flex items-center justify-between gap-3 py-1 text-[13px]">
                    <span className="text-dim">{t(k)}</span>
                    <Keys keys={keys} />
                  </div>
                ))}
              </div>
            </Searchable>
          ))}
        </div>
      </Card>
    </Section>
  );
}

/** "?" anywhere (outside text fields) opens the full list. Mounted once (SettingsHost). */
export function ShortcutsOverlay() {
  const t = useT();
  const { open, set } = useShortcutsOverlay();
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement;
      if (e.key !== '?' || e.ctrlKey || e.metaKey || e.altKey || /^(input|textarea|select)$/i.test(el.tagName) || el.isContentEditable) return;
      e.preventDefault();
      set(!useShortcutsOverlay.getState().open);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [set]);
  return (
    <Modal open={open} onClose={() => set(false)} title={t('pref.sec.shortcuts')} width={720}>
      <div className="grid gap-x-8 gap-y-5 pb-2 sm:grid-cols-2" data-testid="shortcuts-overlay">
        {groups().map((g) => (
          <div key={g.title}>
            <div className="mb-1.5 text-[11px] font-semibold tracking-wide text-faint uppercase">{t(g.title)}</div>
            <div className="divide-y divide-line">
              {g.items.map(([k, keys]) => (
                <div key={k} className="flex items-center justify-between gap-3 py-1.5 text-sm">
                  <span className="text-dim">{t(k)}</span>
                  <Keys keys={keys} />
                </div>
              ))}
            </div>
          </div>
        ))}
      </div>
    </Modal>
  );
}
