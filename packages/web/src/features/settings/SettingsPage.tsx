import { useContext, useDeferredValue, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { useLocation as useRouterLocation } from 'react-router';
import { motion } from 'motion/react';
import { Compass, Eye, Folder, Gauge, Info, Keyboard, Laptop, LayoutGrid, Network, Palette, Search, SearchX, ShieldCheck, X } from 'lucide-react';
import { desktop } from '@/backend/desktop';
import { useT, type MessageKey } from '@/i18n';
import { cn } from '@/lib/cn';
import { useChrome } from '@/store/chrome';
import { usePrefs } from '@/store/prefs';
import { Input } from '@/components/ui';
import { ScrollContext } from '@/features/gallery/GalleryPage';
import { AppearanceSection, ExploreSection, GallerySection, PrivacySection, ViewerSection } from './GeneralSections';
import { AboutSection, LibrariesSection, NetworkSection, PerformanceSection } from './SystemSections';
import { DesktopSection } from './DesktopSection';
import { ShortcutsSection } from './Shortcuts';
import { SettingsQuery } from './search';
import './settings.css';

/**
 * Settings: section nav (sticky, scroll-spy) + search on wide screens, a sticky search + section
 * chips on phones. Per-device preferences live in the prefs store (localStorage), server settings
 * behind GET/PATCH /api/settings, desktop shell options in the Electron main process.
 * Feature cards from other features mount inside their section (see SystemSections / GeneralSections).
 */
interface SectionDef {
  id: string;
  title: MessageKey;
  icon: ReactNode;
  render: () => ReactNode;
  desktopOnly?: boolean;
}

const SECTIONS: SectionDef[] = [
  { id: 'appearance', title: 'pref.sec.appearance', icon: <Palette />, render: () => <AppearanceSection /> },
  { id: 'gallery', title: 'pref.sec.gallery', icon: <LayoutGrid />, render: () => <GallerySection /> },
  { id: 'viewer', title: 'pref.sec.viewer', icon: <Eye />, render: () => <ViewerSection /> },
  { id: 'explore', title: 'pref.sec.explore', icon: <Compass />, render: () => <ExploreSection /> },
  { id: 'libraries', title: 'pref.sec.libraries', icon: <Folder />, render: () => <LibrariesSection /> },
  { id: 'performance', title: 'pref.sec.performance', icon: <Gauge />, render: () => <PerformanceSection /> },
  { id: 'network', title: 'pref.sec.network', icon: <Network />, render: () => <NetworkSection /> },
  { id: 'desktop', title: 'pref.sec.desktop', icon: <Laptop />, render: () => <DesktopSection />, desktopOnly: true },
  { id: 'privacy', title: 'pref.sec.privacy', icon: <ShieldCheck />, render: () => <PrivacySection /> },
  { id: 'shortcuts', title: 'pref.sec.shortcuts', icon: <Keyboard />, render: () => <ShortcutsSection /> },
  { id: 'about', title: 'pref.sec.about', icon: <Info />, render: () => <AboutSection /> },
];

function SearchBox({ value, onChange, className }: { value: string; onChange: (v: string) => void; className?: string }) {
  const t = useT();
  return (
    <div className={cn('relative', className)}>
      <Input
        type="text"
        enterKeyHint="search"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => e.key === 'Escape' && onChange('')}
        placeholder={t('pref.search')}
        aria-label={t('pref.search')}
        icon={<Search />}
        data-testid="settings-search"
      />
      {value && (
        <button type="button" aria-label={t('common.close')} onClick={() => onChange('')} className="absolute top-1/2 right-2 grid size-6 -translate-y-1/2 place-items-center rounded-md text-faint hover:text-fg">
          <X className="size-4" />
        </button>
      )}
    </div>
  );
}

