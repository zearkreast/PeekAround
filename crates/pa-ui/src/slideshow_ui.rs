//! Slideshow builder page + full-screen player (features/slideshow): the slide list and its
//! settings (kept in the UI preferences), the live preview, playback with background music, the
//! music picker and the video export (`POST /api/jobs/slideshow`).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use pa_core::{BrowseQuery, DirTarget, KindFilter, LibrariesResponse, MediaItem, MediaKind, SlideTransition};
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};

use crate::app::AppInner;
use crate::router::Route;
use crate::slideshow::{self as model, Aspect, Rng, SlideItem, SlideSettings};
use crate::slideshow_show::{Show, ShowStatus};
use crate::video;
use crate::{MusicRow, SlideBuilder, SlidePlayer, SlideRow, ToastKind};

/// slides in the builder's preview
const PREVIEW_SLIDES: usize = 12;

struct Ctl {
    app: Weak<AppInner>,
    items: RefCell<Vec<SlideItem>>,
    seq: Cell<u64>,
    target: RefCell<Option<DirTarget>>,
    preview: RefCell<Option<Rc<Show>>>,
    player: RefCell<Option<Rc<Show>>>,
    music: RefCell<Option<video::Player>>,
    music_muted: Cell<bool>,
    /// the player made the window full screen (undo on close)
    went_full: Cell<bool>,
    hide_timer: slint::Timer,
}

thread_local! {
    static CTL: RefCell<Option<Rc<Ctl>>> = const { RefCell::new(None) };
}

fn ctl() -> Option<Rc<Ctl>> {
    CTL.with(|c| c.borrow().clone())
}

fn parse_color(s: &str) -> Color {
    let h = s.trim_start_matches('#');
    let v = u32::from_str_radix(h, 16).unwrap_or(0);
    if h.len() == 6 {
        Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8)
    } else {
        Color::from_rgb_u8(0, 0, 0)
    }
}

fn hex(c: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", c.red(), c.green(), c.blue())
}

fn transition_str(t: SlideTransition) -> &'static str {
    match t {
        SlideTransition::None => "none",
        SlideTransition::Fade => "fade",
        SlideTransition::Slide => "slide",
        SlideTransition::Zoom => "zoom",
        SlideTransition::Kenburns => "kenburns",
        SlideTransition::Blur => "blur",
        SlideTransition::Flip => "flip",
        SlideTransition::Circle => "circle",
        SlideTransition::Random => "random",
    }
}

fn parse_transition(s: &str) -> SlideTransition {
    serde_json::from_value(serde_json::Value::String(s.into())).unwrap_or(SlideTransition::Fade)
}

fn visual(m: &MediaItem) -> bool {
    matches!(m.kind, MediaKind::Image | MediaKind::Video)
}

impl Ctl {
    fn app(&self) -> Option<Rc<AppInner>> {
        self.app.upgrade()
    }

    fn settings(&self) -> SlideSettings {
        self.app().map(|a| a.prefs.borrow().slideshow.clone()).unwrap_or_default()
    }

    fn td(s: &SlideSettings) -> f64 {
        if s.transition == SlideTransition::None {
            0.0
        } else {
            s.transition_duration
        }
    }

    fn to_item(&self, m: MediaItem) -> SlideItem {
        let k = self.seq.get() + 1;
        self.seq.set(k);
        SlideItem { key: format!("{}-{k}", m.id), media: m, duration: None, trim: None }
    }

    /// The settings controls → preferences (+ the preview restarts with the new style).
    fn read_settings(&self) {
        let Some(a) = self.app() else { return };
        let b = a.window.global::<SlideBuilder>();
        {
            let mut p = a.prefs.borrow_mut();
            let s = &mut p.slideshow;
            s.transition = parse_transition(&b.get_transition());
            s.duration = (b.get_duration() as f64 * 2.0).round() / 2.0;
            s.transition_duration = (b.get_transition_duration() as f64 * 10.0).round() / 10.0;
            s.ken_burns = b.get_ken_burns();
            s.aspect = Aspect::parse(&b.get_aspect()).unwrap_or(Aspect::Wide);
            s.background = hex(b.get_background());
            s.shuffle = b.get_shuffle();
            s.looping = b.get_loop();
            s.captions = b.get_captions();
            s.res = b.get_res().parse().unwrap_or(1080);
        }
        a.save_prefs();
        self.publish();
    }

