//! Parity with the TypeScript server's metadata extraction (exifr + sharp + Takeout sidecars +
//! groupColumns). tests/golden-meta.json was produced by running packages/server/src/media/metadata.ts on
//! every file of the fixture library, real camera samples and crafted EXIF / XMP / Takeout cases
//! (tests/files/*; see PARITY.md for how to regenerate it).

use std::path::{Path, PathBuf};

use pa_core::{MediaKind, ProbeResult};
use pa_meta::*;
use serde_json::Value;

fn here() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn golden() -> Value {
    serde_json::from_slice(
        &std::fs::read(here().join("tests/golden-meta.json")).expect("golden-meta.json"),
    )
    .expect("json")
}

fn to_v<T: serde::Serialize>(t: &T) -> Value {
    serde_json::from_str(&serde_json::to_string(t).expect("ser")).expect("reparse")
}

fn same(a: &Value, b: &Value, path: &str, diffs: &mut Vec<String>) {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (
                x.as_f64().unwrap_or(f64::NAN),
                y.as_f64().unwrap_or(f64::NAN),
            );
            if (x - y).abs() > 1e-9 * x.abs().max(y.abs()).max(1.0) {
                diffs.push(format!("{path}: rust {x} != ts {y}"));
            }
        }
        (Value::Object(x), Value::Object(y)) => {
            for (k, p) in x {
                match y.get(k) {
                    Some(q) => same(p, q, &format!("{path}.{k}"), diffs),
                    None if p.is_null() => {}
                    None => diffs.push(format!("{path}.{k}: only in Rust ({p})")),
                }
            }
            for (k, q) in y {
                if !x.contains_key(k) && !q.is_null() {
                    diffs.push(format!("{path}.{k}: only in TS ({q})"));
                }
            }
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => {
            for (i, (p, q)) in x.iter().zip(y).enumerate() {
                same(p, q, &format!("{path}[{i}]"), diffs);
            }
        }
        _ if a == b => {}
        _ => diffs.push(format!("{path}: rust {a} != ts {b}")),
    }
}

fn kind_of(s: &str) -> MediaKind {
    serde_json::from_value(Value::String(s.into())).expect("kind")
}

fn root_dir(root: &str) -> PathBuf {
    here().join("tests/files").join(root)
}

#[test]
fn extract_meta_matches_typescript() {
    let g = golden();
    let geo = pa_core::decode_geo_asset(
        &std::fs::read(here().join("../../packages/web/public/geo/cities.bin")).expect("geo"),
    )
    .expect("geo index");
    let zones: Vec<String> = serde_json::from_value(g["zones"].clone()).expect("zones");
    let mut failures = vec![];
    let mut n = 0;
    for f in g["files"].as_array().expect("files") {
        let rel = f["rel"].as_str().expect("rel");
        let kind = kind_of(f["kind"].as_str().expect("kind"));
        let ext = f["ext"].as_str().expect("ext");
        let abs = root_dir(f["root"].as_str().expect("root")).join(rel);
        if kind == MediaKind::Image && !abs.exists() {
            failures.push(format!("{rel}: test file missing"));
            continue;
        }
        n += 1;
        let probe: Option<ProbeResult> = serde_json::from_value(f["probe"].clone()).expect("probe");
        let mut diffs = vec![];
        clear_sidecar_cache();
        let m = extract_meta(&abs, kind, ext, probe.as_ref(), None);
        same(&to_v(&m), &f["meta"], "meta", &mut diffs);
        clear_sidecar_cache();
        let k = extract_meta(
            &abs,
            kind,
            ext,
            probe.as_ref(),
            Some(Known {
                width: Some(100.0),
                height: Some(50.0),
            }),
        );
        same(&to_v(&k), &f["known"], "known", &mut diffs);
        if kind == MediaKind::Image {
            let ex = read_exif(&abs, "UTC");
            same(&to_v(&ex), &f["exif"], "exif", &mut diffs);
        }
        let row: RowInfo = serde_json::from_value(f["row"].clone()).expect("row");
        // group columns from the TS-extracted meta, so a meta difference does not hide a grouping one
        let ts_meta: ExtractedMeta = serde_json::from_value(f["meta"].clone()).expect("meta");
        for z in &zones {
            let gc = group_columns(&row, &ts_meta, Some(&geo), z);
            same(
                &to_v(&gc),
                &f["group"][z],
                &format!("group[{z}]"),
                &mut diffs,
            );
        }
        if !diffs.is_empty() {
            failures.push(format!(
                "{}/{rel}: {}",
                f["root"].as_str().unwrap_or(""),
                diffs.join("; ")
            ));
        }
    }
    for f in &failures {
        eprintln!("{f}");
    }
    assert!(
        failures.is_empty(),
        "{} of {n} files differ from the TypeScript extraction",
        failures.len()
    );
    assert!(n > 150);
}

