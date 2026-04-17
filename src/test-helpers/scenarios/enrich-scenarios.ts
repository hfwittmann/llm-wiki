import type { EnrichScenario } from "./types"

/**
 * Reminder: enrichWithWikilinks only writes back to disk if the LLM
 * response length is at least 50% of the original content length
 * (guards against LLM errors returning something short and bogus).
 */

const WIKI_INDEX_WITH_TRANSFORMER = `# Index

## Concepts
- [[attention]]
- [[transformer]]
- [[encoder]]
`

export const enrichScenarios: EnrichScenario[] = [
  // 1. adds-wikilinks — LLM wraps recognized terms with [[...]]
  {
    name: "adds-wikilinks",
    description:
      "A page mentions 'Transformer' without wikilinks. LLM returns the " +
      "same content with [[Transformer]] added on first mention. " +
      "writeFile must be called with the enriched content.",
    initialWiki: {
      "wiki/index.md": WIKI_INDEX_WITH_TRANSFORMER,
      "wiki/survey.md":
        "# Deep Learning Survey\n\n" +
        "Modern NLP relies on Transformer architectures for most tasks. " +
        "The Transformer was introduced in 2017 and has since dominated. " +
        "Attention is the key mechanism that makes it work.\n",
    },
    pageToEnrich: "wiki/survey.md",
    // Enriched version — same length-ish (LLM guard: >= 50% of original length).
    llmResponse:
      "# Deep Learning Survey\n\n" +
      "Modern NLP relies on [[Transformer]] architectures for most tasks. " +
      "The Transformer was introduced in 2017 and has since dominated. " +
      "[[Attention]] is the key mechanism that makes it work.\n",
    expected: {
      writeCalled: true,
      expectedContent:
        "# Deep Learning Survey\n\n" +
        "Modern NLP relies on [[Transformer]] architectures for most tasks. " +
        "The Transformer was introduced in 2017 and has since dominated. " +
        "[[Attention]] is the key mechanism that makes it work.\n",
    },
  },

  // 2. preserves-frontmatter — YAML block must survive enrichment verbatim
  {
    name: "preserves-frontmatter",
    description:
      "Page has YAML frontmatter with title, tags, sources. LLM response " +
      "must preserve the frontmatter exactly. writeFile is called and the " +
      "content starts with the original frontmatter.",
    initialWiki: {
      "wiki/index.md": WIKI_INDEX_WITH_TRANSFORMER,
      "wiki/attention.md":
        "---\n" +
        "title: Attention\n" +
        "tags: [deep-learning, transformer]\n" +
        "sources: [paper-2017.pdf]\n" +
        "---\n\n" +
        "# Attention\n\n" +
        "Attention scores are computed by the encoder. The encoder layer " +
        "uses these scores to weight values. Detailed derivation below.\n",
    },
    pageToEnrich: "wiki/attention.md",
    llmResponse:
      "---\n" +
      "title: Attention\n" +
      "tags: [deep-learning, transformer]\n" +
      "sources: [paper-2017.pdf]\n" +
      "---\n\n" +
      "# Attention\n\n" +
      "Attention scores are computed by the [[encoder]]. The [[encoder]] layer " +
      "uses these scores to weight values. Detailed derivation below.\n",
    expected: {
      writeCalled: true,
      expectedContent:
        "---\n" +
        "title: Attention\n" +
        "tags: [deep-learning, transformer]\n" +
        "sources: [paper-2017.pdf]\n" +
        "---\n\n" +
        "# Attention\n\n" +
        "Attention scores are computed by the [[encoder]]. The [[encoder]] layer " +
        "uses these scores to weight values. Detailed derivation below.\n",
    },
  },

  // 3. short-response-rejected — guards against truncated LLM replies
  {
    name: "short-response-rejected",
    description:
      "Original content is long (~400 chars) but LLM returns only a brief " +
      "snippet (<50% original length). The guard rejects the write to avoid " +
      "data loss. writeFile MUST NOT be called.",
    initialWiki: {
      "wiki/index.md": WIKI_INDEX_WITH_TRANSFORMER,
      "wiki/long-page.md":
        "# Long Page\n\n" +
        "This page has substantial content that must not be clobbered by a " +
        "truncated LLM response. ".repeat(10) +
        "\n\nFinal line here for good measure.\n",
    },
    pageToEnrich: "wiki/long-page.md",
    // Way shorter than original — must trigger the guard
    llmResponse: "Short reply with [[attention]].",
    expected: {
      writeCalled: false,
    },
  },

  // 4. cjk-terms — Chinese content + Chinese wikilinks
  {
    name: "cjk-terms",
    description:
      "Chinese content with Chinese wiki index entries. LLM adds CJK " +
      "wikilinks on first mention. UTF-8 round-trip must be clean.",
    initialWiki: {
      "wiki/index.md": "# 索引\n\n- [[注意力机制]]\n- [[transformer]]\n",
      "wiki/intro.md":
        "# 简介\n\n" +
        "注意力机制是 transformer 架构的核心组件之一。" +
        "注意力机制让模型能够关注序列中最相关的部分。\n",
    },
    pageToEnrich: "wiki/intro.md",
    llmResponse:
      "# 简介\n\n" +
      "[[注意力机制]]是 [[transformer]] 架构的核心组件之一。" +
      "注意力机制让模型能够关注序列中最相关的部分。\n",
    expected: {
      writeCalled: true,
      expectedContent:
        "# 简介\n\n" +
        "[[注意力机制]]是 [[transformer]] 架构的核心组件之一。" +
        "注意力机制让模型能够关注序列中最相关的部分。\n",
    },
  },
]
