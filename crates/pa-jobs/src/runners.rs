//! Job runners for the video editor and the slideshow exporter (mirrors `jobs/runners.ts`).
//! Every request is validated (and the source probed) *before* it is queued, so bad input fails
//! the POST with a 400 instead of producing a failed job.

use crate::error::{HttpError, JobFailure};
use crate::paths::{
    exists, join_rel, normalize_rel, rand_hex4, stem, unique_edit_path, unique_path,
};
use crate::queue::{runner, JobContext, JobService};
use crate::slideshow::{self as ss, PieceInput, SlideKind, SlideSource, SlideshowPlan};
use crate::video_edit::{self as ve, VideoEditIo, VideoEditPlan, VideoSourceInfo};
use async_trait::async_trait;
use futures_util::{StreamExt, TryStreamExt};
use pa_core::{FileLocation, Job, JobType, MediaKind, VideoFormat};
use pa_media::ffmpeg::{Ffmpeg, ProbeOptions, ProbeResult};
use pa_media::jobs::{pipe_frames, run_ffmpeg, spawn_fed, JobError, JobRun};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A media row as the export jobs need it (`mediaRow` + `mediaAbs`).
#[derive(Debug, Clone)]
pub struct JobMedia {
    pub id: i64,
    pub folder_id: i64,
    pub dir: String,
    pub name: String,
    pub kind: MediaKind,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub duration: Option<f64>,
    /// `media.extra` parsed (video facts: hasAudio, fps …)
    pub extra: Option<Value>,
    /// the local file, or the error `mediaAbs` throws for it (403 for phone / MEGA folders)
    pub abs: Result<PathBuf, HttpError>,
}

/// What the export runners need from the server (library DB + scanner + settings).
#[async_trait]
pub trait MediaLookup: Send + Sync + 'static {
    /// `mediaRow(id)` + `mediaAbs`: 404 'Media not found' for unknown ids.
    fn media(&self, id: &str) -> Result<JobMedia, HttpError>;
    /// `folderPath(folderId, dir)`: a local, writable folder → (folder id, absolute dir).
    /// 404 unknown folder, 403 phone / MEGA folders, 400 path escapes.
    fn target_dir(&self, folder_id: &str, dir: &str) -> Result<(i64, PathBuf), HttpError>;
    /// `Settings.editSuffix` ("_edit")
    fn edit_suffix(&self) -> String;
    /// `scanner.upsertFile(folderId, rel)`: index a file the job produced.
    async fn upsert_file(&self, folder_id: i64, rel: &str);
    /// Decode an image slide (EXIF-rotated, HEIC-capable), fit + pad it onto `bg` and return raw
    /// RGB24 of exactly `width × height`. Default: pa-media (`sharp … fit: 'contain'`).
    async fn render_canvas(
        &self,
        m: &JobMedia,
        abs: &Path,
        width: u32,
        height: u32,
        bg: [u8; 3],
    ) -> Result<Vec<u8>, String> {
        let _ = m;
        let abs = abs.to_path_buf();
        tokio::task::spawn_blocking(move || crate::canvas::contain_rgb(&abs, width, height, bg))
            .await
            .map_err(|e| e.to_string())?
    }
}

/// The export side of `registerJobRunners(ctx)`.
#[derive(Clone)]
pub struct Exporter {
    pub jobs: JobService,
    pub ffmpeg: Arc<Ffmpeg>,
    pub host: Arc<dyn MediaLookup>,
    /// `cfg.tempDir` (gif palettes, slideshow canvases)
    pub temp_dir: PathBuf,
}

async fn is_file(p: &Path) -> bool {
    tokio::fs::metadata(p).await.is_ok_and(|m| m.is_file())
}

fn extra_bool(e: &Option<Value>, k: &str) -> bool {
    e.as_ref().and_then(|v| v.get(k)) == Some(&Value::Bool(true))
}

fn extra_num(e: &Option<Value>, k: &str) -> Option<f64> {
    e.as_ref().and_then(|v| v.get(k)).and_then(|v| v.as_f64())
}

