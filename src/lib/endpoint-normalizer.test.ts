import { describe, it, expect } from "vitest"
import { normalizeEndpoint } from "./endpoint-normalizer"

describe("normalizeEndpoint — chat_completions mode", () => {
  it("leaves a well-formed URL untouched", () => {
    const r = normalizeEndpoint("https://api.openai.com/v1", "chat_completions")
    expect(r.normalized).toBe("https://api.openai.com/v1")
    expect(r.changed).toBe(false)
    expect(r.warning).toBeUndefined()
  })

  it("strips trailing slash", () => {
    const r = normalizeEndpoint("https://api.openai.com/v1/", "chat_completions")
    expect(r.normalized).toBe("https://api.openai.com/v1")
    expect(r.changed).toBe(true)
  })

  it("strips pasted /chat/completions", () => {
    const r = normalizeEndpoint(
      "https://api.openai.com/v1/chat/completions",
      "chat_completions",
    )
    expect(r.normalized).toBe("https://api.openai.com/v1")
    expect(r.changed).toBe(true)
    expect(r.warning).toMatch(/chat\/completions/)
  })

  it("strips pasted /chat/completions with trailing slash", () => {
    const r = normalizeEndpoint(
      "https://api.openai.com/v1/chat/completions/",
      "chat_completions",
    )
    expect(r.normalized).toBe("https://api.openai.com/v1")
  })

  it("strips pasted /embeddings", () => {
    const r = normalizeEndpoint(
      "https://api.openai.com/v1/embeddings",
      "chat_completions",
    )
    expect(r.normalized).toBe("https://api.openai.com/v1")
  })

  it("preserves non-/v1 version segments (Zhipu, Arcee, etc.)", () => {
    const r = normalizeEndpoint(
      "https://open.bigmodel.cn/api/paas/v4",
      "chat_completions",
    )
    expect(r.normalized).toBe("https://open.bigmodel.cn/api/paas/v4")
    expect(r.changed).toBe(false)
  })

  it("strips /chat/completions while keeping a non-/v1 version segment", () => {
    const r = normalizeEndpoint(
      "https://open.bigmodel.cn/api/paas/v4/chat/completions",
      "chat_completions",
    )
    expect(r.normalized).toBe("https://open.bigmodel.cn/api/paas/v4")
  })

  it("warns when the URL is a bare host with no version path", () => {
    const r = normalizeEndpoint("https://api.openai.com", "chat_completions")
    // Don't auto-add /v1 — different providers use different segments.
    expect(r.normalized).toBe("https://api.openai.com")
    expect(r.warning).toMatch(/v1|version/i)
  })

  it("warns when protocol is missing", () => {
    const r = normalizeEndpoint("api.openai.com/v1", "chat_completions")
    expect(r.warning).toMatch(/https?:\/\//i)
  })

  it("handles empty / whitespace input", () => {
    expect(normalizeEndpoint("", "chat_completions").normalized).toBe("")
    expect(normalizeEndpoint("   ", "chat_completions").normalized).toBe("")
  })

  it("strips enclosing whitespace", () => {
    const r = normalizeEndpoint("  https://api.openai.com/v1  ", "chat_completions")
    expect(r.normalized).toBe("https://api.openai.com/v1")
  })

  it("handles localhost with a port and version segment", () => {
    const r = normalizeEndpoint("http://localhost:8080/v1", "chat_completions")
    expect(r.normalized).toBe("http://localhost:8080/v1")
    expect(r.changed).toBe(false)
  })

  it("strips /chat/completions on a localhost llama.cpp URL", () => {
    const r = normalizeEndpoint(
      "http://192.168.1.50:8080/v1/chat/completions",
      "chat_completions",
    )
    expect(r.normalized).toBe("http://192.168.1.50:8080/v1")
  })
})

describe("normalizeEndpoint — anthropic_messages mode", () => {
  it("keeps a bare /anthropic base as-is (dispatch appends /v1/messages)", () => {
    const r = normalizeEndpoint("https://api.minimax.io/anthropic", "anthropic_messages")
    expect(r.normalized).toBe("https://api.minimax.io/anthropic")
    expect(r.changed).toBe(false)
  })

  it("keeps a full /v1/messages URL as-is (dispatch uses it verbatim)", () => {
    const r = normalizeEndpoint(
      "https://api.anthropic.com/v1/messages",
      "anthropic_messages",
    )
    expect(r.normalized).toBe("https://api.anthropic.com/v1/messages")
    expect(r.changed).toBe(false)
  })

  it("strips trailing slash on an anthropic base", () => {
    const r = normalizeEndpoint("https://api.minimax.io/anthropic/", "anthropic_messages")
    expect(r.normalized).toBe("https://api.minimax.io/anthropic")
    expect(r.changed).toBe(true)
  })

  it("strips stray /chat/completions (user pasted the wrong shape)", () => {
    const r = normalizeEndpoint(
      "https://api.anthropic.com/v1/chat/completions",
      "anthropic_messages",
    )
    expect(r.warning).toMatch(/chat\/completions/)
    expect(r.normalized).toBe("https://api.anthropic.com/v1")
  })
})
