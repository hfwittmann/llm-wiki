/**
 * dedup_embedding.test.ts — unit tests for #359 prefilter (R3)
 */
import { describe, it, expect, vi } from 'vitest';
import {
  candidatePairs,
  clusterByPairs,
  cosineSimilarity,
  type Page,
} from '../dedup_embedding';

// Mock embedPage to produce realistic, sparse vectors.
// Strategy: numeric id → topic axis (mod dim). Pages with same numeric
// suffix share a topic (e.g. p1 and q1 both on axis 1), but consecutive
// ids (p0, p1, p2, ...) land on consecutive axes → low mutual similarity.
vi.mock('../embedding', () => ({
  embedPage: vi.fn(async (page: Page) => {
    const dim = 64;
    const v = new Array(dim).fill(0);
    // Extract numeric portion of id (handles 'p123', 'q42', 'aa100' etc.)
    const numMatch = page.id.match(/\d+/);
    const num = numMatch ? parseInt(numMatch[0], 10) : 0;
    const topicAxis = num % dim;
    v[topicAxis] = 1.0;
    v[(topicAxis + 1) % dim] = 0.05;
    v[(topicAxis - 1 + dim) % dim] = 0.03;
    return v;
  }),
}));

const page = (id: string, title: string, body = ''): Page => ({
  id, title, body, tags: [],
});

describe('cosineSimilarity', () => {
  it('returns 1 for identical vectors', () => {
    expect(cosineSimilarity([1, 0, 0], [1, 0, 0])).toBeCloseTo(1.0, 5);
  });
  it('returns 0 for orthogonal vectors', () => {
    expect(cosineSimilarity([1, 0, 0], [0, 1, 0])).toBeCloseTo(0.0, 5);
  });
  it('returns 0 for zero vectors', () => {
    expect(cosineSimilarity([0, 0], [1, 1])).toBe(0);
  });
  it('handles mismatched lengths', () => {
    expect(cosineSimilarity([1, 0], [1, 0, 0])).toBe(0);
  });
});

describe('candidatePairs', () => {
  it('returns empty array for empty input', async () => {
    expect(await candidatePairs([])).toEqual([]);
  });

  it('returns empty for single page (self-exclusion)', async () => {
    expect(await candidatePairs([page('a', 'Foo')])).toEqual([]);
  });

  it('generates symmetric, deduplicated pairs', async () => {
    // p11 and q11 share axis 11 → high similarity
    const pages = [
      page('p11', 'Foo bar baz'),
      page('q11', 'completely different topic'),
      page('p12', 'yet another topic'),
      page('q12', 'totally unrelated'),
    ];
    const pairs = await candidatePairs(pages, { threshold: 0.8 });
    expect(pairs.every(([x, y]) => x !== y)).toBe(true);
    const keys = pairs.map(([x, y]) => x < y ? `${x}|${y}` : `${y}|${x}`);
    expect(new Set(keys).size).toBe(keys.length);
  });

  it('respects threshold filter (higher threshold → fewer pairs)', async () => {
    const pages = Array.from({ length: 30 }, (_, i) => page(`p${i}`, `t${i}`));
    const high = await candidatePairs(pages, { threshold: 0.99 });
    const low  = await candidatePairs(pages, { threshold: 0.5 });
    expect(low.length).toBeGreaterThanOrEqual(high.length);
  });

  it('respects topK (caps total pairs at topK * n)', async () => {
    // With numeric-id mock: p0..p19 → distinct axes 0..19.
    // Threshold=0 means even unrelated neighbors count (low but non-zero).
    // topK=3 means each page iteration contributes AT MOST 3 NEW pairs to pairSet
    // (dedup prevents double-counting, but a pair (a,b) can be added by EITHER
    // a's iteration OR b's iteration, whichever has b/a in topK).
    // Worst case: each iteration adds 3 NEW pairs → max = topK * n = 60.
    const pages = Array.from({ length: 20 }, (_, i) => page(`p${i}`, `t${i}`));
    const pairs = await candidatePairs(pages, { threshold: 0.0, topK: 3 });
    // Strict upper bound: topK * n = 60
    expect(pairs.length).toBeLessThanOrEqual(60);
  });

  it('scales to 1000 pages with realistic output (acceptance for #359)', async () => {
    // 1000 pages with ids p0..p999 → 64 distinct axes (15-16 pages per axis).
    // Realistic scenario: topK=3 + threshold=0.95 (semantic duplicates only).
    // Expected: ~ (3 pairs per page × 1000 pages) / 2 dedup = ~1500 pairs.
    // Acceptance: <3000 (vs R1 behavior which is N² = 1M LLM calls).
    // 300× reduction in candidates is the core value of issue #359.
    const pages = Array.from({ length: 1000 }, (_, i) => page(`p${i}`, `s${i}`));
    const t0 = Date.now();
    const pairs = await candidatePairs(pages, { threshold: 0.95, topK: 3 });
    const elapsed = Date.now() - t0;
    expect(elapsed).toBeLessThan(5000);
    expect(pairs.length).toBeLessThan(3000);
    expect(pairs.length).toBeGreaterThan(0); // sanity: did we actually dedup?
  });
});

describe('clusterByPairs', () => {
  it('returns empty for no pairs', () => {
    expect(clusterByPairs(['a','b'], [])).toEqual([]);
  });

  it('groups transitive duplicates', () => {
    const groups = clusterByPairs(['a','b','c'], [['a','b'], ['b','c']]);
    expect(groups).toHaveLength(1);
    expect(groups[0].sort()).toEqual(['a','b','c']);
  });

  it('keeps isolated pages separate', () => {
    const groups = clusterByPairs(['a','b','c'], [['a','b']]);
    expect(groups).toHaveLength(1);
    expect(groups[0].sort()).toEqual(['a','b']);
  });

  it('handles 10k page IDs without stack overflow (R1 review Major)', () => {
    const ids = Array.from({ length: 10000 }, (_, i) => `id${i}`);
    const pairs: Array<readonly [string, string]> = [];
    for (let i = 0; i < 5000; i++) {
      pairs.push([`id${i}`, `id${i + 1}`] as const);
    }
    expect(() => clusterByPairs(ids, pairs)).not.toThrow();
    const groups = clusterByPairs(ids, pairs);
    expect(groups.length).toBe(1);
  });
});
