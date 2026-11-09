//! Video / audio playback for the viewer: the bundled **ffmpeg executable** decodes, we present.
//!
//! - video: `ffmpeg -ss T -i SRC -vf scale=W:H,fps=F -pix_fmt rgba -f rawvideo -` → fixed-size
//!   RGBA frames at display size over a pipe (frame k has media time `T + k/F`);
//! - audio: a second `ffmpeg -ss T -i SRC -af atempo=S -f f32le -ac 2 -ar R -` → PCM played with
//!   `cpal` (WASAPI / ALSA / CoreAudio / AAudio); without an output device (servers, xvfb) a
//!   null sink drains the samples in real time so the clock still runs;
//! - A/V sync on the audio clock (samples played → media time), the wall clock when the file
//!   has no audio; seeking / speed changes restart both processes at the new position.
//!
//! Why the executable rather than linking libav (`ffmpeg-next`): ffmpeg is already shipped for the
//! server's thumbnails / transcodes, nothing extra to cross-compile or license-check for Windows
//! and Android (a libav link needs per-target prebuilt libraries and a matching ffmpeg-sys), and
//! every codec / container ffmpeg knows plays, local files and server URLs alike (ranged HTTP).

use std::collections::VecDeque;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use slint::{Rgba8Pixel, SharedPixelBuffer};

static FFMPEG: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
static PRESENTED: AtomicU64 = AtomicU64::new(0);

static INTERVALS: Mutex<Vec<f32>> = Mutex::new(Vec::new());
static LAST_PRESENT: Mutex<Option<Instant>> = Mutex::new(None);

/// A frame was put on screen (perf counters: count + intervals between presented frames).
pub fn count_presented() {
    PRESENTED.fetch_add(1, Ordering::Relaxed);
    let now = Instant::now();
    let mut last = LAST_PRESENT.lock().unwrap();
    if let Some(l) = *last {
        let mut v = INTERVALS.lock().unwrap();
        if v.len() < 100_000 {
            v.push(now.duration_since(l).as_secs_f32() * 1000.0);
        }
    }
    *last = Some(now);
}

/// Presented-frame intervals since the last call (ms) — and reset.
pub fn take_intervals() -> Vec<f32> {
    *LAST_PRESENT.lock().unwrap() = None;
    std::mem::take(&mut *INTERVALS.lock().unwrap())
}

/// Frames presented since start.
pub fn presented() -> u64 {
    PRESENTED.load(Ordering::Relaxed)
}

/// Use this ffmpeg executable (the desktop app passes its bundled one).
pub fn set_ffmpeg_path(p: PathBuf) {
    *FFMPEG.get_or_init(Default::default).lock().unwrap() = Some(p);
}

/// The ffmpeg executable: `set_ffmpeg_path`, `$PA_FFMPEG`, next to the executable, else `ffmpeg`
/// on the PATH.
pub fn ffmpeg_path() -> PathBuf {
    if let Some(p) = FFMPEG.get_or_init(Default::default).lock().unwrap().clone() {
        return p;
    }
    if let Some(p) = std::env::var_os("PA_FFMPEG").filter(|p| !p.is_empty()) {
        return p.into();
    }
    let exe = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.to_path_buf()))
    {
        for c in [
            dir.join(exe),
            dir.join("ffmpeg").join(exe),
            dir.join("resources").join(exe),
        ] {
            if c.is_file() {
                return c;
            }
        }
    }
    exe.into()
}

fn command() -> Command {
    let mut c = Command::new(ffmpeg_path());
    c.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashing up
        c.creation_flags(0x0800_0000);
    }
    c
}

/// What to play.
#[derive(Debug, Clone)]
pub struct Source {
    /// ffmpeg input (path or URL)
    pub input: String,
    /// has a picture (video) — false for audio files
    pub video: bool,
    /// output frame size (display size, even numbers)
    pub width: u32,
    /// output frame height
    pub height: u32,
    /// output frame rate (source fps capped at 60)
    pub fps: f64,
    /// duration (s) when known
    pub duration: Option<f64>,
    /// whether the file has an audio stream (None: unknown → try)
    pub audio: Option<bool>,
    /// ffmpeg filters before the scale to `width`×`height` (video editor: crop, rotate, flip)
    pub vf_pre: Vec<String>,
    /// filters after it (video editor: colour, at display size)
    pub vf_post: Vec<String>,
}

