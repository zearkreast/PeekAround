//! HTTP plumbing with Fastify semantics: the request wrapper handlers get, content-type parsers
//! (JSON / text / raw streams per scope, the same 400 / 415 messages), JS-like body access
//! (`Cannot read properties of undefined (reading 'x')`), JSON replies and `sendFile`
//! (@fastify/send: ETag, Last-Modified, Range / 206 / 416, conditional GET / 304).

use crate::error::{ApiResult, HttpError};
use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Method, Request, Response, StatusCode};
use bytes::Bytes;
use futures_util::StreamExt;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::net::IpAddr;
use std::path::Path;
use tokio_util::sync::CancellationToken;

/// Per-connection facts (which listener, client address).
#[derive(Clone, Debug)]
pub struct ConnInfo {
    /// client IP (`req.ip`)
    pub ip: IpAddr,
    /// came in through the desktop LAN listener (`peekLan`)
    pub lan: bool,
    /// cancelled when the listener stops (WebSockets / long replies end with it)
    pub closed: CancellationToken,
}

impl Default for ConnInfo {
    fn default() -> Self {
        ConnInfo {
            ip: IpAddr::from([127, 0, 0, 1]),
            lan: false,
            closed: CancellationToken::new(),
        }
    }
}

/// Which content-type parsers a route has (Fastify encapsulation scopes of app.ts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyScope {
    /// core / browse / media / explore / … (+ the jobs scope's octet-stream / image/png raw parsers)
    Main,
    /// file routes: raw streams for octet-stream, image/*, video/*, audio/*, zip, font/*
    Files,
    /// device routes (defaults only)
    Devices,
    /// every content type raw (phone replies)
    Any,
}

/// The parsed body (`req.body`).
#[derive(Debug, Clone)]
pub enum JsBody {
    /// `undefined`
    Undefined,
    /// parsed JSON (or a text/plain string)
    Json(Value),
}

/// JS `typeof` style access on JSON values.
pub trait JsAccess<'a> {
    /// `v.key` (TypeError on undefined / null)
    fn prop(self, key: &str) -> ApiResult<Option<&'a Value>>;
}

fn type_error(of: &str, key: &str) -> HttpError {
    HttpError::new(
        500,
        format!("Cannot read properties of {of} (reading '{key}')"),
    )
}

impl<'a> JsAccess<'a> for Option<&'a Value> {
    fn prop(self, key: &str) -> ApiResult<Option<&'a Value>> {
        match self {
            None => Err(type_error("undefined", key)),
            Some(Value::Null) => Err(type_error("null", key)),
            Some(Value::Object(m)) => Ok(m.get(key)),
            Some(_) => Ok(None),
        }
    }
}

impl JsBody {
    /// the value (None = undefined)
    pub fn value(&self) -> Option<&Value> {
        match self {
            JsBody::Undefined => None,
            JsBody::Json(v) => Some(v),
        }
    }

    /// `req.body.key` (TypeError when the body is undefined / null)
    pub fn prop(&self, key: &str) -> ApiResult<Option<&Value>> {
        self.value().prop(key)
    }

    /// `req.body?.key`
    pub fn opt(&self, key: &str) -> Option<&Value> {
        match self.value() {
            Some(Value::Object(m)) => m.get(key),
            _ => None,
        }
    }

    /// `req.body ?? {}` then `.key`
    pub fn or_empty(&self, key: &str) -> Option<&Value> {
        match self.value() {
            Some(Value::Null) | None => None,
            _ => self.opt(key),
        }
    }

    /// the body as a typed value (missing / mismatching → TypeError-like 500 only when undefined)
    pub fn obj(&self) -> Option<&serde_json::Map<String, Value>> {
        match self.value() {
            Some(Value::Object(m)) => Some(m),
            _ => None,
        }
    }
}

/// JS `String(v)` for query / body values
pub fn js_string(v: &Value) -> String {
    pa_core::js::to_string_of(v)
}

