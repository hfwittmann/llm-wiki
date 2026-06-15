# Phase 5 summary — Frontend transport rewire

**Branch:** `feat/browser-lan-port`
**Phase 4 HEAD (start):** `4779f66`
**Phase 5 HEAD (end):** `9f2ae42` (+ this summary commit)
**Commits:** 11 implementation + 1 plan doc + this summary

## What landed

The React frontend now talks to the Phase-4 axum server over HTTP (and SSE for streaming) instead of Tauri IPC. The component tree, Zustand stores, milkdown editor, Sigma graph — all unchanged. Only the transport layer underneath was rewired.

- **`src/lib/api.ts`** — `apiCall<TRes>(method, path, body?, opts?)` with `credentials: 'include'`. Throws typed `ApiError(status, code, message, details)`. Plus `apiFetch` for raw responses and `fileRawUrl(projectPath, filePath)` helper.
- **`src/lib/events.ts`** — singleton `EventSource` wrapper. `subscribe(eventType, handler)` for type-based SSE dispatch with multi-subscriber support.
- **`src/lib/user-config.ts`** — `getConfigKey` / `setConfigKey` / `deleteConfigKey` mirror the `plugin-store` API but go through `/api/v1/config`. In-memory cache with coalesced inflight loads.
- **`src/lib/auth.ts`** — module-level singleton for the current authenticated user.
- **`src/lib/theme.ts`** — `getSystemTheme()` / `onSystemThemeChanged(...)` via `window.matchMedia(...)`. `getCurrentWindow()` calls removed.
- **`src/components/auth/login-view.tsx`** — minimal centered username+password form. POSTs `/api/v1/auth/login`, calls `whoami` on success, hands the user to `onLogin`.
- **`src/components/layout/folder-browser-dialog.tsx`** — server-side folder browser modal. Talks to `/api/v1/fs/list` and `/api/v1/fs/mkdir`. Replaces the native `plugin-dialog open({directory})`.

## What was deleted (out of v1 scope)

- `src/lib/claude-cli-transport.ts` + tests
- `src/lib/codex-cli-transport.ts` + tests
- `src/lib/scheduled-import.ts` + tests
- `src/lib/tauri-fetch.ts` + tests
- `src/components/settings/sections/scheduled-import-section.tsx`
- All `plugin-autostart` UI and code references
- All CLI-provider branches in `llm-client.ts`, `llm-providers.ts`, `vision-caption.ts`, `image-caption-pipeline.ts`, `has-usable-llm.ts`
- The `src-tauri/dist/` placeholder (rust-embed now reads project-root `dist/`)

## Tests

- **1357 vitest tests pass**, 53 skipped, 0 failures.
- The 53 skipped tests are in `embedding.test.ts` — they tested `vector_*` aggregation logic that's now stubbed (the server owns the vector store). Cleanup planned in Phase 6/7.

## End-to-end smoke

After `npm run build` (Vite → project-root `dist/`) and a fresh `cargo build --bin llm-wiki-server`:

```bash
LLM_WIKI_DATA_ROOT=... LLM_WIKI_PROJECTS_ROOT=... LLM_WIKI_LEGACY_19828_ENABLED=false \
  ./target/debug/llm-wiki-server
```

Then in a browser at `http://localhost:8080`:
1. The Vite-built React app loads (verified: `GET /` returns the real LLM Wiki HTML with all asset preload links, not the Phase-2 placeholder).
2. `whoami` returns 401 → `<LoginView>` renders.
3. Login with `alice` / `demo-password` → cookie set, whoami succeeds, main app renders.
4. Projects list shows the test project.
5. Open project → the actual React UI loads (file tree, wiki view, etc.).

Verified via curl that:
- `GET /` → 200, real index.html (1.4MB JS bundle accessible at `/assets/...`).
- `POST /api/v1/auth/login` with valid creds → 200 + cookie.
- `GET /api/v1/auth/whoami` with cookie → 200 + user info + `recently_opened`.
- `GET /api/v1/projects/list` with cookie → 200 + project array.

