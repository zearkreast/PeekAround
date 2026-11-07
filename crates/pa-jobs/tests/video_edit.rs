//! Mirrors packages/server/test/video-edit.test.ts.

use pa_jobs::video_edit::*;
use pa_jobs::HttpError;
use serde_json::{json, Value};
use std::collections::HashSet;

const ASSET: &str = "0b8f0a2e-1d0e-4a57-9a0e-2a9b8f3c1d11.png";

fn src() -> VideoSourceInfo {
    VideoSourceInfo {
        width: 1920.0,
        height: 1080.0,
        duration: 10.0,
        has_audio: true,
        fps: Some(30.0),
    }
}

fn all() -> HashSet<String> {
    [
        "libx264",
        "aac",
        "libvpx-vp9",
        "libvpx",
        "libopus",
        "libvorbis",
        "gif",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

fn spec(extra: Value) -> Value {
    let mut s = json!({ "sourceId": "7", "output": { "format": "mp4", "height": null, "quality": "medium" } });
    for (k, v) in extra.as_object().unwrap() {
        s[k] = v.clone();
    }
    s
}

fn plan_with(extra: Value, src: VideoSourceInfo) -> Result<VideoEditPlan, HttpError> {
    plan_video_edit(&normalize_video_edit_spec(&spec(extra))?, &src)
}

fn plan(extra: Value) -> VideoEditPlan {
    plan_with(extra, src()).unwrap()
}

fn e400<T: std::fmt::Debug>(r: Result<T, HttpError>, msg: &str) {
    let e = r.unwrap_err();
    assert_eq!(e.status, 400);
    assert!(e.message.contains(msg), "{} !~ {msg}", e.message);
}

#[test]
fn fills_defaults() {
    let n = normalize_video_edit_spec(&json!({ "sourceId": "3", "output": { "format": "mp4", "height": null, "quality": "high" } })).unwrap();
    assert_eq!(
        (
            n.source_id.as_str(),
            n.trim,
            n.crop,
            n.rotate,
            n.speed,
            n.volume,
            n.mute
        ),
        ("3", None, None, 0, 1.0, 1.0, false)
    );
    assert!(n.overlays.is_empty());
    assert_eq!(n.adjust, Adjust::default());
}

#[test]
fn rejects_bad_input_with_400() {
    e400(normalize_video_edit_spec(&Value::Null), "");
    e400(
        normalize_video_edit_spec(&json!({ "output": {} })),
        "sourceId",
    );
    e400(
        normalize_video_edit_spec(&spec(json!({ "sourceId": "../etc" }))),
        "sourceId",
    );
    e400(
        normalize_video_edit_spec(&spec(json!({ "speed": 8 }))),
        "speed",
    );
    e400(
        normalize_video_edit_spec(&spec(json!({ "speed": "NaN" }))),
        "speed",
    );
    e400(
        normalize_video_edit_spec(&spec(json!({ "rotate": 45 }))),
        "rotate",
    );
    e400(
        normalize_video_edit_spec(&spec(json!({ "trim": { "start": 3, "end": 2 } }))),
        "trim",
    );
    e400(
        normalize_video_edit_spec(&spec(
            json!({ "crop": { "x": -1, "y": 0, "w": 10, "h": 10 } }),
        )),
        "crop",
    );
    e400(
        normalize_video_edit_spec(&spec(json!({ "adjust": { "hue": 500 } }))),
        "hue",
    );
    e400(
        normalize_video_edit_spec(&spec(
            json!({ "output": { "format": "avi", "height": null, "quality": "high" } }),
        )),
        "format",
    );
    e400(
        normalize_video_edit_spec(&spec(
            json!({ "overlays": [{ "assetId": "../../x.png", "start": 0, "end": 1 }] }),
        )),
        "PNG",
    );
}

#[test]
fn drops_overlays_that_are_never_visible() {
    let n = normalize_video_edit_spec(&spec(json!({ "overlays": [
        { "assetId": ASSET, "start": 1, "end": 1 }, { "assetId": ASSET, "start": 0, "end": 2 }
    ] })))
    .unwrap();
    assert_eq!(
        n.overlays,
        vec![Overlay {
            asset_id: ASSET.into(),
            start: 0.0,
            end: 2.0
        }]
    );
}

#[test]
fn plans() {
    let p = plan(json!({ "trim": { "start": 1, "end": 3 }, "speed": 2 }));
    assert_eq!(
        (p.start, p.end, p.trimmed, p.duration, p.width, p.height),
        (1.0, 3.0, true, 1.0, 1920.0, 1080.0)
    );
    let p = plan(json!({ "trim": { "start": 8, "end": 20 } }));
    assert_eq!((p.end, p.duration), (10.0, 2.0));
    e400(
        plan_with(json!({ "trim": { "start": 10, "end": 12 } }), src()),
        "beyond",
    );
    assert!(!plan(json!({ "trim": { "start": 0, "end": 10 } })).trimmed);

    let p = plan(json!({ "crop": { "x": 100, "y": 50, "w": 641, "h": 481 }, "rotate": 90 }));
    assert_eq!(
        p.crop,
        Some(Crop {
            x: 100.0,
            y: 50.0,
            w: 641.0,
            h: 481.0
        })
    );
    assert_eq!(
        (p.base_width, p.base_height, p.width, p.height),
        (481.0, 641.0, 480.0, 640.0)
    );
    let s = plan(
        json!({ "crop": { "x": 0, "y": 0, "w": 1280, "h": 720 }, "output": { "format": "mp4", "height": 360, "quality": "low" } }),
    );
    assert_eq!((s.width, s.height), (640.0, 360.0));

    let p = plan(json!({ "crop": { "x": 1800, "y": 1000, "w": 400, "h": 400 } }));
    assert_eq!(
        p.crop,
        Some(Crop {
            x: 1800.0,
            y: 1000.0,
            w: 120.0,
            h: 80.0
        })
    );
    e400(
        plan_with(
            json!({ "crop": { "x": 1920, "y": 0, "w": 10, "h": 10 } }),
            src(),
        ),
        "outside",
    );
    assert_eq!(
        plan(json!({ "crop": { "x": 0, "y": 0, "w": 1920, "h": 1080 } })).crop,
        None
    );

    assert!(
        !plan_with(
            json!({}),
            VideoSourceInfo {
                has_audio: false,
                ..src()
            }
        )
        .unwrap()
        .audio
    );
    assert!(!plan(json!({ "mute": true })).audio);
    assert!(
        !plan(json!({ "output": { "format": "gif", "height": 240, "quality": "high" } })).audio
    );
    assert!(plan(json!({})).audio);

    let gif = |q: &str| json!({ "format": "gif", "height": null, "quality": q });
    assert_eq!(plan(json!({ "output": gif("high") })).fps, Some(15.0));
    assert_eq!(plan(json!({ "output": gif("low") })).fps, Some(10.0));
    assert_eq!(
        plan_with(
            json!({ "output": gif("high") }),
            VideoSourceInfo {
                fps: Some(8.0),
                ..src()
            }
        )
        .unwrap()
        .fps,
        Some(8.0)
    );
    assert_eq!(
        plan_with(
            json!({ "speed": 0.5, "output": gif("high") }),
            VideoSourceInfo {
                fps: Some(24.0),
                ..src()
            }
        )
        .unwrap()
        .fps,
        Some(12.0)
    );
    assert_eq!(plan(json!({ "speed": 4 })).fps, Some(30.0));
    assert_eq!(plan(json!({ "speed": 0.5 })).fps, Some(15.0));
    assert_eq!(
        plan_with(
            json!({ "speed": 2 }),
            VideoSourceInfo { fps: None, ..src() }
        )
        .unwrap()
        .fps,
        None
    );
    assert_eq!(plan(json!({})).fps, None);

    e400(
        plan_with(
            json!({}),
            VideoSourceInfo {
                width: 0.0,
                ..src()
            },
        ),
        "",
    );
    e400(
        plan_with(
            json!({}),
            VideoSourceInfo {
                duration: 0.0,
                ..src()
            },
        ),
        "",
    );
}

#[test]
fn filters() {
    for s in [0.25, 0.3, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0] {
        let steps = atempo_chain(s);
        assert!(steps.iter().all(|x| (0.5 - 1e-9..=2.0 + 1e-9).contains(x)));
        assert!((steps.iter().product::<f64>() - s).abs() < 1e-9);
    }
    assert!(atempo_chain(1.0).is_empty());
    assert_eq!(atempo_chain(4.0), vec![2.0, 2.0]);
    assert_eq!(atempo_chain(0.25), vec![0.5, 0.5]);

    let p = plan(
        json!({ "crop": { "x": 10, "y": 20, "w": 640, "h": 480 }, "rotate": 270, "flipH": true, "flipV": true, "speed": 2,
        "output": { "format": "mp4", "height": 320, "quality": "medium" } }),
    );
    assert_eq!(
        geometry_filters(&p),
        [
            "crop=640:480:10:20:exact=1",
            "transpose=cclock",
            "hflip",
            "vflip",
            "scale=240:320:flags=bicubic",
            "setsar=1",
            "setpts=(PTS-STARTPTS)/2",
            "fps=30"
        ]
    );
    assert_eq!(
        geometry_filters(&plan(json!({ "rotate": 180 }))),
        ["hflip", "vflip", "setsar=1", "setpts=PTS-STARTPTS"]
    );
    assert_eq!(
        geometry_filters(&plan(json!({ "rotate": 90 })))[0],
        "transpose=clock"
    );

    assert!(color_filters(&Adjust::default()).is_empty());
    assert_eq!(
        color_filters(&Adjust {
            brightness: 0.5,
            contrast: -0.5,
            saturation: 1.0,
            hue: 30.0,
            warmth: 1.0,
            vignette: 0.5
        }),
        [
            "eq=brightness=0.15:contrast=0.5:saturation=2",
            "hue=h=30",
            "colortemperature=temperature=4000:pl=1",
            "vignette=angle=0.6283"
        ]
    );

    let p = plan(
        json!({ "speed": 2, "overlays": [{ "assetId": ASSET, "start": 0.5, "end": 1.25 }, { "assetId": ASSET, "start": 0, "end": 3 }],
        "output": { "format": "mp4", "height": 360, "quality": "medium" } }),
    );
    let g = video_graph(&p);
    assert!(g.starts_with("[0:V:0]"));
    assert!(g.contains("[1:v]scale=640:360,format=rgba[ov0]"));
    assert!(g.contains("[vb][ov0]overlay=0:0:enable='between(t,0.5,1.25)'[v0]"));
    assert!(g.contains("[v0][ov1]overlay=0:0:enable='between(t,0,3)'[v1]"));
    assert!(g.ends_with("[v1]null[vo]"));
    assert!(g.find("setpts").unwrap() < g.find("overlay").unwrap());

    assert_eq!(
        audio_graph(&plan(json!({ "speed": 4, "volume": 0.5 }))).unwrap(),
        "[0:a:0]asetpts=PTS-STARTPTS,atempo=2,atempo=2,volume=0.5[ao]"
    );
    assert_eq!(audio_graph(&plan(json!({ "mute": true }))), None);
}

fn io() -> VideoEditIo {
    VideoEditIo {
        input: "/in/clip.mov".into(),
        overlays: vec![],
        output: "/out/clip_edit.mp4.part".into(),
        palette: None,
        encoders: all(),
    }
}

#[test]
fn passes() {
    let pass = &build_video_edit_passes(&plan(json!({ "trim": { "start": 1, "end": 3 }, "output": { "format": "mp4", "height": null, "quality": "high" } })), &io()).unwrap()[0];
    let a = &pass.args;
    assert_eq!(&a[..7], ["-y", "-ss", "1", "-t", "2", "-i", "/in/clip.mov"]);
    let j = a.join(" ");
    assert!(j.contains("-c:v libx264 -preset medium -crf 18 -pix_fmt yuv420p"));
    assert!(j.contains("-c:a aac") && j.contains("-movflags +faststart -f mp4"));
    assert_eq!(a.last().unwrap(), "/out/clip_edit.mp4.part");
    assert!(a.contains(&"[ao]".to_string()));

    let low = &build_video_edit_passes(
        &plan_with(
            json!({ "output": { "format": "mp4", "height": null, "quality": "low" } }),
            VideoSourceInfo {
                has_audio: false,
                ..src()
            },
        )
        .unwrap(),
        &io(),
    )
    .unwrap()[0];
    assert!(
        low.args.join(" ").contains("-crf 28")
            && low.args.contains(&"-an".into())
            && !low.args.join(" ").contains("[0:a:0]")
    );

    let p = plan(json!({ "output": { "format": "webm", "height": null, "quality": "medium" } }));
    let vp9 = build_video_edit_passes(&p, &io()).unwrap()[0]
        .args
        .join(" ");
    assert!(
        vp9.contains("-c:v libvpx-vp9 -crf 32 -b:v 0")
            && vp9.contains("-c:a libopus")
            && vp9.contains("-f webm")
    );
    let vp8 = build_video_edit_passes(
        &p,
        &VideoEditIo {
            encoders: ["libvpx", "libvorbis"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ..io()
        },
    )
    .unwrap()[0]
        .args
        .join(" ");
    assert!(vp8.contains("-c:v libvpx -crf 10") && vp8.contains("-c:a libvorbis"));
    e400(
        pick_codecs(pa_core::VideoFormat::Webm, &["libx264".to_string()].into()),
        "WebM",
    );
    e400(
        pick_codecs(pa_core::VideoFormat::Mp4, &["libvpx".to_string()].into()),
        "H.264",
    );

    let p = plan(
        json!({ "overlays": [{ "assetId": ASSET, "start": 0, "end": 1 }], "output": { "format": "gif", "height": 240, "quality": "medium" } }),
    );
    let passes = build_video_edit_passes(
        &p,
        &VideoEditIo {
            overlays: vec!["/tmp/ov.png".into()],
            palette: Some("/tmp/pal.png".into()),
            output: "/out/x.gif.part".into(),
            ..io()
        },
    )
    .unwrap();
    assert_eq!(passes.len(), 2);
    assert!((passes[0].weight + passes[1].weight - 1.0).abs() < 1e-9);
    let (pal, enc) = (passes[0].args.join(" "), passes[1].args.join(" "));
    assert!(
        pal.contains("palettegen=stats_mode=diff")
            && pal.contains("fps=12")
            && pal.ends_with("/tmp/pal.png")
    );
    assert!(
        enc.contains("-i /in/clip.mov -i /tmp/ov.png -i /tmp/pal.png")
            && enc.contains("[vo][2:v]paletteuse=")
            && enc.contains("-an")
            && enc.contains("-f gif")
    );

    let p = plan(json!({ "overlays": [{ "assetId": ASSET, "start": 0, "end": 1 }] }));
    assert!(build_video_edit_passes(&p, &io()).is_err());
}
