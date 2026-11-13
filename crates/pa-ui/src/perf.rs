//! Verification aids: frame timing (rendering notifier), a self-driving scroll for the grid perf
//! check, and a tiny automation script (navigate, change settings, take window snapshots → PNG).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, RenderingState};

use crate::app::AppInner;
use crate::grid::GridCtl;
use crate::{GridState, Page, Shell};

/// Self-driving scroll (perf check).
#[derive(Debug, Clone, Copy)]
pub struct AutoScroll {
    /// logical px per 16 ms tick
    pub px_per_tick: f32,
    /// how long (s)
    pub seconds: f32,
    /// quit afterwards
    pub quit: bool,
}

/// Frame timings.
#[derive(Default)]
pub struct FrameStats {
    begin: Option<Instant>,
    last_frame: Option<Instant>,
    /// time between consecutive frames (ms)
    pub intervals: Vec<f32>,
    /// BeforeRendering → AfterRendering (ms)
    pub render: Vec<f32>,
    /// record only while true
    pub recording: bool,
    log: bool,
}

impl FrameStats {
    fn summary(v: &[f32]) -> String {
        if v.is_empty() {
            return "n/a".into();
        }
        let mut s = v.to_vec();
        s.sort_by(|a, b| a.total_cmp(b));
        let avg = s.iter().sum::<f32>() / s.len() as f32;
        let p = |q: f32| s[((s.len() - 1) as f32 * q) as usize];
        format!(
            "n={} avg={avg:.2} p50={:.2} p95={:.2} p99={:.2} max={:.2} ms",
            s.len(),
            p(0.5),
            p(0.95),
            p(0.99),
            s[s.len() - 1]
        )
    }
}

/// Install the rendering notifier.
pub fn install_frame_stats(window: &crate::AppWindow, log: bool) -> Rc<RefCell<FrameStats>> {
    let stats = Rc::new(RefCell::new(FrameStats {
        log,
        recording: log,
        ..Default::default()
    }));
    let s = stats.clone();
    let r = window.window().set_rendering_notifier(move |state, _| {
        let mut st = s.borrow_mut();
        if !st.recording {
            return;
        }
        match state {
            RenderingState::BeforeRendering => {
                let now = Instant::now();
                if let Some(last) = st.last_frame {
                    let dt = now.duration_since(last).as_secs_f32() * 1000.0;
                    st.intervals.push(dt);
                }
                st.last_frame = Some(now);
                st.begin = Some(now);
            }
            RenderingState::AfterRendering => {
                if let Some(b) = st.begin.take() {
                    let ms = b.elapsed().as_secs_f32() * 1000.0;
                    st.render.push(ms);
                    if st.log && st.render.len().is_multiple_of(120) {
                        log::info!(
                            "frames: render {}",
                            FrameStats::summary(&st.render[st.render.len() - 120..])
                        );
                    }
                }
            }
            _ => {}
        }
    });
    if let Err(e) = r {
        log::warn!("no rendering notifier: {e:?}");
    }
    stats
}

/// Print the frame / grid statistics (stderr).
pub fn report(stats: &Rc<RefCell<FrameStats>>, grid: &GridCtl) {
    let st = stats.borrow();
    if st.render.is_empty() {
        return;
    }
    let g = &grid.stats;
    let (vis, slots, thumbs, mb) = grid.debug_counts();
    eprintln!(
        "[perf] frame interval: {}",
        FrameStats::summary(&st.intervals)
    );
    eprintln!(
        "[perf] render (CPU, BeforeRendering→AfterRendering): {}",
        FrameStats::summary(&st.render)
    );
    eprintln!(
        "[perf] grid updates={} avg={:.1}µs max={}µs slot writes={} pages={} thumbs loaded={} | slots {vis}/{slots} cached thumbs {thumbs} ({mb} MB)",
        g.updates,
        if g.updates > 0 { g.total_us as f64 / g.updates as f64 } else { 0.0 },
        g.max_us,
        g.slot_writes,
        g.pages_loaded,
        g.thumbs_loaded
    );
}

