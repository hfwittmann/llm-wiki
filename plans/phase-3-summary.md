# Phase 3 summary — Core extraction

**Branch:** `feat/browser-lan-port`
**Phase 2 HEAD (start):** `2bdd931`
**Phase 3 HEAD (end):** `673c7fc`
**Commits:** 10 (`git log --oneline 2bdd931..673c7fc`)

## What landed

`src-tauri/src/core/` — 9012 lines of pure-Rust business logic, zero `use tauri::*` imports, zero `use crate::commands::*` imports:

| Module | Lines | Purpose |
|---|---|---|
| `events.rs` | 81 | `EventSink` trait + `NullEventSink` + `CapturingEventSink` for tests |
| `extract/{mod,pdf,office}.rs` | 840 | PDF / Office image extraction |
| `file_sync.rs` | 1618 | File watcher with `EventSink` (12 emit calls migrated from `AppHandle`) |
| `files.rs` | 1564 | File IO, pdfium global handle (`lock_pdfium`) |
| `fs_ops.rs` | 339 | Directory listing, file preprocessing |
| `ingest_queue.rs` | 775 | Persistent serial queue (split out of file_sync) |
| `llm_client.rs` | 302 | OpenAI-compatible HTTP client + SSE streaming |
| `project.rs` | 500 | Project create/open (excluding `open_project_folder` which stays in Tauri shell) |
| `search.rs` | 1457 | Hybrid BM25 + vector search |
| `vectorstore.rs` | 1127 | LanceDB wrapper |
| `wiki.rs` | 389 | Wiki page reads, related-page lookup |
| `mod.rs` | 20 | Module declarations |

`src-tauri/src/commands/` — drastically thinned. Each former big file is now a small bag of `#[tauri::command]` wrappers that:
- Construct any needed adapters (`TauriEventSink::new(app)`).
- Call `crate::core::*::*` with typed-error returns.
- Map errors to `String` for the Tauri IPC boundary via `.map_err(|e| e.to_string())`.

| Wrapper file | Before | After |
|---|---|---|
| `commands/fs.rs` | 2178 | 114 |
| `commands/file_sync.rs` | 1810 | 75 |
| `commands/search.rs` | 1414 | 27 |
| `commands/vectorstore.rs` | 1071 | 92 |
| `commands/extract_images.rs` | 972 | 70 |
| `commands/project.rs` | 328 | 50 |

`src-tauri/src/commands/tauri_event_sink.rs` (new) — the `TauriEventSink(app: AppHandle)` adapter implementing `core::events::EventSink`.

## Tests

- 207 passing in full lib suite (was 159 at end of Phase 1, 186 at end of Phase 2).
- 48 new tests landed during Phase 3 — most are existing tests migrated alongside the code they cover. Notable new tests:
  - 5 tests using `CapturingEventSink` to verify file-watcher event order and payload shape (Task 3.7).
  - 5 tests for `LlmClient` (non-streaming + streaming + auth + error mapping, mocked with `mockito`).

## Layering invariants verified

- `grep -rn "use tauri::\|use crate::commands::" src-tauri/src/core/` returns zero matches.
- `core::extract::pdf` correctly imports `lock_pdfium` from `core::files`, not the old `commands::fs` location.
- `core::file_sync` uses `OnceLock<Mutex<FileSyncInner>>` instead of `tauri::State<FileSyncState>`.
- `core::extract` uses typed `ExtractError`; `commands/extract_images.rs` wrappers do the `.map_err(|e| e.to_string())` conversion.

## Pattern established for Phase 4

Phase 4 will mount axum handlers under `/projects`, `/wiki`, `/sources`, `/chat`, `/config`, `/fs`, `/files`, `/proxy/llm`. Each handler will:

1. Take `axum::extract::State<AppState>` + `AuthUser` (for protected routes).
2. Call into `core::*` — never `commands::*`.
3. For streaming endpoints, construct a `SessionEventSink` (Phase 4 implementation) wired to `SessionBus` and pass it as `&dyn EventSink` to `core::*`.
4. Convert typed `XError` enums into `ApiError` with appropriate HTTP status codes via per-module `From<XError> for ApiError` impls (or inline match).

The `LlmClient` is ready to be wired into `/proxy/llm`. The `EventSink` abstraction lets the same `core::file_sync::start_project_file_watcher` work for both:
- Desktop (via `TauriEventSink(app)`)
- HTTP (via `SessionEventSink(session_id, bus)` — Phase 4)

## Deviations from plan (worth noting)

- **`commands/vectorstore.rs` initially landed without typed errors** (Task 3.2). Caught in inline review; corrected in a follow-up commit. Subsequent task prompts emphasized the typed-error requirement explicitly.
- **`tauri::async_runtime::spawn_blocking` was used at exactly one site** in `fs.rs`; swapped to `tokio::task::spawn_blocking` cleanly.
- **`tauri::State<FileSyncState>` was the only `tauri::State<...>` extractor** encountered. Migrated to a module-level `OnceLock<Mutex<FileSyncInner>>` static during Task 3.7. No others needed conversion.
- **`open_project_folder`** is the only command intentionally NOT extracted; it stays in `commands/` as Tauri-shell-only and will be deleted in Phase 7.
- **`api_server.rs`** (the legacy `127.0.0.1:19828` tiny_http server) keeps calling into `core::*` functions directly. Phase 2's axum legacy listener does NOT replace it yet — that's still Phase 4. Two HTTP listeners on different ports coexist for now.
- **The `start_project_file_watcher_boxed` variant** was added during Task 3.7 because the background watcher thread needs `Arc<dyn EventSink + Send + Sync + 'static>`, while the public `start_project_file_watcher` accepts `&dyn EventSink` for tests and synchronous callers. Idiomatic Rust, but slightly off-script.

## Carryover bugs (still tracked)

`plans/phase-3-pre-phase-4-bugs.md` still lists two latent issues from Phase 2:
- **I1** — `SessionBus` keyed by session_id means concurrent browser tabs unregister each other.
- **I2** — No graceful TCP drain on Ctrl+C; SSE clients get RST instead of EOF.

Both must be fixed **before any Phase 4 task wires real event senders into `SessionBus`**. The Phase 4 plan's first task addresses both.

## Surface unchanged

The desktop Tauri app keeps working: the Tauri command surface (`invoke('foo', ...)` from the frontend) is unchanged. The desktop binary `llm-wiki` builds and runs identically to before Phase 3. Manual smoke test (open project, ingest small PDF, search, edit a wiki page) is the recommended final acceptance.
