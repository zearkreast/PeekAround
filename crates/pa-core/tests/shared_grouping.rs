//! Mirrors packages/server/test/shared-grouping.test.ts

use pa_core::*;

const DAY: i64 = 86_400_000;

/// wall clock as UTC ms of "YYYY-MM-DDTHH:MM:SS(.mmm)"
fn l(iso: &str) -> i64 {
    let r = parse_date_time_with_offset(iso).expect("iso");
    r.local - r.offset.unwrap_or(0) as i64 * 60_000
}

/// instant of an ISO string with offset / Z
fn p(iso: &str) -> i64 {
    l(iso)
}

fn utc(y: i64, m0: i64, d: i64) -> i64 {
    js::date_utc(y, m0, d, 0, 0, 0, 0)
}

mod time {
    use super::*;

    #[test]
    fn parses_exif_strings_as_wall_clock() {
        assert_eq!(
            parse_exif_date_time("2024:07:01 23:30:00"),
            Some(l("2024-07-01T23:30:00"))
        );
        assert_eq!(
            parse_exif_date_time("2024-07-01T23:30:00.250"),
            Some(l("2024-07-01T23:30:00") + 250)
        );
        assert_eq!(
            parse_exif_date_time("2024:07:01"),
            Some(l("2024-07-01T00:00:00"))
        );
        assert_eq!(parse_exif_date_time("0000:00:00 00:00:00"), None);
        assert_eq!(parse_exif_date_time("garbage"), None);
        assert_eq!(
            parse_date_time_with_offset("2024-07-03T21:00:00+0300"),
            Some(LocalWithOffset {
                local: l("2024-07-03T21:00:00"),
                offset: Some(180)
            })
        );
    }

    #[test]
    fn parses_offsets() {
        assert_eq!(parse_offset("+03:00"), Some(180));
        assert_eq!(parse_offset("-05:30"), Some(-330));
        assert_eq!(parse_offset("+0545"), Some(345));
        assert_eq!(parse_offset("Z"), Some(0));
        assert_eq!(parse_offset("+20:00"), None);
    }

    #[test]
    fn computes_dst_correct_offsets() {
        assert_eq!(tz_offset_at(p("2024-01-15T12:00:00Z"), "Europe/Berlin"), 60);
        assert_eq!(
            tz_offset_at(p("2024-07-15T12:00:00Z"), "Europe/Berlin"),
            120
        );
        assert_eq!(
            tz_offset_at(p("2024-07-15T12:00:00Z"), "Europe/Istanbul"),
            180
        );
        assert_eq!(
            tz_offset_at(p("2024-07-15T12:00:00Z"), "America/New_York"),
            -240
        );
        assert_eq!(tz_offset_at(p("2024-07-15T12:00:00Z"), "Asia/Kolkata"), 330);
        assert_eq!(tz_offset_at(p("2024-03-31T00:59:00Z"), "Europe/Berlin"), 60);
        assert_eq!(
            tz_offset_at(p("2024-03-31T01:00:00Z"), "Europe/Berlin"),
            120
        );
    }

    #[test]
    fn round_trips_local_and_instant() {
        for tz in [
            "Europe/Istanbul",
            "Europe/Berlin",
            "America/Los_Angeles",
            "UTC",
        ] {
            for iso in [
                "2024-01-10T08:00:00Z",
                "2024-07-10T23:30:00Z",
                "2023-10-29T00:30:00Z",
            ] {
                let t = p(iso);
                assert_eq!(
                    local_to_instant(instant_to_local(t, tz), tz),
                    t,
                    "{tz} {iso}"
                );
            }
        }
        let r = resolve_from_local(l("2024-07-01T23:30:00"), Some(180), "UTC");
        assert_eq!(
            r,
            ResolvedTime {
                taken_at: p("2024-07-01T20:30:00Z"),
                local_at: l("2024-07-01T23:30:00"),
                tz_offset: Some(180)
            }
        );
        assert_eq!(
            resolve_from_local(l("2024-07-01T23:30:00"), None, "Europe/Berlin").taken_at,
            p("2024-07-01T21:30:00Z")
        );
        assert_eq!(
            resolve_from_instant(p("2025-12-31T21:30:00Z"), "Europe/Istanbul").local_at,
            l("2026-01-01T00:30:00")
        );
        assert_eq!(
            local_to_instant(l("2024-03-31T02:30:00"), "Europe/Berlin"),
            p("2024-03-31T01:30:00Z")
        );
        assert_eq!(
            local_to_instant(l("2024-10-27T02:30:00"), "Europe/Berlin"),
            p("2024-10-27T00:30:00Z")
        );
    }

