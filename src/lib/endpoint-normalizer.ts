/**
 * Clean up user-entered LLM endpoint URLs. Catches the two most common
 * mistakes:
 *
 *   1. User pastes the full path (e.g. ".../v1/chat/completions") — our
 *      dispatch would then append ANOTHER "/chat/completions" on top,
 *      producing a 404. Always strip trailing path segments that belong
 *      on the request, not on the base.
 *
 *   2. User forgets the version segment entirely (e.g. "https://host.com"
 *      with no /v1). We can't auto-add it because providers use different
 *      segments (OpenAI `/v1`, Zhipu `/api/paas/v4`, Groq `/openai/v1`) —
 *      but we CAN flag it so the user sees the hint.
 *
 * Auto-fixes apply deterministically on blur; hints explain what happened.
 * Warnings are shown inline but never block saving — some self-hosted
 * gateways really do mount the API at a bare host.
 */

export type EndpointMode = "chat_completions" | "anthropic_messages"

export interface NormalizedEndpoint {
  /** The cleaned-up URL to store. Empty string for empty input. */
  normalized: string
  /** True if normalization changed the input (show a "will use" hint). */
  changed: boolean
  /** Human-readable hint / warning. Undefined when the input is fine. */
  warning?: string
}

// Path tails that are always wrong as a base URL and can be safely
// stripped regardless of mode — these belong on the request, not on the
// configured endpoint.
const ALWAYS_WRONG_TAILS = /\/+(chat\/completions|embeddings)\/?$/i
// `/messages` is ambiguous: in anthropic_messages mode our dispatch uses
// it verbatim when present, so we must preserve it. Only strip when the
// configured mode is chat_completions.
const MESSAGES_TAIL = /\/+messages\/?$/i

export function normalizeEndpoint(raw: string, mode: EndpointMode): NormalizedEndpoint {
  const trimmed = (raw ?? "").trim()
  if (!trimmed) return { normalized: "", changed: false }

  // Detect missing protocol — we never auto-add https:// because that
  // would mask the user's typo; just flag it.
  const missingProtocol = !/^https?:\/\//i.test(trimmed)
  if (missingProtocol) {
    return {
      normalized: trimmed.replace(/\/+$/, ""),
      changed: trimmed !== trimmed.replace(/\/+$/, ""),
      warning: "URL should start with http:// or https://",
    }
  }

  let url = trimmed
  const notes: string[] = []

  // Strip trailing slashes (cheap, always safe)
  url = url.replace(/\/+$/, "")

  // Strip request-path tails users paste by accident. Works in both
  // modes for /chat/completions and /embeddings (wrong shape for either
  // wire). /messages is only wrong in chat_completions mode — in
  // anthropic_messages mode the dispatch uses it verbatim.
  if (ALWAYS_WRONG_TAILS.test(url)) {
    const match = url.match(ALWAYS_WRONG_TAILS)
    url = url.replace(ALWAYS_WRONG_TAILS, "")
    if (match) notes.push(`stripped trailing "${match[0].replace(/^\/+/, "").replace(/\/+$/, "")}" — this is appended per-request, not part of the base URL`)
  } else if (mode === "chat_completions" && MESSAGES_TAIL.test(url)) {
    const match = url.match(MESSAGES_TAIL)
    url = url.replace(MESSAGES_TAIL, "")
    if (match) notes.push(`stripped trailing "${match[0].replace(/^\/+/, "").replace(/\/+$/, "")}" — this is an Anthropic-wire path, not a chat/completions base`)
  }

  // After stripping, check for the "bare host, no version segment" case.
  // Only hint for chat_completions — anthropic_messages endpoints sit at
  // various non-/v1 paths (MiniMax `/anthropic`, Anthropic native `/`)
  // and we can't reliably flag them.
  if (mode === "chat_completions") {
    try {
      const u = new URL(url)
      const pathname = u.pathname.replace(/\/+$/, "")
      const hasVersionSegment = /\/(v\d+|paas\/v\d+|openai\/v\d+|api\/v\d+)$/i.test(pathname)
      if (!hasVersionSegment && !notes.length) {
        notes.push('URL has no version segment (expected e.g. "/v1"). Double-check the provider\'s docs.')
      }
    } catch {
      // Malformed URL — leave alone, browser will fail loudly at fetch time.
    }
  }

  const changed = url !== trimmed
  return {
    normalized: url,
    changed,
    warning: notes.length ? notes.join(" ") : undefined,
  }
}
