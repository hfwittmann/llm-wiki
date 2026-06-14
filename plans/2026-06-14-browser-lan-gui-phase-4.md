# Browser/LAN GUI Phase 4 — HTTP handlers over `core::*` (Implementation Plan)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Phase goal:** Wire axum HTTP handlers for every business endpoint the design spec calls for: projects, wiki, sources/ingest (with SSE progress), chat (with token streaming), per-user config, server-side folder browser, file-preview bytes, LLM proxy, and the legacy agent surface. Each handler is thin — extracts inputs, calls into `core::*`, converts typed errors into `ApiError`, returns JSON. By the end of Phase 4, a `curl` script can exercise every feature the desktop Tauri app has. The browser UI is still the Phase-2 placeholder; the React frontend stays on Tauri IPC for now (Phase 5 rewires it).

**Architecture:**
- `src-tauri/src/http/{projects,wiki,sources,chat,config,fs_browser,files,proxy,agent}.rs` — one router-returning function per area, mounted in `http::mod::main_router` and `http::mod::legacy_router`.
- For per-session streamed events (ingest progress, chat tokens, file-watcher updates): handlers construct a `SessionEventSink(session_id, bus)` (new in this phase) and pass it as `&dyn EventSink` to `core::*`.
- Typed `XError` enums get `impl From<XError> for ApiError` so handlers do `core::foo(...).await?` and get HTTP error responses for free.
- ETag-based optimistic concurrency on wiki page PUTs (`If-Match` header → 412 on mismatch).
- The legacy `127.0.0.1:19828` listener mounts the agent-facing read-only subset of routes without auth middleware. Same handler code, two routers, two ports.

**Source spec:** `plans/2026-06-14-browser-lan-gui-design.md` (section "API surface").
**Source plan:** `plans/2026-06-14-browser-lan-gui-implementation.md` (Phase 4 outline section).
**Pre-Phase-4 carryover bugs:** `plans/phase-3-pre-phase-4-bugs.md` — fixed in Task 4.0 of this phase.

**Branch:** Continue on `feat/browser-lan-port`.

**Environment:** macOS dev. `cargo` at `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo`. Prefix:
```
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
```

---

## Phase 4 task overview

| # | Task | Outcome |
|---|---|---|
| 4.0 | Pre-Phase-4 fixes (SessionBus keying, graceful shutdown) | I1 + I2 from carryover doc resolved before any senders exist. |
| 4.1 | `SessionEventSink` + `impl From<XError> for ApiError` helpers | Plumbing: HTTP handlers can now bridge from `core::*` errors to `ApiError`. |
| 4.2 | `/projects/list`, `/open`, `/create` | Browser can enumerate, open, create projects under the projects root. |
| 4.3 | `/wiki/page` (GET/PUT w/ ETag), `/search`, `/graph` | Browser can read/write wiki pages with optimistic concurrency, search the corpus, view the relevance graph. |
| 4.4 | `/sources/ingest`, `/list`, `/ingest/jobs`, SSE wiring | Browser can ingest a source and watch progress in real time via SSE. |
| 4.5 | `/chat/conversations`, `/conversation/<id>`, `/send` (streaming) | Browser can list/load/send conversations with per-user isolation and SSE token streaming. |
| 4.6 | `/config` GET/PUT | Browser can read/write per-user config. |
| 4.7 | `/fs/list`, `/fs/mkdir` | Server-side folder browser rooted at `projects_root` (path-safety enforced). |
| 4.8 | `/files/<id>/raw` | File preview bytes streamed back with correct content-type. |
| 4.9 | `/proxy/llm` (non-streaming + streaming) | Browser/UI requests get forwarded to the user's configured LLM with API key never leaving the server. |
| 4.10 | `/agent/*` legacy surface on both listeners | Bundled MCP server keeps working on `127.0.0.1:19828`; same routes available with auth on the main listener. |
| 4.11 | Phase 4 done-check + curl smoke | Full lib green; per-endpoint curl coverage; release binary smoke; concurrent-tabs SSE smoke. |

---

# Task 4.0 — Pre-Phase-4 fixes

**Background:** `plans/phase-3-pre-phase-4-bugs.md` flagged two latent issues from Phase 2 that must be fixed before real event senders go live in the next tasks of this phase.

## I1 — `SessionBus` keying causes double-unregister with concurrent tabs

**Files:** `src-tauri/src/storage/session_bus.rs`, `src-tauri/src/http/events.rs`.

