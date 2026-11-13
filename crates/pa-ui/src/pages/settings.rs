//! Settings page (SettingsPage.tsx, GeneralSections.tsx, SystemSections.tsx, DesktopSection.tsx,
//! Shortcuts.tsx, ExploreSettings, MetaDiagnostics, DevicesCard, MegaAccountCard, backup.ts):
//! every row is data built here; the search (both languages + keywords) and the scroll-spy work
//! on that data. Device prefs → `AppInner::set_pref`, server settings → `PATCH /api/settings`,
//! desktop shell options → [`crate::host::Host`].

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use pa_core::{DeviceInfo, MegaAccountStatus, MetaStatus, ServerInfo, Settings};
use slint::{ComponentHandle, Image, ModelRc, SharedString, VecModel};

use crate::api;
use crate::app::AppInner;
use crate::fmt;
use crate::host::{LanState, ShellPatch, ShellState};
use crate::i18n::{self, Lang};
use crate::{PIcons, SegOption, SetCard, SetNav, SetRow, SetSection, SetState, Shell, ToastKind};

/// settings page state
#[derive(Default)]
pub(crate) struct SettingsCtl {
    /// MEGA account logged in (banners offer "log in" otherwise)
    pub mega_logged_in: Cell<bool>,
    /// server zipAction ("ask" | "extract" | "temp")
    pub zip_action: RefCell<String>,
    info: RefCell<Option<ServerInfo>>,
    server: RefCell<Option<Settings>>,
    cache: RefCell<Option<serde_json::Value>>,
    tiles: RefCell<Option<serde_json::Value>>,
    meta: RefCell<Option<MetaStatus>>,
    devices: RefCell<Vec<DeviceInfo>>,
    mega: RefCell<Option<MegaAccountStatus>>,
    mega_mfa: Cell<bool>,
    mega_error: RefCell<String>,
    lan: RefCell<Option<LanState>>,
    lan_port: RefCell<String>,
    shell: RefCell<Option<ShellState>>,
    gpu_start: Cell<Option<bool>>,
    busy: RefCell<HashSet<String>>,
    /// section y positions (layout) for jump / scroll-spy
    ys: RefCell<Vec<(String, f32)>>,
    jumping: Cell<i64>,
    loaded: Cell<bool>,
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

struct Ctx<'a> {
    a: &'a AppInner,
    l: Lang,
    ic: PIcons<'a>,
}

impl Ctx<'_> {
    fn t(&self, k: &str) -> String {
        i18n::t(self.l, k)
    }
    fn tv(&self, k: &str, v: &[(&str, &str)]) -> String {
        i18n::tv(self.l, k, v)
    }
    fn opts(&self, list: &[(&str, &str)]) -> ModelRc<SegOption> {
        model(
            list.iter()
                .map(|(v, k)| SegOption {
                    value: (*v).into(),
                    label: self.t(k).into(),
                    ..Default::default()
                })
                .collect(),
        )
    }
    fn raw_opts(&self, list: &[(String, String)]) -> ModelRc<SegOption> {
        model(
            list.iter()
                .map(|(v, l)| SegOption {
                    value: v.into(),
                    label: l.into(),
                    ..Default::default()
                })
                .collect(),
        )
    }
}

/// a row + its search text
struct R(SetRow, String);

fn both(keys: &[&str]) -> String {
    keys.iter()
        .map(|k| format!("{} {}", i18n::t(Lang::Tr, k), i18n::t(Lang::En, k)))
        .collect::<Vec<_>>()
        .join(" ")
}

fn row(kind: &str, key: &str, label: String, hint: String) -> SetRow {
    SetRow {
        kind: kind.into(),
        key: key.into(),
        label: label.into(),
        hint: hint.into(),
        enabled: true,
        ..Default::default()
    }
}

impl Ctx<'_> {
    fn switch(&self, k: &str, hint: Option<&str>, pref: &str, on: bool, kw: &str) -> R {
        let mut r = row(
            "switch",
            pref,
            self.t(k),
            hint.map(|h| self.t(h)).unwrap_or_default(),
        );
        r.checked = on;
        R(r, format!("{} {kw}", both(&[k])))
    }
    fn seg(
        &self,
        k: &str,
        hint: String,
        key: &str,
        value: &str,
        opts: &[(&str, &str)],
        kw: &str,
    ) -> R {
        let mut r = row("seg", key, self.t(k), hint);
        r.value = value.into();
        r.options = self.opts(opts);
        R(r, format!("{} {kw}", both(&[k])))
    }
    fn select(
        &self,
        k: &str,
        hint: String,
        key: &str,
        value: &str,
        opts: Vec<(String, String)>,
        kw: &str,
    ) -> R {
        let mut r = row("select", key, self.t(k), hint);
        r.value = value.into();
        r.sel_options = model(
            opts.iter()
                .map(|(v, l)| crate::SelectOption {
                    value: v.into(),
                    label: l.into(),
                })
                .collect(),
        );
        R(r, format!("{} {kw}", both(&[k])))
    }
    fn button(&self, k: &str, hint: String, key: &str, label: &str, icon: Image, kw: &str) -> R {
        let mut r = row("buttons", key, self.t(k), hint);
        r.b1_key = key.into();
        r.b1_label = self.t(label).into();
        r.b1_icon = icon;
        r.b1_busy = self.a.pages.settings.busy.borrow().contains(key);
        R(r, format!("{} {kw}", both(&[k])))
    }
}

struct Card {
    id: &'static str,
    title_key: String,
    title: String,
    icon: Image,
    scope: &'static str,
    rows: Vec<R>,
}

struct Sec {
    id: &'static str,
    title: &'static str,
    icon: Image,
    cards: Vec<Card>,
}

/// Settings → Network of the Android app: how it is connected, and phone folder sharing with
/// the connected PC (1.x `AndroidConnectionCard`, `PhoneShareCard`)
fn android_cards(c: &Ctx, an: &crate::host::AndroidState, cards: &mut Vec<Card>) {
    let ic = &c.ic;
    let mut r = row(
        "buttons",
        "an:change",
        match &an.server {
            Some(u) => c.tv("android.modeServer", &[("url", u)]),
            None => c.t("android.modeLocal"),
        },
        String::new(),
    );
    r.b1_key = "an:change".into();
    r.b1_label = c.t("android.change").into();
    r.b1_icon = ic.get_refresh_cw();
    cards.push(Card {
        id: "android-mode",
        title_key: "android.mode".into(),
        title: c.t("android.mode"),
        icon: ic.get_smartphone(),
        scope: "",
        rows: vec![R(r, format!("{} android phone telefon", both(&["android.mode"])))],
    });
    if an.server.is_none() {
        return;
    }
    let kw = format!("{} phone telefon share paylaş", both(&["devices.share.title"]));
    let mut rows = vec![R(
        row("note", "", c.t("devices.share.hint"), String::new()),
        kw.clone(),
    )];
    let state = match (an.share_state.as_str(), an.share_error.as_deref()) {
        _ if !an.share_on && an.share_state != "rejected" && an.share_error.as_deref() != Some("forgotten") => {
            c.t("devices.share.state.off")
        }
        ("online", _) => {
            let mut t = c.tv("devices.share.state.online", &[("pc", &an.pc_name)]);
            if an.share_active > 0 {
                t.push_str(" · ");
                t.push_str(&c.tv("devices.share.active", &[("n", &an.share_active.to_string())]));
            }
            t
        }
        ("pending", _) => format!(
            "{}: {}",
            c.t("devices.share.approveOnPc"),
            an.share_code.clone().unwrap_or_default()
        ),
        ("rejected", _) => c.t("devices.share.state.rejected"),
        (_, Some("forgotten")) => c.t("devices.share.state.forgotten"),
        ("error", Some(e)) => c.tv("devices.share.state.error", &[("error", e)]),
        (_, Some(e)) => c.tv("devices.share.state.retrying", &[("error", e)]),
        _ => c.t("devices.share.state.connecting"),
    };
    let mut r = row("switch", "an:share", c.t("devices.share.title"), state);
    r.checked = an.share_on;
    r.enabled = an.folders.iter().any(|f| f.2) || an.share_on;
    rows.push(R(r, kw.clone()));
    if an.share_on && an.share_state == "pending" {
        rows.push(R(
            row("note", "", c.t("devices.share.approveHint"), String::new()),
            kw.clone(),
        ));
    }
    rows.push(R(
        row("note", "", c.t("devices.share.folders"), String::new()),
        kw.clone(),
    ));
    for (name, path, on) in &an.folders {
        let mut r = row("switch", &format!("an:folder:{path}"), name.clone(), path.clone());
        r.checked = *on;
        rows.push(R(r, format!("{kw} {name}")));
    }
    rows.push(R(
        {
            let mut r = row("buttons", "an:reset", c.t("devices.share.reset"), String::new());
            r.b1_key = "an:reset".into();
            r.b1_label = c.t("devices.share.reset").into();
            r
        },
        kw,
    ));
    cards.push(Card {
        id: "phone-share",
        title_key: "devices.share.title".into(),
        title: c.t("devices.share.title"),
        icon: ic.get_smartphone(),
        scope: "",
        rows,
    });
}

fn num_label(v: f64) -> String {
    let s = format!("{v}");
    s.trim_end_matches(".0").to_string()
}