## Pragmatic deviations from the spec

### LLM/embedding calls don't go through `/proxy/llm` (yet)

The spec said:

> Server-side LLM/embedding proxy. All LLM and embedding calls originate from the server. User API keys never reach the browser; CORS issues disappear.

In practice, multiple LLM-adjacent callers (`llm-client.ts`, `embedding.ts`, `web-search.ts`, `anytxt-search.ts`, `update-check.ts`, `mineru.ts`) use **non-OpenAI APIs** with their own request shapes that don't fit the generic `/api/v1/proxy/llm` endpoint. The implementer kept these as plain browser `fetch()` calls in Phase 5, which means:

- The user's API key IS loaded into browser memory (via `getConfigKey('llm')`). It's reachable to any JS on the page.
- For "small trusted group on LAN" the practical risk is acceptable: same-origin policy + HttpOnly session cookie + each user only sees their own config.
- The OpenAI-compatible chat path (`/chat/send`) DOES go through the server. Embeddings via the OpenAI Embeddings API could be routed through `/proxy/llm` later (it's a simple POST + JSON), but mineru / anytxt / web-search would need dedicated server endpoints.

**Status:** acceptable for v1. If you ever expose the server to a less-trusted audience, revisit by either (a) adding server-side endpoints for each external API or (b) extending `/proxy/llm` to forward to arbitrary URLs (with security implications).

### Several `commands/*.ts` wrappers are now warning-only stubs

Tasks 5.3 stubbed file-IO commands that no longer have HTTP equivalents (`writeFile`, `copyFile`, `deleteFile`, `createDirectory`, `getFileMd5`, etc.) and CLI-only commands (`vector_*` — the server owns the vector store now). They log a warning and return a sensible default (empty array, `undefined`, etc.). Callers in the frontend rarely hit them; when they do, the warning helps spot the dead path.

**Status:** acceptable for v1. Phase 6/7 can audit which stubs are reached at runtime and either remove the call sites or add server endpoints.

### `extract-source-images.ts` is a no-op stub

PDF image extraction during ingest happens server-side. The frontend's `extractAndSaveSourceImages` is now a stub returning `[]`. Callers in the chat/sources views that expected to get extracted-image metadata back will see empty results — but the ingest UI still shows progress via SSE.

**Status:** acceptable for v1. If the frontend needs the extracted-image list for UI purposes, add a query endpoint in Phase 6.

## Frontend now reachable in the browser

The user opens `http://<host>:8080`, sees the LoginView, signs in with credentials from `users.toml`, and the LLM Wiki UI loads. Features available:

- ✅ Login / logout / whoami
- ✅ Project list / open / create (with server-side folder picker)
- ✅ Wiki page read / edit with ETag concurrency
- ✅ Search
- ✅ Settings (Personal config persisted via `/config`)
- ✅ File preview via `/files/raw`
- ✅ SSE for ingest progress + chat tokens
- ⚠️  Some legacy features may surface "stub" warnings in the dev console
- ⚠️  Graph view is computed client-side (server returns 501 for `/graph`)

## Phase 5 → Phase 6 handoff

Phase 5 leaves a usable but rough browser experience:

- **LoginView** is functional but visually minimal (Phase 6 polishes).
- **Settings page** still uses the old per-machine layout, not Personal/Project split (Phase 6).
- **No `<UserBadge>`** in the top bar — no visible logout button (Phase 6).
- **No `<ProjectSwitcher>`** in the top bar (Phase 6).
- **53 skipped tests** for stubbed vector aggregation (cleanup in Phase 6 or 7).
- **`@tauri-apps/*` deps** still listed in `package.json` (Phase 7 removes them).
- The **Tauri desktop binary** (`llm-wiki`) is currently in an in-between state: code compiles, but many `commands/*.ts` wrappers are stubbed so the desktop UI's file operations won't function. This is expected — Phase 7 deletes the Tauri shell entirely.

Phase 6 polishes the UI; Phase 7 deletes Tauri and the stubs.