    fn publish(&self) {
        let Some(a) = self.app() else { return };
        let s = self.settings();
        let b = a.window.global::<SlideBuilder>();
        b.set_transition(transition_str(s.transition).into());
        b.set_duration(s.duration as f32);
        b.set_transition_duration(s.transition_duration as f32);
        b.set_ken_burns(s.ken_burns || s.transition == SlideTransition::Kenburns);
        b.set_aspect(s.aspect.label().into());
        b.set_aspect_ratio(s.aspect.ratio() as f32);
        b.set_background(parse_color(&s.background));
        b.set_shuffle(s.shuffle);
        b.set_loop(s.looping);
        b.set_captions(s.captions);
        b.set_res(s.res.to_string().into());
        b.set_has_music(s.music.is_some());
        b.set_music_name(s.music.as_ref().map(|m| m.name.clone()).unwrap_or_default().into());
        let items = self.items.borrow().clone();
        let lang = a.lang();
        let rows: Vec<SlideRow> = items
            .iter()
            .map(|it| {
                let video = it.media.kind == MediaKind::Video;
                let dur = model::item_duration(it, &s);
                SlideRow {
                    thumb: Default::default(),
                    has_thumb: false,
                    name: it.media.name.clone().into(),
                    is_video: video,
                    dur: if video {
                        crate::i18n::format_duration(Some(dur)).into()
                    } else {
                        format!("{} {}", fmt_num(dur), crate::i18n::t(lang, "slide.sec")).into()
                    },
                    custom: !video && it.duration.is_some(),
                }
            })
            .collect();
        let model = Rc::new(VecModel::from(rows));
        for (i, it) in items.iter().enumerate() {
            let w = Rc::downgrade(&model);
            crate::pages::cover(&a, &it.media.id, 320, move |img| {
                if let Some(m) = w.upgrade() {
                    if let Some(mut r) = slint::Model::row_data(&*m, i) {
                        r.thumb = img;
                        r.has_thumb = true;
                        slint::Model::set_row_data(&*m, i, r);
                    }
                }
            });
        }
        b.set_rows(ModelRc::from(model));
        let photos = items.iter().filter(|i| i.media.kind == MediaKind::Image).count();
        let videos = items.len() - photos;
        b.set_stats(
            crate::i18n::tv(
                lang,
                "home.stats",
                &[("photos", &photos.to_string()), ("videos", &videos.to_string())],
            )
            .into(),
        );
        let full = model::playlist(&items, &SlideSettings { shuffle: false, ..s.clone() }, &mut Rng::new(1));
        b.set_total(crate::i18n::format_duration(Some(model::show_length(&full, Self::td(&s)))).into());
        let label = self
            .target
            .borrow()
            .as_ref()
            .map(target_label)
            .unwrap_or_default();
        b.set_target_label(label.into());
        self.restart_preview();
    }

    fn restart_preview(&self) {
        let Some(a) = self.app() else { return };
        if !matches!(*a.route.borrow(), Route::Slideshow) {
            return;
        }
        let s = self.settings();
        let items: Vec<SlideItem> = self.items.borrow().iter().take(PREVIEW_SLIDES).cloned().collect();
        let slides = model::playlist(&items, &SlideSettings { shuffle: false, ..s.clone() }, &mut Rng::from_time());
        let existing = self.preview.borrow().clone();
        let show = existing.unwrap_or_else(|| {
            let w = Rc::downgrade(&a);
            let show = Show::new(
                &a,
                640,
                false,
                move |f| {
                    if let Some(a) = w.upgrade() {
                        a.window.global::<SlideBuilder>().set_preview(f);
                    }
                },
                |_| {},
            );
            *self.preview.borrow_mut() = Some(show.clone());
            show
        });
        if slides.is_empty() {
            show.stop();
            return;
        }
        // the preview is muted and always loops, without captions
        show.load(slides, Self::td(&s), true, parse_color(&s.background), false, true);
    }

