import { apiCall, fileRawUrl } from "@/lib/api"
import type { FileNode, WikiProject } from "@/types/wiki"
import { ensureProjectId, upsertProjectInfo } from "@/lib/project-identity"
import { isAbsolutePath } from "@/lib/path-utils"

/** Raw shape returned by the HTTP projects endpoints. */
interface RawProject {
  name: string
  path: string
}

// ── Wiki page read/write ──────────────────────────────────────────────────────

/**
 * Read a wiki page from the server.
 * For wiki pages (markdown), uses /api/v1/wiki/page and returns the content
 * string. The `extractImages` option is not supported server-side (the server
 * handles image extraction internally during ingest).
 */
// Extensions that the server can extract plain text from via pdfium /
// office parsers. Read via /api/v1/files/extracted-text rather than
// /raw — raw would return MB of binary that locks up the browser.
const EXTRACTED_TEXT_EXTS = new Set([
  "pdf",
  "doc",
  "docx",
  "pptx",
  "xls",
  "xlsx",
  "odt",
  "ods",
  "odp",
])

export async function readFile(
  path: string,
  options?: { extractImages?: boolean },
): Promise<string> {
  void options // extractImages was handled server-side during ingest in the desktop pipeline
  const ext = path.split(".").pop()?.toLowerCase() ?? ""
  const endpoint = EXTRACTED_TEXT_EXTS.has(ext)
    ? "/api/v1/files/extracted-text"
    : "/api/v1/files/raw"
  const qs = new URLSearchParams({ path })
  const text = await apiCall<string>("GET", `${endpoint}?${qs.toString()}`)
  return text
}

export async function writeFile(path: string, contents: string): Promise<void> {
  assertAbsoluteFsPath("writeFile", path)
  // TODO(5.4): update callers to pass (projectPath, pagePath, contents, etag)
  // so we can use the /wiki/page PUT endpoint with If-Match. For now this is
  // a no-op stub so the type contract is preserved until callers are rewired.
  console.warn(
    "[fs] writeFile: server-side write requires project_path + page_path + etag. " +
    `Call site must be updated to use apiCall PUT /api/v1/wiki/page. path=${path}`,
  )
  void contents
}

export async function writeFileBase64(_path: string, _base64: string): Promise<void> {
  // TODO(stub): no HTTP equivalent for raw base64 file writes — the server
  // handles image writes internally during ingest. No frontend caller should
  // reach this path after Task 5.4 rewiring.
  console.warn("[fs] writeFileBase64: no HTTP equivalent; operation is a no-op")
}

export async function writeFileAtomic(path: string, contents: string): Promise<void> {
  assertAbsoluteFsPath("writeFileAtomic", path)
  // Delegates to writeFile — same HTTP path until callers are updated in 5.4.
  await writeFile(path, contents)
}

// ── Directory listing ─────────────────────────────────────────────────────────

export async function listDirectory(path: string): Promise<FileNode[]> {
  // Server returns `{entries: [{name, is_dir, ...}]}` — flat, immediate
  // children only, names only. Legacy Tauri's `list_directory` was
  // recursive AND returned absolute paths per entry. Callers (graph view,
  // file tree) walk `node.children`, so we recurse here in the wrapper
  // to preserve that contract.
  //
  // We deliberately skip hidden directories (`.llm-wiki`, `.obsidian`,
  // `.cache`, etc.) — those can hold thousands of cached/index files and
  // were never browsed by the Tauri tree either. Subdirectory recursion
  // runs in parallel via Promise.all to keep wall-clock latency down on
  // wiki trees with many sibling subdirs.
  type ServerEntry = { name: string; is_dir: boolean }
  const isSkippable = (name: string): boolean => name.startsWith(".")
  const visit = async (dir: string): Promise<FileNode[]> => {
    const qs = new URLSearchParams({ path: dir })
    let resp: { entries: ServerEntry[] }
    try {
      resp = await apiCall<{ entries: ServerEntry[] }>(
        "GET",
        `/api/v1/fs/list?${qs.toString()}`,
      )
    } catch {
      return []
    }
    const parent = dir.replace(/\/+$/, "")
    const usableEntries = resp.entries.filter((e) => !isSkippable(e.name))
    // Recurse in parallel: subdirectory walks don't depend on each other.
    return await Promise.all(
      usableEntries.map(async (e): Promise<FileNode> => {
        const fullPath = parent === "" ? e.name : `${parent}/${e.name}`
        const node: FileNode = { name: e.name, is_dir: e.is_dir, path: fullPath }
        if (e.is_dir) {
          node.children = await visit(fullPath)
        }
        return node
      }),
    )
  }
  return await visit(path)
}

// ── File operations (internal-only stubs) ─────────────────────────────────────

export async function copyFile(
  _source: string,
  _destination: string,
): Promise<void> {
  // TODO(stub): no HTTP equivalent — used internally during ingest image
  // extraction. After Task 5.4, callers should be gone.
  console.warn("[fs] copyFile: no HTTP equivalent; operation is a no-op")
}

export async function copyDirectory(
  _source: string,
  _destination: string,
): Promise<string[]> {
  // TODO(stub): no HTTP equivalent.
  console.warn("[fs] copyDirectory: no HTTP equivalent; returning []")
  return []
}

