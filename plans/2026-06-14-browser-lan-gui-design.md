# Browser/LAN GUI for LLM Wiki — Design

**Date:** 2026-06-14
**Status:** Approved for implementation planning
**Author:** brainstormed with Claude (Opus 4.7)

## Goal

Replace the existing Tauri desktop shell with a browser-accessible GUI served from a single Rust HTTP binary. The server runs on a VM (or any host) and is reached from a browser on the LAN by any of a small number of trusted users. Each user logs in with their own account, has their own chat history and LLM provider configuration, but shares the same projects, wiki content, source documents, and vector store with everyone else. Ingestion of any document is done once and the result is visible to every user.

## Non-goals

- No desktop binary in any form. The Tauri shell is removed entirely.
- No cross-user real-time notifications or collaborative editing UI. Shared state updates are picked up on navigation or manual refresh.
- No project ACLs. Every authenticated user sees and can act on every project.
- No mobile-first UI work. The existing layout is desktop-shaped; it works in a desktop browser, that's all v1 commits to.
- No HTTPS termination in the binary. If TLS is needed later, front the server with nginx/caddy.
- No multi-tenant deployment story. One server per group.

## Decisions (with rationale where non-obvious)

1. **Drop the Tauri desktop shell entirely.** Keeping both shells has small per-feature costs but a fatal model mismatch — desktop is single-user with no auth, the LAN version is multi-user with auth. Maintaining two mental models indefinitely is more painful than the one-time port.
2. **Big-bang port on a feature branch.** User preference is speed over intermediate working states. The desktop app keeps working on `main` while the port happens on the branch; merge when end-to-end runnable.
3. **Single binary, frontend embedded via `rust-embed`.** One file to deploy. The Vite build's `dist/` is slurped into the binary at compile time; the server serves `/` and falls back to `index.html` for SPA routes. No nginx, no separate static host. Cost (rebuild binary to ship frontend changes) is irrelevant at this scale.
4. **`axum` replaces `tiny_http`.** axum provides middleware, typed extractors, SSE, JSON ergonomics, and integrates with the tokio runtime we already pull in. `tiny_http` was right for the old localhost-only read-mostly API; wrong for what we need now.
5. **Two listeners, one router.** The main listener binds `0.0.0.0:<configured>` with auth middleware. A secondary listener stays bound to `127.0.0.1:19828` without auth middleware, serving the legacy agent-facing handlers. The bundled MCP server and external `llm_wiki_skill` keep working unchanged. Threat model is unchanged from today.
6. **Per-user accounts (`users.toml`), shared password is not enough.** Admin-managed file, argon2 password hashing, no signup flow. The shared-password approach was ruled out because per-user state (chat history, LLM API keys) cannot safely live behind a single shared credential.
7. **Persisted sessions via `sled`, 30-day cookie, `HttpOnly` + `SameSite=Lax`.** In-memory sessions force re-login on every server restart, which is noisy during development and forgettable in production. `sled` adds one dep and ~30 lines for a much better UX.
8. **Shared projects, shared wiki, shared sources, shared vector store, shared ingest queue.** Stored under the configured `projects_root`. Per-user data lives only under `data_root/users/<uid>/`.
9. **Per-session SSE streams, no cross-user broadcast.** Streaming is only for the requester's own operations (LLM tokens, ingest progress). Other users see changes on their next refresh. This is "live updates not critical" formalized.
10. **Hybrid folder model.** Admin sets `projects_root`; users browse and create subprojects under it from the browser. Path-traversal safety is enforced through a single chokepoint function.
11. **Server-side LLM/embedding proxy.** All LLM and embedding calls originate from the server. User API keys never reach the browser; CORS issues disappear.
12. **First-cut scope cuts:** CLI subprocess transport (claude/codex) is out — only HTTP-API LLM providers are supported in v1; per-user CLI auth on a shared VM is a rabbit hole worth deferring. Chrome Web Clipper out; scheduled imports out; autostart out (server is a daemon). File preview and embedding proxy in.