/// A decoded frame.
pub struct Frame {
    /// media time (s)
    pub pts: f64,
    /// pixels
    pub px: SharedPixelBuffer<Rgba8Pixel>,
}

/// Player status for the UI.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Status {
    /// media position (s)
    pub position: f64,
    /// playing
    pub playing: bool,
    /// waiting for data
    pub buffering: bool,
    /// reached the end
    pub ended: bool,
    /// an error (ffmpeg missing / unreadable file)
    pub error: Option<String>,
    /// audio plays through the null sink (no output device)
    pub no_device: bool,
    /// furthest decoded media time (buffered bar)
    pub buffered: f64,
}

const RATE: u32 = 48_000;
const QUEUE: usize = 4;

/// shared between the decoder threads, the audio callback and the UI
struct Shared {
    /// bumps on every (re)start; data of older generations is dropped
    gen: AtomicU64,
    playing: AtomicBool,
    /// f32 bits
    volume: AtomicU32,
    muted: AtomicBool,
    frames: Mutex<VecDeque<(u64, Frame)>>,
    frames_cv: Condvar,
    video_eof: AtomicBool,
    /// interleaved stereo samples of the current generation
    pcm: Mutex<VecDeque<f32>>,
    pcm_cv: Condvar,
    audio_eof: AtomicBool,
    /// stereo frames played since the last start
    played: AtomicU64,
    /// the audio process produced data in this generation
    audio_seen: AtomicBool,
    no_device: AtomicBool,
    error: Mutex<Option<String>>,
    decoded_to: Mutex<f64>,
}

impl Shared {
    fn vol(&self) -> f32 {
        if self.muted.load(Ordering::Relaxed) {
            0.0
        } else {
            f32::from_bits(self.volume.load(Ordering::Relaxed))
        }
    }
}

/// One playing item. Dropping it kills the decoders.
pub struct Player {
    src: Source,
    sh: Arc<Shared>,
    procs: Vec<Child>,
    /// media time of the current generation's start
    t0: f64,
    speed: f64,
    /// wall clock (no audio): accumulated media time while playing + the instant it resumed
    wall_acc: f64,
    wall_since: Option<Instant>,
    /// the audio stream (kept across restarts)
    _out: Option<AudioOut>,
    last_pts: f64,
    started_at: Instant,
    /// waiting for the first audio samples after a (re)start: the clock holds at `t0`
    holding: bool,
}

/// The audio output (cpal stream or the null sink thread).
struct AudioOut {
    #[cfg(feature = "audio")]
    _stream: Option<cpal::Stream>,
    _null: Option<std::thread::JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}

impl Drop for AudioOut {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn mix_into(sh: &Shared, out: &mut [f32]) {
    let playing = sh.playing.load(Ordering::Relaxed);
    let vol = sh.vol();
    let mut q = sh.pcm.lock().unwrap();
    if !playing || q.is_empty() {
        out.iter_mut().for_each(|s| *s = 0.0);
        return;
    }
    let n = out.len().min(q.len());
    for (o, s) in out.iter_mut().zip(q.drain(..n)) {
        *o = s * vol;
    }
    out[n..].iter_mut().for_each(|s| *s = 0.0);
    drop(q);
    sh.pcm_cv.notify_all();
    sh.played.fetch_add((n / 2) as u64, Ordering::Relaxed);
}

#[cfg(feature = "audio")]
fn open_device(sh: &Arc<Shared>) -> Option<cpal::Stream> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let dev = host.default_output_device()?;
    let cfg = cpal::StreamConfig {
        channels: 2,
        sample_rate: cpal::SampleRate(RATE),
        buffer_size: cpal::BufferSize::Default,
    };
    let err = |e| log::warn!("audio stream: {e}");
    let sh2 = sh.clone();
    // f32 first, then i16 (some ALSA devices)
    let stream = dev
        .build_output_stream(
            &cfg,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| mix_into(&sh2, data),
            err,
            None,
        )
        .or_else(|_| {
            let sh3 = sh.clone();
            let mut tmp: Vec<f32> = Vec::new();
            dev.build_output_stream(
                &cfg,
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    tmp.resize(data.len(), 0.0);
                    mix_into(&sh3, &mut tmp);
                    for (d, s) in data.iter_mut().zip(&tmp) {
                        *d = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
                    }
                },
                err,
                None,
            )
        })
        .map_err(|e| log::warn!("no audio output: {e}"))
        .ok()?;
    stream
        .play()
        .map_err(|e| log::warn!("audio play: {e}"))
        .ok()?;
    Some(stream)
}

