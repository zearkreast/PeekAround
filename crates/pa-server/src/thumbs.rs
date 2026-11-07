//! ThumbService (mirrors `media/thumbs.ts`): cached renditions (pa-media), one base decode per
//! source shared by every caller, in-flight de-duplication, atomic cache files, two decode lanes
//! (on-demand first, background never takes the last slot) and the foreground hold of the shared
//! background budget.

use futures_util::future::{BoxFuture, FutureExt, Shared};
use pa_media::render::{self, ThumbQuality};
use pa_media::{BackgroundPool, Ffmpeg};
use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

/// Decode lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prio {
    /// someone waits for it
    High,
    /// background passes
    Low,
}

/// The row facts a rendition needs.
#[derive(Debug, Clone)]
pub struct ThumbRow {
    pub kind: String,
    pub ext: String,
    pub mtime: i64,
    pub size: i64,
    pub duration: Option<f64>,
}

impl From<&crate::db::MediaRow> for ThumbRow {
    fn from(r: &crate::db::MediaRow) -> Self {
        ThumbRow {
            kind: r.kind.clone(),
            ext: r.ext.clone(),
            mtime: r.mtime,
            size: r.size,
            duration: r.duration,
        }
    }
}

/// Result of a base decode.
#[derive(Debug, Clone, PartialEq)]
pub struct BaseInfo {
    pub thumbhash: Option<String>,
    pub width: u32,
    pub height: u32,
}

/// base decode outcome (Err = decode failed)
pub type BaseResult = Result<Option<BaseInfo>, String>;
type FileResult = Result<Option<PathBuf>, String>;

/// Images of media that live on a linked phone / in a MEGA link (pa-link / pa-remote implement it).
pub trait RemoteImageSource: Send + Sync {
    /// JPEG thumbnail, long edge ≈ size
    fn thumb(
        &self,
        abs: &str,
        size: u32,
        prio: Prio,
    ) -> BoxFuture<'static, Result<Vec<u8>, String>>;
    /// JPEG, long edge ≤ max
    fn display(&self, abs: &str, max: u32) -> BoxFuture<'static, Result<Vec<u8>, String>>;
    /// JPEG sprite of n frames, h px high
    fn strip(&self, abs: &str, n: u32, h: u32) -> BoxFuture<'static, Result<Vec<u8>, String>>;
}

/// `device://` / `remote://` pseudo paths
pub fn is_remote_abs(abs: &str) -> bool {
    abs.starts_with("device://") || abs.starts_with("remote://")
}

// ───────────────────────── Limiter ─────────────────────────

struct Waiter {
    id: u64,
    tx: oneshot::Sender<bool>,
}

struct LimState {
    max: usize,
    low_max: usize,
    running: usize,
    running_low: usize,
    high: VecDeque<Waiter>,
    low: VecDeque<Waiter>,
    next: u64,
}

/// Two-lane concurrency limiter (`Limiter` of thumbs.ts) with promotion of queued low requests.
#[derive(Clone)]
pub struct Limiter {
    st: Arc<Mutex<LimState>>,
}

/// A held slot (released on drop).
pub struct Slot {
    st: Arc<Mutex<LimState>>,
    low: bool,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut s = self.st.lock();
        s.running -= 1;
        if self.low {
            s.running_low -= 1;
        }
        Limiter::pump(&mut s);
    }
}

/// A queued request.
pub struct Queued {
    rx: oneshot::Receiver<bool>,
    st: Arc<Mutex<LimState>>,
    promoter: Promoter,
}

/// Promotes a queued request (low → high lane) from anywhere.
#[derive(Clone)]
pub struct Promoter {
    id: u64,
    st: Arc<Mutex<LimState>>,
}

impl Promoter {
    /// move to the high lane (no-op once running)
    pub fn promote(&self) {
        let mut s = self.st.lock();
        if let Some(i) = s.low.iter().position(|w| w.id == self.id) {
            if let Some(w) = s.low.remove(i) {
                s.high.push_back(w);
            }
            Limiter::pump(&mut s);
        }
    }
}