## Architecture

### High-level

```
[ Linux/macOS VM (or any host) ]
   └── llm-wiki-server  (single Rust binary, embedded frontend)
        ├── HTTP listener 0.0.0.0:<port>   → full UI API, auth required
        ├── HTTP listener 127.0.0.1:19828  → legacy read-only API, no auth (back-compat)
        ├── core/         pure Rust business logic, no axum, no Tauri
        ├── http/         axum handlers (thin, delegate to core/)
        ├── auth/         argon2 + sled sessions
        ├── storage/      per-user data, session bus, paths chokepoint
        └── embed/        rust-embed frontend bundle

[ Any browser on the LAN ]
   └── React SPA — fetch() and EventSource — points at vm-ip:<port>
```

### Crate layout

Single binary, no workspace split for v1 (easy to split later if useful).

```
src-server/                      (renamed from src-tauri)
└── src/
    ├── main.rs                  binary entry; parse config; bind listeners
    ├── core/                    pure Rust business logic
    │   ├── mod.rs
    │   ├── wiki.rs              page read/write, frontmatter, wikilinks
    │   ├── sources.rs           ingest pipeline orchestration
    │   ├── ingest_queue.rs      persistent serial queue (existing, ported)
    │   ├── search.rs            hybrid BM25 + vector + graph
    │   ├── graph.rs             Louvain, 4-signal relevance, insights
    │   ├── lint.rs              contradiction / orphan / stale detection
    │   ├── extract/             pdfium, calamine, docx parsers (ported)
    │   ├── vectorstore.rs       lancedb wrapper (ported)
    │   ├── llm_client.rs        HTTP client to OpenAI-compatible endpoints
    │   └── project.rs           project open/create, schema/purpose loading
    ├── http/                    axum layer
    │   ├── mod.rs               router assembly, middleware stack
    │   ├── auth.rs              login, logout, whoami, session middleware
    │   ├── projects.rs          list / open / create
    │   ├── wiki.rs              CRUD + search endpoints
    │   ├── sources.rs           ingest, list, file content
    │   ├── chat.rs              history (per-user), send (returns stream id)
    │   ├── config.rs            per-user GET/PUT
    │   ├── fs_browser.rs        rooted folder browser
    │   ├── files.rs             /files/<id>/raw — file preview bytes
    │   ├── proxy.rs             /proxy/llm — server-side LLM/embedding proxy
    │   ├── events.rs            SSE per-session stream
    │   └── agent_api.rs         legacy 19828 surface (mounted on both listeners)
    ├── auth/
    │   ├── users.rs             load/save users.toml, argon2
    │   ├── sessions.rs          sled-backed session table, cookie auth
    │   └── request_ctx.rs       axum extractor returning User
    ├── storage/
    │   ├── user_data.rs         per-user config + chat history
    │   ├── session_bus.rs       per-session mpsc broadcaster
    │   └── paths.rs             resolve_under chokepoint, path-traversal safety
    └── embed/
        └── frontend.rs          rust-embed bundle, SPA fallback to index.html
```

### `core/` ↔ `http/` separation

- `core/` contains all business logic. It does not depend on axum, on Tauri, or on `tokio::net`. It takes plain inputs and returns plain outputs.
- Functions that produce streamed events take an `EventSink` trait parameter defined in `core/`. The HTTP layer provides a session-mpsc-backed implementation in `http/events.rs`; tests provide a `Vec<Event>`-capturing implementation.
- `http/` handlers are thin: extract inputs, call a `core/` function, serialize the output, set status. Auth check is enforced by middleware, never inside a handler.
- The dual listener works because the same router is mounted twice in `main.rs` — once on `0.0.0.0:<port>` with auth middleware, once on `127.0.0.1:19828` without. Same handlers underneath.
- The legacy `127.0.0.1:19828` listener is opt-out via `LEGACY_19828_ENABLED=false` (defaults to enabled for back-compat with the bundled MCP server). When disabled, the binary only opens the main listener; agent integrations stop working until reconfigured.

