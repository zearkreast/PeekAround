import { useEffect, useState } from 'react';
import { AnimatePresence, motion } from 'motion/react';
import { ArrowLeft, ArrowRight, Loader2, Radar, Server, Smartphone, Wifi } from 'lucide-react';
import { useLang } from '@/i18n';
import { cn } from '@/lib/cn';
import { Button, Input } from '@/components/ui';
import { LogoMark, LogoWordmark } from '@/components/Logo';
import { saveMode } from '@/backend/android/config';
import { PeekLocal } from '@/backend/android/plugin';
import { L, fmt } from './strings';
import { probeServer } from './probe';

type Step = 'choose' | 'local' | 'server';

/** First-run screen of the Android app: phone gallery or connect to a PeekAround server. */
export function AndroidSetup() {
  const lang = useLang();
  const s = L[lang];
  const [step, setStep] = useState<Step>('choose');

  return (
    <div className="relative flex min-h-full flex-col items-center justify-center overflow-hidden bg-bg px-5 py-10 text-fg">
      <div className="aurora opacity-80" />
      <motion.div initial={{ opacity: 0, y: 20 }} animate={{ opacity: 1, y: 0 }} transition={{ type: 'spring', stiffness: 200, damping: 24 }} className="relative flex flex-col items-center text-center">
        <LogoMark size={72} />
        <LogoWordmark className="mt-4 text-3xl" />
        <p className="mt-2 max-w-xs text-sm text-dim">{s.welcome}</p>
      </motion.div>
      <div className="relative mt-10 w-full max-w-md">
        <AnimatePresence mode="wait">
          {step === 'choose' && (
            <motion.div key="choose" initial={{ opacity: 0, x: 30 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -30 }} className="flex flex-col gap-3">
              <Choice icon={<Smartphone />} title={s.local} hint={s.localHint} onClick={() => setStep('local')} primary />
              <Choice icon={<Server />} title={s.server} hint={s.serverHint} onClick={() => setStep('server')} />
            </motion.div>
          )}
          {step === 'local' && <LocalStep key="local" onBack={() => setStep('choose')} />}
          {step === 'server' && <ServerStep key="server" onBack={() => setStep('choose')} />}
        </AnimatePresence>
      </div>
    </div>
  );
}

function Choice({ icon, title, hint, onClick, primary }: { icon: React.ReactNode; title: string; hint: string; onClick: () => void; primary?: boolean }) {
  return (
    <motion.button
      type="button"
      whileTap={{ scale: 0.98 }}
      onClick={onClick}
      className={cn(
        'flex items-center gap-4 rounded-3xl border p-5 text-left backdrop-blur',
        primary ? 'border-[color-mix(in_oklab,var(--accent)_45%,transparent)] bg-[color-mix(in_oklab,var(--accent)_12%,transparent)]' : 'border-line bg-surface',
      )}
    >
      <span className={cn('grid size-12 shrink-0 place-items-center rounded-2xl [&>svg]:size-6', primary ? 'bg-accent-gradient text-white' : 'bg-surface-3')}>{icon}</span>
      <span className="min-w-0 flex-1">
        <span className="block font-display text-lg font-semibold">{title}</span>
        <span className="block text-[13px] text-dim">{hint}</span>
      </span>
      <ArrowRight className="size-5 text-faint" />
    </motion.button>
  );
}

function LocalStep({ onBack }: { onBack: () => void }) {
  const lang = useLang();
  const s = L[lang];
  const [busy, setBusy] = useState(false);
  const [waiting, setWaiting] = useState(false);

  const finish = async () => {
    const { AndroidLocalBackend } = await import('@/backend/android/local');
    const b = new AndroidLocalBackend();
    await b.setupDefaults();
    saveMode({ mode: 'local' });
    location.reload();
  };

  // the "All files access" screen is a separate Android settings page; poll until granted
  useEffect(() => {
    if (!waiting) return;
    const t = setInterval(async () => {
      const r = await PeekLocal.checkAccess();
      if (r.granted) {
        clearInterval(t);
        await finish();
      }
    }, 1000);
    return () => clearInterval(t);
  }, [waiting]);

  return (
    <motion.div initial={{ opacity: 0, x: 30 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -30 }} className="rounded-3xl border border-line bg-elev/80 p-6 backdrop-blur">
      <button type="button" onClick={onBack} className="mb-4 inline-flex items-center gap-1.5 text-sm text-dim">
        <ArrowLeft className="size-4" /> {s.back}
      </button>
      <h2 className="font-display text-xl font-semibold">{s.permTitle}</h2>
      <p className="mt-2 text-sm text-dim">{s.permBody}</p>
      <Button
        variant="primary"
        size="lg"
        className="mt-6 w-full"
        loading={busy}
        onClick={async () => {
          setBusy(true);
          const r = await PeekLocal.requestAccess();
          if (r.granted) await finish();
          else setWaiting(true);
        }}
      >
        {waiting ? s.waiting : s.grant}
      </Button>
    </motion.div>
  );
}

