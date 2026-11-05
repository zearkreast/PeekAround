import { useEffect, useId, useState } from 'react';
import { AnimatePresence, motion, useReducedMotion } from 'motion/react';
import { cn } from '@/lib/cn';

/**
 * PeekAround logo — a black-browed albatross looking left (artwork: /brand/logo.svg).
 * Animated: the bird leans and "peeks around", blinks now and then, and tilts its head
 * curiously on hover. All geometry is in a 256×256 box.
 */
const P = {
  wing: 'M168 178 C196 170 236 180 270 198 L270 270 L132 270 C146 238 152 200 168 178 Z',
  beakLow: 'M126 112 C98 111 72 112 52 113 C64 118 92 121 116 121 C121 120 126 118 126 112 Z',
  beakUp: 'M128 89 C102 87 68 91 44 99 C33 103 27 108 29 113 C31 118 36 117 37 113 C39 110 43 109 49 109 C74 108 100 109 128 112 Z',
  beakLine: 'M50 110 C74 109 100 110 120 112',
  beakShine: 'M58 100 C76 95 96 93 112 94',
  neck: 'M152 46 C186 46 200 74 196 102 C192 130 178 150 180 176 C182 204 198 228 208 270 L60 270 C70 236 92 212 100 182 C107 156 106 140 108 126 C110 118 112 112 113 104 C114 92 116 78 124 66 C132 54 140 46 152 46 Z',
  brow: 'M110 80 C122 72 140 72 154 80 C144 80 134 82 126 88 C120 86 114 84 110 80 Z',
};

const svgOrigin = (x: number, y: number) => ({ transformOrigin: `${x}px ${y}px`, transformBox: 'view-box' as const });

export function LogoMark({ size = 32, className, animated = true, framed = true }: { size?: number; className?: string; animated?: boolean; framed?: boolean }) {
  const uid = useId().replace(/:/g, '');
  const reduce = useReducedMotion();
  const live = animated && !reduce;
  const [hover, setHover] = useState(false);
  const id = (k: string) => `${k}-${uid}`;

  return (
    <motion.svg
      viewBox="0 0 256 256"
      width={size}
      height={size}
      className={cn('shrink-0 overflow-visible', className)}
      onHoverStart={() => setHover(true)}
      onHoverEnd={() => setHover(false)}
      whileTap={{ scale: 0.92 }}
      role="img"
      aria-label="PeekAround"
    >
      <defs>
        <linearGradient id={id('bg')} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#1d3557" />
          <stop offset="0.55" stopColor="#3b2a6b" />
          <stop offset="1" stopColor="#7c3aed" />
        </linearGradient>
        <linearGradient id={id('beak')} x1="0" y1="0" x2="1" y2="0">
          <stop offset="0" stopColor="#e11d48" />
          <stop offset="0.14" stopColor="#fb7185" />
          <stop offset="0.42" stopColor="#fb923c" />
          <stop offset="1" stopColor="#fdba74" />
        </linearGradient>
        <linearGradient id={id('low')} x1="0" y1="0" x2="1" y2="0">
          <stop offset="0" stopColor="#f43f5e" />
          <stop offset="0.35" stopColor="#f97316" />
          <stop offset="1" stopColor="#fb923c" />
        </linearGradient>
        <linearGradient id={id('neck')} x1="0" y1="0" x2="1" y2="0">
          <stop offset="0" stopColor="#ffffff" />
          <stop offset="0.7" stopColor="#f4f5f9" />
          <stop offset="1" stopColor="#d5d9e6" />
        </linearGradient>
        <clipPath id={id('frame')}>
          <rect width="256" height="256" rx="60" />
        </clipPath>
      </defs>
      {framed && <rect width="256" height="256" rx="60" fill={`url(#${id('bg')})`} />}
      <g clipPath={framed ? `url(#${id('frame')})` : undefined}>
        <path d={P.wing} fill="#131725" />
        {/* the whole bird leans around a pivot low on the neck */}
        <motion.g
          style={svgOrigin(150, 250)}
          animate={live ? (hover ? { rotate: -7, y: -4 } : { rotate: [0, 0, -4, -4, 2.5, 2.5, 0], y: [0, 0, -2, -2, 0, 0, 0] }) : undefined}
          transition={hover ? { type: 'spring', stiffness: 260, damping: 14 } : { duration: 9, times: [0, 0.3, 0.38, 0.55, 0.63, 0.8, 0.9], repeat: Infinity, ease: 'easeInOut' }}
        >
          <path d={P.beakLow} fill={`url(#${id('low')})`} />
          <path d={P.beakUp} fill={`url(#${id('beak')})`} />
          <path d={P.beakLine} stroke="#9a3412" strokeOpacity={0.45} strokeWidth={1.6} fill="none" strokeLinecap="round" />
          <path d={P.beakShine} stroke="#fff" strokeOpacity={0.35} strokeWidth={2} fill="none" strokeLinecap="round" />
          <path d={P.neck} fill={`url(#${id('neck')})`} />
          {/* curious brow raise on hover */}
          <motion.path d={P.brow} fill="#1b1f2e" style={svgOrigin(130, 82)} animate={live && hover ? { y: -3, rotate: 4 } : { y: 0, rotate: 0 }} transition={{ type: 'spring', stiffness: 300, damping: 15 }} />
          <motion.g
            style={svgOrigin(129, 84)}
            animate={live ? { scaleY: [1, 1, 0.12, 1, 1, 0.12, 1] } : undefined}
            transition={{ duration: 5.5, times: [0, 0.7, 0.73, 0.76, 0.9, 0.93, 0.96], repeat: Infinity }}
          >
            <ellipse cx="129" cy="84" rx="5.6" ry="4.4" fill="#0b0d14" />
            <circle cx="127.4" cy="82.6" r="1.5" fill="#fff" />
          </motion.g>
        </motion.g>
      </g>
    </motion.svg>
  );
}

export function LogoWordmark({ className }: { className?: string }) {
  return (
    <span className={cn('font-display text-[19px] font-bold tracking-tight', className)}>
      Peek<span className="text-gradient">Around</span>
    </span>
  );
}

/** Animated splash shown once per session while the first data loads. */
export function Splash() {
  const [show, setShow] = useState(() => {
    try {
      return !sessionStorage.getItem('peek.splash');
    } catch {
      return true;
    }
  });
  useEffect(() => {
    if (!show) return;
    try {
      sessionStorage.setItem('peek.splash', '1');
    } catch {
      /* ignore */
    }
    const t = setTimeout(() => setShow(false), 1500);
    return () => clearTimeout(t);
  }, [show]);
  return (
    <AnimatePresence>
      {show && (
        <motion.div
          className="fixed inset-0 z-[200] grid place-items-center bg-bg"
          exit={{ opacity: 0, transition: { duration: 0.45, ease: [0.22, 1, 0.36, 1] } }}
        >
          <div className="aurora opacity-60" />
          <div className="relative flex flex-col items-center">
            <motion.div
              initial={{ y: 60, scale: 0.6, opacity: 0, rotate: 8 }}
              animate={{ y: 0, scale: 1, opacity: 1, rotate: 0 }}
              transition={{ type: 'spring', stiffness: 220, damping: 16 }}
            >
              <LogoMark size={112} />
            </motion.div>
            <motion.div initial={{ opacity: 0, y: 12, filter: 'blur(8px)' }} animate={{ opacity: 1, y: 0, filter: 'blur(0px)' }} transition={{ delay: 0.35, duration: 0.5 }}>
              <LogoWordmark className="mt-5 block text-4xl" />
            </motion.div>
          </div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
