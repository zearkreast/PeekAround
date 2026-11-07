//! Albums list (AlbumsPage.tsx) and one album (AlbumPage.tsx).

use std::rc::Rc;

use pa_core::{Album, AlbumsResponse};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::api::{self, enc};
use crate::app::AppInner;
use crate::fmt;
use crate::i18n;
use crate::router::Route;
use crate::{AlbumState, AlbumsState, GridState, ToastKind};

/// GET /api/albums → albums page + hub strip
pub(crate) fn load_albums(app: &Rc<AppInner>) {
    api::call::<AlbumsResponse>(app, "GET", "/api/albums", None, |a, r| match r {
        Ok(l) => {
            *a.pages.albums.borrow_mut() = Some(l.albums);
            publish(a);
            super::hub::publish_albums(a);
            super::dialogs::on_albums(a);
        }
        Err(e) => {
            if e.status != 501 {
                log::warn!("albums: {e}");
            }
            a.window.global::<AlbumsState>().set_loaded(true);
            a.window.global::<crate::HubState>().set_albums_loaded(true);
        }
    });
}

fn publish(a: &Rc<AppInner>) {
    let st = a.window.global::<AlbumsState>();
    let lang = a.lang();
    let list = a.pages.albums.borrow().clone().unwrap_or_default();
    let model = Rc::new(VecModel::from(
        list.iter()
            .map(|al| super::hub::album_card(lang, al))
            .collect::<Vec<_>>(),
    ));
    st.set_albums(ModelRc::from(model.clone()));
    st.set_loaded(true);
    st.set_count_text(if list.is_empty() {
        "".into()
    } else {
        i18n::tv(lang, "album.count", &[("n", &list.len().to_string())]).into()
    });
    for al in &list {
        if let Some(c) = &al.cover_id {
            let id = al.id.clone();
            let m = model.clone();
            super::cover(a, c, 640, move |img| {
                super::hub::set_album_cover(&m, &id, img)
            });
        }
    }
}

fn show_album(a: &Rc<AppInner>, al: &Album) {
    let lang = a.lang();
    let st = a.window.global::<AlbumState>();
    let order_changed = a
        .pages
        .album
        .borrow()
        .as_ref()
        .map(|x| (x.id.clone(), x.order))
        != Some((al.id.clone(), al.order));
    *a.pages.album.borrow_mut() = Some(al.clone());
    st.set_id(al.id.clone().into());
    st.set_name(al.name.clone().into());
    let mut sub = i18n::tv(
        lang,
        "album.items",
        &[("n", &i18n::format_count(lang, al.count))],
    );
    if al.photos > 0 || al.videos > 0 {
        sub.push_str(" · ");
        sub.push_str(&i18n::tv(
            lang,
            "home.stats",
            &[
                ("photos", &i18n::format_count(lang, al.photos)),
                ("videos", &i18n::format_count(lang, al.videos)),
            ],
        ));
    }
    if let Some(r) = fmt::range_label(al.from, al.to, lang) {
        sub.push_str(" · ");
        sub.push_str(&r);
    }
    st.set_sub(sub.into());
    st.set_order(
        match al.order {
            pa_core::AlbumOrder::Asc => "asc",
            pa_core::AlbumOrder::Custom => "custom",
            pa_core::AlbumOrder::Desc => "desc",
        }
        .into(),
    );
    st.set_cover_picked(al.cover_picked);
    st.set_loaded(true);
    st.set_error("".into());
    a.window
        .global::<GridState>()
        .set_title(al.name.clone().into());
    match &al.cover_id {
        Some(c) => {
            let w = Rc::downgrade(a);
            let id = al.id.clone();
            super::cover(a, c, 640, move |img| {
                if let Some(a) = w.upgrade() {
                    let st = a.window.global::<AlbumState>();
                    if st.get_id() == id {
                        st.set_cover(img);
                        st.set_has_cover(true);
                    }
                }
            });
        }
        None => st.set_has_cover(false),
    }
    if order_changed {
        a.load_page_query();
    }
}

