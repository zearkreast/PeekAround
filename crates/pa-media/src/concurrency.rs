//! CPU budget knobs (mirror the `Limiter` of `media/thumbs.ts`, `background.ts` and
//! `intensityLimits` of `runtime-settings.ts`).
//!
//! Decode work runs on blocking threads (`tokio::task::spawn_blocking`); how many run at once is
//! what a [`Limiter`] permits, so the Settings' `indexIntensity` bounds the CPU the image side uses.
//! Each image operation itself is single-threaded (no rayon), so "threads per operation" is always 1.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, Notify};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    /// someone is waiting for it (grid, viewer)
    #[default]
    High,
    /// background passes
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LimiterStats {
    pub running: usize,
    pub high: usize,
    pub low: usize,
}

struct Waiter {
    id: u64,
    tx: oneshot::Sender<Permit>,
}

struct LimState {
    max: usize,
    low_max: usize,
    running: usize,
    running_low: usize,
    high: VecDeque<Waiter>,
    low: VecDeque<Waiter>,
    next_id: u64,
}

/// Two-lane concurrency limiter: 'high' requests always go first, 'low' (background) work never
/// takes more than `low_max` slots (by default all but one).
#[derive(Clone)]
pub struct Limiter {
    st: Arc<Mutex<LimState>>,
}

/// A running slot; released when dropped.
pub struct Permit {
    st: Option<Arc<Mutex<LimState>>>,
    low: bool,
}

impl Drop for Permit {
    fn drop(&mut self) {
        if let Some(st) = self.st.take() {
            let mut s = st.lock().unwrap();
            s.running -= 1;
            if self.low {
                s.running_low -= 1;
            }
            Limiter::pump(&st, &mut s);
        }
    }
}

/// A queued request: [`Ticket::promote`] moves it to the high lane, [`Ticket::wait`] gets the slot.
pub struct Ticket {
    id: u64,
    rx: oneshot::Receiver<Permit>,
    limiter: Limiter,
}

impl Ticket {
    pub fn promote(&self) {
        self.limiter.promote(self.id)
    }
    pub async fn wait(self) -> Permit {
        // the sender lives in the queue until a slot is handed over; it is never dropped otherwise
        self.rx.await.expect("limiter dropped a queued request")
    }
}

impl Limiter {
    pub fn new(max: usize) -> Self {
        Self::with_low(max, max.saturating_sub(1).max(1))
    }

    pub fn with_low(max: usize, low_max: usize) -> Self {
        let max = max.max(1);
        Self {
            st: Arc::new(Mutex::new(LimState {
                max,
                low_max: low_max.clamp(1, max),
                running: 0,
                running_low: 0,
                high: VecDeque::new(),
                low: VecDeque::new(),
                next_id: 0,
            })),
        }
    }

    /// The default of the TS server: 2..=6 slots by CPU count.
    pub fn for_cpus() -> Self {
        let n = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2);
        Self::new(n.clamp(2, 6))
    }

    /// Change the slot counts at runtime (Settings.indexIntensity); running work finishes normally.
    pub fn resize(&self, max: usize, low_max: Option<usize>) {
        let mut s = self.st.lock().unwrap();
        s.max = max.max(1);
        s.low_max = low_max
            .unwrap_or(s.max.saturating_sub(1).max(1))
            .clamp(1, s.max);
        Self::pump(&self.st, &mut s);
    }

    pub fn stats(&self) -> LimiterStats {
        let s = self.st.lock().unwrap();
        LimiterStats {
            running: s.running,
            high: s.high.len(),
            low: s.low.len(),
        }
    }

    /// Queue a request (it may already hold its slot when this returns).
    pub fn ticket(&self, prio: Priority) -> Ticket {
        let (tx, rx) = oneshot::channel();
        let mut s = self.st.lock().unwrap();
        let id = s.next_id;
        s.next_id += 1;
        match prio {
            Priority::High => s.high.push_back(Waiter { id, tx }),
            Priority::Low => s.low.push_back(Waiter { id, tx }),
        }
        Self::pump(&self.st, &mut s);
        Ticket {
            id,
            rx,
            limiter: self.clone(),
        }
    }

    pub async fn acquire(&self, prio: Priority) -> Permit {
        self.ticket(prio).wait().await
    }

    /// Run `f` in a slot.
    pub async fn run<T, F: Future<Output = T>>(&self, prio: Priority, f: F) -> T {
        let _p = self.acquire(prio).await;
        f.await
    }

    /// Run blocking CPU work (decode / resize / encode) on a blocking thread in a slot.
    pub async fn run_blocking<T: Send + 'static>(
        &self,
        prio: Priority,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> T {
        let _p = self.acquire(prio).await;
        tokio::task::spawn_blocking(f)
            .await
            .expect("media worker panicked")
    }

    fn promote(&self, id: u64) {
        let mut s = self.st.lock().unwrap();
        if let Some(i) = s.low.iter().position(|w| w.id == id) {
            let w = s.low.remove(i).unwrap();
            s.high.push_back(w);
            Self::pump(&self.st, &mut s);
        }
    }

    fn pump(st: &Arc<Mutex<LimState>>, s: &mut LimState) {
        while s.running < s.max {
            let (w, low) = if let Some(w) = s.high.pop_front() {
                (w, false)
            } else if s.running_low < s.low_max {
                match s.low.pop_front() {
                    Some(w) => (w, true),
                    None => break,
                }
            } else {
                break;
            };
            s.running += 1;
            if low {
                s.running_low += 1;
            }
            if let Err(mut p) = w.tx.send(Permit {
                st: Some(st.clone()),
                low,
            }) {
                // the request was dropped while queued: undo without re-locking
                p.st = None;
                s.running -= 1;
                if low {
                    s.running_low -= 1;
                }
            }
        }
    }
}