/// The request a handler gets.
pub struct Req {
    /// the request head (method, uri, headers, extensions)
    pub parts: axum::http::request::Parts,
    /// path parameters
    pub params: HashMap<String, String>,
    /// query string (first value per key, '+' decoded)
    pub query: HashMap<String, String>,
    pub conn: ConnInfo,
    /// parsed body (raw-stream routes: Undefined, see `take_raw`)
    pub body: JsBody,
    raw: std::sync::Mutex<Option<Body>>,
    /// route body limit (bytes)
    pub body_limit: u64,
}

/// parse a query string like fast-querystring (first value per key)
pub fn parse_query(q: Option<&str>) -> HashMap<String, String> {
    let mut m = HashMap::new();
    if let Some(q) = q {
        for (k, v) in url::form_urlencoded::parse(q.as_bytes()) {
            m.entry(k.into_owned()).or_insert_with(|| v.into_owned());
        }
    }
    m
}

impl Req {
    /// new request wrapper (body not yet parsed)
    pub fn new(parts: axum::http::request::Parts, body: Body, conn: ConnInfo) -> Self {
        let query = parse_query(parts.uri.query());
        Req {
            parts,
            params: HashMap::new(),
            query,
            conn,
            body: JsBody::Undefined,
            raw: std::sync::Mutex::new(Some(body)),
            body_limit: 32 * 1024 * 1024,
        }
    }

    /// header value as str
    pub fn header(&self, name: &str) -> Option<&str> {
        self.parts.headers.get(name).and_then(|v| v.to_str().ok())
    }

    pub fn param(&self, name: &str) -> &str {
        self.params.get(name).map(String::as_str).unwrap_or("")
    }

    /// query value (None when missing)
    pub fn q(&self, name: &str) -> Option<&str> {
        self.query.get(name).map(String::as_str)
    }

    /// query value, '' when missing
    pub fn qs(&self, name: &str) -> &str {
        self.q(name).unwrap_or("")
    }

    pub fn method(&self) -> &Method {
        &self.parts.method
    }

    /// path + query
    pub fn url(&self) -> String {
        self.parts
            .uri
            .path_and_query()
            .map(|p| p.as_str().to_string())
            .unwrap_or_else(|| self.parts.uri.path().to_string())
    }

    /// came through the desktop LAN listener
    pub fn is_lan(&self) -> bool {
        self.conn.lan
    }

    /// the raw body stream (upload routes)
    pub fn take_raw(&mut self) -> Body {
        self.raw
            .lock()
            .ok()
            .and_then(|mut r| r.take())
            .unwrap_or_else(Body::empty)
    }

    /// Fastify content-type parsing for this route's scope.
    pub async fn parse_body(&mut self, scope: BodyScope) -> ApiResult<()> {
        if matches!(self.parts.method, Method::GET | Method::HEAD) {
            return Ok(());
        }
        let ct = self.header("content-type").map(|c| {
            c.split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase()
        });
        let has_body = self.header("transfer-encoding").is_some()
            || self
                .header("content-length")
                .is_some_and(|l| l.trim() != "0");
        if scope == BodyScope::Any {
            // the '*' catch-all parser also takes requests without a Content-Type
            return Ok(());
        }
        let Some(ct) = ct.filter(|c| !c.is_empty()) else {
            if has_body {
                return Err(HttpError::new(415, "Unsupported Media Type"));
            }
            return Ok(());
        };
        let raw_type = match scope {
            BodyScope::Main => ct == "application/octet-stream" || ct == "image/png",
            BodyScope::Files => {
                ct == "application/octet-stream"
                    || ct.starts_with("image/")
                    || ct.starts_with("video/")
                    || ct.starts_with("audio/")
                    || ct.starts_with("font/")
                    || ct == "application/zip"
                    || ct == "application/x-zip-compressed"
            }
            _ => false,
        };
        if raw_type {
            return Ok(());
        }
        if ct == "application/json" {
            let bytes = self.read_all().await?;
            if bytes.is_empty() {
                return Err(HttpError::new(
                    400,
                    "Body cannot be empty when content-type is set to 'application/json'",
                ));
            }
            let invalid = || {
                HttpError::new(
                    400,
                    "Body is not valid JSON but content-type is set to 'application/json'",
                )
            };
            let v: Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
            if has_proto(&v) {
                return Err(invalid());
            }
            self.body = JsBody::Json(v);
            return Ok(());
        }
        if ct == "text/plain" {
            let bytes = self.read_all().await?;
            self.body = JsBody::Json(Value::String(String::from_utf8_lossy(&bytes).into_owned()));
            return Ok(());
        }
        Err(HttpError::new(415, "Unsupported Media Type"))
    }

