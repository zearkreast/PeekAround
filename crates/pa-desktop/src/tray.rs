//! Tray icon (main.ts `updateTray`): shown while it is needed (LAN share on, a phone connected,
//! the window closed into the tray, or close action "tray"). Menu: PeekAround · the LAN addresses
//! (open in the browser) · Show · Stop LAN sharing (while on) · Quit, in the UI language.
//! Windows: `tray-icon` (Win32, on the UI thread); Linux: `ksni` (StatusNotifierItem over D-Bus,
//! no GTK).

use std::sync::Arc;

use crate::desktop::Shared;
use crate::prefs::l;

#[derive(Debug, Clone)]
pub enum Entry {
    /// disabled title
    Title(String),
    Url(String),
    Sep,
    Show(String),
    StopLan(String),
    Quit(String),
}

/// the menu for the current state
pub fn entries(s: &Shared) -> Vec<Entry> {
    let lg = s.lang();
    let mut v = vec![Entry::Title("PeekAround".into())];
    for u in s.lan_urls() {
        v.push(Entry::Url(u));
    }
    v.push(Entry::Sep);
    v.push(Entry::Show(l(lg, "Göster", "Show")));
    if s.lan_on() {
        v.push(Entry::StopLan(l(
            lg,
            "Ağ paylaşımını durdur",
            "Stop LAN sharing",
        )));
    }
    v.push(Entry::Quit(l(lg, "Çık", "Quit")));
    v
}

/// run a menu entry
pub fn activate(s: &Arc<Shared>, e: &Entry) {
    match e {
        // the LAN address is for the phone app (the LAN listener serves no web UI to browsers):
        // show the window, whose Settings → Network has the QR code
        Entry::Url(_) => s.show_window(),
        Entry::Show(_) => s.show_window(),
        Entry::StopLan(_) => {
            let s2 = s.clone();
            s.rt.spawn(async move {
                let port = s2.prefs.lock().unwrap().lan.port;
                s2.set_lan(false, port).await;
            });
        }
        Entry::Quit(_) => s.quit(),
        Entry::Title(_) | Entry::Sep => {}
    }
}

/// RGBA pixels of the tray icon
fn icon_rgba() -> (u32, u32, Vec<u8>) {
    let img = image::load_from_memory(include_bytes!("../assets/tray-32.png"))
        .map(|i| i.to_rgba8())
        .unwrap_or_else(|_| image::RgbaImage::new(32, 32));
    (img.width(), img.height(), img.into_raw())
}

// ───────── Linux: ksni ─────────

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::*;
    use ksni::TrayMethods;

    struct PaTray {
        s: Arc<Shared>,
        items: Vec<Entry>,
    }

    impl ksni::Tray for PaTray {
        fn id(&self) -> String {
            "peekaround".into()
        }
        fn title(&self) -> String {
            "PeekAround".into()
        }
        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            let (w, h, rgba) = icon_rgba();
            // ARGB32, network byte order
            let mut data = Vec::with_capacity(rgba.len());
            for px in rgba.chunks_exact(4) {
                data.extend_from_slice(&[px[3], px[0], px[1], px[2]]);
            }
            vec![ksni::Icon {
                width: w as i32,
                height: h as i32,
                data,
            }]
        }
        fn activate(&mut self, _x: i32, _y: i32) {
            self.s.show_window();
        }
        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::StandardItem;
            self.items
                .iter()
                .map(|e| match e {
                    Entry::Sep => ksni::MenuItem::Separator,
                    Entry::Title(t) => StandardItem {
                        label: t.clone(),
                        enabled: false,
                        ..Default::default()
                    }
                    .into(),
                    other => {
                        let label = match other {
                            Entry::Url(s) | Entry::Show(s) | Entry::StopLan(s) | Entry::Quit(s) => {
                                s.clone()
                            }
                            _ => String::new(),
                        };
                        let e2 = other.clone();
                        StandardItem {
                            label,
                            activate: Box::new(move |t: &mut PaTray| activate(&t.s, &e2)),
                            ..Default::default()
                        }
                        .into()
                    }
                })
                .collect()
        }
    }

    /// keep the tray in sync with [`Shared::tray_needed`]
    pub fn install(s: &Arc<Shared>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        *s.tray_refresh.lock().unwrap() = Some(Box::new(move || {
            let _ = tx.send(());
        }));
        let s2 = s.clone();
        s.rt.spawn(async move {
            let mut handle: Option<ksni::Handle<PaTray>> = None;
            loop {
                let need = s2.tray_needed();
                match (&handle, need) {
                    (None, true) => {
                        let t = PaTray {
                            s: s2.clone(),
                            items: entries(&s2),
                        };
                        match t.disable_dbus_name(false).spawn().await {
                            Ok(h) => handle = Some(h),
                            Err(e) => log::warn!("tray: {e}"),
                        }
                    }
                    (Some(h), true) => {
                        let items = entries(&s2);
                        h.update(move |t| t.items = items).await;
                    }
                    (Some(h), false) => {
                        h.shutdown().await;
                        handle = None;
                    }
                    (None, false) => {}
                }
                if rx.recv().await.is_none() {
                    break;
                }
                // coalesce bursts
                while rx.try_recv().is_ok() {}
            }
        });
        s.refresh_tray();
    }
}

