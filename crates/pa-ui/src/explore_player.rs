//! The story-style memory player (MemoryPlayer.tsx): a title card + up to 60 items spread over
//! the memory (year in review: a few per month), auto-advance with progress bars (title 3.2 s,
//! photo 5 s, video poster 7.5 s), tap left 30 % / right, hold = pause (+ hide the chrome),
//! swipe down = close, sideways swipe = step, Ken Burns, "see all" and "open as slideshow".
//! Video slides play muted through the ffmpeg player (src/video.rs) for at most 15 s, over the poster.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;
use web_time::Instant;

use pa_core::{BrowseQuery, MediaItem, MediaKind, Memory, MemoryKind, SortDir};
use slint::{ComponentHandle, Image};

use crate::app::AppInner;
use crate::explore_labels as lb;
use crate::router::Route;
use crate::PlayerState;

const PHOTO_MS: f64 = 5000.0;
const TITLE_MS: f64 = 3200.0;
const VIDEO_MAX_MS: f64 = 15000.0;
// a video that can't play (yet): photo-like clock
const VIDEO_MS: f64 = VIDEO_MAX_MS / 2.0;
const MAX_SLIDES: usize = 60;
const HOLD_MS: u64 = 220;

#[derive(Default)]
struct Press {
    x: f64,
    y: f64,
    at: Option<Instant>,
    held: bool,
}

struct Player {
    app: Weak<AppInner>,
    memory: RefCell<Option<Memory>>,
    pending_id: RefCell<String>,
    items: RefCell<Vec<MediaItem>>,
    index: Cell<usize>,
    elapsed: Cell<f64>,
    paused: Cell<bool>,
    last: Cell<Instant>,
    tick: slint::Timer,
    hold: slint::Timer,
    press: RefCell<Press>,
    drag_y: Cell<f64>,
    epoch: Cell<u64>,
    video: RefCell<Option<crate::video::Player>>,
    muted: Cell<bool>,
}

thread_local! {
    static PLAYER: RefCell<Option<Rc<Player>>> = const { RefCell::new(None) };
}

fn get() -> Option<Rc<Player>> {
    PLAYER.with(|p| p.borrow().clone())
}

fn viewable(m: &MediaItem) -> bool {
    matches!(m.kind, MediaKind::Image | MediaKind::Video)
}

/// `spreadPick` over items
fn spread(list: Vec<MediaItem>, n: usize) -> Vec<MediaItem> {
    if list.len() <= n {
        return list;
    }
    let ids: Vec<&str> = list.iter().map(|m| m.id.as_str()).collect();
    let keep: std::collections::HashSet<String> = pa_core::spread_pick(&ids, n)
        .into_iter()
        .map(str::to_string)
        .collect();
    list.into_iter().filter(|m| keep.contains(&m.id)).collect()
}

impl Player {
    fn app(&self) -> Option<Rc<AppInner>> {
        self.app.upgrade()
    }

    fn with(&self, f: impl FnOnce(&Rc<AppInner>, &PlayerState)) {
        if let Some(a) = self.app() {
            let st = a.window.global::<PlayerState>();
            f(&a, &st);
        }
    }

    fn slides(&self) -> usize {
        let n = self.items.borrow().len();
        if n == 0 {
            0
        } else {
            n + 1
        }
    }

    fn open(self: &Rc<Self>, a: &Rc<AppInner>, m: Option<Memory>, id: &str) {
        self.epoch.set(self.epoch.get() + 1);
        *self.pending_id.borrow_mut() = id.to_string();
        *self.memory.borrow_mut() = m.clone();
        self.items.borrow_mut().clear();
        self.index.set(0);
        self.elapsed.set(0.0);
        self.paused.set(false);
        self.drag_y.set(0.0);
        let st = a.window.global::<PlayerState>();
        st.set_ready(false);
        st.set_missing(false);
        st.set_count(0);
        st.set_index(0);
        st.set_progress(0.0);
        st.set_paused(false);
        st.set_ui(true);
        st.set_drag_y(0.0);
        st.set_has_image(false);
        st.set_has_backdrop(false);
        st.set_open(true);
        self.label();
        if let Some(m) = m {
            self.fetch(a, m);
        }
    }