    /// whole body (respecting the route's body limit → 413)
    pub async fn read_all(&mut self) -> ApiResult<Bytes> {
        read_limited(self.take_raw(), self.body_limit).await
    }
}

/// collect a body up to `limit` bytes (413 otherwise)
pub async fn read_limited(body: Body, limit: u64) -> ApiResult<Bytes> {
    let mut s = body.into_data_stream();
    let mut out = bytes::BytesMut::new();
    while let Some(c) = s.next().await {
        let c = c.map_err(|e| HttpError::new(400, e.to_string()))?;
        if (out.len() + c.len()) as u64 > limit {
            return Err(HttpError::new(413, "Request body is too large"));
        }
        out.extend_from_slice(&c);
    }
    Ok(out.freeze())
}

fn has_proto(v: &Value) -> bool {
    match v {
        Value::Object(m) => m.iter().any(|(k, v)| {
            k == "__proto__" || (k == "constructor" && v.get("prototype").is_some()) || has_proto(v)
        }),
        Value::Array(a) => a.iter().any(has_proto),
        _ => false,
    }
}

/// `application/json; charset=utf-8` reply
pub fn json_response<T: Serialize + ?Sized>(v: &T) -> Response<Body> {
    let body = serde_json::to_vec(v).unwrap_or_else(|_| b"{}".to_vec());
    let mut r = Response::new(Body::from(body));
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    r
}

/// JSON reply as an Ok result
pub fn ok_json<T: Serialize + ?Sized>(v: &T) -> ApiResult<Response<Body>> {
    Ok(json_response(v))
}

/// empty reply with a status
pub fn empty(status: u16) -> Response<Body> {
    let mut r = Response::new(Body::empty());
    *r.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
    r
}

/// `reply.redirect(url)` (302)
pub fn redirect(to: &str) -> Response<Body> {
    let mut r = empty(302);
    if let Ok(v) = HeaderValue::from_str(to) {
        r.headers_mut().insert(header::LOCATION, v);
    }
    r
}

/// set a header (ignores invalid values)
pub fn set_header(r: &mut Response<Body>, k: &'static str, v: &str) {
    if let Ok(v) = HeaderValue::from_str(v) {
        r.headers_mut().insert(k, v);
    }
}

/// `attachment; filename="ascii"; filename*=UTF-8''…`
pub fn content_disposition(name: &str) -> String {
    let ascii: String = name
        .chars()
        .map(|c| if (' '..='~').contains(&c) { c } else { '_' })
        .filter(|c| *c != '"')
        .collect();
    format!(
        "attachment; filename=\"{ascii}\"; filename*=UTF-8''{}",
        encode_uri_component(name)
    )
}

/// JS `encodeURIComponent`
pub fn encode_uri_component(s: &str) -> String {
    const SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'!')
        .remove(b'~')
        .remove(b'*')
        .remove(b'\'')
        .remove(b'(')
        .remove(b')');
    percent_encoding::utf8_percent_encode(s, SET).to_string()
}

