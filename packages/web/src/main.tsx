import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { QueryClientProvider } from '@tanstack/react-query';
import { MotionConfig, MotionGlobalConfig } from 'motion/react';
import '@fontsource-variable/inter';
import '@fontsource-variable/bricolage-grotesque';
import './styles.css';
import { App } from './App';
import { BackendProvider } from './backend';
import { createInitialBackend } from './backend/select';
import { queryClient } from './lib/queryClient';
import { usePrefs } from './store/prefs';

function Root() {
  const motion = usePrefs((s) => s.motion);
  // Settings → animations "off": every motion animation (page fades, springs, exits) completes instantly
  MotionGlobalConfig.skipAnimations = motion === 'off';
  return (
    <MotionConfig reducedMotion={motion === 'full' ? 'user' : 'always'}>
      <App />
    </MotionConfig>
  );
}

const backend = await createInitialBackend();
const root = createRoot(document.getElementById('root')!);

if (backend) {
  root.render(
    <StrictMode>
      <QueryClientProvider client={queryClient}>
        <BackendProvider initial={backend}>
          <Root />
        </BackendProvider>
      </QueryClientProvider>
    </StrictMode>,
  );
} else {
  // Android first run: choose phone storage or a server
  const { AndroidSetup } = await import('./features/android/Setup');
  document.documentElement.dataset.theme = 'dark';
  root.render(
    <StrictMode>
      <AndroidSetup />
    </StrictMode>,
  );
}
