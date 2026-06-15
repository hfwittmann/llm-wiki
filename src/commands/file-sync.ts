import { apiCall } from "@/lib/api"
import type { SourceWatchConfig } from "@/stores/wiki-store"
import { normalizeSourceWatchConfig } from "@/lib/source-watch-config"

export type FileChangeKind = "created" | "modified" | "deleted"
export type FileChangeStatus = "pending" | "processing" | "done" | "failed" | "superseded"

export interface FileChangeTask {
  id: string
  projectId: string
  path: string
  kind: FileChangeKind
  status: FileChangeStatus
  hashBefore?: string | null
  hashAfter?: string | null
  size?: number | null
  mtimeMs?: number | null
  createdAt: number
  updatedAt: number
  retryCount: number
  error?: string | null
  needsRerun: boolean
}

export interface FileChangeQueue {
  version: number
  tasks: FileChangeTask[]
}

export interface FileChangeRescanResult {
  queue: FileChangeQueue
  changedTasks: FileChangeTask[]
}

export interface FileSyncPayload {
  projectId: string
  tasks: FileChangeTask[]
}

/**
 * Kick off a project rescan and start the file watcher.
 * HTTP: POST /api/v1/sources/ingest — returns 202 immediately.
 * The server runs a full rescan and watches for changes.
 *
 * Note: The response shape has changed from the Tauri command — the server
 * returns 202 with an empty object, not a `FileChangeRescanResult`. We return
 * a sensible default so callers that previously consumed the result can still
 * compile; they should be updated in Task 5.4 / Task 5.5 to subscribe to SSE
 * events instead of waiting for a synchronous result.
 */
export async function startProjectFileWatcher(
  _projectId: string,
  projectPath: string,
  sourceWatchConfig?: SourceWatchConfig,
): Promise<FileChangeRescanResult> {
  void normalizeSourceWatchConfig(sourceWatchConfig) // kept for type-check; server ignores it
  await apiCall("POST", "/api/v1/sources/ingest", { project_path: projectPath })
  return { queue: { version: 0, tasks: [] }, changedTasks: [] }
}

/**
 * Stop the project file watcher.
 * The HTTP server's watcher is global (process-lifetime); there is no
 * per-client stop endpoint. This is a no-op stub.
 */
export function stopProjectFileWatcher(): Promise<void> {
  console.warn("[file-sync] stopProjectFileWatcher: server watcher is global; no-op")
  return Promise.resolve()
}

/**
 * Trigger a project rescan.
 * HTTP: POST /api/v1/sources/ingest
 */
export async function rescanProjectFiles(
  _projectId: string,
  projectPath: string,
  sourceWatchConfig?: SourceWatchConfig,
): Promise<FileChangeRescanResult> {
  void normalizeSourceWatchConfig(sourceWatchConfig)
  await apiCall("POST", "/api/v1/sources/ingest", { project_path: projectPath })
  return { queue: { version: 0, tasks: [] }, changedTasks: [] }
}

/**
 * Get the current file change queue for a project.
 * HTTP: GET /api/v1/sources/ingest/queue?project_path=...
 */
export async function getFileChangeQueue(projectPath: string): Promise<FileChangeQueue> {
  const qs = new URLSearchParams({ project_path: projectPath })
  return apiCall<FileChangeQueue>("GET", `/api/v1/sources/ingest/queue?${qs.toString()}`)
}

/**
 * Retry a failed file change task.
 * TODO: No HTTP equivalent yet. Add /sources/ingest/queue/{taskId}/retry when needed.
 */
export async function retryFileChangeTask(
  _projectId: string,
  _projectPath: string,
  _taskId: string,
): Promise<FileChangeQueue> {
  // TODO(follow-up): add server-side POST /api/v1/sources/ingest/queue/{taskId}/retry
  console.warn("[file-sync] retryFileChangeTask: no HTTP equivalent yet; returning empty queue")
  return { version: 0, tasks: [] }
}

/**
 * Ignore a failed file change task.
 * TODO: No HTTP equivalent yet. Add /sources/ingest/queue/{taskId}/ignore when needed.
 */
export async function ignoreFileChangeTask(
  _projectId: string,
  _projectPath: string,
  _taskId: string,
): Promise<FileChangeQueue> {
  // TODO(follow-up): add server-side POST /api/v1/sources/ingest/queue/{taskId}/ignore
  console.warn("[file-sync] ignoreFileChangeTask: no HTTP equivalent yet; returning empty queue")
  return { version: 0, tasks: [] }
}
