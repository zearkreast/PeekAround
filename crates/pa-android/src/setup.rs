//! First-run screen (1.x `features/android/Setup.tsx`): "Photos on this phone" (asks for All files
//! access, then local mode) or "Connect to a PeekAround server" (LAN discovery + typed address,
//! probed with GET /api/info). Returns the chosen mode, saved to mode.json.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::config::{self, Mode};
use crate::platform;
use crate::probe::{self, ProbeError};

slint::slint! {
    export struct FoundServer { name: string, url: string }

    component Choice inherits Rectangle {
        in property <string> title;
        in property <string> hint;
        in property <image> icon;
        in property <bool> primary;
        callback clicked;
        border-radius: 24px;
        border-width: 1px;
        border-color: primary ? #8b5cf673 : #ffffff14;
        background: primary ? #8b5cf61f : #ffffff0a;
        height: lay.preferred-height;
        lay := HorizontalLayout {
            padding: 18px;
            spacing: 14px;
            Rectangle {
                width: 48px;
                height: 48px;
                border-radius: 16px;
                background: primary ? @linear-gradient(135deg, #8b5cf6 0%, #ec4899 100%) : #ffffff1f;
                Image { source: icon; width: 24px; height: 24px; colorize: white; }
            }
            VerticalLayout {
                alignment: center;
                spacing: 2px;
                Text { text: title; color: #f5f5f7; font-size: 17px; font-weight: 600; wrap: word-wrap; }
                Text { text: hint; color: #a3a3b0; font-size: 13px; wrap: word-wrap; }
            }
            Image { source: @image-url("../pa-ui/ui/icons/chevron-right.svg"); width: 20px; colorize: #6c6c7a; }
        }
        TouchArea { clicked => { root.clicked(); } }
    }

    component Btn inherits Rectangle {
        in property <string> text;
        in property <bool> primary: true;
        in property <bool> enabled: true;
        callback clicked;
        height: 48px;
        border-radius: 14px;
        background: !enabled ? #ffffff13 : primary ? @linear-gradient(90deg, #8b5cf6 0%, #ec4899 100%) : #ffffff13;
        Text { text: root.text; color: enabled ? white : #a3a3b0; font-size: 15px; font-weight: 600; }
        TouchArea { enabled: root.enabled; clicked => { root.clicked(); } }
    }

    export component SetupWindow inherits Window {
        in property <string> s-welcome;
        in property <string> s-local;
        in property <string> s-local-hint;
        in property <string> s-server;
        in property <string> s-server-hint;
        in property <string> s-back;
        in property <string> s-perm-title;
        in property <string> s-perm-body;
        in property <string> s-grant;
        in property <string> s-waiting;
        in property <string> s-found;
        in property <string> s-none-found;
        in property <string> s-manual;
        in property <string> s-connect;
        in-out property <int> step: 0;
        in property <bool> waiting;
        in property <bool> scanning;
        in property <[FoundServer]> servers;
        in property <string> error;
        in property <string> connecting;
        in-out property <string> address: "http://";
        callback choose(int);
        callback grant();
        callback rescan();
        callback connect(string, string);
        callback connected(string, string);

        background: #09090d;
        title: "PeekAround";

        Flickable {
            content-height: max(self.height, col.preferred-height);
            col := VerticalLayout {
                alignment: center;
                padding: 20px;
                padding-top: 48px;
                padding-bottom: 48px;
                spacing: 12px;
                Image {
                    source: @image-url("../pa-ui/ui/assets/logo.svg");
                    height: 72px;
                    image-fit: contain;
                }
                Text { text: "PeekAround"; color: #f5f5f7; font-size: 30px; font-weight: 700; horizontal-alignment: center; }
                Text { text: s-welcome; color: #a3a3b0; font-size: 14px; horizontal-alignment: center; wrap: word-wrap; }
                Rectangle { height: 24px; }

                if step == 0: VerticalLayout {
                    spacing: 12px;
                    Choice { title: s-local; hint: s-local-hint; icon: @image-url("../pa-ui/ui/pages/icons/smartphone.svg"); primary: true; clicked => { choose(1); } }
                    Choice { title: s-server; hint: s-server-hint; icon: @image-url("../pa-ui/ui/pages/icons/server.svg"); clicked => { choose(2); } }
                }

                if step == 1: Rectangle {
                    border-radius: 24px;
                    border-width: 1px;
                    border-color: #ffffff14;
                    background: #111118cc;
                    height: l1.preferred-height;
                    l1 := VerticalLayout {
                        padding: 22px;
                        spacing: 10px;
                        Text {
                            text: "‹ " + s-back; color: #a3a3b0; font-size: 14px;
                            TouchArea { clicked => { step = 0; } }
                        }
                        Text { text: s-perm-title; color: #f5f5f7; font-size: 20px; font-weight: 600; wrap: word-wrap; }
                        Text { text: s-perm-body; color: #a3a3b0; font-size: 14px; wrap: word-wrap; }
                        Rectangle { height: 8px; }
                        Btn { text: waiting ? s-waiting : s-grant; clicked => { grant(); } }
                    }
                }

                if step == 2: Rectangle {
                    border-radius: 24px;
                    border-width: 1px;
                    border-color: #ffffff14;
                    background: #111118cc;
                    height: l2.preferred-height;
                    l2 := VerticalLayout {
                        padding: 22px;
                        spacing: 10px;
                        Text {
                            text: "‹ " + s-back; color: #a3a3b0; font-size: 14px;
                            TouchArea { clicked => { step = 0; } }
                        }
                        HorizontalLayout {
                            Text { text: s-found; color: #f5f5f7; font-size: 16px; font-weight: 600; }
                            Image {
                                source: @image-url("../pa-ui/ui/icons/refresh-cw.svg");
                                width: 20px;
                                height: 20px;
                                colorize: scanning ? #6c6c7a : #a3a3b0;
                                TouchArea { clicked => { rescan(); } }
                            }
                        }
                        for s in servers: Rectangle {
                            height: 56px;
                            border-radius: 14px;
                            background: #ffffff0a;
                            HorizontalLayout {
                                padding-left: 14px;
                                padding-right: 14px;
                                VerticalLayout {
                                    alignment: center;
                                    Text { text: s.name; color: #f5f5f7; font-size: 15px; font-weight: 600; overflow: elide; }
                                    Text { text: connecting == s.url ? "…" : s.url; color: #a3a3b0; font-size: 12px; overflow: elide; }
                                }
                            }
                            TouchArea { clicked => { connect(s.url, s.name); } }
                        }
                        if !scanning && servers.length == 0: Text { text: s-none-found; color: #a3a3b0; font-size: 13px; wrap: word-wrap; }
                        Rectangle { height: 6px; }
                        Text { text: s-manual; color: #f5f5f7; font-size: 14px; font-weight: 600; }
                        Rectangle {
                            height: 48px;
                            border-radius: 14px;
                            border-width: 1px;
                            border-color: #ffffff29;
                            background: #ffffff0a;
                            input := TextInput {
                                x: 14px;
                                width: parent.width - 28px;
                                vertical-alignment: center;
                                text <=> root.address;
                                color: #f5f5f7;
                                font-size: 15px;
                                input-type: text;
                                single-line: true;
                                accepted => { connect(root.address, ""); }
                            }
                        }
                        if error != "": Text { text: error; color: #fb7185; font-size: 13px; wrap: word-wrap; }
                        Btn {
                            text: connecting != "" ? "…" : s-connect;
                            enabled: connecting == "";
                            clicked => { connect(root.address, ""); }
                        }
                    }
                }
            }
        }
    }
}

struct Strings {
    welcome: &'static str,
    local: &'static str,
    local_hint: &'static str,
    server: &'static str,
    server_hint: &'static str,
    back: &'static str,
    perm_title: &'static str,
    perm_body: &'static str,
    grant: &'static str,
    waiting: &'static str,
    found: &'static str,
    none_found: &'static str,
    manual: &'static str,
    connect: &'static str,
    connect_fail: &'static str,
    connect_invalid: &'static str,
    connect_not_peek: &'static str,
    connect_refused: &'static str,
}

// 1.x features/android/strings.ts (Android-only strings, kept out of the shared dictionaries)
const TR: Strings = Strings {
    welcome: "Klasörlerin, en güzel haliyle. Nasıl başlamak istersin?",
    local: "Bu telefondaki fotoğraflar",
    local_hint: "Telefonundaki klasörleri gez, düzenle, slayt ve kolaj yap.",
    server: "Bir PeekAround sunucusuna bağlan",
    server_hint: "Evdeki bilgisayar veya Docker sunucusundaki galeriye bağlan.",
    back: "Geri",
    perm_title: "Dosyalara erişim",
    perm_body: "PeekAround’un klasörlerini gösterebilmesi ve düzenlemeleri kopya olarak kaydedebilmesi için “Tüm dosyalara erişim” iznine ihtiyacı var. Açılan ekranda PeekAround’u etkinleştir ve geri dön.",
    grant: "İzin ver",
    waiting: "İzin bekleniyor…",
    found: "Ağdaki sunucular",
    none_found: "Otomatik bulunamadı. Sunucudaki Ayarlar → Ağ bölümündeki adresi aşağıya yaz.",
    manual: "Adresi elle gir",
    connect: "Bağlan",
    connect_fail: "Bağlanılamadı. Adresi, aynı Wi-Fi ağında olduğunu ve PC’de “Galeriyi yerel ağda paylaş”ın açık olduğunu kontrol et. Windows’ta güvenlik duvarının PeekAround’a Özel ağlarda izin vermesi gerekir.",
    connect_invalid: "Geçerli bir adres yaz, ör. http://192.168.1.10:1313",
    connect_not_peek: "Bu adreste PeekAround yok. PC’de Ayarlar → Ağ bölümündeki adresi (port dahil) yaz.",
    connect_refused: "Sunucu bağlantıyı reddetti (HTTP {status}).",
};

const EN: Strings = Strings {
    welcome: "Your folders, beautifully. How do you want to start?",
    local: "Photos on this phone",
    local_hint: "Browse, edit and make slideshows and collages from your phone’s folders.",
    server: "Connect to a PeekAround server",
    server_hint: "Connect to the gallery on your computer or Docker server at home.",
    back: "Back",
    perm_title: "File access",
    perm_body: "PeekAround needs “All files access” to show your folders and save edits as copies. Enable PeekAround on the next screen and come back.",
    grant: "Allow",
    waiting: "Waiting for permission…",
    found: "Servers on your network",
    none_found: "None found automatically. Type the address shown in the server’s Settings → Network.",
    manual: "Enter the address",
    connect: "Connect",
    connect_fail: "Couldn’t connect. Check the address, that you’re on the same Wi-Fi and that “Share the gallery on the local network” is on on the PC. On Windows the firewall must allow PeekAround on private networks.",
    connect_invalid: "Type a valid address, e.g. http://192.168.1.10:1313",
    connect_not_peek: "No PeekAround at this address. Type the address shown in the PC’s Settings → Network (with the port).",
    connect_refused: "The server refused the connection (HTTP {status}).",
};

fn strings(lang: &str) -> &'static Strings {
    if lang == "tr" {
        &TR
    } else {
        &EN
    }
}

/// the probe error as the message of the connect screen
fn probe_message(s: &Strings, e: &ProbeError) -> String {
    match e {
        ProbeError::Invalid => s.connect_invalid.into(),
        ProbeError::Unreachable => s.connect_fail.into(),
        ProbeError::NotPeekAround => s.connect_not_peek.into(),
        ProbeError::Refused(st) => s.connect_refused.replace("{status}", &st.to_string()),
    }
}

/// Show the first-run screen until a mode is chosen (None: the window was closed).
pub fn run(files: &Path, rt: tokio::runtime::Handle) -> Option<Mode> {
    let s = strings(platform::system_lang());
    let w = match SetupWindow::new() {
        Ok(w) => w,
        Err(e) => {
            log::error!("setup window: {e}");
            return None;
        }
    };
    w.set_s_welcome(s.welcome.into());
    w.set_s_local(s.local.into());
    w.set_s_local_hint(s.local_hint.into());
    w.set_s_server(s.server.into());
    w.set_s_server_hint(s.server_hint.into());
    w.set_s_back(s.back.into());
    w.set_s_perm_title(s.perm_title.into());
    w.set_s_perm_body(s.perm_body.into());
    w.set_s_grant(s.grant.into());
    w.set_s_waiting(s.waiting.into());
    w.set_s_found(s.found.into());
    w.set_s_none_found(s.none_found.into());
    w.set_s_manual(s.manual.into());
    w.set_s_connect(s.connect.into());

    let chosen: Rc<RefCell<Option<Mode>>> = Rc::default();
    let servers = Rc::new(VecModel::<FoundServer>::default());
    w.set_servers(ModelRc::from(servers.clone()));

    let finish = {
        let chosen = chosen.clone();
        let files = files.to_path_buf();
        move |m: Mode| {
            if let Err(e) = config::save_mode(&files, &m) {
                log::warn!("mode.json: {e}");
            }
            *chosen.borrow_mut() = Some(m);
            let _ = slint::quit_event_loop();
        }
    };

    let scan = {
        let ww = w.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            if w.get_scanning() {
                return;
            }
            w.set_scanning(true);
            let ww = ww.clone();
            rt.spawn_blocking(move || {
                let found = probe::discover(Duration::from_secs(4));
                let _ = ww.upgrade_in_event_loop(move |w| {
                    let list: Vec<FoundServer> = found
                        .into_iter()
                        .map(|(name, url)| FoundServer {
                            name: name.into(),
                            url: url.into(),
                        })
                        .collect();
                    w.set_servers(ModelRc::new(VecModel::from(list)));
                    w.set_scanning(false);
                });
            });
        }
    };

    // the All files access page is a separate system screen: poll until it was granted
    let poll = Rc::new(slint::Timer::default());
    {
        let ww = w.as_weak();
        let finish = finish.clone();
        let scan = scan.clone();
        let poll = poll.clone();
        w.on_choose(move |step| {
            let Some(w) = ww.upgrade() else { return };
            if step == 1 && platform::has_all_files() {
                finish(Mode::Local);
                return;
            }
            w.set_step(step);
            if step == 2 {
                scan();
            }
            let _ = &poll;
        });
    }
    {
        let ww = w.as_weak();
        let finish = finish.clone();
        let poll2 = poll.clone();
        w.on_grant(move || {
            if platform::has_all_files() {
                finish(Mode::Local);
                return;
            }
            platform::request_all_files();
            if let Some(w) = ww.upgrade() {
                w.set_waiting(true);
            }
            let finish = finish.clone();
            poll2.start(
                slint::TimerMode::Repeated,
                Duration::from_secs(1),
                move || {
                    if platform::has_all_files() {
                        finish(Mode::Local);
                    }
                },
            );
        });
    }
    w.on_rescan(scan);
    {
        let finish = finish.clone();
        w.on_connected(move |url, name| {
            finish(Mode::Server {
                url: url.into(),
                name: Some(name.into()),
            })
        });
    }
    {
        let ww = w.as_weak();
        let rt = rt.clone();
        w.on_connect(move |url: SharedString, name: SharedString| {
            let Some(w) = ww.upgrade() else { return };
            if !w.get_connecting().is_empty() {
                return;
            }
            w.set_error("".into());
            w.set_connecting(url.clone());
            let ww = ww.clone();
            let url = url.to_string();
            let name = name.to_string();
            // the probe runs on the runtime, the result comes back to the UI thread
            let fut = async move { probe::probe(&url).await };
            let handle = rt.spawn(fut);
            let rt2 = rt.clone();
            rt2.spawn(async move {
                let r = handle.await.unwrap_or(Err(ProbeError::Unreachable));
                let _ = ww.upgrade_in_event_loop(move |w| {
                    w.set_connecting("".into());
                    match r {
                        Ok(found) => {
                            let name = if name.is_empty() || name.starts_with("http") {
                                found.hostname
                            } else {
                                name
                            };
                            w.invoke_connected(found.url.into(), name.into());
                        }
                        Err(e) => w.set_error(probe_message(s, &e).into()),
                    }
                });
            });
        });
    }

    // desktop checks (phone-sim example): a phone-sized window and a pa-ui automation script
    #[cfg(not(target_os = "android"))]
    if let Ok(script) = std::env::var("PA_SETUP_SCRIPT") {
        w.window().set_size(slint::LogicalSize::new(412.0, 900.0));
        pa_ui::perf::run_window_script(w.as_weak(), &script);
    }
    if let Err(e) = w.run() {
        log::error!("setup: {e}");
    }
    poll.stop();
    let r = chosen.borrow_mut().take();
    if r.is_some() {
        // the main window replaces this one
        let _ = w.hide();
    }
    r
}
