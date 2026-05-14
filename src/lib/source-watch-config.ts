import type { SourceWatchConfig } from "@/stores/wiki-store"
import { normalizePath } from "@/lib/path-utils"

export const SOURCE_WATCH_DEFAULT_DOCUMENT_EXTENSIONS = [
  "md",
  "mdx",
  "txt",
  "pdf",
  "doc",
  "docx",
  "ppt",
  "pptx",
  "xls",
  "xlsx",
  "rtf",
  "html",
  "htm",
  "csv",
]

export const DEFAULT_SOURCE_WATCH_CONFIG: SourceWatchConfig = {
  enabled: true,
  autoIngest: true,
  includeExtensions: SOURCE_WATCH_DEFAULT_DOCUMENT_EXTENSIONS,
  excludeExtensions: [
    "tmp",
    "temp",
    "bak",
    "swp",
    "part",
    "partial",
    "crdownload",
    "exe",
    "dll",
    "so",
    "dylib",
    "bin",
  ],
  excludeDirs: [
    ".git",
    ".svn",
    ".hg",
    ".obsidian",
    ".idea",
    ".vscode",
    "node_modules",
    ".cache",
    "__pycache__",
  ],
  excludeGlobs: ["~$*", ".~lock.*#", "*.draft.*", "draft-*", "*.private.*"],
  maxFileSizeMb: 100,
}

export const SOURCE_WATCH_FILE_TYPE_GROUPS = [
  {
    id: "documents",
    extensions: ["md", "mdx", "txt", "pdf", "doc", "docx", "rtf"],
  },
  {
    id: "presentations",
    extensions: ["ppt", "pptx"],
  },
  {
    id: "spreadsheets",
    extensions: ["xls", "xlsx", "csv"],
  },
  {
    id: "web",
    extensions: ["html", "htm"],
  },
  {
    id: "data",
    extensions: ["json", "yaml", "yml", "xml"],
  },
  {
    id: "images",
    extensions: ["png", "jpg", "jpeg", "webp", "gif", "tiff", "bmp", "svg"],
  },
  {
    id: "media",
    extensions: ["mp3", "wav", "m4a", "mp4", "mov", "avi", "mkv", "webm"],
  },
  {
    id: "code",
    extensions: ["js", "ts", "tsx", "py", "rs", "go", "java", "cpp", "c", "h", "sh"],
  },
]

function normalizeExtensions(values: readonly string[] | undefined): string[] {
  return [...new Set((values ?? [])
    .map((value) => value.trim().replace(/^\./, "").toLowerCase())
    .filter(Boolean))]
}

function normalizeList(values: readonly string[] | undefined): string[] {
  return [...new Set((values ?? []).map((value) => value.trim()).filter(Boolean))]
}

export function normalizeSourceWatchConfig(config?: Partial<SourceWatchConfig> | null): SourceWatchConfig {
  return {
    enabled: config?.enabled ?? DEFAULT_SOURCE_WATCH_CONFIG.enabled,
    autoIngest: config?.autoIngest ?? DEFAULT_SOURCE_WATCH_CONFIG.autoIngest,
    includeExtensions: normalizeExtensions(config?.includeExtensions ?? DEFAULT_SOURCE_WATCH_CONFIG.includeExtensions),
    excludeExtensions: normalizeExtensions(config?.excludeExtensions ?? DEFAULT_SOURCE_WATCH_CONFIG.excludeExtensions),
    excludeDirs: normalizeList(config?.excludeDirs ?? DEFAULT_SOURCE_WATCH_CONFIG.excludeDirs),
    excludeGlobs: normalizeList(config?.excludeGlobs ?? DEFAULT_SOURCE_WATCH_CONFIG.excludeGlobs),
    maxFileSizeMb: Math.max(1, Math.min(4096, config?.maxFileSizeMb ?? DEFAULT_SOURCE_WATCH_CONFIG.maxFileSizeMb)),
  }
}

export function getSourceWatchExtension(path: string): string {
  const name = normalizePath(path).split("/").pop() ?? ""
  if (!name || !name.includes(".")) return ""
  return name.split(".").pop()?.toLowerCase() ?? ""
}

function wildcardToRegExp(pattern: string): RegExp {
  const escaped = pattern
    .replace(/[.+^${}()|[\]\\]/g, "\\$&")
    .replace(/\*/g, ".*")
    .replace(/\?/g, ".")
  return new RegExp(`^${escaped}$`, "i")
}

function matchesGlob(path: string, pattern: string): boolean {
  const normalized = normalizePath(path)
  const name = normalized.split("/").pop() ?? normalized
  const re = wildcardToRegExp(pattern)
  return re.test(name) || re.test(normalized)
}

export function isPathAllowedBySourceWatch(path: string, config: SourceWatchConfig): boolean {
  const cfg = normalizeSourceWatchConfig(config)
  const normalized = normalizePath(path)
  const parts = normalized.split("/").filter(Boolean)
  const lowerParts = parts.map((part) => part.toLowerCase())
  const excludedDirs = new Set(cfg.excludeDirs.map((dir) => dir.toLowerCase()))
  if (lowerParts.some((part) => excludedDirs.has(part))) return false
  if (cfg.excludeGlobs.some((pattern) => matchesGlob(normalized, pattern))) return false
  const name = parts[parts.length - 1] ?? ""
  if (!name || name.startsWith(".")) return false
  const ext = getSourceWatchExtension(normalized)
  if (ext && cfg.excludeExtensions.includes(ext)) return false
  if (cfg.includeExtensions.length > 0 && (!ext || !cfg.includeExtensions.includes(ext))) {
    return false
  }
  return true
}
