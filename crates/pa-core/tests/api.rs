//! Real responses of the TypeScript server (tests/golden/api/*.json, captured from the fixture library)
//! must deserialize into the pa-core types and serialize back to the identical JSON.

use std::path::Path;

use pa_core::*;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

/// deserialize, serialize again: (value, exact JSON text)
fn round_trip<T: DeserializeOwned + Serialize>(raw: &str) -> Result<(Value, String), String> {
    let t: T = serde_json::from_str(raw).map_err(|e| format!("deserialize: {e}"))?;
    let s = serde_json::to_string(&t).map_err(|e| format!("serialize: {e}"))?;
    Ok((serde_json::from_str(&s).map_err(|e| e.to_string())?, s))
}

fn check(ty: &str, v: &str) -> Result<(Value, String), String> {
    match ty {
        "ServerInfo" => round_trip::<ServerInfo>(v),
        "Settings" => round_trip::<Settings>(v),
        "LibrariesResponse" => round_trip::<LibrariesResponse>(v),
        "Library" => round_trip::<Library>(v),
        "LibraryStats" => round_trip::<LibraryStats>(v),
        "Album" => round_trip::<Album>(v),
        "AlbumsResponse" => round_trip::<AlbumsResponse>(v),
        "AlbumMembership" => round_trip::<AlbumMembership>(v),
        "BrowseResult" => round_trip::<BrowseResult>(v),
        "BrowseSkeleton" => round_trip::<BrowseSkeleton>(v),
        "TimelineResponse" => round_trip::<TimelineResponse>(v),
        "ExploreResult" => round_trip::<ExploreResult>(v),
        "CalendarYear" => round_trip::<CalendarYear>(v),
        "DuplicatesResponse" => round_trip::<DuplicatesResponse>(v),
        "GeoPoints" => round_trip::<GeoPoints>(v),
        "MetaStatus" => round_trip::<MetaStatus>(v),
        "MediaDetails" => round_trip::<MediaDetails>(v),
        "Job" => round_trip::<Job>(v),
        "JobsResponse" => round_trip::<JobsResponse>(v),
        "FontsResponse" => round_trip::<FontsResponse>(v),
        "DevicesResponse" => round_trip::<DevicesResponse>(v),
        "MegaAccountStatus" => round_trip::<MegaAccountStatus>(v),
        "ZipPeekResponse" => round_trip::<ZipPeekResponse>(v),
        "SearchResponse" => round_trip::<SearchResponse>(v),
        "ServerEvent[]" => round_trip::<Vec<ServerEvent>>(v),
        _ => Err(format!("no type mapping for {ty}")),
    }
}

#[test]
fn api_responses_round_trip() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/api");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .expect("api fixtures")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    files.sort();
    assert!(files.len() > 100);
    let mut bad = vec![];
    for f in &files {
        let j: Value = serde_json::from_slice(&std::fs::read(f).expect("read")).expect("json");
        let ty = j["type"].as_str().expect("type");
        let resp = &j["response"];
        // `raw` = JSON.stringify(response) as the TS server sent it (key order + number formatting)
        let raw = j["raw"].as_str().expect("raw");
        match check(ty, raw) {
            Ok((back, text)) if &back == resp && text == raw => {}
            Ok((back, text)) if &back == resp => bad.push(format!(
                "{}: {ty} same value, different bytes\n  ts:   {raw}\n  rust: {text}",
                f.display()
            )),
            Ok((back, _)) => bad.push(format!(
                "{}: {ty} differs\n  ts:   {}\n  rust: {}",
                f.display(),
                resp,
                back
            )),
            Err(e) => bad.push(format!("{}: {ty}: {e}", f.display())),
        }
    }
    for b in bad.iter().take(10) {
        eprintln!("{}", b.chars().take(3000).collect::<String>());
    }
    assert!(
        bad.is_empty(),
        "{} of {} responses do not round-trip",
        bad.len(),
        files.len()
    );
}

#[test]
fn serializes_like_json_stringify() {
    // integral floats without ".0", optional fields omitted, `x?: T | null` kept as null
    let item = MediaItem {
        id: "1".into(),
        folder_id: "2".into(),
        dir: String::new(),
        name: "a.mp4".into(),
        ext: "mp4".into(),
        kind: MediaKind::Video,
        size: 10,
        mtime: 5,
        width: None,
        height: None,
        duration: Some(8.0),
        taken_at: 5,
        local_at: 5,
        flags: 0,
        thumbhash: None,
        favorite: false,
        playback: Playback::Direct,
    };
    let s = serde_json::to_string(&item).unwrap();
    assert!(s.contains(r#""duration":8,"#), "{s}");
    let lib = Library {
        id: "1".into(),
        name: "x".into(),
        color: "#fff".into(),
        icon: "images".into(),
        sort: 0,
        folders: vec![],
        media_count: 0,
        cover_id: Some(None),
        options: None,
    };
    assert_eq!(
        serde_json::to_string(&lib).unwrap(),
        r##"{"id":"1","name":"x","color":"#fff","icon":"images","sort":0,"folders":[],"mediaCount":0,"coverId":null}"##
    );
    let ev = ServerEvent::Media {
        folder_id: "1".into(),
        dirs: vec!["".into()],
        reason: Some(MediaReason::Thumb),
    };
    assert_eq!(
        serde_json::to_string(&ev).unwrap(),
        r#"{"type":"media","folderId":"1","dirs":[""],"reason":"thumb"}"#
    );
    assert_eq!(
        serde_json::to_string(&ServerEvent::MegaAccount).unwrap(),
        r#"{"type":"megaAccount"}"#
    );
    let r = MegaLoginResult::Refused(MegaLoginRefusal::MfaRequired);
    assert_eq!(
        serde_json::to_string(&r).unwrap(),
        r#"{"ok":false,"reason":"mfa-required"}"#
    );
    assert_eq!(
        serde_json::from_str::<MegaLoginResult>(r#"{"ok":false,"reason":"mfa-required"}"#).unwrap(),
        r
    );
}
