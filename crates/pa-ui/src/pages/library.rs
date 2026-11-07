//! Library page + folder pages (GalleryPage.tsx "library" / "folder", LibraryHeader.tsx,
//! FolderCards.tsx, TempBanner, RemoteBanner).

use std::rc::Rc;
use std::time::Duration;

use pa_core::{BrowseResult, FolderRef, Library, MediaKind, RemoteFolderInfo, SubFolder};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

use crate::api::{self, enc};
use crate::app::AppInner;
use crate::fmt;
use crate::i18n::{self, Lang};
use crate::prefs::Prefs;
use crate::router::Route;
use crate::{
    ArchiveData, CrumbData, FolderCardData, GridState, LibState, RemoteBannerData, RootCardData,
    ToastKind,
};

/// the library a route belongs to
pub(crate) fn lib_of(a: &AppInner, route: &Route) -> Option<Library> {
    let libs = a.pages.libs.borrow();
    let l = libs.as_ref()?;
    match route {
        Route::Lib(id) => l.libraries.iter().find(|x| &x.id == id).cloned(),
        Route::Folder(fid, _) => l
            .libraries
            .iter()
            .find(|x| x.folders.iter().any(|f| &f.id == fid))
            .cloned(),
        _ => None,
    }
}

fn view_key(a: &AppInner, route: &Route) -> String {
    match route {
        Route::Lib(id) => format!("lib:{id}"),
        Route::Folder(fid, _) => match lib_of(a, route) {
            Some(l) => format!("dir:{}", l.id),
            None => format!("dir:f{fid}"),
        },
        _ => String::new(),
    }
}

/// "all" | "folders" of a library / folder route (remembered per library; folders default to folders)
pub(crate) fn view_mode(p: &Prefs, route: &Route) -> String {
    let (key, def) = match route {
        Route::Lib(id) => (format!("lib:{id}"), "all"),
        Route::Folder(..) => {
            let key = crate::pages::app()
                .map(|a| view_key(&a, route))
                .unwrap_or_default();
            (key, "folders")
        }
        _ => return "all".into(),
    };
    p.library_view
        .get(&key)
        .cloned()
        .unwrap_or_else(|| def.into())
}

/// switch All | Folders
pub(crate) fn set_view(a: &Rc<AppInner>, v: &str) {
    let route = a.route.borrow().clone();
    if !matches!(route, Route::Lib(_) | Route::Folder(..)) || (v != "all" && v != "folders") {
        return;
    }
    let key = view_key(a, &route);
    a.prefs.borrow_mut().library_view.insert(key, v.into());
    a.save_prefs();
    a.window.global::<LibState>().set_view(v.into());
    a.load_page_query();
    refresh_meta(a);
}

fn folder_of(a: &AppInner, id: &str) -> Option<FolderRef> {
    let libs = a.pages.libs.borrow();
    let l = libs.as_ref()?;
    l.libraries
        .iter()
        .flat_map(|x| x.folders.iter())
        .chain(l.temp.iter())
        .chain(l.devices.iter().flat_map(|d| d.folders.iter()))
        .find(|f| f.id == id)
        .cloned()
}

/// MEGA status line (remote/status.ts)
pub(crate) fn remote_status_text(a: &AppInner, r: &RemoteFolderInfo) -> String {
    use pa_core::RemoteFolderState as S;
    let lang = a.lang();
    let st = &r.status;
    match st.state {
        S::Listing => a.t("mega.state.listing"),
        S::Offline => a.t("mega.state.offline"),
        S::Quota => match st.retry_at.filter(|t| *t > fmt::now_ms()) {
            Some(t) => i18n::tv(
                lang,
                "mega.state.quota",
                &[("time", &fmt::duration_until(t, lang))],
            ),
            None => a.t("mega.state.quotaSoon"),
        },
        S::Error => {
            use pa_core::RemoteErrorCode as C;
            let status = match st.code {
                Some(C::NotFound) | Some(C::Blocked) => 404,
                Some(C::Key) => 400,
                Some(C::Unavailable) => 503,
                Some(C::Network) => 0,
                _ => 502,
            };
            let err = mega_error(a, status, st.error.as_deref().unwrap_or(""));
            i18n::tv(lang, "mega.state.error", &[("error", &err)])
        }
        S::Ok => {
            if r.indexing.total > 0 && r.indexing.done < r.indexing.total {
                i18n::tv(
                    lang,
                    "mega.state.indexing",
                    &[
                        ("done", &r.indexing.done.to_string()),
                        ("total", &r.indexing.total.to_string()),
                    ],
                )
            } else if let Some(t) = st.listed_at {
                i18n::tv(
                    lang,
                    "mega.state.lastListed",
                    &[("time", &fmt::relative_time(t, lang))],
                )
            } else {
                a.t("mega.state.ok")
            }
        }
    }
}

