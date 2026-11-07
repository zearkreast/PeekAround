//! Golden parity tests: every fixture in tests/golden/*.json was produced by running the TypeScript
//! reference (packages/shared) — see tests/tools/gen-core.ts. Each case is `{ args, out }`; the Rust port
//! must return the same value (floats within 1e-9 relative).

use std::path::Path;

use pa_core as c;
use serde_json::{json, Value};

fn golden_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// decoded argument (`{"$u":1}` = undefined, `{"$n":"NaN"}` = non-finite number)
fn is_undef(v: &Value) -> bool {
    v.get("$u").is_some()
}

fn num(v: &Value) -> f64 {
    if let Some(n) = v.get("$n").and_then(Value::as_str) {
        return match n {
            "Infinity" => f64::INFINITY,
            "-Infinity" => f64::NEG_INFINITY,
            _ => f64::NAN,
        };
    }
    v.as_f64().unwrap_or(f64::NAN)
}

fn i(v: &Value) -> i64 {
    num(v) as i64
}

fn opt_str(v: &Value) -> Option<&str> {
    v.as_str()
}

fn opt_num(v: &Value) -> Option<f64> {
    if v.is_null() || is_undef(v) {
        None
    } else {
        Some(num(v))
    }
}

fn to_v<T: serde::Serialize>(t: T) -> Value {
    // through the JS number formatter so integral floats compare like JSON.parse(JSON.stringify(x))
    serde_json::from_str(&serde_json::to_string(&t).expect("serialize")).expect("reparse")
}

fn opt<T: serde::Serialize>(o: Option<T>) -> Value {
    match o {
        Some(t) => to_v(t),
        None => Value::Null,
    }
}

fn gps_value(v: &Value) -> (Vec<f64>, u8) {
    match v {
        Value::Number(_) => (vec![num(v)], 0),
        Value::Array(a) => (a.iter().map(c::js::number_of).collect(), 1),
        Value::String(_) => (vec![], 2),
        _ => (vec![], 3),
    }
}

fn mem_items(v: &Value) -> Vec<c::MemoryItem> {
    serde_json::from_value(v.clone()).expect("memory items")
}