impl Queued {
    /// a promoter for this request
    pub fn promoter(&self) -> Promoter {
        self.promoter.clone()
    }

    /// wait for the slot
    pub async fn wait(self) -> Slot {
        let low = self.rx.await.unwrap_or(false);
        Slot { st: self.st, low }
    }
}

impl Limiter {
    /// `new Limiter(max)` (lowMax = max - 1)
    pub fn new(max: usize) -> Self {
        let max = max.max(1);
        Limiter {
            st: Arc::new(Mutex::new(LimState {
                max,
                low_max: max.saturating_sub(1).max(1),
                running: 0,
                running_low: 0,
                high: VecDeque::new(),
                low: VecDeque::new(),
                next: 0,
            })),
        }
    }

    /// change slot counts at runtime (running work finishes normally)
    pub fn resize(&self, max: usize, low_max: usize) {
        let mut s = self.st.lock();
        s.max = max.max(1);
        s.low_max = low_max.min(s.max).max(1);
        Self::pump(&mut s);
    }

    /// background slots
    pub fn low_max(&self) -> usize {
        self.st.lock().low_max
    }

    /// all slots
    pub fn max(&self) -> usize {
        self.st.lock().max
    }

    /// (running, queued high, queued low)
    pub fn stats(&self) -> (usize, usize, usize) {
        let s = self.st.lock();
        (s.running, s.high.len(), s.low.len())
    }

    /// queue a request (it may already hold its slot when this returns)
    pub fn queue(&self, prio: Prio) -> Queued {
        let (tx, rx) = oneshot::channel();
        let mut s = self.st.lock();
        let id = s.next;
        s.next += 1;
        match prio {
            Prio::High => s.high.push_back(Waiter { id, tx }),
            Prio::Low => s.low.push_back(Waiter { id, tx }),
        }
        Self::pump(&mut s);
        Queued {
            rx,
            st: self.st.clone(),
            promoter: Promoter {
                id,
                st: self.st.clone(),
            },
        }
    }

    /// run `f` in a slot
    pub async fn run<T>(&self, prio: Prio, f: impl std::future::Future<Output = T>) -> T {
        let _slot = self.queue(prio).wait().await;
        f.await
    }

    fn pump(s: &mut LimState) {
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
            if w.tx.send(low).is_ok() {
                s.running += 1;
                if low {
                    s.running_low += 1;
                }
            }
        }
    }
}

// ───────────────────────── ThumbService ─────────────────────────

struct BaseEntry {
    fut: Shared<BoxFuture<'static, BaseResult>>,
    promote: Option<Promoter>,
}

/// The service.
pub struct ThumbService {
    dir: PathBuf,
    ffmpeg: Arc<Ffmpeg>,
    /// decode slots (on-demand + background)
    pub limiter: Limiter,
    /// CPU budget of every background pass
    pub bg: BackgroundPool,
    quality: Mutex<ThumbQuality>,
    inflight: Mutex<HashMap<String, Shared<BoxFuture<'static, FileResult>>>>,
    bases: Mutex<HashMap<String, BaseEntry>>,
    /// set by the device engine (device://)
    pub remote: Mutex<Option<Arc<dyn RemoteImageSource>>>,
    /// set by the remote-folder engine (remote://)
    pub cloud: Mutex<Option<Arc<dyn RemoteImageSource>>>,
    sized: Mutex<Option<(Instant, Shared<BoxFuture<'static, u64>>)>>,
    /// test hook: count of ffmpeg frame grabs
    pub frames: std::sync::atomic::AtomicUsize,
}

fn to_err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl ThumbService {
    /// new service caching under `dir`
    pub fn new(dir: PathBuf, ffmpeg: Arc<Ffmpeg>) -> Arc<Self> {
        let _ = std::fs::create_dir_all(&dir);
        let n = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2);
        Arc::new(ThumbService {
            dir,
            ffmpeg,
            limiter: Limiter::new(n.clamp(2, 6)),
            bg: BackgroundPool::new(2, Duration::ZERO),
            quality: Mutex::new(ThumbQuality::Normal),
            inflight: Mutex::new(HashMap::new()),
            bases: Mutex::new(HashMap::new()),
            remote: Mutex::new(None),
            cloud: Mutex::new(None),
            sized: Mutex::new(None),
            frames: std::sync::atomic::AtomicUsize::new(0),
        })
    }

