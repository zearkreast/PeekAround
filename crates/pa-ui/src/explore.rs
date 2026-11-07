//! Explore: the home page (features/explore/Home.tsx + insights.tsx), the result pages
//! (ResultPage.tsx: route → browse query + header, drawn above the shared grid), /explore/places,
//! /explore/duplicates and the routing glue. The memory player is `explore_player.rs`, the map
//! `map.rs`. Models go into the globals of `ui/pages/explore/state.slint`.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use pa_core::{
    BrowseQuery, CalendarYear, DuplicatesResponse, ExploreResult, GpsFilter, MediaItem, Memory,
    SortDir, SortKey,
};
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};

use crate::app::AppInner;
use crate::backend::BackendEvent;
use crate::explore_labels as lb;
use crate::i18n::Lang;
use crate::router::Route;
use crate::{
    ECalCell, ECalMonth, ECard, EChip, EDot, EDupGroup, EDupItem, EDupState, EPlaceGroup,
    EPlacesState, EResultState, EYearBtn, ExploreState, Page, Shell,
};

const DUP_PAGE: usize = 120;

thread_local! {
    static EX: RefCell<Option<Rc<Explore>>> = const { RefCell::new(None) };
}

pub(crate) fn get() -> Option<Rc<Explore>> {
    EX.with(|e| e.borrow().clone())
}

/// What a result route shows (ResultPage `useResolved`).
#[derive(Debug, Clone, Default)]
pub(crate) struct Resolved {
    pub query: Option<BrowseQuery>,
    pub title: String,
    pub subtitle: String,
    pub icon: &'static str,
    pub flag: Option<(i32, i32)>,
    pub missing: bool,
    pub memory: Option<Memory>,
    pub place_key: Option<String>,
    pub regions: Vec<String>,
    pub cities: Vec<String>,
    pub hint: String,
    pub no_groups: bool,
}

#[derive(Default)]
struct Dups {
    data: Option<DuplicatesResponse>,
    /// groups shown (oldest file first), as item lists
    groups: Vec<(pa_core::DuplicateGroup, Vec<MediaItem>)>,
    limit: usize,
    loading: bool,
    model: Option<Rc<VecModel<EDupGroup>>>,
}

pub(crate) struct Explore {
    app: Weak<AppInner>,
    pub(crate) data: RefCell<Option<ExploreResult>>,
    loading: Cell<bool>,
    loaded_once: Cell<bool>,
    error: RefCell<String>,
    refresh_timer: slint::Timer,
    poll_timer: slint::Timer,
    cal_year: Cell<i64>,
    cal: RefCell<Option<CalendarYear>>,
    dups: RefCell<Dups>,
    /// open the viewer on this item once the grid has it (`?view=<id>` of the web UI)
    pending_view: RefCell<Option<(String, u32)>>,
    view_timer: slint::Timer,
    lang: Cell<Lang>,
    week_start: RefCell<String>,
    seed: Cell<u64>,
}