const ZONES: [&str; 40] = [
    "UTC",
    "Europe/Istanbul",
    "Europe/London",
    "Europe/Dublin",
    "Europe/Lisbon",
    "Europe/Paris",
    "Europe/Berlin",
    "Europe/Amsterdam",
    "Europe/Madrid",
    "Europe/Rome",
    "Europe/Vienna",
    "Europe/Zurich",
    "Europe/Stockholm",
    "Europe/Warsaw",
    "Europe/Athens",
    "Europe/Kiev",
    "Europe/Moscow",
    "Asia/Dubai",
    "Asia/Tehran",
    "Asia/Karachi",
    "Asia/Kolkata",
    "Asia/Bangkok",
    "Asia/Shanghai",
    "Asia/Singapore",
    "Asia/Tokyo",
    "Asia/Seoul",
    "Australia/Sydney",
    "Pacific/Auckland",
    "Africa/Cairo",
    "Africa/Johannesburg",
    "Africa/Lagos",
    "America/Sao_Paulo",
    "America/Argentina/Buenos_Aires",
    "America/Mexico_City",
    "America/New_York",
    "America/Chicago",
    "America/Denver",
    "America/Los_Angeles",
    "America/Anchorage",
    "Pacific/Honolulu",
];

fn build(a: &Rc<AppInner>) -> Vec<Sec> {
    let st = &a.pages.settings;
    let c = Ctx {
        a,
        l: a.lang(),
        ic: a.window.global::<PIcons>(),
    };
    let ic = &c.ic;
    let p = a.prefs.borrow().clone();
    let ro = a.window.global::<Shell>().get_readonly();
    let srv = st.server.borrow().clone();
    let info = st.info.borrow().clone();
    let shell = st.shell.borrow().clone();
    let desktop = a.host.is_desktop();
    let mut secs = vec![];

    // ── appearance ──
    let mut theme = vec![];
    let mut r = row("seg", "lang", c.t("settings.language"), String::new());
    r.value = p.lang.clone().into();
    r.options = c.raw_opts(&[
        ("tr".into(), "Türkçe".into()),
        ("en".into(), "English".into()),
    ]);
    theme.push(R(
        r,
        format!(
            "{} language dil türkçe english",
            both(&["settings.language"])
        ),
    ));
    theme.push(c.seg(
        "settings.theme",
        String::new(),
        "theme",
        &p.theme,
        &[
            ("dark", "settings.theme.dark"),
            ("light", "settings.theme.light"),
            ("system", "settings.theme.system"),
        ],
        "dark light karanlık",
    ));
    theme.push(R(
        row("accent", "accent", c.t("settings.accent"), String::new()),
        format!("{} color renk", both(&["settings.accent"])),
    ));
    theme.push(c.seg(
        "settings.motion",
        String::new(),
        "motion",
        &p.motion,
        &[
            ("full", "settings.motion.full"),
            ("reduced", "settings.motion.reduced"),
            ("off", "settings.motion.off"),
        ],
        "animation reduce",
    ));
    theme.push(c.switch(
        "pref.wideSidebar",
        Some("pref.wideSidebarHint"),
        "wideSidebar",
        p.wide_sidebar,
        "sidebar kenar çubuğu rail menu panel sol left",
    ));
    let ps = a.window.global::<crate::PrefsState>();
    let format = vec![
        c.seg(
            "pref.dateFormat",
            c.tv("pref.preview", &[("date", &ps.get_date_preview())]),
            "dateFormat",
            &p.date_format,
            &[
                ("locale", "pref.dateFormat.locale"),
                ("iso", "pref.dateFormat.iso"),
            ],
            "iso tarih date",
        ),
        c.seg(
            "pref.hourCycle",
            c.tv("pref.preview", &[("date", &ps.get_time_preview())]),
            "hourCycle",
            &p.hour_cycle,
            &[
                ("auto", "pref.hourCycle.auto"),
                ("h23", "pref.hourCycle.h23"),
                ("h12", "pref.hourCycle.h12"),
            ],
            "am pm saat clock",
        ),
        c.seg(
            "pref.weekStart",
            c.t("pref.weekStartHint"),
            "weekStart",
            &p.week_start,
            &[
                ("mon", "pref.weekStart.mon"),
                ("sun", "pref.weekStart.sun"),
                ("sat", "pref.weekStart.sat"),
            ],
            "calendar takvim hafta week",
        ),
    ];
    secs.push(Sec {
        id: "appearance",
        title: "pref.sec.appearance",
        icon: ic.get_palette(),
        cards: vec![
            Card {
                id: "theme",
                title_key: "pref.themeCard".into(),
                title: c.t("pref.themeCard"),
                icon: ic.get_palette(),
                scope: "device",
                rows: theme,
            },
            Card {
                id: "format",
                title_key: "pref.format".into(),
                title: c.t("pref.format"),
                icon: ic.get_calendar_days(),
                scope: "device",
                rows: format,
            },
        ],
    });

    // ── gallery ──
    let sorts: Vec<(String, String)> = ["takenAt", "mtime", "name", "size", "kind"]
        .iter()
        .map(|s| (s.to_string(), c.t(&format!("pref.sort.{s}"))))
        .collect();
    let mut grid = vec![
        c.select(
            "pref.defaultSort",
            String::new(),
            "sort",
            &p.sort,
            sorts,
            "sort sırala order",
        ),
        c.seg(
            "gallery.sort",
            String::new(),
            "order",
            &p.order,
            &[("desc", "pref.order.desc"), ("asc", "pref.order.asc")],
            "order sıra",
        ),
        c.seg(
            "pref.gridMode",
            String::new(),
            "gridMode",
            &p.grid_mode,
            &[
                ("justified", "pref.gridMode.justified"),
                ("square", "pref.gridMode.square"),
            ],
            "justified square kare layout",
        ),
    ];
    let mut rh = row(
        "slider",
        "rowHeight",
        c.t("pref.rowHeight"),
        c.tv(
            "pref.rowHeightHint",
            &[("px", &(p.row_height as i64).to_string())],
        ),
    );
    rh.num = p.row_height as f32;
    rh.min = 110.0;
    rh.max = 420.0;
    rh.step = 10.0;
    rh.unit = " px".into();
    grid.push(R(
        rh,
        format!("{} size row height boyut", both(&["pref.rowHeight"])),
    ));
    grid.push(c.switch(
        "pref.dateHeaders",
        Some("pref.dateHeadersHint"),
        "dateHeaders",
        p.date_headers,
        "header başlık",
    ));
    grid.push(c.seg(
        "pref.groupBy",
        String::new(),
        "groupBy",
        &p.group_by,
        &[
            ("auto", "pref.groupBy.auto"),
            ("day", "pref.groupBy.day"),
            ("month", "pref.groupBy.month"),
        ],
        "group day month gün ay",
    ));
    let tiles = vec![
        c.switch(
            "pref.tileNames",
            None,
            "tileNames",
            p.tile_names,
            "filename caption",
        ),
        c.switch(
            "pref.tileDuration",
            None,
            "tileDuration",
            p.tile_duration,
            "badge rozet",
        ),
        c.switch(
            "pref.tileFavorites",
            None,
            "tileFavorites",
            p.tile_favorites,
            "heart favorite",
        ),
        c.switch(
            "settings.hoverPreview",
            None,
            "hoverPreview",
            p.hover_preview,
            "hover preview önizleme",
        ),
        c.switch(
            "pref.confirmDelete",
            Some("pref.confirmDeleteHint"),
            "confirmDelete",
            p.confirm_delete,
            "trash çöp delete sil",
        ),
    ];
    secs.push(Sec {
        id: "gallery",
        title: "pref.sec.gallery",
        icon: ic.get_layout_grid(),
        cards: vec![
            Card {
                id: "grid",
                title_key: "pref.grid".into(),
                title: c.t("pref.grid"),
                icon: ic.get_grid_3x3(),
                scope: "device",
                rows: grid,
            },
            Card {
                id: "tiles",
                title_key: "pref.tiles".into(),
                title: c.t("pref.tiles"),
                icon: ic.get_image(),
                scope: "device",
                rows: tiles,
            },
        ],
    });

    // ── viewer & video ──
    let viewer = vec![
        c.seg(
            "pref.viewerBg",
            String::new(),
            "viewerBg",
            &p.viewer_bg,
            &[
                ("blur", "pref.viewerBg.blur"),
                ("black", "pref.viewerBg.black"),
                ("tint", "pref.viewerBg.tint"),
            ],
            "background black blur",
        ),
        c.switch(
            "pref.filmstrip",
            None,
            "filmstrip",
            p.filmstrip,
            "filmstrip strip şerit",
        ),
        c.seg(
            "pref.infoDefault",
            String::new(),
            "infoDefault",
            &p.info_default,
            &[
                ("remember", "pref.infoDefault.remember"),
                ("open", "pref.infoDefault.open"),
                ("closed", "pref.infoDefault.closed"),
            ],
            "info details bilgi",
        ),
        c.select(
            "pref.zoomStep",
            c.t("pref.zoomStepHint"),
            "zoomStep",
            &num_label(p.zoom_step),
            [1.2, 1.4, 1.6, 2.0]
                .iter()
                .map(|v| (num_label(*v), format!("×{}", num_label(*v))))
                .collect(),
            "zoom",
        ),
        c.select(
            "pref.dblZoom",
            String::new(),
            "dblZoom",
            &num_label(p.dbl_zoom),
            [2.0, 2.5, 3.0, 4.0]
                .iter()
                .map(|v| (num_label(*v), format!("{}%", (v * 100.0) as i64)))
                .collect(),
            "double tap zoom",
        ),
    ];
    let video = vec![
        c.switch(
            "pref.videoAutoplay",
            None,
            "videoAutoplay",
            p.video_autoplay,
            "autoplay",
        ),
        c.switch(
            "pref.videoMuted",
            None,
            "videoMuted",
            p.video_muted,
            "mute ses sound",
        ),
        c.switch(
            "pref.rememberVolume",
            None,
            "rememberVolume",
            p.remember_volume,
            "volume ses",
        ),
        c.select(
            "pref.loopShort",
            c.t("pref.loopShortHint"),
            "loopShortSec",
            &num_label(p.loop_short_sec),
            [0.0, 5.0, 10.0, 15.0, 30.0, 60.0]
                .iter()
                .map(|n| {
                    (
                        num_label(*n),
                        if *n == 0.0 {
                            c.t("pref.loopShort.off")
                        } else {
                            c.tv("pref.loopShort.sec", &[("n", &num_label(*n))])
                        },
                    )
                })
                .collect(),
            "loop repeat tekrar",
        ),
        c.select(
            "pref.videoSpeed",
            String::new(),
            "videoSpeed",
            &num_label(p.video_speed),
            [0.5, 0.75, 1.0, 1.25, 1.5, 2.0]
                .iter()
                .map(|v| (num_label(*v), format!("{}×", num_label(*v))))
                .collect(),
            "speed playback rate hız",
        ),
    ];
    let mut durs: Vec<f64> = vec![2.0, 3.0, 4.0, 5.0, 7.0, 10.0, 15.0];
    if !durs.contains(&p.slide_duration) {
        durs.push(p.slide_duration);
        durs.sort_by(|a, b| a.total_cmp(b));
    }
    let slides = vec![
        c.select(
            "pref.slideDuration",
            String::new(),
            "slideDuration",
            &num_label(p.slide_duration),
            durs.iter()
                .map(|n| {
                    (
                        num_label(*n),
                        c.tv("pref.slideDurationValue", &[("n", &num_label(*n))]),
                    )
                })
                .collect(),
            "slideshow interval süre",
        ),
        c.select(
            "pref.slideTransition",
            String::new(),
            "slideTransition",
            &p.slide_transition,
            [
                "fade", "slide", "zoom", "kenburns", "blur", "flip", "circle", "random", "none",
            ]
            .iter()
            .map(|v| (v.to_string(), c.t(&format!("slide.tr.{v}"))))
            .collect(),
            "transition geçiş",
        ),
    ];
    secs.push(Sec {
        id: "viewer",
        title: "pref.sec.viewer",
        icon: ic.get_eye(),
        cards: vec![
            Card {
                id: "viewer",
                title_key: "pref.viewer".into(),
                title: c.t("pref.viewer"),
                icon: ic.get_eye(),
                scope: "device",
                rows: viewer,
            },
            Card {
                id: "video",
                title_key: "pref.video".into(),
                title: c.t("pref.video"),
                icon: ic.get_film(),
                scope: "device",
                rows: video,
            },
            Card {
                id: "slideshow",
                title_key: "pref.slideshow".into(),
                title: c.t("pref.slideshow"),
                icon: ic.get_presentation(),
                scope: "device",
                rows: slides,
            },
        ],
    });

    // ── explore & map ──
    let mut explore_cards = vec![Card {
        id: "memories",
        title_key: "explore.memories".into(),
        title: c.t("explore.memories"),
        icon: ic.get_clapperboard(),
        scope: "device",
        rows: vec![c.switch(
            "pref.memories",
            Some("pref.memoriesHint"),
            "showMemories",
            p.show_memories,
            "memories anılar on this day",
        )],
    }];
    if let Some(s) = &srv {
        let kw = "map harita tile time zone saat dilimi osm satellite uydu";
        let style = match s.map_tiles {
            pa_core::MapTilesSetting::Topo => "topo",
            pa_core::MapTilesSetting::Satellite => "satellite",
            pa_core::MapTilesSetting::Off => "off",
            _ => "osm",
        };
        let mut rows = vec![c.seg(
            "map.settings.style",
            c.t("map.settings.styleHint"),
            "s:mapTiles",
            style,
            &[
                ("osm", "map.style.osm"),
                ("topo", "map.style.topo"),
                ("satellite", "map.style.satellite"),
                ("off", "map.style.off"),
            ],
            kw,
        )];
        let tc = st.tiles.borrow().clone();
        let hint = tc
            .as_ref()
            .map(|t| {
                c.tv(
                    "map.settings.cacheUsage",
                    &[
                        ("size", &fmt::format_bytes(t["bytes"].as_i64().unwrap_or(0))),
                        ("limit", &fmt::format_bytes(s.tile_cache_mb * 1024 * 1024)),
                        ("n", &t["files"].as_i64().unwrap_or(0).to_string()),
                    ],
                )
            })
            .unwrap_or_default();
        let mut tr = c.select(
            "map.settings.cache",
            hint,
            "s:tileCacheMb",
            &s.tile_cache_mb.to_string(),
            [128, 256, 512, 1024, 2048, 4096]
                .iter()
                .map(|mb| {
                    (
                        mb.to_string(),
                        if *mb >= 1024 {
                            format!("{} GB", mb / 1024)
                        } else {
                            format!("{mb} MB")
                        },
                    )
                })
                .collect(),
            kw,
        );
        tr.0.b1_key = "clear-tiles".into();
        tr.0.b1_label = c.t("map.settings.clear").into();
        tr.0.b1_icon = ic.get_trash_2();
        tr.0.b1_busy = st.busy.borrow().contains("clear-tiles");
        tr.0.enabled = !ro;
        rows.push(tr);
        let mut zones: Vec<String> = ZONES.iter().map(|z| z.to_string()).collect();
        let tz = if s.home_time_zone.is_empty() {
            "UTC".to_string()
        } else {
            s.home_time_zone.clone()
        };
        if !zones.contains(&tz) {
            zones.insert(0, tz.clone());
        }
        rows.push(
            c.select(
                "explore.settings.homeTz",
                c.t("explore.settings.homeTzHint"),
                "s:homeTimeZone",
                &tz,
                zones
                    .iter()
                    .map(|z| (z.clone(), z.replace('_', " ")))
                    .collect(),
                kw,
            ),
        );
        explore_cards.push(Card {
            id: "explore",
            title_key: "explore.settings.title".into(),
            title: c.t("explore.settings.title"),
            icon: ic.get_map_pin(),
            scope: "server",
            rows,
        });
    }
    if let Some(m) = st.meta.borrow().as_ref() {
        let mut rows = vec![];
        let mut intro = row(
            "note",
            "",
            format!("{} {}", c.t("meta.intro"), c.t("meta.takeoutHint")),
            String::new(),
        );
        intro.kind = "note".into();
        rows.push(R(intro, "gps metadata exif".into()));
        for l in &m.libraries {
            let mut r = row("meta-lib", "", l.name.clone(), String::new());
            r.sub = format!(
                "{} · {} · {}",
                c.tv("meta.withLocation", &[("n", &l.with_location.to_string())]),
                c.tv(
                    "meta.withoutLocation",
                    &[("n", &l.without_location.to_string())]
                ),
                c.tv("meta.pending", &[("n", &l.pending.to_string())])
            )
            .into();
            r.b1_key = format!("reread:{}", l.id).into();
            r.b1_label = c.t("meta.reread").into();
            r.b1_icon = ic.get_refresh_cw();
            r.b1_busy = st.busy.borrow().contains(&format!("reread:{}", l.id));
            r.enabled = !ro;
            rows.push(R(r, format!("{} gps metadata", l.name)));
        }
        if m.libraries.is_empty() {
            rows.push(R(
                row("note", "", c.t("meta.empty"), String::new()),
                String::new(),
            ));
        }
        let mut all = c.button(
            "meta.rereadAll",
            if m.enriching {
                c.tv(
                    "meta.progress",
                    &[
                        ("done", &m.indexing.done.max(0).to_string()),
                        ("total", &m.indexing.total.to_string()),
                    ],
                )
            } else {
                c.t("meta.idle")
            },
            "reread:*",
            "meta.rereadAll",
            ic.get_refresh_cw(),
            "gps reread",
        );
        all.0.enabled = !ro;
        rows.push(all);
        explore_cards.push(Card {
            id: "meta",
            title_key: "meta.title".into(),
            title: c.t("meta.title"),
            icon: ic.get_map_pin_off(),
            scope: "server",
            rows,
        });
    }
    secs.push(Sec {
        id: "explore",
        title: "pref.sec.explore",
        icon: ic.get_sparkles(),
        cards: explore_cards,
    });

    // ── libraries & files ──
    let mut lib_rows = vec![];
    if let Some(l) = a.pages.libs.borrow().as_ref() {
        for x in &l.libraries {
            let mut r = row(
                "lib",
                &format!("edit-lib:{}", x.id),
                x.name.clone(),
                String::new(),
            );
            r.color = crate::theme::color(&x.color);
            r.icon = x.icon.clone().into();
            r.sub = x
                .folders
                .iter()
                .map(|f| f.path.clone())
                .collect::<Vec<_>>()
                .join(" · ")
                .into();
            r.badge = x.media_count.to_string().into();
            r.enabled = !ro;
            lib_rows.push(R(
                r,
                format!(
                    "{} {} {}",
                    x.name,
                    r_paths(x),
                    both(&["settings.libraries"])
                ),
            ));
        }
    }
    if !ro {
        let mut add = c.button(
            "nav.addLibrary",
            String::new(),
            "add-lib",
            "nav.addLibrary",
            ic.get_plus(),
            "folder klasör",
        );
        add.0.label = "".into();
        lib_rows.push(add);
    }
    let mut lib_cards = vec![Card {
        id: "libraries",
        title_key: "settings.libraries".into(),
        title: c.t("settings.libraries"),
        icon: ic.get_folder(),
        scope: "",
        rows: lib_rows,
    }];
    if let Some(s) = srv.as_ref().filter(|_| !ro) {
        let za = serde_json::to_value(s.zip_action)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        let mut sfx = row(
            "text",
            "s:editSuffix",
            c.t("settings.editSuffix"),
            c.tv(
                "settings.editSuffixHint",
                &[(
                    "suffix",
                    if s.edit_suffix.is_empty() {
                        "_edit"
                    } else {
                        &s.edit_suffix
                    },
                )],
            ),
        );
        sfx.value = s.edit_suffix.clone().into();
        let mut hours = row(
            "number",
            "s:tempSessionHours",
            c.t("settings.tempHours"),
            String::new(),
        );
        hours.value = s.temp_session_hours.to_string().into();
        let rows = vec![
            c.seg(
                "settings.zipAction",
                String::new(),
                "s:zipAction",
                &za,
                &[
                    ("ask", "settings.zip.ask"),
                    ("extract", "settings.zip.extract"),
                    ("temp", "settings.zip.temp"),
                ],
                "zip archive arşiv",
            ),
            R(
                sfx,
                format!("{} edit copy kopya", both(&["settings.editSuffix"])),
            ),
            R(
                hours,
                format!("{} temporary geçici", both(&["settings.tempHours"])),
            ),
            c.switch(
                "settings.showHidden",
                None,
                "s:showHidden",
                s.show_hidden,
                "hidden dotfiles gizli",
            ),
        ];
        lib_cards.push(Card {
            id: "files",
            title_key: "settings.files".into(),
            title: c.t("settings.files"),
            icon: ic.get_archive(),
            scope: "server",
            rows,
        });
    }
    secs.push(Sec {
        id: "libraries",
        title: "pref.sec.libraries",
        icon: ic.get_folder(),
        cards: lib_cards,
    });

    // ── performance & cache ──
    let mut perf_cards = vec![];
    if let Some(s) = srv.as_ref().filter(|_| !ro) {
        let q = |v: pa_core::ThumbQuality| {
            serde_json::to_value(v)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default()
        };
        let levels = [
            ("low", "pref.q.low"),
            ("normal", "pref.q.normal"),
            ("high", "pref.q.high"),
        ];
        let mut rows = vec![
            c.seg(
                "pref.thumbQuality",
                c.t("pref.thumbQualityHint"),
                "s:thumbQuality",
                &q(s.thumb_quality),
                &levels,
                "webp jpeg thumbnail",
            ),
            c.seg(
                "pref.intensity",
                c.t("pref.intensityHint"),
                "s:indexIntensity",
                &q(s.index_intensity),
                &levels,
                "cpu index scan tarama fan",
            ),
        ];
        if info.as_ref().is_some_and(|i| i.caps.video_jobs) {
            rows.push(c.switch(
                "pref.hwAccel",
                Some("pref.hwAccelHint"),
                "s:hwAccel",
                s.hw_accel,
                "gpu ffmpeg transcode nvenc vaapi",
            ));
        }
        perf_cards.push(Card {
            id: "processing",
            title_key: "pref.processing".into(),
            title: c.t("pref.processing"),
            icon: ic.get_cpu(),
            scope: "server",
            rows,
        });
    }
    let mut cache_rows = vec![];
    if let Some(d) = st.cache.borrow().as_ref() {
        let b = |k: &str| d[k]["bytes"].as_i64().unwrap_or(0);
        let mut r = c.button(
            "pref.cache.thumbs",
            format!(
                "{} · {}",
                fmt::format_bytes(b("thumbs")),
                c.t("pref.cache.thumbsHint")
            ),
            "clear:thumbs",
            "pref.cache.clear",
            ic.get_trash_2(),
            "cache önbellek thumbnail",
        );
        r.0.enabled = !ro;
        cache_rows.push(r);
        let limit = srv
            .as_ref()
            .map(|s| s.tile_cache_mb)
            .or(d["tiles"]["limitMb"].as_i64())
            .unwrap_or(512);
        let mut r = c.button(
            "pref.cache.tiles",
            format!(
                "{} · {}",
                fmt::format_bytes(b("tiles")),
                c.tv(
                    "pref.cache.limit",
                    &[("size", &fmt::format_bytes(limit * 1024 * 1024))]
                )
            ),
            "clear:tiles",
            "pref.cache.clear",
            ic.get_trash_2(),
            "cache map harita",
        );
        r.0.enabled = !ro;
        cache_rows.push(r);
        if let Some(s) = &srv {
            let cur = s.remote_cache_mb;
            let mut sizes: Vec<i64> = vec![512, 1024, 2048, 4096, 8192, 16384];
            if !sizes.contains(&cur) {
                sizes.push(cur);
                sizes.sort();
            }
            cache_rows.push(
                c.select(
                    "mega.cache.title",
                    c.t("mega.cache.hint"),
                    "s:remoteCacheMb",
                    &cur.to_string(),
                    sizes
                        .iter()
                        .map(|mb| {
                            (
                                mb.to_string(),
                                if *mb >= 1024 {
                                    format!("{} GB", mb / 1024)
                                } else {
                                    format!("{mb} MB")
                                },
                            )
                        })
                        .collect(),
                    "mega cache önbellek",
                ),
            );
        }
        if b("remote") > 0 {
            let mut r = c.button(
                "pref.cache.remote",
                fmt::format_bytes(b("remote")),
                "clear:remote",
                "pref.cache.clear",
                ic.get_trash_2(),
                "mega cache",
            );
            r.0.enabled = !ro;
            cache_rows.push(r);
        }
    }
    perf_cards.push(Card {
        id: "cache",
        title_key: "pref.cacheUsage".into(),
        title: c.t("pref.cacheUsage"),
        icon: ic.get_hard_drive(),
        scope: "server",
        rows: cache_rows,
    });
    secs.push(Sec {
        id: "performance",
        title: "pref.sec.performance",
        icon: ic.get_gauge(),
        cards: perf_cards,
    });

    // ── network & devices ──
    let mut net_cards = vec![];
    if let Some(an) = a.host.android() {
        android_cards(&c, &an, &mut net_cards);
    }
    let lan = st.lan.borrow().clone();
    let urls: Vec<String> = match (&lan, &info) {
        (Some(l), _) => l.urls.clone(),
        (None, Some(i)) if i.mode == pa_core::RunMode::Server => i.lan.urls.clone(),
        _ => vec![],
    };
    if lan.is_some() || !urls.is_empty() {
        let mut rows = vec![];
        if let Some(l) = &lan {
            let mut r = row(
                "lan",
                "lan",
                c.t("settings.lanShare"),
                c.t("settings.lanShareHint"),
            );
            r.checked = l.enabled;
            r.value = st.lan_port.borrow().clone().into();
            r.enabled = !st.busy.borrow().contains("lan");
            rows.push(R(
                r,
                format!("{} lan wifi port qr", both(&["settings.lanShare"])),
            ));
        }
        if !urls.is_empty() {
            let mut r = row(
                "lan-urls",
                "",
                c.t("settings.lanUrls"),
                // the desktop LAN share is for the phone app only (no browser access)
                c.t(if lan.is_some() {
                    "native.lanScanApp"
                } else {
                    "settings.scanQr"
                }),
            );
            r.image = qr_image(&urls[0]);
            r.urls = model(
                urls.iter()
                    .map(|u| SharedString::from(u.as_str()))
                    .collect(),
            );
            if a.host.platform() == "win32" {
                r.warn = c.t("settings.lanFirewall").into();
            }
            rows.push(R(
                r,
                format!(
                    "{} lan qr wifi",
                    both(&["settings.network", "settings.lanUrls", "settings.lanShare"])
                ),
            ));
        }
        net_cards.push(Card {
            id: "lan",
            title_key: "settings.network".into(),
            title: c.t("settings.network"),
            icon: ic.get_wifi(),
            scope: "",
            rows,
        });
    }
    if info.is_some() {
        let mut rows = vec![];
        let mode_desktop = info
            .as_ref()
            .is_some_and(|i| i.mode == pa_core::RunMode::Desktop);
        rows.push(R(
            row(
                "note",
                "",
                c.t(if mode_desktop {
                    "devices.cardHintDesktop"
                } else {
                    "devices.cardHint"
                }),
                String::new(),
            ),
            String::new(),
        ));
        let devs = st.devices.borrow().clone();
        if devs.is_empty() {
            rows.push(R(
                row("note", "", c.t("devices.empty"), String::new()),
                String::new(),
            ));
        }
        for d in &devs {
            let mut r = row("device", "", d.name.clone(), String::new());
            r.online = d.online;
            if !d.paired && d.code.is_some() {
                r.sub = format!(
                    "{} {}",
                    c.t("devices.pendingCode"),
                    d.code.clone().unwrap_or_default()
                )
                .into();
                r.warn = "1".into();
                r.b1_key = format!("approve:{}", d.id).into();
                r.b1_label = c.t("devices.pair.approve").into();
                r.b1_primary = true;
                r.b2_key = format!("reject:{}", d.id).into();
                r.b2_label = c.t("devices.pair.reject").into();
            } else {
                let mut sub = if d.online {
                    c.t("devices.online")
                } else {
                    c.tv(
                        "devices.lastSeen",
                        &[("time", &fmt::relative_time(d.last_seen, c.l))],
                    )
                };
                if !d.folders.is_empty() {
                    sub.push_str(" · ");
                    sub.push_str(
                        &d.folders
                            .iter()
                            .map(|f| f.name.clone())
                            .collect::<Vec<_>>()
                            .join(", "),
                    );
                }
                r.sub = sub.into();
                r.b2_key = format!("forget:{}", d.id).into();
                r.b2_label = c.t("devices.forget").into();
            }
            rows.push(R(r, format!("{} phone telefon", d.name)));
        }
        net_cards.push(Card {
            id: "devices",
            title_key: "devices.title".into(),
            title: c.t("devices.title"),
            icon: ic.get_smartphone(),
            scope: "",
            rows,
        });
    }
    if let Some(m) = st.mega.borrow().as_ref() {
        let mut rows = vec![];
        if m.logged_in {
            let transfer = m.transfer.map(|t| match t.total {
                Some(total) => c.tv(
                    "mega.account.transfer",
                    &[
                        ("used", &fmt::format_bytes(t.used)),
                        ("total", &fmt::format_bytes(total)),
                    ],
                ),
                None => c.tv(
                    "mega.account.transferUsed",
                    &[("used", &fmt::format_bytes(t.used))],
                ),
            });
            let mut r = row(
                "buttons",
                "mega-logout",
                c.tv(
                    "mega.account.loggedIn",
                    &[("email", m.email.as_deref().unwrap_or(""))],
                ),
                transfer.unwrap_or_else(|| c.t("mega.account.hint")),
            );
            r.b1_key = "mega-logout".into();
            r.b1_label = c.t("mega.account.logout").into();
            r.b1_icon = ic.get_log_out();
            r.badge = match m.kind {
                Some(pa_core::MegaAccountType::Pro) => c.t("mega.account.pro"),
                Some(pa_core::MegaAccountType::Business) => c.t("mega.account.business"),
                Some(pa_core::MegaAccountType::Free) => c.t("mega.account.free"),
                None => String::new(),
            }
            .into();
            rows.push(R(r, both(&["mega.account.title"])));
        } else {
            let mut r = row(
                "mega-form",
                "mega-login",
                String::new(),
                format!(
                    "{}{} {}",
                    if m.expired {
                        format!("{} · ", c.t("mega.account.expired"))
                    } else {
                        String::new()
                    },
                    c.t("mega.account.hint"),
                    c.t("mega.account.why")
                ),
            );
            r.value = if st.mega_mfa.get() { "mfa" } else { "" }.into();
            r.warn = st.mega_error.borrow().clone().into();
            r.b1_busy = st.busy.borrow().contains("mega-login");
            r.enabled = !ro;
            rows.push(R(
                r,
                format!(
                    "{} mega account hesap login giriş quota kota",
                    both(&["mega.account.title", "mega.account.login"])
                ),
            ));
        }
        net_cards.push(Card {
            id: "mega-account",
            title_key: "mega.account.title".into(),
            title: c.t("mega.account.title"),
            icon: ic.get_user(),
            scope: "server",
            rows,
        });
    }
    secs.push(Sec {
        id: "network",
        title: "pref.sec.network",
        icon: ic.get_network(),
        cards: net_cards,
    });

    // ── desktop ──
    if let Some(sh) = &shell {
        let mut startup = vec![];
        if sh.login_supported {
            startup.push(c.switch(
                "pref.desk.loginStart",
                None,
                "d:startAtLogin",
                sh.start_at_login,
                "autostart login windows startup",
            ));
            let mut hid = c.switch(
                "pref.desk.startHidden",
                Some("pref.desk.startHiddenHint"),
                "d:startHidden",
                sh.start_hidden,
                "tray tepsi minimized",
            );
            hid.0.enabled = sh.start_at_login;
            startup.push(hid);
        }
        let mut close = c.seg(
            "pref.desk.close",
            c.t("pref.desk.closeHint"),
            "d:closeAction",
            &sh.close_action,
            &[
                ("auto", "pref.desk.close.auto"),
                ("tray", "pref.desk.close.tray"),
                ("quit", "pref.desk.close.quit"),
            ],
            "tray tepsi quit çık close kapat",
        );
        close.0.stack = true;
        startup.push(close);
        let start_gpu = st.gpu_start.get().unwrap_or(sh.gpu_active);
        let mut gpu = c.switch(
            "pref.desk.gpu",
            Some("pref.desk.gpuHint"),
            "d:gpu",
            sh.gpu,
            "gpu hardware acceleration donanım ekran kartı",
        );
        if start_gpu != sh.gpu {
            gpu.0.badge = c.t("pref.desk.restartNeeded").into();
            gpu.0.b2_key = "restart".into();
            gpu.0.b2_label = c.t("pref.desk.restart").into();
            gpu.0.b2_icon = ic.get_rotate_cw();
        }
        startup.push(gpu);
        startup.push(c.switch(
            "pref.desk.notify",
            Some("pref.desk.notifyHint"),
            "d:notify",
            sh.notify,
            "notification bildirim",
        ));
        let mut updates = vec![c.switch(
            "pref.desk.autoUpdate",
            Some("pref.desk.autoUpdateHint"),
            "d:autoUpdate",
            sh.auto_update,
            "update güncelleme github release",
        )];
        let last = sh.last_update.clone();
        let hint = last
            .as_ref()
            .map(|u| {
                if u.error.is_some() {
                    c.t("pref.desk.checkFailed")
                } else {
                    c.tv(
                        "pref.desk.lastCheck",
                        &[("time", &fmt::relative_time(u.checked_at, c.l))],
                    )
                }
            })
            .unwrap_or_default();
        let mut r = row(
            "buttons",
            "check-update",
            c.tv("settings.version", &[("v", &sh.version)]),
            hint,
        );
        r.b1_key = "check-update".into();
        r.b1_label = c.t("pref.desk.checkNow").into();
        r.b1_icon = ic.get_refresh_cw();
        r.b1_busy = st.busy.borrow().contains("check-update");
        if let Some(u) = last.as_ref().filter(|u| u.newer && u.url.is_some()) {
            r.b3_key = format!("open-url:{}", u.url.clone().unwrap_or_default()).into();
            r.b3_label = c
                .tv(
                    "pref.desk.updateAvailable",
                    &[("v", u.latest.as_deref().unwrap_or(""))],
                )
                .into();
        }
        updates.push(R(
            r,
            format!(
                "{} update güncelleme version sürüm",
                both(&["pref.desk.checkNow"])
            ),
        ));
        let folders = vec![
            c.button(
                "pref.desk.openData",
                c.t("pref.desk.openDataHint"),
                "open:data",
                "pref.desk.open",
                ic.get_folder_open(),
                "data folder veri klasör database",
            ),
            c.button(
                "pref.desk.openLogs",
                String::new(),
                "open:logs",
                "pref.desk.open",
                ic.get_scroll_text(),
                "log günlük debug",
            ),
        ];
        secs.push(Sec {
            id: "desktop",
            title: "pref.sec.desktop",
            icon: ic.get_laptop(),
            cards: vec![
                Card {
                    id: "startup",
                    title_key: "pref.desk.startup".into(),
                    title: c.t("pref.desk.startup"),
                    icon: ic.get_power(),
                    scope: "device",
                    rows: startup,
                },
                Card {
                    id: "updates",
                    title_key: "pref.desk.updates".into(),
                    title: c.t("pref.desk.updates"),
                    icon: ic.get_sparkles(),
                    scope: "device",
                    rows: updates,
                },
                Card {
                    id: "folders",
                    title_key: "pref.desk.folders".into(),
                    title: c.t("pref.desk.folders"),
                    icon: ic.get_folder_open(),
                    scope: "",
                    rows: folders,
                },
            ],
        });
    }

    // ── privacy ──
    secs.push(Sec {
        id: "privacy",
        title: "pref.sec.privacy",
        icon: ic.get_shield_check(),
        cards: vec![Card {
            id: "location",
            title_key: "pref.location".into(),
            title: c.t("pref.location"),
            icon: ic.get_map_pin_off(),
            scope: "device",
            rows: vec![c.switch(
                "pref.hideLocation",
                Some("pref.hideLocationHint"),
                "hideLocation",
                p.hide_location,
                "gps location konum map privacy",
            )],
        }],
    });

    // ── shortcuts (a phone has no keyboard) ──
    if !a.window.global::<crate::Shell>().get_phone() {
        let mut keys = vec![c.button(
            "pref.shortcutsOpen",
            c.t("pref.shortcutsHint"),
            "shortcuts",
            "pref.shortcutsOpen",
            ic.get_keyboard(),
            "keyboard klavye hotkey",
        )];
        let mut group = String::new();
        for k in super::dialogs::shortcut_rows(a) {
            if !k.group.is_empty() {
                group = k.group.to_string();
                keys.push(R(
                    row("keys", "", k.group.to_string(), String::new()),
                    format!("{group} keyboard klavye kısayol shortcut"),
                ));
            } else {
                let mut r = row("key", "", k.label.to_string(), String::new());
                r.keys = k.keys.clone();
                keys.push(R(
                    r,
                    format!("{} {group} keyboard klavye kısayol shortcut", k.label),
                ));
            }
        }
        secs.push(Sec {
            id: "shortcuts",
            title: "pref.sec.shortcuts",
            icon: ic.get_keyboard(),
            cards: vec![Card {
                id: "shortcuts",
                title_key: "pref.sec.shortcuts".into(),
                title: c.t("pref.sec.shortcuts"),
                icon: ic.get_keyboard(),
                scope: "",
                rows: keys,
            }],
        });
    }

    // ── about + backup ──
    let mut about = vec![];
    let mut r = row("about", "", String::new(), String::new());
    r.sub = match &info {
        Some(i) => format!(
            "{} · {} · {}",
            c.tv("settings.version", &[("v", &i.version)]),
            c.t(&format!(
                "settings.mode.{}",
                match i.mode {
                    pa_core::RunMode::Desktop => "desktop",
                    pa_core::RunMode::Server => "server",
                    pa_core::RunMode::AndroidLocal => "android-local",
                }
            )),
            i.hostname
        ),
        None => c.tv("settings.version", &[("v", "…")]),
    }
    .into();
    about.push(R(
        r,
        format!(
            "peekaround version {}",
            both(&["settings.about", "settings.version"])
        ),
    ));
    let conn = a.window.global::<Shell>().get_conn();
    let mut r = row("status", "", c.t("settings.connection"), String::new());
    r.online = conn == crate::Conn::Online;
    r.value = format!(
        "● {}",
        match conn {
            crate::Conn::Online => "online",
            crate::Conn::Offline => "offline",
            crate::Conn::Connecting => "connecting",
        }
    )
    .into();
    if ro {
        r.sub = c.t("settings.readonly").into();
    }
    about.push(R(
        r,
        format!("{} version sürüm online", both(&["settings.connection"])),
    ));
    let mut b = row(
        "buttons",
        "backup",
        c.t("pref.backup"),
        c.t("pref.backupHint"),
    );
    b.stack = true;
    b.b1_key = "export".into();
    b.b1_label = c.t("pref.export").into();
    b.b1_icon = ic.get_download();
    b.b2_key = "import".into();
    b.b2_label = c.t("pref.import").into();
    b.b2_icon = ic.get_upload();
    b.b3_key = "reset".into();
    b.b3_label = c.t("pref.reset").into();
    b.b3_icon = ic.get_rotate_ccw();
    secs.push(Sec {
        id: "about",
        title: "pref.sec.about",
        icon: ic.get_info(),
        cards: vec![
            Card {
                id: "about",
                title_key: "settings.about".into(),
                title: c.t("settings.about"),
                icon: ic.get_info(),
                scope: "",
                rows: about,
            },
            Card {
                id: "backup",
                title_key: "pref.backup".into(),
                title: c.t("pref.backup"),
                icon: ic.get_database_backup(),
                scope: "",
                rows: vec![R(
                    b,
                    format!(
                        "{} export import json reset sıfırla yedek",
                        both(&["pref.backup"])
                    ),
                )],
            },
        ],
    });
    let _ = desktop;
    secs
}

