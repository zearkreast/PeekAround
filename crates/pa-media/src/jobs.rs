//! Process plumbing of the export jobs (mirrors `jobs/ffmpeg-process.ts`): ffmpeg with
//! `-progress pipe:1`, cancellation (SIGKILL), error messages from the stderr tail, a fed encoder
//! process (slideshow) and raw-frame piping between two ffmpegs. Jobs run at normal priority.

use crate::ffmpeg::Ffmpeg;
use regex::Regex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, thiserror::Error, PartialEq)]
pub enum JobError {
    /// the job was canceled (`AbortError` in TS)
    #[error("Canceled")]
    Aborted,
    #[error("{0}")]
    Failed(String),
}

impl From<std::io::Error> for JobError {
    fn from(e: std::io::Error) -> Self {
        JobError::Failed(e.to_string())
    }
}

/// Incremental parser for `-progress pipe:1` output (key=value lines).
#[derive(Debug, Default)]
pub struct ProgressParser {
    rest: String,
}

impl ProgressParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// The newest output time (seconds) in this chunk, None when it had none.
    pub fn feed(&mut self, chunk: &str) -> Option<f64> {
        let text = std::mem::take(&mut self.rest) + chunk;
        let mut lines: Vec<&str> = text.split('\n').collect();
        self.rest = lines.pop().unwrap_or("").to_string();
        let mut latest = None;
        for raw in lines {
            let line = raw.trim();
            let Some(eq) = line.find('=') else { continue };
            let (key, val) = (&line[..eq], &line[eq + 1..]);
            // out_time_ms is (despite the name) in microseconds as well — an old ffmpeg quirk
            if key == "out_time_us" || key == "out_time_ms" {
                let us = crate::ffmpeg::js_number(val);
                if us.is_finite() && us >= 0.0 {
                    latest = Some(us / 1e6);
                }
            } else if key == "out_time" && latest.is_none() {
                if let Some(t) = parse_clock(val) {
                    latest = Some(t);
                }
            }
        }
        latest
    }
}

/// "00:01:02.500000" → 62.5
pub fn parse_clock(v: &str) -> Option<f64> {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(-?\d+):(\d{2}):(\d{2}(?:\.\d+)?)$").unwrap());
    let c = RE.captures(v.trim())?;
    let t = c[1].parse::<f64>().ok()? * 3600.0
        + c[2].parse::<f64>().ok()? * 60.0
        + c[3].parse::<f64>().ok()?;
    (t.is_finite() && t >= 0.0).then_some(t)
}

const TAIL: usize = 6000;

fn push_tail(buf: &mut String, add: &str) {
    buf.push_str(add);
    if buf.len() > TAIL {
        let cut = crate::process::tail_str(buf, TAIL).len();
        let start = buf.len() - cut;
        buf.drain(..start);
    }
}

/// "ffmpeg failed: <last 4 stderr lines>" (≤ 480 chars)
pub fn ff_error(stderr: &str, code: Option<i32>) -> JobError {
    let lines: Vec<&str> = stderr
        .split('\n')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let msg = if lines.is_empty() {
        format!(
            "ffmpeg exited with {}",
            code.map(|c| c.to_string())
                .unwrap_or_else(|| "SIGKILL".into())
        )
    } else {
        lines[lines.len().saturating_sub(4)..].join(" | ")
    };
    let full = format!("ffmpeg failed: {msg}");
    let mut end = full.len().min(480);
    while !full.is_char_boundary(end) {
        end -= 1;
    }
    JobError::Failed(full[..end].to_string())
}

async fn collect_stderr(mut s: tokio::process::ChildStderr) -> String {
    let mut out = String::new();
    let mut buf = [0u8; 8192];
    while let Ok(n) = s.read(&mut buf).await {
        if n == 0 {
            break;
        }
        push_tail(&mut out, &String::from_utf8_lossy(&buf[..n]));
    }
    out
}

pub struct JobRun<'a> {
    pub cancel: &'a CancellationToken,
    /// expected output duration in seconds (for progress)
    pub duration: Option<f64>,
    /// 0..=1 within this run
    pub on_progress: Option<&'a mut (dyn FnMut(f64) + Send)>,
}

