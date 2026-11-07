//! Parity runner: renders the same renditions as the TypeScript reference runner for a list of
//! files and writes them next to a results.json (compare with `scripts/compare.py`).
//!
//! cargo run --release -p pa-media --example parity -- <list.json> <outdir> [<ts-outdir>]

use base64::Engine;
use pa_media::render::{self, ThumbQuality};
use pa_media::{ffmpeg, image_info_path, thumbhash, Ffmpeg};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Instant;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let list: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(&args[1]).unwrap()).unwrap();
    let out = Path::new(&args[2]);
    let ts_out = args.get(3).map(Path::new);
    std::fs::create_dir_all(out).unwrap();
    let ff = Ffmpeg::new("ffmpeg", Some("ffprobe".into()));
    ff.check().await;
    let mut results = Vec::new();
    for (i, f) in list.iter().enumerate() {
        let id = format!("{i:03}");
        let abs = f["abs"].as_str().unwrap();
        let kind = f["kind"].as_str().unwrap();
        let ext = f["ext"].as_str().unwrap();
        let p = Path::new(abs);
        let mut r = json!({ "id": id, "abs": abs });
        match image_info_path(p) {
            Ok(m) => {
                r["meta"] = json!({ "width": m.width, "height": m.height, "pages": m.pages, "orientation": m.orientation, "format": m.format })
            }
            Err(e) => r["meta"] = json!({ "error": e.to_string() }),
        }
        let t0 = Instant::now();
        let base = if kind == "video" {
            let png = ff
                .frame(abs, ffmpeg::thumb_frame_time(f["duration"].as_f64()), 1600)
                .await;
            png.and_then(|png| render::render_base_frame(&png, ThumbQuality::Normal))
        } else {
            render::decode_for_base(p)
                .and_then(|img| render::render_base(&img, ThumbQuality::Normal))
        };
        r["baseMs"] = json!(t0.elapsed().as_secs_f64() * 1000.0);
        match base {
            Ok(b) => {
                std::fs::write(out.join(format!("{id}-640.webp")), &b.webp640).unwrap();
                std::fs::write(out.join(format!("{id}-320.webp")), &b.webp320).unwrap();
                if let Some(x) = &b.webp1600 {
                    std::fs::write(out.join(format!("{id}-1600.webp")), x).unwrap();
                }
                r["base"] =
                    json!({ "thumbhash": b.thumbhash, "width": b.width, "height": b.height });
            }
            Err(e) => r["baseErr"] = json!(e.to_string()),
        }
        if kind == "image" {
            match render::render_1600_file(p, ThumbQuality::Normal) {
                Ok(b) => std::fs::write(out.join(format!("{id}-1600.webp")), b).unwrap(),
                Err(e) => r["t1600Err"] = json!(e.to_string()),
            }
            for max in [None, Some(2048)] {
                match render::render_display_file(p, ext, max) {
                    Ok((b, fmt)) => std::fs::write(
                        out.join(format!(
                            "{id}-display-{}.{}",
                            max.map(|m| m.to_string()).unwrap_or("full".into()),
                            fmt.ext()
                        )),
                        b,
                    )
                    .unwrap(),
                    Err(e) => {
                        r[format!(
                            "display{}Err",
                            max.map(|m| m.to_string()).unwrap_or("full".into())
                        )] = json!(e.to_string())
                    }
                }
            }
        }
        // thumbhash port on the exact RGBA the TS code hashed
        if let Some(ts) = ts_out {
            let rgba = ts.join(format!("{id}-hash.rgba"));
            if let Ok(px) = std::fs::read(&rgba) {
                let tsr: Value = serde_json::from_str(
                    &std::fs::read_to_string(ts.join("results.json")).unwrap(),
                )
                .unwrap();
                let tr = &tsr[i];
                let (w, h) = (
                    tr["hashInput"]["w"].as_u64().unwrap() as usize,
                    tr["hashInput"]["h"].as_u64().unwrap() as usize,
                );
                let mine = base64::engine::general_purpose::STANDARD
                    .encode(thumbhash::rgba_to_thumb_hash(w, h, &px).unwrap());
                r["hashOfTsInput"] = json!(mine);
                r["hashOfTsInputEqual"] =
                    json!(tr["base"]["thumbhash"].as_str() == Some(mine.as_str()));
            }
        }
        eprintln!("{id} {abs} {}", r["baseErr"].as_str().unwrap_or(""));
        results.push(r);
    }
    std::fs::write(
        out.join("results.json"),
        serde_json::to_string_pretty(&results).unwrap(),
    )
    .unwrap();
}