    fn stop_preview(&self) {
        if let Some(p) = self.preview.borrow_mut().take() {
            p.stop();
        }
    }

    // ───────── player ─────────

    fn play(self: &Rc<Self>) {
        let Some(a) = self.app() else { return };
        let s = self.settings();
        let slides = model::playlist(&self.items.borrow(), &s, &mut Rng::from_time());
        if slides.is_empty() {
            return;
        }
        self.stop_preview();
        let w = Rc::downgrade(&a);
        let me = Rc::downgrade(self);
        let td = Self::td(&s);
        let total = model::show_length(&slides, td);
        let show = Show::new(
            &a,
            1600,
            true,
            {
                let w = w.clone();
                move |f| {
                    if let Some(a) = w.upgrade() {
                        a.window.global::<SlidePlayer>().set_frame(f);
                    }
                }
            },
            move |st: &ShowStatus| {
                if let (Some(a), Some(c)) = (w.upgrade(), me.upgrade()) {
                    c.on_status(&a, st, total);
                }
            },
        );
        show.load(slides, td, s.looping, parse_color(&s.background), s.captions, true);
        *self.player.borrow_mut() = Some(show);
        let p = a.window.global::<SlidePlayer>();
        p.set_open(true);
        p.set_captions(s.captions);
        p.set_has_music(s.music.is_some());
        p.set_muted(self.music_muted.get());
        p.set_fullscreen(a.window.window().is_fullscreen());
        // like 1.x: the Play click goes full screen
        if !a.window.window().is_fullscreen() {
            a.window.window().set_fullscreen(true);
            self.went_full.set(true);
            p.set_fullscreen(true);
        }
        self.start_music(&a, &s);
        self.poke();
    }

    fn start_music(&self, a: &Rc<AppInner>, s: &SlideSettings) {
        *self.music.borrow_mut() = None;
        let Some(m) = s.music.as_ref() else { return };
        let Some(src) = a.backend.ops().and_then(|o| o.source(m, None)) else {
            return;
        };
        let source = video::Source {
            input: src.as_input(),
            video: false,
            width: 2,
            height: 2,
            fps: 30.0,
            duration: m.duration,
            audio: Some(true),
        };
        *self.music.borrow_mut() = Some(video::Player::new(source, 0.0, true, 1.0, self.music_muted.get(), 1.0));
    }

    fn on_status(&self, a: &Rc<AppInner>, st: &ShowStatus, total: f64) {
        let p = a.window.global::<SlidePlayer>();
        p.set_count(st.count as i32);
        p.set_index(st.index as i32);
        p.set_progress(st.progress as f32);
        p.set_playing(st.playing);
        p.set_ended(st.ended);
        let pos = (total - st.remaining).clamp(0.0, total);
        p.set_time_text(
            format!(
                "{} / {}",
                crate::viewer_fmt::clock(pos),
                crate::viewer_fmt::clock(total)
            )
            .into(),
        );
        let looping = a.prefs.borrow().slideshow.looping;
        if let Some(m) = self.music.borrow_mut().as_mut() {
            let s = m.status();
            if s.playing != st.playing {
                m.set_playing(st.playing);
            }
            // loop the track; fade out over the show's last 2 s
            if s.ended && st.playing {
                m.seek(0.0);
            }
            m.set_volume(if looping { 1.0 } else { (st.remaining / 2.0).clamp(0.0, 1.0) });
        }
    }

