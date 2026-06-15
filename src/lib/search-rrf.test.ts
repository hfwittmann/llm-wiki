/**
 * Tests for the searchWiki HTTP wrapper.
 * After Task 5.3, `searchWiki` uses POST /api/v1/search (apiCall) instead
 * of the `search_project` Tauri invoke command. Tests updated accordingly.
 * (The file is named search-rrf.test.ts for historical reasons; RRF ranking
 * now lives server-side but the wrapper contract is still worth guarding.)
 */
import { beforeEach, describe, expect, it, vi } from "vitest"
import { useWikiStore } from "@/stores/wiki-store"

const mockApiCall = vi.fn()

vi.mock("@/lib/api", () => ({
  apiCall: (...args: unknown[]) => mockApiCall(...args),
  fileRawUrl: (projectPath: string, filePath: string) =>
    `/api/v1/files/raw?project_path=${encodeURIComponent(projectPath)}&path=${encodeURIComponent(filePath)}`,
}))

import { searchWiki, tokenizeQuery } from "./search"

beforeEach(() => {
  mockApiCall.mockReset()
  useWikiStore.getState().setEmbeddingConfig({
    enabled: true,
    endpoint: "http://test/v1/embeddings",
    apiKey: "",
    model: "test-embed",
  })
})

describe("searchWiki HTTP wrapper", () => {
  it("calls POST /api/v1/search and absolutizes paths", async () => {
    mockApiCall.mockResolvedValueOnce({
      mode: "hybrid",
      tokenHits: 1,
      vectorHits: 1,
      results: [
        {
          path: "wiki/concepts/attention.md",
          title: "Attention",
          snippet: "Attention",
          titleMatch: true,
          score: 1 / 61,
          images: [],
        },
      ],
    })

    const out = await searchWiki("/tmp/project", "attention")

    expect(mockApiCall).toHaveBeenCalledWith("POST", "/api/v1/search", {
      project_path: "/tmp/project",
      query: "attention",
      top_k: 20,
      include_content: false,
    })
    expect(out[0].path).toBe("/tmp/project/wiki/concepts/attention.md")
  })

  it("returns [] for an empty results list", async () => {
    mockApiCall.mockResolvedValueOnce({
      mode: "keyword",
      tokenHits: 0,
      vectorHits: 0,
      results: [],
    })

    const out = await searchWiki("/tmp/project", "attention")
    expect(out).toEqual([])
  })

  it("keeps CJK tokenization behavior for image caption filtering", () => {
    const tokens = tokenizeQuery("默会知识")
    expect(tokens).toContain("默会")
    expect(tokens).toContain("知识")
    expect(tokens).toContain("默")
  })
})