/// Scroll the grid down (and back up at the end) every 16 ms; logs the stats when done.
pub(crate) fn autoscroll(app: &Rc<AppInner>, auto: AutoScroll, stats: Rc<RefCell<FrameStats>>) {
    let weak = Rc::downgrade(app);
    let timer = Rc::new(slint::Timer::default());
    let started: Rc<RefCell<Option<Instant>>> = Rc::new(RefCell::new(None));
    let dir = Rc::new(RefCell::new(1.0f32));
    let t2 = timer.clone();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(16),
        move || {
            let Some(a) = weak.upgrade() else { return };
            // wait until the skeleton is there
            if !a.grid.0.borrow().ready() {
                return;
            }
            let mut st = started.borrow_mut();
            let t0 = *st.get_or_insert_with(|| {
                let mut s = stats.borrow_mut();
                s.recording = true;
                s.intervals.clear();
                s.render.clear();
                s.last_frame = None;
                Instant::now()
            });
            let gs = a.window.global::<GridState>();
            let max = (a.grid.0.borrow().height() as f32 + 240.0 - 600.0).max(0.0);
            let mut y = -gs.get_scroll_y() + auto.px_per_tick * *dir.borrow();
            if y >= max {
                y = max;
                *dir.borrow_mut() = -1.0;
            } else if y <= 0.0 {
                y = 0.0;
                *dir.borrow_mut() = 1.0;
            }
            gs.set_scroll_y(-y);
            if t0.elapsed().as_secs_f32() >= auto.seconds {
                t2.stop();
                report(&stats, &a.grid.0.borrow());
                stats.borrow_mut().recording = false;
                if auto.quit {
                    let _ = slint::quit_event_loop();
                }
            }
        },
    );
    // keep the timer alive for the app's lifetime
    std::mem::forget(timer);
}

fn page_of(s: &str) -> Option<Page> {
    Some(match s {
        "photos" => Page::Photos,
        "explore" => Page::Explore,
        "favorites" => Page::Favorites,
        "albums" => Page::Albums,
        "library" => Page::Library,
        "settings" => Page::Settings,
        "search" => Page::Search,
        _ => return None,
    })
}

/// Save a snapshot of a window as PNG.
pub fn snapshot(window: &slint::Window, path: &str) {
    match window.take_snapshot() {
        Ok(buf) => {
            let (w, h) = (buf.width(), buf.height());
            match image::RgbaImage::from_raw(w, h, buf.as_bytes().to_vec()) {
                Some(img) => match img.save(path) {
                    Ok(()) => eprintln!("[shot] {path} ({w}x{h})"),
                    Err(e) => eprintln!("[shot] {path}: {e}"),
                },
                None => eprintln!("[shot] {path}: bad buffer"),
            }
        }
        Err(e) => eprintln!("[shot] {path}: {e:?}"),
    }
}

fn steps(script: &str) -> Vec<Vec<String>> {
    script
        .split([';', '\n'])
        .map(|s| s.split_whitespace().map(str::to_string).collect::<Vec<_>>())
        .filter(|v| !v.is_empty())
        .collect()
}

