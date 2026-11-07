//! Tiny pub/sub used by the WebSocket route (mirrors `events.ts`): coalesces + rate-limits noisy
//! media events per folder and reason, and keeps the media-table generation (`BrowseSkeleton.gen`).

use indexmap::IndexSet;
use pa_core::{MediaReason, ServerEvent};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::time::{Duration, Instant};

type Listener = Arc<dyn Fn(&ServerEvent) + Send + Sync>;

/// Minimum spacing (ms) between two media events of one folder, per reason.
#[derive(Debug, Clone, Copy)]
pub struct Intervals {
    pub add: u64,
    pub remove: u64,
    pub change: u64,
    pub meta: u64,
    pub thumb: u64,
}

impl Default for Intervals {
    fn default() -> Self {
        Intervals {
            add: 250,
            remove: 250,
            change: 250,
            meta: 3000,
            thumb: 10_000,
        }
    }
}

impl Intervals {
    fn of(&self, r: MediaReason) -> u64 {
        match r {
            MediaReason::Add => self.add,
            MediaReason::Remove => self.remove,
            MediaReason::Change => self.change,
            MediaReason::Meta => self.meta,
            MediaReason::Thumb => self.thumb,
        }
    }
}

struct Pending {
    dirs: IndexSet<String>,
    timer: bool,
    last: Option<Instant>,
}

struct MetaGen {
    at: Option<Instant>,
    timer: bool,
}

/// The event hub (cheap to clone).
#[derive(Clone)]
pub struct EventHub {
    inner: Arc<Inner>,
}

struct Inner {
    listeners: Mutex<Vec<(u64, Listener)>>,
    next_id: AtomicU64,
    pending: Mutex<HashMap<(String, u8), Pending>>,
    boot: String,
    counter: AtomicU64,
    intervals: Mutex<Intervals>,
    meta_gen_ms: AtomicU64,
    meta_gen: Mutex<MetaGen>,
    closed: AtomicBool,
    epoch: AtomicU64,
    rt: Option<tokio::runtime::Handle>,
}

fn reason_key(r: MediaReason) -> u8 {
    r as u8
}

/// Unsubscribes when dropped / called.
pub struct Subscription {
    inner: std::sync::Weak<Inner>,
    id: u64,
}

impl Subscription {
    /// stop listening
    pub fn off(self) {}
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(i) = self.inner.upgrade() {
            i.listeners.lock().retain(|(id, _)| *id != self.id);
        }
    }
}

fn base36(mut n: u128) -> String {
    const D: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut s = Vec::new();
    while n > 0 {
        s.push(D[(n % 36) as usize]);
        n /= 36;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

/// ms since the epoch (`Date.now()`)
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new()
    }
}

impl EventHub {
    /// new hub (timers run on the current tokio runtime, when there is one)
    pub fn new() -> Self {
        EventHub {
            inner: Arc::new(Inner {
                listeners: Mutex::new(Vec::new()),
                next_id: AtomicU64::new(1),
                pending: Mutex::new(HashMap::new()),
                boot: base36(now_ms() as u128),
                counter: AtomicU64::new(0),
                intervals: Mutex::new(Intervals::default()),
                meta_gen_ms: AtomicU64::new(1500),
                meta_gen: Mutex::new(MetaGen {
                    at: None,
                    timer: false,
                }),
                closed: AtomicBool::new(false),
                epoch: AtomicU64::new(0),
                rt: tokio::runtime::Handle::try_current().ok(),
            }),
        }
    }

    /// test hook: override the per-reason intervals
    pub fn set_intervals(&self, i: Intervals) {
        *self.inner.intervals.lock() = i;
    }

    /// test hook: spacing of throttled generation changes
    pub fn set_meta_gen_ms(&self, ms: u64) {
        self.inner.meta_gen_ms.store(ms, Ordering::Relaxed);
    }