fn sv(s: impl Into<SharedString>) -> SharedString {
    s.into()
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

/// a covers model filled as the thumbnails arrive (`big`: the first one at 640)
fn covers(app: &Rc<AppInner>, ids: &[String], big: bool) -> ModelRc<Image> {
    let m = Rc::new(VecModel::from(vec![Image::default(); ids.len()]));
    for (i, id) in ids.iter().enumerate() {
        let w = Rc::downgrade(&m);
        let size = if big && i == 0 { 640 } else { 320 };
        crate::pages::cover(app, id, size, move |img| {
            if let Some(m) = w.upgrade() {
                if i < m.row_count() {
                    m.set_row_data(i, img);
                }
            }
        });
    }
    ModelRc::from(m)
}

fn flag_of(cc: &str) -> Option<(i32, i32)> {
    crate::explore_flags::cell(cc)
}

impl Explore {
    fn app(&self) -> Option<Rc<AppInner>> {
        self.app.upgrade()
    }

    fn lang(&self) -> Lang {
        self.lang.get()
    }

    fn t(&self, k: &str) -> String {
        crate::i18n::t(self.lang(), k)
    }

    fn tv(&self, k: &str, v: &[(&str, &str)]) -> String {
        crate::i18n::tv(self.lang(), k, v)
    }

    fn count(&self, n: i64) -> String {
        self.tv("explore.count", &[("n", &n.to_string())])
    }

    fn rnd(&self) -> f64 {
        let mut s = self.seed.get();
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        self.seed.set(s);
        (s >> 11) as f64 / (1u64 << 53) as f64
    }

    // ───────── data ─────────

    /// GET /api/explore (polls while metadata indexing runs)
    pub(crate) fn load(self: &Rc<Self>) {
        let Some(a) = self.app() else { return };
        if self.loading.replace(true) {
            return;
        }
        if !self.loaded_once.get() {
            self.publish_home();
        }
        let me = Rc::downgrade(self);
        crate::api::call::<ExploreResult>(&a, "GET", "/api/explore", None, move |_, r| {
            let Some(me) = me.upgrade() else { return };
            me.loading.set(false);
            me.loaded_once.set(true);
            match r {
                Ok(d) => {
                    me.error.borrow_mut().clear();
                    let indexing = d.indexing.total > 0 && d.indexing.done < d.indexing.total;
                    *me.data.borrow_mut() = Some(d);
                    if indexing {
                        let w = Rc::downgrade(&me);
                        me.poll_timer.start(
                            slint::TimerMode::SingleShot,
                            Duration::from_secs(20),
                            move || {
                                if let Some(m) = w.upgrade() {
                                    m.load();
                                }
                            },
                        );
                    }
                }
                Err(e) => *me.error.borrow_mut() = e.message,
            }
            me.on_data();
        });
    }

    /// the data changed: redraw whatever explore page is open
    fn on_data(self: &Rc<Self>) {
        let Some(a) = self.app() else { return };
        self.publish_home();
        let route = a.route.borrow().clone();
        match &route {
            Route::ExploreResult(..) => {
                self.publish_result(&a, &route);
                a.load_page_query();
            }
            Route::ExplorePlaces => self.publish_places(),
            _ => {}
        }
        crate::map::on_explore_data(&a);
        crate::explore_player::on_explore_data(&a);
    }

    /// throttle refreshes after server events
    fn schedule_refresh(self: &Rc<Self>) {
        let w = Rc::downgrade(self);
        self.refresh_timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(800),
            move || {
                if let Some(m) = w.upgrade() {
                    m.load();
                    m.cal.borrow_mut().take();
                    m.load_calendar();
                }
            },
        );
    }

    // ───────── home ─────────

    fn publish_home(self: &Rc<Self>) {
        let Some(a) = self.app() else { return };
        let st = a.window.global::<ExploreState>();
        let lang = self.lang();
        let d = self.data.borrow();
        st.set_loading(d.is_none() && self.error.borrow().is_empty());
        let Some(r) = d.as_ref() else {
            st.set_empty(!self.error.borrow().is_empty());
            st.set_error(self.error.borrow().as_str().into());
            return;
        };
        let x = &r.extras;
        let empty = r.years.is_empty() && r.memories.is_empty() && r.places.is_empty();
        st.set_empty(empty);
        st.set_error(SharedString::new());
        st.set_indexing(
            if r.indexing.total > 0 && r.indexing.done < r.indexing.total {
                ((r.indexing.done as f64 / r.indexing.total as f64) * 100.0).floor() as i32
            } else {
                -1
            },
        );

        // stats chips
        let mut chips = vec![];
        let mut total = 0;
        st.set_has_size(false);
        st.set_has_cloud(false);
        if let Some(s) = x.stats.as_ref().filter(|s| s.photos + s.videos > 0) {
            total = s.photos + s.videos;
            let chip = |icon: &str, text: String, key: &str| EChip {
                icon: icon.into(),
                text: text.into(),
                key: key.into(),
                tip: SharedString::new(),
            };
            chips.push(chip(
                "images",
                self.tv(
                    "insights.stats.photos",
                    &[("n", &lb::num_label(s.photos, lang))],
                ),
                "",
            ));
            if s.videos > 0 {
                chips.push(chip(
                    "film",
                    self.tv(
                        "insights.stats.videos",
                        &[("n", &lb::num_label(s.videos, lang))],
                    ),
                    "type:video",
                ));
            }
            if s.countries > 0 {
                chips.push(chip(
                    "globe",
                    self.tv(
                        "insights.stats.countries",
                        &[("n", &s.countries.to_string())],
                    ),
                    "",
                ));
            }
            if s.cities > 0 {
                chips.push(chip(
                    "map-pin",
                    self.tv("insights.stats.cities", &[("n", &s.cities.to_string())]),
                    "",
                ));
            }
            if let (Some(f), Some(l)) = (s.first_year, s.last_year) {
                let text = if f == l {
                    self.tv("insights.stats.year", &[("year", &f.to_string())])
                } else {
                    self.tv(
                        "insights.stats.years",
                        &[("from", &f.to_string()), ("to", &l.to_string())],
                    )
                };
                chips.push(chip("calendar", text, ""));
            }
            let cloud = s.cloud_bytes.unwrap_or(0);
            st.set_has_size(true);
            if cloud > 0 {
                st.set_has_cloud(true);
                st.set_size_local(
                    self.tv(
                        "insights.stats.local",
                        &[("size", &lb::bytes_label((s.bytes - cloud).max(0), lang))],
                    )
                    .into(),
                );
                st.set_size_cloud(
                    self.tv(
                        "insights.stats.cloud",
                        &[("size", &lb::bytes_label(cloud, lang))],
                    )
                    .into(),
                );
            } else {
                st.set_size_local(lb::bytes_label(s.bytes, lang).into());
            }
            let with = s.with_location;
            let without = x.no_location.unwrap_or(0);
            if with + without > 0 {
                st.set_meta_line(
                    format!(
                        "{}: {} · {}: {}",
                        self.t("meta.withLocation"),
                        lb::num_label(with, lang),
                        self.t("meta.withoutLocation"),
                        lb::num_label(without, lang)
                    )
                    .into(),
                );
                st.set_meta_share(with as f32 / (with + without) as f32);
            } else {
                st.set_meta_line(SharedString::new());
            }
        } else {
            st.set_meta_line(SharedString::new());
        }
        if total == 0 {
            total = r.years.iter().map(|y| y.count).sum();
        }
        st.set_stats(model(chips));
        let memories_all = r.memories.len() + x.year_reviews.as_ref().map_or(0, |v| v.len());
        st.set_can_surprise(total > 0 || memories_all > 0);

        // memories
        let mems: Vec<ECard> = r
            .memories
            .iter()
            .map(|m| {
                let l = lb::memory_label(m, lang);
                let n = if m.cover_ids.len() >= 3 { 3 } else { 1 };
                ECard {
                    key: sv(format!("play:{}", m.id)),
                    title: l.title.into(),
                    sub: l.subtitle.into(),
                    covers: covers(&a, &m.cover_ids[..m.cover_ids.len().min(n)], true),
                    ..Default::default()
                }
            })
            .collect();
        st.set_memories(model(mems));

        // year in review
        let yrs: Vec<ECard> = x
            .year_reviews
            .iter()
            .flatten()
            .map(|m| {
                let l = lb::memory_label(m, lang);
                let months: Vec<f32> = match &m.months {
                    Some(ms) => {
                        let max = ms.iter().copied().max().unwrap_or(1).max(1) as f32;
                        ms.iter().map(|n| *n as f32 / max).collect()
                    }
                    None => vec![],
                };
                ECard {
                    key: sv(format!("play:{}", m.id)),
                    title: l.title.into(),
                    sub: l.subtitle.into(),
                    big: lb::ymd(m.from).0.to_string().into(),
                    covers: covers(&a, &m.cover_ids[..m.cover_ids.len().min(9)], true),
                    months: model(months),
                    ..Default::default()
                }
            })
            .collect();
        st.set_year_reviews(model(yrs));

        // places + map card
        st.set_places_total(r.places.len() as i32);
        let places: Vec<ECard> = r
            .places
            .iter()
            .take(24)
            .map(|p| self.place_card(&a, p))
            .collect();
        st.set_places(model(places));
        st.set_map_dots(model(map_dots(&r.places)));
        st.set_map_sub(
            self.tv("explore.placesCount", &[("n", &r.places.len().to_string())])
                .into(),
        );
        st.set_no_location(match x.no_location.unwrap_or(0) {
            0 => SharedString::new(),
            n => self.tv("map.noLocation", &[("n", &n.to_string())]).into(),
        });
        let indexing = r.indexing.total > 0 && r.indexing.done < r.indexing.total;
        st.set_no_places_text(
            self.t(if indexing {
                "explore.noPlacesIndexing"
            } else {
                "explore.noPlaces"
            })
            .into(),
        );

        // countries
        let countries: Vec<ECard> = x
            .countries
            .iter()
            .flatten()
            .map(|c| {
                let f = flag_of(&c.cc);
                ECard {
                    key: sv(format!("country:{}", c.cc.to_uppercase())),
                    title: lb::country_name(&c.cc, lang).into(),
                    sub: format!(
                        "{} · {}",
                        self.count(c.count),
                        self.tv(
                            "insights.countrySub",
                            &[
                                ("regions", &c.regions.to_string()),
                                ("cities", &c.cities.to_string())
                            ]
                        )
                    )
                    .into(),
                    covers: covers(&a, std::slice::from_ref(&c.cover_id), false),
                    has_flag: f.is_some(),
                    flag_x: f.map_or(0, |f| f.0),
                    flag_y: f.map_or(0, |f| f.1),
                    ..Default::default()
                }
            })
            .collect();
        st.set_countries(model(countries));

        // calendar
        let cal_years = x.calendar_years.clone().unwrap_or_default();
        st.set_has_calendar(!cal_years.is_empty());
        if !cal_years.is_empty() {
            let y = self.cal_year.get();
            if !cal_years.contains(&y) {
                let counts: Vec<(i64, i64)> = r
                    .years
                    .iter()
                    .filter(|g| cal_years.contains(&g.year))
                    .map(|g| (g.year, g.count))
                    .collect();
                let pick = lb::default_calendar_year(&counts).unwrap_or(cal_years[0]);
                self.cal_year.set(pick);
                self.cal.borrow_mut().take();
            }
            let cy = self.cal_year.get();
            st.set_cal_year(cy as i32);
            st.set_cal_years(model(
                cal_years
                    .iter()
                    .take(30)
                    .map(|y| EYearBtn {
                        year: *y as i32,
                        active: *y == cy,
                    })
                    .collect(),
            ));
        }

        // time of day / colours
        let daytimes: Vec<ECard> = x
            .day_times
            .iter()
            .flatten()
            .map(|g| {
                let (tk, sk, icon) = match g.value {
                    pa_core::DayTime::Night => (
                        "insights.daytime.night",
                        "insights.daytime.nightSub",
                        "moon",
                    ),
                    pa_core::DayTime::Golden => (
                        "insights.daytime.golden",
                        "insights.daytime.goldenSub",
                        "sunset",
                    ),
                };
                let ids = g
                    .cover_ids
                    .clone()
                    .unwrap_or_else(|| vec![g.cover_id.clone()]);
                let ids = if ids.len() >= 4 {
                    ids[..4].to_vec()
                } else {
                    ids[..1.min(ids.len())].to_vec()
                };
                ECard {
                    key: sv(format!("time:{}", value_str(&g.value))),
                    title: self.t(tk).into(),
                    sub: format!("{} · {}", self.t(sk), g.count).into(),
                    icon: icon.into(),
                    covers: covers(&a, &ids, ids.len() == 1),
                    ..Default::default()
                }
            })
            .collect();
        st.set_daytimes(model(daytimes));
        let colors: Vec<ECard> = x
            .colors
            .iter()
            .flatten()
            .map(|g| {
                let ids = g
                    .cover_ids
                    .clone()
                    .unwrap_or_else(|| vec![g.cover_id.clone()]);
                let bw = g.value == pa_core::ColorBucket::Bw;
                let sw = crate::theme::color(g.value.swatch());
                ECard {
                    key: sv(format!("color:{}", g.value.as_str())),
                    title: self
                        .t(&format!("insights.color.{}", g.value.as_str()))
                        .into(),
                    badge: g.count.to_string().into(),
                    c1: if bw {
                        crate::theme::color("#f1f1f4")
                    } else {
                        sw
                    },
                    c2: if bw {
                        crate::theme::color("#2a2a33")
                    } else {
                        sw
                    },
                    covers: covers(&a, &ids[..ids.len().min(2)], false),
                    ..Default::default()
                }
            })
            .collect();
        st.set_colors(model(colors));

        // shapes: orientations + video lengths
        let mut shapes = vec![];
        for g in x.orientations.iter().flatten() {
            let v = value_str(&g.value);
            shapes.push(ECard {
                key: sv(format!("shape:{v}")),
                title: self.t(&format!("insights.orient.{v}")).into(),
                sub: g.count.to_string().into(),
                icon: match g.value {
                    pa_core::Orientation::Portrait => "portrait",
                    pa_core::Orientation::Landscape => "landscape",
                    pa_core::Orientation::Square => "square",
                    pa_core::Orientation::Panorama => "gallery",
                }
                .into(),
                covers: covers(&a, std::slice::from_ref(&g.cover_id), false),
                ..Default::default()
            });
        }
        for g in x.video_lengths.iter().flatten() {
            let v = value_str(&g.value);
            shapes.push(ECard {
                key: sv(format!("length:{v}")),
                title: self.t(&format!("insights.vlen.{v}")).into(),
                sub: format!("{} · {}", self.t(&format!("insights.vlen.{v}Sub")), g.count).into(),
                icon: if v == "short" {
                    "timer"
                } else {
                    "clapperboard"
                }
                .into(),
                covers: covers(&a, std::slice::from_ref(&g.cover_id), false),
                ..Default::default()
            });
        }
        st.set_shapes(model(shapes));

        // types (TYPE_ORDER) + favourites
        let mut types = vec![];
        for (flag, icon) in [
            ("video", "film"),
            ("selfie", "scan-face"),
            ("screenshot", "smartphone"),
            ("panorama", "gallery"),
            ("animated", "image-play"),
        ] {
            if let Some(g) = r.types.iter().find(|g| value_str(&g.flag) == flag) {
                types.push(ECard {
                    key: sv(format!("type:{flag}")),
                    title: self.t(&format!("explore.type.{flag}")).into(),
                    sub: g.count.to_string().into(),
                    icon: icon.into(),
                    covers: covers(&a, std::slice::from_ref(&g.cover_id), false),
                    ..Default::default()
                });
            }
        }
        types.push(ECard {
            key: sv("favorites"),
            title: self.t("explore.type.favorites").into(),
            icon: "heart".into(),
            ..Default::default()
        });
        st.set_types(model(types));

        // recent / largest
        let ranked = |list: &Option<Vec<pa_core::RankedItem>>, kind: &str| -> Vec<ECard> {
            list.iter()
                .flatten()
                .take(16)
                .map(|it| ECard {
                    key: sv(format!("ranked:{kind}:{}", it.id)),
                    badge: if kind == "largest" {
                        lb::bytes_label(it.value, lang)
                    } else {
                        lb::day_month(it.value + crate::viewer_fmt::local_offset_ms(), lang)
                    }
                    .into(),
                    covers: covers(&a, std::slice::from_ref(&it.id), false),
                    ..Default::default()
                })
                .collect()
        };
        st.set_recent(model(ranked(&x.recent, "recent")));
        st.set_largest(model(ranked(&x.largest, "largest")));

        // duplicates
        st.set_has_dups(x.duplicates.is_some());
        if let Some(dd) = &x.duplicates {
            let mut c = ECard {
                title: self.t("insights.dup.title").into(),
                covers: covers(&a, &dd.cover_ids[..dd.cover_ids.len().min(2)], false),
                ..Default::default()
            };
            if dd.groups > 0 {
                c.sub = self
                    .tv(
                        "insights.dup.summary",
                        &[
                            ("groups", &dd.groups.to_string()),
                            ("files", &dd.files.to_string()),
                        ],
                    )
                    .into();
                c.big = self
                    .tv(
                        "insights.dup.wasted",
                        &[("size", &lb::bytes_label(dd.wasted, lang))],
                    )
                    .into();
                if dd.pending > 0 {
                    c.extra = self
                        .tv("insights.dup.pending", &[("n", &dd.pending.to_string())])
                        .into();
                }
            } else if dd.pending > 0 {
                c.sub = self
                    .tv("insights.dup.pending", &[("n", &dd.pending.to_string())])
                    .into();
            } else {
                c.sub = self.t("insights.dup.none").into();
            }
            st.set_dups(c);
        }

        // cameras: wrapping pills (approximate text widths → positions)
        let cw = a.window.window().size().width as f64 / a.window.window().scale_factor() as f64;
        let avail =
            (cw - a.window.global::<crate::Theme>().get_rail_w() as f64 - 56.0 - 12.0).min(1800.0);
        let (mut x0, mut y0) = (0.0f64, 0.0f64);
        let mut cams = vec![];
        for c in r.cameras.iter().take(30) {
            let label = lb::camera_label(c.make.as_deref(), Some(&c.model));
            let badge = c.count.to_string();
            let w = 4.0 + 40.0 + 10.0 + text_w(&label, 14.0) + 10.0 + text_w(&badge, 12.0) + 16.0;
            if x0 > 0.0 && x0 + w > avail {
                x0 = 0.0;
                y0 += 48.0 + 8.0;
            }
            cams.push(ECard {
                key: sv(format!("camera:{}", c.key)),
                title: label.into(),
                badge: badge.into(),
                covers: covers(&a, std::slice::from_ref(&c.cover_id), false),
                months: model(vec![x0 as f32, y0 as f32]),
                ..Default::default()
            });
            x0 += w + 8.0;
        }
        st.set_cam_height(if cams.is_empty() {
            0.0
        } else {
            (y0 + 48.0) as f32
        });
        st.set_cameras(model(cams));

        // years
        let years: Vec<ECard> = r
            .years
            .iter()
            .map(|y| ECard {
                key: sv(format!("year:{}", y.year)),
                title: y.year.to_string().into(),
                sub: self.count(y.count).into(),
                covers: covers(&a, std::slice::from_ref(&y.cover_id), false),
                ..Default::default()
            })
            .collect();
        st.set_years(model(years));
        drop(d);
        if a.window.global::<ExploreState>().get_has_calendar() {
            self.publish_calendar();
            self.load_calendar();
        }
    }

    fn place_card(&self, a: &Rc<AppInner>, p: &pa_core::PlaceGroup) -> ECard {
        ECard {
            key: sv(format!("place:{}", p.key)),
            title: p.name.clone().into(),
            sub: format!(
                "{} · {}",
                lb::country_name(&p.cc, self.lang()),
                self.count(p.count)
            )
            .into(),
            covers: covers(a, std::slice::from_ref(&p.cover_id), false),
            ..Default::default()
        }
    }

    fn city_card(&self, a: &Rc<AppInner>, c: &pa_core::CityGroup) -> ECard {
        ECard {
            key: sv(format!("city:{}", c.key)),
            title: c.name.clone().into(),
            sub: self.count(c.count).into(),
            covers: covers(a, std::slice::from_ref(&c.cover_id), false),
            ..Default::default()
        }
    }

    // ───────── calendar ─────────

    fn load_calendar(self: &Rc<Self>) {
        let Some(a) = self.app() else { return };
        let year = self.cal_year.get();
        if year == 0 || self.cal.borrow().as_ref().is_some_and(|c| c.year == year) {
            return;
        }
        a.window.global::<ExploreState>().set_cal_loading(true);
        let me = Rc::downgrade(self);
        crate::api::call::<CalendarYear>(
            &a,
            "GET",
            &format!("/api/explore/calendar?year={year}"),
            None,
            move |_, r| {
                let Some(me) = me.upgrade() else { return };
                if me.cal_year.get() != year {
                    return;
                }
                if let Ok(c) = r {
                    *me.cal.borrow_mut() = Some(c);
                }
                me.publish_calendar();
            },
        );
    }

    fn publish_calendar(&self) {
        let Some(a) = self.app() else { return };
        let st = a.window.global::<ExploreState>();
        let lang = self.lang();
        let year = self.cal_year.get();
        let ws = lb::week_start_day(&self.week_start.borrow());
        let (lead, days, months) = lb::calendar_layout(year, ws);
        let cal = self.cal.borrow();
        let cal = cal.as_ref().filter(|c| c.year == year);
        st.set_cal_loading(cal.is_none());
        let n = cal.map_or(days, |c| c.days.len() as i64);
        let jan1 = lb::utc(year, 1, 1);
        let cells: Vec<ECalCell> = (0..n)
            .map(|d| {
                let c = cal
                    .and_then(|c| c.days.get(d as usize))
                    .copied()
                    .unwrap_or(0);
                let key = lb::day_key(jan1 + d * lb::DAY_MS);
                let date = lb::day_label(&key, lang, false);
                let p = d + lead;
                ECalCell {
                    col: (p / 7) as i32,
                    row: (p % 7) as i32,
                    level: lb::heat_level(c, cal.map_or(0, |c| c.max)),
                    label: if c > 0 {
                        self.tv(
                            "insights.calendar.day",
                            &[("date", &date), ("n", &c.to_string())],
                        )
                    } else {
                        self.tv("insights.calendar.dayEmpty", &[("date", &date)])
                    }
                    .into(),
                    key: key.into(),
                }
            })
            .collect();
        st.set_cal_cols(((n + lead + 6) / 7) as i32);
        st.set_cal_cells(model(cells));
        st.set_cal_months(model(
            months
                .into_iter()
                .map(|(m, col)| ECalMonth {
                    text: lb::month_short(m, lang).into(),
                    col: col as i32,
                })
                .collect(),
        ));
        st.set_cal_total(match cal {
            Some(c) => self
                .tv(
                    "insights.calendar.total",
                    &[("year", &year.to_string()), ("n", &c.total.to_string())],
                )
                .into(),
            None => SharedString::new(),
        });
        st.set_cal_year(year as i32);
        if let Some(r) = self.data.borrow().as_ref() {
            let ys = r.extras.calendar_years.clone().unwrap_or_default();
            st.set_cal_years(model(
                ys.iter()
                    .take(30)
                    .map(|y| EYearBtn {
                        year: *y as i32,
                        active: *y == year,
                    })
                    .collect(),
            ));
        }
    }

    // ───────── navigation ─────────

    /// ExploreState.open(key)
    pub(crate) fn open(self: &Rc<Self>, key: &str) {
        let Some(a) = self.app() else { return };
        let (head, rest) = key.split_once(':').unwrap_or((key, ""));
        let go = |r: Route| a.go(r, true);
        match head {
            "play" => crate::explore_player::open_id(&a, rest),
            "memory" | "place" | "country" | "city" | "camera" | "type" | "year" | "color"
            | "time" | "shape" | "length" | "day" => {
                go(Route::ExploreResult(head.into(), rest.into()))
            }
            "largest" | "recent" | "nogps" => go(Route::ExploreResult(head.into(), String::new())),
            "ranked" => {
                let (kind, id) = rest.split_once(':').unwrap_or((rest, ""));
                go(Route::ExploreResult(kind.into(), String::new()));
                self.view_when_loaded(id);
            }
            "places" => go(Route::ExplorePlaces),
            "duplicates" => go(Route::ExploreDuplicates),
            "map" => go(Route::ExploreMap {
                place: rest.into(),
                focus: None,
            }),
            "favorites" => a.navigate(Page::Favorites, true),
            "meta" => {
                a.window
                    .global::<Shell>()
                    .set_settings_section("explore".into());
                a.go(Route::Settings, true);
            }
            "" => {}
            _ => log::warn!("explore: unknown key {key}"),
        }
    }

    /// open the viewer on `id` once the grid holds it (the `?view=` of the web UI's links)
    pub(crate) fn view_when_loaded(self: &Rc<Self>, id: &str) {
        if id.is_empty() {
            return;
        }
        *self.pending_view.borrow_mut() = Some((id.to_string(), 0));
        let w = Rc::downgrade(self);
        self.view_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(120),
            move || {
                let Some(me) = w.upgrade() else { return };
                let Some(a) = me.app() else { return };
                let mut p = me.pending_view.borrow_mut();
                let Some((id, n)) = p.as_mut() else {
                    me.view_timer.stop();
                    return;
                };
                *n += 1;
                let idx = a.grid.0.borrow().index_of_id(id);
                if let Some(i) = idx {
                    p.take();
                    drop(p);
                    me.view_timer.stop();
                    a.grid.0.borrow_mut().scroll_to_index(i, true);
                    crate::actions_ui::open_viewer(i);
                } else if *n > 40 {
                    p.take();
                    me.view_timer.stop();
                }
            },
        );
    }

    /// "Surprise me": half the time a memory, otherwise a random photo on its day
    fn surprise(self: &Rc<Self>) {
        let Some(a) = self.app() else { return };
        let (mems, total) = {
            let d = self.data.borrow();
            let Some(r) = d.as_ref() else { return };
            let mut m: Vec<String> = r.memories.iter().map(|m| m.id.clone()).collect();
            m.extend(r.extras.year_reviews.iter().flatten().map(|m| m.id.clone()));
            let total = r
                .extras
                .stats
                .as_ref()
                .map(|s| s.photos + s.videos)
                .filter(|n| *n > 0)
                .unwrap_or_else(|| r.years.iter().map(|y| y.count).sum());
            (m, total)
        };
        if !mems.is_empty() && (self.rnd() < 0.5 || total == 0) {
            let i = (self.rnd() * mems.len() as f64) as usize % mems.len();
            crate::explore_player::open_id(&a, &mems[i]);
            return;
        }
        let st = a.window.global::<ExploreState>();
        st.set_surprise_busy(true);
        let q = BrowseQuery {
            offset: Some((self.rnd() * total as f64).floor() as i64),
            limit: Some(1),
            ..lb::explore_query()
        };
        let me = Rc::downgrade(self);
        let fut = a.backend.browse(q);
        a.rt.spawn_then(fut, move |r| {
            let Some(me) = me.upgrade() else { return };
            let Some(a) = me.app() else { return };
            a.window.global::<ExploreState>().set_surprise_busy(false);
            match r {
                Ok(res) => match res.items.into_iter().next() {
                    Some(it) => {
                        a.go(
                            Route::ExploreResult("day".into(), lb::day_key(it.local_at)),
                            true,
                        );
                        me.view_when_loaded(&it.id);
                    }
                    None => a.toast(crate::ToastKind::Info, &me.t("insights.surpriseNone"), ""),
                },
                Err(e) => a.error_toast(&e.message),
            }
        });
    }

    // ───────── result pages ─────────

    /// route params → query + header (ResultPage `useResolved`)
    pub(crate) fn resolve(&self, kind: &str, key: &str) -> Resolved {
        let lang = self.lang();
        let d = self.data.borrow();
        let loaded = d.is_some();
        let empty = ExploreResult {
            extras: Default::default(),
            memories: vec![],
            places: vec![],
            types: vec![],
            cameras: vec![],
            years: vec![],
            indexing: pa_core::Progress { done: 0, total: 0 },
        };
        let r = d.as_ref().unwrap_or(&empty);
        let q = |f: &dyn Fn(&mut BrowseQuery)| {
            let mut b = lb::explore_query();
            f(&mut b);
            Some(b)
        };
        let mut res = Resolved::default();
        match kind {
            "place" => {
                let g = r.places.iter().find(|p| p.key == key);
                let cc = key.split('-').next().unwrap_or("");
                let cities: Vec<String> = r
                    .extras
                    .cities
                    .iter()
                    .flatten()
                    .filter(|c| c.place_key == key)
                    .map(|c| c.key.clone())
                    .collect();
                if cities.len() > 1 {
                    res.cities = cities;
                }
                res.query = q(&|b| b.place = Some(key.into()));
                res.title = g
                    .map(|g| g.name.clone())
                    .unwrap_or_else(|| lb::country_name(cc, lang));
                let mut sub = vec![];
                let gcc = g.map(|g| g.cc.as_str()).unwrap_or(cc);
                if g.is_some() {
                    sub.push(lb::country_name(gcc, lang));
                }
                if let Some(city) = g
                    .and_then(|g| g.city.as_ref())
                    .filter(|c| Some(c.as_str()) != g.map(|g| g.name.as_str()))
                {
                    sub.push(city.clone());
                }
                res.subtitle = sub.join(" · ");
                res.icon = "map-pin";
                res.place_key = Some(key.into());
            }
            "type" => {
                let ok = matches!(
                    key,
                    "video" | "screenshot" | "selfie" | "panorama" | "animated"
                );
                res.query = if ok {
                    q(&|b| {
                        b.flag = serde_json::from_value(serde_json::Value::String(key.into())).ok()
                    })
                } else {
                    None
                };
                res.title = if ok {
                    self.t(&format!("explore.type.{key}"))
                } else {
                    key.into()
                };
                res.icon = "sparkles";
                res.missing = !ok;
            }
            "camera" => {
                let g = r.cameras.iter().find(|c| c.key == key);
                let (mk, md) = key.split_once('|').unwrap_or((key, ""));
                res.query = q(&|b| b.camera = Some(key.into()));
                res.title = match g {
                    Some(g) => lb::camera_label(g.make.as_deref(), Some(&g.model)),
                    None => lb::camera_label(Some(mk), Some(md)),
                };
                res.icon = "camera";
            }
            "year" => {
                let y: i64 = key.parse().unwrap_or(0);
                let ok = y > 1900 && y < 3000;
                let (from, to) = lb::year_range(y);
                res.query = if ok {
                    q(&|b| {
                        b.from = Some(from);
                        b.to = Some(to);
                    })
                } else {
                    None
                };
                res.title = key.into();
                res.icon = "calendar";
                res.missing = !ok;
            }
            "memory" => {
                let m = r
                    .memories
                    .iter()
                    .chain(r.extras.year_reviews.iter().flatten())
                    .find(|m| m.id == key)
                    .cloned()
                    .or_else(|| {
                        (loaded || key.starts_with("year-"))
                            .then(|| lb::memory_from_id(key, crate::fmt::now_ms()))
                            .flatten()
                    });
                res.icon = "sparkles";
                match m {
                    Some(m) => {
                        let l = lb::memory_label(&m, lang);
                        res.query = q(&|b| {
                            b.from = Some(m.from);
                            b.to = Some(m.to);
                        });
                        res.title = l.title;
                        res.subtitle = l.subtitle;
                        res.memory = Some(m);
                    }
                    None => res.missing = loaded,
                }
            }
            "country" => {
                let cc = key.to_uppercase();
                let ok = cc.len() == 2 && cc.bytes().all(|b| b.is_ascii_uppercase());
                let c = r.extras.countries.iter().flatten().find(|c| c.cc == cc);
                res.regions = r
                    .places
                    .iter()
                    .filter(|p| p.cc == cc)
                    .map(|p| p.key.clone())
                    .collect();
                res.cities = r
                    .extras
                    .cities
                    .iter()
                    .flatten()
                    .filter(|c| c.cc == cc)
                    .take(60)
                    .map(|c| c.key.clone())
                    .collect();
                res.query = if ok {
                    q(&|b| b.place = Some(cc.clone()))
                } else {
                    None
                };
                res.title = lb::country_name(&cc, lang);
                res.subtitle = c
                    .map(|c| {
                        self.tv(
                            "insights.countrySub",
                            &[
                                ("regions", &c.regions.to_string()),
                                ("cities", &c.cities.to_string()),
                            ],
                        )
                    })
                    .unwrap_or_default();
                res.flag = flag_of(&cc);
                res.icon = "globe";
                res.missing = !ok;
                res.place_key = ok.then_some(cc);
            }
            "city" => {
                let (place_key, city) = key.split_once('|').unwrap_or((key, ""));
                let ok = !place_key.is_empty() && !city.is_empty();
                let cc = place_key.split('-').next().unwrap_or("");
                let g = r.places.iter().find(|p| p.key == place_key);
                res.query = if ok {
                    q(&|b| {
                        b.place = Some(place_key.into());
                        b.city = Some(city.into());
                    })
                } else {
                    None
                };
                res.title = city.into();
                let mut sub = vec![lb::country_name(cc, lang)];
                if let Some(g) = g.filter(|g| g.name != city) {
                    sub.push(g.name.clone());
                }
                res.subtitle = sub.join(" · ");
                res.icon = "map-pin";
                res.missing = !ok;
                res.place_key = ok.then(|| place_key.to_string());
            }
            "color" => {
                let c = pa_core::parse_color_bucket(key);
                res.query = c.and_then(|c| q(&|b| b.color = Some(c)));
                res.title = if c.is_some() {
                    self.t(&format!("insights.color.{key}"))
                } else {
                    key.into()
                };
                res.icon = "palette";
                res.missing = c.is_none();
            }
            "time" => {
                let v: Option<pa_core::DayTime> =
                    serde_json::from_value(serde_json::Value::String(key.into())).ok();
                res.query = v.and_then(|v| q(&|b| b.daytime = Some(v)));
                if v.is_some() {
                    res.title = self.t(&format!("insights.daytime.{key}"));
                    res.subtitle = self.t(&format!("insights.daytime.{key}Sub"));
                } else {
                    res.title = key.into();
                }
                res.icon = if key == "night" { "moon" } else { "sunset" };
                res.missing = v.is_none();
            }
            "shape" => {
                let v: Option<pa_core::Orientation> =
                    serde_json::from_value(serde_json::Value::String(key.into())).ok();
                res.query = v.and_then(|v| q(&|b| b.orient = Some(v)));
                res.title = if v.is_some() {
                    self.t(&format!("insights.orient.{key}"))
                } else {
                    key.into()
                };
                res.icon = "shapes";
                res.missing = v.is_none();
            }
            "length" => {
                let v: Option<pa_core::VideoLength> =
                    serde_json::from_value(serde_json::Value::String(key.into())).ok();
                res.query = v.and_then(|v| {
                    q(&|b| {
                        b.vlen = Some(v);
                        b.kind = Some(pa_core::KindFilter::Video);
                    })
                });
                if v.is_some() {
                    res.title = self.t(&format!("insights.vlen.{key}"));
                    res.subtitle = self.t(&format!("insights.vlen.{key}Sub"));
                } else {
                    res.title = key.into();
                }
                res.icon = "clapperboard";
                res.missing = v.is_none();
            }
            "largest" => {
                res.query = q(&|b| {
                    b.sort = Some(SortKey::Size);
                    b.order = Some(SortDir::Desc);
                });
                res.title = self.t("insights.largest");
                res.subtitle = self.t("insights.largestSub");
                res.icon = "hard-drive";
                res.no_groups = true;
            }
            "recent" => {
                res.query = q(&|b| {
                    b.sort = Some(SortKey::Mtime);
                    b.order = Some(SortDir::Desc);
                    b.by_added = Some(true);
                });
                res.title = self.t("insights.recent");
                res.subtitle = self.t("insights.recentSub");
                res.icon = "history";
                res.no_groups = true;
            }
            "day" => {
                let dr = lb::day_range(key);
                res.query = dr.and_then(|(from, to)| {
                    q(&|b| {
                        b.from = Some(from);
                        b.to = Some(to);
                    })
                });
                res.title = if dr.is_some() {
                    lb::day_label(key, lang, true)
                } else {
                    key.into()
                };
                res.icon = "calendar";
                res.missing = dr.is_none();
            }
            "nogps" => {
                res.query = q(&|b| b.gps = Some(GpsFilter::None));
                res.title = self.t("map.noLocationTitle");
                res.icon = "map-pin-off";
                res.hint = format!(
                    "{} {}",
                    self.t("map.noLocationHint"),
                    self.t("meta.takeoutHint")
                );
            }
            _ => res.missing = true,
        }
        res
    }

    fn publish_result(&self, a: &Rc<AppInner>, route: &Route) {
        let Route::ExploreResult(kind, key) = route else {
            return;
        };
        let res = self.resolve(kind, key);
        let st = a.window.global::<EResultState>();
        st.set_title(res.title.clone().into());
        st.set_subtitle(res.subtitle.clone().into());
        st.set_icon(res.icon.into());
        st.set_has_flag(res.flag.is_some());
        if let Some((x, y)) = res.flag {
            st.set_flag_x(x);
            st.set_flag_y(y);
        }
        st.set_missing(res.missing);
        st.set_can_play(res.memory.is_some());
        st.set_can_map(res.place_key.is_some());
        st.set_memory_id(
            res.memory
                .as_ref()
                .map(|m| m.id.clone())
                .unwrap_or_default()
                .into(),
        );
        st.set_place_key(res.place_key.clone().unwrap_or_default().into());
        st.set_hint(res.hint.clone().into());
        st.set_no_groups(res.no_groups);
        let d = self.data.borrow();
        let (regions, cities) = match d.as_ref() {
            Some(r) => (
                res.regions
                    .iter()
                    .filter_map(|k| r.places.iter().find(|p| &p.key == k))
                    .map(|p| self.place_card(a, p))
                    .collect(),
                res.cities
                    .iter()
                    .filter_map(|k| r.extras.cities.iter().flatten().find(|c| &c.key == k))
                    .map(|c| self.city_card(a, c))
                    .collect(),
            ),
            None => (vec![], vec![]),
        };
        st.set_regions(model(regions));
        st.set_cities(model(cities));
    }

    // ───────── places ─────────

    fn publish_places(&self) {
        let Some(a) = self.app() else { return };
        let st = a.window.global::<EPlacesState>();
        let lang = self.lang();
        let d = self.data.borrow();
        st.set_loading(d.is_none());
        let Some(r) = d.as_ref() else { return };
        st.set_sub(
            self.tv("explore.placesCount", &[("n", &r.places.len().to_string())])
                .into(),
        );
        st.set_dots(model(map_dots(&r.places)));
        let indexing = r.indexing.total > 0 && r.indexing.done < r.indexing.total;
        st.set_empty_text(
            self.t(if indexing {
                "explore.noPlacesIndexing"
            } else {
                "explore.noPlaces"
            })
            .into(),
        );
        let mut by: Vec<(String, Vec<&pa_core::PlaceGroup>, i64)> = vec![];
        for p in &r.places {
            match by.iter_mut().find(|g| g.0 == p.cc) {
                Some(g) => {
                    g.1.push(p);
                    g.2 += p.count;
                }
                None => by.push((p.cc.clone(), vec![p], p.count)),
            }
        }
        by.sort_by_key(|g| std::cmp::Reverse(g.2));
        let groups: Vec<EPlaceGroup> = by
            .into_iter()
            .map(|(cc, list, n)| {
                let f = flag_of(&cc);
                EPlaceGroup {
                    name: lb::country_name(&cc, lang).into(),
                    count: self.count(n).into(),
                    has_flag: f.is_some(),
                    flag_x: f.map_or(0, |f| f.0),
                    flag_y: f.map_or(0, |f| f.1),
                    cards: model(list.iter().map(|p| self.place_card(&a, p)).collect()),
                    cc: cc.into(),
                }
            })
            .collect();
        st.set_groups(model(groups));
    }

    // ───────── duplicates ─────────

    fn load_dups(self: &Rc<Self>) {
        let Some(a) = self.app() else { return };
        let limit = {
            let mut d = self.dups.borrow_mut();
            if d.limit == 0 {
                d.limit = DUP_PAGE;
            }
            d.loading = true;
            d.limit
        };
        let st = a.window.global::<EDupState>();
        st.set_more_busy(true);
        if self.dups.borrow().data.is_none() {
            st.set_loading(true);
        }
        let me = Rc::downgrade(self);
        crate::api::call::<DuplicatesResponse>(
            &a,
            "GET",
            &format!("/api/explore/duplicates?offset=0&limit={limit}"),
            None,
            move |a, r| {
                let Some(me) = me.upgrade() else { return };
                me.dups.borrow_mut().loading = false;
                match r {
                    Ok(d) => {
                        let pending = d.pending > 0;
                        me.set_dups(d);
                        if pending && matches!(&*a.route.borrow(), Route::ExploreDuplicates) {
                            let w = Rc::downgrade(&me);
                            me.poll_timer.start(
                                slint::TimerMode::SingleShot,
                                Duration::from_secs(5),
                                move || {
                                    if let Some(m) = w.upgrade() {
                                        m.load_dups();
                                    }
                                },
                            );
                        }
                    }
                    Err(e) => {
                        a.error_toast(&e.message);
                        me.publish_dups();
                    }
                }
            },
        );
    }

    fn set_dups(&self, d: DuplicatesResponse) {
        let by_id: std::collections::HashMap<&str, &MediaItem> =
            d.items.iter().map(|i| (i.id.as_str(), i)).collect();
        let groups = d
            .groups
            .iter()
            .map(|g| {
                let mut items: Vec<MediaItem> = g
                    .ids
                    .iter()
                    .filter_map(|id| by_id.get(id.as_str()).map(|i| (*i).clone()))
                    .collect();
                // oldest file first: the one "select extra copies" keeps
                items.sort_by(|a, b| {
                    a.mtime.cmp(&b.mtime).then(
                        a.id.parse::<i64>()
                            .unwrap_or(0)
                            .cmp(&b.id.parse::<i64>().unwrap_or(0)),
                    )
                });
                (g.clone(), items)
            })
            .filter(|(_, items)| items.len() > 1)
            .collect();
        {
            let mut s = self.dups.borrow_mut();
            s.groups = groups;
            s.data = Some(d);
            s.model = None;
        }
        self.publish_dups();
    }

    fn publish_dups(&self) {
        let Some(a) = self.app() else { return };
        let st = a.window.global::<EDupState>();
        let lang = self.lang();
        let mut s = self.dups.borrow_mut();
        st.set_more_busy(s.loading);
        st.set_loading(s.data.is_none());
        let Some(d) = s.data.as_ref() else { return };
        let files: usize = s.groups.iter().map(|g| g.1.len()).sum();
        let wasted: i64 = s
            .groups
            .iter()
            .map(|g| g.0.size * (g.1.len() as i64 - 1))
            .sum();
        let mut parts = vec![self.tv(
            "insights.dup.summary",
            &[
                ("groups", &d.total.to_string()),
                ("files", &files.to_string()),
            ],
        )];
        if wasted > 0 {
            parts.push(self.tv(
                "insights.dup.wasted",
                &[("size", &lb::bytes_label(wasted, lang))],
            ));
        }
        if d.pending > 0 {
            parts.push(self.tv("insights.dup.pending", &[("n", &d.pending.to_string())]));
        }
        st.set_summary(parts.join(" · ").into());
        st.set_empty_text(
            if d.pending > 0 {
                self.tv("insights.dup.pending", &[("n", &d.pending.to_string())])
            } else {
                self.t("insights.dup.none")
            }
            .into(),
        );
        st.set_can_more(d.total as usize > s.groups.len() && s.groups.len() >= s.limit);
        let sel = a.grid.0.borrow().ix.selection.clone();
        let groups: Vec<EDupGroup> = s
            .groups
            .iter()
            .map(|(g, items)| EDupGroup {
                title: self
                    .tv(
                        "insights.dup.group",
                        &[
                            ("n", &items.len().to_string()),
                            ("size", &lb::bytes_label(g.size, lang)),
                        ],
                    )
                    .into(),
                name: items[0].name.clone().into(),
                items: model(
                    items
                        .iter()
                        .enumerate()
                        .map(|(i, it)| EDupItem {
                            name: it.name.clone().into(),
                            path: d
                                .paths
                                .get(&it.id)
                                .cloned()
                                .filter(|p| !p.is_empty())
                                .unwrap_or_else(|| "—".into())
                                .into(),
                            cover: covers(&a, std::slice::from_ref(&it.id), false),
                            selected: sel.contains(&it.id),
                            original: i == 0,
                        })
                        .collect(),
                ),
            })
            .collect();
        let m = Rc::new(VecModel::from(groups));
        s.model = Some(m.clone());
        st.set_groups(ModelRc::from(m));
    }

    /// selection changed: only flip the `selected` flags (keeps the thumbnails)
    fn refresh_dup_selection(&self) {
        let Some(a) = self.app() else { return };
        let s = self.dups.borrow();
        let Some(m) = s.model.as_ref() else { return };
        let sel = a.grid.0.borrow().ix.selection.clone();
        for (gi, (_, items)) in s.groups.iter().enumerate() {
            let Some(g) = m.row_data(gi) else { continue };
            for (ii, it) in items.iter().enumerate() {
                if let Some(mut row) = g.items.row_data(ii) {
                    let want = sel.contains(&it.id);
                    if row.selected != want {
                        row.selected = want;
                        g.items.set_row_data(ii, row);
                    }
                }
            }
        }
    }

    fn dup_item(&self, gi: usize, ii: usize) -> Option<MediaItem> {
        self.dups
            .borrow()
            .groups
            .get(gi)
            .and_then(|g| g.1.get(ii))
            .cloned()
    }

    fn dup_select(&self, f: impl FnOnce(&mut crate::selection::Selection)) {
        let Some(a) = self.app() else { return };
        {
            let mut g = a.grid.0.borrow_mut();
            f(&mut g.ix.selection);
            g.selection_changed();
            g.notify_listeners();
        }
        self.refresh_dup_selection();
    }
}