    fn poke(&self) {
        let Some(a) = self.app() else { return };
        a.window.global::<SlidePlayer>().set_ui(true);
        let w = self.app.clone();
        self.hide_timer.start(slint::TimerMode::SingleShot, std::time::Duration::from_millis(2600), move || {
            if let Some(a) = w.upgrade() {
                a.window.global::<SlidePlayer>().set_ui(false);
            }
        });
    }

    fn close_player(&self) {
        let Some(a) = self.app() else { return };
        if let Some(p) = self.player.borrow_mut().take() {
            p.stop();
        }
        *self.music.borrow_mut() = None;
        a.window.global::<SlidePlayer>().set_open(false);
        if self.went_full.replace(false) {
            a.window.window().set_fullscreen(false);
        }
        self.restart_preview();
    }

    // ───────── music picker ─────────

    fn pick_music(&self) {
        let Some(a) = self.app() else { return };
        let b = a.window.global::<SlideBuilder>();
        b.set_music_open(true);
        b.set_music_loading(true);
        b.set_music(ModelRc::new(VecModel::from(Vec::<MusicRow>::new())));
        let backend = a.backend.clone();
        crate::api::call::<LibrariesResponse>(&a, "GET", "/api/libraries", None, move |a, r| {
            let libs = r.map(|r| r.libraries).unwrap_or_default();
            let backend = backend.clone();
            let w = Rc::downgrade(a);
            a.rt.spawn_then(
                async move {
                    let mut found: Vec<MediaItem> = vec![];
                    for l in libs {
                        let q = BrowseQuery {
                            library_id: Some(l.id.clone()),
                            recursive: Some(true),
                            kind: Some(KindFilter::All),
                            limit: Some(2000),
                            ..Default::default()
                        };
                        if let Ok(res) = backend.browse(q).await {
                            for it in res.items.into_iter().filter(|i| i.kind == MediaKind::Audio) {
                                if !found.iter().any(|f| f.id == it.id) {
                                    found.push(it);
                                }
                            }
                        }
                    }
                    found
                },
                move |found| {
                    let Some(a) = w.upgrade() else { return };
                    let rows: Vec<MusicRow> = found
                        .iter()
                        .map(|m| MusicRow {
                            id: m.id.clone().into(),
                            name: m.name.clone().into(),
                            sub: format!(
                                "{}{}",
                                if m.dir.is_empty() { String::new() } else { format!("{} · ", m.dir) },
                                crate::i18n::format_duration(m.duration)
                            )
                            .into(),
                        })
                        .collect();
                    MUSIC.with(|c| *c.borrow_mut() = found);
                    let b = a.window.global::<SlideBuilder>();
                    b.set_music(ModelRc::new(VecModel::from(rows)));
                    b.set_music_loading(false);
                },
            );
        });
    }

    fn use_music(&self, id: &str) {
        let Some(a) = self.app() else { return };
        let m = MUSIC.with(|c| c.borrow().iter().find(|m| m.id == id).cloned());
        a.prefs.borrow_mut().slideshow.music = m;
        a.save_prefs();
        a.window.global::<SlideBuilder>().set_music_open(false);
        self.publish();
    }

    // ───────── export ─────────

    fn export(&self) {
        let Some(a) = self.app() else { return };
        let items = self.items.borrow().clone();
        if items.is_empty() {
            return;
        }
        let s = self.settings();
        let name = a.window.global::<SlideBuilder>().get_name().to_string();
        let spec = model::build_export_spec(&items, &s, self.target.borrow().clone(), &name, &mut Rng::from_time());
        let Ok(body) = serde_json::to_value(&spec) else { return };
        a.window.global::<SlideBuilder>().set_exporting(true);
        crate::api::call::<serde_json::Value>(&a, "POST", "/api/jobs/slideshow", Some(body), |a, r| {
            a.window.global::<SlideBuilder>().set_exporting(false);
            match r {
                Ok(_) => a.toast(ToastKind::Info, &a.t("slide.exportStarted"), ""),
                Err(e) => a.toast(ToastKind::Error, &a.t("slide.exportFailed"), &e.message),
            }
        });
    }
}