/// `megaErrorKey` → text
pub(crate) fn mega_error(a: &AppInner, status: u16, message: &str) -> String {
    let lang = a.lang();
    match status {
        429 => {
            let lower = message.to_lowercase();
            let min = lower.find("retry in about ").and_then(|i| {
                lower[i + 15..]
                    .split_whitespace()
                    .next()
                    .and_then(|n| n.parse::<i64>().ok())
            });
            match min {
                Some(m) => i18n::tv(lang, "remote.err.quota", &[("min", &m.to_string())]),
                None => a.t("remote.err.quotaNoTime"),
            }
        }
        404 => a.t("remote.err.notFound"),
        400 if message.to_lowercase().contains("key") => a.t("remote.err.badKey"),
        400 => a.t("remote.err.notMega"),
        503 => a.t("remote.err.unavailable"),
        502 if message.to_lowercase().contains("reach") => a.t("remote.err.network"),
        0 => a.t("remote.err.network"),
        _ => i18n::tv(lang, "remote.err.other", &[("error", message)]),
    }
}

fn banner_of(a: &AppInner, f: &FolderRef) -> Option<RemoteBannerData> {
    let r = f.remote.clone().flatten()?;
    use pa_core::RemoteFolderState as S;
    let state = match r.status.state {
        S::Listing => "listing",
        S::Offline => "offline",
        S::Quota => "warn",
        S::Error => "error",
        S::Ok if r.indexing.total > 0 && r.indexing.done < r.indexing.total => "indexing",
        S::Ok => "ok",
    };
    let logged_in = a.pages.settings.mega_logged_in.get();
    Some(RemoteBannerData {
        id: f.id.clone().into(),
        name: f.name.clone().into(),
        status: remote_status_text(a, &r).into(),
        state: state.into(),
        progress: if r.indexing.total > 0 {
            r.indexing.done as f32 / r.indexing.total as f32
        } else {
            1.0
        },
        login_link: r.status.state == S::Quota && !logged_in,
        refreshing: false,
    })
}

/// sort sub folders (LibraryHeader.tsx `sortFolders`)
pub(crate) fn sort_folders(list: &mut [SubFolder], sort: &str) {
    let by_name = |a: &SubFolder, b: &SubFolder| natural_cmp(&a.name, &b.name);
    match sort {
        "date" => list.sort_by(|a, b| {
            let (x, y) = (
                a.latest.flatten().unwrap_or(i64::MIN),
                b.latest.flatten().unwrap_or(i64::MIN),
            );
            y.cmp(&x).then_with(|| by_name(a, b))
        }),
        "count" => list.sort_by(|a, b| {
            b.media_count
                .cmp(&a.media_count)
                .then_with(|| by_name(a, b))
        }),
        _ => list.sort_by(by_name),
    }
}

/// `localeCompare(…, { numeric: true, sensitivity: 'base' })` (approximation)
pub(crate) fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (fa, fb) = (fmt::fold(a), fmt::fold(b));
    let (mut x, mut y) = (fa.chars().peekable(), fb.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(c), Some(d)) if c.is_ascii_digit() && d.is_ascii_digit() => {
                let mut n1 = String::new();
                while let Some(c) = x.peek().copied().filter(char::is_ascii_digit) {
                    n1.push(c);
                    x.next();
                }
                let mut n2 = String::new();
                while let Some(c) = y.peek().copied().filter(char::is_ascii_digit) {
                    n2.push(c);
                    y.next();
                }
                let o = n1
                    .trim_start_matches('0')
                    .len()
                    .cmp(&n2.trim_start_matches('0').len())
                    .then_with(|| n1.trim_start_matches('0').cmp(n2.trim_start_matches('0')));
                if o.is_ne() {
                    return o;
                }
            }
            (Some(c), Some(d)) => {
                if c != d {
                    return c.cmp(&d);
                }
                x.next();
                y.next();
            }
        }
    }
}

