//! Library hub (LibraryPage.tsx): libraries with cover / numbers, albums strip, phones, temp
//! sessions, tools.

use std::rc::Rc;

use pa_core::{BrowseResult, LibrariesResponse, Library, LibraryStats, MediaKind};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::api::{self, enc};
use crate::app::AppInner;
use crate::fmt;
use crate::i18n::{self, Lang};
use crate::router::Route;
use crate::{AlbumCardData, HubState, LibCardData, RowData, ToastKind};

/// MEGA state of a library (badge)
pub(crate) fn remote_state(lib: &Library) -> String {
    for f in &lib.folders {
        if let Some(Some(r)) = &f.remote {
            use pa_core::RemoteFolderState as S;
            return match r.status.state {
                S::Error => "error",
                S::Offline => "offline",
                S::Quota => "warn",
                S::Listing => "busy",
                S::Ok if r.indexing.done < r.indexing.total => "busy",
                _ => "ok",
            }
            .into();
        }
    }
    String::new()
}

fn lib_card(a: &AppInner, l: &Library, stats: Option<&LibraryStats>) -> LibCardData {
    let lang = a.lang();
    let n = stats.map(|s| s.items).unwrap_or(l.media_count);
    LibCardData {
        id: l.id.clone().into(),
        name: l.name.clone().into(),
        color: crate::theme::color(&l.color),
        icon: l.icon.clone().into(),
        cover: Default::default(),
        has_cover: false,
        items: i18n::tv(
            lang,
            "lib2.stats.items",
            &[("n", &i18n::format_count(lang, n))],
        )
        .into(),
        size: stats
            .filter(|s| s.bytes > 0)
            .map(|s| fmt::format_bytes(s.bytes))
            .unwrap_or_default()
            .into(),
        range: stats
            .and_then(|s| fmt::range_label(s.from, s.to, lang))
            .unwrap_or_default()
            .into(),
        remote: remote_state(l).into(),
    }
}

/// album card data
pub(crate) fn album_card(lang: Lang, al: &pa_core::Album) -> AlbumCardData {
    let mut sub = i18n::tv(
        lang,
        "album.items",
        &[("n", &i18n::format_count(lang, al.count))],
    );
    if let Some(r) = fmt::range_label(al.from, al.to, lang) {
        sub.push_str(" · ");
        sub.push_str(&r);
    }
    AlbumCardData {
        id: al.id.clone().into(),
        name: al.name.clone().into(),
        cover: Default::default(),
        has_cover: false,
        sub: sub.into(),
        count: al.count as i32,
    }
}

/// the cover id of a library: picked, else the newest picture
pub(crate) fn lib_cover(a: &AppInner, l: &Library) -> Option<String> {
    match &l.cover_id {
        Some(Some(id)) => Some(id.clone()),
        _ => a.pages.auto_covers.borrow().get(&l.id).cloned().flatten(),
    }
}

/// set a card's cover image (row by id)
fn set_lib_cover(a: &AppInner, id: &str, img: slint::Image) {
    let hs = a.window.global::<HubState>();
    let m = hs.get_libraries();
    for i in 0..m.row_count() {
        if let Some(mut r) = m.row_data(i) {
            if r.id == id {
                r.cover = img;
                r.has_cover = true;
                m.set_row_data(i, r);
                return;
            }
        }
    }
}

fn load_cover(a: &Rc<AppInner>, lib_id: &str, cover: &str) {
    let w = Rc::downgrade(a);
    let id = lib_id.to_string();
    super::cover(a, cover, 640, move |img| {
        if let Some(a) = w.upgrade() {
            set_lib_cover(&a, &id, img);
        }
    });
}

/// GET /api/libraries → hub + everything that lists libraries
pub(crate) fn load_libraries(app: &Rc<AppInner>) {
    api::call::<LibrariesResponse>(app, "GET", "/api/libraries", None, |a, r| match r {
        Ok(l) => {
            *a.pages.libs.borrow_mut() = Some(l);
            publish(a);
            super::library::on_libraries(a);
            super::dialogs::on_libraries(a);
            super::settings::on_libraries(a);
        }
        Err(e) => {
            if e.status != 501 {
                log::warn!("libraries: {e}");
            }
            a.window.global::<HubState>().set_loaded(true);
        }
    });
}

