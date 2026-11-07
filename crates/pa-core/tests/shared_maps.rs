//! Mirrors packages/server/test/shared-maps.test.ts (the pa-core parts; the video tag tests live in pa-meta).

use pa_core::*;

const IST_LAT: f64 = 41.0082;
const IST_LON: f64 = 28.9784;

fn p(iso: &str) -> f64 {
    let r = parse_date_time_with_offset(iso).expect("iso");
    (r.local - r.offset.unwrap_or(0) as i64 * 60_000) as f64
}

fn utc(y: i64, m0: i64, d: i64, h: i64, mi: i64) -> i64 {
    js::date_utc(y, m0, d, h, mi, 0, 0)
}

#[test]
fn sun_matches_known_solar_altitudes() {
    assert!((sun_altitude(p("2024-06-21T10:05:00Z"), IST_LAT, IST_LON) - 72.4).abs() < 0.5);
    assert!((sun_altitude(p("2024-12-21T10:08:00Z"), IST_LAT, IST_LON) - 25.5).abs() < 0.5);
    assert!(sun_altitude(p("2024-06-21T21:05:00Z"), IST_LAT, IST_LON) < -20.0);
    assert!(sun_altitude(p("2024-03-20T12:07:00Z"), 0.0, 0.0) > 88.0);
}

#[test]
fn sunrise_and_sunset_are_golden() {
    let rise = sun_altitude(p("2024-06-21T02:32:00Z"), IST_LAT, IST_LON);
    let set = sun_altitude(p("2024-06-21T17:39:00Z"), IST_LAT, IST_LON);
    assert!((rise + 0.8).abs() < 1.2);
    assert!((set + 0.8).abs() < 1.2);
    assert_eq!(time_of_day_of(rise), TimeOfDay::Golden);
    assert_eq!(time_of_day_of(3.0), TimeOfDay::Golden);
    assert_eq!(time_of_day_of(30.0), TimeOfDay::Day);
    assert_eq!(time_of_day_of(-6.0), TimeOfDay::Twilight);
    assert_eq!(time_of_day_of(-20.0), TimeOfDay::Night);
}

#[test]
fn day_times() {
    let at = |h| utc(2024, 0, 1, h, 0);
    assert_eq!(day_time_of(Some(-30.0), at(12), true), Some(DayTime::Night));
    assert_eq!(day_time_of(Some(2.0), at(12), true), Some(DayTime::Golden));
    assert_eq!(day_time_of(Some(40.0), at(23), true), None);
    assert_eq!(day_time_of(None, at(23), true), Some(DayTime::Night));
    assert_eq!(day_time_of(None, at(3), true), Some(DayTime::Night));
    assert_eq!(day_time_of(None, at(12), true), None);
    assert_eq!(day_time_of(None, at(23), false), None);
}

#[test]
fn estimates_the_instant_abroad() {
    let local = utc(2024, 6, 1, 20, 0);
    let home = local - 180 * 60_000;
    assert_eq!(estimate_instant(local, home, 29.0), home);
    assert_eq!(estimate_instant(local, home, -74.0), local + 5 * 3_600_000);
}

#[test]
fn colour_buckets() {
    assert_eq!(color_bucket_of(0.85, 0.15, 0.12), ColorBucket::Red);
    assert_eq!(color_bucket_of(0.9, 0.5, 0.1), ColorBucket::Orange);
    assert_eq!(color_bucket_of(0.9, 0.82, 0.2), ColorBucket::Yellow);
    assert_eq!(color_bucket_of(0.2, 0.6, 0.25), ColorBucket::Green);
    assert_eq!(color_bucket_of(0.2, 0.35, 0.8), ColorBucket::Blue);
    assert_eq!(color_bucket_of(0.5, 0.25, 0.75), ColorBucket::Purple);
    assert_eq!(color_bucket_of(0.9, 0.3, 0.6), ColorBucket::Pink);
    assert_eq!(color_bucket_of(0.5, 0.5, 0.52), ColorBucket::Bw);
    assert_eq!(color_bucket_of(0.04, 0.02, 0.03), ColorBucket::Bw);
    assert_eq!(color_bucket_of(0.98, 0.97, 0.96), ColorBucket::Bw);
}

