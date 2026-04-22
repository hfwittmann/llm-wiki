import { readFile, listDirectory } from "@/commands/fs"
import { invoke } from "@tauri-apps/api/core"
import type { EmbeddingConfig } from "@/stores/wiki-store"
import type { FileNode } from "@/types/wiki"
import { normalizePath } from "@/lib/path-utils"
import { getHttpFetch, isFetchNetworkError } from "@/lib/tauri-fetch"

// ── Embedding API ─────────────────────────────────────────────────────────

/**
 * Track the most recent embedding failure so the UI / callers can
 * surface *why* embedding isn't working instead of silently falling
 * back to BM25-only retrieval. Set on every failure, cleared on every
 * success. Read-only from outside via `getLastEmbeddingError`.
 */
let lastEmbeddingError: string | null = null

export function getLastEmbeddingError(): string | null {
  return lastEmbeddingError
}

async function fetchEmbedding(
  text: string,
  embeddingConfig: EmbeddingConfig,
): Promise<number[] | null> {
  if (!embeddingConfig.endpoint) return null

  const headers: Record<string, string> = { "Content-Type": "application/json" }
  if (embeddingConfig.apiKey) {
    headers["Authorization"] = `Bearer ${embeddingConfig.apiKey}`
  }

  try {
    // Route through the Tauri HTTP plugin so user-configured embedding
    // endpoints — local LM Studio / llama.cpp with `--embedding`, remote
    // proxies, enterprise gateways — aren't blocked by the same CORS
    // preflight issues the LLM path hit before we migrated it. See
    // `src/lib/tauri-fetch.ts` for why the plugin beats browser fetch
    // for third-party URLs.
    const httpFetch = await getHttpFetch()
    const resp = await httpFetch(embeddingConfig.endpoint, {
      method: "POST",
      headers,
      body: JSON.stringify({
        model: embeddingConfig.model,
        input: text.slice(0, 2000),
      }),
    })
    if (!resp.ok) {
      let detail = `${resp.status} ${resp.statusText}`
      try {
        const body = await resp.text()
        if (body) detail += ` — ${body.slice(0, 200)}`
      } catch {
        // ignore body read errors
      }
      lastEmbeddingError = `API ${detail} at ${embeddingConfig.endpoint}`
      console.warn(`[Embedding] ${lastEmbeddingError}`)
      return null
    }
    const data = await resp.json()
    const embedding = data?.data?.[0]?.embedding ?? null
    if (embedding) {
      lastEmbeddingError = null
    } else {
      lastEmbeddingError = `Response did not contain data[0].embedding (got ${JSON.stringify(data).slice(0, 200)})`
      console.warn(`[Embedding] ${lastEmbeddingError}`)
    }
    return embedding
  } catch (err) {
    // Translate cross-webview fetch failures into a single actionable
    // message that names the URL, mirroring the LLM-client treatment.
    if (isFetchNetworkError(err)) {
      lastEmbeddingError = `Network error reaching ${embeddingConfig.endpoint}. Check endpoint URL, API key, and connectivity.`
    } else {
      lastEmbeddingError = err instanceof Error ? err.message : String(err)
    }
    console.warn(`[Embedding] ${lastEmbeddingError}`)
    return null
  }
}

// ── LanceDB operations via Tauri commands ─────────────────────────────────

async function vectorUpsert(projectPath: string, pageId: string, embedding: number[]): Promise<void> {
  await invoke("vector_upsert", {
    projectPath: normalizePath(projectPath),
    pageId,
    embedding: embedding.map((v) => Math.fround(v)), // ensure f32
  })
}

async function vectorSearchLance(projectPath: string, queryEmbedding: number[], topK: number): Promise<Array<{ page_id: string; score: number }>> {
  return await invoke("vector_search", {
    projectPath: normalizePath(projectPath),
    queryEmbedding: queryEmbedding.map((v) => Math.fround(v)),
    topK,
  })
}

async function vectorDelete(projectPath: string, pageId: string): Promise<void> {
  await invoke("vector_delete", {
    projectPath: normalizePath(projectPath),
    pageId,
  })
}

