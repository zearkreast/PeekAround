//! JavaScript-like access to a client JSON body (`spec as Partial<…>`): property reads that are
//! `undefined` on non-objects, truthiness, and the `num()` coercion of the TS validators.

use crate::error::HttpError;
use serde_json::Value;

/// `v?.[key]` — None for missing keys and for non-objects (strings, numbers… have no such props)
pub fn prop<'a>(v: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    match v {
        Some(Value::Object(m)) => m.get(key),
        _ => None,
    }
}

/// `v === undefined || v === null`
pub fn nullish(v: Option<&Value>) -> bool {
    matches!(v, None | Some(Value::Null))
}

/// JS truthiness (`if (v)`, `!!v`)
pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|x| x != 0.0 && !x.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// `typeof v === 'object'` for a non-null value (arrays included)
pub fn is_object(v: Option<&Value>) -> bool {
    matches!(v, Some(Value::Object(_)) | Some(Value::Array(_)))
}

/// `String(v)` for a string or number
pub fn js_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => n.as_f64().map(pa_core::js::num_to_string),
        _ => None,
    }
}

/// How a string value is turned into a number.
#[derive(Clone, Copy)]
pub enum StrNum {
    /// video-edit.ts: `typeof v === 'string' && v.trim() !== '' ? Number(v) : v` ("" / "  " → not a number)
    NonBlank,
    /// slideshow.ts: `''` is "absent", any other string → `Number(v)` ("  " → 0)
    Any,
}

/// `num(v, name, def)` of the TS validators. `def = None` → required.
pub fn num(
    v: Option<&Value>,
    name: &str,
    def: Option<f64>,
    mode: StrNum,
) -> Result<f64, HttpError> {
    let absent = match mode {
        StrNum::NonBlank => nullish(v),
        StrNum::Any => nullish(v) || matches!(v, Some(Value::String(s)) if s.is_empty()),
    };
    if absent {
        return def.ok_or_else(|| HttpError::bad(format!("{name} is required")));
    }
    let n = match v {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => match mode {
            StrNum::NonBlank if pa_core::js::trim(s).is_empty() => None,
            _ => Some(pa_core::js::number(s)),
        },
        _ => None,
    };
    match n {
        Some(x) if x.is_finite() => Ok(x),
        _ => Err(HttpError::bad(format!("{name} must be a number"))),
    }
}

/// `inRange` / `range`: within [lo, hi] (±1e-9), clamped
pub fn in_range(n: f64, lo: f64, hi: f64, name: &str) -> Result<f64, HttpError> {
    if n < lo - 1e-9 || n > hi + 1e-9 {
        return Err(HttpError::bad(format!(
            "{name} must be between {} and {}",
            pa_core::js::num_to_string(lo),
            pa_core::js::num_to_string(hi)
        )));
    }
    Ok(n.max(lo).min(hi))
}
