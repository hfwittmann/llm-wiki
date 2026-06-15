import { beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({
  apiCall: vi.fn(),
}))

vi.mock("@/lib/api", () => ({
  apiCall: mocks.apiCall,
  fileRawUrl: (projectPath: string, filePath: string) =>
    `/api/v1/files/raw?project_path=${encodeURIComponent(projectPath)}&path=${encodeURIComponent(filePath)}`,
}))

import { createDirectory, writeFile, writeFileAtomic } from "./fs"

describe("fs command path guards", () => {
  beforeEach(() => {
    mocks.apiCall.mockReset()
  })

  it("rejects relative write paths before calling apiCall", async () => {
    await expect(writeFile("wiki/sources/stray.md", "content")).rejects.toThrow(
      /absolute path/i,
    )

    expect(mocks.apiCall).not.toHaveBeenCalled()
  })

  it("rejects relative atomic write paths before calling apiCall", async () => {
    await expect(writeFileAtomic("wiki/sources/stray.md", "content")).rejects.toThrow(
      /absolute path/i,
    )

    expect(mocks.apiCall).not.toHaveBeenCalled()
  })

  it("no-ops for createDirectory in browser/LAN mode (stub, does not throw)", async () => {
    // createDirectory is a no-op stub in the browser/LAN port — the server
    // handles directory creation internally. It no longer has a path guard
    // since it doesn't make any real call. The path guard test is replaced
    // with a smoke test confirming the stub resolves without throwing.
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {})
    await expect(createDirectory("wiki/sources")).resolves.toBeUndefined()
    expect(mocks.apiCall).not.toHaveBeenCalled()
    warnSpy.mockRestore()
  })

  it("allows absolute write paths (logs warning, server-side write is a stub until Task 5.4)", async () => {
    // writeFile with an absolute path no longer calls apiCall — it's a stub
    // that logs a warning until Task 5.4 rewires callers to pass project_path
    // + page_path + etag for the /wiki/page PUT endpoint.
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {})
    await writeFile("/tmp/project/wiki/sources/page.md", "content")
    expect(mocks.apiCall).not.toHaveBeenCalled()
    expect(warnSpy).toHaveBeenCalledWith(expect.stringContaining("writeFile"))
    warnSpy.mockRestore()
  })
})