    /// Settings.thumbQuality
    pub fn quality(&self) -> ThumbQuality {
        *self.quality.lock()
    }

    /// set Settings.thumbQuality
    pub fn set_quality(&self, q: ThumbQuality) {
        *self.quality.lock() = q;
    }

    /// WebP quality for a rendition whose 'normal' quality is `base`
    pub fn q(&self, base: u8) -> u8 {
        self.quality().q(base)
    }

    /// cache root
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// sha1(`abs|mtime|size`)
    pub fn key(&self, abs: &str, row: &ThumbRow) -> String {
        render::cache_key(abs, row.mtime as f64, row.size as f64)
    }

    /// `<dir>/<key[0..2]>/<key>-<variant>`
    pub fn file(&self, key: &str, variant: &str) -> PathBuf {
        render::cache_file(&self.dir, key, variant)
    }

    fn remote_of(&self, abs: &str) -> Result<Option<Arc<dyn RemoteImageSource>>, String> {
        if !is_remote_abs(abs) {
            return Ok(None);
        }
        if abs.starts_with("remote://") {
            return self
                .cloud
                .lock()
                .clone()
                .map(Some)
                .ok_or_else(|| "remote media unavailable".to_string());
        }
        self.remote
            .lock()
            .clone()
            .map(Some)
            .ok_or_else(|| "device media unavailable".to_string())
    }