/// entering a library / folder route
pub(crate) fn enter(a: &Rc<AppInner>) {
    let route = a.route.borrow().clone();
    let st = a.window.global::<LibState>();
    st.set_mode(
        if matches!(route, Route::Lib(_)) {
            "library"
        } else {
            "folder"
        }
        .into(),
    );
    st.set_view(view_mode(&a.prefs.borrow(), &route).into());
    st.set_folders(ModelRc::default());
    st.set_roots(ModelRc::default());
    st.set_archives(ModelRc::default());
    st.set_crumbs(ModelRc::default());
    st.set_has_cover(false);
    st.set_loading(true);
    st.set_empty(false);
    st.set_title("".into());
    st.set_folder_sort(a.prefs.borrow().folder_sort.clone().into());
    *a.pages.meta.borrow_mut() = None;
    publish_header(a);
    refresh_meta(a);
    // folder avatar: the newest picture inside
    if let Route::Folder(fid, dir) = &route {
        let q = format!(
            "/api/browse{}",
            api::qs(&[
                ("folderId", fid),
                ("dir", dir),
                ("recursive", "1"),
                ("limit", "8"),
                ("sort", "takenAt"),
                ("order", "desc"),
                ("kind", "all")
            ])
        );
        let r2 = route.clone();
        api::call::<BrowseResult>(a, "GET", &q, None, move |a, r| {
            if *a.route.borrow() != r2 {
                return;
            }
            let c = r.ok().and_then(|b| {
                b.items
                    .iter()
                    .find(|i| matches!(i.kind, MediaKind::Image | MediaKind::Video))
                    .map(|i| i.id.clone())
            });
            if let Some(c) = c {
                let w = Rc::downgrade(a);
                super::cover(a, &c, 320, move |img| {
                    if let Some(a) = w.upgrade() {
                        if *a.route.borrow() == r2 {
                            let st = a.window.global::<LibState>();
                            st.set_cover(img);
                            st.set_has_cover(true);
                        }
                    }
                });
            }
        });
    }
    poll_stats(a);
}

/// libraries reloaded (names, covers, banners)
pub(crate) fn on_libraries(a: &Rc<AppInner>) {
    if matches!(&*a.route.borrow(), Route::Lib(_) | Route::Folder(..)) {
        publish_header(a);
    }
}

/// stats / auto cover of a library arrived
pub(crate) fn on_stats(a: &Rc<AppInner>, id: &str) {
    if matches!(&*a.route.borrow(), Route::Lib(l) if l == id) {
        publish_header(a);
    }
}

fn stats_lines(a: &AppInner, lib: &Library) {
    let st = a.window.global::<LibState>();
    let lang = a.lang();
    let fc = |n: i64| i18n::format_count(lang, n);
    let Some(s) = a.pages.stats.borrow().get(&lib.id).cloned() else {
        st.set_stats_main(
            i18n::tv(lang, "lib2.stats.items", &[("n", &fc(lib.media_count))]).into(),
        );
        for f in [
            LibState::set_stats_size,
            LibState::set_stats_range,
            LibState::set_stats_folders,
            LibState::set_stats_scan,
            LibState::set_stats_indexing,
        ] {
            f(&st, "".into());
        }
        return;
    };
    let mut main = i18n::tv(lang, "lib2.stats.items", &[("n", &fc(s.items))]);
    for (n, k) in [
        (s.photos, "lib2.stats.photos"),
        (s.videos, "lib2.stats.videos"),
        (s.audio, "lib2.stats.audio"),
    ] {
        if n > 0 {
            main.push_str("  ·  ");
            main.push_str(&i18n::tv(lang, k, &[("n", &fc(n))]));
        }
    }
    st.set_stats_main(main.into());
    st.set_stats_size(
        if s.bytes > 0 {
            i18n::tv(
                lang,
                "lib2.stats.size",
                &[("size", &fmt::format_bytes(s.bytes))],
            )
        } else {
            String::new()
        }
        .into(),
    );
    st.set_stats_range(
        fmt::range_label(s.from, s.to, lang)
            .unwrap_or_default()
            .into(),
    );
    st.set_stats_folders(
        if s.folders > 0 {
            i18n::tv(lang, "lib2.stats.folders", &[("n", &s.folders.to_string())])
        } else {
            String::new()
        }
        .into(),
    );
    st.set_stats_scanning(s.scanning);
    st.set_stats_scan(
        if s.scanning {
            a.t("lib2.stats.scanning")
        } else if let Some(t) = s.scanned_at {
            i18n::tv(
                lang,
                "lib2.stats.scanned",
                &[("time", &fmt::relative_time(t, lang))],
            )
        } else {
            a.t("lib2.stats.never")
        }
        .into(),
    );
    st.set_stats_indexing(
        if !s.scanning && s.pending > 0 {
            i18n::tv(lang, "lib2.stats.indexing", &[("n", &fc(s.pending))])
        } else {
            String::new()
        }
        .into(),
    );
}