fn r_paths(l: &pa_core::Library) -> String {
    l.folders
        .iter()
        .map(|f| f.path.clone())
        .collect::<Vec<_>>()
        .join(" ")
}

/// QR code of a LAN address as an image (white background added by the row)
fn qr_image(text: &str) -> Image {
    let Ok(code) = qrcode::QrCode::new(text.as_bytes()) else {
        return Image::default();
    };
    let w = code.width();
    let border = 1usize;
    let n = w + 2 * border;
    let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(n as u32, n as u32);
    let px = buf.make_mut_slice();
    for (i, p) in px.iter_mut().enumerate() {
        let (x, y) = (i % n, i / n);
        let dark = x >= border
            && y >= border
            && x < w + border
            && y < w + border
            && code[(x - border, y - border)] == qrcode::Color::Dark;
        let v = if dark { 0 } else { 255 };
        *p = slint::Rgba8Pixel {
            r: v,
            g: v,
            b: v,
            a: 255,
        };
    }
    Image::from_rgba8(buf)
}

/// push the (filtered) sections
pub(crate) fn publish(a: &Rc<AppInner>) {
    let s = a.window.global::<SetState>();
    let q = s.get_query().trim().to_string();
    let secs = build(a);
    let mut out = vec![];
    let mut nav = vec![];
    for sec in secs {
        let sec_all = !q.is_empty() && fmt::matches_query(&both(&[sec.title]), &q);
        let mut cards = vec![];
        for card in sec.cards {
            let card_all = sec_all
                || (!q.is_empty()
                    && fmt::matches_query(
                        &format!("{} {}", both(&[&card.title_key]), card.title),
                        &q,
                    ));
            let rows: Vec<SetRow> = card
                .rows
                .into_iter()
                .filter(|R(r, kw)| {
                    q.is_empty()
                        || card_all
                        || fmt::matches_query(&format!("{} {} {kw}", r.label, r.hint), &q)
                })
                .map(|R(r, _)| r)
                .collect();
            if rows.is_empty()
                || (rows.iter().all(|r| r.kind == "note") && !q.is_empty() && !card_all)
            {
                continue;
            }
            cards.push(SetCard {
                id: card.id.into(),
                title: card.title.into(),
                icon: card.icon,
                scope: card.scope.into(),
                rows: model(rows),
            });
        }
        if cards.is_empty() {
            continue;
        }
        let title = i18n::t(a.lang(), sec.title);
        nav.push(SetNav {
            id: sec.id.into(),
            label: title.clone().into(),
            icon: sec.icon.clone(),
        });
        out.push(SetSection {
            id: sec.id.into(),
            title: title.into(),
            icon: sec.icon,
            cards: model(cards),
        });
    }
    s.set_empty(out.is_empty());
    let n = out.len();
    a.pages
        .settings
        .ys
        .borrow_mut()
        .resize(n, (String::new(), 0.0));
    for (i, sec) in out.iter().enumerate() {
        a.pages.settings.ys.borrow_mut()[i].0 = sec.id.to_string();
    }
    s.set_nav(model(nav));
    s.set_sections(model(out));
}

