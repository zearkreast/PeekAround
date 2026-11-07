//! Dynamic metadata values with the JavaScript semantics the TypeScript reference relies on
//! (exifr output is untyped JS: numbers, strings, plain arrays, typed arrays, objects, `undefined`).

use indexmap::IndexMap;
use pa_core::js;

/// One metadata value as exifr would return it.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Val {
    /// `undefined` (a key that exists with no value — it still overrides on merge)
    #[default]
    Undef,
    /// boolean (XMP "True"/"false")
    Bool(bool),
    /// number
    Num(f64),
    /// string
    Str(String),
    /// plain JS array (EXIF RATIONAL lists, XMP rdf:Seq / Bag / Alt)
    Arr(Vec<Val>),
    /// `Uint8Array` (EXIF BYTE / UNDEFINED values) — not `Array.isArray`
    Bytes(Vec<u8>),
    /// other typed arrays (EXIF SHORT / LONG / SBYTE / FLOAT / DOUBLE … lists) — not `Array.isArray`
    Typed(Vec<f64>),
    /// object (XMP structures)
    Obj(IndexMap<String, Val>),
}

/// Merged metadata output (`mergeOutput: true`): tag name → value.
pub type Tags = IndexMap<String, Val>;

impl Val {
    /// `undefined`
    pub fn is_undef(&self) -> bool {
        matches!(self, Val::Undef)
    }

    /// JavaScript `Number(v)`
    pub fn number(&self) -> f64 {
        match self {
            Val::Undef | Val::Obj(_) => f64::NAN,
            Val::Bool(b) => f64::from(u8::from(*b)),
            Val::Num(n) => *n,
            Val::Str(s) => js::number(s),
            Val::Arr(_) | Val::Typed(_) | Val::Bytes(_) => js::number(&self.js_string()),
        }
    }

    /// JavaScript `String(v)`
    pub fn js_string(&self) -> String {
        match self {
            Val::Undef => "undefined".into(),
            Val::Bool(b) => b.to_string(),
            Val::Num(n) => js::num_to_string(*n),
            Val::Str(s) => s.clone(),
            Val::Arr(a) => a
                .iter()
                .map(|x| {
                    if x.is_undef() {
                        String::new()
                    } else {
                        x.js_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(","),
            Val::Typed(a) => a
                .iter()
                .map(|x| js::num_to_string(*x))
                .collect::<Vec<_>>()
                .join(","),
            Val::Bytes(a) => a
                .iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join(","),
            Val::Obj(_) => "[object Object]".into(),
        }
    }

    /// the string when this is a string
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Val::Str(s) => Some(s),
            _ => None,
        }
    }

    /// to JSON (undefined → null)
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::Value;
        match self {
            Val::Undef => Value::Null,
            Val::Bool(b) => Value::Bool(*b),
            Val::Num(n) => serde_json::to_value(js::JsF64(*n)).unwrap_or(Value::Null),
            Val::Str(s) => Value::String(s.clone()),
            Val::Arr(a) => Value::Array(a.iter().map(Val::to_json).collect()),
            Val::Typed(a) => Value::Array(
                a.iter()
                    .map(|x| serde_json::to_value(js::JsF64(*x)).unwrap_or(Value::Null))
                    .collect(),
            ),
            Val::Bytes(a) => Value::Array(a.iter().map(|x| Value::from(*x)).collect()),
            Val::Obj(m) => Value::Object(
                m.iter()
                    .filter(|(_, v)| !v.is_undef())
                    .map(|(k, v)| (k.clone(), v.to_json()))
                    .collect(),
            ),
        }
    }
}

/// `ex[k]` (missing → undefined)
pub fn get<'a>(tags: &'a Tags, k: &str) -> &'a Val {
    static UNDEF: Val = Val::Undef;
    tags.get(k).unwrap_or(&UNDEF)
}

/// JS `a + b` where `b` is a number (string concatenation when `a` is a string)
pub(crate) fn js_add(a: &Val, b: f64) -> Val {
    match a {
        Val::Str(s) => Val::Str(format!("{s}{}", js::num_to_string(b))),
        Val::Arr(_) | Val::Typed(_) | Val::Bytes(_) | Val::Obj(_) => {
            Val::Str(format!("{}{}", a.js_string(), js::num_to_string(b)))
        }
        _ => Val::Num(a.number() + b),
    }
}