**The bug:** `SessionBus` is keyed by `session_id`. If the same session has two open SSE streams (two browser tabs), the second registration replaces the first sender; when the first tab closes, its `SessionGuard::drop` calls `unregister(session_id)` which removes the *second* tab's sender.

**The fix:** introduce a per-connection `ConnectionId`. The bus internally maps `HashMap<ConnectionId, (SessionId, Sender)>`. Public surface:

```rust
pub struct ConnectionId(String);

impl SessionBus {
    pub fn register(&self, session_id: &str) -> (ConnectionId, mpsc::Receiver<SseEvent>);
    pub fn unregister(&self, connection_id: &ConnectionId);
    pub fn send_to(&self, session_id: &str, event: SseEvent) -> usize; // returns count of receivers actually fed
}
```

- `register` returns both a new `ConnectionId` (UUID-based, 22 chars URL-safe base64) and the receiver. The connection_id is what the caller stores in its `SessionGuard`.
- `send_to` iterates all entries; for each whose `SessionId == session_id`, calls `try_send`. Returns the count of successful sends. Allows multiple subscribers for the same session (multi-tab "just works").
- `unregister(connection_id)` removes exactly that connection. Cannot accidentally remove another tab's slot.

The SSE handler in `http/events.rs`:

```rust
let (conn_id, rx) = state.session_bus.register(&session_id);
let guard = SessionGuard::new(state.session_bus.clone(), conn_id);
// ... stream rx ...
// On drop, guard removes only this connection.
```

**Tests** (add to `storage/session_bus.rs`):
- `two_concurrent_subscribers_for_same_session_both_receive_events` — register twice, send once, both receivers wake.
- `dropping_one_subscriber_does_not_remove_the_other` — register A and B, unregister A, send → B still receives.

## I2 — Graceful TCP drain on Ctrl+C

**File:** `src-tauri/src/bin/llm_wiki_server.rs`.

**The bug:** `tokio::select!` on Ctrl+C calls `main_handle.abort()` + `legacy_handle.abort()` — hard-cancels the axum task. In-flight HTTP requests and long-lived SSE streams die without an EOF.

**The fix:** Replace the `tokio::select!` + `abort()` pattern with `axum::serve(...).with_graceful_shutdown(shutdown_signal()).await` on both listeners. `shutdown_signal()` returns a future that completes on Ctrl+C. Use `tokio::join!` instead of `select!` to await both listeners.

```rust
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    eprintln!("shutdown signal received");
}

let main_fut = axum::serve(main_listener, main_app).with_graceful_shutdown(shutdown_signal());
let legacy_fut = async {
    if let Some(l) = legacy_listener {
        axum::serve(l, legacy_app).with_graceful_shutdown(shutdown_signal()).await
    } else { Ok(()) }
};
let (a, b) = tokio::join!(main_fut, legacy_fut);
a?; b?;
```

**Acceptance for Task 4.0:**
- `cargo test --lib storage::session_bus` shows the two new tests pass.
- `cargo build --bin llm-wiki-server` succeeds.
- Manual smoke: open `/api/v1/events` SSE stream in a curl, hit Ctrl+C on server — observe the stream closes cleanly (not RST).

**Commit:** `fix(http): SessionBus per-connection keying; graceful TCP drain`

After landing, **delete `plans/phase-3-pre-phase-4-bugs.md`** in the same commit (or in a follow-up tiny commit titled `chore: drop resolved pre-Phase-4 bug log`).

---

# Task 4.1 — `SessionEventSink` + error-mapping helpers

**Files to create:** `src-tauri/src/http/session_event_sink.rs`, `src-tauri/src/http/error_mapping.rs`
**Files to modify:** `src-tauri/src/http/mod.rs`

## `SessionEventSink`

Implements `core::events::EventSink` for the HTTP world. Sends to a specific session via `SessionBus`.

```rust
//! src-tauri/src/http/session_event_sink.rs
use std::sync::Arc;
use crate::core::events::EventSink;
use crate::storage::session_bus::{SessionBus, SseEvent};

#[derive(Clone)]
pub struct SessionEventSink {
    pub bus: SessionBus,
    pub session_id: Arc<str>,
}

impl SessionEventSink {
    pub fn new(bus: SessionBus, session_id: String) -> Self {
        Self { bus, session_id: Arc::from(session_id) }
    }
}

impl EventSink for SessionEventSink {
    fn emit(&self, event_type: &str, payload: serde_json::Value) {
        self.bus.send_to(&self.session_id, SseEvent {
            event_type: event_type.to_string(),
            data: payload,
        });
    }
}
```

