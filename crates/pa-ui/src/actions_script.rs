//! Automation steps of the viewer / selection / drag & drop (PA_SCRIPT, see perf.rs):
//!
//! `open <grid index>` · `key <name>[+…]` (left right up down esc enter space delete f2 home end
//! plus minus or a letter; `ctrl+a`, `shift+right`) · `rclick x y` · `dblclick x y` ·
//! `mclick <ctrl|shift> x y` · `drag x0 y0 x1 y1 [steps]` · `wheel x y dy` (plain wheel) ·
//! `cwheel x y dy` (Ctrl+wheel density) · `drop <path> [path…]` (OS file drop) ·
//! `vstat` (frames presented / video status) · `seek <s>` · `sel` (print the selection).

use std::rc::Rc;
use std::time::Duration;

use slint::platform::{Key, PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, SharedString};

use crate::app::AppInner;

fn key_text(name: &str) -> SharedString {
    let k = match name {
        "left" => Some(Key::LeftArrow),
        "right" => Some(Key::RightArrow),
        "up" => Some(Key::UpArrow),
        "down" => Some(Key::DownArrow),
        "esc" | "escape" => Some(Key::Escape),
        "enter" | "return" => Some(Key::Return),
        "delete" | "del" => Some(Key::Delete),
        "backspace" => Some(Key::Backspace),
        "f2" => Some(Key::F2),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "ctrl" => Some(Key::Control),
        "shift" => Some(Key::Shift),
        "alt" => Some(Key::Alt),
        _ => None,
    };
    match (k, name) {
        (Some(k), _) => {
            let c: char = k.into();
            c.to_string().into()
        }
        (None, "space") => " ".into(),
        (None, "plus") => "+".into(),
        (None, "minus") => "-".into(),
        (None, s) => s.into(),
    }
}

fn press(w: &slint::Window, t: SharedString) {
    w.dispatch_event(WindowEvent::KeyPressed { text: t });
}
fn release(w: &slint::Window, t: SharedString) {
    w.dispatch_event(WindowEvent::KeyReleased { text: t });
}

fn num(cmd: &[String], i: usize) -> f32 {
    cmd.get(i).and_then(|v| v.parse().ok()).unwrap_or(0.0)
}

fn click(w: &slint::Window, x: f32, y: f32, button: PointerEventButton) {
    let position = LogicalPosition::new(x, y);
    w.dispatch_event(WindowEvent::PointerMoved { position });
    w.dispatch_event(WindowEvent::PointerPressed { position, button });
    w.dispatch_event(WindowEvent::PointerReleased { position, button });
}

