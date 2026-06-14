# Browser/LAN GUI Phase 5 — Frontend transport rewire (Implementation Plan)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task.

**Phase goal:** Replace every `@tauri-apps/*` import in the React frontend with HTTP-based equivalents that hit the Phase-4 axum server. The React component tree, Zustand stores, layouts, milkdown editor, Sigma graph — all stay. Only the transport layer underneath changes. By the end of Phase 5, the user can open the browser at `http://localhost:8080`, log in, and use the actual LLM Wiki UI (not the placeholder smoke page).

**Architecture:**
- `src/lib/api.ts` — typed `apiCall<TReq, TRes>(method, path, body?)` plus `apiCallRaw` for non-JSON. Throws `ApiError` matching the server's error envelope. `credentials: 'include'` for session cookies.
- `src/lib/events.ts` — singleton `EventSource` to `/api/v1/events`, multi-subscriber dispatch by event type, exponential reconnect.
- Tauri-only feature modules get deleted (`claude-cli-transport`, `codex-cli-transport`, `scheduled-import-section`) — these are explicitly out of v1 scope per the spec.
- A minimal `<LoginView>` is added in this phase so the browser is usable end-to-end. Phase 6 will polish it.
- The settings page keeps its current structure; the "Personal vs Project" reorganization is Phase 6.

**Source plan:** `plans/2026-06-14-browser-lan-gui-implementation.md` (Phase 5 section).
**Endpoint shapes:** Tested in `plans/phase-4-smoke.md` and `plans/phase-4-summary.md`.

**Branch:** `feat/browser-lan-port` (continue).

**Environment:**
- `npm`/`node` versions per the existing repo.
- `cargo` at `~/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo`.
- Run `npm run typecheck` after each substantive change.
- Run `npm run test:mocks` (vitest) as the default test loop.

---

## Phase 5 task overview

| # | Task | Outcome |
|---|---|---|
| 5.1 | Transport foundation + audit | `src/lib/api.ts` + `src/lib/events.ts` exist with tests. Audit doc lists every `@tauri-apps/*` call site and what replaces it. |
| 5.2 | Delete out-of-v1 features | `claude-cli-transport.ts`, `codex-cli-transport.ts`, `scheduled-import-section.tsx` removed; importers cleaned up. |
| 5.3 | Rewire `invoke()` call sites — foundation files | `src/commands/*.ts`, `src/lib/{search,project-store,project-identity,project-file-sync,markdown-image-resolver,extract-source-images,embedding,llm-providers,theme,tauri-fetch}.ts` migrated. |
| 5.4 | Rewire `invoke()` call sites — UI components | `src/App.tsx`, `src/components/**/*.tsx` migrated. |
| 5.5 | Rewire `listen()` → `EventSource` subscriptions | Three call sites in `lib/project-file-sync.ts`, `lib/claude-cli-transport.ts` (already deleted), `lib/codex-cli-transport.ts` (already deleted) become one helper. |
| 5.6 | Replace `plugin-store` with `/config` endpoint | The per-machine key-value store becomes a per-user JSON blob via `apiCall('PUT', '/config', ...)`. |
| 5.7 | Replace `plugin-dialog open()` with server-side folder browser | `<FolderBrowserDialog>` component talks to `/api/v1/fs/list` and `/api/v1/fs/mkdir`. Used by `create-project-dialog`, `sources-view`, etc. |
| 5.8 | Replace remaining small plugins | `plugin-opener` → `window.open`; `plugin-autostart` → delete; `convertFileSrc` → `/api/v1/files/raw` URL; `getCurrentWindow().theme()` → `matchMedia(...)`. |
| 5.9 | Replace `tauri-fetch.ts` (CORS-bypassed HTTP) with `/proxy/llm` | LLM and embedding calls go through the server. |
| 5.10 | Add minimal `<LoginView>` | If `whoami` returns 401, show username+password form that POSTs `/api/v1/auth/login` and reloads. Phase 6 will polish. |
| 5.11 | Build + smoke | `npm run build` succeeds, `dist/` regenerated, server rebuilt to bundle the new frontend, browse to localhost:8080 → log in → open project → read wiki page. |

---

# Task 5.1 — Transport foundation

**Files to create:** `src/lib/api.ts`, `src/lib/events.ts`, `src/lib/api.test.ts`, `src/lib/events.test.ts`.
**Files to read:** the existing import map (the `grep` result from phase planning).

## `src/lib/api.ts`