`Arc<str>` so cloning is cheap (the same sink may be cloned across spawned tasks).

## Error mapping

For each `core::*` error enum (`VectorstoreError`, `SearchError`, `ExtractError`, `FilesError`, `WikiError`, `FsOpsError`, `ProjectError`, `FileSyncError`, `IngestQueueError`, `LlmError`), add `impl From<XError> for ApiError`. Bucket variants into HTTP status codes:

- `*Error::InvalidArgument(_)` → 400 with `code: "BAD_REQUEST"`
- `*Error::NotFound(_)` → 404 with `code: "NOT_FOUND"`
- `*Error::AlreadyExists(_)` → 409 with `code: "ALREADY_EXISTS"`
- `*Error::Io(e)` if `e.kind() == ErrorKind::NotFound` → 404; else 500
- `LlmError::UpstreamStatus { status, body }` → forward the upstream status with `code: "LLM_PROVIDER_REQUEST_FAILED"`, body in `details`
- `LlmError::Timeout` → 504 with `code: "LLM_PROVIDER_REQUEST_FAILED"`, `details.kind = "timeout"`
- `LlmError::InvalidConfig(_)` → 400 with `code: "LLM_PROVIDER_NOT_CONFIGURED"`
- All other variants → 500 with `code: "INTERNAL"`, message includes the variant's error string

Put each `impl From<...>` in `src-tauri/src/http/error_mapping.rs`. This keeps the error mapping in one place.

Mount the modules in `http/mod.rs`. Add `pub mod session_event_sink; pub mod error_mapping;` (note: `error_mapping.rs` is just impls — its `pub mod` declaration is enough to make the impls available).

**Tests:**
- `SessionEventSink::emit` actually delivers to a subscriber via `SessionBus` (smoke test in `http/session_event_sink.rs`).
- For each error mapping, a tiny test: build an error variant, `let api: ApiError = err.into()`, assert the status + code.

**Commit:** `feat(http): add SessionEventSink + From<*Error> for ApiError mappings`

---

# Task 4.2 — Projects endpoints

**File to create:** `src-tauri/src/http/projects.rs`
**File to modify:** `src-tauri/src/http/mod.rs` (mount the router)

## Endpoints

| Method | Path | Handler | Body | Returns |
|---|---|---|---|---|
| GET | `/api/v1/projects/list` | `list` | — | `[{ "id": "<canonical-path-hash>", "name": "thesis", "path": "research/thesis" }, ...]` |
| POST | `/api/v1/projects/open` | `open` | `{"path": "research/thesis"}` | `{"project_id", "schema", "purpose", "file_tree"}` |
| POST | `/api/v1/projects/create` | `create` | `{"path": "research/new", "scenario_template": "research"}` | same as `open` |

## Notes

- `list`: scans `state.config.projects_root` recursively (but only one or two levels deep — same heuristic as `core::project::list_projects` if it exists; otherwise implement a simple "directories containing `.llm-wiki/schema.md`" check).
- `open`: forwards `path` to `core::project::open_project` after `resolve_under(projects_root, path)`. On success, calls `state.user_data.add_recently_opened(user.id, project_id)`.
- `create`: similar; `scenario_template` is forwarded to `core::project::create_project`.
- All three require auth via `AuthUser`.

**`project_id`** is the **blake3 hash** of the canonical project path, hex-encoded, first 16 chars. Add `core::project::project_id_from_path(p: &Path) -> String` if it doesn't exist (just `hex::encode(&blake3::hash(canonical_bytes)[..8])`). `blake3` is already a Cargo dep (added in Task 2.1).

**Tests:**
- Happy path: open a project that exists under projects_root → 200 + payload.
- Path escape: `path = "../etc"` → 400 `PATH_ESCAPE`.
- Auth gate: no cookie → 401.
- `list` returns at least the test project.

**Commit:** `feat(http): add /projects/{list,open,create} handlers`

---

# Task 4.3 — Wiki endpoints

**File to create:** `src-tauri/src/http/wiki.rs`
**File to modify:** `src-tauri/src/http/mod.rs`

## Endpoints