/// header data from the libraries cache (+ stats)
fn publish_header(a: &Rc<AppInner>) {
    let route = a.route.borrow().clone();
    let st = a.window.global::<LibState>();
    let lang = a.lang();
    let writable = !a.window.global::<crate::Shell>().get_readonly();
    st.set_writable(writable);
    let lib = lib_of(a, &route);
    let libs = a.pages.libs.borrow().clone();
    match &route {
        Route::Lib(_) => {
            let Some(l) = lib else { return };
            st.set_name(l.name.clone().into());
            st.set_color(crate::theme::color(&l.color));
            st.set_icon(l.icon.clone().into());
            st.set_cover_picked(matches!(l.cover_id, Some(Some(_))));
            st.set_exclusions(
                l.options
                    .as_ref()
                    .map(|o| o.exclude.len() as i32)
                    .unwrap_or(0),
            );
            a.window
                .global::<GridState>()
                .set_title(l.name.clone().into());
            stats_lines(a, &l);
            let remotes: Vec<RemoteBannerData> =
                l.folders.iter().filter_map(|f| banner_of(a, f)).collect();
            st.set_remotes(ModelRc::from(Rc::new(VecModel::from(remotes))));
            st.set_temp(false);
            st.set_device(false);
            if let Some(c) = super::hub::lib_cover(a, &l) {
                let w = Rc::downgrade(a);
                let r2 = route.clone();
                // the card's 640 thumb first (cached → instant), then the sharp 1600 one
                let w2 = w.clone();
                let c2 = c.clone();
                super::cover(a, &c, 640, move |img| {
                    if let Some(a) = w.upgrade() {
                        if *a.route.borrow() == r2 {
                            let st = a.window.global::<LibState>();
                            if !st.get_has_cover() {
                                st.set_cover(img);
                                st.set_has_cover(true);
                            }
                            let r3 = r2.clone();
                            let w3 = w2.clone();
                            super::cover(&a, &c2, 1600, move |img| {
                                if let Some(a) = w3.upgrade() {
                                    if *a.route.borrow() == r3 {
                                        let st = a.window.global::<LibState>();
                                        st.set_cover(img);
                                        st.set_has_cover(true);
                                    }
                                }
                            });
                        }
                    }
                });
            }
        }
        Route::Folder(fid, _) => {
            let f = folder_of(a, fid);
            let temp = libs
                .as_ref()
                .and_then(|l| l.temp.iter().find(|t| &t.id == fid).cloned());
            st.set_temp(temp.is_some());
            if let Some(t) = &temp {
                st.set_temp_title(
                    i18n::tv(
                        lang,
                        "temp.banner",
                        &[("name", t.source_name.as_deref().unwrap_or(&t.name))],
                    )
                    .into(),
                );
                st.set_temp_expires(
                    t.expires_at
                        .map(|e| {
                            i18n::tv(
                                lang,
                                "temp.expires",
                                &[("time", &fmt::duration_until(e, lang))],
                            )
                        })
                        .unwrap_or_default()
                        .into(),
                );
            }
            let remotes: Vec<RemoteBannerData> = f.iter().filter_map(|f| banner_of(a, f)).collect();
            st.set_remotes(ModelRc::from(Rc::new(VecModel::from(remotes))));
            match f.as_ref().and_then(|f| f.device.clone()) {
                Some(d) => {
                    st.set_device(true);
                    st.set_device_online(d.online);
                    st.set_device_name(d.name.into());
                }
                None => st.set_device(false),
            }
            st.set_writable(
                writable
                    && f.as_ref()
                        .is_none_or(|f| !matches!(f.remote, Some(Some(_)))),
            );
        }
        _ => {}
    }
    let _ = Lang::Tr;
}

/// GET /api/browse page 0 (crumbs, sub folders, roots, archives, target) of the current route
pub(crate) fn refresh_meta(a: &Rc<AppInner>) {
    let route = a.route.borrow().clone();
    let view = view_mode(&a.prefs.borrow(), &route);
    let q = match &route {
        Route::Lib(id) => api::qs(&[
            ("libraryId", id),
            ("recursive", if view == "all" { "1" } else { "" }),
            ("limit", "1"),
        ]),
        Route::Folder(fid, dir) => api::qs(&[
            ("folderId", fid),
            ("dir", dir),
            ("recursive", if view == "all" { "1" } else { "" }),
            ("limit", "1"),
        ]),
        _ => return,
    };
    let r2 = route.clone();
    api::call::<BrowseResult>(a, "GET", &format!("/api/browse{q}"), None, move |a, r| {
        if *a.route.borrow() != r2 {
            return;
        }
        let st = a.window.global::<LibState>();
        st.set_loading(false);
        match r {
            Ok(b) => {
                *a.pages.meta.borrow_mut() = Some(b);
                publish_meta(a);
            }
            Err(e) => {
                st.set_empty(true);
                a.window.global::<GridState>().set_error(e.message.into());
            }
        }
    });
}