export async function preprocessFile(_path: string): Promise<string> {
  // TODO(stub): no HTTP equivalent — used during ingest. Server handles this.
  console.warn("[fs] preprocessFile: no HTTP equivalent; returning empty string")
  return ""
}

export async function deleteFile(_path: string): Promise<void> {
  // TODO(stub): no HTTP equivalent exposed to the frontend.
  console.warn("[fs] deleteFile: no HTTP equivalent; operation is a no-op")
}

export async function findRelatedWikiPages(
  _projectPath: string,
  _sourceName: string,
): Promise<string[]> {
  // TODO(stub): no HTTP equivalent for find_related_wiki_pages yet.
  console.warn("[fs] findRelatedWikiPages: no HTTP equivalent; returning []")
  return []
}

export async function createDirectory(_path: string): Promise<void> {
  // TODO(stub): /fs/mkdir is available but rooted at projects_root, not
  // arbitrary absolute paths. Update callers in Task 5.4 to use project-
  // relative paths via apiCall POST /api/v1/fs/mkdir.
  console.warn("[fs] createDirectory: no HTTP equivalent for absolute paths; operation is a no-op")
}

export async function fileExists(_path: string): Promise<boolean> {
  // TODO(stub): no dedicated HTTP endpoint. Callers should be eliminated
  // in Task 5.4 as the server handles existence checks internally.
  console.warn("[fs] fileExists: no HTTP equivalent; returning false")
  return false
}

export async function getFileModifiedTime(_path: string): Promise<number> {
  // TODO(stub): no HTTP equivalent.
  console.warn("[fs] getFileModifiedTime: no HTTP equivalent; returning 0")
  return 0
}

export async function getFileSize(_path: string): Promise<number> {
  // TODO(stub): no HTTP equivalent.
  console.warn("[fs] getFileSize: no HTTP equivalent; returning 0")
  return 0
}

export async function getFileMd5(_path: string): Promise<string> {
  // TODO(stub): no HTTP equivalent.
  console.warn("[fs] getFileMd5: no HTTP equivalent; returning empty string")
  return ""
}

function assertAbsoluteFsPath(operation: string, path: string): void {
  if (!isAbsolutePath(path)) {
    throw new Error(`${operation} requires an absolute path: ${path}`)
  }
}

/** Mirror of `commands::fs::FileBase64` (Rust side). */
export interface FileBase64 {
  base64: string
  mimeType: string
}

/**
 * Read any file as base64 + guessed mime type.
 * TODO(stub): no HTTP equivalent for arbitrary base64 reads. The vision-caption
 * pipeline that used this will be refactored to go through /proxy/llm in Task 5.9.
 */
export async function readFileAsBase64(_path: string): Promise<FileBase64> {
  console.warn("[fs] readFileAsBase64: no HTTP equivalent; returning empty base64")
  return { base64: "", mimeType: "application/octet-stream" }
}

// ── Projects ──────────────────────────────────────────────────────────────────

export async function createProject(
  name: string,
  _path: string,
): Promise<WikiProject> {
  // HTTP: POST /api/v1/projects/create — body takes { name, scenario_template? }
  // Note: the old Tauri command took (name, path); the HTTP endpoint takes (name)
  // and creates under the server's projects_root. The `path` argument is ignored.
  const raw = await apiCall<RawProject>("POST", "/api/v1/projects/create", { name })
  const id = await ensureProjectId(raw.path)
  await upsertProjectInfo(id, raw.path, raw.name)
  return { id, name: raw.name, path: raw.path }
}

export async function openProject(path: string): Promise<WikiProject> {
  const raw = await apiCall<RawProject>("POST", "/api/v1/projects/open", { path })
  const id = await ensureProjectId(raw.path)
  await upsertProjectInfo(id, raw.path, raw.name)
  return { id, name: raw.name, path: raw.path }
}

export async function openProjectFolder(_path: string): Promise<void> {
  // No HTTP equivalent — this opened the OS file explorer in the Tauri desktop
  // app. In the browser/LAN context there is no OS-level action we can take.
  console.warn("[fs] openProjectFolder: no HTTP equivalent; operation is a no-op")
}

// ── Legacy server-status commands (Tauri-only, no HTTP equivalent) ────────────

export async function clipServerStatus(): Promise<string> {
  // TODO(stub): Tauri-only clip server. No HTTP equivalent.
  console.warn("[fs] clipServerStatus: Tauri-only; returning empty string")
  return ""
}

export async function apiServerStatus(): Promise<string> {
  // TODO(stub): Tauri-only legacy API server status.
  console.warn("[fs] apiServerStatus: Tauri-only; returning empty string")
  return ""
}

export async function apiServerReloadConfig(): Promise<string> {
  // TODO(stub): Tauri-only.
  console.warn("[fs] apiServerReloadConfig: Tauri-only; returning empty string")
  return ""
}

export async function mcpServerEntryPath(): Promise<string> {
  // TODO(stub): Tauri-only.
  console.warn("[fs] mcpServerEntryPath: Tauri-only; returning empty string")
  return ""
}

// Re-export fileRawUrl for callers that used to call convertFileSrc.
// Task 5.8 will update call sites to use fileRawUrl directly.
export { fileRawUrl }
