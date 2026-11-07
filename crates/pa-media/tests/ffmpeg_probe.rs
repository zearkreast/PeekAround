//! Mirrors packages/server/test/ffmpeg-probe.test.ts: the `ffmpeg -i` parser must give what
//! ffprobe gives (samples captured with ffmpeg 6.1 next to the ffprobe JSON of the same file).

use pa_media::ffmpeg::{
    js_date_parse, parse_ffmpeg_info, probe_from_json, Ffmpeg, ProbeOptions, ProbeResult,
};
use std::path::{Path, PathBuf};

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ffmpeg-info")
}

fn samples() -> Vec<PathBuf> {
    let mut v: Vec<_> = std::fs::read_dir(dir())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "txt"))
        .collect();
    v.sort();
    v
}

fn parse(name: &str) -> ProbeResult {
    parse_ffmpeg_info(&std::fs::read_to_string(dir().join(name)).unwrap()).unwrap()
}

#[test]
fn has_samples() {
    assert!(samples().len() >= 10);
}

#[test]
fn matches_ffprobe_for_every_sample() {
    for f in samples() {
        let parsed = parse_ffmpeg_info(&std::fs::read_to_string(&f).unwrap())
            .unwrap_or_else(|| panic!("{f:?}"));
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(f.with_extension("json")).unwrap())
                .unwrap();
        let reference = probe_from_json(&json);
        // ffmpeg prints the duration with 2 decimals and bit rates in kb/s
        assert!(
            (parsed.duration.unwrap_or(0.0) - reference.duration.unwrap_or(0.0)).abs() < 0.011,
            "{f:?}"
        );
        assert!(
            (parsed.bitrate.unwrap_or(0.0) - reference.bitrate.unwrap_or(0.0)).abs() < 1000.0,
            "{f:?}"
        );
        let strip = |p: &ProbeResult| ProbeResult {
            duration: None,
            bitrate: None,
            ..p.clone()
        };
        assert_eq!(strip(&parsed), strip(&reference), "{f:?}");
    }
}

#[test]
fn reads_rotation_quicktime_and_android_tags() {
    let rot = parse("rot90.mp4.txt");
    assert_eq!(rot.rotation, 90);
    assert_eq!((rot.width, rot.height), (Some(240), Some(320)));
    let mov = parse("apple.mov.txt");
    assert_eq!(
        mov.tags["com.apple.quicktime.location.iso6709"],
        "+39.9208+032.8541+0938.000/"
    );
    assert_eq!(
        mov.tags["com.apple.quicktime.creationdate"],
        "2024-07-03T21:00:00+0300"
    );
    assert_eq!(mov.tags["com.apple.quicktime.model"], "iPhone 15");
    assert_eq!(
        parse("android.mp4.txt").tags["com.android.manufacturer"],
        "Google"
    );
    let mkv = parse("hevc.mkv.txt");
    assert_eq!(mkv.codec.as_deref(), Some("hevc"));
    assert_eq!(mkv.fps, Some(29.97));
    let avi = parse("noaudio.avi.txt");
    assert!(!avi.has_audio);
    assert_eq!(avi.audio_codec, None);
}

const EDGE: &str = "Input #0, mov,mp4,m4a,3gp,3g2,mj2, from 'IMG_0001.MOV':
  Metadata:
    creation_time   : 2023-05-01T10:00:00.000000Z
    comment         : line one
                    : line two
  Duration: N/A, start: 0.000000, bitrate: N/A
  Stream #0:0(eng): Video: hevc (Main) (hvc1 / 0x31637668), yuv420p(tv, bt709), 1920x1080, 10000 kb/s, 29.98 fps, 30 tbr, 600 tbn (default)
    Metadata:
      rotate          : 90
      creation_time   : 2023-05-01T10:00:00.000000Z
  Stream #0:1(eng): Audio: aac (LC) (mp4a / 0x6134706D), 44100 Hz, stereo, fltp, 128 kb/s (default)
  Stream #0:2: Video: mjpeg (Baseline), yuvj420p(pc), 320x240, 90k tbr, 90k tbn (attached pic)
  Stream #0:3[0x4](und): Video: h264, yuv420p, 1280x720, 1k fps, 1k tbr
      Side data:
        displaymatrix: rotation of -90.00 degrees
At least one output file must be specified";

#[test]
fn handles_edge_cases_of_other_ffmpeg_versions() {
    let p = parse_ffmpeg_info(EDGE).unwrap();
    assert_eq!(p.duration, None);
    assert_eq!(p.bitrate, None);
    assert_eq!(p.codec.as_deref(), Some("hevc"));
    assert_eq!(p.rotation, 90); // rotate tag (old ffmpeg)
    assert_eq!((p.width, p.height), (Some(1080), Some(1920)));
    assert_eq!(p.fps, Some(29.98));
    assert_eq!(p.tags["comment"], "line one\nline two");
    assert_eq!(p.tags["language"], "eng");
    assert_eq!(p.creation_time, js_date_parse("2023-05-01T10:00:00Z"));
    assert_eq!(p.audio_codec.as_deref(), Some("aac"));
    // side data of a later stream does not leak into the first video stream
    let no_rotate = parse_ffmpeg_info(&EDGE.replace("      rotate          : 90\n", "")).unwrap();
    assert_eq!(no_rotate.rotation, 0);
    assert_eq!(
        (no_rotate.width, no_rotate.height),
        (Some(1920), Some(1080))
    );
    assert_eq!(
        parse_ffmpeg_info("garbage\n/x: No such file or directory"),
        None
    );
    assert_eq!(parse_ffmpeg_info(""), None);
    // CRLF output (Windows builds)
    assert_eq!(
        parse_ffmpeg_info(&EDGE.replace('\n', "\r\n")).unwrap().tags["comment"],
        "line one\nline two"
    );
}

fn fixture_video() -> Option<PathBuf> {
    let lib = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/library/Videolar");
    let lib = if lib.exists() {
        lib
    } else {
        PathBuf::from("/home/user/peekaround/fixtures/library/Videolar")
    };
    std::fs::read_dir(&lib)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "mp4"))
}

#[tokio::test]
async fn probe_falls_back_to_the_parser_without_ffprobe() {
    let Some(file) = fixture_video() else { return };
    let no_probe = Ffmpeg::new("ffmpeg", Some(dir().join("no-such-ffprobe")));
    no_probe.check().await;
    assert!(!no_probe.probe_available());
    if !no_probe.available() {
        return; // no ffmpeg on this machine
    }
    let f = file.to_string_lossy();
    let a = no_probe.probe(&f, &ProbeOptions::default()).await.unwrap();
    assert_eq!(a.codec.as_deref(), Some("h264"));
    assert!(a.duration.unwrap() > 0.0);
    let with_probe = Ffmpeg::new("ffmpeg", Some("ffprobe".into()));
    with_probe.check().await;
    if with_probe.probe_available() {
        let b = with_probe
            .probe(&f, &ProbeOptions::default())
            .await
            .unwrap();
        assert_eq!(
            (
                a.width,
                a.height,
                a.rotation,
                a.creation_time,
                a.tags.get("location")
            ),
            (
                b.width,
                b.height,
                b.rotation,
                b.creation_time,
                b.tags.get("location")
            )
        );
    }
    let missing = file.with_file_name("missing.mp4");
    assert_eq!(
        no_probe
            .probe(&missing.to_string_lossy(), &ProbeOptions::default())
            .await,
        None
    );
}