fn publish_meta(a: &Rc<AppInner>) {
    let Some(m) = a.pages.meta.borrow().clone() else {
        return;
    };
    let route = a.route.borrow().clone();
    let st = a.window.global::<LibState>();
    let lang = a.lang();
    let browse = view_mode(&a.prefs.borrow(), &route) == "folders";
    // crumbs: parents only (the last one is the title)
    let n = m.crumbs.len();
    let crumbs: Vec<CrumbData> = m
        .crumbs
        .iter()
        .take(n.saturating_sub(1))
        .enumerate()
        .map(|(i, c)| CrumbData {
            label: c.label.clone().into(),
            index: i as i32,
        })
        .collect();
    st.set_crumbs(ModelRc::from(Rc::new(VecModel::from(crumbs))));
    if let Route::Folder(..) = route {
        let title = m.crumbs.last().map(|c| c.label.clone()).unwrap_or_default();
        st.set_title(title.clone().into());
        a.window.global::<GridState>().set_title(title.into());
        st.set_sub(a.window.global::<GridState>().get_subtitle());
    }
    let mut folders = if browse { m.folders.clone() } else { vec![] };
    sort_folders(&mut folders, &a.prefs.borrow().folder_sort);
    let fmodel = Rc::new(VecModel::from(
        folders
            .iter()
            .map(|f| FolderCardData {
                dir: f.dir.clone().into(),
                name: f.name.clone().into(),
                count: i18n::format_count(lang, f.media_count).into(),
                n: f.cover_ids.len().min(3) as i32,
                c1: Default::default(),
                c2: Default::default(),
                c3: Default::default(),
            })
            .collect::<Vec<_>>(),
    ));
    st.set_folders(ModelRc::from(fmodel.clone()));
    for (i, f) in folders.iter().enumerate() {
        for (k, id) in f.cover_ids.iter().take(3).enumerate() {
            let m2 = fmodel.clone();
            let dir = f.dir.clone();
            super::cover(a, id, 320, move |img| {
                if let Some(mut r) = m2.row_data(i).filter(|r| r.dir == dir) {
                    match k {
                        0 => r.c1 = img,
                        1 => r.c2 = img,
                        _ => r.c3 = img,
                    }
                    m2.set_row_data(i, r);
                }
            });
        }
    }
    let roots: Vec<RootCardData> = if browse {
        m.roots
            .iter()
            .map(|f| RootCardData {
                id: f.id.clone().into(),
                name: f.name.clone().into(),
                path: match &f.remote {
                    Some(Some(r)) => r.label.clone(),
                    _ => f.path.clone(),
                }
                .into(),
                count: i18n::format_count(lang, f.media_count).into(),
                remote: match &f.remote {
                    Some(Some(_)) => "ok".into(),
                    _ => "".into(),
                },
            })
            .collect()
    } else {
        vec![]
    };
    st.set_roots(ModelRc::from(Rc::new(VecModel::from(roots))));
    let archives: Vec<ArchiveData> = if browse {
        m.archives
            .iter()
            .map(|x| ArchiveData {
                id: x.id.clone().into(),
                name: x.name.clone().into(),
                size: fmt::format_bytes(x.size).into(),
                busy: "".into(),
            })
            .collect()
    } else {
        vec![]
    };
    st.set_archives(ModelRc::from(Rc::new(VecModel::from(archives))));
    let empty = m.total == 0
        && st.get_folders().row_count() == 0
        && st.get_roots().row_count() == 0
        && st.get_archives().row_count() == 0;
    st.set_empty(empty);
    st.set_empty_hint(
        if m.target.is_some() && st.get_writable() {
            a.t("gallery.emptyHint")
        } else {
            String::new()
        }
        .into(),
    );
    st.set_can_slideshow(m.total > 0);
    st.set_base_folder(folder_base(a).unwrap_or_default().into());
    super::chrome(a);
}