```typescript
//! HTTP transport for the LLM Wiki server.

export interface ApiErrorBody {
  code: string;
  message: string;
  details?: unknown;
}

export class ApiError extends Error {
  constructor(
    public readonly status: number,
    public readonly code: string,
    message: string,
    public readonly details?: unknown,
  ) {
    super(message);
    this.name = "ApiError";
  }

  static unauthenticated(): ApiError {
    return new ApiError(401, "UNAUTHENTICATED", "authentication required");
  }
}

// Default base URL: same origin (production). Tests can override via env.
const BASE_URL = (import.meta.env?.VITE_API_BASE as string | undefined) ?? "";

export interface ApiCallOptions {
  /** If true, response is returned as a Response object (for streaming/binary). */
  raw?: boolean;
  /** Extra headers to set on the request. */
  headers?: Record<string, string>;
}

export async function apiCall<TRes = unknown>(
  method: "GET" | "POST" | "PUT" | "DELETE",
  path: string,
  body?: unknown,
  options: ApiCallOptions = {},
): Promise<TRes> {
  const url = `${BASE_URL}${path}`;
  const headers: Record<string, string> = { ...(options.headers ?? {}) };
  let bodyInit: BodyInit | undefined;
  if (body !== undefined) {
    headers["Content-Type"] = "application/json";
    bodyInit = JSON.stringify(body);
  }
  const resp = await fetch(url, {
    method,
    credentials: "include",
    headers,
    body: bodyInit,
  });
  if (!resp.ok) {
    let parsed: ApiErrorBody | undefined;
    try {
      const text = await resp.text();
      parsed = (text ? JSON.parse(text) : undefined) as { error?: ApiErrorBody } | undefined;
    } catch {
      // body wasn't JSON
    }
    const err = parsed?.error ?? { code: "UNKNOWN", message: resp.statusText };
    throw new ApiError(resp.status, err.code, err.message, err.details);
  }
  if (options.raw) {
    return resp as unknown as TRes;
  }
  // 204 / empty body
  if (resp.status === 204) {
    return undefined as unknown as TRes;
  }
  const ct = resp.headers.get("content-type") ?? "";
  if (ct.includes("application/json")) {
    return (await resp.json()) as TRes;
  }
  return (await resp.text()) as unknown as TRes;
}

/** Builds a URL to the file preview endpoint with proper escaping. */
export function fileRawUrl(projectPath: string, filePath: string): string {
  const qs = new URLSearchParams({ project_path: projectPath, path: filePath });
  return `${BASE_URL}/api/v1/files/raw?${qs.toString()}`;
}
```

## `src/lib/events.ts`

```typescript
//! Singleton EventSource wrapper with type-based subscription.

type Handler = (payload: unknown) => void;

class EventBus {
  private es: EventSource | null = null;
  private subscribers = new Map<string, Set<Handler>>();
  private reconnectAttempts = 0;

  constructor(private url: string) {}

  private ensureConnected() {
    if (this.es) return;
    const es = new EventSource(this.url, { withCredentials: true });
    this.es = es;
    es.onopen = () => {
      this.reconnectAttempts = 0;
    };
    es.onerror = () => {
      // EventSource auto-reconnects, but if we get a persistent error, log it.
      // We can implement custom backoff later if needed.
    };
    // Wildcard: listen for every event type the server might send by attaching
    // a listener on demand. For now we attach a generic onmessage and dispatch
    // by `event.type` (the EventSource API sets `event.type` based on the
    // `event:` SSE field).
    // We also need to attach addEventListener for each named type. We do that
    // lazily in subscribe().
  }

  subscribe(eventType: string, handler: Handler): () => void {
    this.ensureConnected();
    let set = this.subscribers.get(eventType);
    if (!set) {
      set = new Set();
      this.subscribers.set(eventType, set);
      // Attach a typed listener once.
      this.es?.addEventListener(eventType, (e: MessageEvent) => {
        const payload = (() => {
          try {
            return JSON.parse(e.data);
          } catch {
            return e.data;
          }
        })();
        for (const h of this.subscribers.get(eventType) ?? []) {
          h(payload);
        }
      });
    }
    set.add(handler);
    return () => {
      set!.delete(handler);
    };
  }

  close() {
    this.es?.close();
    this.es = null;
    this.subscribers.clear();
  }
}

const eventBus = new EventBus("/api/v1/events");

export function subscribe(eventType: string, handler: (payload: unknown) => void): () => void {
  return eventBus.subscribe(eventType, handler);
}

export function closeEventBus() {
  eventBus.close();
}
```