/// Run one step; false when the step is not ours.
pub(crate) fn step(a: &Rc<AppInner>, cmd: &[String]) -> bool {
    let w = a.window.window();
    match cmd[0].as_str() {
        "open" => crate::actions_ui::open_viewer(num(cmd, 1) as usize),
        "key" => {
            let combo = cmd.get(1).cloned().unwrap_or_default();
            let parts: Vec<&str> = combo.split('+').filter(|p| !p.is_empty()).collect();
            let parts = if combo.ends_with("++") || combo == "+" {
                let mut v: Vec<&str> = combo
                    .trim_end_matches('+')
                    .split('+')
                    .filter(|p| !p.is_empty())
                    .collect();
                v.push("plus");
                v
            } else {
                parts
            };
            let keys: Vec<SharedString> = parts.iter().map(|p| key_text(p)).collect();
            for k in &keys {
                press(w, k.clone());
            }
            for k in keys.iter().rev() {
                release(w, k.clone());
            }
        }
        "rclick" => click(w, num(cmd, 1), num(cmd, 2), PointerEventButton::Right),
        "dblclick" => {
            click(w, num(cmd, 1), num(cmd, 2), PointerEventButton::Left);
            click(w, num(cmd, 1), num(cmd, 2), PointerEventButton::Left);
        }
        "mclick" => {
            let m = key_text(cmd.get(1).map(String::as_str).unwrap_or("ctrl"));
            press(w, m.clone());
            click(w, num(cmd, 2), num(cmd, 3), PointerEventButton::Left);
            release(w, m);
        }
        "drag" => {
            let (x0, y0, x1, y1) = (num(cmd, 1), num(cmd, 2), num(cmd, 3), num(cmd, 4));
            let n = cmd
                .get(5)
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(12)
                .max(1);
            let p0 = LogicalPosition::new(x0, y0);
            w.dispatch_event(WindowEvent::PointerMoved { position: p0 });
            w.dispatch_event(WindowEvent::PointerPressed {
                position: p0,
                button: PointerEventButton::Left,
            });
            // the moves are spread over time (velocity / timers see a real gesture)
            let weak = Rc::downgrade(a);
            for k in 1..=n {
                let f = k as f32 / n as f32;
                let p = LogicalPosition::new(x0 + (x1 - x0) * f, y0 + (y1 - y0) * f);
                let weak = weak.clone();
                slint::Timer::single_shot(Duration::from_millis(16 * k as u64), move || {
                    if let Some(a) = weak.upgrade() {
                        let w = a.window.window();
                        w.dispatch_event(WindowEvent::PointerMoved { position: p });
                        if k == n {
                            w.dispatch_event(WindowEvent::PointerReleased {
                                position: p,
                                button: PointerEventButton::Left,
                            });
                        }
                    }
                });
            }
        }
        "wheel" => {
            let position = LogicalPosition::new(num(cmd, 1), num(cmd, 2));
            w.dispatch_event(WindowEvent::PointerMoved { position });
            w.dispatch_event(WindowEvent::PointerScrolled {
                position,
                delta_x: 0.0,
                delta_y: num(cmd, 3),
            });
        }
        "cwheel" => crate::actions_ui::simulate_density_wheel(
            num(cmd, 3) as f64,
            num(cmd, 1) as f64,
            num(cmd, 2) as f64,
        ),
        "drop" => crate::actions_ui::simulate_drop(cmd[1..].iter().map(Into::into).collect()),
        "vstat" => {
            let vs = a.window.global::<crate::ViewerState>();
            eprintln!(
                "[video] presented={} playing={} position={:.2}/{:.2} has_frame={} no_audio_device={} error={:?}",
                crate::video::presented(),
                vs.get_playing(),
                vs.get_position(),
                vs.get_duration(),
                vs.get_has_frame(),
                vs.get_no_audio_device(),
                vs.get_video_error().to_string()
            );
        }
        "vtime" => {
            let mut v = crate::video::take_intervals();
            v.sort_by(|a, b| a.total_cmp(b));
            if !v.is_empty() {
                let p = |q: f32| v[((v.len() - 1) as f32 * q) as usize];
                let avg = v.iter().sum::<f32>() / v.len() as f32;
                eprintln!(
                    "[video] frame intervals n={} avg={avg:.1} p50={:.1} p95={:.1} max={:.1} ms → {:.1} fps",
                    v.len(),
                    p(0.5),
                    p(0.95),
                    v[v.len() - 1],
                    1000.0 / avg
                );
            }
        }
        "seek" => {
            let vs = a.window.global::<crate::ViewerState>();
            let d = vs.get_duration().max(0.001);
            vs.invoke_seek(num(cmd, 1) / d);
        }
        // photo / video editor on grid item N; `estate` / `vestate` print their state
        "edit" => {
            let n = num(cmd, 1) as usize;
            let weak = Rc::downgrade(a);
            a.grid.0.borrow_mut().items_in_range(n, n, move |list| {
                if let (Some(a), Some(item)) = (weak.upgrade(), list.into_iter().next()) {
                    if item.kind == pa_core::MediaKind::Video {
                        crate::video_editor::open(&a, item);
                    } else {
                        crate::editor::open(&a, item);
                    }
                }
            });
        }
        "estate" => crate::editor::debug_state(),
        "vestate" => crate::video_editor::debug_state(),
        "sel" => {
            let g = a.grid.0.borrow();
            let ids = g.ix.selection.ids();
            eprintln!(
                "[sel] {} selected: {:?}",
                ids.len(),
                ids.iter().take(12).collect::<Vec<_>>()
            );
        }
        _ => return false,
    }
    true
}