function ServerStep({ onBack }: { onBack: () => void }) {
  const lang = useLang();
  const s = L[lang];
  const [servers, setServers] = useState<{ name: string; url: string }[]>([]);
  const [scanning, setScanning] = useState(true);
  const [url, setUrl] = useState('http://');
  const [error, setError] = useState('');
  const [connecting, setConnecting] = useState<string | null>(null);

  const scan = async () => {
    setScanning(true);
    try {
      const r = await PeekLocal.discover({ timeout: 4000 });
      setServers(r.servers);
    } catch {
      setServers([]);
    } finally {
      setScanning(false);
    }
  };
  useEffect(() => {
    void scan();
  }, []);

  const connect = async (u: string, name?: string) => {
    setError('');
    setConnecting(u);
    const r = await probeServer(u);
    if (r.ok) {
      saveMode({ mode: 'server', url: r.url, name: name ?? r.info.hostname });
      location.reload();
      return;
    }
    setError(r.error === 'invalid' ? s.connectInvalid : r.error === 'not-peekaround' ? s.connectNotPeek : r.error === 'refused' ? fmt(s.connectRefused, { status: r.status ?? '' }) : s.connectFail);
    setConnecting(null);
  };

  return (
    <motion.div initial={{ opacity: 0, x: 30 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -30 }} className="rounded-3xl border border-line bg-elev/80 p-6 backdrop-blur">
      <button type="button" onClick={onBack} className="mb-4 inline-flex items-center gap-1.5 text-sm text-dim">
        <ArrowLeft className="size-4" /> {s.back}
      </button>
      <div className="flex items-center justify-between">
        <h2 className="font-display text-xl font-semibold">{s.found}</h2>
        <button type="button" onClick={scan} className="grid size-9 place-items-center rounded-xl text-dim hover:bg-surface-2" aria-label="Scan">
          {scanning ? <Loader2 className="size-4 animate-spin" /> : <Radar className="size-4" />}
        </button>
      </div>
      <div className="mt-3 flex flex-col gap-2">
        {servers.map((sv) => (
          <button key={sv.url} type="button" onClick={() => connect(sv.url, sv.name)} className="flex items-center gap-3 rounded-2xl border border-line bg-surface p-3 text-left">
            <Wifi className="size-5 text-accent" />
            <span className="min-w-0 flex-1">
              <span className="block truncate text-sm font-medium">{sv.name}</span>
              <span className="block font-mono text-xs text-faint">{sv.url}</span>
            </span>
            {connecting === sv.url ? <Loader2 className="size-4 animate-spin" /> : <ArrowRight className="size-4 text-faint" />}
          </button>
        ))}
        {!scanning && !servers.length && <p className="rounded-2xl bg-surface p-3 text-sm text-dim">{s.noneFound}</p>}
      </div>
      <form
        className="mt-5 flex flex-col gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          void connect(url);
        }}
      >
        <label className="text-[13px] text-dim">{s.manual}</label>
        <div className="flex gap-2">
          <Input value={url} onChange={(e) => setUrl(e.target.value)} inputMode="url" placeholder="http://192.168.1.10:1313" className="flex-1" />
          <Button type="submit" variant="primary" loading={!!connecting && connecting === url}>
            {s.connect}
          </Button>
        </div>
        {error && <p className="text-sm text-danger">{error}</p>}
      </form>
    </motion.div>
  );
}