fn open_audio(sh: &Arc<Shared>) -> AudioOut {
    let stop = Arc::new(AtomicBool::new(false));
    #[cfg(feature = "audio")]
    {
        if std::env::var_os("PA_NO_AUDIO").is_none() {
            if let Some(s) = open_device(sh) {
                return AudioOut {
                    _stream: Some(s),
                    _null: None,
                    stop,
                };
            }
        }
    }
    // null sink: consume in real time (10 ms chunks) so the audio clock advances
    sh.no_device.store(true, Ordering::Relaxed);
    let (sh2, st2) = (sh.clone(), stop.clone());
    let t = std::thread::Builder::new()
        .name("pa-null-audio".into())
        .spawn(move || {
            let mut buf = vec![0f32; (RATE as usize / 100) * 2];
            let start = Instant::now();
            let mut done = 0u64;
            while !st2.load(Ordering::Relaxed) {
                let due = (start.elapsed().as_secs_f64() * RATE as f64) as u64;
                while done + (buf.len() / 2) as u64 <= due {
                    mix_into(&sh2, &mut buf);
                    done += (buf.len() / 2) as u64;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        })
        .ok();
    AudioOut {
        #[cfg(feature = "audio")]
        _stream: None,
        _null: t,
        stop,
    }
}

impl Player {
    /// Start playing `src` at `start` seconds (`playing`: start right away or paused on the
    /// first frame).
    pub fn new(
        src: Source,
        start: f64,
        playing: bool,
        volume: f64,
        muted: bool,
        speed: f64,
    ) -> Player {
        let sh = Arc::new(Shared {
            gen: AtomicU64::new(0),
            playing: AtomicBool::new(playing),
            volume: AtomicU32::new((volume as f32).to_bits()),
            muted: AtomicBool::new(muted),
            frames: Mutex::new(VecDeque::new()),
            frames_cv: Condvar::new(),
            video_eof: AtomicBool::new(false),
            pcm: Mutex::new(VecDeque::new()),
            pcm_cv: Condvar::new(),
            audio_eof: AtomicBool::new(false),
            played: AtomicU64::new(0),
            audio_seen: AtomicBool::new(false),
            no_device: AtomicBool::new(false),
            error: Mutex::new(None),
            decoded_to: Mutex::new(0.0),
        });
        let out = (src.audio != Some(false)).then(|| open_audio(&sh));
        let mut p = Player {
            src,
            sh,
            procs: vec![],
            t0: start,
            speed: speed.clamp(0.25, 4.0),
            wall_acc: 0.0,
            wall_since: None,
            _out: out,
            last_pts: start,
            started_at: Instant::now(),
            holding: false,
        };
        p.restart(start);
        p
    }

    fn kill(&mut self) {
        for mut c in self.procs.drain(..) {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// (Re)start the decoders at media time `t`.
    fn restart(&mut self, t: f64) {
        self.kill();
        let gen = self.sh.gen.fetch_add(1, Ordering::SeqCst) + 1;
        self.t0 = t.max(0.0);
        self.last_pts = self.t0;
        self.wall_acc = 0.0;
        self.wall_since = self.sh.playing.load(Ordering::Relaxed).then(Instant::now);
        self.started_at = Instant::now();
        self.holding = self._out.is_some();
        self.sh.frames.lock().unwrap().clear();
        self.sh.frames_cv.notify_all();
        self.sh.pcm.lock().unwrap().clear();
        self.sh.pcm_cv.notify_all();
        self.sh.played.store(0, Ordering::SeqCst);
        self.sh.video_eof.store(!self.src.video, Ordering::SeqCst);
        self.sh
            .audio_eof
            .store(self._out.is_none(), Ordering::SeqCst);
        self.sh.audio_seen.store(false, Ordering::SeqCst);
        *self.sh.decoded_to.lock().unwrap() = self.t0;
        let ss = format!("{:.3}", self.t0);
        if self.src.video {
            let (w, h) = (self.src.width.max(2), self.src.height.max(2));
            let mut vf = self.src.vf_pre.clone();
            vf.push(format!("scale={w}:{h}:flags=bilinear"));
            vf.extend(self.src.vf_post.iter().cloned());
            vf.push(format!("fps={:.3}", self.src.fps));
            let vf = vf.join(",");
            let mut c = command();
            c.args(["-nostdin", "-hide_banner", "-loglevel", "error"]);
            if self.t0 > 0.0 {
                c.args(["-ss", &ss]);
            }
            c.args([
                "-i",
                &self.src.input,
                "-map",
                "0:v:0",
                "-an",
                "-sn",
                "-vf",
                &vf,
            ])
            .args(["-pix_fmt", "rgba", "-f", "rawvideo", "-"]);
            match c.spawn() {
                Ok(mut child) => {
                    let out = child.stdout.take();
                    let err = child.stderr.take();
                    let sh = self.sh.clone();
                    let (t0, fps) = (self.t0, self.src.fps);
                    std::thread::Builder::new()
                        .name("pa-video".into())
                        .spawn(move || video_reader(sh, gen, out, err, w, h, t0, fps))
                        .ok();
                    self.procs.push(child);
                }
                Err(e) => {
                    *self.sh.error.lock().unwrap() = Some(format!("ffmpeg: {e}"));
                    self.sh.video_eof.store(true, Ordering::SeqCst);
                }
            }
        }
        if self._out.is_some() {
            let mut af = Vec::new();
            // atempo takes 0.5..2 per instance: chain for 0.25
            let mut s = self.speed;
            while s < 0.5 {
                af.push("atempo=0.5".to_string());
                s /= 0.5;
            }
            while s > 2.0 {
                af.push("atempo=2.0".to_string());
                s /= 2.0;
            }
            if (s - 1.0).abs() > 1e-3 {
                af.push(format!("atempo={s:.4}"));
            }
            let mut c = command();
            c.args(["-nostdin", "-hide_banner", "-loglevel", "error"]);
            if self.t0 > 0.0 {
                c.args(["-ss", &ss]);
            }
            c.args(["-i", &self.src.input, "-map", "0:a:0?", "-vn", "-sn"]);
            if !af.is_empty() {
                c.args(["-af", &af.join(",")]);
            }
            c.args(["-ac", "2", "-ar", &RATE.to_string(), "-f", "f32le", "-"]);
            match c.spawn() {
                Ok(mut child) => {
                    let out = child.stdout.take();
                    let sh = self.sh.clone();
                    std::thread::Builder::new()
                        .name("pa-audio".into())
                        .spawn(move || audio_reader(sh, gen, out))
                        .ok();
                    self.procs.push(child);
                }
                Err(e) => {
                    log::warn!("ffmpeg audio: {e}");
                    self.sh.audio_eof.store(true, Ordering::SeqCst);
                }
            }
        }
    }

    /// Is the audio clock in charge (audio data seen in this generation)?
    fn audio_clock(&self) -> bool {
        self._out.is_some() && self.sh.audio_seen.load(Ordering::Relaxed)
    }

    /// Current media time.
    pub fn clock(&self) -> f64 {
        if self.holding {
            return self.t0;
        }
        if self.audio_clock() {
            self.t0 + self.sh.played.load(Ordering::Relaxed) as f64 / RATE as f64 * self.speed
        } else {
            let run = self
                .wall_since
                .map(|s| s.elapsed().as_secs_f64())
                .unwrap_or(0.0);
            self.t0 + (self.wall_acc + run) * self.speed
        }
    }

    /// The frame to show now (drops the late ones), if a newer one is due.
    pub fn take_frame(&mut self) -> Option<Frame> {
        let gen = self.sh.gen.load(Ordering::SeqCst);
        let playing = self.sh.playing.load(Ordering::Relaxed);
        // after a (re)start the clock holds until audio arrives (no audio track / device
        // trouble: the wall clock takes over after at most 1.5 s)
        if self.holding
            && (self.sh.audio_seen.load(Ordering::Relaxed)
                || self.sh.audio_eof.load(Ordering::Relaxed)
                || self.started_at.elapsed() > Duration::from_millis(1500))
        {
            self.holding = false;
            self.wall_acc = 0.0;
            self.wall_since = playing.then(Instant::now);
        }
        let now = self.clock();
        let mut q = self.sh.frames.lock().unwrap();
        q.retain(|(g, _)| *g == gen);
        let mut best = None;
        while let Some((_, f)) = q.front() {
            // the first frame after a (re)start is shown at once (paused seek / poster)
            let first = self.last_pts <= self.t0 + 1e-6 && best.is_none();
            if f.pts <= now + 0.004 || (first && !playing) || (first && f.pts <= self.t0 + 0.05) {
                best = q.pop_front().map(|(_, f)| f);
                if !playing {
                    break;
                }
            } else {
                break;
            }
        }
        drop(q);
        if best.is_some() {
            self.sh.frames_cv.notify_all();
        }
        if let Some(f) = &best {
            self.last_pts = f.pts.max(self.last_pts + 1e-6);
        }
        best
    }

    /// Status for the UI.
    pub fn status(&self) -> Status {
        let vq = self.sh.frames.lock().unwrap().len();
        let veof = self.sh.video_eof.load(Ordering::Relaxed);
        let aeof = self.sh.audio_eof.load(Ordering::Relaxed);
        let pcm = self.sh.pcm.lock().unwrap().len();
        let pos = self.clock().min(self.src.duration.unwrap_or(f64::MAX));
        let ended = veof
            && vq == 0
            && (aeof && pcm == 0 || !self.audio_clock())
            && (self.src.video || aeof && pcm == 0);
        let playing = self.sh.playing.load(Ordering::Relaxed);
        let buffering = playing
            && !ended
            && ((self.src.video && vq == 0 && !veof) || (self.audio_clock() && pcm == 0 && !aeof));
        Status {
            position: if ended {
                self.src.duration.unwrap_or(pos)
            } else {
                pos
            },
            playing,
            buffering,
            ended,
            error: self.sh.error.lock().unwrap().clone(),
            no_device: self.sh.no_device.load(Ordering::Relaxed),
            buffered: *self.sh.decoded_to.lock().unwrap(),
        }
    }

    /// play / pause
    pub fn set_playing(&mut self, on: bool) {
        let was = self.sh.playing.swap(on, Ordering::SeqCst);
        if was == on {
            return;
        }
        if on {
            self.wall_since = Some(Instant::now());
        } else if let Some(s) = self.wall_since.take() {
            self.wall_acc += s.elapsed().as_secs_f64();
        }
    }

    /// Seek (restarts the decoders).
    pub fn seek(&mut self, t: f64) {
        let t = match self.src.duration {
            Some(d) => t.clamp(0.0, (d - 0.05).max(0.0)),
            None => t.max(0.0),
        };
        self.restart(t);
    }

    /// Playback speed (restarts at the current position).
    pub fn set_speed(&mut self, s: f64) {
        let s = s.clamp(0.25, 4.0);
        if (s - self.speed).abs() < 1e-6 {
            return;
        }
        let at = self.clock();
        self.speed = s;
        self.restart(at);
    }

    /// volume 0..1
    pub fn set_volume(&self, v: f64) {
        self.sh
            .volume
            .store((v.clamp(0.0, 1.0) as f32).to_bits(), Ordering::Relaxed);
    }

    /// mute
    pub fn set_muted(&self, m: bool) {
        self.sh.muted.store(m, Ordering::Relaxed);
    }

    /// New frame size / filters (restarts the video at the current position).
    pub fn set_filters(&mut self, width: u32, height: u32, vf_pre: Vec<String>, vf_post: Vec<String>) {
        if (width, height) == (self.src.width, self.src.height) && vf_pre == self.src.vf_pre && vf_post == self.src.vf_post {
            return;
        }
        let at = self.clock();
        self.src.width = width;
        self.src.height = height;
        self.src.vf_pre = vf_pre;
        self.src.vf_post = vf_post;
        self.restart(at);
    }

    /// what is playing
    pub fn source(&self) -> &Source {
        &self.src
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.sh.gen.fetch_add(1, Ordering::SeqCst);
        self.sh.frames_cv.notify_all();
        self.sh.pcm_cv.notify_all();
        self.kill();
    }
}

#[allow(clippy::too_many_arguments)]
fn video_reader(
    sh: Arc<Shared>,
    gen: u64,
    out: Option<std::process::ChildStdout>,
    err: Option<std::process::ChildStderr>,
    w: u32,
    h: u32,
    t0: f64,
    fps: f64,
) {
    // stderr in the background (error messages)
    let errbuf = Arc::new(Mutex::new(String::new()));
    if let Some(mut e) = err {
        let eb = errbuf.clone();
        std::thread::spawn(move || {
            let mut s = String::new();
            let _ = e.read_to_string(&mut s);
            *eb.lock().unwrap() = s;
        });
    }
    let Some(mut out) = out else { return };
    let n = (w * h * 4) as usize;
    let mut k = 0u64;
    loop {
        if sh.gen.load(Ordering::SeqCst) != gen {
            return;
        }
        let mut px = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
        let buf = px.make_mut_bytes();
        debug_assert_eq!(buf.len(), n);
        if out.read_exact(buf).is_err() {
            break;
        }
        let pts = t0 + k as f64 / fps;
        k += 1;
        *sh.decoded_to.lock().unwrap() = pts;
        let mut q = sh.frames.lock().unwrap();
        while q.len() >= QUEUE && sh.gen.load(Ordering::SeqCst) == gen {
            q = sh
                .frames_cv
                .wait_timeout(q, Duration::from_millis(50))
                .unwrap()
                .0;
        }
        if sh.gen.load(Ordering::SeqCst) != gen {
            return;
        }
        q.push_back((gen, Frame { pts, px }));
    }
    if sh.gen.load(Ordering::SeqCst) == gen {
        if k == 0 {
            std::thread::sleep(Duration::from_millis(100));
            let e = errbuf.lock().unwrap().trim().to_string();
            let msg = if e.is_empty() {
                "no video frames".to_string()
            } else {
                e.lines().last().unwrap_or("").to_string()
            };
            log::warn!("video decode: {msg}");
            *sh.error.lock().unwrap() = Some(msg);
        }
        sh.video_eof.store(true, Ordering::SeqCst);
    }
}

fn audio_reader(sh: Arc<Shared>, gen: u64, out: Option<std::process::ChildStdout>) {
    let Some(mut out) = out else { return };
    let mut buf = vec![0u8; 16 * 1024];
    let mut carry: Vec<u8> = Vec::new();
    // keep ≈ 1 s buffered
    let max = (RATE * 2) as usize;
    loop {
        if sh.gen.load(Ordering::SeqCst) != gen {
            return;
        }
        let n = match out.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        carry.extend_from_slice(&buf[..n]);
        let whole = carry.len() / 4 * 4;
        let samples: Vec<f32> = carry[..whole]
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        carry.drain(..whole);
        let mut q = sh.pcm.lock().unwrap();
        while q.len() + samples.len() > max && sh.gen.load(Ordering::SeqCst) == gen {
            q = sh
                .pcm_cv
                .wait_timeout(q, Duration::from_millis(50))
                .unwrap()
                .0;
        }
        if sh.gen.load(Ordering::SeqCst) != gen {
            return;
        }
        q.extend(samples);
        sh.audio_seen.store(true, Ordering::SeqCst);
    }
    if sh.gen.load(Ordering::SeqCst) == gen {
        sh.audio_eof.store(true, Ordering::SeqCst);
    }
}

/// Output size for a picture of `aspect` shown in a `w`×`h` box (physical px), even numbers,
/// never above the source size.
pub fn output_size(
    aspect: f64,
    w: f64,
    h: f64,
    src_w: Option<i64>,
    src_h: Option<i64>,
) -> (u32, u32) {
    let aspect = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        16.0 / 9.0
    };
    let (mut ow, mut oh) = if w / h > aspect {
        (h * aspect, h)
    } else {
        (w, w / aspect)
    };
    if let (Some(sw), Some(sh)) = (src_w, src_h) {
        let (sw, sh) = (sw as f64, sh as f64);
        if sw > 0.0 && sh > 0.0 && (ow > sw || oh > sh) {
            let k = (sw / ow).min(sh / oh);
            ow *= k;
            oh *= k;
        }
    }
    let even = |v: f64| ((v.round() as u32) / 2 * 2).max(2);
    (even(ow), even(oh))
}

/// Extract the full-size frame at `t` as JPEG (save frame as photo).
pub fn grab_frame(input: &str, t: f64, out: &std::path::Path) -> Result<(), String> {
    let o = command()
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-ss",
            &format!("{t:.3}"),
            "-i",
            input,
        ])
        .args(["-frames:v", "1", "-q:v", "2", "-y"])
        .arg(out)
        .output()
        .map_err(|e| format!("ffmpeg: {e}"))?;
    if o.status.success() && out.is_file() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(
            output_size(16.0 / 9.0, 1280.0, 1280.0, None, None),
            (1280, 720)
        );
        assert_eq!(
            output_size(9.0 / 16.0, 1280.0, 720.0, None, None),
            (404, 720)
        );
        // never upscale past the source
        assert_eq!(
            output_size(16.0 / 9.0, 3840.0, 2160.0, Some(1920), Some(1080)),
            (1920, 1080)
        );
        assert_eq!(output_size(f64::NAN, 100.0, 100.0, None, None), (100, 56));
    }

    /// Needs ffmpeg on the PATH (skipped otherwise): decode a generated clip with audio and check
    /// that frames advance with the (null sink) audio clock and seeking restarts at the target.
    #[test]
    fn decode_generated_clip() {
        if Command::new(ffmpeg_path())
            .arg("-version")
            .output()
            .is_err()
        {
            eprintln!("no ffmpeg: skipped");
            return;
        }
        let dir = std::env::temp_dir().join(format!("pa-ui-video-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("clip.mp4");
        let ok = Command::new(ffmpeg_path())
            .args([
                "-nostdin",
                "-loglevel",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=320x240:rate=30:duration=3",
            ])
            .args([
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=3",
                "-shortest",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&clip)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "ffmpeg could not make the test clip");
        std::env::set_var("PA_NO_AUDIO", "1");
        let src = Source {
            input: clip.to_string_lossy().into(),
            video: true,
            width: 160,
            height: 120,
            fps: 30.0,
            duration: Some(3.0),
            audio: Some(true),
            vf_pre: Vec::new(),
            vf_post: Vec::new(),
        };
        let mut p = Player::new(src, 0.0, true, 1.0, false, 1.0);
        let t0 = Instant::now();
        let mut frames = Vec::new();
        while t0.elapsed() < Duration::from_millis(1200) {
            if let Some(f) = p.take_frame() {
                assert_eq!((f.px.width(), f.px.height()), (160, 120));
                frames.push(f.pts);
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        assert!(frames.len() >= 15, "only {} frames in 1.2 s", frames.len());
        assert!(frames.windows(2).all(|w| w[1] > w[0]));
        let st = p.status();
        assert!(st.no_device && st.playing && st.position > 0.5, "{st:?}");
        // seek: the next frame is at the target
        p.seek(2.0);
        let t1 = Instant::now();
        let mut first = None;
        while first.is_none() && t1.elapsed() < Duration::from_secs(3) {
            first = p.take_frame().map(|f| f.pts);
            std::thread::sleep(Duration::from_millis(4));
        }
        assert!((first.unwrap() - 2.0).abs() < 0.05);
        // play to the end
        let t2 = Instant::now();
        while !p.status().ended && t2.elapsed() < Duration::from_secs(4) {
            let _ = p.take_frame();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(p.status().ended);
        drop(p);
        std::fs::remove_dir_all(dir).ok();
    }
}