fn call(name: &str, a: &[Value], geo: &c::GeoIndex) -> Value {
    match name {
        // time
        "parseDateTimeWithOffset" => opt(a[0].as_str().and_then(c::parse_date_time_with_offset)),
        "parseOffset" => opt(a[0].as_str().and_then(c::parse_offset)),
        "isValidTimeZone" => json!(c::is_valid_time_zone(a[0].as_str().unwrap_or(""))),
        "tzOffsetAt" => json!(c::tz_offset_at(i(&a[0]), a[1].as_str().unwrap_or(""))),
        "instantToLocal" => json!(c::instant_to_local(i(&a[0]), a[1].as_str().unwrap_or(""))),
        "localToInstant" => json!(c::local_to_instant(i(&a[0]), a[1].as_str().unwrap_or(""))),
        "offsetFromGps" => opt(c::offset_from_gps(i(&a[0]), i(&a[1]))),
        "parseGpsDateTime" => {
            let Some(d) = a[0].as_str() else {
                return Value::Null;
            };
            let parts: Vec<f64>;
            let t = match &a[1] {
                Value::Array(x) => {
                    parts = x.iter().map(c::js::number_of).collect();
                    c::GpsTime::Parts(&parts)
                }
                Value::String(s) => c::GpsTime::Text(s),
                _ => return Value::Null,
            };
            opt(c::parse_gps_date_time(d, t))
        }
        "resolveFromLocal" => to_v(c::resolve_from_local(
            i(&a[0]),
            opt_num(&a[1]).map(|x| x as i32),
            a[2].as_str().unwrap_or(""),
        )),
        "resolveFromInstant" => to_v(c::resolve_from_instant(
            i(&a[0]),
            a[1].as_str().unwrap_or(""),
        )),
        "localDayKey" => json!(c::local_day_key(i(&a[0]))),
        "localDayStart" => json!(c::local_day_start(i(&a[0]))),
        // geo
        "geo.nearest" => opt(geo.nearest(num(&a[0]), num(&a[1]), a.get(2).map(num))),
        "geo.place" => {
            let id = i(&a[0]);
            opt(if id < 0 { None } else { geo.place(id as usize) })
        }
        "geo.admin1Name" => opt(geo.admin1_name(a[0].as_str().unwrap_or(""), a[1].as_str())),
        "geo.info" => json!({"size": geo.size(), "attribution": geo.attribution()}),
        "distanceKm" => to_v(c::js::JsF64(c::distance_km(
            num(&a[0]),
            num(&a[1]),
            num(&a[2]),
            num(&a[3]),
        ))),
        "isValidGps" => json!(c::is_valid_gps(num(&a[0]), num(&a[1]))),
        "parseIso6709" => opt(a[0].as_str().and_then(c::parse_iso6709)),
        "parseGpsCoordinate" => {
            let (p, kind) = gps_value(&a[0]);
            let v = match kind {
                0 => c::GpsValue::Num(p[0]),
                1 => c::GpsValue::Parts(&p),
                2 => c::GpsValue::Text(a[0].as_str().unwrap_or("")),
                _ => c::GpsValue::Other,
            };
            opt(c::parse_gps_coordinate(v, opt_str(&a[1])).map(c::js::JsF64))
        }
        // classify
        "isScreenshotName" => json!(c::is_screenshot_name(
            a[0].as_str().unwrap_or(""),
            a[1].as_str().unwrap_or("")
        )),
        "normalizeMake" => opt(c::normalize_make(a[0].as_str())),
        "normalizeCamera" => to_v(c::normalize_camera(a[0].as_str(), a[1].as_str())),
        "classify" => json!(c::classify(
            &serde_json::from_value(a[0].clone()).expect("classify input")
        )),
        "isMessagingName" => json!(c::is_messaging_name(
            a[0].as_str().unwrap_or(""),
            a[1].as_str().unwrap_or("")
        )),
        // search
        "foldText" => json!(c::fold_text(a[0].as_str())),
        "parseSearch" => to_v(c::parse_search(a[0].as_str().unwrap_or(""))),
        // memories
        "spreadPick" => {
            let ids: Vec<&str> = a[0]
                .as_array()
                .expect("list")
                .iter()
                .map(|x| x["id"].as_str().unwrap_or(""))
                .collect();
            to_v(c::spread_pick(&ids, i(&a[1]) as usize))
        }
        "onThisDay" => to_v(c::on_this_day(
            &mem_items(&a[0]),
            i(&a[1]),
            &serde_json::from_value(a[2].clone()).expect("opts"),
        )),
        // trips
        "homesByYear" => {
            let items: Vec<c::TripItem> = serde_json::from_value(a[0].clone()).expect("trip items");
            let m = c::homes_by_year(&items);
            json!({"$map": m.into_iter().map(|(y, h)| json!([y, to_v(h)])).collect::<Vec<_>>()})
        }
        "findTrips" => {
            let items: Vec<c::TripItem> = serde_json::from_value(a[0].clone()).expect("trip items");
            to_v(c::find_trips(
                &items,
                &serde_json::from_value(a[1].clone()).expect("opts"),
            ))
        }
        // takeout
        "parseSidecarName" => opt(c::parse_sidecar_name(a[0].as_str().unwrap_or(""))),
        "mediaNameVariants" => to_v(c::media_name_variants(a[0].as_str().unwrap_or(""))),
        "matchSidecar" => {
            let names: Vec<&str> = a[1]
                .as_array()
                .expect("names")
                .iter()
                .filter_map(Value::as_str)
                .collect();
            opt(c::match_sidecar(
                a[0].as_str().unwrap_or(""),
                names.iter().copied(),
            ))
        }
        "parseTakeoutJson" => opt(c::parse_takeout_json(&a[0])),
        // sun
        "sunAltitude" => to_v(c::js::JsF64(c::sun_altitude(
            num(&a[0]),
            num(&a[1]),
            num(&a[2]),
        ))),
        "timeOfDayOf" => to_v(c::time_of_day_of(num(&a[0]))),
        "isNightHour" => json!(c::is_night_hour(i(&a[0]))),
        "estimateInstant" => json!(c::estimate_instant(i(&a[0]), i(&a[1]), num(&a[2]))),
        // color
        "colorBucketOf" => to_v(c::color_bucket_of(num(&a[0]), num(&a[1]), num(&a[2]))),
        "rgbToHsl" => {
            let h = c::rgb_to_hsl(num(&a[0]), num(&a[1]), num(&a[2]));
            to_v(json!({"h": c::js::JsF64(h.h), "s": c::js::JsF64(h.s), "l": c::js::JsF64(h.l)}))
        }
        "thumbhashAverage" => {
            let b: Vec<u8> = a[0]
                .as_array()
                .expect("bytes")
                .iter()
                .map(|x| i(x) as u8)
                .collect();
            opt(c::thumbhash_average(&b).map(|r| json!({"r": c::js::JsF64(r.r), "g": c::js::JsF64(r.g), "b": c::js::JsF64(r.b), "a": c::js::JsF64(r.a)})))
        }
        "colorIndexOfThumbhash" => json!(c::color_index_of_thumbhash(a[0].as_str())),
        "base64Head" => opt(c::base64_head(
            a[0].as_str().unwrap_or(""),
            i(&a[1]) as usize,
        )),
        // insights
        "orientationOf" => opt(c::orientation_of(
            opt_num(&a[0]),
            opt_num(&a[1]),
            i(&a[2]) as u32,
        )),
        "videoLengthOf" => opt(c::video_length_of(opt_num(&a[0]))),
        "dayTimeOf" => opt(c::day_time_of(
            opt_num(&a[0]),
            i(&a[1]),
            a[2].as_bool().unwrap_or(false),
        )),
        "computeInsights" => {
            let rows: Vec<c::InsightRow> =
                serde_json::from_value(strip_undef(&a[0])).expect("rows");
            to_v(c::compute_insights(
                &rows,
                &serde_json::from_value(a[1].clone()).expect("opts"),
            ))
        }
        "yearReview" => {
            let items = mem_items(&a[1]);
            let pairs: Vec<(&str, i64)> =
                items.iter().map(|m| (m.id.as_str(), m.local_at)).collect();
            opt(c::year_review(
                i(&a[0]),
                &pairs,
                a.get(2)
                    .map(|x| i(x) as usize)
                    .unwrap_or(c::YEAR_REVIEW_MIN),
            ))
        }
        "calendarOf" => to_v(c::calendar_of(
            i(&a[0]),
            a[1].as_array().expect("times").iter().map(i),
        )),
        "calendarDayKey" => json!(c::calendar_day_key(i(&a[0]), i(&a[1]))),
        // tiles
        "tileStyleOf" => opt(c::tile_style_of(a[0].as_str())),
        "isValidTile" => json!(c::is_valid_tile(
            c::parse_tile_style(a[0].as_str().unwrap_or("")).expect("style"),
            num(&a[1]),
            num(&a[2]),
            num(&a[3])
        )),
        "upstreamTileUrl" => {
            json!(c::upstream_tile_url(
                c::parse_tile_style(a[0].as_str().unwrap_or("")).expect("style"),
                i(&a[1]),
                i(&a[2]),
                i(&a[3])
            ))
        }
        "tileOf" => to_v(c::tile_of(num(&a[0]), num(&a[1]), i(&a[2]) as u32)),
        "TILE_STYLES" => to_v(
            c::MAP_TILE_STYLES
                .iter()
                .map(|s| (s.as_str(), c::tile_style(*s)))
                .collect::<std::collections::BTreeMap<_, _>>(),
        ),
        // albums
        "normalizeLibraryOptions" => to_v(c::normalize_library_options(if is_undef(&a[0]) {
            None
        } else {
            Some(&a[0])
        })),
        "excludeMatcher" => {
            let ps: Vec<&str> = a[0]
                .as_array()
                .expect("patterns")
                .iter()
                .filter_map(Value::as_str)
                .collect();
            let m = c::exclude_matcher(&ps);
            let all: Vec<&str> = a[1]
                .as_array()
                .expect("paths")
                .iter()
                .filter_map(Value::as_str)
                .collect();
            json!({"empty": m.is_empty(), "path": all.iter().map(|p| m.path(p)).collect::<Vec<_>>(), "name": all.iter().map(|p| m.name(p)).collect::<Vec<_>>()})
        }
        // index helpers
        "aspectToByte" => json!(c::aspect_to_byte(num(&a[0]))),
        "byteToAspect" => to_v(c::js::JsF64(c::byte_to_aspect(i(&a[0]) as u8))),
        "extOf" => json!(c::ext_of(a[0].as_str().unwrap_or(""))),
        "kindOf" => opt(c::kind_of(a[0].as_str().unwrap_or(""))),
        "joinRel" => json!(c::join_rel(
            a[0].as_str().unwrap_or(""),
            a[1].as_str().unwrap_or("")
        )),
        "parentDir" => json!(c::parent_dir(a[0].as_str().unwrap_or(""))),
        "DEFAULT_SETTINGS" => to_v(c::Settings::default()),
        "constants" => to_v(json!({
            "APP_NAME": c::APP_NAME, "API_VERSION": c::API_VERSION,
            "MEDIA_FLAG": {"screenshot": c::media_flag::SCREENSHOT, "selfie": c::media_flag::SELFIE, "panorama": c::media_flag::PANORAMA, "animated": c::media_flag::ANIMATED},
            "IMAGE_EXT": c::IMAGE_EXT, "BROWSER_IMAGE_EXT": c::BROWSER_IMAGE_EXT, "VIDEO_EXT": c::VIDEO_EXT, "AUDIO_EXT": c::AUDIO_EXT,
            "ARCHIVE_EXT": c::ARCHIVE_EXT, "FONT_EXT": c::FONT_EXT, "COLOR_BUCKETS": c::COLOR_BUCKETS,
            "COLOR_SWATCH": c::COLOR_BUCKETS.iter().map(|b| (b.as_str(), b.swatch())).collect::<std::collections::BTreeMap<_, _>>(),
            "MAP_TILE_STYLES": c::MAP_TILE_STYLES, "ORIENTATIONS": c::ORIENTATIONS, "VIDEO_LENGTHS": c::VIDEO_LENGTHS, "DAY_TIMES": c::DAY_TIMES,
            "SHORT_VIDEO_S": c::js::JsF64(c::SHORT_VIDEO_S), "LONG_VIDEO_S": c::js::JsF64(c::LONG_VIDEO_S), "YEAR_REVIEW_MIN": c::YEAR_REVIEW_MIN,
            "GOLDEN_MIN": c::js::JsF64(c::GOLDEN_MIN), "GOLDEN_MAX": c::js::JsF64(c::GOLDEN_MAX), "NIGHT_MAX": c::js::JsF64(c::NIGHT_MAX),
            "TAKEOUT_MAX_NAME": c::TAKEOUT_MAX_NAME, "DEFAULT_REMOTE_CACHE_MB": c::DEFAULT_REMOTE_CACHE_MB,
            "DEFAULT_LIBRARY_OPTIONS": c::default_library_options(), "GEO_ASSET_PATH": c::GEO_ASSET_PATH, "GEO_MAX_KM": c::js::JsF64(c::GEO_MAX_KM),
            "DAY_MS": c::DAY_MS, "MESSAGING_SQL": c::MESSAGING_SQL,
        })),
        "localeCompare" => {
            json!(c::locale_compare(a[0].as_str().unwrap_or(""), a[1].as_str().unwrap_or("")) as i8)
        }
        _ => panic!("no Rust binding for golden function {name}"),
    }
}

