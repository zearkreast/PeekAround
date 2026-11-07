//! Mirrors packages/server/test/slideshow.test.ts.

use pa_core::SlideTransition;
use pa_jobs::slideshow::*;
use pa_jobs::HttpError;
use serde_json::{json, Value};

fn spec(extra: Value) -> Value {
    let mut s = json!({ "items": [{ "id": "1" }, { "id": "2" }, { "id": "3" }], "duration": 2, "transition": "fade", "transitionDuration": 0.5,
        "kenBurns": false, "width": 1280, "height": 720, "fps": 30, "background": "#000" });
    for (k, v) in extra.as_object().unwrap() {
        s[k] = v.clone();
    }
    s
}

fn plan_r(
    extra: Value,
    sources: Option<Vec<SlideSource>>,
    rnd: &mut dyn FnMut() -> f64,
) -> Result<SlideshowPlan, HttpError> {
    let n = normalize_slideshow_spec(&spec(extra))?;
    let s = sources.unwrap_or_else(|| n.items.iter().map(|_| SlideSource::Image).collect());
    plan_slideshow(&n, &s, rnd)
}

fn plan(extra: Value) -> SlideshowPlan {
    plan_r(extra, None, &mut || 0.5).unwrap()
}

fn plan_s(extra: Value, s: Vec<SlideSource>) -> SlideshowPlan {
    plan_r(extra, Some(s), &mut || 0.5).unwrap()
}

fn e400<T: std::fmt::Debug>(r: Result<T, HttpError>, msg: &str) {
    let e = r.unwrap_err();
    assert_eq!(e.status, 400);
    assert!(e.message.contains(msg), "{} !~ {msg}", e.message);
}

fn canvas(p: &SlideshowPlan, i: usize) -> PieceInput {
    let (width, height) = canvas_size(p, &p.slides[i]);
    PieceInput::Canvas {
        path: format!("/tmp/s{i}.rgb"),
        width,
        height,
    }
}

const IMG: SlideSource = SlideSource::Image;

#[test]
fn normalizes() {
    let n =
        normalize_slideshow_spec(&json!({ "items": [{ "id": 5 }], "width": 1279, "height": 721 }))
            .unwrap();
    assert_eq!(
        n.items[0],
        SlideItem {
            id: "5".into(),
            duration: None,
            trim: None
        }
    );
    assert_eq!(
        (
            n.duration,
            n.transition,
            n.fps,
            n.background.as_str(),
            n.name.as_str()
        ),
        (4.0, SlideTransition::Fade, 30.0, "#000000", "Slayt")
    );
    assert_eq!((n.audio_id, n.target), (None, None));
    assert_eq!((n.width % 2.0, n.height % 2.0), (0.0, 0.0));
    assert_eq!(
        normalize_slideshow_spec(&spec(json!({ "name": "Tatil: 2024/Yaz.mp4" })))
            .unwrap()
            .name,
        "Tatil_ 2024_Yaz"
    );
    e400(normalize_slideshow_spec(&Value::Null), "");
    e400(
        normalize_slideshow_spec(&spec(json!({ "items": [] }))),
        "items",
    );
    e400(
        normalize_slideshow_spec(&spec(json!({ "items": [{ "id": "x/../1" }] }))),
        "id",
    );
    e400(
        normalize_slideshow_spec(&spec(json!({ "transition": "wobble" }))),
        "transition",
    );
    e400(
        normalize_slideshow_spec(&spec(json!({ "fps": 500 }))),
        "fps",
    );
    e400(
        normalize_slideshow_spec(&spec(json!({ "width": 10 }))),
        "width",
    );
    e400(
        normalize_slideshow_spec(&spec(json!({ "duration": "Infinity" }))),
        "duration",
    );
    e400(
        normalize_slideshow_spec(&spec(json!({ "background": "red" }))),
        "background",
    );
    e400(
        normalize_slideshow_spec(&spec(
            json!({ "items": [{ "id": "1", "trim": { "start": 4, "end": 2 } }] }),
        )),
        "trim",
    );
    assert_eq!(normalize_hex(Some(&json!("#AbC"))).unwrap(), "#aabbcc");
    assert_eq!(normalize_hex(Some(&json!("112233"))).unwrap(), "#112233");
}

