import { Suspense, useEffect, useRef } from 'react';
import { Outlet, useLocation as useRouterLocation, useNavigate } from 'react-router';
import { AnimatePresence, motion } from 'motion/react';
import { setNavigate } from '@/lib/nav';
import { useThemeEffect } from '@/lib/useTheme';
import { Spinner } from '@/components/ui';
import { Toaster } from '@/components/Toaster';
import { Splash } from '@/components/Logo';
import { Sidebar } from '@/features/gallery/Sidebar';
import { TopBar } from '@/features/gallery/TopBar';
import { ScrollContext } from '@/features/gallery/GalleryPage';
import { ActionsHost } from '@/features/gallery/actions';
import { DropZone } from '@/features/upload/DropZone';
import { UploadPanel } from '@/features/upload/UploadPanel';
import { Viewer } from '@/features/viewer/Viewer';
import { ShareHandler } from '@/features/android/ShareHandler';

/** Full-screen tools (editors, slideshow player) render without the gallery chrome. */
const IMMERSIVE = /^\/(edit\/|slideshow\/play)/;

export function Layout() {
  useThemeEffect();
  const loc = useRouterLocation();
  const nav = useNavigate();
  const scrollRef = useRef<HTMLDivElement>(null);
  useEffect(() => setNavigate((to) => nav(to)), [nav]);

  useEffect(() => {
    scrollRef.current?.scrollTo({ top: 0 });
  }, [loc.pathname]);

  const immersive = IMMERSIVE.test(loc.pathname);

  return (
    <ScrollContext.Provider value={scrollRef}>
      <div className="flex h-full w-full overflow-hidden bg-bg text-fg">
        {!immersive && <Sidebar />}
        <div className="relative flex min-w-0 flex-1 flex-col">
          {!immersive && <TopBar />}
          <div ref={scrollRef} id="scroller" className="relative min-h-0 flex-1 overflow-x-hidden overflow-y-auto [scrollbar-gutter:stable]">
            <AnimatePresence mode="popLayout" initial={false}>
              <motion.div
                key={loc.pathname}
                initial={{ opacity: 0, y: 10 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, transition: { duration: 0.12 } }}
                transition={{ type: 'spring', stiffness: 360, damping: 34 }}
                className="min-h-full"
              >
                <Suspense
                  fallback={
                    <div className="grid h-[60vh] place-items-center">
                      <Spinner className="size-7" />
                    </div>
                  }
                >
                  <Outlet />
                </Suspense>
              </motion.div>
            </AnimatePresence>
          </div>
          {!immersive && <DropZone />}
        </div>
      </div>
      <Viewer />
      <UploadPanel />
      <ActionsHost />
      <Toaster />
      <ShareHandler />
      <Splash />
    </ScrollContext.Provider>
  );
}