    fn label(&self) {
        let Some(a) = self.app() else { return };
        let st = a.window.global::<PlayerState>();
        match &*self.memory.borrow() {
            Some(m) => {
                let l = lb::memory_label(m, a.lang());
                st.set_title(l.title.into());
                st.set_subtitle(l.subtitle.into());
            }
            None => {
                st.set_title("".into());
                st.set_subtitle("".into());
            }
        }
    }

    /// items of the memory, chronological, spread to at most MAX_SLIDES
    fn fetch(self: &Rc<Self>, a: &Rc<AppInner>, m: Memory) {
        let epoch = self.epoch.get();
        let backend = a.backend.clone();
        let fut = async move {
            let base = |from, to| BrowseQuery {
                from: Some(from),
                to: Some(to),
                order: Some(SortDir::Asc),
                offset: Some(0),
                ..lb::explore_query()
            };
            if m.kind == MemoryKind::Year {
                let year = lb::ymd(m.from).0;
                let active = m
                    .months
                    .as_ref()
                    .map_or(12, |ms| ms.iter().filter(|n| **n > 0).count());
                let per = MAX_SLIDES.div_ceil(active.max(1));
                let mut out = vec![];
                for mo in 0..12i64 {
                    if m.months
                        .as_ref()
                        .is_some_and(|ms| ms.get(mo as usize).copied().unwrap_or(0) == 0)
                    {
                        continue;
                    }
                    let q = BrowseQuery {
                        limit: Some(400),
                        ..base(lb::utc(year, mo + 1, 1), lb::utc(year, mo + 2, 1))
                    };
                    let r = backend.browse(q).await?;
                    let list: Vec<MediaItem> = r.items.into_iter().filter(viewable).collect();
                    out.extend(spread(list, per));
                }
                out.truncate(MAX_SLIDES + 12);
                Ok(out)
            } else {
                let q = BrowseQuery {
                    limit: Some(2000),
                    ..base(m.from, m.to)
                };
                let r = backend.browse(q).await?;
                let list: Vec<MediaItem> = r.items.into_iter().filter(viewable).collect();
                Ok::<_, crate::backend::BackendError>(spread(list, MAX_SLIDES))
            }
        };
        let w = Rc::downgrade(self);
        a.rt.spawn_then(fut, move |r| {
            let Some(me) = w.upgrade() else { return };
            if me.epoch.get() != epoch {
                return;
            }
            match r {
                Ok(items) if !items.is_empty() => {
                    *me.items.borrow_mut() = items;
                    me.with(|_, st| {
                        st.set_ready(true);
                        st.set_count(me.slides() as i32);
                    });
                    me.show();
                    me.start_clock();
                }
                Ok(_) => me.with(|_, st| st.set_missing(true)),
                Err(e) => {
                    me.with(|a, st| {
                        st.set_missing(true);
                        a.error_toast(&e.message);
                    });
                }
            }
        });
    }

    fn cover_id(&self) -> Option<String> {
        self.memory
            .borrow()
            .as_ref()
            .and_then(|m| m.cover_ids.first().cloned())
            .or_else(|| self.items.borrow().first().map(|i| i.id.clone()))
    }