    #[test]
    fn derives_the_offset_from_gps_time() {
        let gps = parse_gps_date_time("2024:07:01", GpsTime::Parts(&[20.0, 29.0, 47.0]));
        assert_eq!(gps, Some(p("2024-07-01T20:29:47Z")));
        assert_eq!(
            offset_from_gps(l("2024-07-01T23:30:00"), gps.unwrap()),
            Some(180)
        );
        assert_eq!(
            offset_from_gps(l("2024-07-01T02:15:00"), p("2024-06-30T20:30:00Z")),
            Some(345)
        );
        assert_eq!(
            offset_from_gps(l("2024-07-01T02:15:00"), p("2024-06-20T20:30:00Z")),
            None
        );
        assert_eq!(
            parse_gps_date_time("2024:07:01", GpsTime::Text("10:00:00")),
            Some(p("2024-07-01T10:00:00Z"))
        );
    }
}

mod geo {
    use super::*;

    fn index() -> GeoIndex {
        let raw = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../packages/web/public/geo/cities.bin"
        ))
        .expect("asset");
        decode_geo_asset(&raw).expect("index")
    }

    #[test]
    fn finds_turkish_districts_with_local_spelling() {
        let geo = index();
        let k = geo.nearest(40.9903, 29.029, None).unwrap();
        assert_eq!(
            (
                k.city.as_str(),
                k.admin1.as_deref(),
                k.cc.as_str(),
                k.key.as_str()
            ),
            ("Kadıköy", Some("İstanbul"), "TR", "TR-34")
        );
        let kizilay = geo.nearest(39.9208, 32.8541, None).unwrap();
        assert_eq!(
            (
                kizilay.admin1.as_deref(),
                kizilay.cc.as_str(),
                kizilay.key.as_str()
            ),
            (Some("Ankara"), "TR", "TR-68")
        );
        assert!(["Çankaya", "Ankara"].contains(&kizilay.city.as_str()));
        let a = geo.nearest(36.8969, 30.7133, None).unwrap();
        assert_eq!(
            (a.admin1.as_deref(), a.key.as_str()),
            (Some("Antalya"), "TR-07")
        );
    }

    #[test]
    fn works_abroad_and_rejects_the_ocean() {
        let geo = index();
        let b = geo.nearest(52.52, 13.405, None).unwrap();
        assert_eq!((b.cc.as_str(), b.admin1.as_deref()), ("DE", Some("Berlin")));
        assert!(["Mitte", "Berlin"].contains(&b.city.as_str()));
        assert!(geo.nearest(30.0, -40.0, None).is_none());
        assert!(geo.nearest(0.0, 0.0, None).is_none());
        assert!(geo.nearest(f64::NAN, 10.0, None).is_none());
        assert!(geo.attribution().contains("GeoNames"));
        assert!(GeoIndex::from_bytes(
            &std::fs::read(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../packages/web/public/geo/cities.bin"
            ))
            .unwrap()
        )
        .is_err());
    }

    #[test]
    fn respects_the_80_km_limit() {
        let geo = index();
        assert!(geo.nearest(40.9903, 29.029, None).unwrap().km < 5.0);
        assert!(geo.nearest(34.5, 32.5, Some(1.0)).is_none());
    }

    #[test]
    fn parses_iso6709_and_validates_gps() {
        assert_eq!(
            parse_iso6709("+41.0123+028.9744+050.000/"),
            Some(LatLon {
                lat: 41.0123,
                lon: 28.9744
            })
        );
        assert_eq!(
            parse_iso6709("-33.8688+151.2093/"),
            Some(LatLon {
                lat: -33.8688,
                lon: 151.2093
            })
        );
        assert!((parse_iso6709("+4100.738+02858.464/").unwrap().lat - 41.0123).abs() < 5e-4);
        assert_eq!(parse_iso6709("+00.0000+000.0000/"), None);
        assert_eq!(parse_iso6709("nope"), None);
        assert!(!is_valid_gps(91.0, 0.0));
        assert!(!is_valid_gps(0.0, 0.0));
        assert!(is_valid_gps(41.0, 29.0));
    }
}

mod classify_tests {
    use super::*;

    fn png(name: &str) -> ClassifyInput {
        ClassifyInput {
            ext: "png".into(),
            kind: "image".into(),
            width: Some(1080.0),
            height: Some(2340.0),
            name: name.into(),
            ..Default::default()
        }
    }

