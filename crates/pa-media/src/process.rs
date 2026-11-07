//! Child process plumbing for ffmpeg / ffprobe: no console windows on Windows, killed when dropped,
//! optional lower priority (Windows BELOW_NORMAL, elsewhere nice 10 — like `os.setPriority(pid,
//! PRIORITY_BELOW_NORMAL)` in the TypeScript server), timeouts, bounded stderr capture.

use crate::error::{MediaError, Result};
use std::ffi::OsStr;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
#[cfg(windows)]
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

/// Priority of a spawned helper process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    /// probes, thumbnail frames, previews: below normal, they never take the CPU from the UI
    Low,
    /// export jobs (video edit, slideshow) keep the normal priority
    Normal,
}

/// A `Command` with the platform defaults every helper process gets (no window, kill on drop).
pub fn command(bin: impl AsRef<OsStr>, prio: Priority) -> Command {
    let mut c = Command::new(bin);
    c.kill_on_drop(true);
    #[cfg(windows)]
    {
        let flags = CREATE_NO_WINDOW
            | if prio == Priority::Low {
                BELOW_NORMAL_PRIORITY_CLASS
            } else {
                0
            };
        c.creation_flags(flags);
    }
    #[cfg(not(windows))]
    let _ = prio;
    c
}

/// Lower the priority of a running child (Unix: nice 10). On Windows the priority class is set at
/// spawn time by [`command`]. Errors (already exited, not permitted) are ignored.
pub fn lower_priority(child: &Child) {
    #[cfg(unix)]
    if let Some(pid) = child
        .id()
        .and_then(|p| rustix::process::Pid::from_raw(p as i32))
    {
        let _ = rustix::process::setpriority_process(Some(pid), 10);
    }
    #[cfg(not(unix))]
    let _ = child;
}

/// Spawn with the given stdio and priority applied.
pub fn spawn(mut cmd: Command, prio: Priority) -> std::io::Result<Child> {
    let child = cmd.spawn()?;
    if prio == Priority::Low {
        lower_priority(&child);
    }
    Ok(child)
}

/// `bin args` exits with 0 within 10 s (a hanging binary never blocks start-up).
pub async fn can_run(bin: impl AsRef<OsStr>, args: &[&str]) -> bool {
    let mut c = command(bin, Priority::Normal);
    c.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let Ok(mut child) = c.spawn() else {
        return false;
    };
    match tokio::time::timeout(Duration::from_secs(10), child.wait()).await {
        Ok(Ok(st)) => st.success(),
        _ => {
            let _ = child.start_kill();
            false
        }
    }
}

/// Run to completion and collect stdout; non-zero exit → error with the stderr tail (TS `runCollect`).
pub async fn run_collect<S: AsRef<OsStr>>(
    bin: impl AsRef<OsStr>,
    args: &[S],
    timeout: Option<Duration>,
    input: Option<Vec<u8>>,
    prio: Priority,
) -> Result<Vec<u8>> {
    let bin_name = bin.as_ref().to_string_lossy().into_owned();
    let mut c = command(bin, prio);
    c.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = spawn(c, prio)?;
    let mut stdin = child.stdin.take();
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let work = async {
        let feed = async {
            if let Some(mut si) = stdin.take() {
                if let Some(data) = input {
                    let _ = si.write_all(&data).await;
                }
                let _ = si.shutdown().await;
            }
        };
        let mut out = Vec::new();
        let read_out = stdout.read_to_end(&mut out);
        let read_err = read_tail(&mut stderr, 4000);
        let (_, r, err) = tokio::join!(feed, read_out, read_err);
        r?;
        let status = child.wait().await?;
        Ok::<_, std::io::Error>((status, out, err))
    };
    let res = match timeout {
        Some(t) => match tokio::time::timeout(t, work).await {
            Ok(r) => r,
            Err(_) => return Err(MediaError::Timeout(bin_name)), // child is killed on drop
        },
        None => work.await,
    };
    let (status, out, err) = res?;
    if status.success() {
        Ok(out)
    } else {
        let code = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "null".into());
        Err(MediaError::Ffmpeg(format!(
            "{bin_name} exited with {code}: {}",
            tail_str(err.trim(), 800)
        )))
    }
}

/// stderr of a process whatever its exit code (`ffmpeg -i` without output exits 1); killed after
/// `timeout` (what was printed until then is returned).
pub async fn run_stderr<S: AsRef<OsStr>>(
    bin: impl AsRef<OsStr>,
    args: &[S],
    timeout: Duration,
) -> Result<String> {
    let mut c = command(bin, Priority::Low);
    c.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = spawn(c, Priority::Low)?;
    let mut stderr = child.stderr.take().expect("piped");
    let mut buf = Vec::new();
    let read = async {
        let mut chunk = [0u8; 16 * 1024];
        loop {
            match stderr.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if buf.len() < 256 * 1024 {
                        buf.extend_from_slice(&chunk[..n]);
                    }
                }
            }
        }
    };
    if tokio::time::timeout(timeout, read).await.is_err() {
        let _ = child.start_kill();
    }
    let _ = child.wait().await;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Read a stream to its end, keeping at most `max` bytes (the first ones, like the TS code).
async fn read_tail<R: tokio::io::AsyncRead + Unpin>(r: &mut R, max: usize) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match r.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if buf.len() < max {
                    buf.extend_from_slice(&chunk[..n]);
                }
            }
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// the last `max` bytes of `s` (cut at a character boundary)
pub(crate) fn tail_str(s: &str, max: usize) -> &str {
    let mut i = s.len().saturating_sub(max);
    while !s.is_char_boundary(i) {
        i += 1;
    }
    &s[i..]
}