    pub fn on(&self, f: impl Fn(&ServerEvent) + Send + Sync + 'static) -> Subscription {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        self.inner.listeners.lock().push((id, Arc::new(f)));
        Subscription {
            inner: Arc::downgrade(&self.inner),
            id,
        }
    }

    /// publish to every listener
    pub fn emit(&self, e: ServerEvent) {
        Inner::emit(&self.inner, e)
    }

    /// generation of the media table (`${boot}.${counter}`)
    pub fn gen(&self) -> String {
        format!(
            "{}.{}",
            self.inner.boot,
            self.inner.counter.load(Ordering::SeqCst)
        )
    }

    /// mark the media table as changed without an event
    pub fn bump(&self) {
        self.inner.counter.fetch_add(1, Ordering::SeqCst);
    }

    /// a throttled generation change is still due
    pub fn gen_pending(&self) -> bool {
        self.inner.meta_gen.lock().timer
    }

    /// bump() for bulk writes: right away, or once at the end of the current window
    pub fn bump_throttled(&self) {
        Inner::bump_throttled(&self.inner)
    }

    /// Queue a media event for a folder dir (coalesced per folder + reason).
    pub fn media_changed(&self, folder_id: &str, dir: &str, reason: MediaReason) {
        let inner = &self.inner;
        match reason {
            MediaReason::Meta => Inner::bump_throttled(inner),
            MediaReason::Thumb => {}
            _ => {
                inner.counter.fetch_add(1, Ordering::SeqCst);
            }
        }
        let key = (folder_id.to_string(), reason_key(reason));
        let mut pending = inner.pending.lock();
        let p = pending.entry(key.clone()).or_insert_with(|| Pending {
            dirs: IndexSet::new(),
            timer: false,
            last: None,
        });
        p.dirs.insert(dir.to_string());
        if p.timer {
            return;
        }
        let interval = inner.intervals.lock().of(reason);
        let now = Instant::now();
        let quick = 250.min(interval);
        let due = p.last.map(|l| l + Duration::from_millis(interval));
        let delay = match due {
            Some(d) if d > now => (d - now).max(Duration::from_millis(quick)),
            _ => Duration::from_millis(quick),
        };
        p.timer = true;
        drop(pending);
        let Some(rt) = inner
            .rt
            .clone()
            .or_else(|| tokio::runtime::Handle::try_current().ok())
        else {
            return;
        };
        let weak = Arc::downgrade(inner);
        let epoch = inner.epoch.load(Ordering::SeqCst);
        let folder = folder_id.to_string();
        rt.spawn(async move {
            tokio::time::sleep(delay).await;
            let Some(inner) = weak.upgrade() else { return };
            if inner.epoch.load(Ordering::SeqCst) != epoch || inner.closed.load(Ordering::SeqCst) {
                return;
            }
            let dirs = {
                let mut pending = inner.pending.lock();
                let Some(p) = pending.get_mut(&key) else {
                    return;
                };
                p.timer = false;
                p.last = Some(Instant::now());
                let d: Vec<String> = if p.dirs.contains("*") {
                    vec!["*".into()]
                } else {
                    p.dirs.iter().cloned().collect()
                };
                p.dirs.clear();
                d
            };
            Inner::emit(
                &inner,
                ServerEvent::Media {
                    folder_id: folder,
                    dirs,
                    reason: Some(reason),
                },
            );
        });
    }

    /// drop pending timers (shutdown)
    pub fn close(&self) {
        self.inner.closed.store(true, Ordering::SeqCst);
        self.inner.epoch.fetch_add(1, Ordering::SeqCst);
        self.inner.pending.lock().clear();
        self.inner.meta_gen.lock().timer = false;
    }
}