fn poll_stats(a: &Rc<AppInner>) {
    let Route::Lib(id) = a.route.borrow().clone() else {
        return;
    };
    let epoch = a.pages.epoch.get();
    api::call::<pa_core::LibraryStats>(
        a,
        "GET",
        &format!("/api/libraries/{}/stats", enc(&id)),
        None,
        move |a, r| {
            if a.pages.epoch.get() != epoch {
                return;
            }
            if let Ok(s) = r {
                let busy = s.scanning || s.pending > 0;
                a.pages.stats.borrow_mut().insert(id.clone(), s);
                publish_header(a);
                if busy {
                    let w = Rc::downgrade(a);
                    slint::Timer::single_shot(Duration::from_secs(4), move || {
                        if let Some(a) = w.upgrade() {
                            if a.pages.epoch.get() == epoch {
                                poll_stats(&a);
                            }
                        }
                    });
                }
            }
        },
    );
}

/// folder id of the sub folder cards (folder page: its folder; library: its single folder / target)
fn folder_base(a: &AppInner) -> Option<String> {
    let route = a.route.borrow().clone();
    match &route {
        Route::Folder(fid, _) => Some(fid.clone()),
        Route::Lib(_) => {
            let lib = lib_of(a, &route)?;
            if lib.folders.len() == 1 {
                return Some(lib.folders[0].id.clone());
            }
            a.pages
                .meta
                .borrow()
                .as_ref()?
                .target
                .as_ref()
                .map(|t| t.folder_id.clone())
        }
        _ => None,
    }
}

fn cur_dir(a: &AppInner) -> String {
    match &*a.route.borrow() {
        Route::Folder(_, d) => d.clone(),
        _ => String::new(),
    }
}

fn cur_title(a: &AppInner) -> String {
    let route = a.route.borrow().clone();
    match &route {
        Route::Lib(_) => lib_of(a, &route).map(|l| l.name).unwrap_or_default(),
        _ => a.window.global::<LibState>().get_title().to_string(),
    }
}

