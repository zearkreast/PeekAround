//! HTTP routes (mirror `packages/server/src/routes/*`).

pub mod albums;
pub mod browse;
pub mod core;
pub mod explore;
pub mod files;
pub mod media;
pub mod misc;
pub mod util;

use crate::router::Router;
use serde_json::Value;

/// every API route
pub fn router() -> Router {
    let mut r = Router::default();
    core::routes(&mut r);
    browse::routes(&mut r);
    media::routes(&mut r);
    explore::routes(&mut r);
    albums::routes(&mut r);
    misc::routes(&mut r);
    files::routes(&mut r);
    r
}

/// JS truthiness of a JSON value
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

/// a JSON number the way JSON.stringify writes it (integral → no fraction)
pub fn js_number(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 9e15 {
        Value::from(f as i64)
    } else {
        serde_json::Number::from_f64(f)
            .map(Value::Number)
            .unwrap_or(Value::Null)
    }
}

/// `new Date(ms).toISOString()`
pub fn iso_string(ms: i64) -> String {
    let p = pa_core::js::utc_parts(ms);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        p.year,
        p.month0 + 1,
        p.day,
        p.hour,
        p.minute,
        p.second,
        p.ms
    )
}