impl Inner {
    fn emit(this: &Arc<Inner>, e: ServerEvent) {
        if let ServerEvent::Folders { status: None } = &e {
            this.counter.fetch_add(1, Ordering::SeqCst);
        }
        let ls: Vec<Listener> = this
            .listeners
            .lock()
            .iter()
            .map(|(_, l)| l.clone())
            .collect();
        for l in ls {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| l(&e)));
        }
    }

    fn bump_throttled(this: &Arc<Inner>) {
        let ms = this.meta_gen_ms.load(Ordering::Relaxed);
        let now = Instant::now();
        let mut g = this.meta_gen.lock();
        let elapsed_ok =
            g.at.is_none_or(|a| now.duration_since(a) >= Duration::from_millis(ms));
        if elapsed_ok {
            g.at = Some(now);
            this.counter.fetch_add(1, Ordering::SeqCst);
            return;
        }
        if g.timer {
            return;
        }
        g.timer = true;
        let delay =
            (g.at.unwrap_or(now) + Duration::from_millis(ms)).saturating_duration_since(now);
        drop(g);
        let Some(rt) = this
            .rt
            .clone()
            .or_else(|| tokio::runtime::Handle::try_current().ok())
        else {
            return;
        };
        let weak = Arc::downgrade(this);
        let epoch = this.epoch.load(Ordering::SeqCst);
        rt.spawn(async move {
            tokio::time::sleep(delay).await;
            let Some(inner) = weak.upgrade() else { return };
            if inner.epoch.load(Ordering::SeqCst) != epoch {
                return;
            }
            let mut g = inner.meta_gen.lock();
            g.timer = false;
            g.at = Some(Instant::now());
            inner.counter.fetch_add(1, Ordering::SeqCst);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn coalesces_and_rate_limits() {
        let hub = EventHub::new();
        let got = Arc::new(Mutex::new(Vec::<ServerEvent>::new()));
        let g2 = got.clone();
        let _s = hub.on(move |e| g2.lock().push(e.clone()));
        let mut t = 0;
        while t < 20_000 {
            hub.media_changed("1", &format!("d{}", (t / 40) % 3), MediaReason::Thumb);
            hub.media_changed("1", "d0", MediaReason::Meta);
            tokio::time::sleep(Duration::from_millis(40)).await;
            t += 40;
        }
        tokio::time::sleep(Duration::from_millis(15_000)).await;
        let got = got.lock();
        let thumbs: Vec<_> = got
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ServerEvent::Media {
                        reason: Some(MediaReason::Thumb),
                        ..
                    }
                )
            })
            .collect();
        let metas = got
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    ServerEvent::Media {
                        reason: Some(MediaReason::Meta),
                        ..
                    }
                )
            })
            .count();
        assert!(thumbs.len() <= 3 && thumbs.len() >= 2, "{}", thumbs.len());
        assert!(metas <= 8);
        if let ServerEvent::Media { dirs, .. } = thumbs[0] {
            let mut d = dirs.clone();
            d.sort();
            assert_eq!(d, vec!["d0", "d1", "d2"]);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn add_remove_quick_and_gen() {
        let hub = EventHub::new();
        let got = Arc::new(Mutex::new(Vec::<ServerEvent>::new()));
        let g2 = got.clone();
        let _s = hub.on(move |e| g2.lock().push(e.clone()));
        let g0 = hub.gen();
        hub.media_changed("1", "", MediaReason::Thumb);
        assert_eq!(hub.gen(), g0);
        hub.media_changed("1", "a", MediaReason::Add);
        hub.media_changed("2", "*", MediaReason::Remove);
        assert_ne!(hub.gen(), g0);
        tokio::time::sleep(Duration::from_millis(300)).await;
        let got: Vec<_> = got
            .lock()
            .iter()
            .filter(|e| {
                !matches!(
                    e,
                    ServerEvent::Media {
                        reason: Some(MediaReason::Thumb),
                        ..
                    }
                )
            })
            .cloned()
            .collect();
        assert_eq!(
            got,
            vec![
                ServerEvent::Media {
                    folder_id: "1".into(),
                    dirs: vec!["a".into()],
                    reason: Some(MediaReason::Add)
                },
                ServerEvent::Media {
                    folder_id: "2".into(),
                    dirs: vec!["*".into()],
                    reason: Some(MediaReason::Remove)
                },
            ]
        );
        hub.close();
    }
}
