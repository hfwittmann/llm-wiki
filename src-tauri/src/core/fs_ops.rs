//! Directory listing and file preprocessing — no Tauri, no AppHandle.
//!
//! Covers: `list_directory` (recursive tree builder), `create_directory`,
//! and `preprocess_file` (PDF/Office text extraction + cache write).
//!
//! Called by thin `#[tauri::command]` wrappers in `commands::fs`.

use std::fs;
use std::path::Path;

use crate::core::files::{extract_office_text, extract_pdf_text, write_cache};
use crate::panic_guard::run_guarded;
use crate::types::wiki::FileNode;

// ──────────────────────────────────────────────────────────────────────────
// Error type
// ──────────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum FsOpsError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("blocking task panicked: {0}")]
    Join(String),
    #[error("internal: {0}")]
    Internal(String),
}

// ──────────────────────────────────────────────────────────────────────────
// Internal helpers
// ──────────────────────────────────────────────────────────────────────────

const OFFICE_EXTS: &[&str] = &["doc", "docx", "pptx", "xls", "xlsx", "odt", "ods", "odp"];

pub(crate) fn build_tree(dir: &Path, depth: usize, max_depth: usize) -> Result<Vec<FileNode>, String> {
    if depth >= max_depth {
        return Ok(vec![]);
    }

    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("Failed to read directory '{}': {}", dir.display(), e))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            // Skip dotfiles
            entry
                .file_name()
                .to_str()
                .map(|n| !n.starts_with('.'))
                .unwrap_or(false)
        })
        .collect();

    // Sort: directories first, then alphabetical within each group
    entries.sort_by(|a, b| {
        let a_is_dir = a.path().is_dir();
        let b_is_dir = b.path().is_dir();
        match (a_is_dir, b_is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.file_name().cmp(&b.file_name()),
        }
    });

    let mut nodes = Vec::new();
    for entry in entries {
        let entry_path = entry.path();
        let name = entry.file_name().to_str().unwrap_or("").to_string();
        // Always return forward-slash paths so the TS layer can compare
        // and compose paths consistently across Windows and Unix. Windows
        // APIs accept forward slashes, so normalizing here is safe and
        // prevents a whole class of bugs where TS-constructed `/` paths
        // fail to match Rust-returned `\` paths.
        let path_str = entry_path.to_string_lossy().replace('\\', "/");
        let is_dir = entry_path.is_dir();

        let children = if is_dir {
            let kids = build_tree(&entry_path, depth + 1, max_depth)?;
            if kids.is_empty() {
                None
            } else {
                Some(kids)
            }
        } else {
            None
        };

        nodes.push(FileNode {
            name,
            path: path_str,
            is_dir,
            children,
        });
    }

    Ok(nodes)
}

// ──────────────────────────────────────────────────────────────────────────
// Public core functions
// ──────────────────────────────────────────────────────────────────────────

/// List the contents of a directory recursively (up to 30 levels deep).
pub async fn list_directory(path: String) -> Result<Vec<FileNode>, String> {
    tokio::task::spawn_blocking(move || {
        run_guarded("list_directory", || {
            let p = Path::new(&path);
            if !p.exists() {
                return Err(format!("Path does not exist: '{}'", path));
            }
            if !p.is_dir() {
                return Err(format!("Path is not a directory: '{}'", path));
            }
            let nodes = build_tree(p, 0, 30)?;
            Ok(nodes)
        })
    })
    .await
    .map_err(|e| format!("list_directory blocking task join error: {e}"))?
}

/// Create a directory (and all required parent directories).
pub async fn create_directory(path: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        run_guarded("create_directory", || {
            crate::core::files::require_absolute_path("create_directory", &path)?;
            fs::create_dir_all(&path)
                .map_err(|e| format!("Failed to create directory '{}': {}", path, e))
        })
    })
    .await
    .map_err(|e| format!("create_directory blocking task join error: {e}"))?
}

/// Pre-process a file and cache the extracted text.
pub async fn preprocess_file(path: String) -> Result<String, String> {
    // See `core::files::read_file` for why `spawn_blocking` is required.
    tokio::task::spawn_blocking(move || {
        run_guarded("preprocess_file", || {
            let p = Path::new(&path);
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();

            let text = match ext.as_str() {
                "pdf" => extract_pdf_text(&path, false)?,
                e if OFFICE_EXTS.contains(&e) => extract_office_text(&path, e)?,
                _ => return Ok("no preprocessing needed".to_string()),
            };

            write_cache(p, &text)?;
            Ok(text)
        })
    })
    .await
    .map_err(|e| format!("preprocess_file blocking task join error: {e}"))?
}