/// Run ffmpeg to completion with `-nostdin -loglevel error -progress pipe:1 -nostats`; killed when
/// `cancel` fires (→ [`JobError::Aborted`]).
pub async fn run_ffmpeg<S: AsRef<str>>(
    ff: &Ffmpeg,
    args: &[S],
    mut run: JobRun<'_>,
) -> Result<(), JobError> {
    if run.cancel.is_cancelled() {
        return Err(JobError::Aborted);
    }
    let mut a: Vec<&str> = vec![
        "-nostdin",
        "-loglevel",
        "error",
        "-progress",
        "pipe:1",
        "-nostats",
    ];
    a.extend(args.iter().map(|s| s.as_ref()));
    let mut child = ff.spawn(&a)?;
    drop(child.stdin.take());
    let mut stdout = child.stdout.take().expect("piped");
    let stderr = tokio::spawn(collect_stderr(child.stderr.take().expect("piped")));
    let mut parser = ProgressParser::new();
    let mut buf = [0u8; 4096];
    let status = loop {
        tokio::select! {
            _ = run.cancel.cancelled() => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(JobError::Aborted);
            }
            r = stdout.read(&mut buf) => match r {
                Ok(0) | Err(_) => {
                    tokio::select! {
                        _ = run.cancel.cancelled() => {
                            let _ = child.start_kill();
                            let _ = child.wait().await;
                            return Err(JobError::Aborted);
                        }
                        st = child.wait() => break st?,
                    }
                }
                Ok(n) => {
                    if let Some(t) = parser.feed(&String::from_utf8_lossy(&buf[..n])) {
                        if let (Some(cb), Some(d)) = (run.on_progress.as_mut(), run.duration) {
                            if d > 0.0 {
                                cb((t / d).min(1.0));
                            }
                        }
                    }
                }
            }
        }
    };
    let err = stderr.await.unwrap_or_default();
    if run.cancel.is_cancelled() {
        Err(JobError::Aborted)
    } else if status.success() {
        Ok(())
    } else {
        Err(ff_error(&err, status.code()))
    }
}

/// A long-running ffmpeg whose stdin the caller feeds (the slideshow encoder).
pub struct FeedProcess {
    pub stdin: Option<ChildStdin>,
    done: watch::Receiver<Option<Result<(), JobError>>>,
    exited: Arc<AtomicBool>,
    kill: CancellationToken,
}

impl FeedProcess {
    /// resolves when the process has exited (Ok on exit code 0)
    pub async fn done(&mut self) -> Result<(), JobError> {
        let r = self.done.wait_for(|v| v.is_some()).await;
        match r {
            Ok(v) => v
                .clone()
                .unwrap_or(Err(JobError::Failed("encoder vanished".into()))),
            Err(_) => Err(JobError::Failed("encoder vanished".into())),
        }
    }
    pub fn exited(&self) -> bool {
        self.exited.load(Ordering::Relaxed)
    }
    pub fn kill(&self) {
        self.kill.cancel();
    }
    /// close stdin (end of input) and wait for the encoder
    pub async fn finish(&mut self) -> Result<(), JobError> {
        if let Some(mut s) = self.stdin.take() {
            let _ = s.shutdown().await;
        }
        self.done().await
    }
}

/// Spawn an ffmpeg (`-loglevel error -nostats` + args) fed through stdin; killed on `cancel`.
pub fn spawn_fed<S: AsRef<str>>(
    ff: &Ffmpeg,
    args: &[S],
    cancel: &CancellationToken,
) -> Result<FeedProcess, JobError> {
    if cancel.is_cancelled() {
        return Err(JobError::Aborted);
    }
    let mut a: Vec<&str> = vec!["-loglevel", "error", "-nostats"];
    a.extend(args.iter().map(|s| s.as_ref()));
    let mut child: Child = ff.spawn(&a)?;
    let stdin = child.stdin.take();
    let mut stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let (tx, rx) = watch::channel(None);
    let exited = Arc::new(AtomicBool::new(false));
    let kill = CancellationToken::new();
    let (ex, k, c) = (exited.clone(), kill.clone(), cancel.clone());
    tokio::spawn(async move {
        let err = tokio::spawn(collect_stderr(stderr));
        let drain = tokio::spawn(async move {
            let mut b = [0u8; 8192];
            while matches!(stdout.read(&mut b).await, Ok(n) if n > 0) {}
        });
        let res = tokio::select! {
            _ = c.cancelled() => { let _ = child.start_kill(); let _ = child.wait().await; Err(JobError::Aborted) }
            _ = k.cancelled() => { let _ = child.start_kill(); let _ = child.wait().await; Err(JobError::Failed("ffmpeg exited with SIGKILL".into())) }
            st = child.wait() => match st {
                Ok(st) if c.is_cancelled() => { let _ = st; Err(JobError::Aborted) }
                Ok(st) if st.success() => Ok(()),
                Ok(st) => Err(ff_error(&err.await.unwrap_or_default(), st.code())),
                Err(e) => Err(JobError::Failed(e.to_string())),
            },
        };
        drain.abort();
        ex.store(true, Ordering::Relaxed);
        let _ = tx.send(Some(res));
    });
    Ok(FeedProcess {
        stdin,
        done: rx,
        exited,
        kill,
    })
}

