//! The job queue (mirrors `jobs/index.ts`): two independent serial lanes,
//!  - `media`: video-edit / slideshow (one ffmpeg export at a time — they already use all cores)
//!  - `transfer`: device copies + MEGA imports, one at a time, never waiting for an export.
//!
//! Job lifecycle `queued → running → done | error | canceled`, progress clamped to 0..1, `job`
//! events at most every 300 ms while running (plus every status change), cancel of queued and
//! running jobs, shutdown that cancels the queue and waits ≤ 10 s for running jobs.

use crate::error::{HttpError, JobFailure};
use crate::events::EventSink;
use futures_util::future::BoxFuture;
use futures_util::{Stream, StreamExt};
use pa_core::{FileLocation, Job, JobStatus, JobType, Progress, ServerEvent};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;
use tokio::sync::{watch, Notify};
use tokio_util::sync::CancellationToken;

/// What a runner produces: the saved file (`Job.result`), or why it failed.
pub type JobResult = Result<Option<FileLocation>, JobFailure>;
/// A queued unit of work. Use [`runner`] to build one from an async closure.
pub type JobRunner = Box<dyn FnOnce(JobContext) -> BoxFuture<'static, JobResult> + Send>;

/// Box an async closure as a [`JobRunner`].
pub fn runner<F, Fut>(f: F) -> JobRunner
where
    F: FnOnce(JobContext) -> Fut + Send + 'static,
    Fut: Future<Output = JobResult> + Send + 'static,
{
    Box::new(move |jc| Box::pin(f(jc)))
}

/// job assets (overlay PNGs, music) may only use these extensions
pub const ASSET_EXT: [&str; 12] = [
    "png", "jpg", "jpeg", "webp", "mp3", "m4a", "aac", "ogg", "opus", "wav", "flac", "bin",
];

const TICK: Duration = Duration::from_millis(300);
const CLOSE_WAIT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lane {
    Media = 0,
    Transfer = 1,
}

fn lane_of(t: JobType) -> Lane {
    if t == JobType::Transfer {
        Lane::Transfer
    } else {
        Lane::Media
    }
}

struct State {
    jobs: HashMap<String, Job>,
    /// insertion order (a JS Map iterates in it)
    order: Vec<String>,
    queues: [VecDeque<(String, JobRunner)>; 2],
    running: [Option<(String, CancellationToken)>; 2],
    closed: bool,
}

struct Inner {
    st: Mutex<State>,
    events: Arc<dyn EventSink>,
    /// bumped on every emitted change (wait())
    gen: watch::Sender<u64>,
    active: AtomicUsize,
    idle: Notify,
    asset_dir: PathBuf,
}

/// The job service (`ctx.jobs`). Cheap to clone.
#[derive(Clone)]
pub struct JobService {
    inner: Arc<Inner>,
}