// ───────────────────────── sendFile (@fastify/send) ─────────────────────────

/// `Date.toUTCString()` of a system time (second precision)
fn utc_string(t: std::time::SystemTime) -> String {
    httpdate::fmt_http_date(t)
}

fn mtime_ms(md: &std::fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn parse_token_list(s: &str) -> Vec<&str> {
    s.split(',')
        .map(|t| t.trim_matches(' '))
        .filter(|t| !t.is_empty())
        .collect()
}

/// `Date.parse` of an HTTP date (IMF-fixdate, RFC 850, asctime; the weekday is not checked) → ms
fn parse_http_date(s: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let mut month = None;
    let mut nums: Vec<i64> = Vec::new();
    let mut time: Option<(i64, i64, i64)> = None;
    for tok in s.split([' ', ',', '-']).filter(|t| !t.is_empty()) {
        let l = tok.to_ascii_lowercase();
        if tok.contains(':') {
            let p: Vec<i64> = tok
                .split(':')
                .map(|x| x.parse::<i64>().ok())
                .collect::<Option<Vec<_>>>()?;
            time = Some((*p.first()?, *p.get(1)?, p.get(2).copied().unwrap_or(0)));
        } else if let Some(m) = MONTHS.iter().position(|m| l.starts_with(m)) {
            month = Some(m as i64);
        } else if let Ok(n) = tok.parse::<i64>() {
            nums.push(n);
        } else if l == "gmt"
            || l == "utc"
            || l == "ut"
            || l == "z"
            || MONTHS.iter().all(|m| !l.starts_with(m))
                && l.len() <= 9
                && l.chars().all(|c| c.is_ascii_alphabetic())
        {
            // weekday / zone
        } else {
            return None;
        }
    }
    let month = month?;
    let (day, mut year) = match nums.as_slice() {
        [a, b] if *a > 31 => (*b, *a),
        [a, b] => (*a, *b),
        _ => return None,
    };
    if year < 50 {
        year += 2000;
    } else if year < 100 {
        year += 1900;
    }
    let (h, mi, sec) = time.unwrap_or((0, 0, 0));
    if !(1..=31).contains(&day) || h > 24 || mi > 59 || sec > 59 {
        return None;
    }
    Some(pa_core::js::date_utc(year, month, day, h, mi, sec, 0))
}

/// send's `parseBytesRange`
fn parse_ranges(size: i64, s: &str) -> Vec<(i64, i64)> {
    let values = &s[s.find('=').map(|i| i + 1).unwrap_or(0)..];
    let js_int = |t: &str| -> Option<i64> {
        let t = t.trim_start();
        let mut end = 0;
        let b = t.as_bytes();
        if end < b.len() && (b[end] == b'-' || b[end] == b'+') {
            end += 1;
        }
        let ds = end;
        while end < b.len() && b[end].is_ascii_digit() {
            end += 1;
        }
        if end == ds {
            return None;
        }
        t[..end].parse::<i64>().ok()
    };
    let mut ranges: Vec<(i64, i64, usize)> = Vec::new();
    for (idx, part) in values.split(',').enumerate() {
        let (a, b) = match part.find('-') {
            Some(d) => (&part[..d], &part[d + 1..]),
            None => (part, ""),
        };
        let mut start = js_int(a);
        let mut end = js_int(b);
        match (start, end) {
            (None, Some(e)) => {
                start = Some(size - e);
                end = Some(size - 1);
            }
            (Some(_), None) => end = Some(size - 1),
            (Some(_), Some(e)) if e > size - 1 => end = Some(size - 1),
            _ => {}
        }
        if let (Some(s0), Some(e0)) = (start, end) {
            if s0 > -1 && s0 <= e0 {
                ranges.push((s0, e0, idx));
            }
        }
    }
    if ranges.len() < 2 {
        return ranges.into_iter().map(|(a, b, _)| (a, b)).collect();
    }
    ranges.sort_by_key(|r| r.0);
    let mut merged: Vec<(i64, i64, usize)> = vec![ranges[0]];
    for r in ranges.into_iter().skip(1) {
        let cur = merged.last_mut().unwrap();
        if r.0 > cur.1 + 1 {
            merged.push(r);
        } else if r.1 > cur.1 {
            cur.1 = r.1;
            cur.2 = cur.2.min(r.2);
        }
    }
    merged.sort_by_key(|r| r.2);
    merged.into_iter().map(|(a, b, _)| (a, b)).collect()
}

/// a `send` error (412 / 416) as @fastify/static hands it to the app's error handler: `{error}`
/// JSON; headers the route set before `sendFile` (Cache-Control) survive, send's own do not.
fn send_error(status: u16, title: &str, opts: &SendOpts) -> Response<Body> {
    let body = serde_json::json!({ "error": title }).to_string();
    let mut r = Response::new(Body::from(body));
    *r.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    set_header(&mut r, "content-type", "application/json; charset=utf-8");
    if let Some(cc) = opts.preset_cache_control {
        set_header(&mut r, "cache-control", cc);
    }
    r
}

/// Options of [`send_file`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SendOpts {
    /// send's own `Cache-Control: public, max-age=0` (static routes); false for `{ cacheControl: false }`
    pub cache_control: bool,
    /// Cache-Control the route set before `sendFile` (kept on error replies only: on success the
    /// app's `setHeaders` overrides it)
    pub preset_cache_control: Option<&'static str>,
}