// ───────── Windows: tray-icon ─────────

#[cfg(windows)]
mod imp {
    use super::*;
    use std::cell::RefCell;
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};

    thread_local! {
        static TRAY: RefCell<Option<(TrayIcon, Vec<(tray_icon::menu::MenuId, Entry)>)>> = const { RefCell::new(None) };
    }

    fn rebuild(s: &Arc<Shared>) {
        let need = s.tray_needed();
        TRAY.with(|t| {
            let mut t = t.borrow_mut();
            if !need {
                *t = None;
                return;
            }
            let menu = Menu::new();
            let mut ids = Vec::new();
            for e in entries(s) {
                match &e {
                    Entry::Sep => {
                        let _ = menu.append(&PredefinedMenuItem::separator());
                    }
                    Entry::Title(x) => {
                        let _ = menu.append(&MenuItem::new(x, false, None));
                    }
                    Entry::Url(x) | Entry::Show(x) | Entry::StopLan(x) | Entry::Quit(x) => {
                        let it = MenuItem::new(x, true, None);
                        ids.push((it.id().clone(), e.clone()));
                        let _ = menu.append(&it);
                    }
                }
            }
            match t.as_mut() {
                Some((icon, old)) => {
                    icon.set_menu(Some(Box::new(menu)));
                    *old = ids;
                }
                None => {
                    let (w, h, rgba) = icon_rgba();
                    let icon = tray_icon::Icon::from_rgba(rgba, w, h).ok();
                    let mut b = TrayIconBuilder::new()
                        .with_tooltip("PeekAround")
                        .with_menu(Box::new(menu))
                        .with_menu_on_left_click(false);
                    if let Some(i) = icon {
                        b = b.with_icon(i);
                    }
                    match b.build() {
                        Ok(icon) => *t = Some((icon, ids)),
                        Err(e) => log::warn!("tray: {e}"),
                    }
                }
            }
        });
    }

    /// keep the tray in sync with [`Shared::tray_needed`] (the icon lives on the UI thread)
    pub fn install(s: &Arc<Shared>) {
        let s2 = s.clone();
        *s.tray_refresh.lock().unwrap() = Some(Box::new(move || {
            let s3 = s2.clone();
            let _ = slint::invoke_from_event_loop(move || rebuild(&s3));
        }));
        let s2 = s.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let s3 = s2.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let entry = TRAY.with(|t| {
                    t.borrow().as_ref().and_then(|(_, ids)| {
                        ids.iter()
                            .find(|(id, _)| *id == e.id)
                            .map(|(_, e)| e.clone())
                    })
                });
                if let Some(en) = entry {
                    activate(&s3, &en);
                }
            });
        }));
        let s2 = s.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: tray_icon::MouseButton::Left,
                button_state: tray_icon::MouseButtonState::Up,
                ..
            } = e
            {
                s2.show_window();
            }
        }));
        s.refresh_tray();
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    /// (no tray on macOS yet)
    pub fn install(_s: &Arc<Shared>) {}
}

pub use imp::install;