#[test]
fn reads_the_average_colour_from_a_real_thumbhash() {
    // rgbaToThumbHash(16, 16, solid rgb(40, 90, 210)) from the `thumbhash` npm package
    let b64 = "nJMBBwBaaTt3eHeHeMeHV4eKAK+Hh3AI";
    let head = base64_head(b64, 6).unwrap();
    assert_eq!(head, vec![156, 147, 1, 7, 0, 90]);
    let avg = thumbhash_average(&head).unwrap();
    assert!(avg.b > avg.r);
    assert_eq!(
        COLOR_BUCKETS[color_index_of_thumbhash(Some(b64)) as usize],
        ColorBucket::Blue
    );
    assert_eq!(color_index_of_thumbhash(None), -1);
    assert_eq!(color_index_of_thumbhash(Some("!!")), -1);
}

#[test]
fn takeout_sidecar_names() {
    assert_eq!(
        parse_sidecar_name("IMG_1.jpg.json"),
        Some(SidecarKey {
            target: "IMG_1.jpg".into(),
            dup: 0,
            truncated: false
        })
    );
    assert_eq!(
        parse_sidecar_name("IMG_1.jpg.supplemental-metadata.json").map(|k| (k.target, k.dup)),
        Some(("IMG_1.jpg".into(), 0))
    );
    assert_eq!(
        parse_sidecar_name("IMG_1.jpg.supplemental-metadata(2).json").map(|k| (k.target, k.dup)),
        Some(("IMG_1.jpg".into(), 2))
    );
    assert_eq!(
        parse_sidecar_name("IMG_1.jpg(1).json").map(|k| (k.target, k.dup)),
        Some(("IMG_1.jpg".into(), 1))
    );
    assert_eq!(
        parse_sidecar_name("IMG_20190705_121212345_HDR.jpg.supplemental-me.json")
            .map(|k| (k.target, k.truncated)),
        Some(("IMG_20190705_121212345_HDR.jpg".into(), true))
    );
    assert_eq!(
        parse_sidecar_name("metadata.json").map(|k| k.target),
        Some("metadata".into())
    );
    assert_eq!(parse_sidecar_name("photo.jpg"), None);
}

#[test]
fn takeout_media_name_variants() {
    assert_eq!(
        media_name_variants("IMG_1(1).jpg"),
        NameVariants {
            names: vec!["IMG_1.jpg".into()],
            dup: 1
        }
    );
    assert_eq!(
        media_name_variants("IMG_1-edited.jpg"),
        NameVariants {
            names: vec!["IMG_1-edited.jpg".into(), "IMG_1.jpg".into()],
            dup: 0
        }
    );
    assert!(media_name_variants("IMG_1-düzenlendi.jpg")
        .names
        .contains(&"IMG_1.jpg".to_string()));
}

#[test]
fn takeout_matches_files_to_sidecars() {
    let dir = [
        "IMG_1.jpg.json",
        "IMG_1.jpg(1).json",
        "IMG_2.jpg.supplemental-metadata.json",
        "IMG_3.jpg.supplemental-metadata(1).json",
        "Screenshot_20190101-120000_Some Long App Na.json",
        "IMG_20190705_121212345_HDR.jpg.supplemental-me.json",
        "IMG_9.HEIC.json",
        "metadata.json",
    ];
    let m = |n: &str| match_sidecar(n, dir.iter().copied());
    assert_eq!(m("IMG_1.jpg"), Some("IMG_1.jpg.json"));
    assert_eq!(m("IMG_1(1).jpg"), Some("IMG_1.jpg(1).json"));
    assert_eq!(m("IMG_1-edited.jpg"), Some("IMG_1.jpg.json"));
    assert_eq!(m("IMG_2.jpg"), Some("IMG_2.jpg.supplemental-metadata.json"));
    assert_eq!(
        m("IMG_3(1).jpg"),
        Some("IMG_3.jpg.supplemental-metadata(1).json")
    );
    assert_eq!(m("IMG_3.jpg"), None);
    assert_eq!(
        m("Screenshot_20190101-120000_Some Long App Name.jpg"),
        Some("Screenshot_20190101-120000_Some Long App Na.json")
    );
    assert_eq!(
        m("IMG_20190705_121212345_HDR.jpg"),
        Some("IMG_20190705_121212345_HDR.jpg.supplemental-me.json")
    );
    assert_eq!(m("IMG_9.MP4"), Some("IMG_9.HEIC.json"));
    assert_eq!(m("IMG_4.jpg"), None);
    assert_eq!(
        match_sidecar("IMG_1.jpeg", ["IMG_1.jpg.json", "IMG_1.jpeg.json"]),
        Some("IMG_1.jpeg.json")
    );
}

