import {
  copyFile,
  fileExists,
  getFileMd5,
  listDirectory,
  preprocessFile,
  readFile,
  writeFile,
} from "@/commands/fs"
import type { FileNode, WikiProject } from "@/types/wiki"
import { normalizePath } from "@/lib/path-utils"
import { useWikiStore } from "@/stores/wiki-store"
import type { ScheduledImportConfig } from "@/stores/wiki-store"
import {
  loadScheduledImportConfig,
  saveScheduledImportConfig,
} from "@/lib/project-store"
import {
  enqueueSourceIngest,
  isIngestableSourcePath,
} from "@/lib/source-lifecycle"

interface ImportDb {
  files: Record<string, string>
  lastScan: number | null
}

interface ImportDbStore {
  version: 1
  directories: Record<string, ImportDb>
}

type ScanOptions = {
  runId?: number
}

const EMPTY_DB: ImportDb = {
  files: {},
  lastScan: null,
}

let scanTimer: ReturnType<typeof setInterval> | null = null
let scanning = false
let activeRunId = 0

const DB_PATH = ".llm-wiki/scheduled-import-db.json"
const LEGACY_DB_DIR = ".llm-wiki-imported"
const SCHEDULED_IMPORT_DIR = "scheduled-import"
const RESERVED_WINDOWS_NAMES = new Set([
  "con",
  "prn",
  "aux",
  "nul",
  "com1",
  "com2",
  "com3",
  "com4",
  "com5",
  "com6",
  "com7",
  "com8",
  "com9",
  "lpt1",
  "lpt2",
  "lpt3",
  "lpt4",
  "lpt5",
  "lpt6",
  "lpt7",
  "lpt8",
  "lpt9",
])

function emptyStore(): ImportDbStore {
  return { version: 1, directories: {} }
}

function dbFilePath(projectPath: string): string {
  return `${normalizePath(projectPath)}/${DB_PATH}`
}

function dbDirectoryKey(importPath: string): string {
  return normalizePath(importPath)
}

function cloneDb(db: ImportDb): ImportDb {
  return {
    files: { ...db.files },
    lastScan: db.lastScan,
  }
}

function isPathInside(path: string, parent: string): boolean {
  const normalizedPath = normalizePath(path)
  const normalizedParent = normalizePath(parent).replace(/\/+$/, "")
  return (
    normalizedPath === normalizedParent ||
    normalizedPath.startsWith(`${normalizedParent}/`)
  )
}

function projectSubpath(projectPath: string, relPath: string): string {
  return `${normalizePath(projectPath)}/${relPath}`
}

