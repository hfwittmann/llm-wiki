//! Per-session SSE event bus.
//!
//! Each active SSE connection registers an `mpsc::Sender<SseEvent>` keyed by
//! its session id. Business code (LLM streaming, ingest progress, etc.) calls
//! `bus.send_to(session_id, event)` to deliver an event to that session and
//! that session only — no cross-user broadcast.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::mpsc;

/// Bounded channel size per session. Trades a little latency for an upper
/// bound on memory if a browser pauses an SSE stream. 32 events is plenty
/// for chat-token streaming with reasonable per-tick batching.
const PER_SESSION_BUFFER: usize = 32;

#[derive(Clone, Debug, serde::Serialize)]
pub struct SseEvent {
    pub event_type: String,
    pub data: serde_json::Value,
}

#[derive(Clone, Default)]
pub struct SessionBus {
    inner: Arc<Mutex<HashMap<String, mpsc::Sender<SseEvent>>>>,
}

impl SessionBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, session_id: &str) -> mpsc::Receiver<SseEvent> {
        let (tx, rx) = mpsc::channel(PER_SESSION_BUFFER);
        self.inner.lock().insert(session_id.to_string(), tx);
        rx
    }

    pub fn unregister(&self, session_id: &str) {
        self.inner.lock().remove(session_id);
    }

    pub fn send_to(&self, session_id: &str, event: SseEvent) -> bool {
        let guard = self.inner.lock();
        let Some(sender) = guard.get(session_id) else {
            return false;
        };
        sender.try_send(event).is_ok()
    }

    #[cfg(test)]
    pub(crate) fn registered_count(&self) -> usize {
        self.inner.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn evt(t: &str) -> SseEvent {
        SseEvent { event_type: t.into(), data: json!({}) }
    }

    #[tokio::test]
    async fn register_and_send_delivers_event() {
        let bus = SessionBus::new();
        let mut rx = bus.register("sid-1");
        assert!(bus.send_to("sid-1", evt("ping")));
        let received = rx.recv().await.unwrap();
        assert_eq!(received.event_type, "ping");
    }

    #[tokio::test]
    async fn send_to_unknown_session_returns_false() {
        let bus = SessionBus::new();
        assert!(!bus.send_to("nobody", evt("ping")));
    }

    #[tokio::test]
    async fn unregister_removes_session() {
        let bus = SessionBus::new();
        let _rx = bus.register("sid-1");
        assert_eq!(bus.registered_count(), 1);
        bus.unregister("sid-1");
        assert_eq!(bus.registered_count(), 0);
        assert!(!bus.send_to("sid-1", evt("ping")));
    }

    #[tokio::test]
    async fn re_register_replaces_previous_sender() {
        let bus = SessionBus::new();
        let _rx1 = bus.register("sid-1");
        let mut rx2 = bus.register("sid-1");
        // Send: should go to rx2 (latest)
        assert!(bus.send_to("sid-1", evt("hello")));
        let received = rx2.recv().await.unwrap();
        assert_eq!(received.event_type, "hello");
    }

    #[tokio::test]
    async fn send_drops_silently_when_buffer_full() {
        let bus = SessionBus::new();
        let _rx = bus.register("sid-1");
        // Fill the buffer without draining
        for _ in 0..PER_SESSION_BUFFER {
            assert!(bus.send_to("sid-1", evt("ping")));
        }
        // Next send must fail (buffer full)
        assert!(!bus.send_to("sid-1", evt("overflow")));
    }

    #[tokio::test]
    async fn bus_is_cheaply_cloneable() {
        let bus = SessionBus::new();
        let bus2 = bus.clone();
        let _rx = bus.register("sid-1");
        assert!(bus2.send_to("sid-1", evt("ping")));
    }
}