    #[test]
    fn detects_screenshots_by_name_and_folder() {
        for name in [
            "Screenshot_20240812-101500.png",
            "Screen Shot 2023-05-02 at 10.11.12.png",
            "Ekran Görüntüsü 2025-03-01 141230.png",
            "Ekran Resmi 2024-01-01 10.00.00.png",
            "Bildschirmfoto 2025-04-19 um 09.30.00.png",
            "Capture d’écran 2024-02-02 à 10.00.00.png",
            "Captura de pantalla 2024-02-02.png",
            "Скриншот 2024-02-02.png",
            "スクリーンショット 2024-02-02.png",
        ] {
            assert!(is_screenshot_name(name, ""), "{name}");
        }
        assert!(is_screenshot_name("IMG_0001.png", "DCIM/Screenshots"));
        assert!(is_screenshot_name("a.png", "Resimler/Ekran görüntüleri"));
        assert!(!is_screenshot_name("IMG_0001.jpg", "DCIM/Camera"));
        assert!(classify(&png("IMG_7781.png")) & media_flag::SCREENSHOT != 0);
        assert!(
            classify(&ClassifyInput {
                make: Some("Apple".into()),
                ..png("IMG_7781.png")
            }) & media_flag::SCREENSHOT
                == 0
        );
        assert_eq!(
            classify(&ClassifyInput {
                width: Some(400.0),
                height: Some(400.0),
                ..png("logo.png")
            }),
            0
        );
    }

    #[test]
    fn detects_selfies_panoramas_animations() {
        let jpg = ClassifyInput {
            ext: "jpg".into(),
            kind: "image".into(),
            name: "IMG_1.jpg".into(),
            width: Some(3000.0),
            height: Some(4000.0),
            make: Some("samsung".into()),
            model: Some("SM-G991B".into()),
            ..Default::default()
        };
        let with = |f: &dyn Fn(&mut ClassifyInput)| {
            let mut c = jpg.clone();
            f(&mut c);
            classify(&c)
        };
        assert_eq!(
            with(&|c| c.lens = Some("Samsung Galaxy S21 front camera 2.2mm f/2.2".into())),
            media_flag::SELFIE
        );
        assert_eq!(
            with(&|c| c.lens = Some("iPhone 13 front TrueDepth camera 2.71mm f/2.2".into())),
            media_flag::SELFIE
        );
        assert_eq!(
            with(&|c| c.name = "IMG_20240101_120000_selfie.jpg".into()),
            media_flag::SELFIE
        );
        assert_eq!(with(&|c| c.lens = Some("back camera".into())), 0);
        assert_eq!(
            with(&|c| (c.width, c.height) = (Some(9000.0), Some(3000.0))),
            media_flag::PANORAMA
        );
        assert_eq!(
            with(&|c| (c.width, c.height) = (Some(3000.0), Some(9000.0))),
            media_flag::PANORAMA
        );
        assert_eq!(
            with(&|c| {
                (c.width, c.height) = (Some(2000.0), Some(1000.0));
                c.gpano = Some(true);
            }),
            media_flag::PANORAMA
        );
        assert_eq!(
            with(&|c| {
                (c.width, c.height) = (Some(2000.0), Some(800.0));
                c.make = None;
                c.model = None;
            }),
            0
        );
        let k = |ext: &str, kind: &str, name: &str, pages: Option<f64>| {
            classify(&ClassifyInput {
                ext: ext.into(),
                kind: kind.into(),
                name: name.into(),
                pages,
                ..Default::default()
            })
        };
        assert_eq!(k("gif", "image", "a.gif", None), media_flag::ANIMATED);
        assert_eq!(
            k("webp", "image", "a.webp", Some(10.0)),
            media_flag::ANIMATED
        );
        assert_eq!(k("webp", "image", "a.webp", Some(1.0)), 0);
        assert_eq!(k("mp4", "video", "a.mp4", None), 0);
    }