## Data layout on disk

### Shared per-project

```
<projects_root>/<project>/
   ├── wiki/                    shared markdown files (the wiki itself)
   ├── raw/                     shared source documents
   ├── .llm-wiki/
   │   ├── schema.md            shared
   │   ├── purpose.md           shared
   │   ├── index.md             shared
   │   ├── log.md               shared
   │   ├── vectorstore/         shared LanceDB tables
   │   └── ingest_queue.db      shared sled queue state
```

Identical to the existing project layout. Ingestion writes here; every user opening the same project sees the same content. No replication.

### Per-user

```
<data_root>/
   ├── users.toml               admin-managed: [users.alice] password_hash = "$argon2..."
   ├── sessions/                sled tree: session_id -> {user_id, expires_at}
   └── users/<uid>/
       ├── config.json          LLM provider, embedding provider, theme, zoom, recently_opened
       └── chat/<project_id>/<conversation_id>.json
```

API keys live in `config.json`. The entire `data_root` tree is created with restrictive perms (`0700` for dirs, `0600` for files) owned by the server's OS user; only that user can read it. This is the v1 threat model. Encryption at rest is a v2 question.

## Frontend changes

The React/Vite app, Zustand stores, milkdown editor, sigma graph, and three-column layout all stay. What changes is the transport underneath and a small set of new screens.

### What gets ripped out

| Tauri thing | Replaced by |
|---|---|
| `invoke()` | `fetch()` via `src/lib/api.ts` |
| `convertFileSrc()` | `/api/v1/files/<id>/raw` URL |
| `listen()` | `EventSource('/api/v1/events')` via `src/lib/events.ts` |
| `@tauri-apps/plugin-dialog` `open()` | `<FolderBrowserDialog>` talking to `/api/v1/fs/list` |
| `@tauri-apps/plugin-dialog` `message()` | Existing toast/dialog UI |
| `@tauri-apps/plugin-store` `load()` | `/api/v1/config/*` (per-user, server-side) |
| `@tauri-apps/plugin-autostart` | Removed |
| `@tauri-apps/plugin-opener` `openUrl()` | `window.open(url, '_blank')` |
| `@tauri-apps/plugin-http` (CORS bypass) | `/api/v1/proxy/llm` server-side |
| `getCurrentWindow()` theme | `matchMedia('(prefers-color-scheme: dark)')` |

### New thin abstractions

- `src/lib/api.ts` — typed `apiCall<TReq, TRes>(method, path, body?)`. Base URL, cookie handling, error normalization, JSON parsing. ~100 lines. The permanent transport abstraction (not a transitional shim).
- `src/lib/events.ts` — singleton `EventSource` wrapper. `subscribe('chat:token', handler)` style. Multi-subscriber dispatch, reconnect-on-drop.

### New screens

- `<LoginView>` — shown when `whoami` returns 401. Username + password.
- User badge + logout in the top bar.
- `<FolderBrowserDialog>` — server-side folder browser, used by create-project and sources view.
- Top-bar project switcher (`📂 <project-name> ▾`) always visible; replaces "what project am I in" anxiety.

### Settings reorganized

Single Settings page with visual grouping (not tabs):

```
Settings
─── 👤 Personal — only you see these
    LLM Provider
    Embedding Provider
    Appearance (theme, zoom, language)

─── 📂 Project: <project-name> — shared with everyone on this project
    Schema
    Purpose
    Sources (folder paths, auto-watch)
    Scenario template

─── ℹ️  About
    Version, server status, signed in as: <user>
```

The grouping plus helper text makes the sharing semantics obvious without a click.

### Project navigation

