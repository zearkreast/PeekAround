//! `/api/events` in the browser: the page's own `WebSocket`, reconnecting with the same backoff
//! as the native transport.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use pa_core::ServerEvent;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::events::backoff;
use crate::{Client, ClientEvent, ConnectionState};

/// The open socket and the handlers it calls.
struct Conn {
    ws: web_sys::WebSocket,
    _open: Closure<dyn FnMut()>,
    _message: Closure<dyn FnMut(web_sys::MessageEvent)>,
    _close: Closure<dyn FnMut()>,
}

struct Inner {
    url: String,
    sink: Box<dyn Fn(ClientEvent)>,
    conn: RefCell<Option<Conn>>,
    retry: Cell<u32>,
    state: Cell<Option<ConnectionState>>,
    stopped: Cell<bool>,
    timer: Cell<Option<i32>>,
    reconnect: RefCell<Option<Closure<dyn FnMut()>>>,
}

impl Inner {
    fn set(&self, s: ConnectionState) {
        if self.state.get() != Some(s) {
            self.state.set(Some(s));
            (self.sink)(ClientEvent::Connection(s));
        }
    }
}

/// Live events in the browser; the socket closes when this is dropped.
pub struct WebEvents {
    inner: Rc<Inner>,
}

fn connect(me: &Rc<Inner>) {
    if me.stopped.get() {
        return;
    }
    let Ok(ws) = web_sys::WebSocket::new(&me.url) else {
        schedule(me);
        return;
    };
    let w = Rc::downgrade(me);
    let open = Closure::<dyn FnMut()>::new(move || {
        if let Some(m) = w.upgrade() {
            m.retry.set(0);
            m.set(ConnectionState::Online);
        }
    });
    ws.set_onopen(Some(open.as_ref().unchecked_ref()));
    let w = Rc::downgrade(me);
    let message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
        let Some(m) = w.upgrade() else { return };
        let Some(text) = e.data().as_string() else { return };
        match serde_json::from_str::<ServerEvent>(&text) {
            Ok(ev) => (m.sink)(ClientEvent::Server(ev)),
            // unknown / newer event types are ignored like in the web UI
            Err(e) => log::debug!("events: ignored message ({e})"),
        }
    });
    ws.set_onmessage(Some(message.as_ref().unchecked_ref()));
    let w = Rc::downgrade(me);
    let close = Closure::<dyn FnMut()>::new(move || {
        if let Some(m) = w.upgrade() {
            schedule(&m);
        }
    });
    ws.set_onclose(Some(close.as_ref().unchecked_ref()));
    // the previous socket's handlers are dropped here, never from inside one of them: `schedule`
    // only arms the timer
    *me.conn.borrow_mut() = Some(Conn {
        ws,
        _open: open,
        _message: message,
        _close: close,
    });
}

fn schedule(me: &Rc<Inner>) {
    if me.stopped.get() {
        return;
    }
    let retry = me.retry.get();
    me.set(if retry > 1 {
        ConnectionState::Offline
    } else {
        ConnectionState::Connecting
    });
    me.retry.set(retry + 1);
    let Some(win) = web_sys::window() else { return };
    let cb = me.reconnect.borrow();
    let Some(cb) = cb.as_ref() else { return };
    let id = win
        .set_timeout_with_callback_and_timeout_and_arguments_0(
            cb.as_ref().unchecked_ref(),
            backoff(retry).as_millis() as i32,
        )
        .ok();
    me.timer.set(id);
}

impl Client {
    /// Open the events socket of this page's server; `sink` runs on the UI thread for every event
    /// (the browser has no other). Reconnects until the returned handle is dropped.
    pub fn events_web(&self, sink: impl Fn(ClientEvent) + 'static) -> WebEvents {
        let inner = Rc::new(Inner {
            url: self.events_url(),
            sink: Box::new(sink),
            conn: RefCell::new(None),
            retry: Cell::new(0),
            state: Cell::new(None),
            stopped: Cell::new(false),
            timer: Cell::new(None),
            reconnect: RefCell::new(None),
        });
        let w = Rc::downgrade(&inner);
        *inner.reconnect.borrow_mut() = Some(Closure::<dyn FnMut()>::new(move || {
            if let Some(m) = w.upgrade() {
                m.timer.set(None);
                connect(&m);
            }
        }));
        inner.set(ConnectionState::Connecting);
        connect(&inner);
        WebEvents { inner }
    }
}

impl Drop for WebEvents {
    fn drop(&mut self) {
        self.inner.stopped.set(true);
        if let (Some(id), Some(win)) = (self.inner.timer.take(), web_sys::window()) {
            win.clear_timeout_with_handle(id);
        }
        if let Some(c) = self.inner.conn.borrow().as_ref() {
            c.ws.set_onclose(None);
            let _ = c.ws.close();
        }
    }
}