/// GET the server settings / cache / devices / MEGA account / meta status, the desktop shell state
pub(crate) fn refresh(a: &Rc<AppInner>) {
    let st = &a.pages.settings;
    *st.lan.borrow_mut() = a.host.lan();
    if let Some(l) = st.lan.borrow().as_ref() {
        if st.lan_port.borrow().is_empty() {
            *st.lan_port.borrow_mut() = l.port.to_string();
        }
    }
    let sh = a.host.shell();
    if st.gpu_start.get().is_none() {
        st.gpu_start.set(sh.as_ref().map(|s| s.gpu_active));
    }
    *st.shell.borrow_mut() = sh;
    api::call::<ServerInfo>(a, "GET", "/api/info", None, |a, r| {
        if let Ok(i) = r {
            *a.pages.settings.info.borrow_mut() = Some(i);
            publish(a);
        }
    });
    api::call::<Settings>(a, "GET", "/api/settings", None, |a, r| {
        if let Ok(s) = r {
            set_server(a, s);
        }
    });
    api::call::<serde_json::Value>(a, "GET", "/api/cache/details", None, |a, r| {
        if let Ok(v) = r {
            *a.pages.settings.cache.borrow_mut() = Some(v);
            publish(a);
        }
    });
    api::call::<serde_json::Value>(a, "GET", "/api/tiles/cache", None, |a, r| {
        if let Ok(v) = r {
            *a.pages.settings.tiles.borrow_mut() = Some(v);
            publish(a);
        }
    });
    api::call::<MetaStatus>(a, "GET", "/api/meta/status", None, |a, r| {
        if let Ok(v) = r {
            *a.pages.settings.meta.borrow_mut() = Some(v);
            publish(a);
        }
    });
    api::call::<Vec<DeviceInfo>>(a, "GET", "/api/devices", None, |a, r| {
        if let Ok(v) = r {
            *a.pages.settings.devices.borrow_mut() = v;
            publish(a);
        }
    });
    load_mega(a);
}