#[test]
fn plans() {
    let p = plan(json!({}));
    assert_eq!(
        p.slides.iter().map(|s| s.frames).collect::<Vec<_>>(),
        [60, 60, 60]
    );
    assert_eq!(
        p.cuts,
        vec![
            CutPlan {
                frames: 15,
                transition: Some("fade".into())
            };
            2
        ]
    );
    assert_eq!(p.total_frames, 150);
    assert!((p.duration - 5.0).abs() < 1e-9);
    let mut start = 0;
    for i in 0..p.cuts.len() {
        let r = piece_range(&p, i);
        let off = (start + r.frames - r.xfade.as_ref().unwrap().frames) as f64 / p.fps;
        assert!((off - ((i + 1) as f64 * 2.0 - (i + 1) as f64 * 0.5)).abs() < 1e-6);
        start += r.frames;
    }
    let p = plan(
        json!({ "items": [{ "id": "1", "duration": 0.4 }, { "id": "2" }, { "id": "3", "duration": 1 }], "transitionDuration": 2 }),
    );
    assert_eq!(
        p.slides.iter().map(|s| s.frames).collect::<Vec<_>>(),
        [12, 60, 30]
    );
    assert_eq!(p.cuts.iter().map(|c| c.frames).collect::<Vec<_>>(), [6, 15]);
    assert_eq!(p.total_frames, 81);
    let p = plan(json!({ "transition": "none" }));
    assert!(p
        .cuts
        .iter()
        .all(|c| c.frames == 0 && c.transition.is_none()));
    assert_eq!(p.total_frames, 180);
    assert!(piece_range(&p, 1).xfade.is_none());
    for (ours, theirs) in [
        ("fade", "fade"),
        ("slide", "slideleft"),
        ("zoom", "zoomin"),
        ("blur", "hblur"),
        ("flip", "squeezev"),
        ("circle", "circleopen"),
        ("kenburns", "fade"),
    ] {
        assert_eq!(
            plan(json!({ "transition": ours })).cuts[0]
                .transition
                .as_deref(),
            Some(theirs)
        );
    }
    let seq = [0.0, 0.0, 0.5, 0.99];
    let mut k = 0;
    let p = plan_r(json!({ "transition": "random", "items": (1..=5).map(|i| json!({ "id": i.to_string() })).collect::<Vec<_>>() }), None, &mut || {
        let v = seq[k % 4];
        k += 1;
        v
    })
    .unwrap();
    let names: Vec<String> = p
        .cuts
        .iter()
        .map(|c| c.transition.clone().unwrap())
        .collect();
    assert!(names
        .iter()
        .all(|n| RANDOM_TRANSITIONS.contains(&n.as_str())));
    assert!(names.windows(2).all(|w| w[0] != w[1]));

    let p = plan(json!({ "transition": "kenburns" }));
    assert_eq!(
        p.slides
            .iter()
            .map(|s| s.ken_burns.unwrap())
            .collect::<Vec<_>>(),
        KEN_BURNS_MOVES[..3]
    );
    assert_eq!(p.kb_scale, 2.0);
    assert_eq!(canvas_size(&p, &p.slides[0]), (2560, 1440));
    assert_eq!(
        plan(json!({ "width": 3840, "height": 2160, "kenBurns": true })).kb_scale,
        2.0
    );
    assert_eq!(
        plan(json!({ "width": 4096, "height": 2160, "kenBurns": true })).kb_scale,
        1.0
    );
    assert_eq!(plan(json!({})).kb_scale, 1.0);

    let p = plan_s(
        json!({ "items": [{ "id": "1" }, { "id": "2", "trim": { "start": 1.5, "end": 4 } }, { "id": "3" }] }),
        vec![
            IMG,
            SlideSource::Video { duration: 10.0 },
            SlideSource::Video { duration: 3.0 },
        ],
    );
    assert_eq!(
        (
            p.slides[1].kind,
            p.slides[1].frames,
            p.slides[1].video_start,
            p.slides[1].ken_burns
        ),
        (SlideKind::Video, 75, 1.5, None)
    );
    assert_eq!((p.slides[2].frames, p.slides[2].video_start), (90, 0.0));
    e400(
        plan_r(
            json!({ "items": [{ "id": "1", "trim": { "start": 9.99, "end": 12 } }] }),
            Some(vec![SlideSource::Video { duration: 10.0 }]),
            &mut || 0.0,
        ),
        "short",
    );
}

