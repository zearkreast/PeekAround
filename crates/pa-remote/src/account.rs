//! The optional MEGA account (`remote/account.ts`) wired to the remote folders: the account lives
//! in pa-mega ([`MegaAccount`] — login with 2FA, `ug` / `uq` status, `sml` logout, ESID / expiry
//! handling, only the sid persisted through [`SessionStore`]); this glue builds the MEGA client
//! that sends the session with every request and runs `changed()` on the service: download URLs
//! cleared, quota pause lifted (`resume_transfers`), `megaAccount` event.
//!
//! Routes (all three behind `requireLocal` in the server — 403 for the desktop's LAN listener):
//! `GET /api/remote/account` → `account.refreshed_status(false)`, `POST …/login` →
//! `account.login(&body)` (`LoginRequest`; the body holds the password: never logged),
//! `POST …/logout` → `account.logout()`.

use crate::service::RemoteService;
use pa_mega::{ApiOptions, MegaAccount, MegaAuth, MegaClient, SessionStore};
use std::sync::{Arc, OnceLock, Weak};

pub struct RemoteAccount {
    pub account: Arc<MegaAccount<MegaAuth>>,
    /// the MEGA client of every link request (listing, ranged reads, attributes, imports)
    pub client: MegaClient,
    svc: Arc<OnceLock<Weak<RemoteService>>>,
}

impl RemoteAccount {
    /// `opts.session` is replaced by the account
    pub fn new(store: Arc<dyn SessionStore>, opts: ApiOptions) -> Self {
        let svc: Arc<OnceLock<Weak<RemoteService>>> = Arc::new(OnceLock::new());
        let s2 = svc.clone();
        let account = MegaAccount::new(
            store,
            MegaAuth::new(opts.clone()),
            Box::new(move || {
                if let Some(s) = s2.get().and_then(Weak::upgrade) {
                    s.account_changed();
                }
            }),
        );
        let client = MegaClient::new(ApiOptions {
            session: Some(account.clone()),
            ..opts
        });
        Self {
            account,
            client,
            svc,
        }
    }

    /// route the account's `changed()` to the service (call once, after `RemoteService::new`)
    pub fn bind(&self, svc: &Arc<RemoteService>) {
        let _ = self.svc.set(Arc::downgrade(svc));
    }
}
