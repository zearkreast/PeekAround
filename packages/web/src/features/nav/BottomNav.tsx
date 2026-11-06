import { useEffect, useRef, type RefObject } from 'react';
import { NavLink, useLocation } from 'react-router';
import { Compass, Images, Library, Settings } from 'lucide-react';
import { useT, type MessageKey } from '@/i18n';
import { cn } from '@/lib/cn';
import { useSelecting } from '@/store/selection';

const TABS: { to: string; icon: typeof Images; label: MessageKey; match: (p: string) => boolean }[] = [
  { to: '/', icon: Images, label: 'mobile.nav.photos', match: (p) => p === '/' || p.startsWith('/timeline') || p.startsWith('/favorites') || p.startsWith('/search') },
  { to: '/explore', icon: Compass, label: 'mobile.nav.explore', match: (p) => p.startsWith('/explore') },
  { to: '/library', icon: Library, label: 'mobile.nav.library', match: (p) => p === '/library' || p.startsWith('/library/') || p.startsWith('/folder/') || p.startsWith('/album') || p.startsWith('/collage') || p.startsWith('/slideshow') || p.startsWith('/fonts') },
  { to: '/settings', icon: Settings, label: 'mobile.nav.settings', match: (p) => p.startsWith('/settings') },
];

/**
 * Phone bottom navigation (Photos / Explore / Library / Settings). Slides away while
 * scrolling down and back on scroll up — direct DOM writes, no re-render per scroll.
 */
export function BottomNav({ scrollRef }: { scrollRef: RefObject<HTMLElement | null> }) {
  const t = useT();
  const loc = useLocation();
  const selecting = useSelecting();
  const ref = useRef<HTMLElement>(null);

  useEffect(() => {
    const sc = scrollRef.current;
    const el = ref.current;
    if (!sc || !el) return;
    let last = sc.scrollTop;
    let acc = 0;
    const on = () => {
      const top = sc.scrollTop;
      const d = top - last;
      last = top;
      if (top < 40) {
        delete el.dataset.hidden;
        acc = 0;
        return;
      }
      acc = Math.sign(d) === Math.sign(acc) ? acc + d : d;
      if (acc > 24) el.dataset.hidden = '1';
      else if (acc < -16) delete el.dataset.hidden;
    };
    sc.addEventListener('scroll', on, { passive: true });
    return () => sc.removeEventListener('scroll', on);
  }, [scrollRef]);

  useEffect(() => {
    if (ref.current) delete ref.current.dataset.hidden;
  }, [loc.pathname]);

  return (
    <nav ref={ref} aria-label={t('mobile.nav.label')} className={cn('bottom-nav', selecting && 'bottom-nav-off')}>
      {TABS.map((tab) => {
        const active = tab.match(loc.pathname);
        const Icon = tab.icon;
        return (
          <NavLink key={tab.to} to={tab.to} className="bottom-tab" data-active={active ? '' : undefined} aria-current={active ? 'page' : undefined}>
            <span className="bottom-tab-pill">
              <Icon className="size-[22px]" strokeWidth={active ? 2.3 : 1.8} />
            </span>
            <span className="text-[11.5px] leading-none font-medium">{t(tab.label)}</span>
          </NavLink>
        );
      })}
    </nav>
  );
}