/// serde string of a unit enum value ("night", "portrait" …)
fn value_str<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// approximate text width (Inter, medium): pill layout without font metrics
fn text_w(s: &str, size: f64) -> f64 {
    s.chars()
        .map(|c| match c {
            'i' | 'l' | 'j' | 'I' | '|' | '.' | ',' | '\'' | ' ' => 0.3,
            'm' | 'w' | 'M' | 'W' => 0.85,
            c if c.is_ascii_uppercase() => 0.68,
            c if c.is_ascii_digit() => 0.6,
            _ => 0.56,
        })
        .sum::<f64>()
        * size
}

/// MapCard: places projected into their bounding box (+ 18 % / 2° padding)
fn map_dots(places: &[pa_core::PlaceGroup]) -> Vec<EDot> {
    let pts = &places[..places.len().min(200)];
    if pts.is_empty() {
        return vec![];
    }
    let (mut min_lat, mut max_lat, mut min_lon, mut max_lon) =
        (90.0f64, -90.0f64, 180.0f64, -180.0f64);
    for p in pts {
        min_lat = min_lat.min(p.lat);
        max_lat = max_lat.max(p.lat);
        min_lon = min_lon.min(p.lon);
        max_lon = max_lon.max(p.lon);
    }
    let pad_lon = 2f64.max((max_lon - min_lon) * 0.18);
    let pad_lat = 2f64.max((max_lat - min_lat) * 0.18);
    let w = max_lon - min_lon + pad_lon * 2.0;
    let h = max_lat - min_lat + pad_lat * 2.0;
    let max = pts.iter().map(|p| p.count).max().unwrap_or(1).max(1) as f64;
    pts.iter()
        .map(|p| EDot {
            x: ((p.lon - min_lon + pad_lon) / w) as f32,
            y: ((max_lat + pad_lat - p.lat) / h) as f32,
            r: (5.0 + (p.count as f64 / max).sqrt() * 9.0) as f32,
        })
        .collect()
}

