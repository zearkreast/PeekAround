//! Parity with the TypeScript builders: `tests/golden/jobs.json.gz` was produced by running
//! packages/server/src/jobs/{video-edit,slideshow}.ts on 5500 generated specs
//! (`tests/tools/gen-jobs.ts`); every normalized spec, plan, ffmpeg argument list and 400 error
//! must come out identical.

use pa_jobs::slideshow::{self as ss, PieceInput, SlideKind, SlideSource};
use pa_jobs::video_edit::{self as ve, VideoEditIo, VideoSourceInfo};
use pa_jobs::HttpError;
use serde::Serialize;
use serde_json::{json, Value};
use std::io::Read;

fn golden() -> Value {
    let f = std::fs::File::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/jobs.json.gz"
    ))
    .unwrap();
    let mut s = String::new();
    flate2::read::GzDecoder::new(f)
        .read_to_string(&mut s)
        .unwrap();
    serde_json::from_str(&s).unwrap()
}

/// structural equality with every number compared as f64 (JS has one number type)
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

fn res<T: Serialize>(r: &Result<T, HttpError>) -> Value {
    match r {
        Ok(v) => serde_json::to_value(v).unwrap(),
        Err(e) => json!({ "error": { "status": e.status, "message": e.message } }),
    }
}

fn check(what: &str, i: usize, got: &Value, want: &Value, spec: &Value) {
    assert!(
        same(got, want),
        "{what} #{i}\nspec: {spec}\n got: {got}\nwant: {want}"
    );
}

#[test]
fn video_edit_matches_typescript() {
    let g = golden();
    let mut passes = 0;
    for (i, c) in g["video"].as_array().unwrap().iter().enumerate() {
        let spec = &c["spec"];
        let norm = ve::normalize_video_edit_spec(spec);
        check("normalize", i, &res(&norm), &c["norm"], spec);
        let Ok(n) = norm else { continue };
        let s = &c["src"];
        let src = VideoSourceInfo {
            width: s["width"].as_f64().unwrap(),
            height: s["height"].as_f64().unwrap(),
            duration: s["duration"].as_f64().unwrap(),
            has_audio: s["hasAudio"].as_bool().unwrap(),
            fps: s["fps"].as_f64(),
        };
        let plan = ve::plan_video_edit(&n, &src);
        let mut want = c["plan"].clone();
        // `...n` also spreads `trim` into the TS plan object (not part of VideoEditPlan)
        if let Some(o) = want.as_object_mut() {
            o.remove("trim");
        }
        check("plan", i, &res(&plan), &want, spec);
        let Ok(p) = plan else { continue };
        let encoders = c["encoders"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e.as_str().unwrap().to_string())
            .collect();
        let io = VideoEditIo {
            input: "/in/src.mp4".into(),
            overlays: (0..p.overlays.len())
                .map(|k| format!("/assets/o{k}.png"))
                .collect(),
            output: "/out/x.part".into(),
            palette: (p.ext == pa_core::VideoFormat::Gif).then(|| "/tmp/pal.png".into()),
            encoders,
        };
        let got = ve::build_video_edit_passes(&p, &io);
        check("passes", i, &res(&got), &c["passes"], spec);
        if got.is_ok() {
            passes += 1;
        }
        check(
            "atempo",
            i,
            &json!(ve::atempo_chain(p.speed)),
            &c["atempo"],
            spec,
        );
    }
    assert!(passes > 500, "{passes}");
}

