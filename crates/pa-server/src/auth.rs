//! Optional password login (PEEK_PASSWORD): sessions, the request gate and the /api/auth routes.
//! Without a password nothing changes. Tokens are random; the database keeps only their SHA-256.

use std::collections::HashMap;

use parking_lot::Mutex;
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::ctx::C;
use crate::error::HttpError;
use crate::events::now_ms;
use crate::http::{ok_json, set_header, BodyScope, Req};
use crate::router::{h, Router};
use crate::security::Limit::Heavy;

const DAY_MS: i64 = 86_400_000;
/// session lifetime: "keep me signed in" / otherwise
const LONG_DAYS: i64 = 30;
const SHORT_DAYS: i64 = 1;
/// settings key of the stored sessions (hash → expiry)
const KEY: &str = "authSessions";
/// cookie of the browser UI (the WebSocket cannot send a header)
pub const COOKIE: &str = "peek_session";

/// Live sessions, loaded from the database on first use.
#[derive(Default)]
pub struct Sessions {
    map: Mutex<Option<HashMap<String, i64>>>,
}

fn hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

impl Sessions {
    fn with<T>(&self, ctx: &C, f: impl FnOnce(&mut HashMap<String, i64>) -> T) -> T {
        let mut g = self.map.lock();
        let m = g.get_or_insert_with(|| {
            ctx.db
                .get_setting(KEY)
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default()
        });
        f(m)
    }

    fn save(ctx: &C, m: &HashMap<String, i64>) {
        let _ = ctx.db.set_setting(KEY, &json!(m));
    }

    /// a new session; returns (token, expiry)
    pub fn create(&self, ctx: &C, long: bool) -> (String, i64) {
        let token = hex::encode(rand::random::<[u8; 32]>());
        let exp = now_ms() + if long { LONG_DAYS } else { SHORT_DAYS } * DAY_MS;
        self.with(ctx, |m| {
            let now = now_ms();
            m.retain(|_, e| *e > now);
            m.insert(hash(&token), exp);
            Self::save(ctx, m);
        });
        (token, exp)
    }

    /// the token names a live session
    pub fn valid(&self, ctx: &C, token: &str) -> bool {
        let h = hash(token);
        self.with(ctx, |m| m.get(&h).is_some_and(|e| *e > now_ms()))
    }

    pub fn remove(&self, ctx: &C, token: &str) {
        let h = hash(token);
        self.with(ctx, |m| {
            if m.remove(&h).is_some() {
                Self::save(ctx, m);
            }
        });
    }
}

/// a password is set: every API request needs a session (or a paired phone / the server itself)
pub fn required(ctx: &C) -> bool {
    ctx.security.password.as_deref().is_some_and(|p| !p.is_empty())
}

/// Paths that work without a session: the web UI's files (it has to show the login), the login
/// itself, the healthcheck, and phone pairing / the device data plane (they authenticate as devices).
pub fn open_path(path: &str) -> bool {
    !path.starts_with("/api/")
        || path.starts_with("/api/auth")
        || path.starts_with("/api/devices/")
}

/// The session token of a request: `Authorization: Bearer`, `?psess=` (WebSocket, media URLs of
/// native clients) or the browser cookie.
pub fn token_of(headers: &axum::http::HeaderMap, query: Option<&str>) -> Option<String> {
    let bearer = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_string);
    let q = query.and_then(|q| {
        q.split('&')
            .filter_map(|kv| kv.split_once('='))
            .find(|(k, _)| *k == "psess")
            .map(|(_, v)| v.to_string())
    });
    let cookie = headers
        .get_all("cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string());
    bearer.or(q).or(cookie)
}

/// A request may pass the gate: a session, a paired phone, or the server's own loopback reads.
pub fn authorized(ctx: &C, req: &Req) -> bool {
    if req
        .header("x-peek-internal")
        .is_some_and(|v| crate::security::ct_eq(v.as_bytes(), ctx.devices.internal_token.as_bytes()))
    {
        return true;
    }
    if ctx.devices.paired_device(ctx, &crate::devices::device_auth_of(req)).is_some() {
        return true;
    }
    token_of(&req.parts.headers, req.parts.uri.query()).is_some_and(|t| ctx.sessions.valid(ctx, &t))
}

fn login_failed() -> HttpError {
    HttpError::new(401, "Wrong password")
}

pub fn routes(r: &mut Router) {
    let m = BodyScope::Main;
    r.add(
        "GET",
        "/api/auth",
        Heavy,
        m,
        h(|ctx, req: Req| async move {
            let ok = !required(&ctx)
                || token_of(&req.parts.headers, req.parts.uri.query())
                    .is_some_and(|t| ctx.sessions.valid(&ctx, &t));
            ok_json(&json!({ "required": required(&ctx), "ok": ok }))
        }),
    );
    r.add(
        "POST",
        "/api/auth/login",
        Heavy,
        m,
        h(|ctx, req: Req| async move {
            let Some(pw) = ctx.security.password.clone().filter(|p| !p.is_empty()) else {
                return ok_json(&json!({ "token": null }));
            };
            let given = req
                .body
                .opt("password")
                .map(crate::http::js_string)
                .unwrap_or_default();
            // compare digests: equal length, no early exit
            let ok = crate::security::ct_eq(&Sha256::digest(given.as_bytes()), &Sha256::digest(pw.as_bytes()));
            if !ok {
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                return Err(login_failed());
            }
            let long = req.body.opt("remember").is_some_and(crate::routes::truthy);
            let (token, exp) = ctx.sessions.create(&ctx, long);
            let mut resp = ok_json(&json!({ "token": token, "expiresAt": exp }))?;
            let age = (exp - now_ms()) / 1000;
            set_header(
                &mut resp,
                "set-cookie",
                &format!("{COOKIE}={token}; Path=/; Max-Age={age}; HttpOnly; SameSite=Strict"),
            );
            Ok(resp)
        }),
    )
    // passwords of any sensible length (thousands of characters); a bigger body is refused (413)
    .body_limit = Some(64 * 1024);
    r.add(
        "POST",
        "/api/auth/logout",
        Heavy,
        m,
        h(|ctx, req: Req| async move {
            if let Some(t) = token_of(&req.parts.headers, req.parts.uri.query()) {
                ctx.sessions.remove(&ctx, &t);
            }
            let mut resp = ok_json(&json!({ "ok": true }))?;
            set_header(&mut resp, "set-cookie", &format!("{COOKIE}=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict"));
            Ok(resp)
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_from_headers_query_and_cookie() {
        let mut h = axum::http::HeaderMap::new();
        assert_eq!(token_of(&h, None), None);
        assert_eq!(token_of(&h, Some("a=1&psess=abc")).as_deref(), Some("abc"));
        h.insert("cookie", "x=1; peek_session=ck; y=2".parse().unwrap());
        assert_eq!(token_of(&h, None).as_deref(), Some("ck"));
        h.insert("authorization", "Bearer hd".parse().unwrap());
        assert_eq!(token_of(&h, Some("psess=q")).as_deref(), Some("hd"));
        assert!(open_path("/") && open_path("/assets/app.wasm") && open_path("/api/auth/login"));
        assert!(!open_path("/api/info") && !open_path("/api/events") && !open_path("/api/browse"));
    }
}
