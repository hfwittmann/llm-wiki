import { useState, useEffect, useCallback } from "react"
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from "@/components/ui/dialog"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { apiCall } from "@/lib/api"
import { Folder, FolderOpen, ChevronRight, FilePlus } from "lucide-react"

// ─── Types ────────────────────────────────────────────────────────────────────

interface FsEntry {
  name: string
  is_dir: boolean
  is_project: boolean
  size?: number
  modified_unix?: number
}

interface FsListResponse {
  entries: FsEntry[]
}

export interface FolderBrowserDialogProps {
  open: boolean
  onClose: () => void
  onSelect: (path: string) => void
  /** Optional starting path; defaults to root "/" */
  initialPath?: string
  /** Title shown at the top of the dialog */
  title?: string
}

// ─── Component ────────────────────────────────────────────────────────────────

export function FolderBrowserDialog({
  open,
  onClose,
  onSelect,
  initialPath = "/",
  title = "Select Folder",
}: FolderBrowserDialogProps) {
  const [currentPath, setCurrentPath] = useState(initialPath)
  const [entries, setEntries] = useState<FsEntry[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // "create folder" inline form state
  const [showNewFolder, setShowNewFolder] = useState(false)
  const [newFolderName, setNewFolderName] = useState("")
  const [creating, setCreating] = useState(false)
  const [createError, setCreateError] = useState<string | null>(null)

  // Reset state whenever the dialog opens with a (potentially different) initial path
  useEffect(() => {
    if (open) {
      setCurrentPath(initialPath)
      setShowNewFolder(false)
      setNewFolderName("")
      setCreateError(null)
      setError(null)
    }
  }, [open, initialPath])

  const loadEntries = useCallback(async (path: string) => {
    setLoading(true)
    setError(null)
    try {
      const resp = await apiCall<FsListResponse>(
        "GET",
        `/api/v1/fs/list?path=${encodeURIComponent(path)}`,
      )
      setEntries(resp.entries ?? [])
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      setEntries([])
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    if (open) {
      loadEntries(currentPath)
    }
  }, [open, currentPath, loadEntries])

  // ── Breadcrumb ──────────────────────────────────────────────────────────────

  const breadcrumbSegments = (() => {
    // Build [{label, path}] from root down to currentPath
    const normalized = currentPath.replace(/\/+$/, "") || "/"
    if (normalized === "/") return [{ label: "/", path: "/" }]
    const parts = normalized.split("/").filter(Boolean)
    const segs: { label: string; path: string }[] = [{ label: "/", path: "/" }]
    let accumulated = ""
    for (const part of parts) {
      accumulated += "/" + part
      segs.push({ label: part, path: accumulated })
    }
    return segs
  })()

  function navigateTo(path: string) {
    setShowNewFolder(false)
    setNewFolderName("")
    setCreateError(null)
    setCurrentPath(path)
  }

  function navigateUp() {
    const normalized = currentPath.replace(/\/+$/, "") || "/"
    if (normalized === "/") return
    const parent = normalized.substring(0, normalized.lastIndexOf("/")) || "/"
    navigateTo(parent)
  }

  // ── Create folder ───────────────────────────────────────────────────────────

  async function handleCreateFolder() {
    const name = newFolderName.trim()
    if (!name) {
      setCreateError("Folder name cannot be empty.")
      return
    }
    const sep = currentPath.endsWith("/") ? "" : "/"
    const newPath = currentPath === "/" ? `/${name}` : `${currentPath}${sep}${name}`
    setCreating(true)
    setCreateError(null)
    try {
      await apiCall("POST", "/api/v1/fs/mkdir", { path: newPath })
      setShowNewFolder(false)
      setNewFolderName("")
      await loadEntries(currentPath)
    } catch (err) {
      setCreateError(err instanceof Error ? err.message : String(err))
    } finally {
      setCreating(false)
    }
  }

  // ── Select ──────────────────────────────────────────────────────────────────

  function handleSelect() {
    onSelect(currentPath)
    onClose()
  }

  // ── Render ──────────────────────────────────────────────────────────────────

  return (
    <Dialog open={open} onOpenChange={(isOpen) => { if (!isOpen) onClose() }}>
      <DialogContent
        className="max-w-lg max-h-[80vh] grid-rows-[auto_1fr_auto] overflow-hidden"
        showCloseButton
      >
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>

          {/* Breadcrumb */}
          <nav className="flex flex-wrap items-center gap-0.5 text-xs text-muted-foreground pt-1">
            {breadcrumbSegments.map((seg, i) => (
              <span key={seg.path} className="flex items-center gap-0.5">
                {i > 0 && <ChevronRight className="size-3 shrink-0" />}
                <button
                  type="button"
                  className="hover:text-foreground underline-offset-2 hover:underline"
                  onClick={() => navigateTo(seg.path)}
                >
                  {seg.label}
                </button>
              </span>
            ))}
          </nav>
        </DialogHeader>

        {/* Directory listing */}
        <div className="min-h-0 overflow-y-auto rounded-lg border bg-muted/30 py-1">
          {loading && (
            <p className="px-3 py-4 text-center text-xs text-muted-foreground">Loading…</p>
          )}
          {!loading && error && (
            <p className="px-3 py-2 text-xs text-destructive">{error}</p>
          )}
          {!loading && !error && entries.length === 0 && (
            <p className="px-3 py-4 text-center text-xs text-muted-foreground">
              Empty directory
            </p>
          )}
          {!loading &&
            !error &&
            entries.map((entry) => {
              if (entry.is_dir) {
                return (
                  <button
                    key={entry.name}
                    type="button"
                    className="flex w-full items-center gap-2 px-3 py-1.5 text-sm hover:bg-muted/60 text-left"
                    onClick={() => {
                      const sep = currentPath.endsWith("/") ? "" : "/"
                      const next =
                        currentPath === "/"
                          ? `/${entry.name}`
                          : `${currentPath}${sep}${entry.name}`
                      navigateTo(next)
                    }}
                  >
                    {entry.is_project ? (
                      <FolderOpen className="size-4 shrink-0 text-primary" />
                    ) : (
                      <Folder className="size-4 shrink-0 text-muted-foreground" />
                    )}
                    <span className="flex-1 truncate">{entry.name}</span>
                    {entry.is_project && (
                      <span className="rounded bg-primary/10 px-1.5 py-0.5 text-xs text-primary">
                        project
                      </span>
                    )}
                    <ChevronRight className="size-3.5 shrink-0 text-muted-foreground" />
                  </button>
                )
              }
              // Files — greyed out, not clickable
              return (
                <div
                  key={entry.name}
                  className="flex items-center gap-2 px-3 py-1.5 text-sm text-muted-foreground/60 select-none"
                >
                  <FilePlus className="size-4 shrink-0" />
                  <span className="truncate">{entry.name}</span>
                </div>
              )
            })}
        </div>

        {/* Inline "create folder" form */}
        {showNewFolder ? (
          <div className="flex flex-col gap-1.5">
            <div className="flex gap-2">
              <Input
                autoFocus
                value={newFolderName}
                onChange={(e) => setNewFolderName(e.target.value)}
                placeholder="New folder name"
                onKeyDown={(e) => {
                  if (e.key === "Enter") handleCreateFolder()
                  if (e.key === "Escape") {
                    setShowNewFolder(false)
                    setNewFolderName("")
                    setCreateError(null)
                  }
                }}
              />
              <Button onClick={handleCreateFolder} disabled={creating} size="default">
                {creating ? "Creating…" : "Create"}
              </Button>
              <Button
                variant="outline"
                onClick={() => {
                  setShowNewFolder(false)
                  setNewFolderName("")
                  setCreateError(null)
                }}
              >
                Cancel
              </Button>
            </div>
            {createError && (
              <p className="text-xs text-destructive">{createError}</p>
            )}
          </div>
        ) : null}

        <DialogFooter>
          <Button
            variant="outline"
            size="sm"
            onClick={() => setShowNewFolder(true)}
            disabled={showNewFolder}
          >
            <Folder className="size-3.5" />
            New folder
          </Button>
          {/* Spacer pushes nav-up + cancel + select to the right */}
          <span className="flex-1" />
          <Button
            variant="outline"
            size="sm"
            onClick={navigateUp}
            disabled={currentPath === "/"}
          >
            Up
          </Button>
          <Button variant="outline" onClick={onClose} size="sm">
            Cancel
          </Button>
          <Button onClick={handleSelect} size="sm">
            Select this folder
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