// ──────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── copy_directory: folder import recursion + filtering ──────────
    //
    // The folder-import flow on the JS side calls this command and
    // expects:
    //   1. Recursion goes ALL the way down (no depth cap) — users
    //      drop trees with arbitrary nesting and every file inside
    //      should reach the wiki.
    //   2. Dotfiles / dot-directories are skipped (`.git`, `.cache`,
    //      `.DS_Store`) — otherwise a folder with a `.git/` would
    //      import megabytes of git plumbing as "source files."
    //   3. Returned paths are FLAT (one entry per file, regardless
    //      of depth) and use forward slashes (the JS layer normalizes
    //      everything to `/` before doing path comparisons).
    //
    // These are exactly the invariants `handleImportFolder` in
    // sources-view.tsx assumes — pinning them here keeps a future
    // refactor of the recursive copier from silently breaking the
    // folder import button.

    fn make_temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "llmwiki-copydir-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Pull the inner sync `copy_recursive` body out from
    /// `copy_directory` so the test doesn't need to spin up a
    /// tokio runtime just to exercise file-system recursion.
    /// Mirrors the same logic the async command uses.
    fn copy_dir_for_test(src: &Path, dest: &Path) -> Vec<String> {
        std::fs::create_dir_all(dest).unwrap();
        let mut out = Vec::new();
        fn rec(src: &Path, dest: &Path, files: &mut Vec<String>) {
            std::fs::create_dir_all(dest).unwrap();
            for entry in std::fs::read_dir(src).unwrap().flatten() {
                let path = entry.path();
                let name = entry.file_name();
                let dest_path = dest.join(&name);
                if name.to_string_lossy().starts_with('.') {
                    continue;
                }
                if path.is_dir() {
                    rec(&path, &dest_path, files);
                } else {
                    std::fs::copy(&path, &dest_path).unwrap();
                    files.push(dest_path.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        rec(src, dest, &mut out);
        out
    }

    #[test]
    fn copy_directory_recurses_arbitrary_depth() {
        let src = make_temp_dir("src-deep");
        // Build /src/a/b/c/d/e/leaf.txt — five levels under root.
        let leaf_dir = src.join("a/b/c/d/e");
        std::fs::create_dir_all(&leaf_dir).unwrap();
        std::fs::write(leaf_dir.join("leaf.txt"), b"deep content").unwrap();
        // Plus a top-level file to ensure root files come along too.
        std::fs::write(src.join("top.md"), b"# top").unwrap();

        let dest = make_temp_dir("dest-deep");
        let copied = copy_dir_for_test(&src, &dest);

        assert_eq!(copied.len(), 2, "expected two files, got: {:?}", copied);
        // Deep file made it across with full nesting preserved.
        let leaf_dest = dest.join("a/b/c/d/e/leaf.txt");
        assert!(
            leaf_dest.exists(),
            "deep leaf.txt missing at {:?}",
            leaf_dest
        );
        assert_eq!(std::fs::read(&leaf_dest).unwrap(), b"deep content");
        // Top-level file too.
        assert!(dest.join("top.md").exists());
        // Returned paths are forward-slashed and absolute.
        for p in &copied {
            assert!(!p.contains('\\'), "path should be /-normalized: {p}");
            assert!(Path::new(p).is_absolute(), "path should be absolute: {p}");
        }

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn copy_directory_skips_dotfiles_and_dot_directories() {
        let src = make_temp_dir("src-dots");
        // Visible content:
        std::fs::write(src.join("keep.md"), b"keep me").unwrap();
        std::fs::create_dir_all(src.join("subdir")).unwrap();
        std::fs::write(src.join("subdir/keep2.md"), b"keep me too").unwrap();
        // Things that must be skipped:
        std::fs::write(src.join(".DS_Store"), b"junk").unwrap();
        std::fs::create_dir_all(src.join(".git/objects")).unwrap();
        std::fs::write(src.join(".git/HEAD"), b"ref: refs/heads/main").unwrap();
        std::fs::write(src.join(".git/objects/abc"), b"\x78\x9c").unwrap();
        std::fs::write(src.join(".env"), b"SECRET=foo").unwrap();
        // Sneaky one: a dot-prefixed dir nested inside a normal dir
        // should ALSO be skipped (the dotfile rule applies at every
        // recursion level, not just the top).
        std::fs::create_dir_all(src.join("subdir/.cache")).unwrap();
        std::fs::write(src.join("subdir/.cache/blob"), b"cache").unwrap();

        let dest = make_temp_dir("dest-dots");
        let copied = copy_dir_for_test(&src, &dest);

        assert_eq!(
            copied.len(),
            2,
            "should copy only the 2 visible files, got: {:?}",
            copied,
        );
        assert!(dest.join("keep.md").exists());
        assert!(dest.join("subdir/keep2.md").exists());
        // Dot-stuff must NOT be on disk in the destination.
        assert!(!dest.join(".DS_Store").exists());
        assert!(!dest.join(".git").exists());
        assert!(!dest.join(".env").exists());
        assert!(!dest.join("subdir/.cache").exists());

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn copy_directory_returns_flat_list_with_forward_slashes() {
        let src = make_temp_dir("src-flat");
        std::fs::create_dir_all(src.join("year/2024/q3")).unwrap();
        std::fs::write(src.join("year/2024/q3/report.pdf"), b"%PDF-fake").unwrap();
        std::fs::write(src.join("year/2024/notes.md"), b"# notes").unwrap();

        let dest = make_temp_dir("dest-flat");
        let copied = copy_dir_for_test(&src, &dest);

        // Both files in the flat list, ordered by file-system traversal
        // (we don't care about exact order, but every entry must be
        // forward-slashed and end with the expected filename).
        let names: Vec<String> = copied
            .iter()
            .map(|p| {
                Path::new(p)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_string()
            })
            .collect();
        assert!(names.contains(&"report.pdf".to_string()));
        assert!(names.contains(&"notes.md".to_string()));
        assert_eq!(copied.len(), 2);
        for p in &copied {
            assert!(p.contains('/'), "should contain at least one /: {p}");
            assert!(!p.contains('\\'), "should NOT contain \\: {p}");
        }

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dest);
    }
}