    #[test]
    fn normalises_camera_names() {
        let c = |a: Option<&str>, b: Option<&str>| normalize_camera(a, b);
        assert_eq!(
            c(Some("NIKON CORPORATION"), Some("NIKON D750")),
            Camera {
                make: Some("Nikon".into()),
                model: Some("D750".into()),
                key: Some("nikon|d750".into())
            }
        );
        assert_eq!(
            c(Some("SAMSUNG"), Some("SM-G991B")),
            Camera {
                make: Some("Samsung".into()),
                model: Some("SM-G991B".into()),
                key: Some("samsung|sm-g991b".into())
            }
        );
        let pair = |a, b| {
            let r = c(Some(a), Some(b));
            (r.make.unwrap_or_default(), r.model.unwrap_or_default())
        };
        assert_eq!(
            pair("samsung", "Galaxy S21"),
            ("Samsung".into(), "Galaxy S21".into())
        );
        assert_eq!(
            pair("Canon", "Canon EOS R5"),
            ("Canon".into(), "EOS R5".into())
        );
        assert_eq!(
            pair("Apple", "iPhone 13"),
            ("Apple".into(), "iPhone 13".into())
        );
        assert_eq!(
            pair("OLYMPUS IMAGING CORP.", "E-M10"),
            ("Olympus".into(), "E-M10".into())
        );
        assert_eq!(pair("FOO ELECTRONICS", "X1"), ("Foo".into(), "X1".into()));
        assert_eq!(c(None, None), Camera::default());
    }
}

mod search_tests {
    use super::*;

    #[test]
    fn folds_turkish_case_and_diacritics() {
        for s in ["ISTANBUL", "istanbul", "İstanbul", "İSTANBUL", "ıstanbul"] {
            assert_eq!(fold_text(Some(s)), "istanbul");
        }
        assert_eq!(
            fold_text(Some("Kadıköy ŞİŞLİ Çağlayan Göztepe Ümraniye")),
            "kadikoy sisli caglayan goztepe umraniye"
        );
        assert_eq!(fold_text(Some("Düsseldorf Straße")), "dusseldorf strasse");
    }

    #[test]
    fn extracts_dates_months_and_types() {
        assert_eq!(
            parse_search("istanbul 2023 video"),
            ParsedSearch {
                text: "istanbul".into(),
                terms: vec!["istanbul".into()],
                flag: Some(MediaFlagName::Video),
                from: Some(utc(2023, 0, 1)),
                to: Some(utc(2024, 0, 1)),
                month: None
            }
        );
        let a = parse_search("Ağustos 2024");
        assert_eq!(
            (a.terms.len(), a.from, a.to),
            (0, Some(utc(2024, 7, 1)), Some(utc(2024, 8, 1)))
        );
        assert_eq!(parse_search("haziran").month, Some(6));
        let j = parse_search("June selfie");
        assert_eq!((j.month, j.flag), (Some(6), Some(MediaFlagName::Selfie)));
        let e = parse_search("ekran görüntüleri");
        assert_eq!(
            (e.terms.len(), e.flag),
            (0, Some(MediaFlagName::Screenshot))
        );
        let e = parse_search("EKRAN GÖRÜNTÜSÜ 2025");
        assert_eq!(
            (e.flag, e.from),
            (Some(MediaFlagName::Screenshot), Some(utc(2025, 0, 1)))
        );
        let k = parse_search("panorama kapadokya");
        assert_eq!(
            (k.terms, k.flag),
            (vec!["kapadokya".to_string()], Some(MediaFlagName::Panorama))
        );
        assert_eq!(
            parse_search("animasyon").flag,
            Some(MediaFlagName::Animated)
        );
        assert_eq!(parse_search("gif").flag, Some(MediaFlagName::Animated));
        assert_eq!(parse_search("ara").terms, vec!["ara".to_string()]);
        let a = parse_search("ara 2024");
        assert_eq!(
            (a.from, a.to),
            (Some(utc(2024, 11, 1)), Some(utc(2025, 0, 1)))
        );
        assert_eq!(
            parse_search("IMG_2400.jpg").terms,
            vec!["img_2400.jpg".to_string()]
        );
    }
}

mod memories_tests {
    use super::*;

    fn it(id: &str, t: i64) -> MemoryItem {
        MemoryItem {
            id: id.into(),
            local_at: t,
        }
    }