/// push the libraries / phones / temp sessions into the hub
pub(crate) fn publish(a: &Rc<AppInner>) {
    let hs = a.window.global::<HubState>();
    let lang = a.lang();
    let libs = a.pages.libs.borrow().clone();
    let Some(l) = libs else { return };
    let stats = a.pages.stats.borrow().clone();
    let cards: Vec<LibCardData> = l
        .libraries
        .iter()
        .map(|x| lib_card(a, x, stats.get(&x.id)))
        .collect();
    hs.set_libraries(ModelRc::from(Rc::new(VecModel::from(cards))));
    let total: i64 = l.libraries.iter().map(|x| x.media_count).sum();
    hs.set_subtitle(if l.libraries.is_empty() {
        "".into()
    } else {
        let items = i18n::tv(
            lang,
            "common.items",
            &[("n", &i18n::format_count(lang, total))],
        );
        i18n::tv(
            lang,
            "lib2.hub.subtitle",
            &[("n", &l.libraries.len().to_string()), ("items", &items)],
        )
        .into()
    });
    let mut devs = vec![];
    for d in &l.devices {
        for f in &d.folders {
            let sub = if d.online {
                format!(
                    "{} · {}",
                    d.name,
                    i18n::tv(
                        lang,
                        "common.items",
                        &[("n", &i18n::format_count(lang, f.media_count))]
                    )
                )
            } else {
                format!("{} · {}", d.name, i18n::t(lang, "devices.offlineTip"))
            };
            devs.push(RowData {
                id: f.id.clone().into(),
                title: f.name.clone().into(),
                sub: sub.into(),
                online: d.online,
            });
        }
    }
    hs.set_devices(ModelRc::from(Rc::new(VecModel::from(devs))));
    let temp: Vec<RowData> = l
        .temp
        .iter()
        .map(|f| RowData {
            id: f.id.clone().into(),
            title: f
                .source_name
                .clone()
                .unwrap_or_else(|| f.name.clone())
                .into(),
            sub: f
                .expires_at
                .map(|e| {
                    i18n::tv(
                        lang,
                        "mobile.library.expires",
                        &[("time", &fmt::duration_until(e, lang))],
                    )
                })
                .unwrap_or_default()
                .into(),
            online: true,
        })
        .collect();
    hs.set_temp(ModelRc::from(Rc::new(VecModel::from(temp))));
    hs.set_loaded(true);
    hs.set_mega(!a.window.global::<crate::Shell>().get_readonly());

    for x in &l.libraries {
        // numbers (size, date range) once per library
        if !stats.contains_key(&x.id) {
            let id = x.id.clone();
            api::call::<LibraryStats>(
                a,
                "GET",
                &format!("/api/libraries/{}/stats", enc(&id)),
                None,
                move |a, r| {
                    if let Ok(s) = r {
                        a.pages.stats.borrow_mut().insert(id.clone(), s.clone());
                        update_stats(a, &id, &s);
                        super::library::on_stats(a, &id);
                    }
                },
            );
        }
        match lib_cover(a, x) {
            Some(c) => load_cover(a, &x.id, &c),
            None if !a.pages.auto_covers.borrow().contains_key(&x.id) => {
                let id = x.id.clone();
                let q = format!(
                    "/api/browse?libraryId={}&recursive=1&limit=8&sort=takenAt&order=desc&kind=all",
                    enc(&id)
                );
                api::call::<BrowseResult>(a, "GET", &q, None, move |a, r| {
                    let c = r.ok().and_then(|b| {
                        b.items
                            .iter()
                            .find(|i| matches!(i.kind, MediaKind::Image | MediaKind::Video))
                            .map(|i| i.id.clone())
                    });
                    a.pages
                        .auto_covers
                        .borrow_mut()
                        .insert(id.clone(), c.clone());
                    if let Some(c) = c {
                        load_cover(a, &id, &c);
                        super::library::on_stats(a, &id);
                    }
                });
            }
            None => {}
        }
    }
}

