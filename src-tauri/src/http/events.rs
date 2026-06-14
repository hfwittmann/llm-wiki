//! SSE endpoint: a per-session event stream.
//!
//! The browser opens a long-lived GET to `/api/v1/events` with its session
//! cookie. The handler registers an mpsc receiver in `SessionBus` keyed by
//! the session id, then forwards events to the client. Disconnection drops
//! the guard, which unregisters the session from the bus.
//!
//! For Phase 2 there are no senders yet — events get wired in later phases.

use std::convert::Infallible;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::Stream;
use tower_cookies::Cookies;

use crate::http::error::ApiError;
use crate::http::AppState;

pub async fn events_handler(
    State(state): State<AppState>,
    cookies: Cookies,
    _headers: HeaderMap,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    // We re-read the cookie here rather than using the AuthUser extractor
    // because SessionBus is keyed by session_id, and AuthUser doesn't carry
    // it. The middleware already validated the cookie before this handler
    // ran; the re-lookup below is belt-and-suspenders against a session
    // that expired in the tiny window between middleware and handler.
    let cookie = cookies
        .get(&state.config.session_cookie_name)
        .ok_or_else(ApiError::unauthenticated)?;
    let session_id = cookie.value().to_string();

    if state.sessions.lookup(&session_id).is_none() {
        return Err(ApiError::unauthenticated());
    }

    let rx = state.session_bus.register(&session_id);
    let guard = SessionGuard::new(state.session_bus.clone(), session_id.clone());

    let stream = async_stream::stream! {
        // Move the guard into the stream so it lives as long as the
        // connection. When the client disconnects axum drops the stream,
        // which drops the guard, which unregisters from the bus.
        let _guard = guard;
        let mut rx = rx;
        while let Some(evt) = rx.recv().await {
            let body = serde_json::to_string(&evt.data).unwrap_or_else(|_| "{}".into());
            let event = Event::default()
                .event(evt.event_type)
                .data(body);
            yield Ok::<Event, Infallible>(event);
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::new()))
}

struct SessionGuard {
    bus: crate::storage::session_bus::SessionBus,
    session_id: String,
}

impl SessionGuard {
    fn new(bus: crate::storage::session_bus::SessionBus, session_id: String) -> Self {
        Self { bus, session_id }
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.bus.unregister(&self.session_id);
    }
}
