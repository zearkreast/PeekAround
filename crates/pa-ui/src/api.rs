//! JSON API calls of the pages (through [`crate::backend::Backend::request`]) with the result
//! delivered on the UI thread.

use std::rc::Rc;

use serde::de::DeserializeOwned;

use crate::app::AppInner;
use crate::backend::BackendError;

/// percent-encode a path segment / query value
pub fn enc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                o.push(b as char)
            }
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

/// `?k=v&…` (empty values skipped)
pub fn qs(pairs: &[(&str, &str)]) -> String {
    let v: Vec<String> = pairs
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| format!("{k}={}", enc(v)))
        .collect();
    if v.is_empty() {
        String::new()
    } else {
        format!("?{}", v.join("&"))
    }
}

/// Call a route; `then` runs on the UI thread with the decoded answer.
pub(crate) fn call<T: DeserializeOwned + Send + 'static>(
    app: &Rc<AppInner>,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
    then: impl FnOnce(&Rc<AppInner>, Result<T, BackendError>) + 'static,
) {
    let fut = app.backend.request(method, path, body);
    let weak = Rc::downgrade(app);
    app.rt.spawn_then(
        async move {
            let v = fut.await?;
            serde_json::from_value::<T>(v).map_err(|e| BackendError {
                status: 0,
                message: format!("decode: {e}"),
            })
        },
        move |r| {
            if let Some(a) = weak.upgrade() {
                then(&a, r);
            }
        },
    );
}

/// [`call`] that only reports errors (toast); `ok` runs on success.
pub(crate) fn act(
    app: &Rc<AppInner>,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
    ok: impl FnOnce(&Rc<AppInner>, serde_json::Value) + 'static,
) {
    call::<serde_json::Value>(app, method, path, body, move |a, r| match r {
        Ok(v) => ok(a, v),
        Err(e) => a.error_toast(&e.message),
    });
}