    #[test]
    fn finds_on_this_day_in_previous_years() {
        let now = l("2026-11-05T10:00:00");
        let mut items = vec![];
        for (i, d) in [-3, 0, 2].iter().enumerate() {
            items.push(it(&format!("a{i}"), l("2025-11-05T12:00:00") + d * DAY));
        }
        for (i, d) in [0, 1].iter().enumerate() {
            items.push(it(&format!("b{i}"), l("2024-11-05T12:00:00") + d * DAY));
        }
        for (i, d) in [0, 0, 1, -1].iter().enumerate() {
            items.push(it(&format!("c{i}"), l("2022-11-04T09:00:00") + d * DAY));
        }
        items.push(it("far", l("2025-11-09T12:00:00")));
        items.push(it("now", l("2026-11-05T08:00:00")));
        let m = on_this_day(&items, now, &Default::default());
        assert_eq!(
            m.iter().map(|x| (x.years_ago, x.count)).collect::<Vec<_>>(),
            vec![(Some(1), 3), (Some(4), 4)]
        );
        assert_eq!(m[0].from, l("2025-11-02T00:00:00"));
        assert_eq!(m[0].to, l("2025-11-09T00:00:00"));
        assert_eq!(m[0].cover_ids, vec!["a0", "a1", "a2"]);
        let mut rev = items.clone();
        rev.reverse();
        assert_eq!(on_this_day(&rev, now, &Default::default()), m);
    }

    #[test]
    fn handles_feb_29() {
        let items: Vec<_> = (0..3)
            .map(|i| it(&i.to_string(), l("2027-02-28T10:00:00") + i * 3_600_000))
            .collect();
        assert_eq!(
            on_this_day(&items, l("2028-02-29T10:00:00"), &Default::default()).len(),
            1
        );
    }

    #[test]
    fn window_reaching_into_the_next_year() {
        let items: Vec<_> = (0..3)
            .map(|i| it(&format!("j{i}"), l("2025-01-02T10:00:00") + i * 3_600_000))
            .collect();
        let r = on_this_day(&items, l("2025-12-31T12:00:00"), &Default::default());
        assert_eq!(
            r.iter().map(|m| (m.years_ago, m.count)).collect::<Vec<_>>(),
            vec![(Some(1), 3)]
        );
        let dec: Vec<_> = (0..3)
            .map(|i| it(&format!("d{i}"), l("2024-12-30T10:00:00") + i * 3_600_000))
            .collect();
        let r = on_this_day(&dec, l("2026-01-01T12:00:00"), &Default::default());
        assert_eq!(
            r.iter().map(|m| (m.years_ago, m.count)).collect::<Vec<_>>(),
            vec![(Some(1), 3)]
        );
        assert!(on_this_day(&dec, l("2026-07-01T12:00:00"), &Default::default()).is_empty());
    }
}

mod trips_tests {
    use super::*;
    use std::cell::Cell;

