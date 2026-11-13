//! Export (ExportPanel `useVideoExport` + JobProgress `useJobRun`): text layers rendered to
//! transparent PNGs at the output size, uploaded as job assets, then the video-edit job; its
//! progress comes back through the job events.

use std::rc::Rc;

use pa_core::{Job, JobStatus, VideoOverlay};

use super::model::{self, Size};
use super::{ctl, Ctl, JobRun};
use crate::editor::doc::TextLayer;
use crate::editor::overlay::{Canvas, Fonts};
use crate::router::Route;
use crate::ToastKind;

fn status_str(s: JobStatus) -> &'static str {
    match s {
        JobStatus::Queued => "queued",
        JobStatus::Running => "running",
        JobStatus::Done => "done",
        JobStatus::Error => "error",
        JobStatus::Canceled => "canceled",
    }
}

/// One overlay group drawn at the exact output size (`scale` output px per design px) as PNG.
pub(super) fn render_overlay(layers: &[TextLayer], out: Size, scale: f64, fonts: &mut Fonts) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    let (w, h) = (out.w as u32, out.h as u32);
    let mut c = Canvas::new(w, h);
    for l in layers {
        let r = fonts.render_text(l, scale);
        c.draw_raster(&r, l.x * scale, l.y * scale, l.rotation, scale / r.scale, l.opacity);
    }
    let mut px = c.px;
    for p in px.as_chunks_mut::<4>().0.iter_mut() {
        if p[3] > 0 && p[3] < 255 {
            let a = p[3] as u32;
            for ch in &mut p[..3] {
                *ch = ((*ch as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new_with_quality(
        &mut png,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Adaptive,
    )
    .write_image(&px, w, h, image::ExtendedColorType::Rgba8)
    .map_err(|e| e.to_string())?;
    Ok(png)
}

/// Client-side progress of the "preparing" phase (overlay uploads).
fn prep_progress(p: f64) {
    let _ = slint::invoke_from_event_loop(move || {
        let Some(c) = ctl() else { return };
        if c.job.borrow().status == "preparing" {
            c.job.borrow_mut().progress = p;
            c.publish_job();
        }
    });
}

impl Ctl {
    pub(super) fn export(self: &Rc<Self>) {
        if !self.ready.get() || self.job_busy() || !self.disabled.borrow().is_empty() {
            return;
        }
        let Some(a) = self.app() else { return };
        let Some(item) = self.item.borrow().clone() else { return };
        let s = self.doc();
        let src = self.src.get();
        let duration = self.duration.get();
        let base = model::base_size(src, s.crop, s.rotate);
        let out = model::export_size(src, &s);
        let groups = model::overlay_groups(&s.texts, model::out_duration(&s));
        let scale = out.h / base.h;
        *self.job.borrow_mut() = JobRun { status: "preparing".into(), title: item.name.clone(), ..Default::default() };
        self.publish_job();
        let collection = (!groups.is_empty()).then(slint::fontique_011::shared_collection);
        let backend = a.backend.clone();
        let fut = async move {
            let pngs = crate::rt::spawn_blocking(move || -> Result<Vec<(f64, f64, Vec<u8>)>, String> {
                let Some(collection) = collection else { return Ok(vec![]) };
                let mut fonts = Fonts::new(collection);
                groups.into_iter().map(|(st, en, layers)| Ok((st, en, render_overlay(&layers, out, scale, &mut fonts)?))).collect()
            })
            .await
            .map_err(|e| e.to_string())??;
            let n = pngs.len();
            let mut overlays = Vec::new();
            for (i, (start, end, png)) in pngs.into_iter().enumerate() {
                let v = backend.put_bytes("/api/jobs/assets?ext=png", png).await.map_err(|e| e.message)?;
                let asset_id = v.get("assetId").and_then(|x| x.as_str()).ok_or("no asset id")?.to_string();
                overlays.push(VideoOverlay { asset_id, start, end });
                prep_progress((i + 1) as f64 / n as f64);
            }
            let spec = model::build_spec(&item.id, &s, src, duration, overlays);
            let body = serde_json::to_value(&spec).map_err(|e| e.to_string())?;
            let v = backend.request("POST", "/api/jobs/video-edit", Some(body)).await.map_err(|e| e.message)?;
            serde_json::from_value::<Job>(v).map_err(|e| e.to_string())
        };
        let me = Rc::downgrade(self);
        a.rt.spawn_then(fut, move |r| {
            let Some(c) = me.upgrade() else { return };
            let Some(a) = c.app() else { return };
            match r {
                Ok(j) => {
                    {
                        let mut run = c.job.borrow_mut();
                        run.id = Some(j.id.clone());
                        run.status = status_str(j.status).into();
                        run.progress = j.progress;
                    }
                    c.publish_job();
                    a.toast(ToastKind::Info, &a.t("video.exportStarted"), &j.title);
                    crate::pages::jobs::on_job(&a, &j);
                }
                Err(e) => {
                    {
                        let mut run = c.job.borrow_mut();
                        run.status = "error".into();
                        run.error = e.clone();
                    }
                    c.publish_job();
                    a.toast(ToastKind::Error, &a.t("video.exportFailed"), &e);
                }
            }
        });
    }

    pub(super) fn cancel_job(&self) {
        let Some(a) = self.app() else { return };
        let Some(id) = self.job.borrow().id.clone() else { return };
        crate::api::act(&a, "DELETE", &format!("/api/jobs/{}", crate::api::enc(&id)), None, |_, _| {});
    }

    /// Leave the editor for the folder the result was written to.
    pub(super) fn open_result(self: &Rc<Self>) {
        let Some(r) = self.job.borrow().result.clone() else { return };
        let Some(a) = self.app() else { return };
        open_location(&a, r);
    }
}

fn open_location(a: &Rc<crate::app::AppInner>, r: pa_core::FileLocation) {
    if let Some(c) = ctl() {
        c.close();
    }
    if let Some(v) = crate::viewer::get() {
        v.close_now();
    }
    a.go(Route::Folder(r.folder_id, r.dir), true);
}

/// Our export job: the global jobs list leaves its toasts to us.
pub(crate) fn owns_job(id: &str) -> bool {
    ctl().is_some_and(|c| c.job.borrow().id.as_deref() == Some(id))
}

/// A job event (pages/jobs.rs `on_job`).
pub(crate) fn on_job(a: &Rc<crate::app::AppInner>, j: &Job) {
    let Some(c) = ctl() else { return };
    let was = {
        let mut run = c.job.borrow_mut();
        if run.id.as_deref() != Some(j.id.as_str()) {
            return;
        }
        let was = std::mem::replace(&mut run.status, status_str(j.status).into());
        run.progress = j.progress;
        run.error = j.error.clone().unwrap_or_default();
        run.result = j.result.clone();
        was
    };
    c.publish_job();
    if was == status_str(j.status) {
        return;
    }
    match j.status {
        JobStatus::Done => match j.result.clone() {
            Some(r) => {
                let label = a.t("jobs.open");
                let name = r.name.clone();
                crate::pages::dialogs::toast_with(a, ToastKind::Success, &a.t("video.exportDone"), &name, &label, move |a| {
                    open_location(a, r)
                });
            }
            None => a.toast(ToastKind::Success, &a.t("video.exportDone"), &j.title),
        },
        JobStatus::Error => a.toast(ToastKind::Error, &a.t("video.exportFailed"), j.error.as_deref().unwrap_or(&j.title)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::doc::Align;

    #[test]
    fn overlay_png_is_output_sized_with_text_in_place() {
        let mut f = Fonts::new(parley::fontique::Collection::new(parley::fontique::CollectionOptions {
            shared: false,
            system_fonts: true,
        }));
        let l = TextLayer {
            id: 1,
            text: "Hello".into(),
            family: "Arial".into(),
            weight: 700.0,
            italic: false,
            size: 40.0,
            color: [255, 255, 255, 255],
            align: Align::Center,
            line_height: 1.15,
            letter_spacing: 0.0,
            x: 160.0,
            y: 90.0,
            rotation: 0.0,
            opacity: 1.0,
            stroke: None,
            shadow: None,
            background: None,
        };
        // design 320×180 exported at 640×360
        let png = render_overlay(&[l], Size { w: 640.0, h: 360.0 }, 2.0, &mut f).unwrap();
        let img = image::load_from_memory(&png).unwrap().into_rgba8();
        assert_eq!(img.dimensions(), (640, 360));
        let ink = |x0: u32, x1: u32, y0: u32, y1: u32| {
            (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).filter(|(x, y)| img.get_pixel(*x, *y)[3] > 128).count()
        };
        // the text sits around the centre (320, 180); the corners stay clear
        assert!(ink(220, 420, 140, 220) > 300);
        assert_eq!(ink(0, 100, 0, 100), 0);
        assert_eq!(ink(540, 640, 260, 360), 0);
    }
}