/// Handle a runner gets: cancellation, progress, transfer counters, title / result.
#[derive(Clone)]
pub struct JobContext {
    inner: Arc<Inner>,
    id: String,
    /// fires on cancel (`DELETE /api/jobs/:id`) and on shutdown
    pub cancel: CancellationToken,
    last: Arc<Mutex<Option<Instant>>>,
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `crypto.randomUUID()`
pub fn random_uuid() -> String {
    let mut b: [u8; 16] = rand::random();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

fn clamp01(p: f64) -> f64 {
    // Math.max(0, Math.min(1, p)) — NaN stays NaN
    if p.is_nan() {
        p
    } else {
        p.clamp(0.0, 1.0)
    }
}

fn is_final(s: JobStatus) -> bool {
    !matches!(s, JobStatus::Queued | JobStatus::Running)
}

/// `String(e.message).slice(0, 500)` (UTF-16 units)
fn cut_message(m: &str) -> String {
    pa_core::js::slice16_to(m, 500).to_string()
}

impl Inner {
    fn emit(&self, job: Job) {
        self.events.emit(ServerEvent::Job { job });
        self.gen.send_modify(|g| *g += 1);
    }
}

impl JobService {
    /// `new JobService(events, tempDir)`: assets live in `<temp_dir>/job-assets`.
    pub fn new(events: Arc<dyn EventSink>, temp_dir: impl AsRef<Path>) -> Self {
        Self {
            inner: Arc::new(Inner {
                st: Mutex::new(State {
                    jobs: HashMap::new(),
                    order: Vec::new(),
                    queues: [VecDeque::new(), VecDeque::new()],
                    running: [None, None],
                    closed: false,
                }),
                events,
                gen: watch::channel(0).0,
                active: AtomicUsize::new(0),
                idle: Notify::new(),
                asset_dir: temp_dir.as_ref().join("job-assets"),
            }),
        }
    }

    pub fn asset_dir(&self) -> &Path {
        &self.inner.asset_dir
    }

    /// `GET /api/jobs` → `{ jobs }`: newest first (stable for equal timestamps)
    pub fn list(&self) -> Vec<Job> {
        let st = self.inner.st.lock().unwrap();
        let mut v: Vec<Job> = st
            .order
            .iter()
            .filter_map(|id| st.jobs.get(id).cloned())
            .collect();
        v.sort_by_key(|j| std::cmp::Reverse(j.created_at));
        v
    }

    pub fn get(&self, id: &str) -> Option<Job> {
        self.inner.st.lock().unwrap().jobs.get(id).cloned()
    }

    /// Queue a job. Returns its state right after queueing (already `running` when its lane was idle).
    pub fn enqueue(&self, kind: JobType, title: impl Into<String>, run: JobRunner) -> Job {
        self.enqueue_with(kind, title, |_| {}, run)
    }

    /// Like [`JobService::enqueue`], `init` sets fields before the first event (e.g. the transfer
    /// totals the TS runner sets synchronously on its first line).
    pub fn enqueue_with(
        &self,
        kind: JobType,
        title: impl Into<String>,
        init: impl FnOnce(&mut Job),
        run: JobRunner,
    ) -> Job {
        let transfer = kind == JobType::Transfer;
        let mut job = Job {
            id: random_uuid(),
            kind,
            title: title.into(),
            status: JobStatus::Queued,
            progress: 0.0,
            bytes: transfer.then(Progress::default),
            files: transfer.then(Progress::default),
            error: None,
            result: None,
            created_at: now_ms(),
        };
        init(&mut job);
        let lane = lane_of(kind);
        let id = job.id.clone();
        {
            let mut st = self.inner.st.lock().unwrap();
            if st.closed {
                job.status = JobStatus::Canceled;
                st.order.push(id.clone());
                st.jobs.insert(id, job.clone());
                return job;
            }
            st.order.push(id.clone());
            st.jobs.insert(id.clone(), job.clone());
            st.queues[lane as usize].push_back((id.clone(), run));
        }
        self.inner.emit(job);
        Self::pump(&self.inner, lane);
        self.get(&id).expect("just inserted")
    }

    fn pump(inner: &Arc<Inner>, lane: Lane) {
        let (ctx, run, snapshot) = {
            let mut st = inner.st.lock().unwrap();
            if st.closed || st.running[lane as usize].is_some() {
                return;
            }
            let Some((id, run)) = st.queues[lane as usize].pop_front() else {
                return;
            };
            let cancel = CancellationToken::new();
            st.running[lane as usize] = Some((id.clone(), cancel.clone()));
            let job = st.jobs.get_mut(&id).expect("queued job");
            job.status = JobStatus::Running;
            let snapshot = job.clone();
            let ctx = JobContext {
                inner: inner.clone(),
                id,
                cancel,
                last: Arc::new(Mutex::new(None)),
            };
            (ctx, run, snapshot)
        };
        inner.emit(snapshot);
        inner.active.fetch_add(1, Ordering::SeqCst);
        let inner2 = inner.clone();
        tokio::spawn(async move {
            let id = ctx.id.clone();
            let cancel = ctx.cancel.clone();
            // a panicking runner fails its job instead of wedging the lane
            let res = tokio::spawn(run(ctx)).await.unwrap_or_else(|e| {
                Err(JobFailure::new(if e.is_panic() {
                    "job crashed".to_string()
                } else {
                    "Canceled".to_string()
                }))
            });
            let snapshot = {
                let mut st = inner2.st.lock().unwrap();
                st.running[lane as usize] = None;
                st.jobs.get_mut(&id).map(|job| {
                    match res {
                        Ok(r) => {
                            job.result = r;
                            job.status = JobStatus::Done;
                            job.progress = 1.0;
                        }
                        Err(e) => {
                            let aborted = cancel.is_cancelled();
                            job.status = if aborted {
                                JobStatus::Canceled
                            } else {
                                JobStatus::Error
                            };
                            job.error = if aborted {
                                None
                            } else {
                                Some(cut_message(&e.message))
                            };
                        }
                    }
                    job.clone()
                })
            };
            if let Some(j) = snapshot {
                inner2.emit(j);
            }
            Self::pump(&inner2, lane);
            if inner2.active.fetch_sub(1, Ordering::SeqCst) == 1 {
                inner2.idle.notify_waiters();
            }
        });
    }

    /// `DELETE /api/jobs/:id`: abort a running job, drop a queued one (unknown ids: no-op).
    pub fn cancel(&self, id: &str) {
        let emit = {
            let mut st = self.inner.st.lock().unwrap();
            let Some(j) = st.jobs.get(id) else { return };
            let lane = lane_of(j.kind) as usize;
            let status = j.status;
            if let Some((rid, tok)) = &st.running[lane] {
                if rid == id {
                    tok.cancel();
                    return;
                }
            }
            if status != JobStatus::Queued {
                return;
            }
            st.queues[lane].retain(|(q, _)| q != id);
            let j = st.jobs.get_mut(id).unwrap();
            j.status = JobStatus::Canceled;
            j.clone()
        };
        self.inner.emit(emit);
    }

    /// `DELETE /api/jobs`: remove finished jobs from the list.
    pub fn clear_finished(&self) {
        let mut st = self.inner.st.lock().unwrap();
        let keep: Vec<String> = st
            .order
            .iter()
            .filter(|id| st.jobs.get(*id).is_some_and(|j| !is_final(j.status)))
            .cloned()
            .collect();
        st.jobs.retain(|_, j| !is_final(j.status));
        st.order = keep;
    }

    /// Shutdown: cancel queued jobs, abort running ones and wait (≤ 10 s) for them to wind down.
    pub async fn close(&self) {
        {
            let mut st = self.inner.st.lock().unwrap();
            st.closed = true;
            for lane in 0..2 {
                let q: Vec<String> = st.queues[lane].drain(..).map(|(id, _)| id).collect();
                for id in q {
                    if let Some(j) = st.jobs.get_mut(&id) {
                        j.status = JobStatus::Canceled;
                    }
                }
                if let Some((_, tok)) = &st.running[lane] {
                    tok.cancel();
                }
            }
        }
        let wait = async {
            loop {
                let n = self.inner.idle.notified();
                if self.inner.active.load(Ordering::SeqCst) == 0 {
                    return;
                }
                n.await;
            }
        };
        let _ = tokio::time::timeout(CLOSE_WAIT, wait).await;
    }

    /// Resolves when the job finished (any final status) — tests and chained work.
    pub async fn wait(&self, id: &str) -> Option<Job> {
        let mut rx = self.inner.gen.subscribe();
        loop {
            match self.get(id) {
                None => return None,
                Some(j) if is_final(j.status) => return Some(j),
                _ => {}
            }
            if rx.changed().await.is_err() {
                return self.get(id);
            }
        }
    }

    /// `PUT /api/jobs/assets?ext=` → asset id (`<uuid>.<ext>`; unknown extensions become `bin`)
    pub async fn save_asset(&self, data: &[u8], ext: &str) -> std::io::Result<String> {
        let (id, file) = self.new_asset(ext).await?;
        tokio::fs::write(&file, data).await?;
        Ok(id)
    }

    /// Streaming variant of [`JobService::save_asset`] (request bodies).
    pub async fn save_asset_stream<S, E>(&self, mut body: S, ext: &str) -> std::io::Result<String>
    where
        S: Stream<Item = Result<bytes::Bytes, E>> + Unpin,
        E: std::fmt::Display,
    {
        let (id, file) = self.new_asset(ext).await?;
        let r = async {
            let mut f = tokio::fs::File::create(&file).await?;
            while let Some(c) = body.next().await {
                let c = c.map_err(|e| std::io::Error::other(e.to_string()))?;
                f.write_all(&c).await?;
            }
            f.flush().await
        }
        .await;
        if let Err(e) = r {
            let _ = tokio::fs::remove_file(&file).await;
            return Err(e);
        }
        Ok(id)
    }

    async fn new_asset(&self, ext: &str) -> std::io::Result<(String, PathBuf)> {
        tokio::fs::create_dir_all(&self.inner.asset_dir).await?;
        let lower = ext.to_lowercase();
        let safe = if ASSET_EXT.contains(&lower.as_str()) {
            lower
        } else {
            "bin".into()
        };
        let id = format!("{}.{safe}", random_uuid());
        let file = self
            .asset_path(&id)
            .map_err(|e| std::io::Error::other(e.message))?;
        Ok((id, file))
    }

    /// File of an asset id; 'Invalid asset id' for anything that is not `<36 hex/->.<ext>`.
    pub fn asset_path(&self, id: &str) -> Result<PathBuf, HttpError> {
        static RE: LazyLock<regex::Regex> =
            LazyLock::new(|| regex::Regex::new(r"(?i)^[0-9a-f-]{36}\.[a-z0-9]{1,5}$").unwrap());
        if !RE.is_match(id) {
            return Err(HttpError::bad("Invalid asset id"));
        }
        Ok(self.inner.asset_dir.join(id))
    }
}

impl JobContext {
    pub fn id(&self) -> &str {
        &self.id
    }

    /// current state of this job
    pub fn job(&self) -> Job {
        self.inner
            .st
            .lock()
            .unwrap()
            .jobs
            .get(&self.id)
            .cloned()
            .expect("job of a running context")
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    fn update(&self, f: impl FnOnce(&mut Job)) {
        let snap = {
            let mut st = self.inner.st.lock().unwrap();
            let Some(j) = st.jobs.get_mut(&self.id) else {
                return;
            };
            f(j);
            j.clone()
        };
        let mut last = self.last.lock().unwrap();
        let now = Instant::now();
        if last.is_none_or(|l| now.duration_since(l) > TICK) {
            *last = Some(now);
            drop(last);
            self.inner.emit(snap);
        }
    }

    /// 0..1 (clamped); emits at most every 300 ms
    pub fn progress(&self, p: f64) {
        self.update(|j| j.progress = clamp01(p));
    }

    /// transfers: byte + file counters (progress follows bytes, or files when sizes are unknown)
    pub fn transfer(&self, bytes: Option<Progress>, files: Option<Progress>) {
        self.update(|j| {
            if let Some(b) = bytes {
                j.bytes = Some(b);
            }
            if let Some(f) = files {
                j.files = Some(f);
            }
            if let Some(b) = j.bytes.filter(|b| b.total > 0) {
                j.progress = clamp01(b.done as f64 / b.total as f64);
            } else if let Some(f) = j.files.filter(|f| f.total > 0) {
                j.progress = clamp01(f.done as f64 / f.total as f64);
            }
        });
    }

    /// `jc.job.title = …` (no event of its own)
    pub fn set_title(&self, t: impl Into<String>) {
        if let Some(j) = self.inner.st.lock().unwrap().jobs.get_mut(&self.id) {
            j.title = t.into();
        }
    }

    /// `jc.job.result = …` — kept when the runner then fails (partial imports)
    pub fn set_result(&self, r: Option<FileLocation>) {
        if let Some(j) = self.inner.st.lock().unwrap().jobs.get_mut(&self.id) {
            j.result = r;
        }
    }
}