/// Run an automation script on the app window: `wait <ms>`, `shot <png>`, `nav <page>`,
/// `set <pref> <value>`, `search <text>`, `scroll <px>`, `section <settings section>`,
/// `toast <text>`, `quit`.
pub(crate) fn run_script(app: &Rc<AppInner>, script: &str) {
    let weak = Rc::downgrade(app);
    let steps = steps(script);
    next_step(
        Rc::new(move |cmd: &[String]| {
            let Some(a) = weak.upgrade() else { return };
            let arg = cmd.get(1).cloned().unwrap_or_default();
            match cmd[0].as_str() {
                "shot" => snapshot(a.window.window(), &arg),
                "nav" => {
                    if let Some(p) = page_of(&arg) {
                        a.navigate(p, true);
                    }
                }
                "set" => a.set_pref(&arg, cmd.get(2).map(String::as_str).unwrap_or("")),
                "search" => a.search(&cmd[1..].join(" ")),
                "scroll" => a
                    .window
                    .global::<GridState>()
                    .set_scroll_y(-arg.parse::<f32>().unwrap_or(0.0)),
                "section" => a.window.global::<Shell>().set_settings_section(arg.into()),
                "toast" => a.toast(crate::ToastKind::Success, &cmd[1..].join(" "), ""),
                "click" | "move" | "down" | "up" => pointer(a.window.window(), cmd),
                "size" => {
                    let n = |i: usize| cmd.get(i).and_then(|v| v.parse::<f32>().ok()).unwrap_or(400.0);
                    a.window.window().set_size(slint::LogicalSize::new(n(1), n(2)));
                }
                "type" => type_text(a.window.window(), &cmd[1..].join(" ")),
                "quit" => {
                    let _ = slint::quit_event_loop();
                }
                other => {
                    // viewer / selection / dnd steps
                    // go <route>, dialog <name>, view all|folders, settings-search <q>, back
                    // explore-open <key>, play <id|first>, player …, map-* steps
                    if !crate::actions_script::step(&a, cmd)
                        && !crate::pages::script_step(&a, cmd)
                        && !crate::explore::script_step(&a, cmd)
                    {
                        eprintln!("[script] unknown step {other}");
                    }
                }
            }
        }),
        steps,
        0,
    );
}

/// Script runner for any window (component gallery): `wait`, `shot`, `quit`.
pub fn run_window_script<T: ComponentHandle + 'static>(win: slint::Weak<T>, script: &str) {
    next_step(
        Rc::new(move |cmd: &[String]| {
            let Some(w) = win.upgrade() else { return };
            match cmd[0].as_str() {
                "shot" => snapshot(
                    w.window(),
                    cmd.get(1).map(String::as_str).unwrap_or("shot.png"),
                ),
                "click" | "move" => pointer(w.window(), cmd),
                "quit" => {
                    let _ = slint::quit_event_loop();
                }
                other => eprintln!("[script] unknown step {other}"),
            }
        }),
        steps(script),
        0,
    );
}

/// `click x y` / `move x y` / `down x y` / `up x y` (logical px): synthetic pointer events
fn pointer(window: &slint::Window, cmd: &[String]) {
    use slint::platform::{PointerEventButton, WindowEvent};
    let x: f32 = cmd.get(1).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let y: f32 = cmd.get(2).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let position = slint::LogicalPosition::new(x, y);
    window.dispatch_event(WindowEvent::PointerMoved { position });
    if cmd[0] == "click" || cmd[0] == "down" {
        window.dispatch_event(WindowEvent::PointerPressed {
            position,
            button: PointerEventButton::Left,
        });
    }
    if cmd[0] == "click" || cmd[0] == "up" {
        window.dispatch_event(WindowEvent::PointerReleased {
            position,
            button: PointerEventButton::Left,
        });
    }
}

/// `type <text>`: key events into the focused element (`\n` = Enter)
fn type_text(window: &slint::Window, text: &str) {
    use slint::platform::WindowEvent;
    for ch in text.replace("\\n", "\n").chars() {
        let text: slint::SharedString = ch.to_string().into();
        window.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        window.dispatch_event(WindowEvent::KeyReleased { text });
    }
}

/// executes one script step
type StepFn = Rc<dyn Fn(&[String])>;

fn next_step(exec: StepFn, steps: Vec<Vec<String>>, i: usize) {
    if i >= steps.len() {
        return;
    }
    let delay = if steps[i][0] == "wait" {
        steps[i].get(1).and_then(|s| s.parse().ok()).unwrap_or(500)
    } else {
        30
    };
    slint::Timer::single_shot(Duration::from_millis(delay), move || {
        if steps[i][0] != "wait" {
            exec(&steps[i]);
        }
        next_step(exec, steps, i + 1);
    });
}