/// drop `{"$u":1}` object properties (undefined fields)
fn strip_undef(v: &Value) -> Value {
    match v {
        Value::Array(a) => Value::Array(a.iter().map(strip_undef).collect()),
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(_, x)| !is_undef(x))
                .map(|(k, x)| (k.clone(), strip_undef(x)))
                .collect(),
        ),
        _ => v.clone(),
    }
}

/// structural equality, numbers within 1e-9 (relative)
pub fn same(a: &Value, b: &Value, path: &str, diffs: &mut Vec<String>) {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (
                x.as_f64().unwrap_or(f64::NAN),
                y.as_f64().unwrap_or(f64::NAN),
            );
            if (x - y)
                .abs()
                .partial_cmp(&(1e-9 * x.abs().max(y.abs()).max(1.0)))
                != Some(std::cmp::Ordering::Less)
                && x != y
            {
                diffs.push(format!("{path}: {x} != {y}"));
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                diffs.push(format!("{path}: len {} != {}", x.len(), y.len()));
                return;
            }
            for (k, (p, q)) in x.iter().zip(y).enumerate() {
                same(p, q, &format!("{path}[{k}]"), diffs);
            }
        }
        (Value::Object(x), Value::Object(y)) => {
            for (k, p) in x {
                match y.get(k) {
                    Some(q) => same(p, q, &format!("{path}.{k}"), diffs),
                    None => diffs.push(format!("{path}.{k}: only in Rust ({p})")),
                }
            }
            for (k, q) in y {
                if !x.contains_key(k) {
                    diffs.push(format!("{path}.{k}: only in TS ({q})"));
                }
            }
        }
        _ if a == b => {}
        _ => diffs.push(format!("{path}: {a} != {b}")),
    }
}