    /// publish the current slide (+ preload the next two photos)
    fn show(self: &Rc<Self>) {
        let Some(a) = self.app() else { return };
        let st = a.window.global::<PlayerState>();
        let i = self.index.get();
        st.set_index(i as i32);
        st.set_kb_b(i % 2 == 1);
        st.set_kb(0.0);
        st.set_progress(0.0);
        st.set_has_image(false);
        st.set_has_backdrop(false);
        st.set_has_frame(false);
        st.set_muted(self.muted.get());
        self.video.borrow_mut().take();
        let epoch = self.epoch.get();
        let set_img = |id: String, size: u32, back: bool| {
            let w = Rc::downgrade(self);
            let want = (i, epoch);
            crate::pages::cover(&a, &id, size, move |img: Image| {
                let Some(me) = w.upgrade() else { return };
                if (me.index.get(), me.epoch.get()) != want {
                    return;
                }
                me.with(|_, st| {
                    if back {
                        st.set_backdrop(img);
                        st.set_has_backdrop(true);
                    } else {
                        st.set_image(img);
                        st.set_has_image(true);
                    }
                });
            });
        };
        if i == 0 {
            st.set_is_title(true);
            st.set_is_video(false);
            if let Some(c) = self.cover_id() {
                set_img(c, 1600, false);
            }
        } else {
            let it = self.items.borrow().get(i - 1).cloned();
            let Some(it) = it else { return };
            st.set_is_title(false);
            st.set_is_video(it.kind == MediaKind::Video);
            let aspect = match (it.width, it.height) {
                (Some(w), Some(h)) if w > 0 && h > 0 => w as f32 / h as f32,
                _ => 1.5,
            };
            st.set_aspect(aspect);
            set_img(it.id.clone(), 320, true);
            set_img(it.id.clone(), 1600, false);
            if it.kind == MediaKind::Video {
                self.start_video(&a, &it);
            }
        }
        // preload
        for k in 1..=2 {
            let next = self.items.borrow().get(i + k - 1).cloned();
            if let Some(n) = next.filter(|n| n.kind == MediaKind::Image) {
                crate::pages::cover(&a, &n.id, 1600, |_| {});
            }
        }
    }

    fn duration(&self) -> f64 {
        let i = self.index.get();
        if i == 0 {
            return TITLE_MS;
        }
        match self.items.borrow().get(i - 1) {
            Some(it) if it.kind == MediaKind::Video => VIDEO_MS,
            _ => PHOTO_MS,
        }
    }

    fn start_video(&self, a: &Rc<AppInner>, it: &MediaItem) {
        let Some(src) = a.backend.ops().and_then(|o| o.source(it, None)) else {
            return;
        };
        let size = a.window.window().size();
        let aspect = match (it.width, it.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => w as f64 / h as f64,
            _ => 16.0 / 9.0,
        };
        let (w, h) = crate::video::output_size(
            aspect,
            size.width as f64,
            size.height as f64,
            it.width,
            it.height,
        );
        let source = crate::video::Source {
            input: src.as_input(),
            video: true,
            width: w,
            height: h,
            fps: 30.0,
            duration: it.duration,
            audio: None,
            vf_pre: Vec::new(),
            vf_post: Vec::new(),
        };
        let player =
            crate::video::Player::new(source, 0.0, !self.paused.get(), 1.0, self.muted.get(), 1.0);
        *self.video.borrow_mut() = Some(player);
    }

    /// 0..1 of the current video slide by its own clock (at most 15 s); None = use the timer
    fn video_progress(&self) -> Option<f64> {
        let mut v = self.video.borrow_mut();
        let p = v.as_mut()?;
        if let Some(f) = p.take_frame() {
            self.with(|_, st| {
                st.set_frame(Image::from_rgba8(f.px));
                st.set_has_frame(true);
            });
        }
        let s = p.status();
        if s.error.is_some() {
            return None;
        }
        let d = p.source().duration.filter(|d| *d > 0.0)?;
        if s.ended {
            return Some(1.0);
        }
        Some((p.clock() * 1000.0 / (d * 1000.0).min(VIDEO_MAX_MS)).min(1.0))
    }

