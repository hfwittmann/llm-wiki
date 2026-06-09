/**
 * dedup_embedding.ts
 *
 * Vector-embedding candidate generation for duplicate-page scan.
 * Pre-filters pages by cosine similarity so the downstream LLM detector
 * only sees a small candidate set (issue #359).
 *
 * Uses real embedPage() from ./embedding (no batch API exposed).
 */
import { embedPage } from './embedding';

export interface Page {
  id: string;
  title: string;
  body: string;
  tags?: string[];
}

export interface PageEmbedding {
  pageId: string;
  vector: number[];
}

export interface CandidateOptions {
  topK?: number;          // default 8
  threshold?: number;     // default 0.82 (cosine, 0..1)
  maxPages?: number;      // hard cap to avoid OOM, default 5000
}

export type CandidatePair = readonly [string, string];

/**
 * Cosine similarity between two equal-length vectors. Returns 0 if either is zero.
 */
export function cosineSimilarity(a: number[], b: number[]): number {
  if (a.length !== b.length) return 0;
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
 * Embed pages one-by-one using embedPage(). Sequential because batch API
 * is not exposed in current embedding.ts.
 */
export async function embedPages(pages: Page[]): Promise<PageEmbedding[]> {
  const out: PageEmbedding[] = [];
  for (const p of pages) {
    const vector = await embedPage(p);
    out.push({ pageId: p.id, vector });
  }
  return out;
}

/**
 * Generate candidate duplicate pairs: each page's top-K nearest neighbors
 * above threshold, self-excluded, symmetric deduplicated.
 */
export async function candidatePairs(
  pages: Page[],
  opts: CandidateOptions = {},
): Promise<CandidatePair[]> {
  const topK     = opts.topK ?? 8;
  const threshold = opts.threshold ?? 0.82;
  const maxPages  = opts.maxPages ?? 5000;

  if (pages.length === 0) return [];
  const subset = pages.slice(0, maxPages);

  const embeddings = await embedPages(subset);
  const embMap = new Map(embeddings.map(e => [e.pageId, e.vector]));

  const pairSet = new Set<string>();
  const pairs: CandidatePair[] = [];

  for (let i = 0; i < subset.length; i++) {
    const vi = embMap.get(subset[i].id)!;
    const scored: Array<{ j: number; sim: number }> = [];
    for (let j = 0; j < subset.length; j++) {
      if (i === j) continue;
      const vj = embMap.get(subset[j].id)!;
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
 * ITERATIVE find() to avoid stack overflow on large inputs (R1 Reviewer Major).
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