// ───────── app glue ─────────

/// wire the Explore globals
pub(crate) fn install(app: &Rc<AppInner>) {
    let ex = Rc::new(Explore {
        app: Rc::downgrade(app),
        data: RefCell::new(None),
        loading: Cell::new(false),
        loaded_once: Cell::new(false),
        error: RefCell::new(String::new()),
        refresh_timer: slint::Timer::default(),
        poll_timer: slint::Timer::default(),
        cal_year: Cell::new(0),
        cal: RefCell::new(None),
        dups: RefCell::new(Dups::default()),
        pending_view: RefCell::new(None),
        view_timer: slint::Timer::default(),
        lang: Cell::new(app.lang()),
        week_start: RefCell::new(app.prefs.borrow().week_start.clone()),
        seed: Cell::new((crate::fmt::now_ms() as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1),
    });
    EX.with(|e| *e.borrow_mut() = Some(ex.clone()));
    let st = app.window.global::<ExploreState>();
    let w = Rc::downgrade(&ex);
    st.on_open(move |k| {
        if let Some(e) = w.upgrade() {
            e.open(&k);
        }
    });
    let w = Rc::downgrade(&ex);
    st.on_surprise(move || {
        if let Some(e) = w.upgrade() {
            e.surprise();
        }
    });
    let w = Rc::downgrade(&ex);
    st.on_cal_pick(move |y| {
        if let Some(e) = w.upgrade() {
            e.cal_year.set(y as i64);
            e.publish_calendar();
            e.load_calendar();
        }
    });
    let ds = app.window.global::<EDupState>();
    let w = Rc::downgrade(&ex);
    ds.on_open_item(move |gi, ii| {
        if let Some(e) = w.upgrade() {
            if let Some(it) = e.dup_item(gi as usize, ii as usize) {
                crate::actions_ui::open_viewer_item(it);
            }
        }
    });
    let w = Rc::downgrade(&ex);
    ds.on_toggle(move |gi, ii| {
        if let Some(e) = w.upgrade() {
            if let Some(it) = e.dup_item(gi as usize, ii as usize) {
                e.dup_select(|s| {
                    if s.contains(&it.id) {
                        s.remove(std::slice::from_ref(&it.id));
                    } else {
                        s.add([it.clone()]);
                    }
                });
            }
        }
    });
    let w = Rc::downgrade(&ex);
    ds.on_select_extra(move || {
        if let Some(e) = w.upgrade() {
            let extra: Vec<MediaItem> = e
                .dups
                .borrow()
                .groups
                .iter()
                .flat_map(|g| g.1.iter().skip(1).cloned())
                .collect();
            e.dup_select(|s| s.set(extra));
        }
    });
    let w = Rc::downgrade(&ex);
    ds.on_more(move || {
        if let Some(e) = w.upgrade() {
            e.dups.borrow_mut().limit += DUP_PAGE;
            e.load_dups();
        }
    });
    // the selection bar clears / changes the selection: follow it on the duplicates page
    let w = Rc::downgrade(&ex);
    app.grid.0.borrow_mut().ix.listeners.push(Rc::new(move || {
        if let Some(e) = w.upgrade() {
            if let Some(a) = e.app() {
                if matches!(&*a.route.borrow(), Route::ExploreDuplicates) {
                    e.refresh_dup_selection();
                }
            }
        }
    }));
    crate::explore_player::install(app);
    crate::map::install(app);
    ex.load();
}

/// the grid query of an explore route (None: not an explore grid / not resolved yet)
pub(crate) fn grid_query(app: &AppInner) -> Option<BrowseQuery> {
    let ex = get()?;
    match &*app.route.borrow() {
        Route::ExploreResult(kind, key) => ex.resolve(kind, key).query,
        // the duplicates page keeps the selection in the shared grid (selection bar, actions)
        Route::ExploreDuplicates => Some(BrowseQuery {
            dup: Some(true),
            ..lb::explore_query()
        }),
        _ => None,
    }
}

/// a route was entered (after the shell set the page)
pub(crate) fn enter(app: &Rc<AppInner>, route: &Route) {
    let Some(ex) = get() else { return };
    crate::explore_player::close();
    // pages without a grid: a selection made on the duplicates page ends with it
    if matches!(
        route,
        Route::Explore | Route::ExplorePlaces | Route::ExploreMap { .. }
    ) {
        let mut g = app.grid.0.borrow_mut();
        if !g.ix.selection.is_empty() {
            g.ix.selection.clear();
            g.selection_changed();
            g.notify_listeners();
        }
    }
    match route {
        Route::Explore => {
            ex.publish_home();
            if ex.data.borrow().is_none() || !ex.loading.get() {
                ex.load();
            }
        }
        Route::ExploreResult(..) => ex.publish_result(app, route),
        Route::ExplorePlaces => ex.publish_places(),
        Route::ExploreDuplicates => {
            ex.dups.borrow_mut().limit = DUP_PAGE;
            ex.publish_dups();
            ex.load_dups();
        }
        _ => {}
    }
    crate::map::enter(app, route);
}

/// live events
pub(crate) fn on_event(app: &Rc<AppInner>, e: &BackendEvent) {
    let Some(ex) = get() else { return };
    let BackendEvent::Server(ev) = e else { return };
    use pa_core::ServerEvent as E;
    match ev {
        E::Explore | E::Folders { status: None } => ex.schedule_refresh(),
        E::Media {
            reason: Some(pa_core::MediaReason::Add | pa_core::MediaReason::Remove),
            ..
        } => {
            if matches!(&*app.route.borrow(), Route::ExploreDuplicates) {
                ex.load_dups();
            }
        }
        _ => {}
    }
    crate::map::on_event(app, ev);
}

/// preferences changed (language, week start, theme)
pub(crate) fn on_prefs(app: &Rc<AppInner>) {
    let Some(ex) = get() else { return };
    let lang = app.lang();
    let ws = app.prefs.borrow().week_start.clone();
    let changed = ex.lang.get() != lang || *ex.week_start.borrow() != ws;
    ex.lang.set(lang);
    *ex.week_start.borrow_mut() = ws;
    if changed {
        ex.publish_home();
        let route = app.route.borrow().clone();
        match &route {
            Route::ExploreResult(..) => ex.publish_result(app, &route),
            Route::ExplorePlaces => ex.publish_places(),
            Route::ExploreDuplicates => ex.publish_dups(),
            _ => {}
        }
    }
    crate::map::on_prefs(app);
}

/// automation steps: `explore-open <key>`, `play <memory id | first>`, `player <next|prev|pause|close>`
pub(crate) fn script_step(app: &Rc<AppInner>, cmd: &[String]) -> bool {
    let arg = cmd.get(1).cloned().unwrap_or_default();
    match cmd[0].as_str() {
        "explore-open" => {
            if let Some(e) = get() {
                e.open(&arg);
            }
        }
        "play" => {
            let id = if arg == "first" || arg.is_empty() {
                get().and_then(|e| {
                    e.data.borrow().as_ref().and_then(|r| {
                        r.memories
                            .first()
                            .or(r.extras.year_reviews.iter().flatten().next())
                            .map(|m| m.id.clone())
                    })
                })
            } else {
                Some(arg)
            };
            match id {
                Some(id) => crate::explore_player::open_id(app, &id),
                None => eprintln!("[script] no memory to play"),
            }
        }
        "player" => crate::explore_player::script(app, &arg),
        _ => return crate::map::script_step(app, cmd),
    }
    true
}

/// tests reach the label helpers through here
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dots_in_box() {
        let p = |lat, lon, count| pa_core::PlaceGroup {
            key: "TR-34".into(),
            cc: "TR".into(),
            name: "İstanbul".into(),
            city: None,
            count,
            cover_id: "1".into(),
            lat,
            lon,
        };
        let d = map_dots(&[p(41.0, 29.0, 10), p(37.0, 27.0, 1)]);
        assert_eq!(d.len(), 2);
        assert!(d
            .iter()
            .all(|d| (0.0..=1.0).contains(&d.x) && (0.0..=1.0).contains(&d.y)));
        // north is up
        assert!(d[0].y < d[1].y);
        assert!((d[0].r - 14.0).abs() < 1e-4);
        assert!(text_w("Canon EOS R6", 14.0) > 60.0);
    }
}