## Tests

Quick smoke tests for both modules using vitest's built-in mocks for fetch/EventSource (`vi.spyOn(globalThis, 'fetch')` and a hand-rolled EventSource mock).

## Migration audit

Append to the plan a short file at `plans/phase-5-audit.md` listing each file and its required migration:

```markdown
# Phase 5 migration map

| File | Tauri import(s) | Replacement |
|---|---|---|
| src/App.tsx | invoke, plugin-dialog open, plugin-autostart | apiCall, FolderBrowserDialog, [DELETE] |
| src/lib/embedding.ts | invoke, tauri-fetch | apiCall('POST', '/proxy/llm', ...) |
| ... | ... | ... |
```

Use the grep results from this conversation to populate it. The audit doc serves as the checklist for Tasks 5.3+.

**Commit:** `feat(frontend): add api.ts + events.ts transport modules`

---

# Task 5.2 — Delete out-of-v1 features

Three feature areas are explicitly out of v1 scope per the design spec; delete them now so subsequent tasks don't have to migrate them:

- **CLI subprocess transports for Claude/Codex** — `src/lib/claude-cli-transport.ts`, `src/lib/codex-cli-transport.ts` and their test files. Any importers (likely chat or LLM provider settings) lose the option to use those transports.
- **Scheduled imports** — `src/components/settings/sections/scheduled-import-section.tsx`. Any place that mounts this section drops it.
- **Autostart** — `@tauri-apps/plugin-autostart` is OS-shell only; remove all references. The Settings page probably has an "autostart" toggle — delete the row.

Steps:
1. Delete the listed files (and their test files).
2. Grep the codebase for imports of those modules and update each importer:
   - For CLI transports: the chat input or LLM selection UI probably has a "Use CLI" option; delete it. The default is HTTP.
   - For scheduled imports: the settings page imports the section; delete the import + the section reference in the JSX.
   - For autostart: delete the import, the state, the toggle UI, and the function calls.
3. Run `npm run typecheck` to find what broke; fix all errors.

**Commit:** `chore(frontend): remove CLI transports, scheduled imports, autostart (out of v1 scope)`

---

# Task 5.3 — Rewire `invoke()` in foundation files

Foundation files first — these are imported by stores/components, so migrating them before the components avoids second-order breakage:

- `src/commands/fs.ts` — wraps `invoke('read_file', ...)`, `invoke('write_file', ...)`, etc. Migrate each to the corresponding HTTP endpoint (`/wiki/page`, `/files/raw`, etc.). The function signatures must stay identical so callers don't change.
- `src/commands/file-sync.ts` — `invoke('start_project_file_watcher', ...)` → call `/sources/ingest` (rescan kicks off the watcher); `invoke('get_file_change_queue', ...)` → `/sources/ingest/queue`.
- `src/lib/search.ts` — `invoke('search_project', ...)` → `apiCall('POST', '/search', ...)`.
- `src/lib/project-store.ts` — `load()` from `plugin-store` → `apiCall('GET', '/config')` (this overlaps with Task 5.6; pick one).
- `src/lib/project-identity.ts` — similar, plugin-store-backed.
- `src/lib/project-file-sync.ts` — `listen('queue_updated', ...)` etc. → `subscribe('queue:updated', ...)` (see Task 5.5).
- `src/lib/markdown-image-resolver.ts` — `convertFileSrc(path)` → `fileRawUrl(projectPath, path)`.
- `src/lib/extract-source-images.ts` — `invoke('extract_pdf_images_cmd', ...)` → `apiCall('POST', '/sources/extract-images', ...)`. **Note:** Phase 4 didn't add this HTTP endpoint. **Either add it now** (small follow-up task) or document that image extraction is server-side-only during ingest (not a one-off frontend call).
- `src/lib/embedding.ts` — `invoke` + `tauri-fetch` → `apiCall('POST', '/proxy/llm', ...)`.
- `src/lib/llm-providers.ts` — same.
- `src/lib/theme.ts` — `getCurrentWindow().theme()` → `matchMedia('(prefers-color-scheme: dark)')`.
- `src/lib/tauri-fetch.ts` — delete entirely; its callers now use `apiCall('POST', '/proxy/llm', ...)`.

