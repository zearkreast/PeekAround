import { Suspense, useEffect, useRef, useState } from 'react';
import { useLocation as useRouterLocation, useNavigate, useOutlet } from 'react-router';
import { AnimatePresence, motion } from 'motion/react';
import { setNavigate } from '@/lib/nav';
import { notifyHistoryChange } from '@/lib/backStack';
import { useIsMobile } from '@/lib/useMedia';
import { useThemeEffect } from '@/lib/useTheme';
import { Spinner } from '@/components/ui';
import { Toaster } from '@/components/Toaster';
import { Splash } from '@/components/Logo';
import { Sidebar } from '@/features/gallery/Sidebar';
import { TopBar } from '@/features/gallery/TopBar';
import { ScrollContext } from '@/features/gallery/GalleryPage';
import { ActionsHost } from '@/features/gallery/actions';
import { BottomNav } from '@/features/nav/BottomNav';
import { NavRail } from '@/features/nav/NavRail';
import { usePrefs } from '@/store/prefs';
import { DropZone } from '@/features/upload/DropZone';
import { UploadPanel } from '@/features/upload/UploadPanel';
import { Viewer } from '@/features/viewer/Viewer';
import { ShareHandler } from '@/features/android/ShareHandler';
import { DevicesHost } from '@/features/devices/DevicesHost';
import { RemoteHost } from '@/features/remote/RemoteHost';
import { LibraryHost } from '@/features/library/LibraryHost';
import { SettingsHost } from '@/features/settings/SettingsHost';
import { initAndroidBack } from '@/backend/android/back';

/** Full-screen tools (editors, slideshow player) render without the gallery chrome. */
const IMMERSIVE = /^\/(edit\/|slideshow\/play|explore\/memory\/[^/]+\/play)/;

/**
 * The route element as it was when this page mounted: the page that animates out keeps showing
 * itself (a plain <Outlet /> would render the NEW route inside the exiting page → a duplicate
 * page flashing during the transition).
 */
function FrozenOutlet() {
  const outlet = useOutlet();
  const [frozen] = useState(outlet);
  return frozen;
}

export function Layout() {
  useThemeEffect();
  const loc = useRouterLocation();
  const nav = useNavigate();
  const mobile = useIsMobile();
  const wideSidebar = usePrefs((s) => s.wideSidebar);
  const scrollRef = useRef<HTMLDivElement>(null);
  useEffect(() => setNavigate((to) => nav(to)), [nav]);

  // Android hardware / predictive back ↔ JS back stack
  useEffect(() => initAndroidBack(), []);
  useEffect(() => notifyHistoryChange(), [loc.key]);

  useEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 });
  }, [loc.pathname]);

  const immersive = IMMERSIVE.test(loc.pathname);

  return (
    <ScrollContext.Provider value={scrollRef}>
      <div className="flex h-full w-full overflow-hidden bg-bg text-fg">
        {/* desktop: the slim navigation rail (or the full sidebar, Settings → Appearance); phones: the drawer */}
        {!immersive && (mobile || wideSidebar ? <Sidebar /> : <NavRail />)}
        <div className="relative flex min-w-0 flex-1 flex-col">
          {!immersive && <TopBar />}
          <div ref={scrollRef} id="scroller" className="relative min-h-0 flex-1 overflow-x-hidden overflow-y-auto [scrollbar-gutter:stable]">
            <AnimatePresence mode="popLayout" initial={false}>
              <motion.div
                key={loc.pathname}
                initial={mobile ? { opacity: 0 } : { opacity: 0, y: 10 }}
                animate={mobile ? { opacity: 1 } : { opacity: 1, y: 0 }}
                exit={{ opacity: 0, transition: { duration: mobile ? 0.08 : 0.12 } }}
                transition={mobile ? { duration: 0.16, ease: 'easeOut' } : { type: 'spring', stiffness: 360, damping: 34 }}
                className="min-h-full"
              >
                <Suspense
                  fallback={
                    <div className="grid h-[60vh] place-items-center">
                      <Spinner className="size-7" />
                    </div>
                  }
                >
                  <FrozenOutlet />
                </Suspense>
              </motion.div>
            </AnimatePresence>
          </div>
          {!immersive && <DropZone />}
        </div>
      </div>
      {!immersive && mobile && <BottomNav scrollRef={scrollRef} />}
      <Viewer />
      <UploadPanel />
      <ActionsHost />
      <Toaster />
      <ShareHandler />
      <DevicesHost />
      <RemoteHost />
      <LibraryHost />
      <SettingsHost />
      <Splash />
    </ScrollContext.Provider>
  );
}