#[test]
fn takeout_json() {
    use serde_json::json;
    let t = |v| parse_takeout_json(&v);
    assert_eq!(
        t(
            json!({"photoTakenTime": {"timestamp": "1562235010"}, "geoData": {"latitude": 37.9715, "longitude": 23.7267}})
        ),
        Some(TakeoutMeta {
            taken_at: Some(1_562_235_010_000),
            lat: Some(37.9715),
            lon: Some(23.7267)
        })
    );
    let r = t(json!({"photoTakenTime": {"timestamp": "1562235010"}, "geoData": {"latitude": 0, "longitude": 0}, "geoDataExif": {"latitude": 41, "longitude": 29}})).unwrap();
    assert_eq!((r.lat, r.lon), (Some(41.0), Some(29.0)));
    assert_eq!(
        t(
            json!({"photoTakenTime": {"timestamp": "1562235010"}, "geoData": {"latitude": 0, "longitude": 0}})
        ),
        Some(TakeoutMeta {
            taken_at: Some(1_562_235_010_000),
            lat: None,
            lon: None
        })
    );
    assert_eq!(t(json!({"title": "x"})), None);
    assert_eq!(t(json!(null)), None);
}

#[test]
fn gps_strings_in_every_common_form() {
    let g = |s: &str| parse_gps_coordinate(GpsValue::Text(s), None);
    let close = |a: Option<f64>, b: f64, eps: f64| {
        assert!((a.expect("value") - b).abs() < eps, "{a:?} != {b}")
    };
    close(g("41,0.738N"), 41.0123, 1e-4);
    close(g("28,58.476E"), 28.9746, 1e-4);
    close(g("33,51,54.5S"), -33.86514, 1e-4);
    close(g("70,30.0W"), -70.5, 1e-6);
    close(g("41.0123"), 41.0123, 1e-6);
    assert_eq!(g("-12.5"), Some(-12.5));
    assert_eq!(
        parse_gps_coordinate(GpsValue::Text("12.5"), Some("S")),
        Some(-12.5)
    );
    close(
        parse_gps_coordinate(GpsValue::Parts(&[41.0, 0.0, 44.28]), Some("N")),
        41.0123,
        1e-4,
    );
    assert_eq!(
        parse_gps_coordinate(GpsValue::Num(28.97), Some("W")),
        Some(-28.97)
    );
    close(g("41°0'44.28\"N"), 41.0123, 1e-4);
    assert_eq!(g("N 41.5"), Some(41.5));
    assert_eq!(g("41,75.0N"), None);
    assert_eq!(g("abc"), None);
    assert_eq!(g(""), None);
    assert_eq!(parse_gps_coordinate(GpsValue::Other, None), None);
}

#[test]
fn messaging_app_files() {
    assert!(is_messaging_name("IMG-20240101-WA0001.jpg", ""));
    assert!(is_messaging_name("VID-20240101-WA0012.mp4", ""));
    assert!(is_messaging_name(
        "WhatsApp Image 2024-01-01 at 12.00.00.jpeg",
        ""
    ));
    assert!(is_messaging_name("photo_2024-01-01_12-00-00.jpg", ""));
    assert!(is_messaging_name("x.jpg", "Telegram/Telegram Images"));
    assert!(!is_messaging_name("IMG_20240101_120000.jpg", ""));
}

#[test]
fn tiles_styles_bounds_and_urls() {
    assert_eq!(tile_style_of(Some("carto")), Some(MapTileStyle::Osm));
    assert_eq!(tile_style_of(Some("topo")), Some(MapTileStyle::Topo));
    assert_eq!(tile_style_of(Some("off")), None);
    let v = |s, z: f64, x: f64, y: f64| is_valid_tile(s, z, x, y);
    assert!(v(MapTileStyle::Osm, 0.0, 0.0, 0.0));
    assert!(v(MapTileStyle::Osm, 3.0, 7.0, 7.0));
    assert!(!v(MapTileStyle::Osm, 3.0, 8.0, 0.0));
    assert!(!v(MapTileStyle::Osm, 20.0, 0.0, 0.0));
    assert!(!v(MapTileStyle::Topo, 18.0, 0.0, 0.0));
    assert!(!v(MapTileStyle::Osm, 2.0, -1.0, 0.0));
    assert!(!v(MapTileStyle::Osm, 2.5, 0.0, 0.0));
    assert_eq!(
        upstream_tile_url(MapTileStyle::Osm, 3, 4, 2),
        "https://tile.openstreetmap.org/3/4/2.png"
    );
    assert_eq!(
        upstream_tile_url(MapTileStyle::Satellite, 3, 4, 2),
        "https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/3/2/4"
    );
    let t = upstream_tile_url(MapTileStyle::Topo, 3, 4, 2);
    assert!(["a", "b", "c"]
        .iter()
        .any(|s| t == format!("https://{s}.tile.opentopomap.org/3/4/2.png")));
}