**Step pattern per file:**
1. Read the file.
2. Identify each `invoke('foo', args)` call and find the matching HTTP endpoint by reading Phase 4 handlers.
3. Replace with `apiCall(method, path, body)`.
4. If the endpoint doesn't exist yet, flag it — most are covered, a few (like `extract_pdf_images_cmd`) may need a small server-side handler added.
5. Update tests if any.
6. `npm run typecheck`.

**Commit:** `refactor(frontend): rewire foundation files from invoke() to apiCall()`

---

# Task 5.4 — Rewire `invoke()` in UI components

After foundation files compile, migrate the components:

- `src/App.tsx`
- `src/components/settings/settings-view.tsx`
- `src/components/settings/sections/about-section.tsx`
- `src/components/settings/sections/llm-provider-section.tsx`
- `src/components/settings/sections/api-server-section.tsx`
- `src/components/chat/chat-message.tsx`
- `src/components/project/create-project-dialog.tsx`
- `src/components/layout/file-tree.tsx`
- `src/components/layout/update-banner.tsx`
- `src/components/sources/sources-view.tsx`
- `src/components/editor/file-preview.tsx`

Each component's migration is straightforward IF the underlying logic is already in `commands/` or `lib/` (Task 5.3). Mostly the components just need their import statements updated.

For dialogs that use `plugin-dialog`:
- `<CreateProjectDialog>` needs `<FolderBrowserDialog>` (Task 5.7) for the "where do you want the project" picker.
- `<SourcesView>` needs `<FolderBrowserDialog>` for "import folder".
- `<FileTree>` uses `message()` for native confirm dialogs — replace with whatever toast/dialog primitive the codebase already has (search for `useDialog` or similar).

For `<AboutSection>` and `<UpdateBanner>` that use `openUrl()`: replace with `window.open(url, '_blank', 'noopener')`.

**Commit:** `refactor(frontend): rewire UI components from invoke() to apiCall()`

---

# Task 5.5 — `listen()` → SSE `subscribe()`

Find all `listen()` and `addEventListener` usages of Tauri events. After Task 5.2 deleted the CLI transports, only `lib/project-file-sync.ts` should still use `listen`.

For each:
1. Identify the event name (e.g., `queue_updated`, `file_changed`).
2. Find the server event-type constant in `core/file_sync.rs` (look for `EVENT_QUEUE_UPDATED` and `EVENT_CHANGED`).
3. Replace `listen(EVENT_NAME, handler)` with `subscribe(EVENT_NAME, handler)` from `src/lib/events.ts`.
4. The unsubscribe semantics are the same (the return value is a function to call).

**Commit:** `refactor(frontend): replace Tauri listen() with SSE subscribe()`

---

# Task 5.6 — `plugin-store` → `/config`

`plugin-store` provides per-machine key-value persistence (used for theme, zoom, language, recently-opened, LLM provider config, etc.). The browser model needs per-user persistence via `/api/v1/config`.

Strategy:
- Build a `src/lib/user-config.ts` that mirrors the plugin-store API but goes through `apiCall`:
  ```typescript
  export async function loadConfig(): Promise<Record<string, unknown>> {
    return await apiCall('GET', '/api/v1/config');
  }
  export async function saveConfig(cfg: Record<string, unknown>): Promise<void> {
    await apiCall('PUT', '/api/v1/config', cfg);
  }
  // Convenience: get/set a key path
  export async function getConfigKey(key: string): Promise<unknown>;
  export async function setConfigKey(key: string, value: unknown): Promise<void>;
  ```
- Replace each `load('store').get(key)` / `.set(key, val)` call site with the new helpers. Note that the server stores one blob per user, so set/get becomes load-merge-save.

**Commit:** `refactor(frontend): replace plugin-store with /config API`

---

# Task 5.7 — `<FolderBrowserDialog>` component

Replace `await open({directory: true})` with a server-side folder browser modal.

Create `src/components/layout/folder-browser-dialog.tsx`:
- Renders a modal with a tree of folders under `projects_root`.
- Calls `apiCall('GET', '/api/v1/fs/list?path=...')` on navigation.
- "Create folder" button → `apiCall('POST', '/api/v1/fs/mkdir', ...)`.
- "Select" returns the chosen path string via the promise it resolves.

API for callers:
```typescript
export async function openFolderBrowser(opts?: {
  startPath?: string;
  title?: string;
}): Promise<string | null>;
```

Replace each `await open({directory: true})` with `await openFolderBrowser()`.

**Commit:** `feat(frontend): add server-side FolderBrowserDialog`

---

# Task 5.8 — Remaining small plugins