struct PoolState {
    size: usize,
    gap: Duration,
    active: usize,
    quiet_until: Option<Instant>,
    closed: bool,
}

/// One budget for all background work (scanner meta / thumbnail / enrichment passes, duplicate
/// hashing): at most `size` items at once; on-demand work calls [`BackgroundPool::foreground`] and
/// new background items then wait until the user has been quiet for `foreground_ms`.
#[derive(Clone)]
pub struct BackgroundPool {
    st: Arc<Mutex<PoolState>>,
    notify: Arc<Notify>,
    foreground: Duration,
}

impl BackgroundPool {
    pub fn new(size: usize, gap: Duration) -> Self {
        Self {
            st: Arc::new(Mutex::new(PoolState {
                size: size.max(1),
                gap,
                active: 0,
                quiet_until: None,
                closed: false,
            })),
            notify: Arc::new(Notify::new()),
            foreground: Duration::from_millis(1200),
        }
    }

    pub fn with_foreground(mut self, d: Duration) -> Self {
        self.foreground = d;
        self
    }

    pub fn configure(&self, size: usize, gap: Duration) {
        let mut s = self.st.lock().unwrap();
        s.size = size.max(1);
        s.gap = gap;
        drop(s);
        self.notify.notify_waiters();
    }

    /// A user is waiting for something: hold new background items back for a moment.
    pub fn foreground(&self) {
        if self.foreground.is_zero() {
            return;
        }
        let until = Instant::now() + self.foreground;
        let mut s = self.st.lock().unwrap();
        s.quiet_until = Some(s.quiet_until.map_or(until, |q| q.max(until)));
    }

    /// (active, size)
    pub fn stats(&self) -> (usize, usize) {
        let s = self.st.lock().unwrap();
        (s.active, s.size)
    }

    /// Release everything that waits (shutdown); callers check their own stop flag.
    pub fn close(&self) {
        self.st.lock().unwrap().closed = true;
        self.notify.notify_waiters();
    }

    async fn acquire(&self) -> bool {
        enum Step {
            Closed,
            Got,
            Sleep(Duration),
            Wait,
        }
        loop {
            let notified = self.notify.notified();
            let step = {
                let mut s = self.st.lock().unwrap();
                let wait = s
                    .quiet_until
                    .and_then(|q| q.checked_duration_since(Instant::now()))
                    .filter(|w| !w.is_zero());
                if s.closed {
                    Step::Closed
                } else if let Some(w) = wait {
                    Step::Sleep(w)
                } else if s.active < s.size {
                    s.active += 1;
                    Step::Got
                } else {
                    Step::Wait
                }
            };
            match step {
                Step::Closed => return false,
                Step::Got => return true,
                Step::Sleep(w) => {
                    tokio::select! { _ = tokio::time::sleep(w) => {}, _ = notified => {} }
                }
                Step::Wait => notified.await,
            }
        }
    }

    fn release(&self) {
        let gap = {
            let s = self.st.lock().unwrap();
            if s.closed {
                Duration::ZERO
            } else {
                s.gap
            }
        };
        let st = self.st.clone();
        let notify = self.notify.clone();
        let done = move || {
            let mut s = st.lock().unwrap();
            s.active = s.active.saturating_sub(1);
            drop(s);
            notify.notify_one();
        };
        if gap.is_zero() {
            done();
        } else if let Ok(h) = tokio::runtime::Handle::try_current() {
            h.spawn(async move {
                tokio::time::sleep(gap).await;
                done();
            });
        } else {
            done();
        }
    }

    /// Run one background item when a slot is free (after `close()`: right away).
    pub async fn run<T, F: Future<Output = T>>(&self, f: F) -> T {
        let counted = self.acquire().await;
        let r = f.await;
        if counted {
            self.release();
        }
        tokio::task::yield_now().await;
        r
    }
}

/// Settings.indexIntensity
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum IndexIntensity {
    Low,
    #[default]
    Normal,
    High,
}