/// `/album/:id`
pub(crate) fn enter_album(app: &Rc<AppInner>, id: &str) {
    let st = app.window.global::<AlbumState>();
    if st.get_id() != id {
        st.set_has_cover(false);
        st.set_name("".into());
        st.set_sub("".into());
        *app.pages.album.borrow_mut() = None;
    }
    // the grid waits for the album (its order)
    if let Some(al) = app
        .pages
        .albums
        .borrow()
        .as_ref()
        .and_then(|l| l.iter().find(|x| x.id == id).cloned())
    {
        show_album(app, &al);
    }
    reload_album(app, id);
}

/// GET /api/albums/:id
pub(crate) fn reload_album(app: &Rc<AppInner>, id: &str) {
    let id2 = id.to_string();
    api::call::<Album>(
        app,
        "GET",
        &format!("/api/albums/{}", enc(id)),
        None,
        move |a, r| {
            if *a.route.borrow() != Route::Album(id2.clone()) {
                return;
            }
            match r {
                Ok(al) => show_album(a, &al),
                Err(e) => {
                    let st = a.window.global::<AlbumState>();
                    st.set_error(e.message.clone().into());
                    st.set_loaded(true);
                    a.error_toast(&e.message);
                }
            }
        },
    );
}

fn patch(a: &Rc<AppInner>, body: serde_json::Value) {
    let Some(id) = a.pages.album.borrow().as_ref().map(|x| x.id.clone()) else {
        return;
    };
    api::call::<Album>(
        a,
        "PATCH",
        &format!("/api/albums/{}", enc(&id)),
        Some(body),
        |a, r| match r {
            Ok(al) => {
                show_album(a, &al);
                load_albums(a);
            }
            Err(e) => a.error_toast(&e.message),
        },
    );
}

pub(crate) fn install(app: &Rc<AppInner>) {
    let st = app.window.global::<AlbumsState>();
    let w = Rc::downgrade(app);
    st.on_open(move |id| {
        if let Some(a) = w.upgrade() {
            a.go(Route::Album(id.into()), true);
        }
    });
    let w = Rc::downgrade(app);
    app.window.global::<crate::Shell>().on_new_album(move || {
        if let Some(a) = w.upgrade() {
            super::dialogs::new_album(&a);
        }
    });

    let st = app.window.global::<AlbumState>();
    let w = Rc::downgrade(app);
    st.on_rename(move |n| {
        if let Some(a) = w.upgrade() {
            let n = n.trim().to_string();
            if !n.is_empty() {
                patch(&a, serde_json::json!({ "name": n }));
            }
        }
    });
    let w = Rc::downgrade(app);
    st.on_set_order(move |o| {
        if let Some(a) = w.upgrade() {
            patch(&a, serde_json::json!({ "order": o.as_str() }));
        }
    });
    let w = Rc::downgrade(app);
    st.on_reset_cover(move || {
        if let Some(a) = w.upgrade() {
            patch(&a, serde_json::json!({ "coverId": null }));
        }
    });
    let w = Rc::downgrade(app);
    st.on_delete(move || {
        let Some(a) = w.upgrade() else { return };
        let Some(al) = a.pages.album.borrow().clone() else {
            return;
        };
        let lang = a.lang();
        super::dialogs::confirm(
            &a,
            &i18n::tv(lang, "album.deleteConfirm", &[("name", &al.name)]),
            &a.t("album.deleteBody"),
            &a.t("album.delete"),
            true,
            move |a| {
                let id = al.id.clone();
                api::act(
                    a,
                    "DELETE",
                    &format!("/api/albums/{}", enc(&id)),
                    None,
                    |a, _| {
                        a.toast(ToastKind::Success, &a.t("album.deleted"), "");
                        load_albums(a);
                        // replace the album in the history
                        a.history
                            .borrow_mut()
                            .retain(|r| !matches!(r, Route::Album(_)));
                        a.go(Route::Albums, false);
                    },
                );
            },
        );
    });
    let w = Rc::downgrade(app);
    st.on_download(move || {
        let Some(a) = w.upgrade() else { return };
        let Some(al) = a.pages.album.borrow().clone() else {
            return;
        };
        let path = format!(
            "/api/download{}",
            api::qs(&[("albumId", &al.id), ("name", &al.name)])
        );
        super::dialogs::download(&a, &path, &format!("{}.zip", al.name));
    });
    let w = Rc::downgrade(app);
    st.on_slideshow(move || {
        if let Some(a) = w.upgrade() {
            a.toast(ToastKind::Info, &a.t("native.slideshowSoon"), "");
        }
    });
}