fn loc(folder_id: i64, dir: &str, name: &str) -> FileLocation {
    FileLocation {
        folder_id: folder_id.to_string(),
        dir: dir.to_string(),
        name: name.to_string(),
    }
}

fn hex_rgb(hex: &str) -> [u8; 3] {
    let c = |i: usize| u8::from_str_radix(hex.get(1 + i..3 + i).unwrap_or("00"), 16).unwrap_or(0);
    [c(0), c(2), c(4)]
}

impl Exporter {
    async fn probe(&self, abs: &Path) -> Option<ProbeResult> {
        self.ffmpeg
            .probe(&abs.to_string_lossy(), &ProbeOptions::default())
            .await
    }

    async fn encoders(&self) -> HashSet<String> {
        self.ffmpeg.encoders().await.clone()
    }

    fn require_ffmpeg(&self) -> Result<(), HttpError> {
        if self.ffmpeg.available() {
            Ok(())
        } else {
            Err(HttpError::new(501, "ffmpeg is not available"))
        }
    }

    /// `POST /api/jobs/video-edit` (after `requireWritable`): validate, probe, queue.
    pub async fn start_video_edit(&self, spec: &Value) -> Result<Job, HttpError> {
        self.require_ffmpeg()?;
        let n = ve::normalize_video_edit_spec(spec)?;
        let row = self.host.media(&n.source_id)?;
        if row.kind != MediaKind::Video {
            return Err(HttpError::bad("The source is not a video"));
        }
        let abs = row.abs.clone()?;
        if !is_file(&abs).await {
            return Err(HttpError::new(404, "Source file not found"));
        }
        let probe = self.probe(&abs).await;
        let src = VideoSourceInfo {
            width: probe
                .as_ref()
                .and_then(|p| p.width.map(f64::from))
                .or(row.width)
                .unwrap_or(0.0),
            height: probe
                .as_ref()
                .and_then(|p| p.height.map(f64::from))
                .or(row.height)
                .unwrap_or(0.0),
            duration: probe
                .as_ref()
                .and_then(|p| p.duration)
                .or(row.duration)
                .unwrap_or(0.0),
            has_audio: match &probe {
                Some(p) => p.has_audio,
                None => extra_bool(&row.extra, "hasAudio"),
            },
            fps: match probe.as_ref().and_then(|p| p.fps) {
                Some(f) => Some(f),
                None => extra_num(&row.extra, "fps"),
            },
        };
        let plan = ve::plan_video_edit(&n, &src)?;
        let mut overlays = Vec::new();
        for o in &plan.overlays {
            overlays.push(
                self.jobs
                    .asset_path(&o.asset_id)
                    .map_err(|_| HttpError::bad("Invalid overlay asset id"))?,
            );
        }
        for o in &overlays {
            if !is_file(o).await {
                return Err(HttpError::bad("Overlay asset not found — upload it first"));
            }
        }
        let encoders = self.encoders().await;
        ve::pick_codecs(plan.ext, &encoders)?; // 400 when this ffmpeg can't write the format
        let suffix = self.host.edit_suffix();
        let me = self.clone();
        let o = EditRun {
            plan,
            abs,
            overlays,
            encoders,
            folder_id: row.folder_id,
            dir: row.dir.clone(),
            base: stem(&row.name).to_string(),
            suffix,
        };
        Ok(self.jobs.enqueue(
            JobType::VideoEdit,
            row.name.clone(),
            runner(move |jc| async move { me.run_video_edit(jc, o).await }),
        ))
    }