| Tauri | Replacement |
|---|---|
| `openUrl(url)` from `plugin-opener` | `window.open(url, '_blank', 'noopener')` |
| `convertFileSrc(path)` from `api/core` | `fileRawUrl(projectPath, path)` from `src/lib/api.ts` |
| `getCurrentWindow().theme()` from `api/window` | `matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'` + a `change` listener |
| `message(text)` from `plugin-dialog` | existing toast/dialog primitive — search the codebase for `useToast` or `<Dialog>` |
| `plugin-autostart` | DELETED (Task 5.2) |

Each is a small mechanical replacement, ~5 call sites total.

**Commit:** `refactor(frontend): replace small Tauri plugins with web equivalents`

---

# Task 5.9 — `tauri-fetch.ts` → `/proxy/llm`

`src/lib/tauri-fetch.ts` was a CORS-bypassing HTTP client used by LLM and embedding calls. Replace its callers:

- For chat completion: `apiCall('POST', '/api/v1/chat/send', ...)` (Phase 4 endpoint).
- For embeddings: `apiCall('POST', '/api/v1/proxy/llm', { stream: false, body: {...embeddings request...} })`.
- For arbitrary OpenAI-compatible calls: same `/proxy/llm` pattern.

Delete `src/lib/tauri-fetch.ts` and its tests once all callers migrated.

**Commit:** `refactor(frontend): route LLM/embedding through /proxy/llm`

---

# Task 5.10 — Minimal `<LoginView>`

Create `src/components/auth/login-view.tsx`:
- Form: username, password, submit button.
- On submit: `apiCall('POST', '/api/v1/auth/login', { username, password })`.
- On 401: shake / show error.
- On 200: reload the page (simplest auth flow — Phase 6 can refactor to set context state without reload).

In `src/App.tsx`:
- On mount: `apiCall('GET', '/api/v1/auth/whoami')` — if 401 (ApiError code `UNAUTHENTICATED`), render `<LoginView>` instead of the main app shell.

This unlocks the browser end-to-end. Visually crude — Phase 6 polishes.

**Commit:** `feat(frontend): add minimal LoginView gated by whoami`

---

# Task 5.11 — Build + smoke

- `npm run typecheck` — must pass.
- `npm run test:mocks` — vitest still passes (some tests for deleted features may need removal).
- `npm run build` — produces a fresh `dist/`.
- `grep -rn "@tauri-apps" src/` — should return empty (or only files we explicitly skipped).
- Rebuild the server binary (so rust-embed picks up the new `dist/`):
  ```bash
  (cd src-tauri && cargo build --release --bin llm-wiki-server)
  ```
- Start it against the smoke project. Open `http://localhost:8080` in a browser.
- Verify end-to-end:
  - Login screen renders, log in works.
  - Project list shows the test project.
  - Open project — UI loads schema/wiki/file tree.
  - Read a wiki page — content displays.
  - Edit a wiki page — save roundtrips.
  - Settings page renders (even if unpolished).
- **Note:** features that depend on per-user LLM config (chat, embedding) require setting `config.llm = {base_url, model, api_key}` — do so via curl or the Settings page (if LLM provider section works after migration).

**Commit:** `docs: add Phase 5 smoke runbook` (with the actual curl/browser steps)

## Phase 5 done-check

- [ ] `grep -rn "@tauri-apps" src/` returns empty (allowed: nothing).
- [ ] `npm run build` succeeds with no `@tauri-apps/*` in the bundle (check the dist).
- [ ] `npm run typecheck` clean.
- [ ] `npm run test:mocks` green.
- [ ] Browser smoke (login → open project → read/edit page) passes.
- [ ] Both binaries still build (`llm-wiki` desktop may produce a broken UI but it must build).
- [ ] `plans/phase-5-summary.md` documents any deviations.

## Phase 5 → Phase 6 handoff

Phase 5 leaves a usable but unpolished browser experience:
- LoginView is functional but ugly.
- Settings page still uses the old structure (per-machine mindset rather than Personal/Project split).
- No `<UserBadge>` in the top bar (no logout button visible).
- No `<ProjectSwitcher>` — switching projects requires going back to the project picker.
- `<FolderBrowserDialog>` is functional but bare.

Phase 6 adds:
- Polished `<LoginView>`.
- `<UserBadge>` with logout in top bar.
- `<ProjectSwitcher>` in top bar.
- Settings reorganized into Personal / Project sections per the design spec.
- `<FolderBrowserDialog>` styled and integrated.
