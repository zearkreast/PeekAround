//! Transfer slots and the quota pause (the transfer bookkeeping of `remote/folders.ts`):
//! at most 4 transfers per link, background work never above 2, 'high' (viewer, playback) first;
//! a 509 / EOVERQUOTA stops every MEGA transfer until the quota resets (default 30 min), EAGAIN /
//! ERATELIMIT back off for 10 s.

use crate::error::{http_error, HttpError, MegaError};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    High,
    Low,
}

struct LaneState {
    max: usize,
    low_max: usize,
    running: usize,
    running_low: usize,
    queue: VecDeque<(Priority, oneshot::Sender<LaneGuard>)>,
}

/// Two-lane semaphore of one link.
#[derive(Clone)]
pub struct Lanes {
    st: Arc<Mutex<LaneState>>,
}

/// A transfer slot; released on drop.
pub struct LaneGuard {
    st: Option<Arc<Mutex<LaneState>>>,
    prio: Priority,
}

impl Drop for LaneGuard {
    fn drop(&mut self) {
        if let Some(st) = self.st.take() {
            let mut s = st.lock().unwrap();
            s.running -= 1;
            if self.prio == Priority::Low {
                s.running_low -= 1;
            }
            Lanes::pump(&st, &mut s);
        }
    }
}

impl Lanes {
    pub fn new(max: usize, low_max: usize) -> Self {
        Self {
            st: Arc::new(Mutex::new(LaneState {
                max,
                low_max,
                running: 0,
                running_low: 0,
                queue: VecDeque::new(),
            })),
        }
    }

    /// (running, waiting)
    pub fn stats(&self) -> (usize, usize) {
        let s = self.st.lock().unwrap();
        (s.running, s.queue.len())
    }

    /// Wait for a slot ('high' requests queue in front of every waiting 'low' one).
    pub async fn acquire(
        &self,
        prio: Priority,
        cancel: &CancellationToken,
    ) -> Result<LaneGuard, MegaError> {
        if cancel.is_cancelled() {
            return Err(MegaError::Aborted);
        }
        let (tx, rx) = oneshot::channel();
        {
            let mut s = self.st.lock().unwrap();
            let at = match prio {
                Priority::High => s
                    .queue
                    .iter()
                    .position(|q| q.0 == Priority::Low)
                    .unwrap_or(s.queue.len()),
                Priority::Low => s.queue.len(),
            };
            s.queue.insert(at, (prio, tx));
            Self::pump(&self.st, &mut s);
        }
        tokio::select! {
            g = rx => g.map_err(|_| MegaError::Aborted),
            _ = cancel.cancelled() => Err(MegaError::Aborted),
        }
    }