function sanitizePathSegment(segment: string): string {
  let value = segment
    .replace(/[<>:"|?*\x00-\x1F]/g, "_")
    .replace(/[. ]+$/g, "")
    .trim()

  if (!value) {
    value = "_"
  }

  const stem = value.split(".")[0]?.toLowerCase() ?? value.toLowerCase()
  if (RESERVED_WINDOWS_NAMES.has(stem)) {
    value = `_${value}`
  }

  return value
}

function safeRelativePath(path: string): string {
  const parts = normalizePath(path)
    .split("/")
    .filter((part) => part && part !== "." && part !== "..")
    .map(sanitizePathSegment)

  return parts.length > 0 ? parts.join("/") : "_"
}

export function isScheduledImportInternalPath(path: string): boolean {
  const parts = normalizePath(path).split("/")
  return parts.includes(LEGACY_DB_DIR) || parts.includes(".llm-wiki")
}

export function shouldSkipScheduledImportFile(
  projectPath: string,
  filePath: string,
): boolean {
  const path = normalizePath(filePath)
  const project = normalizePath(projectPath)

  if (isScheduledImportInternalPath(path)) {
    return true
  }

  if (isPathInside(path, projectSubpath(project, "wiki"))) {
    return true
  }

  if (isPathInside(path, projectSubpath(project, "raw/sources/.cache"))) {
    return true
  }

  const name = path.split("/").pop() ?? ""
  return name.startsWith(".")
}

export function scheduledImportDestinationForFile(
  projectPath: string,
  importPath: string,
  file: Pick<FileNode, "path" | "name">,
): string {
  const project = normalizePath(projectPath)
  const source = normalizePath(file.path)
  const sourcesRoot = projectSubpath(project, "raw/sources")

  if (isPathInside(source, sourcesRoot)) {
    return source
  }

  const importRoot = normalizePath(importPath).replace(/\/+$/, "")
  const relative =
    source === importRoot || !source.startsWith(`${importRoot}/`)
      ? file.name
      : source.slice(importRoot.length + 1)

  return `${sourcesRoot}/${SCHEDULED_IMPORT_DIR}/${safeRelativePath(relative)}`
}

function collectFiles(nodes: FileNode[]): FileNode[] {
  const files: FileNode[] = []
  for (const node of nodes) {
    if (!node.is_dir) {
      files.push(node)
    } else if (node.children) {
      files.push(...collectFiles(node.children))
    }
  }
  return files
}

async function loadDbStore(projectPath: string): Promise<ImportDbStore> {
  const path = dbFilePath(projectPath)
  try {
    if (!(await fileExists(path))) {
      return emptyStore()
    }
    const content = await readFile(path)
    const parsed = JSON.parse(content) as Partial<ImportDbStore>
    if (!parsed.directories || typeof parsed.directories !== "object") {
      return emptyStore()
    }
    return {
      version: 1,
      directories: parsed.directories as Record<string, ImportDb>,
    }
  } catch (err) {
    console.warn("Failed to load scheduled import database:", err)
    return emptyStore()
  }
}

async function loadImportDb(
  projectPath: string,
  importPath: string,
): Promise<ImportDb> {
  const store = await loadDbStore(projectPath)
  const db = store.directories[dbDirectoryKey(importPath)]
  return db ? cloneDb(db) : cloneDb(EMPTY_DB)
}

async function saveImportDb(
  projectPath: string,
  importPath: string,
  db: ImportDb,
): Promise<void> {
  const store = await loadDbStore(projectPath)
  store.directories[dbDirectoryKey(importPath)] = cloneDb(db)
  await writeFile(dbFilePath(projectPath), JSON.stringify(store, null, 2))
}

function isCurrentProject(projectId: string): boolean {
  return useWikiStore.getState().project?.id === projectId
}

function isCurrentRun(projectId: string, runId?: number): boolean {
  return isCurrentProject(projectId) && (runId === undefined || runId === activeRunId)
}

export async function scanAndImport(
  project: WikiProject,
  importPath: string,
  options: ScanOptions = {},
): Promise<void> {
  if (!importPath || scanning) return

  scanning = true
  const projectPath = normalizePath(project.path)
  const importRoot = normalizePath(importPath)

  try {
    if (!isCurrentRun(project.id, options.runId)) {
      return
    }

    const tree = await listDirectory(importRoot)
    const db = await loadImportDb(projectPath, importRoot)
    const nextDb: ImportDb = {
      files: {},
      lastScan: Date.now(),
    }
    const llmConfig = useWikiStore.getState().llmConfig
    const destPaths: string[] = []

    for (const file of collectFiles(tree)) {
      const sourcePath = normalizePath(file.path)
      if (
        shouldSkipScheduledImportFile(projectPath, sourcePath) ||
        !isIngestableSourcePath(sourcePath)
      ) {
        continue
      }

      if (!isCurrentRun(project.id, options.runId)) {
        return
      }

      const key = sourcePath
      const md5 = await getFileMd5(sourcePath)
      nextDb.files[key] = md5

      if (db.files[key] === md5) {
        continue
      }

      const destPath = scheduledImportDestinationForFile(projectPath, importRoot, file)
      if (normalizePath(destPath) !== sourcePath) {
        await copyFile(sourcePath, destPath)
      }
      destPaths.push(destPath)
    }

    if (!isCurrentRun(project.id, options.runId)) {
      return
    }

    await saveImportDb(projectPath, importRoot, nextDb)

    if (destPaths.length > 0) {
      await Promise.all(destPaths.map((path) => preprocessFile(path).catch(() => {})))
      if (isCurrentRun(project.id, options.runId)) {
        await enqueueSourceIngest(project, destPaths, llmConfig)
        const projectTree = await listDirectory(projectPath)
        useWikiStore.getState().setFileTree(projectTree)
        useWikiStore.getState().bumpDataVersion()
      }
    }

    const currentConfig = (await loadScheduledImportConfig(projectPath)) ?? {
      enabled: false,
      path: importRoot,
      interval: 60,
      lastScan: null,
    }
    await saveScheduledImportConfig(projectPath, {
      ...currentConfig,
      lastScan: nextDb.lastScan,
    })

    if (isCurrentProject(project.id)) {
      useWikiStore.getState().setScheduledImportConfig({
        ...currentConfig,
        lastScan: nextDb.lastScan,
      })
    }
  } catch (err) {
    console.error("Scheduled import scan failed:", err)
  } finally {
    scanning = false
  }
}

export function startScheduledImport(
  project: WikiProject,
  config: ScheduledImportConfig,
): void {
  stopScheduledImport()

  if (!config.enabled || !config.path || config.interval <= 0) {
    return
  }

  const runId = ++activeRunId
  const intervalMs = Math.max(1, Math.min(1440, config.interval)) * 60 * 1000

  void scanAndImport(project, config.path, { runId })

  scanTimer = setInterval(() => {
    void scanAndImport(project, config.path, { runId })
  }, intervalMs)
}

export function stopScheduledImport(): void {
  activeRunId += 1
  if (scanTimer) {
    clearInterval(scanTimer)
    scanTimer = null
  }
}