fn load_mega(a: &Rc<AppInner>) {
    api::call::<MegaAccountStatus>(a, "GET", "/api/remote/account", None, |a, r| {
        if let Ok(v) = r {
            a.pages.settings.mega_logged_in.set(v.logged_in);
            *a.pages.settings.mega.borrow_mut() = Some(v);
            publish(a);
        }
    });
}

fn set_server(a: &Rc<AppInner>, s: Settings) {
    *a.pages.settings.zip_action.borrow_mut() = serde_json::to_value(s.zip_action)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    *a.pages.settings.server.borrow_mut() = Some(s);
    publish(a);
}

/// entering /settings (jump to Shell.settings-section when set)
pub(crate) fn enter(a: &Rc<AppInner>) {
    refresh(a);
    publish(a);
    let sec = a
        .window
        .global::<Shell>()
        .get_settings_section()
        .to_string();
    let w = Rc::downgrade(a);
    if !sec.is_empty() && sec != "appearance" {
        slint::Timer::single_shot(std::time::Duration::from_millis(400), move || {
            if let Some(a) = w.upgrade() {
                jump(&a, &sec);
            }
        });
    } else {
        a.window
            .global::<SetState>()
            .set_active("appearance".into());
        a.window.global::<SetState>().set_scroll_y(0.0);
    }
    a.pages.settings.loaded.set(true);
}

