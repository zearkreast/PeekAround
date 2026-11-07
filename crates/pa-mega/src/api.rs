//! The MEGA API request layer ("cs" commands) — megajs `API.request` + the session wrapper of
//! `remote/mega.ts` (`sessionApi`):
//!
//! - `POST <gateway>cs?id=<seq>[&sid=…][&n=<folder link id>]`, body `[command]`;
//! - EAGAIN (-3, also "server too busy" / HTTP 5xx) is retried 4 times after 2, 4, 8, 16 s;
//! - an `X-Hashcash` challenge is answered and the request repeated;
//! - with a session source the current sid is read on EVERY request; ESID (-15) drops the session
//!   and retries once anonymously; EBLOCKED / ENOENT make the source verify the session.

use crate::crypto::hashcash;
use crate::error::{MegaError, EAGAIN, EBLOCKED, ENOENT};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const DEFAULT_GATEWAY: &str = "https://g.api.mega.co.nz/";
const MAX_RETRIES: u32 = 4;

/// The optional account session, read on every request (links loaded before a login / logout
/// switch accounts without being re-listed).
pub trait SessionSource: Send + Sync {
    /// current session id, or None (anonymous)
    fn sid(&self) -> Option<String>;
    /// MEGA rejected the session (ESID): forget it — the request is retried anonymously
    fn invalid(&self, sid: &str);
    /// a link request failed in a way that can also mean a blocked / removed account
    fn suspect(&self, _sid: &str) {}
}

#[derive(Clone)]
pub struct ApiOptions {
    /// API gateway (tests point it at a local fake MEGA); must end with '/'
    pub gateway: String,
    pub session: Option<Arc<dyn SessionSource>>,
    /// unit of the EAGAIN back-off (2^(n+1) × this); 1 s like megajs
    pub retry_unit: Duration,
    /// per-request timeout of the HTTP client
    pub timeout: Duration,
    pub user_agent: Option<String>,
}

impl Default for ApiOptions {
    fn default() -> Self {
        Self {
            gateway: DEFAULT_GATEWAY.into(),
            session: None,
            retry_unit: Duration::from_secs(1),
            timeout: Duration::from_secs(60),
            user_agent: Some(format!("PeekAround/{}", env!("CARGO_PKG_VERSION"))),
        }
    }
}

/// A MEGA API endpoint (cheap to clone).
#[derive(Clone)]
pub struct Api {
    pub(crate) http: reqwest::Client,
    opts: ApiOptions,
    counter: Arc<AtomicU64>,
    /// fixed session (account calls: ug / uq / sml with a given sid)
    fixed_sid: Arc<Mutex<Option<String>>>,
}

impl Api {
    pub fn new(opts: ApiOptions) -> Self {
        let mut b = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(opts.timeout)
            .pool_idle_timeout(Duration::from_secs(60));
        if let Some(ua) = &opts.user_agent {
            b = b.user_agent(ua.clone());
        }
        let http = b.build().expect("http client");
        let start = 1_000_000_000 + rand::random::<u64>() % 8_000_000_000;
        Self {
            http,
            opts,
            counter: Arc::new(AtomicU64::new(start)),
            fixed_sid: Arc::new(Mutex::new(None)),
        }
    }

    pub fn gateway(&self) -> &str {
        &self.opts.gateway
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// a copy that always sends `sid` (account API calls)
    pub fn with_sid(&self, sid: &str) -> Api {
        Api {
            opts: ApiOptions {
                session: None,
                ..self.opts.clone()
            },
            fixed_sid: Arc::new(Mutex::new(Some(sid.to_string()))),
            ..self.clone()
        }
    }

    fn current_sid(&self) -> Option<String> {
        if let Some(s) = self.fixed_sid.lock().unwrap().clone() {
            return Some(s);
        }
        self.opts.session.as_ref().and_then(|s| s.sid())
    }

    /// One command; `query` adds query parameters (e.g. `n` = folder link id).
    pub async fn request(&self, cmd: &Value, query: &[(&str, &str)]) -> Result<Value, MegaError> {
        let used = self.current_sid();
        let r = self.raw_request(cmd, query, used.as_deref()).await;
        if let (Err(e), Some(used), Some(session)) = (&r, &used, &self.opts.session) {
            if self.fixed_sid.lock().unwrap().is_none() {
                if e.is_session_error() {
                    session.invalid(used);
                    if session.sid().as_deref() != Some(used.as_str()) {
                        let again = session.sid();
                        return self.raw_request(cmd, query, again.as_deref()).await;
                    }
                } else if matches!(e.code(), Some(EBLOCKED) | Some(ENOENT)) {
                    session.suspect(used);
                }
            }
        }
        r
    }

    async fn raw_request(
        &self,
        cmd: &Value,
        query: &[(&str, &str)],
        sid: Option<&str>,
    ) -> Result<Value, MegaError> {
        let mut hashcash_token: Option<String> = None;
        let mut retry = 0u32;
        let mut challenges = 0u32;
        loop {
            let id = self.counter.fetch_add(1, Ordering::Relaxed).to_string();
            let mut url = reqwest::Url::parse(&format!("{}cs", self.opts.gateway))
                .map_err(|e| MegaError::Other(e.to_string()))?;
            {
                let mut q = url.query_pairs_mut();
                q.append_pair("id", &id);
                if let Some(s) = sid {
                    q.append_pair("sid", s);
                }
                for (k, v) in query {
                    q.append_pair(k, v);
                }
            }
            let mut req = self
                .http
                .post(url)
                .header("Content-Type", "application/json")
                .body(serde_json::to_vec(&[cmd]).unwrap());
            if let Some(t) = &hashcash_token {
                req = req.header("X-Hashcash", t);
            }
            let res = req.send().await?;
            let code: Option<i32> = if let Some(ch) = res
                .headers()
                .get("x-hashcash")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
            {
                challenges += 1;
                if challenges > 3 {
                    return Err(MegaError::Invalid("MEGA kept asking for hashcash".into()));
                }
                hashcash_token = tokio::task::spawn_blocking(move || hashcash(&ch))
                    .await
                    .ok()
                    .flatten();
                if hashcash_token.is_none() {
                    return Err(MegaError::Invalid("unsupported hashcash challenge".into()));
                }
                continue;
            } else if res.status().is_server_error() {
                Some(EAGAIN)
            } else if !res.status().is_success() {
                return Err(MegaError::Invalid(format!(
                    "Server returned error: {}",
                    res.status()
                        .canonical_reason()
                        .unwrap_or(res.status().as_str())
                )));
            } else {
                let body = res.bytes().await?;
                let v: Value = serde_json::from_slice(&body)
                    .map_err(|_| MegaError::Invalid("MEGA returned no JSON".into()))?;
                let v = match v {
                    Value::Array(mut a) if !a.is_empty() => a.swap_remove(0),
                    v => v,
                };
                match v.as_i64() {
                    Some(n) if n < 0 => Some(n as i32),
                    _ => return Ok(v),
                }
            };
            let code = code.unwrap();
            if code == EAGAIN && retry < MAX_RETRIES {
                tokio::time::sleep(self.opts.retry_unit * 2u32.pow(retry + 1)).await;
                retry += 1;
                continue;
            }
            return Err(MegaError::Api(code));
        }
    }
}