    async fn run_video_edit(
        &self,
        jc: JobContext,
        o: EditRun,
    ) -> Result<Option<FileLocation>, JobFailure> {
        let plan = &o.plan;
        if !is_file(&o.abs).await {
            return Err("Source file not found".into());
        }
        let dir_abs = o.abs.parent().unwrap_or(Path::new("")).to_path_buf();
        let ext = ve::ext_of(plan.ext);
        let target = unique_edit_path(&dir_abs, &o.base, &o.suffix, ext).await;
        let part = PathBuf::from(format!("{}.{}.part", target.to_string_lossy(), rand_hex4()));
        let mut palette = None;
        if plan.ext == VideoFormat::Gif {
            tokio::fs::create_dir_all(&self.temp_dir).await?;
            palette = Some(self.temp_dir.join(format!("palette-{}.png", rand_hex4())));
        }
        let res = async {
            let passes = ve::build_video_edit_passes(
                plan,
                &VideoEditIo {
                    input: o.abs.to_string_lossy().into_owned(),
                    overlays: o
                        .overlays
                        .iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect(),
                    output: part.to_string_lossy().into_owned(),
                    palette: palette.as_ref().map(|p| p.to_string_lossy().into_owned()),
                    encoders: o.encoders.clone(),
                },
            )?;
            let mut done = 0.0;
            for pass in &passes {
                let w = pass.weight;
                let jc2 = jc.clone();
                let mut cb = move |p: f64| jc2.progress((done + p * w) * 0.99);
                run_ffmpeg(
                    &self.ffmpeg,
                    &pass.args,
                    JobRun {
                        cancel: &jc.cancel,
                        duration: Some(plan.duration),
                        on_progress: Some(&mut cb),
                    },
                )
                .await?;
                done += pass.weight;
            }
            if jc.is_cancelled() {
                return Err(JobFailure::from(JobError::Aborted));
            }
            let size = tokio::fs::metadata(&part)
                .await
                .map(|m| m.len())
                .unwrap_or(0);
            if size == 0 {
                return Err("ffmpeg produced no output".into());
            }
            // the name may have been taken while we were encoding
            let dest = if exists(&target).await {
                unique_edit_path(&dir_abs, &o.base, &o.suffix, ext).await
            } else {
                target.clone()
            };
            tokio::fs::rename(&part, &dest).await?;
            let name = dest
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            self.host
                .upsert_file(o.folder_id, &join_rel(&o.dir, &name))
                .await;
            Ok(Some(loc(o.folder_id, &o.dir, &name)))
        }
        .await;
        let _ = tokio::fs::remove_file(&part).await;
        if let Some(p) = &palette {
            let _ = tokio::fs::remove_file(p).await;
        }
        res
    }

    /// `POST /api/jobs/slideshow` (after `requireWritable`): validate, probe, queue.
    pub async fn start_slideshow(&self, spec: &Value) -> Result<Job, HttpError> {
        self.require_ffmpeg()?;
        let n = ss::normalize_slideshow_spec(spec)?;
        let mut slides = Vec::with_capacity(n.items.len());
        for it in &n.items {
            let row = self.host.media(&it.id)?;
            if row.kind != MediaKind::Image && row.kind != MediaKind::Video {
                return Err(HttpError::bad(format!(
                    "\"{}\" is not an image or a video",
                    row.name
                )));
            }
            let abs = row.abs.clone()?;
            slides.push(Prepared { row, abs });
        }
        let probes: Vec<SlideSource> = futures_util::stream::iter(slides.iter())
            .map(|s| async move {
                if !is_file(&s.abs).await {
                    return Err(HttpError::new(
                        404,
                        format!("File not found: {}", s.row.name),
                    ));
                }
                if s.row.kind == MediaKind::Image {
                    return Ok(SlideSource::Image);
                }
                let p = self.probe(&s.abs).await;
                if p.as_ref().is_some_and(|p| p.width.unwrap_or(0) == 0) {
                    return Err(HttpError::bad(format!(
                        "\"{}\" has no video stream",
                        s.row.name
                    )));
                }
                Ok(SlideSource::Video {
                    duration: p.and_then(|p| p.duration).or(s.row.duration).unwrap_or(0.0),
                })
            })
            .buffered(4)
            .try_collect()
            .await?;

        let mut music = None;
        if let Some(aid) = &n.audio_id {
            let row = self.host.media(aid)?;
            if row.kind != MediaKind::Audio && row.kind != MediaKind::Video {
                return Err(HttpError::bad("The music track must be an audio file"));
            }
            let abs = row.abs.clone()?;
            if !is_file(&abs).await {
                return Err(HttpError::new(404, "Music file not found"));
            }
            let p = self.probe(&abs).await;
            if p.is_some_and(|p| !p.has_audio) {
                return Err(HttpError::bad(format!(
                    "\"{}\" has no audio track",
                    row.name
                )));
            }
            music = Some(abs);
        }

        let (folder_id, dir, dir_abs) = match &n.target {
            Some(t) => {
                let (fid, abs) = self.host.target_dir(&t.folder_id, &t.dir)?;
                if !tokio::fs::metadata(&abs).await.is_ok_and(|m| m.is_dir()) {
                    return Err(HttpError::bad("Target directory does not exist"));
                }
                (fid, normalize_rel(&t.dir)?, abs)
            }
            None => {
                let first = &slides[0];
                (
                    first.row.folder_id,
                    first.row.dir.clone(),
                    first.abs.parent().unwrap_or(Path::new("")).to_path_buf(),
                )
            }
        };

        let mut rng = rand::random::<f64>;
        let plan = ss::plan_slideshow(&n, &probes, &mut rng)?;
        let encoders = self.encoders().await;
        ve::pick_codecs(VideoFormat::Mp4, &encoders)?;
        let me = self.clone();
        let o = ShowRun {
            plan,
            slides,
            music,
            folder_id,
            dir,
            dir_abs,
            name: n.name.clone(),
        };
        Ok(self.jobs.enqueue(
            JobType::Slideshow,
            format!("{}.mp4", n.name),
            runner(move |jc| async move { me.run_slideshow(jc, o).await }),
        ))
    }

