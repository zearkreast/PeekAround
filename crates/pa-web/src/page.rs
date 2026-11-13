//! The tab around the UI: the canvas fills it, and the back button of the browser goes back
//! inside the app before it leaves the page.
//!
//! The listeners here are called by the browser, outside the event loop of the UI. What they
//! change goes through that loop (`upgrade_in_event_loop`): it is the loop that repaints.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use slint::ComponentHandle;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

thread_local! {
    /// the extra history entry of [`back_button`] is in place
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

/// The inside of the tab (CSS pixels).
fn tab_size() -> Option<slint::LogicalSize> {
    let w = web_sys::window()?;
    let width = w.inner_width().ok()?.as_f64()?;
    let height = w.inner_height().ok()?.as_f64()?;
    Some(slint::LogicalSize::new(width as f32, height as f32))
}

/// Has the windowing backend taken the canvas over? (it makes it focusable when it does)
fn canvas_in_use() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("canvas"))
        .is_some_and(|c| c.has_attribute("tabindex"))
}

/// The window is the tab: as big as it is now, and after every resize (or zoom).
pub fn fit(window: &pa_ui::AppWindow) {
    let Some(win) = web_sys::window() else { return };
    // Not before the window exists: until then its scale factor is unknown, a size given now
    // would be converted with 1 and stay wrong on screens with another pixel ratio.
    let first = Rc::new(slint::Timer::default());
    let (weak, timer) = (window.as_weak(), first.clone());
    first.start(slint::TimerMode::Repeated, Duration::from_millis(16), move || {
        if !canvas_in_use() {
            return;
        }
        timer.stop();
        if let (Some(w), Some(s)) = (weak.upgrade(), tab_size()) {
            w.window().set_size(s);
        }
    });
    let weak = window.as_weak();
    let resized = Closure::<dyn FnMut()>::new(move || {
        let _ = weak.upgrade_in_event_loop(|w| {
            if let Some(s) = tab_size() {
                w.window().set_size(s);
            }
        });
    });
    let _ = win.add_event_listener_with_callback("resize", resized.as_ref().unchecked_ref());
    // lives as long as the page
    resized.forget();
}

fn push_entry() -> bool {
    web_sys::window()
        .and_then(|w| w.history().ok())
        .is_some_and(|h| h.push_state_with_url(&JsValue::NULL, "", None).is_ok())
}

/// Back (the button, the swipe of a phone) closes what is open in the app: the viewer, a
/// selection, the page it came from. An extra history entry keeps the tab on this page meanwhile;
/// with nothing left to go back to, back leaves the page as usual.
pub fn back_button(window: &pa_ui::AppWindow) {
    let Some(win) = web_sys::window() else { return };

    let arm = Closure::<dyn FnMut()>::new(|| {
        if !ARMED.get() && push_entry() {
            ARMED.set(true);
        }
    });
    // armed by the first click or key, as browsers ignore entries a page adds on its own
    for ev in ["pointerdown", "keydown"] {
        let _ = win.add_event_listener_with_callback(ev, arm.as_ref().unchecked_ref());
    }
    arm.forget();

    let weak = window.as_weak();
    let popped = Closure::<dyn FnMut()>::new(move || {
        if !ARMED.replace(false) {
            return;
        }
        let _ = weak.upgrade_in_event_loop(|w| {
            if w.global::<pa_ui::Shell>().invoke_back_key() {
                ARMED.set(push_entry());
            } else if let Some(h) = web_sys::window().and_then(|w| w.history().ok()) {
                let _ = h.back();
            }
        });
    });
    let _ = win.add_event_listener_with_callback("popstate", popped.as_ref().unchecked_ref());
    popped.forget();
}