#[test]
fn web_mercator_tile_of_a_point() {
    let t = tile_of(0.0, 0.0, 1);
    assert_eq!((t.x, t.y), (1, 1));
    let t = tile_of(IST_LAT, IST_LON, 14);
    assert_eq!((t.x, t.y), (9510, 6142));
    assert!((0.0..256.0).contains(&t.px));
}

fn row(k: i64) -> InsightRow {
    InsightRow {
        id: k.to_string(),
        kind: "image".into(),
        local_at: js::date_utc(2024, k % 12, 1 + (k % 27), 12, 0, 0, 0),
        size: 1000 + k,
        width: Some(400.0),
        height: Some(300.0),
        flags: 0,
        mtime: 1_700_000_000_000 + k,
        ..Default::default()
    }
}

#[test]
fn orientation_and_video_length_buckets() {
    assert_eq!(
        orientation_of(Some(300.0), Some(400.0), 0),
        Some(Orientation::Portrait)
    );
    assert_eq!(
        orientation_of(Some(400.0), Some(300.0), 0),
        Some(Orientation::Landscape)
    );
    assert_eq!(
        orientation_of(Some(400.0), Some(390.0), 0),
        Some(Orientation::Square)
    );
    assert_eq!(
        orientation_of(Some(4000.0), Some(1000.0), 4),
        Some(Orientation::Panorama)
    );
    assert_eq!(orientation_of(None, Some(100.0), 0), None);
    assert_eq!(video_length_of(Some(3.0)), Some(VideoLength::Short));
    assert_eq!(video_length_of(Some(60.0)), None);
    assert_eq!(video_length_of(Some(600.0)), Some(VideoLength::Long));
    assert_eq!(video_length_of(None), None);
}

#[test]
fn aggregates_stats_countries_cities_facets_and_rankings() {
    let mut rows = vec![];
    for k in 0..40i64 {
        let gps = k % 3 != 0;
        let tr = k % 2 == 1;
        let mut r = row(k);
        r.kind = if k % 10 == 0 {
            "video".into()
        } else {
            "image".into()
        };
        r.duration = if k % 10 == 0 {
            Some(if k == 0 { 5.0 } else { 400.0 })
        } else {
            None
        };
        r.width = Some(if k % 4 == 0 { 300.0 } else { 400.0 });
        r.height = Some(if k % 4 == 0 { 400.0 } else { 300.0 });
        if gps {
            r.lat = Some(41.0);
            r.lon = Some(29.0);
            r.cc = Some(if tr { "TR" } else { "IT" }.into());
            r.place_key = Some(if tr { "TR-34" } else { "IT-16" }.into());
            r.city = Some(if tr { "Kadıköy" } else { "Firenze" }.into());
        }
        r.color = Some(k % 8);
        r.sun = Some(if k % 5 == 0 {
            -20.0
        } else if k % 5 == 1 {
            2.0
        } else {
            40.0
        });
        r.added_at = if k == 7 {
            Some(9_000_000_000_000)
        } else {
            None
        };
        rows.push(r);
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.local_at));
    let r = compute_insights(&rows, &Default::default());
    let s = r.stats.clone().unwrap();
    assert_eq!(
        (
            s.photos,
            s.videos,
            s.countries,
            s.cities,
            s.first_year,
            s.last_year,
            s.with_location
        ),
        (36, 4, 2, 2, Some(2024), Some(2024), 26)
    );
    assert_eq!(r.no_location, Some(14));
    let mut ccs: Vec<String> = r
        .countries
        .as_ref()
        .unwrap()
        .iter()
        .map(|c| c.cc.clone())
        .collect();
    ccs.sort();
    assert_eq!(ccs, vec!["IT", "TR"]);
    let k = r
        .cities
        .as_ref()
        .unwrap()
        .iter()
        .find(|c| c.name == "Kadıköy")
        .unwrap();
    assert_eq!(
        (k.key.as_str(), k.place_key.as_str()),
        ("TR-34|Kadıköy", "TR-34")
    );
    let dt = r.day_times.as_ref().unwrap();
    assert_eq!(
        dt.iter().map(|g| g.value).collect::<Vec<_>>(),
        vec![DayTime::Night, DayTime::Golden]
    );
    assert_eq!(dt[0].count, 8);
    assert_eq!(r.colors.as_ref().unwrap().len(), 8);
    assert_eq!(
        r.orientations
            .as_ref()
            .unwrap()
            .iter()
            .find(|g| g.value == Orientation::Portrait)
            .unwrap()
            .count,
        10
    );
    let vl = r.video_lengths.as_ref().unwrap();
    assert_eq!(
        vl.iter().map(|g| (g.value, g.count)).collect::<Vec<_>>(),
        vec![(VideoLength::Short, 1), (VideoLength::Long, 3)]
    );
    let mut sizes: Vec<i64> = rows.iter().map(|x| x.size).collect();
    sizes.sort_by(|a, b| b.cmp(a));
    assert_eq!(
        r.largest
            .as_ref()
            .unwrap()
            .iter()
            .map(|x| x.value)
            .collect::<Vec<_>>(),
        sizes[..20].to_vec()
    );
    assert_eq!(r.recent.as_ref().unwrap()[0].id, "7");
    assert_eq!(r.calendar_years, Some(vec![2024]));
    assert_eq!(r.year_reviews.as_ref().unwrap().len(), 1);
    assert!(compute_insights(
        &rows,
        &InsightOptions {
            colors: Some(false),
            day_times: Some(false),
            ..Default::default()
        }
    )
    .colors
    .is_none());
}

