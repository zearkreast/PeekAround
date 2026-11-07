//! Decode + resize + encode timings of the thumbnail renditions (compare with the sharp runner).
//!
//! cargo run --release -p pa-media --example bench -- <files.json> <outdir>

use pa_media::render::{self, ThumbQuality};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn base(p: &Path, out: &Path) {
    let img = render::decode_for_base(p).unwrap();
    let b = render::render_base(&img, ThumbQuality::Normal).unwrap();
    render::write_atomic_sync(&out.join("320.webp"), &b.webp320).unwrap();
    render::write_atomic_sync(&out.join("640.webp"), &b.webp640).unwrap();
}

fn med(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (v[v.len() / 2] * 10.0).round() / 10.0
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let files: Vec<String> =
        serde_json::from_str(&std::fs::read_to_string(&args[1]).unwrap()).unwrap();
    let out = PathBuf::from(&args[2]);
    std::fs::create_dir_all(&out).unwrap();
    let mut res = serde_json::Map::new();
    for f in &files {
        let p = Path::new(f);
        let ext = p.extension().unwrap().to_string_lossy().to_lowercase();
        let n = if ext == "heic" { 5 } else { 10 };
        let time = |op: &dyn Fn()| {
            op();
            med((0..n)
                .map(|_| {
                    let t = Instant::now();
                    op();
                    t.elapsed().as_secs_f64() * 1000.0
                })
                .collect())
        };
        let b = time(&|| base(p, &out));
        let t1600 = time(&|| {
            render::write_atomic_sync(
                &out.join("1600.webp"),
                &render::render_1600_file(p, ThumbQuality::Normal).unwrap(),
            )
            .unwrap()
        });
        let d = time(&|| {
            render::write_atomic_sync(
                &out.join("d.jpg"),
                &render::render_display_file(p, &ext, Some(2048)).unwrap().0,
            )
            .unwrap()
        });
        let t0 = Instant::now();
        for _ in 0..3 {
            std::thread::scope(|s| {
                for k in 0..4 {
                    let o = out.join(format!("p{k}"));
                    s.spawn(move || {
                        std::fs::create_dir_all(&o).unwrap();
                        base(p, &o)
                    });
                }
            });
        }
        let per_sec = ((12.0 / t0.elapsed().as_secs_f64()) * 10.0).round() / 10.0;
        let r = serde_json::json!({ "baseMs": b, "t1600Ms": t1600, "display2048Ms": d, "base4xPerSec": per_sec });
        eprintln!("{} {r}", p.file_name().unwrap().to_string_lossy());
        res.insert(p.file_name().unwrap().to_string_lossy().into_owned(), r);
    }
    println!("{}", serde_json::Value::Object(res));
}
