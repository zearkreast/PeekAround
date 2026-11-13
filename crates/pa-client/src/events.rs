//! `/api/events` WebSocket with reconnect (native transport: tokio-tungstenite; the browser's
//! own `WebSocket` in `events_web.rs`).

use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use futures_util::StreamExt;
use pa_core::ServerEvent;
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::mpsc;
#[cfg(not(target_arch = "wasm32"))]
use tokio_tungstenite::tungstenite::Message;

#[cfg(not(target_arch = "wasm32"))]
use crate::{encode_seg, Client};

/// Connection state of the events socket (the web UI's `ConnectionState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    /// (re)connecting — the first retries
    Connecting,
    /// connected
    Online,
    /// several reconnects failed in a row
    Offline,
}

/// What [`Events`] delivers.
#[derive(Debug, Clone, PartialEq)]
// events are rare: no need to box the big `ServerEvent`
#[allow(clippy::large_enum_variant)]
pub enum ClientEvent {
    /// the connection state changed
    Connection(ConnectionState),
    /// a server event (`hello` first after every (re)connect)
    Server(ServerEvent),
}

/// Live events; the background task stops when this is dropped.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct Events {
    rx: mpsc::UnboundedReceiver<ClientEvent>,
    task: tokio::task::JoinHandle<()>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Events {
    /// next event (None once the task ended, which only happens on drop)
    pub async fn recv(&mut self) -> Option<ClientEvent> {
        self.rx.recv().await
    }

    /// next event without waiting
    pub fn try_recv(&mut self) -> Option<ClientEvent> {
        self.rx.try_recv().ok()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Events {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Reconnect delay after `retry` failed attempts: 500 ms · 2ⁿ, at most 15 s (web UI).
pub(crate) fn backoff(retry: u32) -> Duration {
    Duration::from_millis((500u64 << retry.min(10)).min(15_000))
}

#[cfg(not(target_arch = "wasm32"))]
impl Client {
    /// Open the events socket on the current tokio runtime (reconnects until dropped).
    pub fn events(&self) -> Events {
        self.events_on(&tokio::runtime::Handle::current())
    }

    /// [`Client::events`] on a given runtime.
    pub fn events_on(&self, rt: &tokio::runtime::Handle) -> Events {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut url = self.events_url();
        if let Some(a) = self.device_auth() {
            url = format!(
                "{url}?pdev={}&ptok={}",
                encode_seg(&a.device_id),
                encode_seg(&a.token)
            );
        }
        let task = rt.spawn(run(url, tx));
        Events { rx, task }
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn run(url: String, tx: mpsc::UnboundedSender<ClientEvent>) {
    let mut retry: u32 = 0;
    let mut state = None;
    let set = |s: ConnectionState, state: &mut Option<ConnectionState>| {
        if *state != Some(s) {
            *state = Some(s);
            let _ = tx.send(ClientEvent::Connection(s));
        }
    };
    set(ConnectionState::Connecting, &mut state);
    loop {
        match tokio_tungstenite::connect_async(url.as_str()).await {
            Ok((mut ws, _)) => {
                retry = 0;
                set(ConnectionState::Online, &mut state);
                while let Some(msg) = ws.next().await {
                    match msg {
                        Ok(Message::Text(t)) => match serde_json::from_str::<ServerEvent>(&t) {
                            Ok(e) => {
                                if tx.send(ClientEvent::Server(e)).is_err() {
                                    return;
                                }
                            }
                            // unknown / newer event types are ignored like in the web UI
                            Err(e) => log::debug!("events: ignored message ({e})"),
                        },
                        Ok(Message::Close(_)) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
            }
            Err(e) => log::debug!("events: connect failed: {e}"),
        }
        if tx.is_closed() {
            return;
        }
        set(
            if retry > 1 {
                ConnectionState::Offline
            } else {
                ConnectionState::Connecting
            },
            &mut state,
        );
        tokio::time::sleep(backoff(retry)).await;
        retry += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_caps() {
        assert_eq!(backoff(0), Duration::from_millis(500));
        assert_eq!(backoff(3), Duration::from_millis(4000));
        assert_eq!(backoff(5), Duration::from_millis(15_000));
        assert_eq!(backoff(40), Duration::from_millis(15_000));
    }
}