#[test]
fn golden_parity() {
    let raw = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/web/public/geo/cities.bin"),
    )
    .expect("geo asset");
    let geo = c::decode_geo_asset(&raw).expect("geo index");
    let mut files: Vec<_> = std::fs::read_dir(golden_dir())
        .expect("golden dir")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    files.retain(|p| p.extension().is_some_and(|e| e == "json"));
    files.sort();
    assert!(files.len() > 50, "golden fixtures missing");
    let mut total = 0;
    let mut failures = vec![];
    for f in &files {
        let name = f
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let cases: Vec<Value> =
            serde_json::from_slice(&std::fs::read(f).expect("read")).expect("json");
        for (k, case) in cases.iter().enumerate() {
            total += 1;
            let args = case["args"].as_array().expect("args");
            let want = &case["out"];
            if want.get("$err").is_some() {
                // the TypeScript reference throws here (e.g. computeInsights with ranked: 0); the port must not panic
                let _ = call(&name, args, &geo);
                continue;
            }
            let got = call(&name, args, &geo);
            // TS `undefined` results are encoded as {"$u":1}; the Rust port returns None → null
            let mut want = if is_undef(want) {
                Value::Null
            } else {
                want.clone()
            };
            if name == "homesByYear" {
                // a JS Map keeps insertion order; the port returns a BTreeMap (sorted by year)
                if let Some(Value::Array(m)) = want.get_mut("$map") {
                    m.sort_by_key(|e| e[0].as_i64());
                }
            }
            let got = if is_undef(&got) { Value::Null } else { got };
            let mut diffs = vec![];
            same(&got, &want, "", &mut diffs);
            if !diffs.is_empty() {
                failures.push(format!(
                    "{name}#{k} args={} :: {}",
                    Value::Array(args.clone()),
                    diffs.join("; ")
                ));
            }
        }
    }
    for f in failures.iter().take(60) {
        eprintln!("{f}");
    }
    assert!(
        failures.is_empty(),
        "{} of {total} golden cases differ",
        failures.len()
    );
    eprintln!("{total} golden cases OK");
}
