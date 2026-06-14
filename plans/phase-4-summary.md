# Phase 4 summary — HTTP handlers over `core::*`

**Branch:** `feat/browser-lan-port`
**Phase 3 HEAD (start):** `7d09a04`
**Phase 4 HEAD (end):** `8b0a2fc` (+ this summary commit)
**Commits:** 11 implementation + 1 plan doc + this summary

## What landed

`src-tauri/src/http/` grew from the Phase-2 skeleton (auth + events + embed) to a full feature surface:

| File | Endpoint group | New in Phase 4 |
|---|---|---|
| `projects.rs` | `/api/v1/projects/{list,open,create}` | ✓ |
| `wiki.rs` | `/api/v1/wiki/page` (GET+PUT w/ ETag), `/search`, `/graph` (501) | ✓ |
| `sources.rs` | `/api/v1/sources/{ingest,list,queue}` with SSE | ✓ |
| `chat.rs` | `/api/v1/chat/{conversations,conversation,send}` with token streaming | ✓ |
| `config.rs` | `/api/v1/config` GET+PUT | ✓ |
| `fs_browser.rs` | `/api/v1/fs/{list,mkdir}` | ✓ |
| `files.rs` | `/api/v1/files/raw` | ✓ |
| `proxy.rs` | `/api/v1/proxy/llm` non-streaming + streaming | ✓ |
| `agent.rs` | `/api/v1/agent/{projects,search,file}` mounted on both listeners | ✓ |
| `session_event_sink.rs` | `SessionEventSink` implementing `EventSink` for HTTP | ✓ |
| `error_mapping.rs` | `From<XError> for ApiError` for 10 core error types | ✓ |
| `auth.rs` (extended) | `require_auth_middleware` for routes that don't extract `AuthUser` | ✓ |

`AppState` extended to carry `llm_client: Arc<LlmClient>`.

## Tests

- **262 lib tests pass** (was 209 at end of Phase 3; +53 in Phase 4).
- Per endpoint area: 7 (auth+events from Phase 2) + 6 wiki + 4 sources + 9 chat + 4 config + 5 fs_browser + 4 files + 4 proxy + 4 agent + 5 misc = ~52 HTTP integration tests, plus the cross-cutting error_mapping and session_event_sink tests.

## End-to-end curl smoke (verified against the release binary)

All 11 steps green. Highlights:
- Login + cookie + whoami: ✓
- `GET /projects/list` enumerates real projects from `projects_root`.
- `POST /projects/open` returns `{project_id, name, path}` + updates per-user recently_opened.
- `GET /wiki/page` returns content + ETag header.
- `PUT /wiki/page` enforces `If-Match`: 400 without header, 412 stale, 200 + new ETag on match. The full optimistic-concurrency loop works.
- `GET /fs/list` and `POST /fs/mkdir` honor path safety (no `..` escape).
- `PUT /config` then `GET /config` roundtrips arbitrary JSON.
- `GET /files/raw` returns bytes with correct `Content-Type` (text/markdown for .md).
- `POST /chat/send` returns 400 `LLM_PROVIDER_NOT_CONFIGURED` when the user has no `llm` config — exactly per the error mapping.

## Cross-cutting concerns

**Path safety:** every fs-touching endpoint uses `storage::paths::resolve_under` against `state.config.projects_root` (and then again, for project-relative paths, against the resolved project root). Path-escape tests in each handler module verify the chokepoint.

**Auth on agent routes:** since `/agent/*` handlers are reachable on both listeners, they can't use `AuthUser` (which would 401 on the legacy listener). Instead, a `require_auth_middleware` was added that enforces the gate on the main listener; the legacy listener mounts the same routes without it. The MCP server on `127.0.0.1:19828` keeps working unchanged.

**SSE plumbing:** the two latent Phase-2 bugs are fixed (Task 4.0). `SessionBus` is now keyed by `ConnectionId`; concurrent tabs receive every event; closing one doesn't unregister another. The `llm-wiki-server` binary uses `axum::serve(...).with_graceful_shutdown(...)` so Ctrl+C drains in-flight requests cleanly.

**Typed errors:** every `core::*` error enum has a `From<X> for ApiError` impl that buckets the variants into 4xx/5xx with stable codes. Handlers just `?`-propagate; the HTTP error envelope is uniform across the API.

**Session event delivery:** `SessionEventSink` wraps `SessionBus`. Handlers that spawn background work (`/sources/ingest`, `/chat/send`, `/proxy/llm` streaming) construct one from the requester's session cookie and pass it as `&dyn EventSink` or `Arc<dyn EventSink + Send + Sync + 'static>` to `core::*`. Events targeted at the requester only; no cross-user broadcast.

## Deviations from plan

- The plan envisioned `core::project::open_project` returning `{schema, purpose, file_tree}`. The actual implementation returns `WikiProject { name, path }`. The HTTP wrapper enriches with `project_id` but does not currently fetch schema/purpose/file_tree. The frontend can fetch those separately via `/wiki/page` reads. Whether to roll them into the open response is a UX call for Phase 6.
- `/graph` returns 501 `NOT_IMPLEMENTED` because no backend graph function exists in `core::*`. The graph is computed client-side (Sigma + graphology). May stay 501 for v1 — Phase 5 wiring will tell us if the frontend ever needs a server-side graph.
- Originally planned `/sources/ingest/jobs/<id>` was replaced by the simpler `/sources/ingest/queue` snapshot endpoint — the existing `FileChangeQueue` shape already covers what a per-job snapshot would have provided.
- Three logically separate tasks (4.6 config, 4.7 fs_browser, 4.8 files) were folded into a single commit because they all touched `http/mod.rs` together.

## What's now possible

A `curl` script (or any HTTP client — Postman, fetch from a browser console, etc.) can exercise every feature the desktop Tauri app has:
- Manage projects (list, open, create)
- Read and edit wiki pages with proper concurrency control
- Search the corpus
- Ingest sources with live progress events
- Have an LLM conversation with token streaming
- Manage per-user config
- Browse the server-side filesystem within `projects_root`
- Preview file bytes
- Forward arbitrary OpenAI-compatible calls through the server

The browser placeholder UI at `dist/index.html` only exercises auth. The full feature surface is reachable via `curl`; Phase 5 ports the React frontend to use it.

## Carryover

None. The two pre-Phase-4 bugs were resolved in Task 4.0 and `plans/phase-3-pre-phase-4-bugs.md` was deleted. Phase 5 starts clean.

## What Phase 5 will need from Phase 4

Phase 5 rewires the React frontend from Tauri IPC to HTTP. It needs:
- The endpoint surface (this phase delivers it).
- A 1:1 mapping from `invoke('foo', args)` → `apiCall('POST', '/foo', args)`. Mostly mechanical: every `#[tauri::command]` wrapper in `commands/` has a sibling axum handler in `http/`. Where signatures differ (e.g., the wiki ETag flow vs. the older sync write), the frontend store needs minor adapter logic.
- The SSE event-type catalog: `ingest:progress`, `ingest:done`, `ingest:error`, `chat:token`, `chat:done`, `chat:error`, `proxy:token`, `proxy:done`, `proxy:error`, plus the file-watcher events from `core::file_sync` (look up the exact names in that module).
- The uniform error envelope `{error: {code, message, details}}` — frontend `api.ts` parses this and throws typed `ApiError`.