    fn pump(st: &Arc<Mutex<LaneState>>, s: &mut LaneState) {
        while s.running < s.max {
            let Some(i) = s
                .queue
                .iter()
                .position(|q| q.0 == Priority::High || s.running_low < s.low_max)
            else {
                return;
            };
            let (prio, tx) = s.queue.remove(i).unwrap();
            if tx.is_closed() {
                continue;
            }
            s.running += 1;
            if prio == Priority::Low {
                s.running_low += 1;
            }
            if let Err(mut g) = tx.send(LaneGuard {
                st: Some(st.clone()),
                prio,
            }) {
                g.st = None;
                s.running -= 1;
                if prio == Priority::Low {
                    s.running_low -= 1;
                }
            }
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `new Date(ms).toISOString()`
pub fn iso_time(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // civil from days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60,
        ms % 1000
    )
}

pub const DEFAULT_QUOTA_WAIT_S: u64 = 30 * 60;
const BACKOFF: Duration = Duration::from_secs(10);

struct GateState {
    quota_until: u64,
    backoff_until: u64,
    lanes: HashMap<String, Lanes>,
}

/// Shared by every link (the quota is per IP, or per account when logged in).
#[derive(Clone)]
pub struct TransferGate {
    st: Arc<Mutex<GateState>>,
    /// fires on shutdown: queued and running transfers stop
    pub shutdown: CancellationToken,
    pub max: usize,
    pub low_max: usize,
}

impl Default for TransferGate {
    fn default() -> Self {
        Self::new(4, 2)
    }
}

impl TransferGate {
    pub fn new(max: usize, low_max: usize) -> Self {
        Self {
            st: Arc::new(Mutex::new(GateState {
                quota_until: 0,
                backoff_until: 0,
                lanes: HashMap::new(),
            })),
            shutdown: CancellationToken::new(),
            max,
            low_max,
        }
    }

    pub fn lanes(&self, link_id: &str) -> Lanes {
        let mut s = self.st.lock().unwrap();
        let (max, low) = (self.max, self.low_max);
        s.lanes
            .entry(link_id.to_string())
            .or_insert_with(|| Lanes::new(max, low))
            .clone()
    }

    /// ms epoch until which no MEGA transfer runs (0 = none)
    pub fn quota_until(&self) -> u64 {
        let q = self.st.lock().unwrap().quota_until;
        if q > now_ms() {
            q
        } else {
            0
        }
    }

    /// restore a persisted pause (settings `remoteQuota.until`)
    pub fn set_quota_until(&self, until: u64) {
        self.st.lock().unwrap().quota_until = until;
    }

    /// the session changed (other quota now) — lift the pause and the back-off
    pub fn resume(&self) {
        let mut s = self.st.lock().unwrap();
        s.quota_until = 0;
        s.backoff_until = 0;
    }

    /// 429 while the quota pause lasts
    pub fn check_quota(&self) -> Result<(), HttpError> {
        let q = self.quota_until();
        if q == 0 {
            return Ok(());
        }
        let s = (q - now_ms()).div_ceil(1000);
        Err(HttpError {
            status: 429,
            message: format!("MEGA transfer quota exceeded, retry at {}", iso_time(q)),
            retry_after: Some(s),
        })
    }

    /// A failed transfer: quota → pause every transfer until it resets (returns the new
    /// `until` to persist), congestion → 10 s back-off.
    pub fn note_error(&self, e: &MegaError) -> Option<u64> {
        if e.is_congestion() {
            self.st.lock().unwrap().backoff_until = now_ms() + BACKOFF.as_millis() as u64;
            return None;
        }
        let err = http_error(e);
        if err.status != 429 {
            return None;
        }
        let wait = err
            .retry_after
            .filter(|s| *s > 0)
            .unwrap_or(DEFAULT_QUOTA_WAIT_S);
        let until = now_ms() + wait * 1000;
        let mut s = self.st.lock().unwrap();
        if until <= s.quota_until {
            return None;
        }
        s.quota_until = until;
        Some(until)
    }

    /// a cancel token that also fires on shutdown
    pub fn token(&self, cancel: Option<&CancellationToken>) -> CancellationToken {
        let t = self.shutdown.child_token();
        if let Some(c) = cancel {
            let (c, t2) = (c.clone(), t.clone());
            tokio::spawn(async move {
                tokio::select! { _ = c.cancelled() => t2.cancel(), _ = t2.cancelled() => {} }
            });
        }
        t
    }

    /// Wait for a transfer slot of `link_id` (quota check, back-off, lanes).
    pub async fn slot(
        &self,
        link_id: &str,
        prio: Priority,
        cancel: &CancellationToken,
    ) -> Result<LaneGuard, MegaError> {
        self.check_quota()?;
        let backoff = self.st.lock().unwrap().backoff_until;
        let now = now_ms();
        if backoff > now {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(backoff - now)) => {}
                _ = cancel.cancelled() => return Err(MegaError::Aborted),
            }
        }
        self.check_quota()?;
        self.lanes(link_id).acquire(prio, cancel).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// remote-folders.test.ts "lanes: at most 4 transfers, background never above 2, high first"
    #[tokio::test]
    async fn lanes_order_and_caps() {
        let l = Lanes::new(4, 2);
        let order = Arc::new(Mutex::new(Vec::<String>::new()));
        let (gtx, mut grx) = tokio::sync::mpsc::unbounded_channel::<LaneGuard>();
        let never = CancellationToken::new();
        for i in 0..3 {
            let (l, o, g, c) = (l.clone(), order.clone(), gtx.clone(), never.clone());
            tokio::spawn(async move {
                let guard = l.acquire(Priority::Low, &c).await.unwrap();
                o.lock().unwrap().push(format!("low{i}"));
                g.send(guard).ok();
            });
            tokio::task::yield_now().await;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(*order.lock().unwrap(), ["low0", "low1"]);
        for i in 0..3 {
            let (l, o, g, c) = (l.clone(), order.clone(), gtx.clone(), never.clone());
            tokio::spawn(async move {
                let guard = l.acquire(Priority::High, &c).await.unwrap();
                o.lock().unwrap().push(format!("high{i}"));
                g.send(guard).ok();
            });
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(*order.lock().unwrap(), ["low0", "low1", "high0", "high1"]);
        drop(grx.recv().await.unwrap());
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(order.lock().unwrap().last().unwrap(), "high2");
        // a canceled waiter leaves the queue
        let c = CancellationToken::new();
        let l2 = l.clone();
        let c2 = c.clone();
        let h = tokio::spawn(async move { l2.acquire(Priority::High, &c2).await.is_err() });
        tokio::time::sleep(Duration::from_millis(5)).await;
        c.cancel();
        assert!(h.await.unwrap());
    }

    #[tokio::test]
    async fn quota_pause_and_backoff() {
        let g = TransferGate::default();
        assert!(g.check_quota().is_ok());
        assert_eq!(g.note_error(&MegaError::Network("x".into())), None);
        let until = g
            .note_error(&MegaError::Quota { seconds: Some(600) })
            .unwrap();
        assert!(until > now_ms() + 590_000);
        let e = g.check_quota().unwrap_err();
        assert_eq!(e.status, 429);
        assert!(
            e.message
                .starts_with("MEGA transfer quota exceeded, retry at 20"),
            "{}",
            e.message
        );
        assert!(e.retry_after.unwrap() > 590);
        // a shorter quota does not shorten the pause
        assert_eq!(g.note_error(&MegaError::Quota { seconds: Some(1) }), None);
        // no reset time from MEGA: 30 min
        assert!(g.note_error(&MegaError::Http(509)).unwrap() >= until);
        let c = CancellationToken::new();
        assert!(matches!(
            g.slot("L", Priority::High, &c).await,
            Err(MegaError::Http4xx(HttpError { status: 429, .. }))
        ));
        g.resume();
        assert!(g.slot("L", Priority::High, &c).await.is_ok());
        g.note_error(&MegaError::Api(crate::error::EAGAIN));
        assert_eq!(iso_time(1_700_000_000_123), "2023-11-14T22:13:20.123Z");
    }
}
