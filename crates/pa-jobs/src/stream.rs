//! Live H.264 transcode for videos the browser can't play (`GET /api/media/:id/stream?t=`, mirrors
//! the `/stream` route of routes/media.ts): ffmpeg writes a fragmented MP4
//! (`frag_keyframe+empty_moov+default_base_moof`) to stdout, which is streamed as the response body
//! (`Content-Type: video/mp4`, `Cache-Control: no-store`). The process is SIGKILLed when the body is
//! dropped (client went away). Device / MEGA videos are read by ffmpeg over the internal loopback
//! URL ([`internal_input`]) so seeks become ranged `/file` requests.

use bytes::Bytes;
use futures_util::Stream;
use pa_media::ffmpeg::Ffmpeg;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::AsyncRead;
use tokio::process::{Child, ChildStdout};

/// The header that lets ffmpeg read the server's own `/file` endpoint (and see every folder).
pub const INTERNAL_HEADER: &str = "X-Peek-Internal";

/// `Settings.hwAccel` → `-hwaccel auto` (GPU decode, software fallback)
pub fn hw_accel_args(hw_accel: bool) -> Vec<String> {
    if hw_accel {
        vec!["-hwaccel".into(), "auto".into()]
    } else {
        Vec::new()
    }
}

/// ffmpeg input args reading media `id` through the server's own ranged `/file` endpoint
/// (`deviceFfmpegInput` / `RemoteService.ffmpegInput`): `-headers 'X-Peek-Internal: <token>\r\n' -i <url>`.
pub fn internal_input(
    self_url: &str,
    internal_token: &str,
    media_id: impl std::fmt::Display,
) -> Vec<String> {
    vec![
        "-headers".into(),
        internal_headers(internal_token),
        "-i".into(),
        internal_url(self_url, media_id),
    ]
}

/// `X-Peek-Internal: <token>\r\n` (ffmpeg `-headers` / `ProbeOptions.headers`)
pub fn internal_headers(internal_token: &str) -> String {
    format!("{INTERNAL_HEADER}: {internal_token}\r\n")
}

/// `<selfUrl>/api/media/<id>/file`
pub fn internal_url(self_url: &str, media_id: impl std::fmt::Display) -> String {
    format!("{self_url}/api/media/{media_id}/file")
}

/// Input of a local file.
pub fn file_input(abs: &str) -> Vec<String> {
    vec!["-i".into(), abs.into()]
}

/// The transcode arguments (after ffmpeg's own `-hide_banner`), identical to the TS route.
/// `t` = start second (`?t=`, ≤ 0 → from the start).
pub fn transcode_args(input: &[String], t: f64, hw_accel: bool) -> Vec<String> {
    let mut a: Vec<String> = vec!["-loglevel".into(), "error".into()];
    if t > 0.0 {
        a.push("-ss".into());
        a.push(crate::js::to_fixed(t, 2));
    }
    a.extend(hw_accel_args(hw_accel));
    a.extend(input.iter().cloned());
    for s in [
        "-map",
        "0:v:0",
        "-map",
        "0:a:0?",
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-crf",
        "23",
        "-pix_fmt",
        "yuv420p",
        "-vf",
        "scale='min(1920,iw)':-2",
        "-c:a",
        "aac",
        "-b:a",
        "160k",
        "-ac",
        "2",
        "-movflags",
        "frag_keyframe+empty_moov+default_base_moof",
        "-f",
        "mp4",
        "pipe:1",
    ] {
        a.push(s.into());
    }
    a
}

/// `?t=` → seconds (`Math.max(0, Number(t) || 0)`)
pub fn parse_t(q: Option<&str>) -> f64 {
    let n = q.map(pa_core::js::number).unwrap_or(0.0);
    if n.is_nan() {
        0.0
    } else {
        n.max(0.0)
    }
}

/// A running transcode: its stdout as a byte stream. Dropping it kills ffmpeg.
pub struct TranscodeStream {
    child: Child,
    stdout: ChildStdout,
    buf: Box<[u8]>,
}

/// Response headers of the stream route.
pub const STREAM_HEADERS: [(&str, &str); 2] =
    [("Content-Type", "video/mp4"), ("Cache-Control", "no-store")];

/// Start a transcode (`ffmpeg -hide_banner <args>`). stderr is drained in the background.
pub fn spawn_transcode(ff: &Ffmpeg, args: &[String]) -> std::io::Result<TranscodeStream> {
    let mut child = ff.spawn(args)?;
    drop(child.stdin.take());
    let stdout = child.stdout.take().expect("piped");
    if let Some(mut err) = child.stderr.take() {
        tokio::spawn(async move {
            let _ = tokio::io::copy(&mut err, &mut tokio::io::sink()).await;
        });
    }
    Ok(TranscodeStream {
        child,
        stdout,
        buf: vec![0u8; 64 * 1024].into_boxed_slice(),
    })
}

impl TranscodeStream {
    /// pid of the ffmpeg process (tests)
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }
}

impl Stream for TranscodeStream {
    type Item = std::io::Result<Bytes>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let me = self.get_mut();
        let mut rb = tokio::io::ReadBuf::new(&mut me.buf);
        match Pin::new(&mut me.stdout).poll_read(cx, &mut rb) {
            Poll::Ready(Ok(())) => {
                let n = rb.filled().len();
                if n == 0 {
                    Poll::Ready(None)
                } else {
                    Poll::Ready(Some(Ok(Bytes::copy_from_slice(rb.filled()))))
                }
            }
            Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for TranscodeStream {
    fn drop(&mut self) {
        // client gone / finished: SIGKILL like the TS route (`proc.kill('SIGKILL')`)
        let _ = self.child.start_kill();
    }
}
