/**
 * Search ranking scenarios moved to Rust with the shared backend search
 * service (`src-tauri/src/commands/search.rs`). The WebView now only
 * wraps that endpoint, so this file guards the HTTP command contract from
 * the TS side instead of duplicating ranking logic in Node.
 */
import { beforeEach, describe, expect, it, vi } from "vitest"
import { useWikiStore } from "@/stores/wiki-store"

const mockApiCall = vi.fn()

vi.mock("@/lib/api", () => ({
  apiCall: (...args: unknown[]) => mockApiCall(...args),
  fileRawUrl: (projectPath: string, filePath: string) =>
    `/api/v1/files/raw?project_path=${encodeURIComponent(projectPath)}&path=${encodeURIComponent(filePath)}`,
}))

import { searchWiki } from "./search"

beforeEach(() => {
  mockApiCall.mockReset()
  useWikiStore.getState().setEmbeddingConfig({
    enabled: false,
    endpoint: "",
    apiKey: "",
    model: "",
  })
})

describe("searchWiki HTTP endpoint contract", () => {
  it("delegates ranking to POST /api/v1/search and maps relative wiki paths to absolute paths", async () => {
    mockApiCall.mockResolvedValueOnce({
      mode: "keyword",
      tokenHits: 1,
      vectorHits: 0,
      results: [
        {
          path: "wiki/concepts/attention.md",
          title: "Attention",
          snippet: "body",
          titleMatch: true,
          score: 1 / 61,
          images: [],
        },
      ],
    })

    const results = await searchWiki("/tmp/project", "attention")

    expect(mockApiCall).toHaveBeenCalledWith("POST", "/api/v1/search", {
      project_path: "/tmp/project",
      query: "attention",
      top_k: 20,
      include_content: false,
    })
    expect(results[0].path).toBe("/tmp/project/wiki/concepts/attention.md")
  })
})