/// the libraries changed (libraries card)
pub(crate) fn on_libraries(a: &Rc<AppInner>) {
    if matches!(*a.route.borrow(), crate::router::Route::Settings) {
        publish(a);
    }
}

/// the MEGA session changed
pub(crate) fn on_mega_account(a: &Rc<AppInner>) {
    load_mega(a);
}

/// settings search (automation)
pub(crate) fn search(a: &Rc<AppInner>, q: &str) {
    a.window.global::<SetState>().set_query(q.into());
    publish(a);
}

fn jump(a: &Rc<AppInner>, id: &str) {
    let ys = a.pages.settings.ys.borrow().clone();
    if let Some((_, y)) = ys.iter().find(|(s, _)| s == id) {
        a.pages.settings.jumping.set(fmt::now_ms() + 500);
        // the section list starts below the page title (32 px top + title + 28 px gap)
        let s = a.window.global::<SetState>();
        s.set_scroll_y(-(y + 32.0 + 64.0 - 24.0).max(0.0));
        s.set_active(id.into());
    }
}

fn patch_server(a: &Rc<AppInner>, body: serde_json::Value) {
    api::call::<Settings>(a, "PATCH", "/api/settings", Some(body), |a, r| match r {
        Ok(s) => set_server(a, s),
        Err(e) => {
            a.toast(ToastKind::Error, &e.message, "");
            publish(a);
        }
    });
}

fn set_busy(a: &Rc<AppInner>, k: &str, on: bool) {
    if on {
        a.pages.settings.busy.borrow_mut().insert(k.into());
    } else {
        a.pages.settings.busy.borrow_mut().remove(k);
    }
    publish(a);
}