    fn once(
        self: &Arc<Self>,
        id: String,
        f: BoxFuture<'static, FileResult>,
    ) -> Shared<BoxFuture<'static, FileResult>> {
        let mut m = self.inflight.lock();
        if let Some(p) = m.get(&id) {
            return p.clone();
        }
        let me = self.clone();
        let id2 = id.clone();
        // runs to completion whoever waits for it (like a JS promise)
        let task = tokio::spawn(async move {
            let r = f.await;
            me.inflight.lock().remove(&id2);
            r
        });
        let fut = async move { task.await.unwrap_or_else(|e| Err(e.to_string())) }
            .boxed()
            .shared();
        m.insert(id, fut.clone());
        fut
    }

    /// One video frame as PNG (1600 px wide at most).
    async fn frame(&self, abs: &str, row: &ThumbRow) -> Result<Vec<u8>, String> {
        self.frames
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let at = row
            .duration
            .filter(|d| *d != 0.0)
            .map(|d| (d * 0.1).min(3.0))
            .unwrap_or(1.0);
        self.ffmpeg.frame(abs, at, 1600).await.map_err(to_err)
    }

    /// 640 + 320 (+ 1600 for videos) + thumbhash from one decode; concurrent callers share it,
    /// a 'high' caller promotes a queued background decode.
    pub fn generate_base(
        self: &Arc<Self>,
        abs: &str,
        row: &ThumbRow,
        prio: Prio,
    ) -> Shared<BoxFuture<'static, BaseResult>> {
        let key = self.key(abs, row);
        if prio == Prio::High {
            self.bg.foreground();
        }
        let mut bases = self.bases.lock();
        if let Some(cur) = bases.get(&key) {
            if prio == Prio::High {
                if let Some(p) = &cur.promote {
                    p.promote();
                }
            }
            return cur.fut.clone();
        }
        let me = self.clone();
        let k2 = key.clone();
        let abs_s = abs.to_string();
        let row = row.clone();
        let (fut, promote): (BoxFuture<'static, BaseResult>, Option<Promoter>) = match self
            .remote_of(abs)
        {
            Err(e) => (async move { Err(e) }.boxed(), None),
            Ok(Some(remote)) => {
                let me2 = me.clone();
                let key2 = key.clone();
                (
                    async move {
                        let f640 = me2.file(&key2, "640.webp");
                        let buf = match tokio::fs::read(&f640).await {
                            Ok(b) => b,
                            Err(_) => remote.thumb(&abs_s, 640, prio).await?,
                        };
                        let _slot = me2.limiter.queue(prio).wait().await;
                        let q = me2.quality();
                        let base = tokio::task::spawn_blocking(move || {
                            let img =
                                pa_media::decode_bytes(&buf, pa_media::DecodeOptions::default())?;
                            render::render_base(&img, q)
                        })
                        .await
                        .map_err(to_err)?
                        .map_err(to_err)?;
                        me2.write_base(&key2, &base).await?;
                        Ok(Some(BaseInfo {
                            thumbhash: base.thumbhash,
                            width: base.width,
                            height: base.height,
                        }))
                    }
                    .boxed(),
                    None,
                )
            }
            Ok(None) => {
                let queued = self.limiter.queue(prio);
                let promoter = queued.promoter();
                let me2 = me.clone();
                let key2 = key.clone();
                (
                    async move {
                        let _slot = queued.wait().await;
                        me2.decode_base(&abs_s, &row, &key2).await
                    }
                    .boxed(),
                    Some(promoter),
                )
            }
        };
        let task = tokio::spawn(async move {
            let r = fut.await;
            me.bases.lock().remove(&k2);
            r
        });
        let fut = async move { task.await.unwrap_or_else(|e| Err(e.to_string())) }
            .boxed()
            .shared();
        bases.insert(
            key,
            BaseEntry {
                fut: fut.clone(),
                promote,
            },
        );
        fut
    }

    async fn write_base(&self, key: &str, b: &render::BaseRenditions) -> Result<(), String> {
        // 320 first, 640 last: thumb() treats an existing 640 as "base done"
        render::write_atomic(&self.file(key, "320.webp"), &b.webp320)
            .await
            .map_err(to_err)?;
        render::write_atomic(&self.file(key, "640.webp"), &b.webp640)
            .await
            .map_err(to_err)?;
        Ok(())
    }

    async fn decode_base(&self, abs: &str, row: &ThumbRow, key: &str) -> BaseResult {
        let q = self.quality();
        let base = if row.kind == "video" {
            if !self.ffmpeg.available() {
                return Ok(None);
            }
            let frame = self.frame(abs, row).await?;
            if frame.is_empty() {
                return Ok(None);
            }
            let b = tokio::task::spawn_blocking(move || render::render_base_frame(&frame, q))
                .await
                .map_err(to_err)?
                .map_err(to_err)?;
            if let Some(w) = &b.webp1600 {
                render::write_atomic(&self.file(key, "1600.webp"), w)
                    .await
                    .map_err(to_err)?;
            }
            b
        } else if row.kind == "image" {
            let p = PathBuf::from(abs);
            tokio::task::spawn_blocking(move || {
                let img = render::decode_for_base(&p)?;
                render::render_base(&img, q)
            })
            .await
            .map_err(to_err)?
            .map_err(to_err)?
        } else {
            return Ok(None);
        };
        self.write_base(key, &base).await?;
        Ok(Some(BaseInfo {
            thumbhash: base.thumbhash,
            width: base.width,
            height: base.height,
        }))
    }

    /// A thumbnail file (320 / 640 / 1600 webp), generated when missing.
    pub async fn thumb(
        self: &Arc<Self>,
        abs: &str,
        row: &ThumbRow,
        size: u32,
        prio: Prio,
    ) -> FileResult {
        let key = self.key(abs, row);
        let f = self.file(&key, &format!("{size}.webp"));
        if tokio::fs::metadata(&f).await.is_ok() {
            return Ok(Some(f));
        }
        if prio == Prio::High {
            self.bg.foreground();
        }
        let remote = self.remote_of(abs)?;
        if let (Some(remote), 1600) = (remote.clone(), size) {
            let me = self.clone();
            let abs_s = abs.to_string();
            let f2 = f.clone();
            return self
                .once(
                    f.to_string_lossy().into_owned(),
                    async move {
                        let buf = remote.thumb(&abs_s, 1600, prio).await?;
                        let _slot = me.limiter.queue(prio).wait().await;
                        let q = me.quality();
                        let data = tokio::task::spawn_blocking(move || {
                            let img =
                                pa_media::decode_bytes(&buf, pa_media::DecodeOptions::default())?;
                            render::render_1600(img, q)
                        })
                        .await
                        .map_err(to_err)?
                        .map_err(to_err)?;
                        render::write_atomic(&f2, &data).await.map_err(to_err)?;
                        Ok(Some(f2))
                    }
                    .boxed(),
                )
                .await;
        }
        if size == 1600 && row.kind != "video" && remote.is_none() {
            if row.kind != "image" {
                return Ok(None);
            }
            let me = self.clone();
            let abs_p = PathBuf::from(abs);
            let f2 = f.clone();
            return self
                .once(
                    f.to_string_lossy().into_owned(),
                    async move {
                        let _slot = me.limiter.queue(prio).wait().await;
                        let q = me.quality();
                        let data = tokio::task::spawn_blocking(move || {
                            render::render_1600_file(&abs_p, q)
                        })
                        .await
                        .map_err(to_err)?
                        .map_err(to_err)?;
                        render::write_atomic(&f2, &data).await.map_err(to_err)?;
                        Ok(Some(f2))
                    }
                    .boxed(),
                )
                .await;
        }
        let r = self.generate_base(abs, row, prio).await?;
        Ok(if r.is_some() && tokio::fs::metadata(&f).await.is_ok() {
            Some(f)
        } else {
            None
        })
    }

    /// Browser friendly rendition (full size for HEIC/TIFF/…, or bounded to `max`).
    pub async fn display(
        self: &Arc<Self>,
        abs: &str,
        row: &ThumbRow,
        max: Option<u32>,
    ) -> FileResult {
        let key = self.key(abs, row);
        if let Some(remote) = self.remote_of(abs)? {
            let m = max.unwrap_or(4096);
            let rf = self.file(&key, &format!("display-{m}.jpg"));
            if tokio::fs::metadata(&rf).await.is_ok() {
                return Ok(Some(rf));
            }
            let abs_s = abs.to_string();
            let rf2 = rf.clone();
            return self
                .once(
                    rf.to_string_lossy().into_owned(),
                    async move {
                        let buf = remote.display(&abs_s, m).await?;
                        render::write_atomic(&rf2, &buf).await.map_err(to_err)?;
                        Ok(Some(rf2))
                    }
                    .boxed(),
                )
                .await;
        }
        let f = self.file(&key, &render::display_variant(&row.ext, max));
        if tokio::fs::metadata(&f).await.is_ok() {
            return Ok(Some(f));
        }
        self.bg.foreground();
        let me = self.clone();
        let abs_p = PathBuf::from(abs);
        let ext = row.ext.clone();
        let kind = row.kind.clone();
        let f2 = f.clone();
        self.once(
            f.to_string_lossy().into_owned(),
            async move {
                if kind != "image" {
                    return Ok(None);
                }
                let _slot = me.limiter.queue(Prio::High).wait().await;
                let (data, _) = tokio::task::spawn_blocking(move || {
                    render::render_display_file(&abs_p, &ext, max)
                })
                .await
                .map_err(to_err)?
                .map_err(to_err)?;
                render::write_atomic(&f2, &data).await.map_err(to_err)?;
                Ok(Some(f2))
            }
            .boxed(),
        )
        .await
    }

    /// ~3 s muted looping preview for hover in the grid.
    pub async fn preview(self: &Arc<Self>, abs: &str, row: &ThumbRow) -> FileResult {
        if row.kind != "video" || !self.ffmpeg.available() || is_remote_abs(abs) {
            return Ok(None);
        }
        let key = self.key(abs, row);
        let f = self.file(&key, "preview.mp4");
        if tokio::fs::metadata(&f).await.is_ok() {
            return Ok(Some(f));
        }
        let ff = self.ffmpeg.clone();
        let abs_p = PathBuf::from(abs);
        let dur = row.duration;
        let f2 = f.clone();
        self.once(
            f.to_string_lossy().into_owned(),
            async move {
                pa_media::video::make_preview(&ff, &abs_p, Some(dur.unwrap_or(0.0)), &f2)
                    .await
                    .map_err(to_err)?;
                Ok(Some(f2))
            }
            .boxed(),
        )
        .await
    }

    /// Horizontal sprite of n frames (video editor timeline).
    pub async fn filmstrip(
        self: &Arc<Self>,
        abs: &str,
        row: &ThumbRow,
        n: f64,
        h: f64,
    ) -> FileResult {
        if row.kind != "video" {
            return Ok(None);
        }
        let (n, h) = pa_media::video::filmstrip_dims(n, h);
        let key = self.key(abs, row);
        let f = self.file(&key, &format!("strip-{n}-{h}.jpg"));
        if tokio::fs::metadata(&f).await.is_ok() {
            return Ok(Some(f));
        }
        if let Some(remote) = self.remote_of(abs)? {
            let abs_s = abs.to_string();
            let f2 = f.clone();
            return self
                .once(
                    f.to_string_lossy().into_owned(),
                    async move {
                        let buf = remote.strip(&abs_s, n, h).await?;
                        render::write_atomic(&f2, &buf).await.map_err(to_err)?;
                        Ok(Some(f2))
                    }
                    .boxed(),
                )
                .await;
        }
        if !self.ffmpeg.available() {
            return Ok(None);
        }
        let ff = self.ffmpeg.clone();
        let abs_p = PathBuf::from(abs);
        let dur = row.duration;
        let f2 = f.clone();
        self.once(
            f.to_string_lossy().into_owned(),
            async move {
                match pa_media::video::filmstrip(&ff, &abs_p, dur, n, h)
                    .await
                    .map_err(to_err)?
                {
                    Some(data) => {
                        render::write_atomic(&f2, &data).await.map_err(to_err)?;
                        Ok(Some(f2))
                    }
                    None => Ok(None),
                }
            }
            .boxed(),
        )
        .await
    }

    /// drop every cached rendition
    pub async fn clear(&self) {
        *self.sized.lock() = None;
        let _ = tokio::fs::remove_dir_all(&self.dir).await;
        let _ = tokio::fs::create_dir_all(&self.dir).await;
    }

    /// Bytes on disk (a result is reused for a minute).
    pub async fn size_bytes(&self) -> u64 {
        let fut = {
            let mut s = self.sized.lock();
            match &*s {
                Some((at, f)) if at.elapsed() < Duration::from_secs(60) => f.clone(),
                _ => {
                    let dir = self.dir.clone();
                    let f = async move {
                        tokio::task::spawn_blocking(move || dir_size(&dir))
                            .await
                            .unwrap_or(0)
                    }
                    .boxed()
                    .shared();
                    *s = Some((Instant::now(), f.clone()));
                    f
                }
            }
        };
        fut.await
    }
}