    async fn run_slideshow(
        &self,
        jc: JobContext,
        o: ShowRun,
    ) -> Result<Option<FileLocation>, JobFailure> {
        tokio::fs::create_dir_all(&self.temp_dir).await?;
        let tmp = self.temp_dir.join(format!("slideshow-{}", rand_hex4()));
        tokio::fs::create_dir_all(&tmp).await?;
        let file_name = format!("{}.mp4", o.name);
        let target = unique_path(&o.dir_abs, &file_name).await;
        let part = PathBuf::from(format!("{}.{}.part", target.to_string_lossy(), rand_hex4()));
        let res = self.slideshow_inner(&jc, &o, &tmp, &target, &part).await;
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        let _ = tokio::fs::remove_file(&part).await;
        res
    }

    async fn slideshow_inner(
        &self,
        jc: &JobContext,
        o: &ShowRun,
        tmp: &Path,
        target: &Path,
        part: &Path,
    ) -> Result<Option<FileLocation>, JobFailure> {
        let plan = &o.plan;
        let cancel = &jc.cancel;
        let fb = ss::frame_bytes(plan.width, plan.height);
        let fill = ss::solid_frame(plan.width as u32, plan.height as u32, &plan.background)?;
        let total_bytes = (plan.total_frames as u64 * fb).max(1);
        let mut sent = 0u64;
        let bg = hex_rgb(&plan.background);
        // pre-rendered canvases: at most the current, next and prefetched slide exist at once
        let mut canvases: HashMap<usize, Canvas> = HashMap::new();
        let prefetch = |canvases: &mut HashMap<usize, Canvas>, i: usize| {
            if i >= plan.slides.len()
                || plan.slides[i].kind != SlideKind::Image
                || canvases.contains_key(&i)
            {
                return;
            }
            let (w, h) = ss::canvas_size(plan, &plan.slides[i]);
            let host = self.host.clone();
            let s = o.slides[i].clone();
            let file = tmp.join(format!("slide-{i}.rgb"));
            canvases.insert(
                i,
                Canvas::Pending(tokio::spawn(async move {
                    let data = host
                        .render_canvas(&s.row, &s.abs, w, h, bg)
                        .await
                        .map_err(|e| format!("Cannot decode {}: {e}", s.row.name))?;
                    if data.len() != (w as usize) * (h as usize) * 3 {
                        return Err(format!("Unexpected decode result for {}", s.row.name));
                    }
                    tokio::fs::write(&file, data)
                        .await
                        .map_err(|e| e.to_string())?;
                    Ok(file)
                })),
            );
        };
        let encoder_args = ss::build_encoder_args(
            plan,
            &part.to_string_lossy(),
            o.music.as_ref().map(|m| m.to_string_lossy()).as_deref(),
        );
        let mut enc = spawn_fed(&self.ffmpeg, &encoder_args, cancel)?;
        let r: Result<(), JobFailure> = async {
            for i in 0..plan.slides.len() {
                if cancel.is_cancelled() {
                    return Err(JobError::Aborted.into());
                }
                let r = ss::piece_range(plan, i);
                prefetch(&mut canvases, i + 1);
                let mut inputs = Vec::new();
                for k in [i, i + 1]
                    .into_iter()
                    .take(if r.xfade.is_some() { 2 } else { 1 })
                {
                    let s = &plan.slides[k];
                    if s.kind == SlideKind::Video {
                        inputs.push(PieceInput::Video {
                            path: o.slides[k].abs.to_string_lossy().into_owned(),
                        });
                    } else {
                        prefetch(&mut canvases, k);
                        let c = canvases.remove(&k).expect("prefetched");
                        let path = match c.wait().await {
                            Ok(p) => p,
                            Err(e) => return Err(e.into()),
                        };
                        // keep the finished result for the next piece
                        canvases.insert(k, Canvas::Ready(path.clone()));
                        let (width, height) = ss::canvas_size(plan, s);
                        inputs.push(PieceInput::Canvas {
                            path: path.to_string_lossy().into_owned(),
                            width,
                            height,
                        });
                    }
                }
                prefetch(&mut canvases, i + 2);
                let (args, frames) = ss::build_piece_args(plan, i, &inputs)?;
                let jc2 = jc.clone();
                let mut on_bytes = |b: u64| {
                    sent += b;
                    jc2.progress(sent as f64 / total_bytes as f64 * 0.98);
                };
                pipe_frames(
                    &self.ffmpeg,
                    &args,
                    &mut enc,
                    frames as u64 * fb,
                    &fill,
                    cancel,
                    &mut on_bytes,
                )
                .await?;
                if let Some(c) = canvases.remove(&i) {
                    if let Ok(f) = c.wait().await {
                        let _ = tokio::fs::remove_file(f).await;
                    }
                }
            }
            enc.finish().await?;
            if cancel.is_cancelled() {
                return Err(JobError::Aborted.into());
            }
            Ok(())
        }
        .await;
        if !enc.exited() {
            drop(enc.stdin.take());
            enc.kill();
            let _ = enc.done().await;
        }
        for (_, c) in canvases.drain() {
            let _ = c.wait().await;
        }
        r?;
        let size = tokio::fs::metadata(part)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        if size == 0 {
            return Err("ffmpeg produced no output".into());
        }
        let file_name = format!("{}.mp4", o.name);
        let dest = if exists(target).await {
            unique_path(&o.dir_abs, &file_name).await
        } else {
            target.to_path_buf()
        };
        tokio::fs::rename(part, &dest).await?;
        let name = dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.host
            .upsert_file(o.folder_id, &join_rel(&o.dir, &name))
            .await;
        Ok(Some(loc(o.folder_id, &o.dir, &name)))
    }
}

/// a pre-rendered slide canvas (raw RGB file)
enum Canvas {
    Pending(tokio::task::JoinHandle<Result<PathBuf, String>>),
    Ready(PathBuf),
}

impl Canvas {
    async fn wait(self) -> Result<PathBuf, String> {
        match self {
            Canvas::Ready(p) => Ok(p),
            Canvas::Pending(h) => h.await.map_err(|e| e.to_string())?,
        }
    }
}

struct EditRun {
    plan: VideoEditPlan,
    abs: PathBuf,
    overlays: Vec<PathBuf>,
    encoders: HashSet<String>,
    folder_id: i64,
    dir: String,
    base: String,
    suffix: String,
}

#[derive(Clone)]
struct Prepared {
    row: JobMedia,
    abs: PathBuf,
}

struct ShowRun {
    plan: SlideshowPlan,
    slides: Vec<Prepared>,
    music: Option<PathBuf>,
    folder_id: i64,
    dir: String,
    dir_abs: PathBuf,
    name: String,
}
