# Pre-Phase-4 fix log — bugs found in Phase 2 final review

The Phase 2 final review (commit `625735c`) flagged two issues that are **latent in Phase 2 and Phase 3** (no event senders exist yet) but **must be fixed before Phase 4** lands. Phase 4 wires real LLM-streaming and ingest-progress event senders into `SessionBus`, at which point these bugs become live.

Track this file in Phase 3 work; resolve both before any Phase 4 task that calls `SessionBus::send_to`.

---

## I1 — `SessionBus` keying causes double-unregister with concurrent tabs

**Files:** `src-tauri/src/storage/session_bus.rs`, `src-tauri/src/http/events.rs`.

**Reproduction (post-Phase-4):**

1. Alice has a valid session cookie `sid-X`.
2. She opens two browser tabs that both call `GET /api/v1/events` with that cookie.
3. Tab 1's handler calls `bus.register("sid-X")` → `inner["sid-X"] = tx1`. Its `SessionGuard` captures `"sid-X"`.
4. Tab 2's handler calls `bus.register("sid-X")` → `inner["sid-X"] = tx2` (replaces tx1). Its `SessionGuard` also captures `"sid-X"`.
5. Tab 1 closes → `guard1.drop()` → `bus.unregister("sid-X")` → removes `tx2`.
6. Tab 2 is silently unregistered. Subsequent `bus.send_to("sid-X", evt)` returns false; Tab 2 sees no events.

**Why it's latent:** Phase 2 has no callers of `send_to`. The bug only manifests when senders exist.

**Fix options:**

A. **Per-connection key** — `register()` mints a `ConnectionId` (UUID). Bus stores `HashMap<ConnectionId, (SessionId, Sender)>`. `send_to(session_id, evt)` iterates entries matching the session_id and `try_send` to each. `unregister(connection_id)` removes by connection_id. Multi-tab works cleanly; each tab gets its own delivery slot.

B. **Tokio broadcast channel per session** — bus stores `HashMap<SessionId, broadcast::Sender<SseEvent>>`. `register()` calls `.subscribe()` to get a fresh receiver. When all receivers drop, the broadcast sender stays in the map (idle) — that's fine. `send_to(session_id, evt)` calls `broadcast.send(evt)` (returns count of live receivers). No per-tab guards needed.

C. **Generation counter** — current map plus a per-session version int. Guard captures the version; `unregister` only removes if the current version matches.

Recommendation: **A**. Smallest API change, easiest to reason about, no broadcast semantics to think about.

---

## I2 — No graceful connection drain on shutdown

**File:** `src-tauri/src/bin/llm_wiki_server.rs:68–79`.

**Current behavior:** `tokio::select!` on Ctrl+C calls `main_handle.abort()` and `legacy_handle.abort()`. These are task-level aborts. In-flight HTTP requests get a hard TCP close. Long-lived SSE streams get a TCP RST rather than an EOF.

**Why it's latent in Phase 2/3:** No long-lived clients exist yet. SSE has no senders. Phase 4 changes this.

**Fix:** Replace `axum::serve(listener, app).await` with `axum::serve(listener, app).with_graceful_shutdown(shutdown_signal()).await`. `shutdown_signal()` returns a future that completes on Ctrl+C. Move the signal logic into a small helper that two listeners can share.

Sketch:

```rust
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    eprintln!("shutdown signal received");
}

let main_handle = tokio::spawn(async move {
    axum::serve(main_listener, main_app)
        .with_graceful_shutdown(shutdown_signal())
        .await
});
```

The legacy listener gets the same wrapping. Drop the `tokio::select!` + `abort()` machinery; replace with `tokio::join!` on the two handles.

---

## Resolution checkpoint

When both fixes land, this file gets a final commit deleting it (or moving to an "addressed" history doc). Phase 4's first task should not begin until this file is empty / removed.