| Method | Path | Handler | Notes |
|---|---|---|---|
| GET | `/api/v1/wiki/page?project_id=&path=` | `read_page` | Returns `{content, frontmatter, etag}`. Sets `ETag: "<hash>"` response header. |
| PUT | `/api/v1/wiki/page` | `write_page` | Body `{project_id, path, content}`. Requires `If-Match: <etag>`. 412 with `WIKI_PAGE_STALE` if etag mismatches. |
| POST | `/api/v1/search` | `search` | Body `{project_id, query, top_k?}`. Returns `core::search::SearchResults`. |
| GET | `/api/v1/graph?project_id=` | `graph` | Returns the relevance graph for the project. Calls `core::search::*` or equivalent. |

## ETag computation

`etag` = first 16 hex chars of `blake3::hash(content_bytes)`. Compute on read, compare on write.

## `If-Match` semantics

- Header absent → 400 `BAD_REQUEST` `"If-Match header required for wiki page writes"`.
- Header present but doesn't match current on-disk hash → 412 `WIKI_PAGE_STALE`. Include current etag in `details`.
- Match → proceed.

## Notes

- All four endpoints need `AuthUser`.
- `project_id` is the blake3 hash from Task 4.2. The handler resolves `project_id` → canonical project root (need a lookup; for now, scan recently-opened or recompute from the path the frontend remembers; simplest: have the frontend always pass `path` instead of `project_id`, OR maintain a `state.project_id_to_path: Arc<DashMap<String, PathBuf>>` populated by `open_project` calls).
  - **Pragmatic choice for v1:** the request includes both `project_id` and (implicitly via cookie + open call) the most-recently-opened path. The handler trusts the path the frontend specifies (e.g. `?project_path=...`) and computes `project_id` from it; the explicit `project_id` parameter is for the frontend's convenience. (Implementer: pick one and document.)
- Search and graph: forward to existing `core::search::*` functions.

**Tests:**
- GET page → returns content + ETag.
- PUT with matching ETag → 200, content updated.
- PUT with stale ETag → 412 `WIKI_PAGE_STALE`.
- PUT without `If-Match` → 400.
- Search returns plausible shape.

**Commit:** `feat(http): add /wiki/page (with ETag), /search, /graph`

---

# Task 4.4 — Sources & ingest endpoints (SSE wiring)

**File to create:** `src-tauri/src/http/sources.rs`
**File to modify:** `src-tauri/src/http/mod.rs`

## Endpoints

| Method | Path | Handler | Notes |
|---|---|---|---|
| POST | `/api/v1/sources/ingest` | `ingest` | Body `{project_id, source_path, hint?}`. Returns 202 `{job_id}`. Spawns the work; progress streams via SSE (`ingest:progress`, `ingest:done`). |
| GET | `/api/v1/sources/list?project_id=` | `list` | Returns sources in the project. |
| GET | `/api/v1/sources/ingest/jobs?mine=true` | `jobs` | Returns the current user's ingest jobs (status snapshot). |
| GET | `/api/v1/sources/ingest/jobs/{job_id}` | `job_status` | Snapshot of a specific job (used on SSE reconnect). |

## SSE wiring

The handler:

1. Reads the session cookie → `session_id`.
2. Constructs `SessionEventSink::new(state.session_bus.clone(), session_id)`.
3. Spawns `tokio::spawn(async move { core::sources::ingest(..., Arc::new(sink) as Arc<dyn EventSink>).await })`.
4. Returns 202 immediately with the job_id.
5. The spawned task emits `ingest:progress` events with `{job_id, phase, pct}` payload; final `ingest:done` with `{job_id, pages_changed: [...]}` payload.

`core::sources` is partly already in `core::file_sync::start_project_file_watcher` and the queue. If a dedicated `core::sources::ingest_one(project_id, source_path, sink)` function does not yet exist, the handler can drive it via `core::file_sync` + `core::ingest_queue` primitives. **Read the existing code** to find the canonical "ingest one source" entry point before writing this handler.

