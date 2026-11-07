//! Event emission behind a trait (the server's `EventHub`), shared by pa-jobs, pa-link and pa-remote.

use pa_core::{MediaReason, ServerEvent};
use std::sync::Mutex;

/// The server's event hub (`ctx.events`). Only [`EventSink::emit`] is required; the server
/// overrides the others with its debounced / generation-counting versions.
pub trait EventSink: Send + Sync + 'static {
    /// broadcast a realtime event (`events.emit`)
    fn emit(&self, ev: ServerEvent);

    /// `events.mediaChanged(folderId, dir, reason)` — debounced per reason by the server's hub;
    /// `dir` is `"*"` for "anything in the folder". Default: an immediate `media` event.
    fn media_changed(&self, folder_id: &str, dir: &str, reason: MediaReason) {
        self.emit(ServerEvent::Media {
            folder_id: folder_id.to_string(),
            dirs: vec![dir.to_string()],
            reason: Some(reason),
        });
    }

    /// `events.bump()` — a new data generation (folders / rows vanished). Default: no-op.
    fn bump(&self) {}

    /// `scanner.onGroupsChanged()` — grouping data (explore, timeline) must be recomputed.
    fn groups_changed(&self) {}
}

/// Collects every event (tests, diagnostics).
#[derive(Default)]
pub struct MemoryEvents {
    pub events: Mutex<Vec<ServerEvent>>,
    pub bumps: std::sync::atomic::AtomicUsize,
    pub groups: std::sync::atomic::AtomicUsize,
}

impl MemoryEvents {
    pub fn take(&self) -> Vec<ServerEvent> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }
    pub fn snapshot(&self) -> Vec<ServerEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl EventSink for MemoryEvents {
    fn emit(&self, ev: ServerEvent) {
        self.events.lock().unwrap().push(ev);
    }
    fn bump(&self) {
        self.bumps
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    fn groups_changed(&self) {
        self.groups
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Drops every event.
pub struct NoEvents;

impl EventSink for NoEvents {
    fn emit(&self, _ev: ServerEvent) {}
}
