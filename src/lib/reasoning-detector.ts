/**
 * Diagnostic helper for "model emitted thinking but no actual answer"
 * symptoms.
 *
 * Some OpenAI-compatible endpoints (DeepSeek-R1, Kimi K2.x, Qwen
 * reasoning models, various third-party deployments) stream the
 * model's chain-of-thought through a non-content delta field —
 * either `reasoning_content` (DeepSeek/Kimi convention) or just
 * `reasoning` (some Qwen-flavored deployments). The user-facing
 * answer normally appears in `delta.content` AFTER the thinking
 * phase completes.
 *
 * When an endpoint misbehaves (max_tokens too small, server-side
 * thinking budget exhaustion, model bug) it can emit megabytes of
 * reasoning text and then end the stream with no content at all.
 * The streaming layer's parser correctly ignores reasoning fields
 * (we don't want to leak chain-of-thought into the user's wiki
 * output), but it leaves us with a silent empty-analysis result —
 * the user sees a meaningless "analysis not available" with no
 * actionable diagnosis.
 *
 * This helper does ONE thing: tally the byte-length of reasoning
 * text seen on a raw SSE line, so the streaming layer can
 * distinguish two stream-end states:
 *
 *   - 0 content + 0 reasoning  → plain empty response, network /
 *     auth / rate-limit territory; the existing error paths cover
 *     this.
 *   - 0 content + N>>0 reasoning → the diagnostic case above;
 *     surface "model only produced N chars of thinking, no final
 *     answer" instead of silently emptying the analysis.
 *
 * Implementation note: counts the JSON-escaped form's length
 * (e.g. `\\n` counts as 2). Close enough for a threshold check —
 * we're distinguishing "0 vs hundreds of chars", not measuring
 * exact tokens.
 */

const REASONING_FIELD_RE =
  /"reasoning(?:_content)?"\s*:\s*"((?:[^"\\]|\\.)*)"/g

export function countReasoningCharsInLine(rawLine: string): number {
  let total = 0
  for (const match of rawLine.matchAll(REASONING_FIELD_RE)) {
    total += match[1].length
  }
  return total
}