async function vectorCount(projectPath: string): Promise<number> {
  return await invoke("vector_count", {
    projectPath: normalizePath(projectPath),
  })
}

// ── Public API ────────────────────────────────────────────────────────────

/**
 * Embed a single page and store in LanceDB.
 * Called after ingest to keep embeddings up to date.
 */
export async function embedPage(
  projectPath: string,
  pageId: string,
  title: string,
  content: string,
  embeddingConfig: EmbeddingConfig,
): Promise<void> {
  if (!embeddingConfig.enabled || !embeddingConfig.model) return

  const t0 = performance.now()
  const text = `${title}\n${content.slice(0, 1500)}`
  const emb = await fetchEmbedding(text, embeddingConfig)
  if (emb) {
    await vectorUpsert(projectPath, pageId, emb)
    console.log(`[Embedding] Indexed "${pageId}" (${emb.length}d) in ${Math.round(performance.now() - t0)}ms`)
  } else {
    console.log(`[Embedding] Failed to embed "${pageId}"`)
  }
}

/**
 * Embed all wiki pages that are not yet indexed.
 * Called on first enable or when model changes.
 */
export async function embedAllPages(
  projectPath: string,
  embeddingConfig: EmbeddingConfig,
  onProgress?: (done: number, total: number) => void,
): Promise<number> {
  if (!embeddingConfig.enabled || !embeddingConfig.model) return 0

  const pp = normalizePath(projectPath)

  let tree: FileNode[]
  try {
    tree = await listDirectory(`${pp}/wiki`)
  } catch {
    return 0
  }

  const mdFiles: { id: string; path: string }[] = []
  function walk(nodes: FileNode[]) {
    for (const node of nodes) {
      if (node.is_dir && node.children) {
        walk(node.children)
      } else if (!node.is_dir && node.name.endsWith(".md")) {
        const id = node.name.replace(/\.md$/, "")
        if (!["index", "log", "overview", "purpose", "schema"].includes(id)) {
          mdFiles.push({ id, path: node.path })
        }
      }
    }
  }
  walk(tree)

  let done = 0
  for (const file of mdFiles) {
    try {
      const content = await readFile(file.path)
      const titleMatch = content.match(/^---\n[\s\S]*?^title:\s*["']?(.+?)["']?\s*$/m)
      const title = titleMatch ? titleMatch[1].trim() : file.id

      const text = `${title}\n${content.slice(0, 1500)}`
      const emb = await fetchEmbedding(text, embeddingConfig)
      if (emb) {
        await vectorUpsert(pp, file.id, emb)
      }
    } catch {
      // skip
    }

    done++
    if (onProgress) onProgress(done, mdFiles.length)
  }

  return done
}

/**
 * Search wiki pages by semantic similarity via LanceDB.
 * Returns page IDs sorted by similarity score.
 */
export async function searchByEmbedding(
  projectPath: string,
  query: string,
  embeddingConfig: EmbeddingConfig,
  topK: number = 10,
): Promise<Array<{ id: string; score: number }>> {
  if (!embeddingConfig.enabled || !embeddingConfig.model) return []

  const queryEmb = await fetchEmbedding(query, embeddingConfig)
  if (!queryEmb) return []

  try {
    const t0 = performance.now()
    const results = await vectorSearchLance(projectPath, queryEmb, topK)
    console.log(`[Embedding] LanceDB search: ${results.length} results in ${Math.round(performance.now() - t0)}ms`)
    return results.map((r) => ({ id: r.page_id, score: r.score }))
  } catch (err) {
    console.log(`[Embedding] LanceDB search failed: ${err instanceof Error ? err.message : err}`)
    return []
  }
}

/**
 * Remove a page from the vector index.
 */
export async function removePageEmbedding(
  projectPath: string,
  pageId: string,
): Promise<void> {
  try {
    await vectorDelete(projectPath, pageId)
  } catch {
    // non-critical
  }
}

/**
 * Get the number of indexed vectors.
 */
export async function getEmbeddingCount(
  projectPath: string,
): Promise<number> {
  try {
    return await vectorCount(projectPath)
  } catch {
    return 0
  }
}