    fn start_clock(self: &Rc<Self>) {
        self.last.set(Instant::now());
        let w = Rc::downgrade(self);
        self.tick.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(16),
            move || {
                if let Some(me) = w.upgrade() {
                    me.on_tick();
                }
            },
        );
    }

    fn on_tick(self: &Rc<Self>) {
        let now = Instant::now();
        let dt = now.duration_since(self.last.get()).as_secs_f64() * 1000.0;
        self.last.set(now);
        // the swipe-down offset springs back when released
        if self.press.borrow().at.is_none() && self.drag_y.get() != 0.0 {
            let y = self.drag_y.get() * (1.0 - (dt / 80.0).min(1.0));
            self.drag_y.set(if y.abs() < 0.5 { 0.0 } else { y });
            self.with(|_, st| st.set_drag_y(self.drag_y.get() as f32));
        }
        if self.paused.get() {
            return;
        }
        let dur = self.duration();
        let e = self.elapsed.get() + dt;
        self.elapsed.set(e);
        let p = self.video_progress().unwrap_or((e / dur).min(1.0));
        let kb_total = dur
            + if self.index.get() == 0 {
                1500.0
            } else {
                1200.0
            };
        self.with(|_, st| {
            st.set_progress(p as f32);
            st.set_kb((e / kb_total).min(1.0) as f32);
        });
        if p >= 1.0 {
            self.go(1);
        }
    }

    fn go(self: &Rc<Self>, d: i64) {
        let n = self.index.get() as i64 + d;
        if n < 0 {
            return;
        }
        if n as usize >= self.slides() {
            self.close();
            return;
        }
        self.index.set(n as usize);
        self.elapsed.set(0.0);
        self.show();
    }

    fn set_paused(&self, p: bool) {
        self.paused.set(p);
        self.last.set(Instant::now());
        if let Some(v) = self.video.borrow_mut().as_mut() {
            v.set_playing(!p);
        }
        self.with(|_, st| st.set_paused(p));
    }

    fn close(&self) {
        self.tick.stop();
        self.hold.stop();
        self.video.borrow_mut().take();
        self.epoch.set(self.epoch.get() + 1);
        self.with(|_, st| st.set_open(false));
    }

    fn press(self: &Rc<Self>, x: f64, y: f64) {
        *self.press.borrow_mut() = Press {
            x,
            y,
            at: Some(Instant::now()),
            held: false,
        };
        let w = Rc::downgrade(self);
        self.hold.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(HOLD_MS),
            move || {
                if let Some(me) = w.upgrade() {
                    me.press.borrow_mut().held = true;
                    me.set_paused(true);
                    me.with(|_, st| st.set_ui(false));
                }
            },
        );
    }

    fn moved(self: &Rc<Self>, x: f64, y: f64) {
        let (px, py, held) = {
            let p = self.press.borrow();
            if p.at.is_none() {
                return;
            }
            (p.x, p.y, p.held)
        };
        let dy = y - py;
        if (dy.abs() > 8.0 || (x - px).abs() > 8.0) && !held {
            self.hold.stop();
            self.press.borrow_mut().held = true;
            self.set_paused(true);
        }
        if dy > 0.0 {
            self.drag_y.set(dy);
            self.with(|_, st| st.set_drag_y(dy as f32));
        }
    }

    fn release(self: &Rc<Self>, x: f64, y: f64, width: f64) {
        self.hold.stop();
        let p = std::mem::take(&mut *self.press.borrow_mut());
        let Some(at) = p.at else { return };
        let (dx, dy) = (x - p.x, y - p.y);
        let ms = at.elapsed().as_secs_f64() * 1000.0;
        if dy > 110.0 || (dy > 50.0 && dy / ms.max(1.0) > 0.6) {
            self.close();
            return;
        }
        if p.held {
            self.set_paused(false);
            self.with(|_, st| st.set_ui(true));
            if dx.abs() > 60.0 && dx.abs() > dy.abs() {
                self.go(if dx < 0.0 { 1 } else { -1 });
            }
            return;
        }
        self.go(if x < width * 0.3 { -1 } else { 1 });
    }
}

