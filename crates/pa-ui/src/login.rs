//! Sign-in to a server with a password (PEEK_PASSWORD): a 401 shows the sign-in screen; a session
//! token is kept per server in the prefs and sent with every request.

use std::rc::Rc;

use slint::ComponentHandle;

use crate::app::AppInner;
use crate::{Login, Shell};

/// Use the token saved for this server, if any (before the first request).
pub(crate) fn restore(app: &Rc<AppInner>) {
    let label = app.backend.label();
    if let Some(t) = app.prefs.borrow().sessions.get(&label) {
        app.backend.set_session(Some(t.clone()));
    }
}

/// The server answered 401: forget the stored token and ask for the password.
pub(crate) fn show(app: &Rc<AppInner>) {
    let shell = app.window.global::<Shell>();
    if shell.get_need_login() {
        return;
    }
    let label = app.backend.label();
    if app.prefs.borrow_mut().sessions.remove(&label).is_some() {
        app.save_prefs();
    }
    app.backend.set_session(None);
    let l = app.window.global::<Login>();
    l.set_server(label.trim_start_matches("http://").trim_start_matches("https://").into());
    l.set_error("".into());
    l.set_busy(false);
    shell.set_need_login(true);
}

#[derive(serde::Deserialize)]
struct LoginResponse {
    token: Option<String>,
}

/// Callbacks of the `Login` global.
pub(crate) fn install(app: &Rc<AppInner>) {
    let l = app.window.global::<Login>();
    let w = Rc::downgrade(app);
    l.on_submit(move |password, remember| {
        let Some(a) = w.upgrade() else { return };
        let l = a.window.global::<Login>();
        l.set_busy(true);
        l.set_error("".into());
        let body = serde_json::json!({ "password": password.as_str(), "remember": remember });
        let fut = a.backend.request("POST", "/api/auth/login", Some(body));
        let w2 = Rc::downgrade(&a);
        a.rt.spawn_then(fut, move |r| {
            let Some(a) = w2.upgrade() else { return };
            let l = a.window.global::<Login>();
            l.set_busy(false);
            match r.map(|v| serde_json::from_value::<LoginResponse>(v).ok().and_then(|r| r.token)) {
                Ok(token) => {
                    if let Some(t) = &token {
                        if remember {
                            let label = a.backend.label();
                            a.prefs.borrow_mut().sessions.insert(label, t.clone());
                            a.save_prefs();
                        }
                    }
                    a.backend.set_session(token);
                    a.window.global::<Shell>().set_need_login(false);
                    signed_in(&a);
                }
                Err(e) if e.status == 401 => l.set_error(a.t("login.wrong").into()),
                Err(e) => l.set_error(e.message.into()),
            }
        });
    });
}

/// Everything that failed while signed out loads again.
fn signed_in(a: &Rc<AppInner>) {
    crate::app::load_info(a);
    a.grid.0.borrow_mut().reload();
    crate::pages::reload_all(a);
    if let Some(ex) = crate::explore::get() {
        ex.load();
    }
    let route = a.route.borrow().clone();
    a.go(route, false);
}