#[test]
fn video_tags_match_typescript() {
    let g = golden();
    let mut bad = vec![];
    for p in g["probes"].as_array().expect("probes") {
        let probe: Option<ProbeResult> = serde_json::from_value(p["probe"].clone()).expect("probe");
        let out = video_group_meta(probe.as_ref());
        let mut d = vec![];
        same(&to_v(&out), &p["out"], "", &mut d);
        if !d.is_empty() {
            bad.push(format!("{}: {}", p["probe"]["tags"], d.join("; ")));
        }
    }
    for l in g["locations"].as_array().expect("locations") {
        let got = parse_video_location(l["s"].as_str().unwrap_or(""));
        let mut d = vec![];
        same(&to_v(&got), &l["out"], "", &mut d);
        if !d.is_empty() {
            bad.push(format!("{}: {}", l["s"], d.join("; ")));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// Mirrors the video-tag cases of packages/server/test/shared-maps.test.ts
#[test]
fn video_location_tags() {
    let ll = |s: &str| parse_video_location(s).map(|l| (l.lat, l.lon));
    assert_eq!(ll("+41.0370+028.9850/"), Some((41.037, 28.985)));
    assert_eq!(ll("41.0370, 28.9850"), Some((41.037, 28.985)));
    assert_eq!(ll("-33.86 151.21 12.0"), Some((-33.86, 151.21)));
    assert_eq!(ll("nowhere"), None);
    let probe = |tags: &[(&str, &str)]| {
        let p = ProbeResult {
            tags: tags
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
        };
        let m = video_group_meta(Some(&p));
        (m.lat, m.lon)
    };
    assert_eq!(
        probe(&[(
            "com.apple.quicktime.location.iso6709",
            "+40.9903+029.0290+012.000/"
        )]),
        (Some(40.9903), Some(29.029))
    );
    assert_eq!(
        probe(&[("location-eng", "+36.8841+030.7056/")]),
        (Some(36.8841), Some(30.7056))
    );
    assert_eq!(
        probe(&[("com.samsung.android.location", "41.0082,28.9784")]),
        (Some(41.0082), Some(28.9784))
    );
    assert_eq!(
        probe(&[("com.vendor.gps.position", "+48.8566+002.3522/")]),
        (Some(48.8566), Some(2.3522))
    );
    assert_eq!(probe(&[("title", "x")]), (None, None));
}

#[test]
fn reads_from_memory_too() {
    // the readers work on bytes (no file system needed, e.g. for uploads or remote ranges)
    let bytes = std::fs::read(here().join("tests/files/craft/exif_full.jpg")).expect("file");
    let m = image_meta(&bytes, "jpg", false);
    assert_eq!((m.width, m.height), (Some(48.0), Some(64.0)));
    assert_eq!(m.offset, Some(180));
    assert_eq!(m.make.as_deref(), Some("NIKON CORPORATION"));
    let ex = exifr::read_exif(&bytes, "Europe/Istanbul").expect("exif");
    assert_eq!(
        ex.get("Flash"),
        Some(&pa_core::ExifValue::Str(
            "Flash did not fire, compulsory flash mode".into()
        ))
    );
    assert_eq!(
        ex.get("ExposureTime"),
        Some(&pa_core::ExifValue::Num(0.004))
    );
    // the wall clock 2024-07-01 23:30 read in the process zone (+03:00)
    assert_eq!(
        ex.get("DateTimeOriginal"),
        Some(&pa_core::ExifValue::Num(1_719_865_800_000.0))
    );
    assert!(Path::new(&here()).exists());
}