/// total bytes of the files below `d`
pub fn dir_size(d: &Path) -> u64 {
    let mut total = 0;
    if let Ok(rd) = std::fs::read_dir(d) {
        for e in rd.flatten() {
            match e.file_type() {
                Ok(t) if t.is_dir() => total += dir_size(&e.path()),
                Ok(_) => total += e.metadata().map(|m| m.len()).unwrap_or(0),
                Err(_) => {}
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn limiter_lanes() {
        let lim = Limiter::new(2);
        let order = Arc::new(Mutex::new(Vec::<String>::new()));
        let gates: Arc<Mutex<Vec<oneshot::Sender<()>>>> = Arc::new(Mutex::new(Vec::new()));
        let job = |name: String, q: Queued| {
            let order = order.clone();
            let gates = gates.clone();
            tokio::spawn(async move {
                let _s = q.wait().await;
                order.lock().push(name);
                let (tx, rx) = oneshot::channel();
                gates.lock().push(tx);
                let _ = rx.await;
            })
        };
        for i in 0..4 {
            job(format!("low{i}"), lim.queue(Prio::Low));
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(*order.lock(), vec!["low0"]);
        job("high0".into(), lim.queue(Prio::High));
        let b = lim.queue(Prio::Low);
        let p = b.promoter();
        job("low-promoted".into(), b);
        p.promote();
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(*order.lock(), vec!["low0", "high0"]);
        let g = gates.lock().remove(0);
        let _ = g.send(());
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(order.lock()[2], "low-promoted");
    }
}
