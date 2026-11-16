//! Trash page: `GET /api/trash`, grouped by the local day of deletion, previews from
//! `/api/trash/thumb`, restore / delete for good / empty.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use serde::Deserialize;
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::api::enc;
use crate::app::AppInner;
use crate::i18n::{self, Lang};
use crate::{TrashGroup, TrashRow, TrashState};

const DAY_MS: i64 = 86_400_000;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    id: String,
    folder_name: String,
    rel: String,
    name: String,
    kind: Option<String>,
    deleted_at: i64,
    purge_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrashResponse {
    items: Vec<Item>,
    bytes: i64,
    keep_days: i64,
}

#[derive(Default)]
struct State {
    items: Vec<Item>,
    selected: HashSet<String>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// "Bugün silinenler", "Dün silinenler", "5 Kasım 2026"
fn day_title(local_day: i64, today: i64, lang: Lang) -> String {
    match today - local_day {
        0 => i18n::t(lang, "trash.today"),
        1 => i18n::t(lang, "trash.yesterday"),
        _ => crate::viewer_fmt::date_long(local_day * DAY_MS, lang),
    }
}

fn local_day(ms: i64) -> i64 {
    (ms + crate::viewer_fmt::local_offset_at(ms)).div_euclid(DAY_MS)
}

/// whole days until the purge, at least 0
fn days_left(purge_at: i64, now: i64) -> i64 {
    ((purge_at - now) as f64 / DAY_MS as f64).ceil().max(0.0) as i64
}

pub(crate) fn enter(app: &Rc<AppInner>) {
    let st = app.window.global::<TrashState>();
    st.set_loading(true);
    STATE.with(|s| s.borrow_mut().selected.clear());
    load(app);
}

fn load(app: &Rc<AppInner>) {
    crate::api::call::<TrashResponse>(app, "GET", "/api/trash", None, |a, r| {
        let st = a.window.global::<TrashState>();
        st.set_loading(false);
        match r {
            Ok(r) => {
                let lang = a.lang();
                st.set_subtitle(
                    i18n::tv(
                        lang,
                        "trash.subtitle",
                        &[
                            ("n", &r.items.len().to_string()),
                            ("size", &crate::fmt::format_bytes(r.bytes)),
                            ("days", &r.keep_days.to_string()),
                        ],
                    )
                    .into(),
                );
                STATE.with(|s| {
                    let mut s = s.borrow_mut();
                    let ids: HashSet<String> = r.items.iter().map(|i| i.id.clone()).collect();
                    s.selected.retain(|id| ids.contains(id));
                    s.items = r.items;
                });
                publish(a);
                load_previews(a);
            }
            Err(e) => a.error_toast(&e.message),
        }
    });
}

fn publish(app: &Rc<AppInner>) {
    let lang = app.lang();
    let now = crate::fmt::now_ms();
    let today = local_day(now);
    let (rows, groups, n_sel) = STATE.with(|s| {
        let s = s.borrow();
        let mut rows = Vec::new();
        let mut groups: Vec<TrashGroup> = Vec::new();
        let mut last_day = None;
        for (i, it) in s.items.iter().enumerate() {
            let day = local_day(it.deleted_at);
            if last_day != Some(day) {
                last_day = Some(day);
                groups.push(TrashGroup {
                    title: day_title(day, today, lang).into(),
                    count: 0,
                    start: i as i32,
                });
            }
            if let Some(g) = groups.last_mut() {
                g.count += 1;
            }
            let left = days_left(it.purge_at, now);
            let dir = it.rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
            rows.push(TrashRow {
                id: it.id.clone().into(),
                name: it.name.clone().into(),
                place: if dir.is_empty() {
                    it.folder_name.clone()
                } else {
                    format!("{} · {dir}", it.folder_name)
                }
                .into(),
                left: if left <= 1 {
                    i18n::t(lang, "trash.lastDay")
                } else {
                    i18n::tv(lang, "trash.left", &[("n", &left.to_string())])
                }
                .into(),
                soon: left <= 3,
                video: it.kind.as_deref() == Some("video"),
                selected: s.selected.contains(&it.id),
                has_image: false,
                image: Default::default(),
            });
        }
        (rows, groups, s.selected.len())
    });
    let st = app.window.global::<TrashState>();
    // keep previews already loaded
    let old = st.get_rows();
    let mut rows = rows;
    for r in rows.iter_mut() {
        if let Some(o) = old.iter().find(|o| o.id == r.id && o.has_image) {
            r.has_image = true;
            r.image = o.image.clone();
        }
    }
    st.set_rows(ModelRc::new(VecModel::from(rows)));
    st.set_groups(ModelRc::new(VecModel::from(groups)));
    st.set_selected(n_sel as i32);
}

fn load_previews(app: &Rc<AppInner>) {
    let ids: Vec<String> = STATE.with(|s| {
        s.borrow()
            .items
            .iter()
            .filter(|i| i.kind.as_deref() == Some("image"))
            .map(|i| i.id.clone())
            .collect()
    });
    let epoch = app.pages.epoch.get();
    for id in ids {
        let fut = app.backend.get_bytes(&format!("/api/trash/thumb?id={}", enc(&id)));
        let weak = Rc::downgrade(app);
        app.rt.spawn_then(
            async move {
                let b = fut.await.ok()?;
                crate::rt::spawn_blocking(move || {
                    crate::thumbs::decode(crate::backend::ThumbData::Encoded(b))
                })
                .await
                .ok()
                .flatten()
            },
            move |px| {
                let (Some(a), Some(px)) = (weak.upgrade(), px) else { return };
                if a.pages.epoch.get() != epoch {
                    return;
                }
                let rows = a.window.global::<TrashState>().get_rows();
                for i in 0..rows.row_count() {
                    if let Some(mut r) = rows.row_data(i).filter(|r| r.id.as_str() == id) {
                        r.image = slint::Image::from_rgba8(px.clone());
                        r.has_image = true;
                        rows.set_row_data(i, r);
                    }
                }
            },
        );
    }
}

fn selected_ids() -> Vec<String> {
    STATE.with(|s| {
        let s = s.borrow();
        s.items
            .iter()
            .filter(|i| s.selected.contains(&i.id))
            .map(|i| i.id.clone())
            .collect()
    })
}

fn after(a: &Rc<AppInner>, key: &str, n: usize) {
    STATE.with(|s| s.borrow_mut().selected.clear());
    a.toast(
        crate::ToastKind::Success,
        &i18n::tv(a.lang(), key, &[("n", &n.to_string())]),
        "",
    );
    load(a);
}

/// Callbacks of `TrashState`.
pub(crate) fn wire(app: &Rc<AppInner>) {
    let st = app.window.global::<TrashState>();
    let w = Rc::downgrade(app);
    st.on_toggle(move |i| {
        let Some(a) = w.upgrade() else { return };
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            if let Some(id) = s.items.get(i as usize).map(|it| it.id.clone()) {
                if !s.selected.remove(&id) {
                    s.selected.insert(id);
                }
            }
        });
        publish(&a);
    });
    let w = Rc::downgrade(app);
    st.on_clear_selection(move || {
        let Some(a) = w.upgrade() else { return };
        STATE.with(|s| s.borrow_mut().selected.clear());
        publish(&a);
    });
    let w = Rc::downgrade(app);
    st.on_restore_selected(move || {
        let Some(a) = w.upgrade() else { return };
        let ids = selected_ids();
        let n = ids.len();
        crate::api::act(&a, "POST", "/api/trash/restore", Some(serde_json::json!({ "ids": ids })), move |a, _| {
            after(a, "trash.restored", n);
        });
    });
    let w = Rc::downgrade(app);
    st.on_delete_selected(move || {
        let Some(a) = w.upgrade() else { return };
        let ids = selected_ids();
        let n = ids.len();
        let lang = a.lang();
        crate::pages::dialogs::confirm(
            &a,
            &i18n::tv(lang, "trash.confirmDelete", &[("n", &n.to_string())]),
            &i18n::t(lang, "trash.confirmBody"),
            &i18n::t(lang, "trash.deleteForGood"),
            true,
            move |a| {
                crate::api::act(a, "POST", "/api/trash/delete", Some(serde_json::json!({ "ids": ids })), move |a, _| {
                    after(a, "trash.deleted", n);
                });
            },
        );
    });
    let w = Rc::downgrade(app);
    st.on_empty(move || {
        let Some(a) = w.upgrade() else { return };
        let n = STATE.with(|s| s.borrow().items.len());
        let lang = a.lang();
        crate::pages::dialogs::confirm(
            &a,
            &i18n::t(lang, "trash.confirmEmpty"),
            &i18n::t(lang, "trash.confirmBody"),
            &i18n::t(lang, "trash.empty"),
            true,
            move |a| {
                crate::api::act(a, "POST", "/api/trash/empty", Some(serde_json::json!({})), move |a, _| {
                    after(a, "trash.deleted", n);
                });
            },
        );
    });
}

/// The hub tile: "Trash · 12" when the server keeps a folder trash that holds something.
pub(crate) fn load_hub_label(app: &Rc<AppInner>) {
    if crate::actions_ui::ctx().trash != Some(pa_core::TrashMode::Folder) {
        app.window.global::<crate::HubState>().set_trash_label("".into());
        return;
    }
    crate::api::call::<TrashResponse>(app, "GET", "/api/trash", None, |a, r| {
        let label = match r {
            Ok(r) if !r.items.is_empty() => {
                i18n::tv(a.lang(), "trash.hub", &[("n", &r.items.len().to_string())])
            }
            _ => String::new(),
        };
        a.window.global::<crate::HubState>().set_trash_label(label.into());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_left_rounds_up() {
        let now = 1_000 * DAY_MS;
        assert_eq!(days_left(now + 30 * DAY_MS, now), 30);
        assert_eq!(days_left(now + 29 * DAY_MS + 1, now), 30);
        assert_eq!(days_left(now + 1, now), 1);
        assert_eq!(days_left(now - 5, now), 0);
    }
}