/// Background CPU budget per intensity (mirrors `intensityLimits`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntensityLimits {
    /// decode limiter slots (on-demand + background)
    pub max: usize,
    /// of which background
    pub low_max: usize,
    /// background items at once across every pass
    pub workers: usize,
    /// pause after each background item
    pub gap_ms: u64,
    /// threads per image operation in sharp (kept for the settings API; the Rust engine always uses 1)
    pub vips_threads: usize,
    /// ffmpeg decoder threads for frames / probes; 0 = ffmpeg's default
    pub ffmpeg_threads: u32,
}

pub fn intensity_limits(i: IndexIntensity, cpus: usize, desktop: bool) -> IntensityLimits {
    let n = cpus.max(1) as f64;
    let clamp = |v: f64, lo: usize, hi: usize| (v.floor() as usize).clamp(lo, hi);
    let r = match i {
        IndexIntensity::Low => IntensityLimits {
            max: clamp(n / 2.0, 2, 4),
            low_max: 1,
            workers: 1,
            gap_ms: 40,
            vips_threads: 1,
            ffmpeg_threads: 1,
        },
        IndexIntensity::High => {
            let workers = clamp(n - 1.0, 2, 8);
            IntensityLimits {
                max: clamp(n, 2, 12),
                low_max: workers,
                workers,
                gap_ms: 0,
                vips_threads: 0,
                ffmpeg_threads: 0,
            }
        }
        IndexIntensity::Normal => {
            let workers = if desktop {
                clamp(n / 4.0, 1, 2)
            } else {
                clamp(n / 2.0, 1, 3)
            };
            IntensityLimits {
                max: clamp(n, 2, 6),
                low_max: workers,
                workers,
                gap_ms: 0,
                vips_threads: 1,
                ffmpeg_threads: 2,
            }
        }
    };
    IntensityLimits {
        low_max: r.low_max.min(r.max),
        ..r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn limiter_lanes_and_promote() {
        let l = Limiter::with_low(2, 1);
        let a = l.acquire(Priority::Low).await;
        let t_low = l.ticket(Priority::Low); // low lane full (1)
        let t_high = l.ticket(Priority::High); // gets the second slot
        assert_eq!(
            l.stats(),
            LimiterStats {
                running: 2,
                high: 0,
                low: 1
            }
        );
        let h = t_high.wait().await;
        drop(a);
        // the low request may now run (low running 0 < 1)
        let lo = t_low.wait().await;
        assert_eq!(l.stats().running, 2);
        drop(h);
        drop(lo);
        assert_eq!(l.stats().running, 0);
        // promote: a queued low request jumps the low cap
        let l = Limiter::with_low(2, 1);
        let _a = l.acquire(Priority::Low).await;
        let t = l.ticket(Priority::Low);
        assert_eq!(l.stats().low, 1);
        t.promote();
        let _p = t.wait().await;
        assert_eq!(l.stats().running, 2);
        // a dropped queued request does not leak its slot
        let l = Limiter::with_low(1, 1);
        let p = l.acquire(Priority::High).await;
        drop(l.ticket(Priority::High));
        drop(p);
        assert_eq!(
            l.stats(),
            LimiterStats {
                running: 0,
                high: 0,
                low: 0
            }
        );
        let _ = l.acquire(Priority::High).await;
        l.resize(4, None);
    }

    #[tokio::test]
    async fn background_pool_bounds_and_foreground() {
        let pool =
            BackgroundPool::new(2, Duration::ZERO).with_foreground(Duration::from_millis(150));
        let live = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut hs = Vec::new();
        for _ in 0..6 {
            let (pool, live, peak) = (pool.clone(), live.clone(), peak.clone());
            hs.push(tokio::spawn(async move {
                pool.run(async {
                    let n = live.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(n, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    live.fetch_sub(1, Ordering::SeqCst);
                })
                .await
            }));
        }
        for h in hs {
            h.await.unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        pool.foreground();
        let t0 = Instant::now();
        pool.run(async {}).await;
        assert!(t0.elapsed() >= Duration::from_millis(120));
        pool.close();
        pool.run(async {}).await;
    }

    #[test]
    fn intensity_table() {
        let l = intensity_limits(IndexIntensity::Normal, 8, false);
        assert_eq!(
            (l.max, l.low_max, l.workers, l.ffmpeg_threads),
            (6, 3, 3, 2)
        );
        let d = intensity_limits(IndexIntensity::Normal, 8, true);
        assert_eq!(d.workers, 2);
        let lo = intensity_limits(IndexIntensity::Low, 4, false);
        assert_eq!((lo.max, lo.low_max, lo.gap_ms), (2, 1, 40));
        let hi = intensity_limits(IndexIntensity::High, 16, false);
        assert_eq!((hi.max, hi.low_max, hi.workers), (12, 8, 8));
    }
}
