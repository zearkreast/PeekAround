//! Tag-level parity with exifr 7 on 700 generated files (random EXIF IFDs with every value type,
//! broken offsets / types / counts, random XMP packets incl. malformed ones, JPEG / PNG / TIFF
//! containers). tests/golden-fuzz.json holds the files (base64) and exifr's output for both option sets
//! the TypeScript server used (see PARITY.md).

use pa_meta::exifr::{self, Mode, PANEL_KEYS};
use pa_meta::{Tags, Val};
use serde_json::{json, Value};

fn b64(s: &str) -> Vec<u8> {
    let val = |c: u8| -> u32 {
        match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a') as u32 + 26,
            b'0'..=b'9' => (c - b'0') as u32 + 52,
            b'+' => 62,
            _ => 63,
        }
    };
    let mut out = vec![];
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes().filter(|c| *c != b'=') {
        acc = (acc << 6) | val(c);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
        acc &= 0xffff;
    }
    out
}

fn enc(v: &Val) -> Value {
    match v {
        Val::Undef => json!({"$u": 1}),
        Val::Bool(b) => json!(b),
        Val::Num(n) if !n.is_finite() => json!({"$n": pa_core::js::num_to_string(*n)}),
        Val::Num(n) => serde_json::to_value(pa_core::js::JsF64(*n)).unwrap_or(Value::Null),
        Val::Str(s) => json!(s),
        Val::Arr(a) => Value::Array(a.iter().map(enc).collect()),
        Val::Typed(a) => json!({"$t": a.iter().map(|x| enc(&Val::Num(*x))).collect::<Vec<_>>()}),
        Val::Bytes(a) => json!({"$t": a}),
        Val::Obj(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), enc(v))).collect()),
    }
}

fn enc_tags(t: Option<Tags>, only: Option<&[&str]>) -> Value {
    match t {
        Some(t)
            if t.keys()
                .any(|k| only.is_none_or(|o| o.contains(&k.as_str()))) =>
        {
            Value::Object(
                t.iter()
                    .filter(|(k, _)| only.is_none_or(|o| o.contains(&k.as_str())))
                    .map(|(k, v)| (k.clone(), enc(v)))
                    .collect(),
            )
        }
        _ => Value::Null,
    }
}

fn norm(v: &Value, only: Option<&[&str]>) -> Value {
    match v {
        Value::Object(m) if m.is_empty() => Value::Null,
        Value::Object(m) if only.is_some() => Value::Object(
            m.iter()
                .filter(|(k, _)| only.is_some_and(|o| o.contains(&k.as_str())))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        ),
        _ => v.clone(),
    }
}

fn close(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap_or(0.0), y.as_f64().unwrap_or(0.0));
            (x - y).abs() <= 1e-9 * x.abs().max(y.abs()).max(1.0)
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| close(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, p)| y.get(k).is_some_and(|q| close(p, q)))
        }
        _ => a == b,
    }
}

#[test]
fn exifr_tag_level_parity() {
    let raw = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden-fuzz.json"
    ))
    .expect("golden-fuzz.json");
    let cases: Vec<Value> = serde_json::from_slice(&raw).expect("json");
    let mut bad = vec![];
    for (i, c) in cases.iter().enumerate() {
        let bytes = b64(c["b64"].as_str().expect("b64"));
        let group = enc_tags(exifr::parse(&bytes, &Mode::Group), None);
        let want = norm(&c["group"], None);
        if !close(&group, &want) {
            bad.push(format!(
                "#{i} {} group\n  rust: {group}\n  ts:   {want}",
                c["kind"]
            ));
        }
        let panel = enc_tags(
            exifr::parse(
                &bytes,
                &Mode::Panel {
                    local_tz: "UTC".into(),
                },
            ),
            Some(&PANEL_KEYS),
        );
        let want = norm(&c["panel"], Some(&PANEL_KEYS));
        let want = if want.as_object().is_some_and(|m| m.is_empty()) {
            Value::Null
        } else {
            want
        };
        if !close(&panel, &want) {
            bad.push(format!(
                "#{i} {} panel\n  rust: {panel}\n  ts:   {want}",
                c["kind"]
            ));
        }
    }
    for b in bad.iter().take(12) {
        eprintln!("{b}");
    }
    assert!(
        bad.is_empty(),
        "{} of {} fuzz cases differ",
        bad.len(),
        cases.len() * 2
    );
}