    #[derive(Clone, Copy)]
    struct Place(&'static str, &'static str, &'static str, f64, f64);
    const IST: Place = Place("TR-34", "İstanbul", "TR", 41.01, 29.0);
    const ANK: Place = Place("TR-68", "Ankara", "TR", 39.92, 32.85);
    const ANT: Place = Place("TR-07", "Antalya", "TR", 36.88, 30.7);
    const BER: Place = Place("DE-16", "Berlin", "DE", 52.52, 13.4);

    struct Gen(Cell<usize>);
    impl Gen {
        fn at(&self, t: i64, p: Place, count: i64) -> Vec<TripItem> {
            (0..count)
                .map(|i| {
                    let n = self.0.get();
                    self.0.set(n + 1);
                    TripItem {
                        id: format!("i{n}"),
                        local_at: t + i * 3_600_000,
                        lat: Some(p.3),
                        lon: Some(p.4),
                        place_key: Some(p.0.into()),
                        place_name: Some(p.1.into()),
                        cc: Some(p.2.into()),
                    }
                })
                .collect()
        }
        fn life(&self, from: i64, days: i64, p: Place) -> Vec<TripItem> {
            (0..(days + 2) / 3)
                .flat_map(|k| self.at(from + k * 3 * DAY, p, 1))
                .collect()
        }
    }

    #[test]
    fn detects_a_trip_away_from_home() {
        let g = Gen(Cell::new(0));
        let mut items = g.life(l("2024-01-01T12:00:00"), 360, IST);
        items.extend(g.at(l("2024-07-02T10:00:00"), ANT, 4));
        items.extend(g.at(l("2024-07-03T10:00:00"), ANT, 3));
        items.extend(g.at(l("2024-07-05T10:00:00"), ANT, 2));
        items.push(TripItem {
            id: "nogps".into(),
            local_at: l("2024-07-04T10:00:00"),
            ..Default::default()
        });
        let trips = find_trips(&items, &Default::default());
        assert_eq!(trips.len(), 1);
        let t = &trips[0];
        assert_eq!(
            (t.kind, t.place.as_deref(), t.cc.as_deref(), t.from, t.to),
            (
                MemoryKind::Trip,
                Some("Antalya"),
                Some("TR"),
                l("2024-07-02T00:00:00"),
                l("2024-07-06T00:00:00")
            )
        );
        assert_eq!(
            t.count as usize,
            items
                .iter()
                .filter(|i| i.local_at >= t.from && i.local_at < t.to)
                .count()
        );
        assert!(t.count >= 10);
    }

    #[test]
    fn needs_two_days_or_ten_items() {
        let g = Gen(Cell::new(0));
        let base = g.life(l("2024-01-01T12:00:00"), 360, IST);
        let with = |extra: Vec<Vec<TripItem>>| {
            let mut v = base.clone();
            for e in extra {
                v.extend(e);
            }
            find_trips(&v, &Default::default()).len()
        };
        assert_eq!(with(vec![g.at(l("2024-05-10T10:00:00"), ANK, 5)]), 0);
        assert_eq!(with(vec![g.at(l("2024-05-10T08:00:00"), ANK, 10)]), 1);
        assert_eq!(
            with(vec![
                g.at(l("2024-05-10T08:00:00"), BER, 3),
                g.at(l("2024-05-11T08:00:00"), IST, 1),
                g.at(l("2024-05-12T08:00:00"), BER, 3)
            ]),
            0
        );
    }

    #[test]
    fn computes_home_per_year() {
        let g = Gen(Cell::new(0));
        let mut items = g.life(l("2023-01-01T12:00:00"), 360, IST);
        items.extend(g.life(l("2024-01-01T12:00:00"), 360, ANK));
        items.extend(g.at(l("2024-08-10T10:00:00"), IST, 6));
        items.extend(g.at(l("2024-08-11T10:00:00"), IST, 6));
        let homes = homes_by_year(&items);
        assert_eq!(homes[&2023][0].key, "TR-34");
        assert_eq!(homes[&2024][0].key, "TR-68");
        let trips = find_trips(&items, &Default::default());
        assert_eq!(
            trips
                .iter()
                .map(|t| t.place.clone().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["İstanbul"]
        );
    }

    #[test]
    fn moving_mid_year_is_not_a_giant_trip() {
        let g = Gen(Cell::new(0));
        let mut items = g.life(l("2024-01-01T12:00:00"), 150, IST);
        items.extend(g.life(l("2024-06-01T12:00:00"), 210, ANK));
        items.extend(g.at(l("2024-09-10T10:00:00"), BER, 6));
        items.extend(g.at(l("2024-09-11T10:00:00"), BER, 6));
        let homes: Vec<String> = homes_by_year(&items)[&2024]
            .iter()
            .map(|h| h.key.clone())
            .collect();
        assert!(homes.contains(&"TR-34".to_string()) && homes.contains(&"TR-68".to_string()));
        assert_eq!(
            find_trips(&items, &Default::default())
                .iter()
                .map(|t| t.place.clone().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["Berlin"]
        );
    }
}

mod redos {
    use super::*;
    use std::time::Instant;

    fn fast(f: impl FnOnce()) -> u128 {
        let t = Instant::now();
        f();
        t.elapsed().as_millis()
    }

    #[test]
    fn parsers_stay_linear_on_hostile_input() {
        assert!(
            fast(|| assert_eq!(
                parse_date_time_with_offset(&format!("2024:01:01{}x", " ".repeat(50_000))),
                None
            )) < 100
        );
        assert_eq!(
            parse_date_time_with_offset("  2024:07:01 10:00:00  "),
            Some(LocalWithOffset {
                local: js::date_utc(2024, 6, 1, 10, 0, 0, 0),
                offset: None
            })
        );
        assert!(
            fast(|| assert_eq!(
                parse_iso6709(&format!("+1+1CRS{}x", " ".repeat(50_000))),
                None
            )) < 100
        );
        assert!(
            fast(|| drop(parse_search(&format!(
                "{}a {}",
                ".".repeat(200_000),
                "-".repeat(200_000)
            )))) < 2000
        );
        assert!(
            fast(|| drop(normalize_camera(
                Some(&format!("x{}co ltdx", " ,".repeat(30_000))),
                Some("M")
            ))) < 500
        );
        assert_eq!(
            normalize_camera(Some("NIKON CORPORATION"), Some("NIKON D750"))
                .model
                .as_deref(),
            Some("D750")
        );
        let s = parse_search("..istanbul.. video");
        assert_eq!(
            (s.terms, s.flag),
            (vec!["istanbul".to_string()], Some(MediaFlagName::Video))
        );
    }
}
