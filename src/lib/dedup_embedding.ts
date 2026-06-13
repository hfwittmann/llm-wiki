/**
 * dedup_embedding.ts
 *
 * Vector-embedding candidate generation for duplicate-page scan.
 * Pre-filters pages by cosine similarity so the downstream LLM detector
 * only sees a small candidate set (issue #359).
 *
 * Uses real fetchEmbedding() from ./embedding (raw text → vector API).
 */
import { fetchEmbedding } from './embedding';
import type { EmbeddingConfig } from '@/stores/wiki-store';

export interface Page {
  id: string;
  title: string;
  body?: string;
  tags?: string[];
}

export interface CandidateOptions {
  topK?: number;          // default 8
  threshold?: number;     // default 0.82 (cosine, 0..1)
  maxPages?: number;      // hard cap to avoid OOM, default 5000
  /**
   * Per-page character budget for the embedding input text.
   * Real pages can be megabytes; we cap to stay within embedding context windows.
   * Default 1500 chars (matches chunker default).
   */
  textBudgetChars?: number;
}

export type CandidatePair = readonly [string, string];

/**
 * Cosine similarity between two equal-length vectors. Returns 0 if either is
 * zero, vectors differ in length, or either is null/undefined (embedding failed).
 */
export function cosineSimilarity(a: number[] | null | undefined, b: number[] | null | undefined): number {
  if (!a || !b || a.length !== b.length) return 0;
  let dot = 0, na = 0, nb = 0;
  for (let i = 0; i < a.length; i++) {
    dot += a[i] * b[i];
    na  += a[i] * a[i];
    nb  += b[i] * b[i];
  }
  const denom = Math.sqrt(na) * Math.sqrt(nb);
  return denom === 0 ? 0 : dot / denom;
}

/**
 * Build the embedding input text from a page.
 * Mirrors embedPage's chunker input but keeps it short for similarity comparison.
 */
export function pageToEmbeddingText(page: Page, budget = 1500): string {
  const tagPart = (page.tags ?? []).join(' ');
  const parts = [page.title, tagPart, (page.body ?? '').slice(0, budget)];
  return parts.filter(Boolean).join('\n');
}

/**
 * Embed pages sequentially via fetchEmbedding.
 * Returns pageId → vector (or null if embedding failed for that page).
 */
export async function embedPages(
  pages: Page[],
  cfg: EmbeddingConfig,
  opts: { textBudgetChars?: number } = {},
): Promise<Map<string, number[] | null>> {
  const out = new Map<string, number[] | null>();
  const budget = opts.textBudgetChars ?? 1500;
  for (const p of pages) {
    const text = pageToEmbeddingText(p, budget);
    const vec = await fetchEmbedding(text, cfg);
    out.set(p.id, vec);
  }
  return out;
}

/**
 * Generate candidate duplicate pairs: each page's top-K nearest neighbors
 * above threshold, self-excluded, symmetric deduplicated.
 *
 * Pages whose embedding failed (null) are silently skipped on the source
 * side; they may still appear as the TARGET of a pair from another page.
 */
export async function candidatePairs(
  pages: Page[],
  cfg: EmbeddingConfig,
  opts: CandidateOptions = {},
): Promise<CandidatePair[]> {
  const topK     = opts.topK ?? 8;
  const threshold = opts.threshold ?? 0.82;
  const maxPages  = opts.maxPages ?? 5000;

  if (pages.length === 0) return [];
  const subset = pages.slice(0, maxPages);

  const embeddings = await embedPages(subset, cfg, {
    textBudgetChars: opts.textBudgetChars,
  });

  const pairSet = new Set<string>();
  const pairs: CandidatePair[] = [];

  for (let i = 0; i < subset.length; i++) {
    const vi = embeddings.get(subset[i].id);
    if (!vi) continue;
    const scored: Array<{ j: number; sim: number }> = [];
    for (let j = 0; j < subset.length; j++) {
      if (i === j) continue;
      const vj = embeddings.get(subset[j].id);
      const sim = cosineSimilarity(vi, vj);
      if (sim >= threshold) scored.push({ j, sim });
    }
    scored.sort((a, b) => b.sim - a.sim);

    for (let k = 0; k < Math.min(topK, scored.length); k++) {
      const a = subset[i].id;
      const b = subset[scored[k].j].id;
      const key = a < b ? `${a}\t${b}` : `${b}\t${a}`;
      if (!pairSet.has(key)) {
        pairSet.add(key);
        pairs.push([a, b] as const);
      }
    }
  }

  return pairs;
}

/**
 * Union-find clustering of candidate pairs into groups.
 * ITERATIVE find() with path compression to avoid stack overflow on large inputs.
 */
export function clusterByPairs(
  pageIds: string[],
  pairs: CandidatePair[],
): string[][] {
  const parent = new Map<string, string>();
  for (const id of pageIds) parent.set(id, id);

  const find = (x: string): string => {
    let root = x;
    while (parent.get(root) !== root) root = parent.get(root)!;
    // path compression
    let cur = x;
    while (parent.get(cur) !== root) {
      const next = parent.get(cur)!;
      parent.set(cur, root);
      cur = next;
    }
    return root;
  };

  for (const [a, b] of pairs) {
    const ra = find(a), rb = find(b);
    if (ra !== rb) parent.set(ra, rb);
  }

  const groups = new Map<string, string[]>();
  for (const id of pageIds) {
    const root = find(id);
    if (!groups.has(root)) groups.set(root, []);
    groups.get(root)!.push(id);
  }

  return [...groups.values()].filter(g => g.length > 1);
}