//! Pure wiki-page-level logic — no Tauri, no AppHandle.
//!
//! Covers: markdown frontmatter scanning, wikilink parsing, and related-page
//! lookup. Called by thin `#[tauri::command]` wrappers in `commands::fs`.

use std::fs;
use std::path::Path;

use crate::panic_guard::run_guarded;

// ──────────────────────────────────────────────────────────────────────────
// Error type
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum WikiError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("blocking task panicked: {0}")]
    Join(String),
    #[error("internal: {0}")]
    Internal(String),
}

// ──────────────────────────────────────────────────────────────────────────
// Internal helpers
// ──────────────────────────────────────────────────────────────────────────

pub(crate) fn collect_related_pages(
    dir: &Path,
    source_name: &str,
    results: &mut Vec<String>,
) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| e.to_string())?;

    // Get just the filename without path — use Path for cross-platform separator handling
    let source_path = std::path::Path::new(source_name);
    let file_name = source_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(source_name);
    let file_name_lower = file_name.to_lowercase();

    // Derive stem (filename without extension) for source summary matching
    let file_stem = file_name
        .rsplit('.')
        .skip(1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(".");
    let file_stem_lower = if file_stem.is_empty() {
        file_name_lower.clone()
    } else {
        file_stem.to_lowercase()
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_related_pages(&path, source_name, results)?;
        } else if path.extension().map(|e| e == "md").unwrap_or(false) {
            let fname = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // Skip index.md, log.md, overview.md — updated separately
            if fname == "index.md" || fname == "log.md" || fname == "overview.md" {
                continue;
            }

            if let Ok(content) = fs::read_to_string(&path) {
                let content_lower = content.to_lowercase();

                // Match 1: frontmatter sources field contains the exact filename
                // e.g., sources: ["2603.25723v1.pdf"]
                let sources_match = content_lower.contains(&format!("\"{}\"", file_name_lower))
                    || content_lower.contains(&format!("'{}'", file_name_lower));

                // Match 2: source summary page (wiki/sources/{stem}.md)
                // Use Path component iteration to avoid hardcoded separator assumptions
                let is_in_sources_dir = path.components().any(|c| c.as_os_str() == "sources");
                let is_source_summary =
                    is_in_sources_dir && fname.to_lowercase().starts_with(&file_stem_lower);

                // Match 3: the page's *sources block* mentions the
                // filename. Covers the multi-line YAML list form
                //
                //   sources:
                //     - test.md         (unquoted, missed by Match 1)
                //     - "other.md"
                //
                // Previous version substring-matched against the ENTIRE
                // frontmatter, which false-positived whenever the
                // filename happened to appear in title / description /
                // any other field — those pages were then handed to
                // the TS delete flow and, because their actual sources
                // list didn't include the deleted file, silently
                // wiped. Tightened: scope the substring check to the
                // `sources:` block only (inline line + any indented
                // continuation lines of a YAML list).
                let frontmatter_match = if content.starts_with("---\n") {
                    if let Some(fm_end_rel) = content[4..].find("\n---") {
                        let frontmatter = &content[4..4 + fm_end_rel].to_lowercase();
                        let mut found = false;
                        let mut in_sources_block = false;
                        for line in frontmatter.split('\n') {
                            if line.starts_with("sources:") {
                                // Inline-form `sources: [...]` lives
                                // entirely on this one line; check it.
                                if line.contains(&file_name_lower) {
                                    found = true;
                                    break;
                                }
                                in_sources_block = true;
                                continue;
                            }
                            if in_sources_block {
                                // Continuation lines of a YAML list are
                                // indented; an un-indented line means
                                // we've left the sources block for
                                // another top-level field.
                                if line.is_empty()
                                    || line.starts_with(' ')
                                    || line.starts_with('\t')
                                {
                                    if line.contains(&file_name_lower) {
                                        found = true;
                                        break;
                                    }
                                } else {
                                    in_sources_block = false;
                                }
                            }
                        }
                        found
                    } else {
                        false
                    }
                } else {
                    false
                };

                if sources_match || is_source_summary || frontmatter_match {
                    // Normalize to forward slashes — matches build_tree /
                    // copy_directory so TS-side comparisons work on Windows.
                    results.push(path.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────
// Public core functions
// ──────────────────────────────────────────────────────────────────────────

/// Find wiki pages that reference a given source file name.
/// Scans all .md files under wiki/ for the source filename in frontmatter or content.
pub async fn find_related_wiki_pages(
    project_path: String,
    source_name: String,
) -> Result<Vec<String>, String> {
    tokio::task::spawn_blocking(move || {
        run_guarded("find_related_wiki_pages", || {
            let wiki_dir = Path::new(&project_path).join("wiki");
            if !wiki_dir.is_dir() {
                return Ok(vec![]);
            }

            let mut related = Vec::new();
            collect_related_pages(&wiki_dir, &source_name, &mut related)?;
            Ok(related)
        })
    })
    .await
    .map_err(|e| format!("find_related_wiki_pages blocking task join error: {e}"))?
}

// ──────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ── collect_related_pages: regression coverage for the three match ─────
    // strategies used by findRelatedWikiPages.
    //
    // Strategy 1: quoted filename anywhere in content
    //               (e.g. `sources: ["test.md"]` inline form)
    // Strategy 2: page lives under wiki/sources/ and starts with file stem
    //               (the source summary page)
    // Strategy 3: filename appears inside the frontmatter's sources BLOCK
    //               (tightened: no longer false-positives on `title:`
    //                `description:` or any other field that happens to
    //                include the filename as a substring)
    //
    // These tests are the regression guard for the Strategy 3 fix — before
    // the tightening, a page whose title included the deleted filename
    // would be surfaced here and then wrongly deleted downstream.

    fn make_wiki(files: &[(&str, &str)]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wiki-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        for (rel, body) in files {
            let p = dir.join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&p, body).unwrap();
        }
        dir
    }

    fn collect(wiki: &std::path::Path, source: &str) -> Vec<String> {
        let mut out = Vec::new();
        collect_related_pages(wiki, source, &mut out).unwrap();
        // Normalize to the wiki-relative suffix so assertions are
        // independent of the temp prefix.
        let prefix = wiki.to_string_lossy().replace('\\', "/");
        out.into_iter()
            .map(|p| {
                let p = p.replace('\\', "/");
                p.strip_prefix(&format!("{}/", prefix))
                    .map(str::to_string)
                    .unwrap_or(p)
            })
            .collect()
    }

    #[test]
    fn collect_related_strategy1_inline_quoted_sources() {
        let wiki = make_wiki(&[
            (
                "concepts/rope.md",
                "---\ntitle: RoPE\nsources: [\"test.md\"]\n---\nbody\n",
            ),
            (
                "concepts/unrelated.md",
                "---\ntitle: Unrelated\nsources: [\"other.md\"]\n---\nbody\n",
            ),
        ]);
        let mut got = collect(&wiki, "test.md");
        got.sort();
        assert_eq!(got, vec!["concepts/rope.md"]);
        let _ = fs::remove_dir_all(&wiki);
    }

    #[test]
    fn collect_related_strategy1_single_quoted_sources() {
        let wiki = make_wiki(&[(
            "concepts/rope.md",
            "---\ntitle: RoPE\nsources: ['test.md']\n---\nbody\n",
        )]);
        let got = collect(&wiki, "test.md");
        assert_eq!(got, vec!["concepts/rope.md"]);
        let _ = fs::remove_dir_all(&wiki);
    }

    #[test]
    fn collect_related_strategy2_source_summary_page() {
        // A page inside wiki/sources/ whose filename starts with the
        // deleted source's stem counts as the source-summary page —
        // kept linked even if its sources field happens to be missing.
        let wiki = make_wiki(&[
            ("sources/test.md", "---\ntitle: Test Summary\n---\nbody\n"),
            (
                "concepts/unrelated.md",
                "---\ntitle: Unrelated\nsources: [\"other.md\"]\n---\nbody\n",
            ),
        ]);
        let got = collect(&wiki, "test.md");
        assert_eq!(got, vec!["sources/test.md"]);
        let _ = fs::remove_dir_all(&wiki);
    }

    #[test]
    fn collect_related_strategy3_multi_line_yaml_list() {
        // Multi-line YAML sources block with an unquoted entry — Strategy
        // 1 can't see this (no quotes), Strategy 3 has to walk the
        // sources block line by line.
        let wiki = make_wiki(&[(
            "concepts/rope.md",
            "---\ntitle: RoPE\nsources:\n  - test.md\n  - \"other.md\"\ntags: []\n---\nbody\n",
        )]);
        let got = collect(&wiki, "test.md");
        assert_eq!(got, vec!["concepts/rope.md"]);
        let _ = fs::remove_dir_all(&wiki);
    }

    #[test]
    fn collect_related_strategy3_does_not_false_positive_on_title_substring() {
        // Regression guard for the bug we just fixed: a page whose
        // title / description contains the deleted filename MUST NOT
        // be surfaced when its actual sources list is unrelated.
        // Before the fix, the whole frontmatter was substring-scanned
        // and this page would have been returned → downstream delete
        // flow → silent data loss on an innocent page.
        let wiki = make_wiki(&[
            (
                "concepts/rope.md",
                "---\ntitle: Analysis of test.md\ndescription: Discusses test.md in depth\nsources: [\"other.md\"]\n---\nbody\n",
            ),
            (
                "concepts/real-match.md",
                "---\ntitle: Real\nsources: [\"test.md\"]\n---\nbody\n",
            ),
        ]);
        let got = collect(&wiki, "test.md");
        // Only the real-match page is surfaced. The title-substring
        // page is correctly ignored now.
        assert_eq!(got, vec!["concepts/real-match.md"]);
        let _ = fs::remove_dir_all(&wiki);
    }

    #[test]
    fn collect_related_strategy3_stops_at_next_top_level_field() {
        // Scan must stop at the next top-level YAML key so that a
        // filename appearing in a later field (e.g. `notes:`) doesn't
        // get pulled into the sources block.
        let wiki = make_wiki(&[(
            "concepts/rope.md",
            "---\ntitle: RoPE\nsources:\n  - other.md\nnotes: See test.md for context\n---\nbody\n",
        )]);
        let got = collect(&wiki, "test.md");
        // sources block has only other.md; test.md appears in `notes:`
        // which is outside the block — must not match.
        assert!(got.is_empty(), "expected empty, got {got:?}");
        let _ = fs::remove_dir_all(&wiki);
    }

    #[test]
    fn collect_related_returns_empty_when_nothing_matches() {
        let wiki = make_wiki(&[(
            "concepts/unrelated.md",
            "---\ntitle: X\nsources: [\"other.md\"]\n---\nbody\n",
        )]);
        let got = collect(&wiki, "nonexistent.md");
        assert!(got.is_empty());
        let _ = fs::remove_dir_all(&wiki);
    }

    #[test]
    fn collect_related_skips_index_log_overview() {
        // Listing pages (index.md, log.md, overview.md) reference the
        // filename heavily but should never be returned here — they're
        // cleaned separately via the TS cleanup helpers.
        let wiki = make_wiki(&[
            (
                "index.md",
                "---\ntitle: Index\n---\n- [[Test]]\nsources: [\"test.md\"]\n",
            ),
            (
                "log.md",
                "---\ntitle: Log\n---\nIngested test.md on 2026-01-01\n",
            ),
            (
                "overview.md",
                "---\ntitle: Overview\n---\nCovers test.md and other.md\n",
            ),
            (
                "concepts/real.md",
                "---\ntitle: Real\nsources: [\"test.md\"]\n---\nbody\n",
            ),
        ]);
        let got = collect(&wiki, "test.md");
        assert_eq!(got, vec!["concepts/real.md"]);
        let _ = fs::remove_dir_all(&wiki);
    }

    #[test]
    fn collect_related_case_insensitive_filename_match() {
        let wiki = make_wiki(&[(
            "concepts/rope.md",
            "---\ntitle: RoPE\nsources: [\"Test.md\"]\n---\nbody\n",
        )]);
        let got = collect(&wiki, "test.md");
        assert_eq!(got, vec!["concepts/rope.md"]);
        let _ = fs::remove_dir_all(&wiki);
    }
}