fn changed(a: &Rc<AppInner>, key: &str, value: &str) {
    if let Some(k) = key.strip_prefix("an:") {
        let on = if value == "true" { "on" } else { "off" };
        match k.strip_prefix("folder:") {
            Some(path) => a.host.android_action(&format!("folder:{on}:{path}")),
            None => a.host.android_action(&format!("{k}:{on}")),
        }
        return publish(a);
    }
    if let Some(k) = key.strip_prefix("s:") {
        let v: serde_json::Value = match k {
            "showHidden" | "hwAccel" => (value == "true").into(),
            "tileCacheMb" | "remoteCacheMb" | "tempSessionHours" => {
                match value.trim().parse::<i64>() {
                    Ok(n) => n.into(),
                    Err(_) => return publish(a),
                }
            }
            _ => value.into(),
        };
        // unchanged text fields fire on blur too
        let cur = a
            .pages
            .settings
            .server
            .borrow()
            .as_ref()
            .and_then(|s| serde_json::to_value(s).ok())
            .map(|s| s[k].clone());
        if cur.as_ref() == Some(&v) {
            return;
        }
        let refetch_tiles = k == "tileCacheMb";
        patch_server(a, serde_json::json!({ k: v }));
        if k == "mapTiles" || k == "homeTimeZone" || refetch_tiles {
            a.toast(ToastKind::Success, &a.t("explore.settings.saved"), "");
        }
        return;
    }
    if let Some(k) = key.strip_prefix("d:") {
        let b = value == "true";
        let mut p = ShellPatch::default();
        match k {
            "startAtLogin" => p.start_at_login = Some(b),
            "startHidden" => p.start_hidden = Some(b),
            "closeAction" => p.close_action = Some(value.into()),
            "gpu" => p.gpu = Some(b),
            "notify" => p.notify = Some(b),
            "autoUpdate" => p.auto_update = Some(b),
            _ => {}
        }
        *a.pages.settings.shell.borrow_mut() = a.host.set_shell(p);
        publish(a);
        return;
    }
    match key {
        "lanPort" => {
            *a.pages.settings.lan_port.borrow_mut() = value.trim().to_string();
        }
        "lan" => {
            let on = value == "true";
            let port: u16 = a
                .pages
                .settings
                .lan_port
                .borrow()
                .trim()
                .parse()
                .unwrap_or(0);
            if port == 0 {
                a.toast(ToastKind::Error, &a.t("common.error"), "Invalid port");
                return publish(a);
            }
            set_busy(a, "lan", true);
            let fut = a.host.set_lan(on, port);
            let w = Rc::downgrade(a);
            a.rt.spawn_then(fut, move |r| {
                let Some(a) = w.upgrade() else { return };
                if let Some(e) = &r.error {
                    a.toast(ToastKind::Error, &a.t("common.error"), e);
                }
                *a.pages.settings.lan.borrow_mut() = Some(r);
                set_busy(&a, "lan", false);
            });
        }
        _ => {
            a.set_pref(key, value);
            publish(a);
        }
    }
}

fn action(a: &Rc<AppInner>, key: &str) {
    if let Some(k) = key.strip_prefix("an:") {
        a.host.android_action(k);
        return publish(a);
    }
    if let Some(u) = key.strip_prefix("open-url:") {
        a.host.open_url(u);
        return;
    }
    if let Some(u) = key.strip_prefix("copy-url:") {
        match crate::actions_ui::set_clipboard(u) {
            Ok(()) => a.toast(ToastKind::Success, &a.t("native.addressCopied"), u),
            Err(e) => a.toast(ToastKind::Error, &a.t("common.error"), &e),
        }
        return;
    }
    if let Some(id) = key.strip_prefix("edit-lib:") {
        return super::dialogs::edit_library(a, Some(id.into()));
    }
    if let Some(kind) = key.strip_prefix("clear:") {
        let name = a.t(&format!("pref.cache.{kind}"));
        set_busy(a, key, true);
        let k = key.to_string();
        api::call::<serde_json::Value>(
            a,
            "DELETE",
            &format!("/api/cache/{kind}"),
            None,
            move |a, r| {
                match r {
                    Ok(_) => a.toast(
                        ToastKind::Success,
                        &i18n::tv(a.lang(), "pref.cache.cleared", &[("name", &name)]),
                        "",
                    ),
                    Err(e) => a.error_toast(&e.message),
                }
                set_busy(a, &k, false);
                refresh(a);
            },
        );
        return;
    }
    if let Some(id) = key.strip_prefix("reread:") {
        let body = if id == "*" {
            serde_json::json!({})
        } else {
            serde_json::json!({ "libraryId": id })
        };
        set_busy(a, key, true);
        let k = key.to_string();
        api::call::<serde_json::Value>(a, "POST", "/api/meta/reread", Some(body), move |a, r| {
            match r {
                Ok(_) => a.toast(ToastKind::Success, &a.t("meta.rereadStarted"), ""),
                Err(e) => a.error_toast(&e.message),
            }
            set_busy(a, &k, false);
            refresh(a);
        });
        return;
    }
    for (prefix, verb) in [("approve:", "approve"), ("reject:", "reject")] {
        if let Some(id) = key.strip_prefix(prefix) {
            api::act(
                a,
                "POST",
                &format!("/api/devices/{}/{verb}", api::enc(id)),
                Some(serde_json::json!({})),
                |a, _| {
                    refresh(a);
                    super::hub::load_libraries(a);
                },
            );
            return;
        }
    }
    if let Some(id) = key.strip_prefix("forget:") {
        let name = a
            .pages
            .settings
            .devices
            .borrow()
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.name.clone())
            .unwrap_or_default();
        let id = id.to_string();
        let (t, b, l) = (
            i18n::tv(a.lang(), "devices.forgetTitle", &[("name", &name)]),
            a.t("devices.forgetBody"),
            a.t("devices.forget"),
        );
        super::dialogs::confirm(a, &t, &b, &l, true, move |a| {
            api::act(
                a,
                "DELETE",
                &format!("/api/devices/{}", api::enc(&id)),
                None,
                |a, _| {
                    refresh(a);
                    super::hub::load_libraries(a);
                },
            );
        });
        return;
    }
    match key {
        "add-lib" => super::dialogs::edit_library(a, None),
        "clear-tiles" => {
            set_busy(a, key, true);
            api::call::<serde_json::Value>(a, "DELETE", "/api/tiles/cache", None, |a, r| {
                match r {
                    Ok(_) => a.toast(ToastKind::Success, &a.t("map.settings.cleared"), ""),
                    Err(e) => a.error_toast(&e.message),
                }
                set_busy(a, "clear-tiles", false);
                refresh(a);
            });
        }
        "shortcuts" => super::dialogs::toggle_shortcuts(a),
        "restart" => a.host.restart(),
        "open:data" | "open:logs" => {
            if let Err(e) = a.host.open_path(&key[5..]) {
                a.error_toast(&e);
            }
        }
        "check-update" => {
            set_busy(a, key, true);
            let fut = a.host.check_update();
            let w = Rc::downgrade(a);
            a.rt.spawn_then(fut, move |u| {
                let Some(a) = w.upgrade() else { return };
                set_busy(&a, "check-update", false);
                if let Some(e) = &u.error {
                    a.toast(ToastKind::Error, &a.t("pref.desk.checkFailed"), e);
                } else if u.newer {
                    super::dialogs::update_toast(&a, &u);
                } else {
                    a.toast(
                        ToastKind::Success,
                        &i18n::tv(a.lang(), "pref.desk.upToDate", &[("v", &u.current)]),
                        "",
                    );
                }
                *a.pages.settings.shell.borrow_mut() = a.host.shell();
                publish(&a);
            });
        }
        "mega-login" => mega_login(a),
        "mega-logout" => {
            api::call::<MegaAccountStatus>(
                a,
                "POST",
                "/api/remote/account/logout",
                Some(serde_json::json!({})),
                |a, r| match r {
                    Ok(m) => {
                        a.pages.settings.mega_logged_in.set(m.logged_in);
                        *a.pages.settings.mega.borrow_mut() = Some(m);
                        a.toast(ToastKind::Success, &a.t("mega.account.loggedOutToast"), "");
                        publish(a);
                    }
                    Err(e) => a.error_toast(&e.message),
                },
            );
        }
        "export" => export_backup(a),
        "import" => import_backup(a),
        "reset" => {
            let (t, b, l) = (
                a.t("pref.resetTitle"),
                a.t("pref.resetBody"),
                a.t("pref.reset"),
            );
            super::dialogs::confirm(a, &t, &b, &l, true, reset_all);
        }
        _ => {}
    }
}

fn mega_login(a: &Rc<AppInner>) {
    let s = a.window.global::<SetState>();
    let mut body = serde_json::json!({ "email": s.get_mega_email().trim(), "password": s.get_mega_password().as_str() });
    if a.pages.settings.mega_mfa.get() && !s.get_mega_code().is_empty() {
        body["code"] = s.get_mega_code().trim().into();
    }
    a.pages.settings.mega_error.borrow_mut().clear();
    set_busy(a, "mega-login", true);
    api::call::<serde_json::Value>(
        a,
        "POST",
        "/api/remote/account/login",
        Some(body),
        |a, r| {
            let st = &a.pages.settings;
            st.busy.borrow_mut().remove("mega-login");
            match r {
                Ok(v) if v["ok"].as_bool() == Some(true) => {
                    if let Ok(m) = serde_json::from_value::<MegaAccountStatus>(v["account"].clone())
                    {
                        st.mega_logged_in.set(m.logged_in);
                        *st.mega.borrow_mut() = Some(m);
                    }
                    st.mega_mfa.set(false);
                    let s = a.window.global::<SetState>();
                    s.set_mega_password("".into());
                    s.set_mega_code("".into());
                    a.toast(ToastKind::Success, &a.t("mega.account.loggedInToast"), "");
                }
                Ok(v) => {
                    let reason = v["reason"].as_str().unwrap_or("");
                    if reason == "mfa-required" || reason == "mfa-invalid" {
                        st.mega_mfa.set(true);
                    }
                    let k = match reason {
                        "credentials" => "mega.account.err.credentials",
                        "mfa-invalid" => "mega.account.err.mfaInvalid",
                        "blocked" => "mega.account.err.blocked",
                        _ => "mega.account.codeHint",
                    };
                    *st.mega_error.borrow_mut() = a.t(k);
                }
                Err(e) => *st.mega_error.borrow_mut() = e.message,
            }
            publish(a);
        },
    );
}

