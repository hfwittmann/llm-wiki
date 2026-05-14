import { describe, expect, it } from "vitest"
import {
  scheduledImportDestinationForFile,
  shouldSkipScheduledImportFile,
} from "./scheduled-import"

describe("scheduled import path handling", () => {
  const projectPath = "/Users/me/wiki-project"

  it("preserves nested relative paths for external directories", () => {
    const dest = scheduledImportDestinationForFile(
      projectPath,
      "/Users/me/inbox",
      {
        name: "report.pdf",
        path: "/Users/me/inbox/a/report.pdf",
      },
    )

    expect(dest).toBe(
      "/Users/me/wiki-project/raw/sources/scheduled-import/a/report.pdf",
    )
  })

  it("does not copy files that are already under raw/sources", () => {
    const dest = scheduledImportDestinationForFile(
      projectPath,
      `${projectPath}/raw/sources`,
      {
        name: "source.md",
        path: `${projectPath}/raw/sources/source.md`,
      },
    )

    expect(dest).toBe(`${projectPath}/raw/sources/source.md`)
  })

  it("sanitizes Windows-unsafe destination path segments", () => {
    const dest = scheduledImportDestinationForFile(
      projectPath,
      "/Users/me/inbox",
      {
        name: "ignored.md",
        path: "/Users/me/inbox/CON/Article: Why?.md",
      },
    )

    expect(dest).toBe(
      "/Users/me/wiki-project/raw/sources/scheduled-import/_CON/Article_ Why_.md",
    )
  })

  it("skips project internals and generated wiki/cache files", () => {
    expect(
      shouldSkipScheduledImportFile(projectPath, `${projectPath}/.llm-wiki/db.json`),
    ).toBe(true)
    expect(
      shouldSkipScheduledImportFile(projectPath, `${projectPath}/wiki/index.md`),
    ).toBe(true)
    expect(
      shouldSkipScheduledImportFile(
        projectPath,
        `${projectPath}/raw/sources/.cache/source.pdf.txt`,
      ),
    ).toBe(true)
  })
})