/// Run an ffmpeg that writes raw frames to stdout and forward exactly `expected_bytes` of them into
/// `sink`'s stdin; missing bytes (a source with fewer frames than planned) are filled from `fill`
/// (one frame), so the encoder's frame count stays exact.
pub async fn pipe_frames<S: AsRef<str>>(
    ff: &Ffmpeg,
    args: &[S],
    sink: &mut FeedProcess,
    expected_bytes: u64,
    fill: &[u8],
    cancel: &CancellationToken,
    on_bytes: &mut (dyn FnMut(u64) + Send),
) -> Result<(), JobError> {
    if cancel.is_cancelled() {
        return Err(JobError::Aborted);
    }
    let mut a: Vec<&str> = vec!["-nostdin", "-loglevel", "error", "-nostats"];
    a.extend(args.iter().map(|s| s.as_ref()));
    let mut child = ff.spawn(&a)?;
    drop(child.stdin.take());
    let mut stdout = child.stdout.take().expect("piped");
    let stderr = tokio::spawn(collect_stderr(child.stderr.take().expect("piped")));
    let mut done_rx = sink.done.clone();
    let Some(si) = sink.stdin.as_mut() else {
        return Err(JobError::Failed("encoder stdin closed".into()));
    };
    let mut written = 0u64;
    let mut buf = vec![0u8; 256 * 1024];
    let encoder_gone = |v: &Option<Result<(), JobError>>| match v {
        Some(Ok(())) => JobError::Failed("encoder exited early".into()),
        Some(Err(e)) => e.clone(),
        None => JobError::Failed("encoder exited early".into()),
    };
    loop {
        let n = tokio::select! {
            _ = cancel.cancelled() => { let _ = child.start_kill(); return Err(JobError::Aborted); }
            r = done_rx.wait_for(|v| v.is_some()) => {
                let e = r.map(|v| encoder_gone(&v)).unwrap_or_else(|_| JobError::Failed("encoder exited early".into()));
                let _ = child.start_kill();
                return Err(e);
            }
            r = stdout.read(&mut buf) => r.unwrap_or(0),
        };
        if n == 0 {
            break;
        }
        let room = expected_bytes.saturating_sub(written) as usize;
        let take = n.min(room);
        if take > 0 {
            tokio::select! {
                _ = cancel.cancelled() => { let _ = child.start_kill(); return Err(JobError::Aborted); }
                r = si.write_all(&buf[..take]) => if let Err(e) = r { let _ = child.start_kill(); return Err(JobError::Failed(e.to_string())); },
            }
            written += take as u64;
            on_bytes(take as u64);
        }
    }
    let status = child.wait().await?;
    if cancel.is_cancelled() {
        return Err(JobError::Aborted);
    }
    if !status.success() {
        return Err(ff_error(&stderr.await.unwrap_or_default(), status.code()));
    }
    if fill.is_empty() && written < expected_bytes {
        return Err(JobError::Failed(
            "short frame source and no fill frame".into(),
        ));
    }
    while written < expected_bytes {
        let off = (written % fill.len() as u64) as usize;
        let n = ((expected_bytes - written) as usize).min(fill.len() - off);
        tokio::select! {
            _ = cancel.cancelled() => return Err(JobError::Aborted),
            r = si.write_all(&fill[off..off + n]) => r.map_err(|e| JobError::Failed(e.to_string()))?,
        }
        written += n as u64;
        on_bytes(n as u64);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_across_chunks() {
        let mut p = ProgressParser::new();
        assert_eq!(p.feed("frame=10\nfps=0.0\nout_time_us=12"), None);
        let t = p
            .feed("50000\nout_time_ms=1250000\nout_time=00:00:01.250000\nprogress=continue\n")
            .unwrap();
        assert!((t - 1.25).abs() < 1e-9);
        assert_eq!(p.feed("out_time_us=N/A\nprogress=continue\n"), None);
        assert!((p.feed("out_time_us=2500000\nprogress=end\n").unwrap() - 2.5).abs() < 1e-9);
    }

    #[test]
    fn out_time_fallback() {
        assert!(
            (ProgressParser::new()
                .feed("out_time=00:01:02.500000\n")
                .unwrap()
                - 62.5)
                .abs()
                < 1e-9
        );
        assert_eq!(parse_clock("01:00:00.0"), Some(3600.0));
        assert_eq!(parse_clock("N/A"), None);
    }

    #[test]
    fn error_message() {
        let e = ff_error("a\n\nb\nc\nd\ne\n", Some(1));
        assert_eq!(e, JobError::Failed("ffmpeg failed: b | c | d | e".into()));
        assert_eq!(
            ff_error("", None),
            JobError::Failed("ffmpeg failed: ffmpeg exited with SIGKILL".into())
        );
    }
}