pub(crate) fn install(app: &Rc<AppInner>) {
    let st = app.window.global::<LibState>();
    macro_rules! cb {
        ($on:ident, |$a:ident $(, $arg:ident)*| $body:block) => {{
            let w = Rc::downgrade(app);
            st.$on(move |$($arg),*| {
                if let Some($a) = w.upgrade() $body
            });
        }};
    }
    cb!(on_set_view, |a, v| { set_view(&a, &v) });
    cb!(on_open_crumb, |a, i| {
        let c = a
            .pages
            .meta
            .borrow()
            .as_ref()
            .and_then(|m| m.crumbs.get(i as usize).cloned());
        if let Some(c) = c {
            let r = match (&c.folder_id, &c.library_id) {
                (Some(f), _) => Route::Folder(f.clone(), c.dir.clone().unwrap_or_default()),
                (None, Some(l)) => Route::Lib(l.clone()),
                _ => Route::Library,
            };
            a.go(r, true);
        }
    });
    cb!(on_open_dir, |a, dir| {
        if let Some(f) = folder_base(&a) {
            a.go(Route::Folder(f, dir.to_string()), true);
        }
    });
    cb!(on_open_root, |a, id| {
        a.go(Route::Folder(id.to_string(), String::new()), true)
    });
    cb!(on_folder_slideshow, |a, _d| {
        a.toast(ToastKind::Info, &a.t("native.slideshowSoon"), "")
    });
    cb!(on_slideshow, |a| {
        a.toast(ToastKind::Info, &a.t("native.slideshowSoon"), "")
    });
    cb!(on_peek, |a, id| {
        set_archive_busy(&a, &id, "peek");
        let id2 = id.to_string();
        api::call::<serde_json::Value>(
            &a,
            "POST",
            "/api/zip/peek",
            Some(serde_json::json!({ "id": id2 })),
            move |a, r| {
                set_archive_busy(a, &id2, "");
                match r {
                    Ok(v) => {
                        super::hub::load_libraries(a);
                        if let Some(f) = v["tempFolder"]["id"].as_str() {
                            a.go(Route::Folder(f.into(), String::new()), true);
                        }
                    }
                    Err(e) => a.error_toast(&e.message),
                }
            },
        );
    });
    cb!(on_extract, |a, id| {
        set_archive_busy(&a, &id, "extract");
        let id2 = id.to_string();
        api::call::<pa_core::DirTarget>(
            &a,
            "POST",
            "/api/zip/extract",
            Some(serde_json::json!({ "id": id2 })),
            move |a, r| {
                set_archive_busy(a, &id2, "");
                match r {
                    Ok(t) => {
                        super::dialogs::toast_open(
                            a,
                            &a.t("upload.extracted"),
                            &t.dir,
                            Route::Folder(t.folder_id.clone(), t.dir.clone()),
                        );
                        refresh_meta(a);
                    }
                    Err(e) => a.error_toast(&e.message),
                }
            },
        );
    });
    cb!(on_rescan, |a| {
        let Route::Lib(id) = a.route.borrow().clone() else {
            return;
        };
        a.window.global::<LibState>().set_rescanning(true);
        api::act(
            &a,
            "POST",
            &format!("/api/libraries/{}/rescan", enc(&id)),
            Some(serde_json::json!({})),
            |a, _| {
                a.toast(ToastKind::Info, &a.t("lib2.rescanStarted"), "");
                poll_stats(a);
            },
        );
        let w = Rc::downgrade(&a);
        slint::Timer::single_shot(Duration::from_millis(1200), move || {
            if let Some(a) = w.upgrade() {
                a.window.global::<LibState>().set_rescanning(false);
            }
        });
    });
    cb!(on_edit, |a| {
        let r = a.route.borrow().clone();
        if let Some(l) = lib_of(&a, &r) {
            super::dialogs::edit_library(&a, Some(l.id));
        }
    });
    cb!(on_exclusions_dialog, |a| {
        let r = a.route.borrow().clone();
        if let Some(l) = lib_of(&a, &r) {
            super::dialogs::exclusions(&a, &l.id);
        }
    });
    cb!(on_reset_cover, |a| {
        let r = a.route.borrow().clone();
        if let Some(l) = lib_of(&a, &r) {
            a.window.global::<LibState>().set_has_cover(false);
            a.pages.auto_covers.borrow_mut().remove(&l.id);
            api::act(
                &a,
                "PATCH",
                &format!("/api/libraries/{}", enc(&l.id)),
                Some(serde_json::json!({ "coverId": null })),
                |a, _| super::hub::load_libraries(a),
            );
        }
    });
    cb!(on_set_folder_sort, |a, s| {
        a.prefs.borrow_mut().folder_sort = s.to_string();
        a.save_prefs();
        a.window.global::<LibState>().set_folder_sort(s);
        publish_meta(&a);
    });
    cb!(on_temp_extend, |a| {
        let Route::Folder(fid, _) = a.route.borrow().clone() else {
            return;
        };
        api::act(
            &a,
            "POST",
            &format!("/api/temp/{}/extend", enc(&fid)),
            Some(serde_json::json!({ "hours": 24 })),
            |a, _| super::hub::load_libraries(a),
        );
    });
    cb!(on_temp_close, |a| {
        let Route::Folder(fid, _) = a.route.borrow().clone() else {
            return;
        };
        api::act(
            &a,
            "DELETE",
            &format!("/api/folders/{}", enc(&fid)),
            None,
            |a, _| {
                super::hub::load_libraries(a);
                a.go(Route::Photos, false);
            },
        );
    });
    cb!(on_temp_keep, |a| {
        let Route::Folder(fid, _) = a.route.borrow().clone() else {
            return;
        };
        let title = a.t("temp.keepTitle");
        let label = a.t("temp.keep");
        super::dialogs::pick_folder(&a, &title, &label, move |a, target| {
            api::call::<pa_core::DirTarget>(
                a,
                "POST",
                &format!("/api/temp/{}/keep", enc(&fid)),
                Some(serde_json::json!({ "folderId": target.folder_id, "dir": target.dir })),
                |a, r| match r {
                    Ok(t) => {
                        super::hub::load_libraries(a);
                        a.toast(ToastKind::Success, &a.t("temp.kept"), "");
                        a.go(Route::Folder(t.folder_id, t.dir), false);
                    }
                    Err(e) => a.error_toast(&e.message),
                },
            );
        });
    });
    cb!(on_remote_refresh, |a, id| {
        set_refreshing(&a, &id, true);
        let id2 = id.to_string();
        api::call::<serde_json::Value>(
            &a,
            "POST",
            &format!("/api/remote/folders/{}/refresh", enc(&id)),
            Some(serde_json::json!({})),
            move |a, r| {
                set_refreshing(a, &id2, false);
                match r {
                    Ok(_) => a.toast(ToastKind::Success, &a.t("mega.refreshed"), ""),
                    Err(e) => {
                        let m = mega_error(a, e.status, &e.message);
                        a.toast(ToastKind::Error, &a.t("common.error"), &m);
                    }
                }
                super::hub::load_libraries(a);
                a.grid.0.borrow_mut().reload();
            },
        );
    });
    cb!(on_remote_copy, |a, id| {
        let dir = cur_dir(&a);
        let label = if dir.is_empty() {
            folder_of(&a, &id).map(|f| f.name).unwrap_or_default()
        } else {
            dir.rsplit('/').next().unwrap_or("").to_string()
        };
        super::dialogs::copy_to_pc(
            &a,
            vec![pa_core::FileRef {
                folder_id: id.to_string(),
                rel: dir,
            }],
            &label,
            true,
        );
    });
    cb!(on_remote_link, |a, id| {
        api::call::<serde_json::Value>(
            &a,
            "GET",
            &format!("/api/remote/folders/{}/link", enc(&id)),
            None,
            |a, r| match r {
                Ok(v) => {
                    a.host.copy_text(v["url"].as_str().unwrap_or(""));
                    a.toast(ToastKind::Success, &a.t("mega.linkCopied"), "");
                }
                Err(e) => a.error_toast(&e.message),
            },
        );
    });
    cb!(on_remote_remove, |a, id| {
        let name = folder_of(&a, &id).map(|f| f.name).unwrap_or_default();
        let id = id.to_string();
        super::dialogs::confirm(
            &a,
            &i18n::tv(a.lang(), "mega.removeConfirm", &[("name", &name)]),
            &a.t("mega.removeBody"),
            &a.t("mega.remove"),
            true,
            move |a| {
                api::act(
                    a,
                    "DELETE",
                    &format!("/api/folders/{}", enc(&id)),
                    None,
                    |a, _| {
                        super::hub::load_libraries(a);
                        a.go(Route::Library, false);
                    },
                );
            },
        );
    });
    cb!(on_remote_login, |a| {
        a.window
            .global::<crate::Shell>()
            .set_settings_section("network".into());
        a.go(Route::Settings, true);
    });
    cb!(on_download_folder, |a| {
        let (Some(f), dir, name) = (folder_base(&a), cur_dir(&a), cur_title(&a)) else {
            return;
        };
        let p = format!(
            "/api/download{}",
            api::qs(&[("folderId", &f), ("dir", &dir), ("name", &name)])
        );
        super::dialogs::download(&a, &p, &format!("{name}.zip"));
    });
    cb!(on_copy_to_pc, |a| {
        let Route::Folder(fid, dir) = a.route.borrow().clone() else {
            return;
        };
        let name = cur_title(&a);
        super::dialogs::copy_to_pc(
            &a,
            vec![pa_core::FileRef {
                folder_id: fid,
                rel: dir,
            }],
            &name,
            false,
        );
    });
    cb!(on_folder_action, |a, action, dir| {
        let Some(fid) = folder_base(&a) else { return };
        let name = dir.rsplit('/').next().unwrap_or("").to_string();
        let r = pa_core::FileRef {
            folder_id: fid.clone(),
            rel: dir.to_string(),
        };
        match action.as_str() {
            "slideshow" => a.toast(ToastKind::Info, &a.t("native.slideshowSoon"), ""),
            "download" => {
                let p = format!(
                    "/api/download{}",
                    api::qs(&[("folderId", &fid), ("dir", &dir), ("name", &name)])
                );
                super::dialogs::download(&a, &p, &format!("{name}.zip"));
            }
            "copy-to-pc" => super::dialogs::copy_to_pc(&a, vec![r], &name, false),
            "rename" => super::dialogs::rename_ref(&a, r, &name),
            "move" => super::dialogs::move_refs(&a, vec![r]),
            "delete" => super::dialogs::delete_refs(&a, vec![r], &name),
            _ => {}
        }
    });
    cb!(on_upload, |a| { super::dialogs::upload_pick(&a, false) });
    cb!(on_new_folder, |a| { super::dialogs::new_folder(&a) });
}

/// the current upload / drop target (DirTarget of the open folder / library)
pub(crate) fn target(a: &AppInner) -> Option<pa_core::DirTarget> {
    if !matches!(&*a.route.borrow(), Route::Lib(_) | Route::Folder(..)) {
        return None;
    }
    a.pages.meta.borrow().as_ref()?.target.clone()
}

fn set_archive_busy(a: &AppInner, id: &str, busy: &str) {
    let m = a.window.global::<LibState>().get_archives();
    for i in 0..m.row_count() {
        if let Some(mut r) = m.row_data(i) {
            if r.id == id {
                r.busy = busy.into();
                m.set_row_data(i, r);
            }
        }
    }
}

fn set_refreshing(a: &AppInner, id: &str, on: bool) {
    let m = a.window.global::<LibState>().get_remotes();
    for i in 0..m.row_count() {
        if let Some(mut r) = m.row_data(i) {
            if r.id == id {
                r.refreshing = on;
                m.set_row_data(i, r);
            }
        }
    }
}