- On login, `whoami` returns `recently_opened: [...]` per-user; UI auto-opens index 0.
- `GET /api/v1/projects/list` scans `projects_root` and returns every valid project (same for every user).
- Top-bar switcher always shows the current project name with a dropdown to switch.
- Empty `recently_opened` (first-time user) → folder browser dialog.

### What is not changing in the frontend

Components, styling, layouts, icons, sigma graph internals, milkdown editor, chat token rendering, Zustand store shapes. The transport beneath them changes; their surfaces don't.

## API surface

All endpoints under `/api/v1/`. JSON request/response. Auth via session cookie (`HttpOnly`, `SameSite=Lax`, 30-day expiry).

### Auth

- `POST /auth/login` — `{username, password}` → 200 + `Set-Cookie`; 401 on bad creds.
- `POST /auth/logout` — invalidates the session row in sled.
- `GET /auth/whoami` — `{user_id, username, recently_opened: [...]}` or 401.

### Projects

- `GET /projects/list` — every valid project under `projects_root`.
- `POST /projects/open` — `{path}` → validates via `resolve_under`, loads project, records as recent for this user. Returns `{project_id, schema, purpose, file_tree}`.
- `POST /projects/create` — `{path, scenario_template}` → creates and opens.

### Wiki

- `GET /wiki/page?path=...` → `{content, frontmatter, etag}`.
- `PUT /wiki/page` with `If-Match: <etag>` → 200 or 412 `WIKI_PAGE_STALE`.
- `POST /search` — hybrid BM25 + vector + graph search.
- `GET /graph` — relevance graph for current project.

### Sources & ingest

- `POST /sources/ingest` — `{source_path, hint?}` → 202 `{job_id}`. Progress streams via SSE; completion event includes `pages_changed`.
- `GET /sources/list` → sources in current project.
- `GET /sources/ingest/jobs?mine=true` → in-flight / recent jobs for the requester.
- `GET /sources/ingest/jobs/<id>` → snapshot of a job (used on SSE reconnect to catch up).

### Chat (per-user)

- `GET /chat/conversations?project_id=...` → user's conversations in this project.
- `GET /chat/conversation/<id>` → full history.
- `POST /chat/send` — `{project_id, conversation_id, message}` → 202 `{request_id}`. Tokens stream via SSE.

### Per-user config

- `GET /config` → current user's config.
- `PUT /config` — atomic write (temp + rename).

### Filesystem (rooted)

- `GET /fs/list?path=<rel>` — entries under `projects_root/<rel>`.
- `POST /fs/mkdir` — `{path}` → create subdir.
- `GET /files/<file-id>/raw` — file bytes for preview. `file-id` is project-scoped.

### LLM proxy

- `POST /proxy/llm` — forwards to the requester's configured provider with their API key.

### SSE

- `GET /events` — opens persistent SSE stream keyed by session. Events have a `type` field; client routes via `events.ts`.

### Legacy / agent

- `/api/v1/agent/*` — mounted on both listeners. Auth-required on main listener, no-auth on `127.0.0.1:19828`.

## Error handling

### Uniform error format

```json
{
  "error": {
    "code": "PATH_ESCAPE",
    "message": "Path escapes the projects root",
    "details": { "requested": "../etc/passwd" }
  }
}
```

HTTP status carries the broad category; `code` is a stable string enum the frontend switches on; `message` is user-facing; `details` is optional structured data.

v1 codes: `UNAUTHENTICATED`, `INVALID_CREDENTIALS`, `PATH_ESCAPE`, `PROJECT_NOT_FOUND`, `WIKI_PAGE_NOT_FOUND`, `WIKI_PAGE_STALE` (412), `LLM_PROVIDER_NOT_CONFIGURED`, `LLM_PROVIDER_REQUEST_FAILED`, `INGEST_FAILED`, `INTERNAL`.

### Path safety chokepoint

Every filesystem op goes through `storage::paths::resolve_under`:

```rust
pub fn resolve_under(root: &Path, requested: &str) -> Result<PathBuf, PathError> {
    // 1. Reject absolute paths or any segment containing '..'.
    // 2. Join + canonicalize.
    // 3. Verify the canonical result still has `root` as a prefix.
}
```