thread_local! {
    /// the music picker's last list
    static MUSIC: RefCell<Vec<MediaItem>> = const { RefCell::new(Vec::new()) };
}

fn fmt_num(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}

fn target_label(t: &DirTarget) -> String {
    let base = crate::actions_ui::folder_name(&t.folder_id).unwrap_or_default();
    match (base.is_empty(), t.dir.is_empty()) {
        (_, true) => base,
        (true, false) => t.dir.clone(),
        (false, false) => format!("{base} / {}", t.dir),
    }
}

/// "Slideshow" on a selection / the viewer / an album / a memory: open the builder with these.
pub(crate) fn open_with(app: &Rc<AppInner>, media: Vec<MediaItem>) {
    let Some(c) = ctl() else { return };
    let list: Vec<SlideItem> = media.into_iter().filter(visual).map(|m| c.to_item(m)).collect();
    *c.target.borrow_mut() = list
        .first()
        .map(|i| DirTarget { folder_id: i.media.folder_id.clone(), dir: i.media.dir.clone() });
    *c.items.borrow_mut() = list;
    app.window.global::<SlideBuilder>().set_name(SharedString::new());
    app.go(Route::Slideshow, true);
}

/// The page's "Slideshow" button: the grid's items (up to 1000) go to the builder.
pub(crate) fn open_from_grid(app: &Rc<AppInner>) {
    let total = app.grid.0.borrow().total();
    if total == 0 {
        return;
    }
    let w = Rc::downgrade(app);
    app.grid.0.borrow_mut().items_in_range(0, total.min(1000) - 1, move |list| {
        if let Some(a) = w.upgrade() {
            open_with(&a, list);
        }
    });
}

/// a route was entered: the preview runs only on the builder page
pub(crate) fn enter(app: &Rc<AppInner>, route: &Route) {
    let Some(c) = ctl() else { return };
    if matches!(route, Route::Slideshow) {
        let _ = app;
        c.publish();
    } else {
        c.stop_preview();
    }
}

/// Stop playback before thread-local storage is torn down (see explore_player::shutdown).
pub(crate) fn shutdown() {
    if let Some(c) = ctl() {
        if let Some(p) = c.player.borrow_mut().take() {
            p.stop();
        }
        if let Some(p) = c.preview.borrow_mut().take() {
            p.stop();
        }
        *c.music.borrow_mut() = None;
    }
}