#[test]
fn stats_cloud_bytes() {
    let r = |id: &str, size: i64, cloud: Option<bool>| InsightRow {
        id: id.into(),
        kind: "image".into(),
        local_at: js::date_utc(2024, 0, 1, 0, 0, 0, 0),
        size,
        width: Some(10.0),
        height: Some(10.0),
        cloud,
        ..Default::default()
    };
    let s = compute_insights(
        &[
            r("1", 100, None),
            r("2", 250, Some(true)),
            r("3", 50, Some(false)),
            r("4", 1000, Some(true)),
        ],
        &Default::default(),
    )
    .stats
    .unwrap();
    assert_eq!((s.bytes, s.cloud_bytes), (1400, Some(1250)));
    assert_eq!(
        compute_insights(&[r("1", 100, None)], &Default::default())
            .stats
            .unwrap()
            .cloud_bytes,
        Some(0)
    );
}

#[test]
fn year_in_review_spreads_covers() {
    let mut items: Vec<(String, i64)> = (0..60)
        .map(|k| {
            (
                format!("i{k}"),
                js::date_utc(2023, k % 12, 1 + (k % 27), 0, 0, 0, 0),
            )
        })
        .collect();
    items.sort_by_key(|i| std::cmp::Reverse(i.1));
    let refs: Vec<(&str, i64)> = items.iter().map(|(a, b)| (a.as_str(), *b)).collect();
    let m = year_review(2023, &refs, YEAR_REVIEW_MIN).unwrap();
    assert_eq!(
        (m.id.as_str(), m.kind, m.count, m.from, m.to),
        (
            "year-2023",
            MemoryKind::Year,
            60,
            js::date_utc(2023, 0, 1, 0, 0, 0, 0),
            js::date_utc(2024, 0, 1, 0, 0, 0, 0)
        )
    );
    assert_eq!(m.months, Some(vec![5; 12]));
    let months: std::collections::HashSet<i64> = m
        .cover_ids
        .iter()
        .map(|id| js::utc_parts(items.iter().find(|i| &i.0 == id).unwrap().1).month0)
        .collect();
    assert_eq!(months.len(), 12);
    assert!(year_review(2023, &refs[..5], YEAR_REVIEW_MIN).is_none());
}

#[test]
fn calendar_of_a_leap_year() {
    let c = calendar_of(
        2024,
        [
            utc(2024, 0, 1, 5, 0),
            utc(2024, 0, 1, 23, 0),
            utc(2024, 11, 31, 12, 0),
            utc(2023, 11, 31, 0, 0),
        ],
    );
    assert_eq!(c.days.len(), 366);
    assert_eq!(c.days[0], 2);
    assert_eq!(c.days[365], 1);
    assert_eq!((c.total, c.max), (3, 2));
    assert_eq!(calendar_day_key(2024, 59), "2024-02-29");
    assert_eq!(calendar_of(2023, []).days.len(), 365);
}