Two roots in use: `projects_root` (user-facing fs) and `data_root` (server-internal). Any handler that takes a `project_id` re-resolves it through `resolve_under` before touching disk — never trust paths returned earlier.

### Concurrent wiki edits

Last-write-wins with `If-Match`. Frontend on 412 shows a "discard mine / overwrite" dialog. Wiki pages are LLM-owned and rarely human-edited; no need for CRDT or 3-way merge.

### Long-running jobs

Existing persistent ingest queue is preserved. On startup, `IN_PROGRESS` jobs are marked `INTERRUPTED` and remain visible for retry. Job-level events are bound to the submitter's session; if the submitter disconnects, the job continues but events stop.

### SSE disconnect

No event replay. On reconnect, the client polls `GET /sources/ingest/jobs/<id>` once to catch up to current state, then resumes the live stream.

### LLM provider failures

Upstream 4xx/5xx is forwarded as `LLM_PROVIDER_REQUEST_FAILED` with the upstream status + body in `details`. Timeouts get `details.kind = "timeout"`. Frontend shows the upstream error inline so users see actionable messages (e.g., "your API key is invalid") rather than generic failures.

### Server crash & restart

Sessions, ingest queue, and per-user config all persist. SSE streams die and the browser's `EventSource` auto-reconnects. In-flight HTTP requests die; frontend retries idempotent reads and surfaces errors on writes. No special recovery code.

## Security posture

- `HttpOnly` + `SameSite=Lax` session cookies. Not `Secure` in v1 (LAN-only over HTTP).
- argon2 password hashing. Naturally slow (~100ms/verify), so brute-force is bounded by CPU.
- No CSRF tokens for v1; `SameSite=Lax` blocks the realistic threats.
- API keys stored plain inside the `data_root` tree (`0700` dirs / `0600` files, owned by the server user). Encryption at rest is a v2 question.
- No rate limiting beyond what the existing `api_server.rs` already has; small trusted group, low realistic risk.
- Path safety via single chokepoint; properties tested.
- The legacy `127.0.0.1:19828` listener is unauthenticated and reachable only from localhost — same threat model as today's app.

## Testing strategy

### Preserve

- All existing `vitest` frontend pure-logic tests (stores, chat-messages-to-llm, lint-store, review-store properties).
- `npm run test:mocks` as the default loop; `npm run test:llm` opt-in.
- Existing `cargo test` for ported crates.

### Add — high leverage

- **Path safety:** ~20 unit tests for `resolve_under` (`..`, absolute, symlink-out, symlink-in, valid, missing, non-UTF8, mixed separators, edge cases). Property test via `proptest`. A bug here is a remote-file-read vulnerability.
- **Auth:** argon2 hash roundtrip, sled session create/lookup/expire/delete, persistence across in-memory restart, axum integration tests for the middleware (401 without cookie, 200 with valid, 401 with expired, logout invalidates).
- **Per-user isolation:** integration tests proving Alice's config/chat is invisible to Bob.

### Add — medium leverage

- One happy-path axum integration test per HTTP handler; one auth-gate test per endpoint; one path-scope test per fs-touching endpoint. Catches wiring mistakes, not exhaustive.
- `core::ingest_queue` tested against a `Vec<Event>`-capturing `EventSink` to assert phase order without HTTP.
- Ingest queue persistence: enqueue, simulate crash, restart, verify `INTERRUPTED`.
- One SSE smoke test: open `EventSource`, trigger event, assert arrival.

### Add — frontend

- `api.ts` with mocked `fetch`: error shape parsing, cookie handling, JSON serialization.
- `events.ts` with mocked `EventSource`: event-type routing, multi-subscriber dispatch, reconnect.
- Component tests continue to mock the transport boundary (was `invoke()`, becomes `api.ts`).

### Manual / exploratory