pub(crate) fn install(app: &Rc<AppInner>) {
    let c = Rc::new(Ctl {
        app: Rc::downgrade(app),
        items: RefCell::new(vec![]),
        seq: Cell::new(0),
        target: RefCell::new(None),
        preview: RefCell::new(None),
        player: RefCell::new(None),
        music: RefCell::new(None),
        music_muted: Cell::new(false),
        went_full: Cell::new(false),
        hide_timer: slint::Timer::default(),
    });
    CTL.with(|x| *x.borrow_mut() = Some(c.clone()));
    let b = app.window.global::<SlideBuilder>();
    macro_rules! on {
        ($g:ident, $cb:ident, |$c:ident $(, $arg:ident)*| $body:expr) => {{
            let w = Rc::downgrade(&c);
            $g.$cb(move |$($arg),*| {
                if let Some($c) = w.upgrade() {
                    $body;
                }
            });
        }};
    }
    on!(b, on_play, |c| c.play());
    on!(b, on_changed, |c| c.read_settings());
    on!(b, on_remove, |c, i| {
        let i = i as usize;
        let n = c.items.borrow().len();
        if i < n {
            c.items.borrow_mut().remove(i);
        }
        c.publish();
    });
    on!(b, on_move, |c, i, d| {
        let i = i as usize;
        let j = i as i64 + d as i64;
        let n = c.items.borrow().len() as i64;
        if (0..n).contains(&j) && (i as i64) < n {
            c.items.borrow_mut().swap(i, j as usize);
        }
        c.publish();
    });
    on!(b, on_set_item_duration, |c, i, secs| {
        if let Some(it) = c.items.borrow_mut().get_mut(i as usize) {
            it.duration = if secs > 0.0 { Some(secs as f64) } else { None };
        }
        c.publish();
    });
    on!(b, on_clear, |c| {
        c.items.borrow_mut().clear();
        c.publish();
    });
    on!(b, on_preview_resized, |c, w, h| {
        if let (Some(p), Some(a)) = (c.preview.borrow().clone(), c.app()) {
            p.resize(w as f64, h as f64, a.window.window().scale_factor() as f64);
        }
    });
    on!(b, on_pick_music, |c| c.pick_music());
    on!(b, on_use_music, |c, id| c.use_music(&id));
    on!(b, on_close_music, |c| {
        if let Some(a) = c.app() {
            a.window.global::<SlideBuilder>().set_music_open(false);
        }
    });
    on!(b, on_remove_music, |c| {
        if let Some(a) = c.app() {
            a.prefs.borrow_mut().slideshow.music = None;
            a.save_prefs();
        }
        c.publish();
    });
    on!(b, on_change_target, |c| {
        if let Some(a) = c.app() {
            let me = Rc::downgrade(&c);
            crate::pages::dialogs::pick_folder(&a, &a.t("slide.saveTo"), "", move |_, t| {
                if let Some(c) = me.upgrade() {
                    *c.target.borrow_mut() = Some(t);
                    c.publish();
                }
            });
        }
    });
    on!(b, on_export, |c| c.export());
    on!(b, on_go_timeline, |c| {
        if let Some(a) = c.app() {
            a.go(Route::Photos, true);
        }
    });

    let p = app.window.global::<SlidePlayer>();
    on!(p, on_close, |c| c.close_player());
    on!(p, on_toggle, |c| {
        if let Some(s) = c.player.borrow().clone() {
            s.toggle();
        }
    });
    on!(p, on_next, |c| {
        if let Some(s) = c.player.borrow().clone() {
            s.next();
        }
    });
    on!(p, on_prev, |c| {
        if let Some(s) = c.player.borrow().clone() {
            s.prev();
        }
    });
    on!(p, on_toggle_mute, |c| {
        let m = !c.music_muted.get();
        c.music_muted.set(m);
        if let Some(pl) = c.music.borrow().as_ref() {
            pl.set_muted(m);
        }
        if let Some(a) = c.app() {
            a.window.global::<SlidePlayer>().set_muted(m);
        }
    });
    on!(p, on_toggle_captions, |c| {
        if let Some(a) = c.app() {
            let on = {
                let mut pr = a.prefs.borrow_mut();
                pr.slideshow.captions = !pr.slideshow.captions;
                pr.slideshow.captions
            };
            a.save_prefs();
            a.window.global::<SlidePlayer>().set_captions(on);
            if let Some(s) = c.player.borrow().clone() {
                s.set_captions(on);
            }
        }
    });
    on!(p, on_toggle_full, |c| {
        if let Some(a) = c.app() {
            let full = !a.window.window().is_fullscreen();
            a.window.window().set_fullscreen(full);
            c.went_full.set(full);
            a.window.global::<SlidePlayer>().set_fullscreen(full);
        }
    });
    on!(p, on_poke, |c| c.poke());
    on!(p, on_resized, |c, w, h| {
        if let (Some(s), Some(a)) = (c.player.borrow().clone(), c.app()) {
            s.resize(w as f64, h as f64, a.window.window().scale_factor() as f64);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_and_numbers() {
        assert_eq!(hex(parse_color("#0f2a3d")), "#0f2a3d");
        assert_eq!(hex(parse_color("nonsense")), "#000000");
        assert_eq!(fmt_num(4.0), "4");
        assert_eq!(fmt_num(2.5), "2.5");
        assert_eq!(transition_str(parse_transition("circle")), "circle");
        assert_eq!(parse_transition("bogus"), SlideTransition::Fade);
    }
}
