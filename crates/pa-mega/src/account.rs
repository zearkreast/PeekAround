//! Optional MEGA account session (mirrors `remote/account.ts`).
//!
//! Anonymous downloads of public links count against the transfer quota of the IP address; with an
//! account session every API request carries `&sid=` and downloads count against the account.
//! Only the session id (+ e-mail, login time) is stored — never the password or any key; the
//! password lives for the duration of [`AuthClient::login`] only.
//!
//! Login (megajs `Storage` flow): `us0` → v2: PBKDF2-HMAC-SHA512(password, salt, 100 000) → AES key +
//! `uh` (v1: prepareKey + stringhash) → `us {user, uh, mfa?}` → master key (ECB), RSA private key
//! (ECB with the master key), session id = RSA-decrypted `csid` → `ug`.

use crate::api::{Api, ApiOptions, SessionSource};
use crate::crypto::{self, b64url, from_b64, RsaPrivateKey};
use crate::error::{
    HttpError, MegaError, EAGAIN, EBLOCKED, EFAILED, EMFAREQUIRED, ENOENT, ERATELIMIT,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::future::Future;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDetails {
    pub email: Option<String>,
    /// MEGA "utype": 0 free, 1–3 Pro I–III, 4 Pro Lite, 100 Business, 101 Pro Flexi
    pub utype: Option<f64>,
    /// bytes, when MEGA reports them
    pub transfer_used: Option<f64>,
    pub transfer_total: Option<f64>,
}

fn num(v: Option<&Value>) -> Option<f64> {
    v.and_then(Value::as_f64)
        .filter(|x| x.is_finite() && *x >= 0.0)
}

/// "ug" / "uq" answers → details
pub fn parse_details(ug: &Value, uq: Option<&Value>) -> AccountDetails {
    let mut used = num(uq.and_then(|q| q.get("caxfer")));
    // free accounts: per-hour transfer history of the current window
    if used.is_none() {
        if let Some(tah) = uq.and_then(|q| q.get("tah")).and_then(Value::as_array) {
            used = Some(tah.iter().map(|x| num(Some(x)).unwrap_or(0.0)).sum());
        }
    }
    AccountDetails {
        email: ug.get("email").and_then(Value::as_str).map(str::to_string),
        utype: num(uq.and_then(|q| q.get("utype"))),
        transfer_used: used,
        transfer_total: num(uq.and_then(|q| q.get("mxfer"))).filter(|x| *x != 0.0),
    }
}

/// account type and plan name of a utype: ('free' | 'pro' | 'business', plan)
pub fn plan_of(utype: Option<f64>) -> (Option<&'static str>, Option<&'static str>) {
    match utype {
        None => (None, None),
        Some(0.0) => (Some("free"), None),
        Some(100.0) => (Some("business"), Some("Business")),
        Some(u) => (
            Some("pro"),
            match u as i64 {
                1 => Some("Pro I"),
                2 => Some("Pro II"),
                3 => Some("Pro III"),
                4 => Some("Pro Lite"),
                101 => Some("Pro Flexi"),
                _ => None,
            },
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LoginRefusal {
    Credentials,
    MfaRequired,
    MfaInvalid,
    Blocked,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoginOutcome {
    Session { sid: String, email: String },
    Refused(LoginRefusal),
}

/// the MEGA account API (injectable for tests)
pub trait AuthClient: Send + Sync + 'static {
    fn login(
        &self,
        email: &str,
        password: &str,
        code: Option<&str>,
    ) -> impl Future<Output = Result<LoginOutcome, MegaError>> + Send;
    /// "ug" + "uq" with the session; Err with the MEGA error (ESID, …)
    fn details(&self, sid: &str) -> impl Future<Output = Result<AccountDetails, MegaError>> + Send;
    /// "sml" — ends the session on MEGA's side
    fn logout(&self, sid: &str) -> impl Future<Output = Result<(), MegaError>> + Send;
}

async fn with_timeout<T>(
    d: Duration,
    what: &str,
    f: impl Future<Output = Result<T, MegaError>>,
) -> Result<T, MegaError> {
    tokio::time::timeout(d, f)
        .await
        .unwrap_or_else(|_| Err(MegaError::Network(format!("{what} timed out"))))
}

/// The real account API.
#[derive(Clone)]
pub struct MegaAuth {
    api: Api,
}

impl MegaAuth {
    pub fn new(opts: ApiOptions) -> Self {
        Self {
            api: Api::new(ApiOptions {
                session: None,
                ..opts
            }),
        }
    }

    async fn login_inner(
        &self,
        email: &str,
        password: &str,
        code: Option<&str>,
    ) -> Result<(String, String), MegaError> {
        let email = email.to_lowercase();
        let info = self
            .api
            .request(&json!({ "a": "us0", "user": email }), &[])
            .await?;
        let pw = password.as_bytes().to_vec();
        let (aes_key, uh) = match info.get("v").and_then(Value::as_i64) {
            Some(1) => {
                let em = email.clone();
                tokio::task::spawn_blocking(move || {
                    let pk = crypto::prepare_key_v1(&pw);
                    (pk, b64url(&crypto::stringhash(&pk, em.as_bytes())))
                })
                .await
                .map_err(|e| MegaError::Other(e.to_string()))?
            }
            Some(2) => {
                let salt = from_b64(info.get("s").and_then(Value::as_str).unwrap_or(""));
                tokio::task::spawn_blocking(move || crypto::prepare_key_v2(&pw, &salt))
                    .await
                    .map_err(|e| MegaError::Other(e.to_string()))?
            }
            _ => return Err(MegaError::Invalid("Account version not supported".into())),
        };
        let mut req = json!({ "a": "us", "user": email, "uh": uh });
        if let Some(c) = code {
            req["mfa"] = json!(c);
        }
        let resp = self.api.request(&req, &[]).await?;
        let invalid = || MegaError::Other("invalid credentials".into());
        let k = from_b64(resp.get("k").and_then(Value::as_str).ok_or_else(invalid)?);
        if k.len() < 16 {
            return Err(invalid());
        }
        let master = crypto::ecb_decrypt(&aes_key, &k[..16]);
        let privk = crypto::ecb_decrypt(
            &master,
            &from_b64(
                resp.get("privk")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?,
            ),
        );
        let rsa = RsaPrivateKey::from_privk(&privk).ok_or_else(invalid)?;
        let sid = crypto::session_id(
            &rsa,
            &from_b64(
                resp.get("csid")
                    .and_then(Value::as_str)
                    .ok_or_else(invalid)?,
            ),
        )
        .ok_or_else(invalid)?;
        // megajs loadUser
        self.api
            .with_sid(&sid)
            .request(&json!({ "a": "ug" }), &[])
            .await?;
        Ok((sid, email))
    }
}

impl AuthClient for MegaAuth {
    async fn login(
        &self,
        email: &str,
        password: &str,
        code: Option<&str>,
    ) -> Result<LoginOutcome, MegaError> {
        match with_timeout(
            Duration::from_secs(45),
            "MEGA login",
            self.login_inner(email, password, code),
        )
        .await
        {
            Ok((sid, email)) => Ok(LoginOutcome::Session { sid, email }),
            Err(e) => {
                let msg = e.to_string();
                match e.code() {
                    Some(EMFAREQUIRED) => Ok(LoginOutcome::Refused(if code.is_some() {
                        LoginRefusal::MfaInvalid
                    } else {
                        LoginRefusal::MfaRequired
                    })),
                    Some(EFAILED) if code.is_some() => {
                        Ok(LoginOutcome::Refused(LoginRefusal::MfaInvalid))
                    }
                    Some(ENOENT) => Ok(LoginOutcome::Refused(LoginRefusal::Credentials)),
                    Some(EBLOCKED) => Ok(LoginOutcome::Refused(LoginRefusal::Blocked)),
                    _ if msg.contains("invalid credentials")
                        || msg.contains("could not be decrypted") =>
                    {
                        Ok(LoginOutcome::Refused(LoginRefusal::Credentials))
                    }
                    _ => Err(e),
                }
            }
        }
    }

    async fn details(&self, sid: &str) -> Result<AccountDetails, MegaError> {
        let api = self.api.with_sid(sid);
        let ug = with_timeout(
            Duration::from_secs(15),
            "MEGA account",
            api.request(&json!({ "a": "ug" }), &[]),
        )
        .await?;
        let uq = with_timeout(
            Duration::from_secs(15),
            "MEGA account",
            api.request(&json!({ "a": "uq", "xfer": 1, "pro": 1 }), &[]),
        )
        .await
        .ok();
        Ok(parse_details(&ug, uq.as_ref()))
    }

    async fn logout(&self, sid: &str) -> Result<(), MegaError> {
        with_timeout(
            Duration::from_secs(10),
            "MEGA logout",
            self.api.with_sid(sid).request(&json!({ "a": "sml" }), &[]),
        )
        .await
        .map(|_| ())
    }
}

/// HTTP errors of a failed login that is not a refusal
pub fn login_error(e: &MegaError) -> HttpError {
    if let MegaError::Http4xx(h) = e {
        return h.clone();
    }
    let msg = e.to_string();
    if matches!(e.code(), Some(EAGAIN) | Some(ERATELIMIT) | Some(-6))
        || msg.contains("ERATELIMIT")
        || msg.contains("ETOOMANY")
    {
        return HttpError::new(429, "Too many MEGA login attempts, try again later");
    }
    let lower = msg.to_lowercase();
    if matches!(e, MegaError::Network(_))
        || [
            "timed out",
            "fetch failed",
            "enotfound",
            "econnreset",
            "etimedout",
            "eai_again",
            "network",
        ]
        .iter()
        .any(|w| lower.contains(w))
    {
        return HttpError::new(502, "Cannot reach MEGA");
    }
    if msg.contains("Account version not supported") {
        return HttpError::new(400, "This MEGA account type is not supported");
    }
    static RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\b(E[A-Z]+) \((-\d+)\)").unwrap());
    match RE.captures(&msg) {
        Some(c) => HttpError::new(502, format!("MEGA login failed ({} ({}))", &c[1], &c[2])),
        None => HttpError::new(502, "MEGA login failed"),
    }
}

/// `MegaLoginRequest` of the shared API
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LoginRequest {
    #[serde(default)]
    pub email: Option<Value>,
    #[serde(default)]
    pub password: Option<Value>,
    #[serde(default)]
    pub code: Option<Value>,
}

/// normalised (email, password, code) or 400
pub fn validate_login(body: &LoginRequest) -> Result<(String, String, Option<String>), HttpError> {
    let s = |v: &Option<Value>| v.as_ref().and_then(Value::as_str).map(str::to_string);
    let email = s(&body.email)
        .map(|e| e.trim().to_lowercase())
        .unwrap_or_default();
    let password = s(&body.password).unwrap_or_default();
    let code: String = s(&body.code)
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let at = email.find('@');
    let valid_email = !email.is_empty()
        && email.len() <= 254
        && !email.chars().any(char::is_whitespace)
        && email.matches('@').count() == 1
        && at.is_some_and(|i| i > 0 && i < email.len() - 1);
    if !valid_email {
        return Err(HttpError::new(400, "A valid email address is required"));
    }
    if password.is_empty() || password.len() > 1024 {
        return Err(HttpError::new(400, "Password required"));
    }
    if !code.is_empty() && !(code.len() == 6 && code.bytes().all(|c| c.is_ascii_digit())) {
        return Err(HttpError::new(400, "The two-factor code has 6 digits"));
    }
    Ok((email, password, (!code.is_empty()).then_some(code)))
}

/// What is persisted (settings key `megaSession`): sid + e-mail + login time, or the expired flag.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StoredSession {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expired: Option<bool>,
}

/// persistence of the session (the server's settings table)
pub trait SessionStore: Send + Sync + 'static {
    fn load(&self) -> StoredSession;
    fn save(&self, s: &StoredSession);
}

/// in-memory store (tests, headless use)
#[derive(Default)]
pub struct MemoryStore(pub Mutex<StoredSession>);

impl SessionStore for MemoryStore {
    fn load(&self) -> StoredSession {
        self.0.lock().unwrap().clone()
    }
    fn save(&self, s: &StoredSession) {
        *self.0.lock().unwrap() = s.clone();
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TransferInfo {
    pub used: f64,
    pub total: Option<f64>,
}

/// `MegaAccountStatus` of the shared API
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountStatus {
    pub logged_in: bool,
    pub email: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<&'static str>,
    pub plan: Option<&'static str>,
    pub transfer: Option<TransferInfo>,
    pub expired: bool,
    pub since: Option<i64>,
}

/// `MegaLoginResult`
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum LoginResult {
    Ok { ok: bool, account: AccountStatus },
    Refused { ok: bool, reason: LoginRefusal },
}

const DETAILS_TTL: Duration = Duration::from_secs(60);
const SUSPECT_GAP: Duration = Duration::from_secs(60);

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

type OnChange = Box<dyn Fn() + Send + Sync>;

/// Login state + the session source of every MEGA request.
pub struct MegaAccount<A: AuthClient> {
    store: Arc<dyn SessionStore>,
    pub auth: Arc<A>,
    details: Mutex<Option<(Instant, AccountDetails)>>,
    loading: tokio::sync::Mutex<()>,
    last_suspect: Mutex<Option<Instant>>,
    /// session started / ended / expired: the server clears download URLs, lifts the quota pause
    /// and emits the `megaAccount` event
    on_change: OnChange,
    me: Weak<Self>,
}

impl<A: AuthClient> MegaAccount<A> {
    pub fn new(store: Arc<dyn SessionStore>, auth: A, on_change: OnChange) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            store,
            auth: Arc::new(auth),
            details: Mutex::new(None),
            loading: tokio::sync::Mutex::new(()),
            last_suspect: Mutex::new(None),
            on_change,
            me: me.clone(),
        })
    }

    fn changed(&self) {
        (self.on_change)();
    }

    pub fn status(&self) -> AccountStatus {
        let s = self.store.load();
        let logged_in = s.sid.as_deref().is_some_and(|x| !x.is_empty());
        let d = if logged_in {
            self.details.lock().unwrap().as_ref().map(|x| x.1.clone())
        } else {
            None
        };
        let (kind, plan) = plan_of(d.as_ref().and_then(|d| d.utype));
        AccountStatus {
            logged_in,
            email: d
                .as_ref()
                .and_then(|d| d.email.clone())
                .filter(|_| logged_in)
                .or(s.email.clone()),
            kind,
            plan,
            transfer: d
                .as_ref()
                .filter(|d| d.transfer_used.is_some() || d.transfer_total.is_some())
                .map(|d| TransferInfo {
                    used: d.transfer_used.unwrap_or(0.0),
                    total: d.transfer_total,
                }),
            expired: !logged_in && s.expired.unwrap_or(false),
            since: if logged_in { s.since } else { None },
        }
    }

    /// status with fresh account details; a rejected session (ESID / EBLOCKED) is marked expired
    pub async fn refreshed_status(&self, force: bool) -> AccountStatus {
        if let Some(sid) = self.sid() {
            let stale = force
                || self
                    .details
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_none_or(|(at, _)| at.elapsed() > DETAILS_TTL);
            if stale {
                let _g = self.loading.lock().await;
                match self.auth.details(&sid).await {
                    Ok(d) => {
                        if self.sid().as_deref() == Some(sid.as_str()) {
                            *self.details.lock().unwrap() = Some((Instant::now(), d));
                        }
                    }
                    Err(e) => {
                        if e.is_session_error() || e.code() == Some(EBLOCKED) {
                            self.invalid(&sid);
                        }
                        // network trouble: keep showing what is stored
                    }
                }
            }
        }
        self.status()
    }

    pub async fn login(&self, body: &LoginRequest) -> Result<LoginResult, HttpError> {
        let (email, password, code) = validate_login(body)?;
        let r = self.auth.login(&email, &password, code.as_deref()).await;
        drop(password);
        let (sid, got_email) = match r {
            Ok(LoginOutcome::Refused(reason)) => {
                return Ok(LoginResult::Refused { ok: false, reason })
            }
            Ok(LoginOutcome::Session { sid, email }) => (sid, email),
            Err(e) => return Err(login_error(&e)),
        };
        let old = self.sid();
        *self.details.lock().unwrap() = None;
        self.store.save(&StoredSession {
            sid: Some(sid.clone()),
            email: Some(if got_email.is_empty() {
                email
            } else {
                got_email
            }),
            since: Some(now_ms()),
            expired: None,
        });
        if let Some(old) = old.filter(|o| *o != sid) {
            let auth = self.auth.clone();
            tokio::spawn(async move {
                let _ = auth.logout(&old).await;
            });
        }
        self.refreshed_status(true).await;
        self.changed();
        Ok(LoginResult::Ok {
            ok: true,
            account: self.status(),
        })
    }

    /// forget the session (and the e-mail); "sml" ends it on MEGA's side (best effort)
    pub async fn logout(&self) -> AccountStatus {
        let sid = self.sid();
        self.store.save(&StoredSession::default());
        *self.details.lock().unwrap() = None;
        self.changed();
        if let Some(sid) = sid {
            let _ = self.auth.logout(&sid).await;
        }
        self.status()
    }
}

impl<A: AuthClient> SessionSource for MegaAccount<A> {
    fn sid(&self) -> Option<String> {
        self.store.load().sid.filter(|s| !s.is_empty())
    }

    fn invalid(&self, sid: &str) {
        let s = self.store.load();
        if s.sid.as_deref() != Some(sid) {
            return;
        }
        self.store.save(&StoredSession {
            email: s.email,
            expired: Some(true),
            ..Default::default()
        });
        self.changed();
    }

    /// EBLOCKED / ENOENT on a link request: usually the link, sometimes the account — ask MEGA (at most once a minute)
    fn suspect(&self, sid: &str) {
        {
            let mut l = self.last_suspect.lock().unwrap();
            if l.is_some_and(|t| t.elapsed() < SUSPECT_GAP) {
                return;
            }
            *l = Some(Instant::now());
        }
        let (Some(me), Ok(rt)) = (self.me.upgrade(), tokio::runtime::Handle::try_current()) else {
            return;
        };
        let sid = sid.to_string();
        rt.spawn(async move {
            match me.auth.details(&sid).await {
                Ok(d) => *me.details.lock().unwrap() = Some((Instant::now(), d)),
                Err(e) => {
                    if e.is_session_error() || matches!(e.code(), Some(EBLOCKED) | Some(ENOENT)) {
                        me.invalid(&sid);
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// mega-account.test.ts "maps account types and transfer numbers"
    #[test]
    fn maps_account_types_and_transfer_numbers() {
        assert_eq!(plan_of(Some(0.0)), (Some("free"), None));
        assert_eq!(plan_of(Some(1.0)), (Some("pro"), Some("Pro I")));
        assert_eq!(plan_of(Some(4.0)), (Some("pro"), Some("Pro Lite")));
        assert_eq!(plan_of(Some(100.0)), (Some("business"), Some("Business")));
        assert_eq!(plan_of(None), (None, None));
        assert_eq!(
            parse_details(
                &json!({ "email": "a@b.c" }),
                Some(&json!({ "utype": 2, "mxfer": 1000, "caxfer": 10 }))
            ),
            AccountDetails {
                email: Some("a@b.c".into()),
                utype: Some(2.0),
                transfer_used: Some(10.0),
                transfer_total: Some(1000.0)
            }
        );
        // free accounts: hourly history, no fixed total
        assert_eq!(
            parse_details(&json!({}), Some(&json!({ "utype": 0, "tah": [5, 6, "x"] }))),
            AccountDetails {
                email: None,
                utype: Some(0.0),
                transfer_used: Some(11.0),
                transfer_total: None
            }
        );
    }

    #[test]
    fn validates_login_requests() {
        let r = |e: &str, p: &str, c: Option<&str>| {
            validate_login(&LoginRequest {
                email: Some(json!(e)),
                password: Some(json!(p)),
                code: c.map(|c| json!(c)),
            })
        };
        assert_eq!(
            r(" A@B.c ", "pw", Some("123 456")).unwrap(),
            ("a@b.c".into(), "pw".into(), Some("123456".into()))
        );
        assert_eq!(r("not-an-email", "pw", None).unwrap_err().status, 400);
        assert_eq!(
            r("a@b.c", "pw", Some("12")).unwrap_err().message,
            "The two-factor code has 6 digits"
        );
        assert_eq!(
            r("a@b.c", "", None).unwrap_err().message,
            "Password required"
        );
        assert_eq!(login_error(&MegaError::Api(ERATELIMIT)).status, 429);
        assert_eq!(
            login_error(&MegaError::Network("x".into())).message,
            "Cannot reach MEGA"
        );
        assert_eq!(
            login_error(&MegaError::Api(-11)).message,
            "MEGA login failed (EACCESS (-11))"
        );
        let ok = serde_json::to_value(LoginResult::Refused {
            ok: false,
            reason: LoginRefusal::MfaRequired,
        })
        .unwrap();
        assert_eq!(ok, json!({ "ok": false, "reason": "mfa-required" }));
    }
}
