/**
 * Tests for FolderBrowserDialog API integration logic.
 *
 * We test the async network calls (list + mkdir) in isolation rather than
 * rendering the component, since the test environment is "node" (no jsdom).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest"
import { apiCall, ApiError } from "@/lib/api"

vi.mock("@/lib/api", () => ({
  apiCall: vi.fn(),
  ApiError: class ApiError extends Error {
    constructor(
      public readonly status: number,
      public readonly code: string,
      message: string,
    ) {
      super(message)
      this.name = "ApiError"
    }
  },
}))

const mockedApiCall = vi.mocked(apiCall)

interface FsEntry {
  name: string
  is_dir: boolean
  is_project: boolean
}

// ── Helpers that mirror component internals ────────────────────────────────────

async function loadEntries(path: string): Promise<{ entries: FsEntry[] } | null> {
  try {
    return await apiCall<{ entries: FsEntry[] }>(
      "GET",
      `/api/v1/fs/list?path=${encodeURIComponent(path)}`,
    )
  } catch {
    return null
  }
}

async function createFolder(
  currentPath: string,
  name: string,
): Promise<string | null> {
  const sep = currentPath.endsWith("/") ? "" : "/"
  const newPath = currentPath === "/" ? `/${name}` : `${currentPath}${sep}${name}`
  try {
    await apiCall("POST", "/api/v1/fs/mkdir", { path: newPath })
    return null
  } catch (err) {
    return err instanceof Error ? err.message : "Unknown error"
  }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

describe("FolderBrowserDialog – directory listing", () => {
  beforeEach(() => vi.clearAllMocks())
  afterEach(() => vi.restoreAllMocks())

  it("happy path: fetches entries for a given path", async () => {
    const entries: FsEntry[] = [
      { name: "research", is_dir: true, is_project: false },
      { name: "notes.md", is_dir: false, is_project: false },
    ]
    mockedApiCall.mockResolvedValueOnce({ entries })

    const result = await loadEntries("/home/user")

    expect(result).not.toBeNull()
    expect(result?.entries).toHaveLength(2)
    expect(mockedApiCall).toHaveBeenCalledWith(
      "GET",
      "/api/v1/fs/list?path=%2Fhome%2Fuser",
    )
  })

  it("returns null on API error (component will show error message)", async () => {
    mockedApiCall.mockRejectedValueOnce(new ApiError(500, "SERVER_ERROR", "oops"))

    const result = await loadEntries("/bad")

    expect(result).toBeNull()
  })

  it("encodes path with special characters", async () => {
    mockedApiCall.mockResolvedValueOnce({ entries: [] })

    await loadEntries("/path with spaces/&symbols")

    const [, url] = mockedApiCall.mock.calls[0]
    expect(url).toContain(encodeURIComponent("/path with spaces/&symbols"))
  })
})

describe("FolderBrowserDialog – create folder", () => {
  beforeEach(() => vi.clearAllMocks())
  afterEach(() => vi.restoreAllMocks())

  it("happy path: POSTs correct path and returns null error", async () => {
    mockedApiCall.mockResolvedValueOnce(undefined)

    const err = await createFolder("/home/user", "new-project")

    expect(err).toBeNull()
    expect(mockedApiCall).toHaveBeenCalledWith("POST", "/api/v1/fs/mkdir", {
      path: "/home/user/new-project",
    })
  })

  it("correctly assembles path when currentPath is root", async () => {
    mockedApiCall.mockResolvedValueOnce(undefined)

    await createFolder("/", "top-level-dir")

    expect(mockedApiCall).toHaveBeenCalledWith("POST", "/api/v1/fs/mkdir", {
      path: "/top-level-dir",
    })
  })

  it("returns error message on API failure", async () => {
    mockedApiCall.mockRejectedValueOnce(new Error("Permission denied"))

    const err = await createFolder("/home/user", "locked")

    expect(err).toBe("Permission denied")
  })
})