#[test]
fn piece_args() {
    let p = plan(json!({}));
    let (args, frames) = build_piece_args(&p, 1, &[canvas(&p, 1), canvas(&p, 2)]).unwrap();
    assert_eq!(frames, 45);
    let s = args.join(" ");
    assert!(
        s.contains("-f rawvideo -pix_fmt rgb24 -video_size 1280x720 -framerate 30 -i /tmp/s1.rgb")
    );
    assert!(s.contains("-i /tmp/s2.rgb"));
    assert!(s.contains("tpad=stop_mode=clone:stop=-1,trim=end_frame=45[a]"));
    assert!(s.contains("trim=end_frame=15[b]"));
    assert!(s.contains("[a][b]xfade=transition=fade:duration=0.5:offset=1[v]"));
    assert!(s.contains("-frames:v 45"));
    assert!(s.ends_with("-f rawvideo -pix_fmt yuv420p pipe:1"));
    let (args, frames) = build_piece_args(&p, 2, &[canvas(&p, 2)]).unwrap();
    assert_eq!(frames, 45);
    assert!(!args.join(" ").contains("xfade"));

    let p = plan(json!({ "kenBurns": true }));
    let s = build_piece_args(&p, 1, &[canvas(&p, 1), canvas(&p, 2)])
        .unwrap()
        .0
        .join(" ");
    for want in [
        "-video_size 2560x1440",
        "format=yuv444p,zoompan=",
        "d=45:s=1280x720:fps=30",
        "(on+15)/59",
        "d=15:s=1280x720:fps=30",
    ] {
        assert!(s.contains(want), "{want}");
    }
    let (z, x, y) = ken_burns_expr(
        &KenBurnsMove {
            zoom_from: 1.0,
            zoom_to: 1.2,
            x_from: 0.5,
            x_to: 0.5,
            y_from: 0.0,
            y_to: 1.0,
        },
        10,
        101,
    );
    assert_eq!(z, "(1+0.2*((on+10)/100))");
    assert_eq!(x, "(iw-iw/zoom)*0.5");
    assert_eq!(y, "(ih-ih/zoom)*(0+1*((on+10)/100))");

    let p = plan_s(
        json!({ "items": [{ "id": "1", "trim": { "start": 2, "end": 5 } }, { "id": "2" }], "background": "#102030" }),
        vec![SlideSource::Video { duration: 10.0 }, IMG],
    );
    let s = build_piece_args(
        &p,
        0,
        &[
            PieceInput::Video {
                path: "/v/clip.mp4".into(),
            },
            canvas(&p, 1),
        ],
    )
    .unwrap()
    .0
    .join(" ");
    assert!(s.contains("-ss 2 -i /v/clip.mp4"));
    assert!(s.contains(
        "[0:V:0]setpts=PTS-STARTPTS,scale=1280:720:force_original_aspect_ratio=decrease"
    ));
    assert!(s.contains("pad=1280:720:(ow-iw)/2:(oh-ih)/2:color=0x102030"));
    assert!(s.contains("-an"));
    let p2 = plan_s(
        json!({ "items": [{ "id": "1" }, { "id": "2", "trim": { "start": 2, "end": 5 } }] }),
        vec![IMG, SlideSource::Video { duration: 10.0 }],
    );
    assert!(build_piece_args(
        &p2,
        1,
        &[PieceInput::Video {
            path: "/v/clip.mp4".into()
        }]
    )
    .unwrap()
    .0
    .join(" ")
    .contains("-ss 2.5 -i /v/clip.mp4"));

    let p = plan(
        json!({ "items": [{ "id": "1", "duration": 1 }, { "id": "2" }, { "id": "3", "duration": 3 }, { "id": "4" }], "transitionDuration": 0.7 }),
    );
    assert_eq!(
        (0..p.slides.len())
            .map(|i| piece_range(&p, i).frames)
            .sum::<i64>(),
        p.total_frames
    );
}

#[test]
fn encoder_and_solid_frame() {
    let p = plan(json!({}));
    let a = build_encoder_args(&p, "/out/s.mp4.part", Some("/m/song.mp3")).join(" ");
    for want in [
        "-f rawvideo -pix_fmt yuv420p -video_size 1280x720 -framerate 30 -i pipe:0",
        "-stream_loop -1 -i /m/song.mp3",
        "[1:a:0]atrim=end=5,asetpts=PTS-STARTPTS,afade=t=out:st=3:d=2[a]",
        "-c:v libx264",
        "-c:a aac",
        "-movflags +faststart -f mp4 /out/s.mp4.part",
    ] {
        assert!(a.contains(want), "{want}");
    }
    let a = build_encoder_args(&p, "/o.mp4", None);
    assert!(!a.join(" ").contains("-c:a") && a.contains(&"0:v".to_string()));
    let black = solid_frame(4, 2, "#000000").unwrap();
    assert_eq!(black.len() as u64, frame_bytes(4.0, 2.0));
    assert_eq!(black, [vec![16u8; 8], vec![128u8; 4]].concat());
    assert_eq!(
        solid_frame(2, 2, "#fff").unwrap(),
        [235, 235, 235, 235, 128, 128]
    );
    let red = solid_frame(2, 2, "#ff0000").unwrap();
    assert_eq!((red[0], red[4], red[5]), (63, 102, 240));
}