fn update_stats(a: &AppInner, id: &str, s: &LibraryStats) {
    let lang = a.lang();
    let hs = a.window.global::<HubState>();
    let m = hs.get_libraries();
    for i in 0..m.row_count() {
        if let Some(mut r) = m.row_data(i) {
            if r.id == id {
                r.items = i18n::tv(
                    lang,
                    "lib2.stats.items",
                    &[("n", &i18n::format_count(lang, s.items))],
                )
                .into();
                r.size = if s.bytes > 0 {
                    fmt::format_bytes(s.bytes)
                } else {
                    String::new()
                }
                .into();
                r.range = fmt::range_label(s.from, s.to, lang)
                    .unwrap_or_default()
                    .into();
                m.set_row_data(i, r);
            }
        }
    }
}

/// albums strip of the hub
pub(crate) fn publish_albums(a: &Rc<AppInner>) {
    let hs = a.window.global::<HubState>();
    let list = a.pages.albums.borrow().clone().unwrap_or_default();
    let cards: Vec<AlbumCardData> = list
        .iter()
        .take(12)
        .map(|al| album_card(a.lang(), al))
        .collect();
    let model = Rc::new(VecModel::from(cards));
    hs.set_albums(ModelRc::from(model.clone()));
    hs.set_albums_loaded(true);
    for al in list.iter().take(12) {
        if let Some(c) = &al.cover_id {
            let id = al.id.clone();
            let m = model.clone();
            super::cover(a, c, 640, move |img| set_album_cover(&m, &id, img));
        }
    }
}

/// set the cover of an album card (by id)
pub(crate) fn set_album_cover(m: &VecModel<AlbumCardData>, id: &str, img: slint::Image) {
    for i in 0..m.row_count() {
        if let Some(mut r) = m.row_data(i) {
            if r.id == id {
                r.cover = img;
                r.has_cover = true;
                m.set_row_data(i, r);
                return;
            }
        }
    }
}

pub(crate) fn install(app: &Rc<AppInner>) {
    let hs = app.window.global::<HubState>();
    let w = Rc::downgrade(app);
    hs.on_open_library(move |id| {
        if let Some(a) = w.upgrade() {
            a.go(Route::Lib(id.into()), true);
        }
    });
    let w = Rc::downgrade(app);
    hs.on_edit_library(move |id| {
        if let Some(a) = w.upgrade() {
            super::dialogs::edit_library(&a, Some(id.to_string()));
        }
    });
    let w = Rc::downgrade(app);
    hs.on_add_library(move || {
        if let Some(a) = w.upgrade() {
            super::dialogs::edit_library(&a, None);
        }
    });
    let w = Rc::downgrade(app);
    hs.on_add_mega(move || {
        if let Some(a) = w.upgrade() {
            super::dialogs::open_mega(&a, "", "stream");
        }
    });
    let w = Rc::downgrade(app);
    hs.on_open_album(move |id| {
        if let Some(a) = w.upgrade() {
            a.go(Route::Album(id.into()), true);
        }
    });
    let w = Rc::downgrade(app);
    hs.on_new_album(move || {
        if let Some(a) = w.upgrade() {
            super::dialogs::new_album(&a);
        }
    });
    let w = Rc::downgrade(app);
    hs.on_open_folder(move |id| {
        if let Some(a) = w.upgrade() {
            a.go(Route::Folder(id.into(), String::new()), true);
        }
    });
    let w = Rc::downgrade(app);
    hs.on_close_temp(move |id| {
        if let Some(a) = w.upgrade() {
            api::act(
                &a,
                "DELETE",
                &format!("/api/folders/{}", enc(&id)),
                None,
                |a, _| load_libraries(a),
            );
        }
    });
    let w = Rc::downgrade(app);
    hs.on_open_quick(move |what| {
        if let Some(a) = w.upgrade() {
            match what.as_str() {
                "favorites" => a.go(Route::Favorites, true),
                // no deep links into Explore's sections (recent / places / duplicates) yet
                _ => a.go(Route::Explore, true),
            }
        }
    });
    let w = Rc::downgrade(app);
    hs.on_open_tool(move |tool| {
        if let Some(a) = w.upgrade() {
            match tool.as_str() {
                "fonts" => a.go(Route::Fonts, true),
                "slideshow" => a.go(Route::Slideshow, true),
                "collage" => a.go(Route::Collage, true),
                _ => a.toast(ToastKind::Info, &a.t("native.soon.title"), ""),
            }
        }
    });
}