/// wire PlayerState
pub(crate) fn install(app: &Rc<AppInner>) {
    let p = Rc::new(Player {
        app: Rc::downgrade(app),
        memory: RefCell::new(None),
        pending_id: RefCell::new(String::new()),
        items: RefCell::new(vec![]),
        index: Cell::new(0),
        elapsed: Cell::new(0.0),
        paused: Cell::new(false),
        last: Cell::new(Instant::now()),
        tick: slint::Timer::default(),
        hold: slint::Timer::default(),
        press: RefCell::new(Press::default()),
        drag_y: Cell::new(0.0),
        epoch: Cell::new(0),
        video: RefCell::new(None),
        muted: Cell::new(true),
    });
    PLAYER.with(|x| *x.borrow_mut() = Some(p.clone()));
    let st = app.window.global::<PlayerState>();
    let w = Rc::downgrade(&p);
    st.on_close(move || {
        if let Some(p) = w.upgrade() {
            p.close();
        }
    });
    let w = Rc::downgrade(&p);
    st.on_step(move |d| {
        if let Some(p) = w.upgrade() {
            if p.slides() > 0 {
                p.go(d as i64);
            }
        }
    });
    let w = Rc::downgrade(&p);
    st.on_toggle_pause(move || {
        if let Some(p) = w.upgrade() {
            p.set_paused(!p.paused.get());
        }
    });
    let w = Rc::downgrade(&p);
    st.on_toggle_mute(move || {
        if let Some(p) = w.upgrade() {
            let m = !p.muted.get();
            p.muted.set(m);
            if let Some(v) = p.video.borrow().as_ref() {
                v.set_muted(m);
            }
            p.with(|_, st| st.set_muted(m));
        }
    });
    let w = Rc::downgrade(&p);
    st.on_press(move |x, y| {
        if let Some(p) = w.upgrade() {
            p.press(x as f64, y as f64);
        }
    });
    let w = Rc::downgrade(&p);
    st.on_move(move |x, y| {
        if let Some(p) = w.upgrade() {
            p.moved(x as f64, y as f64);
        }
    });
    let w = Rc::downgrade(&p);
    st.on_release(move |x, y, width| {
        if let Some(p) = w.upgrade() {
            if p.slides() > 0 {
                p.release(x as f64, y as f64, width as f64);
            } else {
                p.press.borrow_mut().at = None;
                p.hold.stop();
            }
        }
    });
    let w = Rc::downgrade(&p);
    st.on_open_grid(move || {
        if let Some(p) = w.upgrade() {
            let id = p.memory.borrow().as_ref().map(|m| m.id.clone());
            p.close();
            if let (Some(a), Some(id)) = (p.app(), id) {
                a.go(Route::ExploreResult("memory".into(), id), true);
            }
        }
    });
    let w = Rc::downgrade(&p);
    st.on_as_slideshow(move || {
        if let Some(p) = w.upgrade() {
            let items = p.items.borrow().clone();
            p.close();
            if let Some(a) = p.app() {
                crate::slideshow_ui::open_with(&a, items);
            }
        }
    });
}

/// open the player on a memory id (from the Explore list, or rebuilt from the id)
pub(crate) fn open_id(app: &Rc<AppInner>, id: &str) {
    let Some(p) = get() else { return };
    let m = crate::explore::get().and_then(|e| {
        let d = e.data.borrow();
        d.as_ref().and_then(|r| {
            r.memories
                .iter()
                .chain(r.extras.year_reviews.iter().flatten())
                .find(|m| m.id == id)
                .cloned()
        })
    });
    let loaded = crate::explore::get().is_some_and(|e| e.data.borrow().is_some());
    let m = m.or_else(|| {
        (loaded || id.starts_with("year-"))
            .then(|| lb::memory_from_id(id, crate::fmt::now_ms()))
            .flatten()
    });
    let missing = m.is_none() && loaded;
    p.open(app, m, id);
    if missing {
        app.window.global::<PlayerState>().set_missing(true);
    }
}

/// the Explore list arrived: a player waiting for its memory can start
pub(crate) fn on_explore_data(app: &Rc<AppInner>) {
    let Some(p) = get() else { return };
    if !app.window.global::<PlayerState>().get_open() || p.memory.borrow().is_some() {
        return;
    }
    let id = p.pending_id.borrow().clone();
    if !id.is_empty() {
        open_id(app, &id);
    }
}

/// a route was entered: the player (a route of its own on the web) closes
/// Stop a playing video before thread-local storage is torn down: its audio stream and decoder
/// threads can't be shut down from a TLS destructor (the process aborts).
pub(crate) fn shutdown() {
    if let Some(p) = get() {
        p.video.borrow_mut().take();
    }
}

pub(crate) fn close() {
    if let Some(p) = get() {
        if p.app()
            .is_some_and(|a| a.window.global::<PlayerState>().get_open())
        {
            p.close();
        }
    }
}

/// `player next|prev|pause|close`
pub(crate) fn script(_app: &Rc<AppInner>, arg: &str) {
    let Some(p) = get() else { return };
    match arg {
        "next" => p.go(1),
        "prev" => p.go(-1),
        "pause" => p.set_paused(!p.paused.get()),
        "close" => p.close(),
        _ => eprintln!("[script] player next|prev|pause|close"),
    }
}