- Folder browser UX, login flow, PDF/image preview rendering, Chrome + Safari sanity, two-user LAN smoke test. Captured in a `manual-test-plan.md` checklist for the final acceptance pass.

### Not testing

- The Tauri code (being deleted).
- Visual regression (no Playwright/Storybook today; not adding it).
- Load / perf (small trusted group).

### CI

Whatever runs on `main` today must keep running on the port branch. New Rust tests join the existing `cargo test` step; new frontend tests join `vitest`. No new CI infrastructure.

## Deployment

- Development on macOS.
- Production target TBD; the binary is portable. Likely Linux VM with systemd (or Docker) when the time comes.
- **Server config** (process-level): env vars or a small startup TOML — `PORT`, `PROJECTS_ROOT`, `DATA_ROOT`, optional `LEGACY_19828_ENABLED`. Read once at startup; changes require a restart.
- **User config** (`users.toml`): lives inside `data_root` (e.g. `<data_root>/users.toml`). Admin manages users by editing this file directly, or via a CLI subcommand of the same binary (`llm-wiki-server user add alice`) that handles password prompting + argon2 hashing.
- Three files on disk to ship: the binary, the optional startup config (or env vars), and the admin-edited `users.toml`.

## Out of scope for v1

| Feature | Reason |
|---|---|
| Chrome Web Clipper | Each user would need extension reconfiguration; defer. |
| Scheduled imports | Awkward without "is the GUI open" assumption; defer. |
| Autostart | Server is a daemon; OS-level concern, not UI. |
| CLI subprocess transports (claude/codex) | Shared VM auth is awkward; per-user CLI auth is a rabbit hole. HTTP LLM providers cover the use case for v1. |
| Cross-user real-time notifications | User said not critical; polling/manual refresh is fine. |
| Project ACLs | "Small trusted group sharing everything" is the model. |
| HTTPS in the binary | Reverse-proxy later if needed. |
| Encryption at rest for API keys | Linux `0600` perms is the v1 threat model. |
| Audit log of who did what | Existing per-project `log.md` covers ingest/lint events. |
| Rate limiting beyond existing | Small trusted group. |
| Mobile-first UI | Existing layout is desktop-shaped. |

## Open questions deferred to implementation

- Exact `axum` extractor patterns for `User` + `Project` context.
- Whether the legacy `19828` endpoints get a feature flag to require auth (probably yes, off by default).
- CLI subprocess auth UX on the VM (single shared `~/.claude` is fine; we just need to document it).
- Whether session refresh extends the cookie on activity or only on login (probably "on activity, capped at 90 days from login").
- Whether to write a tiny admin CLI for user management or document the `users.toml` format and let the admin edit it.

## Migration

Single feature branch. On `main`, the desktop Tauri app continues to work. On the branch:

1. (Conceptually first, mechanically can be done as part of the same big-bang work) Extract business logic from `#[tauri::command]` wrappers into `core/`.
2. Stand up `axum`, auth, session middleware, per-user storage.
3. Build the HTTP API surface against `core/`.
4. Rip out `@tauri-apps/*` imports across the frontend; introduce `api.ts` and `events.ts`; rewire every call site.
5. Build `<LoginView>`, `<FolderBrowserDialog>`, top-bar project switcher; reorganize settings.
6. Rename `src-tauri/` to `src-server/`; delete Tauri-specific files inside it (`tauri.conf.json`, the `tauri.*.conf.json` variants, `capabilities/`, `windows-app-manifest.xml`, `build.rs` if Tauri-only); drop `tauri = ...` and all `tauri-plugin-*` crates from `Cargo.toml`; drop `@tauri-apps/*` from `package.json`.
7. Smoke test on Mac (localhost) and at least one other browser. Cross-browser sanity check. Two-user LAN smoke if possible.
8. Merge.

"Done" = the app is runnable end-to-end against the new architecture, all preserved tests are green, the manual test checklist passes.