/// `reply.sendFile(abs)`: @fastify/send headers + Range / conditional GET, plus @fastify/static's
/// `setHeaders` of app.ts (`/assets/` → immutable, everything else `no-cache`).
/// None when the file is missing (→ callNotFound).
pub async fn send_file(
    req_headers: &HeaderMap,
    method: &Method,
    abs: &Path,
    opts: SendOpts,
) -> Option<Response<Body>> {
    let md = tokio::fs::metadata(abs).await.ok()?;
    if md.is_dir() {
        return None;
    }
    let size = md.len() as i64;
    let mtime = mtime_ms(&md);
    let mut headers: Vec<(&'static str, String)> = vec![("accept-ranges", "bytes".into())];
    if opts.cache_control {
        headers.push(("cache-control", "public, max-age=0".into()));
    }
    let modified = md.modified().map(utc_string).unwrap_or_default();
    headers.push(("last-modified", modified.clone()));
    let etag = format!("W/\"{:x}-{:x}\"", size, mtime);
    headers.push(("etag", etag.clone()));
    let p = abs.to_string_lossy();
    let mut ctype = crate::mime::mime_of(&p).map(String::from);
    if let Some(t) = &ctype {
        let l = t.len();
        if (l > 21 && t.starts_with("application/javascript"))
            || (l > 14 && t.starts_with("application/json"))
            || (l > 5 && t.starts_with("text/"))
        {
            ctype = Some(format!("{t}; charset=utf-8"));
        }
    }
    let h = |n: &str| req_headers.get(n).and_then(|v| v.to_str().ok());
    let conditional = h("if-match").is_some()
        || h("if-unmodified-since").is_some()
        || h("if-none-match").is_some()
        || h("if-modified-since").is_some();
    let finish = |status: u16,
                  mut headers: Vec<(&'static str, String)>,
                  body: Body,
                  ctype: Option<String>| {
        let mut r = Response::new(body);
        *r.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::OK);
        if let Some(c) = ctype {
            headers.push(("content-type", c));
        }
        for (k, v) in headers {
            set_header(&mut r, k, &v);
        }
        // app.ts setHeaders
        let assets = p.contains("/assets/") || p.contains("\\assets\\");
        set_header(
            &mut r,
            "cache-control",
            if assets {
                "public, max-age=31536000, immutable"
            } else {
                "no-cache"
            },
        );
        r
    };
    if conditional {
        // precondition
        if let Some(im) = h("if-match") {
            if im != "*"
                && !parse_token_list(im)
                    .iter()
                    .any(|m| *m == etag || format!("W/{m}") == etag)
            {
                return Some(send_error(412, "Precondition Failed", &opts));
            }
        }
        if let Some(ius) = h("if-unmodified-since").and_then(parse_http_date) {
            let lm = parse_http_date(&modified);
            if lm.is_none_or(|l| l > ius) {
                return Some(send_error(412, "Precondition Failed", &opts));
            }
        }
        // not modified
        let no_cache = h("cache-control").is_some_and(|c| c.contains("no-cache"));
        let not_modified = !no_cache
            && if let Some(inm) = h("if-none-match") {
                inm == "*"
                    || parse_token_list(inm).iter().any(|m| {
                        (etag.len() == m.len() && *m == etag)
                            || (etag.len() > m.len() && format!("W/{m}") == etag)
                    })
            } else if let Some(ims) = h("if-modified-since") {
                match (parse_http_date(&modified), parse_http_date(ims)) {
                    (Some(l), Some(i)) => l <= i,
                    _ => false,
                }
            } else {
                false
            };
        if not_modified {
            return Some(finish(304, headers, Body::empty(), None));
        }
    }
    let mut status = 200;
    let mut offset = 0i64;
    let mut len = size;
    if let Some(range) = h("range") {
        let is_bytes = range.trim_start_matches(' ').starts_with("bytes=");
        if is_bytes {
            let fresh = match h("if-range") {
                None => true,
                Some(ir) if ir.contains('"') => ir.contains(&etag),
                Some(ir) => match (parse_http_date(ir), parse_http_date(&modified)) {
                    (Some(t), Some(l)) => l <= t,
                    (Some(_), None) => true,
                    _ => false,
                },
            };
            if fresh {
                let ranges = parse_ranges(len, range);
                if ranges.is_empty() {
                    return Some(send_error(416, "Range Not Satisfiable", &opts));
                } else if ranges.len() == 1 {
                    status = 206;
                    let (a, b) = ranges[0];
                    headers.push(("content-range", format!("bytes {a}-{b}/{len}")));
                    offset = a;
                    len = b - a + 1;
                }
            }
        }
    }
    headers.push(("content-length", len.to_string()));
    if *method == Method::HEAD {
        return Some(finish(status, headers, Body::empty(), ctype));
    }
    let body = file_body(abs, offset as u64, len as u64).await?;
    Some(finish(status, headers, body, ctype))
}

/// a streamed slice of a file
pub async fn file_body(abs: &Path, offset: u64, len: u64) -> Option<Body> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let mut f = tokio::fs::File::open(abs).await.ok()?;
    if offset > 0 {
        f.seek(std::io::SeekFrom::Start(offset)).await.ok()?;
    }
    let stream = tokio_util::io::ReaderStream::with_capacity(f.take(len), 64 * 1024);
    Some(Body::from_stream(stream))
}

/// Build a request for [`crate::Server::inject`].
pub fn request(method: &str, uri: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "localhost:80")
        .body(Body::empty())
        .expect("valid request")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_ranges(100, "bytes=0-9"), vec![(0, 9)]);
        assert_eq!(parse_ranges(100, "bytes=-10"), vec![(90, 99)]);
        assert_eq!(parse_ranges(100, "bytes=90-"), vec![(90, 99)]);
        assert_eq!(parse_ranges(100, "bytes=200-300"), vec![]);
        assert_eq!(parse_ranges(100, "bytes=0-5,3-9"), vec![(0, 9)]);
        assert_eq!(
            content_disposition("ä b\".zip"),
            "attachment; filename=\"_ b.zip\"; filename*=UTF-8''%C3%A4%20b%22.zip"
        );
    }
}