// ───────── backup (backup.ts) ─────────

const PREF_KEYS: [&str; 33] = [
    "lang",
    "theme",
    "accent",
    "motion",
    "gridMode",
    "rowHeight",
    "sort",
    "order",
    "kind",
    "wideSidebar",
    "infoOpen",
    "hoverPreview",
    "gridZoomDesktop",
    "dateHeaders",
    "groupBy",
    "tileNames",
    "tileDuration",
    "tileFavorites",
    "confirmDelete",
    "viewerBg",
    "filmstrip",
    "infoDefault",
    "zoomStep",
    "dblZoom",
    "videoAutoplay",
    "videoMuted",
    "loopShortSec",
    "videoSpeed",
    "rememberVolume",
    "videoVolume",
    "showMemories",
    "weekStart",
    "hideLocation",
];

fn server_backup(s: &Settings) -> serde_json::Value {
    let mut v = serde_json::to_value(s).unwrap_or_default();
    if let Some(o) = v.as_object_mut() {
        o.remove("hostBrowseRoots");
    }
    v
}

fn make_backup(a: &AppInner) -> serde_json::Value {
    let p = serde_json::to_value(&*a.prefs.borrow()).unwrap_or_default();
    let mut prefs = serde_json::Map::new();
    for k in PREF_KEYS
        .iter()
        .chain(["dateFormat", "hourCycle", "libraryView", "folderSort"].iter())
    {
        if let Some(v) = p.get(*k) {
            prefs.insert((*k).into(), v.clone());
        }
    }
    let now = fmt::now_ms();
    let (y, m, d) = i18n::civil_from_days(now / 86_400_000);
    let mut out = serde_json::json!({
        "app": "peekaround",
        "kind": "settings",
        "version": 1,
        "exportedAt": format!("{y:04}-{m:02}-{d:02}T00:00:00.000Z"),
        "prefs": prefs,
        "slideshow": { "duration": p["slideDuration"], "transition": p["slideTransition"] },
    });
    if let Some(s) = a.pages.settings.server.borrow().as_ref() {
        out["server"] = server_backup(s);
    }
    out
}

fn export_backup(a: &Rc<AppInner>) {
    let now = fmt::now_ms();
    let (y, m, d) = i18n::civil_from_days(now / 86_400_000);
    let name = format!("peekaround-settings-{y:04}-{m:02}-{d:02}.json");
    let json = serde_json::to_string_pretty(&make_backup(a)).unwrap_or_default();
    let fut = a.host.save_file(&a.t("pref.export"), &name);
    let w = Rc::downgrade(a);
    a.rt.spawn_then(fut, move |dest| {
        let (Some(a), Some(dest)) = (w.upgrade(), dest) else {
            return;
        };
        match std::fs::write(&dest, json) {
            Ok(()) => a.toast(
                ToastKind::Success,
                &a.t("pref.export"),
                &dest.to_string_lossy(),
            ),
            Err(e) => a.error_toast(&e.to_string()),
        }
    });
}

/// apply a backup file; false when it is not one
pub(crate) fn apply_backup(a: &Rc<AppInner>, text: &str) -> bool {
    let Ok(j) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };
    if j["app"] != "peekaround" || j["kind"] != "settings" || j["version"] != 1 {
        return false;
    }
    if let Some(p) = j["prefs"].as_object() {
        let mut prefs = a.prefs.borrow().clone();
        for (k, v) in p {
            let s = match v {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            if k == "libraryView" {
                if let Ok(m) = serde_json::from_value(v.clone()) {
                    prefs.library_view = m;
                }
                continue;
            }
            // unknown keys / wrong types are dropped
            let _ = prefs.set_by_key(k, &s);
        }
        if let Some(d) = j["slideshow"]["duration"].as_f64() {
            prefs.slide_duration = d;
        }
        if let Some(t) = j["slideshow"]["transition"].as_str() {
            prefs.slide_transition = t.into();
        }
        let prefs = prefs.sanitized();
        let keys: Vec<(String, String)> = serde_json::to_value(&prefs)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .map(|o| {
                o.into_iter()
                    .filter(|(k, _)| k != "libraryView")
                    .map(|(k, v)| {
                        (
                            k,
                            match v {
                                serde_json::Value::String(s) => s,
                                o => o.to_string(),
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        a.prefs.borrow_mut().library_view = prefs.library_view.clone();
        for (k, v) in keys {
            a.set_pref(&k, &v);
        }
    }
    if let Some(s) = j["server"].as_object() {
        if !a.window.global::<Shell>().get_readonly() {
            let defaults = serde_json::to_value(Settings::default()).unwrap_or_default();
            let mut patch = serde_json::Map::new();
            for (k, v) in s {
                if k == "hostBrowseRoots" {
                    continue;
                }
                let same_type = defaults
                    .get(k)
                    .is_some_and(|d| std::mem::discriminant(d) == std::mem::discriminant(v));
                if same_type {
                    patch.insert(k.clone(), v.clone());
                }
            }
            if !patch.is_empty() {
                patch_server(a, serde_json::Value::Object(patch));
            }
        }
    }
    true
}

fn import_backup(a: &Rc<AppInner>) {
    let fut = a.host.pick_files(
        &a.t("pref.import"),
        vec![crate::host::FileFilter {
            name: "JSON".into(),
            extensions: vec!["json".into()],
        }],
    );
    let w = Rc::downgrade(a);
    a.rt.spawn_then(fut, move |files| {
        let Some(a) = w.upgrade() else { return };
        let Some(f) = files.first() else { return };
        let ok = std::fs::read_to_string(f)
            .ok()
            .is_some_and(|t| apply_backup(&a, &t));
        if ok {
            a.toast(ToastKind::Success, &a.t("pref.imported"), "");
        } else {
            a.toast(ToastKind::Error, &a.t("pref.importFailed"), "");
        }
        publish(&a);
    });
}

fn reset_all(a: &Rc<AppInner>) {
    let lang = a.prefs.borrow().lang.clone();
    let d = crate::prefs::Prefs::default();
    let keys: Vec<(String, String)> = serde_json::to_value(&d)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .map(|o| {
            o.into_iter()
                .filter(|(k, _)| k != "libraryView" && k != "lang")
                .map(|(k, v)| {
                    (
                        k,
                        match v {
                            serde_json::Value::String(s) => s,
                            o => o.to_string(),
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    for (k, v) in keys {
        a.set_pref(&k, &v);
    }
    a.set_pref("lang", &lang);
    if !a.window.global::<Shell>().get_readonly() {
        let mut v = serde_json::to_value(Settings::default()).unwrap_or_default();
        if let Some(o) = v.as_object_mut() {
            o.remove("hostBrowseRoots");
            o.remove("homeTimeZone");
        }
        patch_server(a, v);
    }
    a.toast(ToastKind::Success, &a.t("pref.resetDone"), "");
    publish(a);
}

pub(crate) fn install(app: &Rc<AppInner>) {
    // the phone sharing service runs in another process: follow its state
    if let Some(first) = app.host.android() {
        let last = std::cell::RefCell::new(first);
        let w = Rc::downgrade(app);
        let t = slint::Timer::default();
        t.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(1500),
            move || {
                let Some(a) = w.upgrade() else { return };
                let now = a.host.android();
                if now.as_ref() != Some(&*last.borrow()) {
                    if let Some(n) = now {
                        *last.borrow_mut() = n;
                    }
                    publish(&a);
                }
            },
        );
        std::mem::forget(t);
    }
    let s = app.window.global::<SetState>();
    let w = Rc::downgrade(app);
    s.on_changed(move |k, v| {
        if let Some(a) = w.upgrade() {
            changed(&a, &k, &v);
        }
    });
    let w = Rc::downgrade(app);
    s.on_action(move |k| {
        if let Some(a) = w.upgrade() {
            action(&a, &k);
        }
    });
    let w = Rc::downgrade(app);
    s.on_search(move |q| {
        if let Some(a) = w.upgrade() {
            search(&a, &q);
        }
    });
    let w = Rc::downgrade(app);
    s.on_jump(move |id| {
        if let Some(a) = w.upgrade() {
            jump(&a, &id);
        }
    });
    let w = Rc::downgrade(app);
    s.on_section_y(move |i, y| {
        if let Some(a) = w.upgrade() {
            if let Some(e) = a.pages.settings.ys.borrow_mut().get_mut(i as usize) {
                e.1 = y;
            }
        }
    });
    let w = Rc::downgrade(app);
    s.on_scrolled(move |y, h| {
        let Some(a) = w.upgrade() else { return };
        if fmt::now_ms() < a.pages.settings.jumping.get() {
            return;
        }
        let ys = a.pages.settings.ys.borrow().clone();
        // the last section whose top passed the upper part of the view
        let top = y + (h / 5.0).min(120.0) - 96.0;
        let mut cur = ys.first().map(|x| x.0.clone()).unwrap_or_default();
        for (id, sy) in &ys {
            if *sy <= top {
                cur = id.clone();
            }
        }
        a.window.global::<SetState>().set_active(cur.into());
    });
}