#[test]
fn slideshow_matches_typescript() {
    let g = golden();
    let mut plans = 0;
    for (i, c) in g["slideshow"].as_array().unwrap().iter().enumerate() {
        let spec = &c["spec"];
        let norm = ss::normalize_slideshow_spec(spec);
        check("normalize", i, &res(&norm), &c["norm"], spec);
        let Ok(n) = norm else { continue };
        let sources: Vec<SlideSource> = c["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| match s["kind"].as_str().unwrap() {
                "image" => SlideSource::Image,
                _ => SlideSource::Video {
                    duration: s["duration"].as_f64().unwrap(),
                },
            })
            .collect();
        let draws: Vec<f64> = c["draws"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d.as_f64().unwrap())
            .collect();
        let mut k = 0;
        let mut rnd = || {
            let d = draws[k % draws.len()];
            k += 1;
            d
        };
        let plan = ss::plan_slideshow(&n, &sources, &mut rnd);
        check("plan", i, &res(&plan), &c["plan"], spec);
        let Ok(p) = plan else { continue };
        plans += 1;
        let input = |j: usize| -> PieceInput {
            let sl = &p.slides[j];
            if sl.kind == SlideKind::Video {
                PieceInput::Video {
                    path: format!("/m/v{j}.mp4"),
                }
            } else {
                let (width, height) = ss::canvas_size(&p, sl);
                PieceInput::Canvas {
                    path: format!("/t/slide-{j}.rgb"),
                    width,
                    height,
                }
            }
        };
        for (j, want) in c["pieces"].as_array().unwrap().iter().enumerate() {
            let r = ss::piece_range(&p, j);
            check(
                "range",
                i,
                &serde_json::to_value(&r).unwrap(),
                &want["range"],
                spec,
            );
            let mut inputs = vec![input(j)];
            if r.xfade.is_some() {
                inputs.push(input(j + 1));
            }
            let (args, frames) = ss::build_piece_args(&p, j, &inputs).unwrap();
            check(
                "piece",
                i,
                &json!({ "args": args, "frames": frames }),
                &want["piece"],
                spec,
            );
            let (w, h) = ss::canvas_size(&p, &p.slides[j]);
            check(
                "canvas",
                i,
                &json!({ "width": w, "height": h }),
                &want["canvas"],
                spec,
            );
        }
        let want_enc = &c["encoder"];
        let music = want_enc
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a == "/m/song.mp3");
        let enc = ss::build_encoder_args(&p, "/o/x.part", music.then_some("/m/song.mp3"));
        check("encoder", i, &json!(enc), want_enc, spec);
        check(
            "frameBytes",
            i,
            &json!(ss::frame_bytes(p.width, p.height)),
            &c["frameBytes"],
            spec,
        );
        check(
            "solid",
            i,
            &json!(ss::solid_frame(4, 2, &p.background).unwrap()),
            &c["solid"],
            spec,
        );
    }
    assert!(plans > 500, "{plans}");
}

#[test]
fn helpers_match_typescript() {
    let g = golden();
    for c in g["fx"].as_array().unwrap() {
        let (x, d) = (c["x"].as_f64().unwrap(), c["d"].as_u64().unwrap() as usize);
        assert_eq!(
            pa_jobs::js::f(x, d),
            c["s"].as_str().unwrap(),
            "f({x}, {d})"
        );
    }
    for c in g["kb"].as_array().unwrap() {
        let m: Value = c["m"].clone();
        let mv = ss::KenBurnsMove {
            zoom_from: m["zoomFrom"].as_f64().unwrap(),
            zoom_to: m["zoomTo"].as_f64().unwrap(),
            x_from: m["xFrom"].as_f64().unwrap(),
            x_to: m["xTo"].as_f64().unwrap(),
            y_from: m["yFrom"].as_f64().unwrap(),
            y_to: m["yTo"].as_f64().unwrap(),
        };
        let (z, x, y) = ss::ken_burns_expr(
            &mv,
            c["from"].as_i64().unwrap(),
            c["total"].as_i64().unwrap(),
        );
        assert_eq!(json!({ "z": z, "x": x, "y": y }), c["e"]);
    }
    for c in g["colors"].as_array().unwrap() {
        let a = &c["a"];
        let adj = ve::Adjust {
            brightness: a["brightness"].as_f64().unwrap(),
            contrast: a["contrast"].as_f64().unwrap(),
            saturation: a["saturation"].as_f64().unwrap(),
            hue: a["hue"].as_f64().unwrap(),
            warmth: a["warmth"].as_f64().unwrap(),
            vignette: a["vignette"].as_f64().unwrap(),
        };
        assert_eq!(json!(ve::color_filters(&adj)), c["out"], "{a}");
    }
}