export function SettingsPage() {
  const t = useT();
  const routerLoc = useRouterLocation();
  const setChrome = useChrome((s) => s.set);
  const motionPref = usePrefs((s) => s.motion);
  const scrollRef = useContext(ScrollContext);
  const [q, setQ] = useState('');
  const query = useDeferredValue(q.trim());
  const sections = SECTIONS.filter((s) => !s.desktopOnly || !!desktop?.getShell);
  const [visible, setVisible] = useState<string[]>(() => sections.map((s) => s.id));
  const [empty, setEmpty] = useState(false);
  const [active, setActive] = useState(sections[0].id);
  const body = useRef<HTMLDivElement>(null);
  const chips = useRef<HTMLDivElement>(null);
  const stickyBar = useRef<HTMLDivElement>(null);
  const jumping = useRef(0);
  useEffect(() => setChrome({ crumbs: [], title: t('settings.title'), viewControls: false }), [setChrome, t]);

  // which sections render anything (cards hide themselves while loading / on other platforms / when searching)
  useLayoutEffect(() => {
    const el = body.current;
    if (!el) return;
    const calc = () => {
      const ids = [...el.querySelectorAll<HTMLElement>('[data-section]')].filter((s) => getComputedStyle(s).display !== 'none').map((s) => s.dataset.section!);
      setVisible((prev) => (prev.join() === ids.join() ? prev : ids));
      setEmpty(!ids.length);
    };
    calc();
    const mo = new MutationObserver(calc);
    mo.observe(el, { childList: true, subtree: true });
    return () => mo.disconnect();
  }, [query]);

  // scroll spy: the last section whose top passed the upper quarter of the scroller
  useEffect(() => {
    const sc = scrollRef?.current;
    if (!sc) return;
    let raf = 0;
    const spy = () => {
      raf = 0;
      // a nav click is scrolling to its section: keep that one active until the scroll settles
      if (Date.now() < jumping.current) return;
      const bar = stickyBar.current;
      const base = bar && bar.offsetParent ? bar.getBoundingClientRect().bottom : sc.getBoundingClientRect().top;
      const top = base + Math.min(120, sc.clientHeight / 5);
      let cur = visible[0];
      for (const id of visible) {
        const s = document.getElementById(`settings-${id}`);
        if (s && s.getBoundingClientRect().top <= top) cur = id;
      }
      // scrolled to the very end: the last section is active even when short
      if (sc.scrollTop + sc.clientHeight >= sc.scrollHeight - 4) cur = visible[visible.length - 1];
      if (cur) setActive(cur);
    };
    const on = () => {
      if (!raf) raf = requestAnimationFrame(spy);
    };
    sc.addEventListener('scroll', on, { passive: true });
    spy();
    return () => {
      sc.removeEventListener('scroll', on);
      cancelAnimationFrame(raf);
    };
  }, [scrollRef, visible]);

  // keep the active chip in view (phones)
  useEffect(() => {
    // only the chip row scrolls (scrollIntoView would also stop the page's smooth scroll)
    const row = chips.current;
    const chip = row?.querySelector<HTMLElement>(`[data-chip="${active}"]`);
    if (row && chip && (chip.offsetLeft < row.scrollLeft || chip.offsetLeft + chip.offsetWidth > row.scrollLeft + row.clientWidth)) {
      row.scrollTo({ left: Math.max(0, chip.offsetLeft - 16) });
    }
  }, [active]);

  const jump = (id: string, smooth = motionPref === 'full') => {
    jumping.current = Date.now() + (smooth ? 900 : 100);
    document.getElementById(`settings-${id}`)?.scrollIntoView({ behavior: smooth ? 'smooth' : 'auto', block: 'start' });
    setActive(id);
  };

  // deep links: /settings#performance
  useEffect(() => {
    const id = routerLoc.hash.replace(/^#/, '');
    if (!id) return;
    // sections that load their data (desktop shell, server settings) appear a bit later: retry, then settle
    const timers = [60, 400, 1000].map((ms) => setTimeout(() => document.getElementById(`settings-${id}`)?.offsetHeight && jump(id, false), ms));
    return () => timers.forEach(clearTimeout);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [routerLoc.hash]);

  const navItems = sections.filter((s) => visible.includes(s.id));

  return (
    <div className="@container mx-auto w-full max-w-6xl px-4 pt-6 pb-24 sm:px-7 sm:pt-8" data-settings-page>
      <motion.h1 initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} className="mb-5 font-display text-3xl font-bold tracking-tight @3xl:mb-7">
        {t('settings.title')}
      </motion.h1>
      <div className="@3xl:grid @3xl:grid-cols-[224px_minmax(0,1fr)] @3xl:gap-8 @5xl:grid-cols-[230px_minmax(0,1fr)] @5xl:gap-10">
        {/* wide: sticky nav + search */}
        <aside className="hidden @3xl:block">
          <nav className="sticky top-6 flex flex-col gap-0.5" aria-label={t('pref.sections')} data-testid="settings-nav">
            <SearchBox value={q} onChange={setQ} className="mb-3" />
            {navItems.map((s) => (
              <button
                key={s.id}
                type="button"
                onClick={() => jump(s.id)}
                aria-current={active === s.id ? 'true' : undefined}
                className={cn(
                  'relative flex h-9 items-center gap-2.5 rounded-xl px-3 text-left text-[13.5px] font-medium transition-colors [&>svg]:size-4 [&>svg]:shrink-0',
                  active === s.id ? 'bg-surface-2 text-fg [&>svg]:text-accent' : 'text-dim hover:bg-surface hover:text-fg',
                )}
              >
                {active === s.id && <span className="absolute top-2 bottom-2 left-0 w-[3px] rounded-full bg-accent" />}
                {s.icon}
                <span className="truncate">{t(s.title)}</span>
              </button>
            ))}
          </nav>
        </aside>

        <div className="min-w-0">
          {/* phones / narrow windows: sticky search + section chips */}
          <div ref={stickyBar} className="sticky top-0 z-20 -mx-4 mb-4 border-b border-line bg-[color-mix(in_oklab,var(--bg)_92%,transparent)] px-4 pt-1 pb-2.5 backdrop-blur-md sm:-mx-7 sm:px-7 @3xl:hidden">
            <SearchBox value={q} onChange={setQ} />
            <div ref={chips} className="no-scrollbar relative mt-2.5 -mb-0.5 flex gap-1.5 overflow-x-auto" role="tablist" aria-label={t('pref.sections')}>
              {navItems.map((s) => (
                <button
                  key={s.id}
                  type="button"
                  role="tab"
                  data-chip={s.id}
                  aria-selected={active === s.id}
                  onClick={() => jump(s.id)}
                  className={cn(
                    'flex h-8 shrink-0 items-center gap-1.5 rounded-full border px-3 text-[12.5px] font-medium whitespace-nowrap [&>svg]:size-3.5',
                    active === s.id ? 'border-transparent bg-accent text-white' : 'border-line text-dim',
                  )}
                >
                  {s.icon}
                  {t(s.title)}
                </button>
              ))}
            </div>
          </div>

          <div ref={body} className="settings-body flex flex-col gap-10" data-searching={query ? '' : undefined}>
            <SettingsQuery.Provider value={query}>
              {sections.map((s) => (
                <div key={s.id} className="contents">
                  {s.render()}
                </div>
              ))}
            </SettingsQuery.Provider>
          </div>
          {empty && (
            <div className="flex flex-col items-center gap-3 py-20 text-center text-dim" data-testid="settings-empty">
              <SearchX className="size-10 text-faint" />
              {t('pref.searchEmpty', { q: query })}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