**Tests:**
- 401 without cookie.
- Happy path: enqueue ingest → 202 returned. (Don't try to drive the full ingest pipeline in an integration test; trust the `core` tests.)
- Job snapshot endpoint returns the queued job.

**Commit:** `feat(http): add /sources/{ingest,list,jobs} with SSE progress`

---

# Task 4.5 — Chat endpoints (token streaming)

**File to create:** `src-tauri/src/http/chat.rs`
**File to modify:** `src-tauri/src/http/mod.rs`

## Endpoints

| Method | Path | Handler | Notes |
|---|---|---|---|
| GET | `/api/v1/chat/conversations?project_id=` | `list` | Returns the current user's conversations in the project (via `UserData::list_conversations`). |
| GET | `/api/v1/chat/conversation/{id}?project_id=` | `load` | Returns the conversation JSON (via `UserData::load_conversation`). |
| POST | `/api/v1/chat/send` | `send` | Body `{project_id, conversation_id, message}`. Returns 202 `{request_id}`. Tokens stream via SSE `chat:token`; final `chat:done`. |

## Send flow

1. Load `user.id`'s LLM provider config from `state.user_data.load_config(&user.id)?` — extract `provider_config = config["llm_provider"]`. If missing → 400 `LLM_PROVIDER_NOT_CONFIGURED`.
2. Build a `core::llm_client::ProviderConfig` from the JSON.
3. Construct the request body (system prompt + project context + recent messages + new user message; details TBD by handler).
4. Construct `SessionEventSink`.
5. `tokio::spawn(async move { client.chat_completion_stream(&cfg, body, &sink).await })`.
6. On stream completion, append the user message + assistant reply to the conversation file via `UserData::save_conversation`.
7. Return 202 `{request_id}` immediately.

**Per-user isolation:** chat data is in `<data_root>/users/<uid>/chat/<project_id>/<conv_id>.json`. The handler must use `user.id` from `AuthUser`, never accept it from the request body.

**Tests:**
- List/load: 401 without cookie; happy path returns expected shape.
- Send: 400 if no LLM provider configured; 202 if configured.
- Isolation: Alice's conversations don't appear in Bob's list (already tested in Phase 1 `user_data`; one HTTP-level test confirms the wire-up).

**Commit:** `feat(http): add /chat/{list,load,send} with token streaming`

---

# Task 4.6 — Config endpoints

**File to create:** `src-tauri/src/http/config.rs`
**File to modify:** `src-tauri/src/http/mod.rs`

## Endpoints

| Method | Path | Handler | Notes |
|---|---|---|---|
| GET | `/api/v1/config` | `get` | Returns `UserData::load_config(user.id)`. |
| PUT | `/api/v1/config` | `put` | Body is the full config object. Calls `UserData::save_config`. |

Trivial. Auth-gated.

**Tests:**
- Get for new user → `{}`.
- Put roundtrips.
- Per-user isolation.

**Commit:** `feat(http): add /config get/put`

---

# Task 4.7 — Folder browser

**File to create:** `src-tauri/src/http/fs_browser.rs`
**File to modify:** `src-tauri/src/http/mod.rs`

## Endpoints

| Method | Path | Handler | Notes |
|---|---|---|---|
| GET | `/api/v1/fs/list?path=` | `list` | Returns directory entries under `projects_root/<path>`. `path` validated via `storage::paths::resolve_under(projects_root, path)`. |
| POST | `/api/v1/fs/mkdir` | `mkdir` | Body `{path}`. Creates the directory; same validation. |

## Response shape

```json
[
  {"name": "thesis", "is_dir": true, "is_project": true, "modified_unix": 1718000000},
  {"name": "scratch.txt", "is_dir": false, "is_project": false, "size": 1234}
]
```

`is_project: true` iff the directory contains `.llm-wiki/schema.md`.

**Tests:**
- Auth-gated.
- Path escape rejected with 400 `PATH_ESCAPE`.
- mkdir creates a directory and subsequent list shows it.

**Commit:** `feat(http): add /fs/{list,mkdir} server-side folder browser`

---

# Task 4.8 — File preview bytes

**File to create:** `src-tauri/src/http/files.rs`
**File to modify:** `src-tauri/src/http/mod.rs`

## Endpoint

| Method | Path | Handler | Notes |
|---|---|---|---|
| GET | `/api/v1/files/raw?project_id=&path=` | `raw` | Streams file bytes back with content-type guessed from extension via `mime_guess`. |

`path` is project-relative; the handler resolves `project_id` → project root, then calls `resolve_under(project_root, path)` and reads bytes. For very large files, use a streamed `Body`.

**Tests:**
- Auth-gated.
- Path escape rejected.
- Content-type set correctly for a `.png` and a `.md`.

**Commit:** `feat(http): add /files/raw with content-type guessing`

---

# Task 4.9 — LLM proxy

**File to create:** `src-tauri/src/http/proxy.rs`
**File to modify:** `src-tauri/src/http/mod.rs`

## Endpoints

| Method | Path | Handler | Notes |
|---|---|---|---|
| POST | `/api/v1/proxy/llm` | `proxy` | Body is `{stream: bool, body: {...}}`. Resolves the user's `ProviderConfig`, forwards via `LlmClient`. If `stream=true`, uses `SessionEventSink`; else returns the JSON inline. |

`/proxy/llm` is the explicit version of what `/chat/send` does internally. Useful for embedding generation, image captioning, etc., where the frontend just wants to delegate an OpenAI-compatible call through the server (with the API key never leaving the server).

**Tests:**
- 400 if no LLM provider config.
- Happy path non-stream: mocks a 200 response, asserts the body is forwarded.
- Streaming: mocks SSE, asserts tokens arrive on the requester's SSE channel.

**Commit:** `feat(http): add /proxy/llm non-streaming and streaming`

---

# Task 4.10 — Legacy `/agent/*` on both listeners

**File to create:** `src-tauri/src/http/agent.rs`
**File to modify:** `src-tauri/src/http/mod.rs` (especially `legacy_router`)

## Endpoints

The bundled MCP server and external agent skills (per the README) expect read-only endpoints under whatever the legacy server provided. Inspect `src-tauri/src/api_server.rs` and `src-tauri/src/clip_server.rs` to find the exact paths and shapes the MCP integration consumes. Likely subset:

| Method | Path | Handler |
|---|---|---|
| GET | `/api/v1/agent/projects` | enumerate projects (same handler as `/projects/list`, possibly trimmed shape) |
| POST | `/api/v1/agent/search` | `core::search::search_project` |
| GET | `/api/v1/agent/file?project=&path=` | file content |
| GET | `/api/v1/agent/graph?project=` | graph |

The `legacy_router` in `http/mod.rs` mounts these handlers WITHOUT auth middleware (it's localhost-only). The main listener also mounts them under the same paths WITH auth.

After this task, `src-tauri/src/api_server.rs` (the existing tiny_http server) can be removed in Phase 7. For now we leave it running on a different port if it would conflict; or, since `LLM_WIKI_LEGACY_19828_ENABLED=true` causes our axum binary to try the same port, we accept the conflict and document it in `phase-4-smoke.md`.

**Tests:**
- The legacy listener serves `/api/v1/agent/projects` without auth.
- The main listener serves the same path WITH auth.

**Commit:** `feat(http): add /agent/* legacy surface on both listeners`

---

# Task 4.11 — Phase 4 done-check + curl smoke

**Files to create / update:**
- `plans/phase-4-smoke.md` — runbook of every endpoint with example curl commands.

## Done-check

- [ ] `cargo test --lib` — full suite green (expect ≈ 230–280 tests).
- [ ] `cargo build --bin llm-wiki-server` and `cargo build --bin llm-wiki` — both succeed.
- [ ] `cargo build --release --bin llm-wiki-server` — succeeds.
- [ ] Manual smoke against the release binary: walk through the runbook in `plans/phase-4-smoke.md`. Every endpoint returns the expected shape.
- [ ] **Two-tab SSE smoke** (validates Task 4.0 I1 fix): open two browser-style SSE connections to `/api/v1/events` with the same session cookie; trigger an event; both receive it; close one; trigger again; the other still receives it.
- [ ] **Concurrent ingest smoke** (validates SSE plumbing end-to-end): in browser tab A, kick off an ingest; observe `ingest:progress` events arrive at A only (not B).
- [ ] Commit a brief Phase 4 summary at `plans/phase-4-summary.md` if there were surprises.

## Phase 4 → Phase 5 handoff

After this phase, every Tauri-IPC `invoke(...)` from the React frontend has a 1:1 HTTP equivalent. The frontend is still on Tauri IPC; Phase 5 swaps the transport layer (axum stays unchanged from Phase 4). The handoff signal is: a `curl` script can mimic every action the desktop UI takes.

---

## What Phase 5 will need from Phase 4

- The full endpoint surface from this phase, with stable shapes.
- The `SessionEventSink` + `SessionBus` per-tab plumbing — Phase 5's `src/lib/events.ts` (EventSource wrapper) will rely on this.
- The error envelope shape (`{error: {code, message, details}}`) — Phase 5's `src/lib/api.ts` will parse it.
- A documented mapping from `invoke('foo', args)` → `apiCall('POST', '/foo', args)` (or appropriate verb/path) that Phase 5 can mechanically apply. This mapping doesn't need to be a separate doc; it falls out of comparing Phase 4's routers with the existing `tauri::generate_handler!` block in `lib.rs`.
