//! Video renditions through the ffmpeg CLI (same arguments as `media/thumbs.ts`): the hover
//! preview clip and the filmstrip sprite of the video editor.

use crate::decode::{decode_bytes, DecodeOptions};
use crate::encode::{self, JpegOptions};
use crate::error::{MediaError, Result};
use crate::ffmpeg::{Ffmpeg, RunOptions};
use image::{DynamicImage, GenericImage, RgbImage};
use std::path::Path;
use std::time::Duration;

/// `-filter_complex` of the ~3 s muted preview: four 0.9 s pieces of long videos, the first 3 s
/// of short ones; 240 px high, 24 fps.
pub fn preview_filter(duration: f64) -> String {
    if duration > 8.0 {
        let pts: Vec<f64> = [0.15, 0.4, 0.65, 0.88]
            .iter()
            .map(|p| duration * p)
            .collect();
        let parts: Vec<String> = pts.iter().enumerate().map(|(i, s)| format!("[0:v]trim=start={s:.2}:duration=0.9,setpts=PTS-STARTPTS,scale=-2:240,fps=24[v{i}]")).collect();
        let labels: String = (0..pts.len()).map(|i| format!("[v{i}]")).collect();
        format!(
            "{};{labels}concat=n={}:v=1:a=0,format=yuv420p[out]",
            parts.join(";"),
            pts.len()
        )
    } else {
        "[0:v]trim=start=0:duration=3,setpts=PTS-STARTPTS,scale=-2:240,fps=24,format=yuv420p[out]"
            .into()
    }
}

pub fn preview_args(input: &str, duration: f64, out: &str) -> Vec<String> {
    let f = preview_filter(duration);
    [
        "-y",
        "-i",
        input,
        "-filter_complex",
        &f,
        "-map",
        "[out]",
        "-an",
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-crf",
        "28",
        "-movflags",
        "+faststart",
        out,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Encode the hover preview into `out` (via `<out>.tmp.mp4` + rename; 120 s timeout).
pub async fn make_preview(
    ff: &Ffmpeg,
    input: &Path,
    duration: Option<f64>,
    out: &Path,
) -> Result<()> {
    if let Some(d) = out.parent() {
        tokio::fs::create_dir_all(d).await?;
    }
    let tmp = format!("{}.tmp.mp4", out.display());
    let args = preview_args(&input.to_string_lossy(), duration.unwrap_or(0.0), &tmp);
    let r = ff
        .run(
            &args,
            RunOptions {
                timeout: Some(Duration::from_secs(120)),
                input: None,
            },
        )
        .await;
    if let Err(e) = r {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e);
    }
    tokio::fs::rename(&tmp, out).await?;
    Ok(())
}

/// clamp the filmstrip request like the TS code: 2..=60 frames, 32..=240 px high
pub fn filmstrip_dims(n: f64, h: f64) -> (u32, u32) {
    let r = |x: f64| (x + 0.5).floor();
    (r(n).clamp(2.0, 60.0) as u32, r(h).clamp(32.0, 240.0) as u32)
}

/// Horizontal JPEG sprite (q 70) of `n` frames, `h` px high; frames that fail repeat the first one.
/// None when no frame could be read.
pub async fn filmstrip(
    ff: &Ffmpeg,
    input: &Path,
    duration: Option<f64>,
    n: u32,
    h: u32,
) -> Result<Option<Vec<u8>>> {
    let dur = duration.unwrap_or(1.0).max(0.1);
    let input = input.to_string_lossy().into_owned();
    let mut frames: Vec<Vec<u8>> = Vec::with_capacity(n as usize);
    for i in 0..n {
        let at = dur * (i as f64 + 0.5) / n as f64;
        let args: Vec<String> = [
            "-ss",
            &format!("{at:.3}"),
            "-i",
            &input,
            "-frames:v",
            "1",
            "-vf",
            &format!("scale=-2:{h}"),
            "-f",
            "image2pipe",
            "-vcodec",
            "png",
            "pipe:1",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        frames.push(
            ff.run(
                &args,
                RunOptions {
                    timeout: Some(Duration::from_secs(20)),
                    input: None,
                },
            )
            .await
            .unwrap_or_default(),
        );
    }
    tokio::task::spawn_blocking(move || compose_strip(&frames, h))
        .await
        .map_err(|e| MediaError::Decode(e.to_string()))?
}

/// Compose PNG frames side by side on black (frame width = the first valid frame's width).
pub fn compose_strip(frames: &[Vec<u8>], h: u32) -> Result<Option<Vec<u8>>> {
    let decoded: Vec<Option<DynamicImage>> = frames
        .iter()
        .map(|b| {
            if b.is_empty() {
                None
            } else {
                decode_bytes(b, DecodeOptions::default()).ok()
            }
        })
        .collect();
    let Some(first) = decoded.iter().flatten().next().cloned() else {
        return Ok(None);
    };
    let w = first.width();
    let n = frames.len() as u32;
    let mut canvas = RgbImage::new(w * n, h);
    for (i, f) in decoded.iter().enumerate() {
        let img = f.as_ref().unwrap_or(&first).to_rgb8();
        let sub = image::imageops::crop_imm(&img, 0, 0, img.width().min(w), img.height().min(h))
            .to_image();
        let _ = canvas.copy_from(&sub, i as u32 * w, 0);
    }
    let opts = JpegOptions {
        quality: 70,
        mozjpeg: false,
        chroma444: false,
    };
    Ok(Some(encode::jpeg(&DynamicImage::ImageRgb8(canvas), opts)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_filters() {
        assert_eq!(preview_filter(5.0), "[0:v]trim=start=0:duration=3,setpts=PTS-STARTPTS,scale=-2:240,fps=24,format=yuv420p[out]");
        let f = preview_filter(20.0);
        assert!(f.starts_with("[0:v]trim=start=3.00:duration=0.9,"), "{f}");
        assert!(f.ends_with("[v0][v1][v2][v3]concat=n=4:v=1:a=0,format=yuv420p[out]"));
        assert_eq!(filmstrip_dims(100.0, 10.0), (60, 32));
    }
}
