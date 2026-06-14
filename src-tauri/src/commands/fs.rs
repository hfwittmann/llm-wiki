//! Thin Tauri command wrappers for file-system operations.
//!
//! All business logic lives in `core::{files, fs_ops, wiki}`. This module
//! contains only the 16 `#[tauri::command]` entry points that bridge the
//! Tauri IPC layer to those core functions.
//!
//! `set_resource_dir_hint` is re-exported here because `lib.rs` calls it
//! during Tauri setup (before it has access to the `core` module path).

// Re-export pdfium helpers so that callers that previously used
// `commands::fs::lock_pdfium` / `commands::fs::set_resource_dir_hint`
// continue to work until they're updated.
pub use crate::core::files::set_resource_dir_hint;

// Re-export FileBase64 so downstream code importing via `commands::fs::FileBase64`
// (e.g. future HTTP handlers) continues to compile.
pub use crate::core::files::FileBase64;

use crate::types::wiki::FileNode;

// ──────────────────────────────────────────────────────────────────────────
// File-IO commands  (core::files)
// ──────────────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn read_file(path: String, extract_images: Option<bool>) -> Result<String, String> {
    crate::core::files::read_file(path, extract_images).await
}

#[tauri::command]
pub async fn write_file(path: String, contents: String) -> Result<(), String> {
    crate::core::files::write_file(path, contents).await
}

#[tauri::command]
pub async fn write_file_base64(path: String, base64: String) -> Result<(), String> {
    crate::core::files::write_file_base64(path, base64).await
}

#[tauri::command]
pub async fn write_file_atomic(path: String, contents: String) -> Result<(), String> {
    crate::core::files::write_file_atomic(path, contents).await
}

#[tauri::command]
pub async fn copy_file(source: String, destination: String) -> Result<(), String> {
    crate::core::files::copy_file(source, destination).await
}

#[tauri::command]
pub async fn copy_directory(source: String, destination: String) -> Result<Vec<String>, String> {
    crate::core::files::copy_directory(source, destination).await
}

#[tauri::command]
pub async fn delete_file(path: String) -> Result<(), String> {
    crate::core::files::delete_file(path).await
}

#[tauri::command]
pub async fn read_file_as_base64(path: String) -> Result<FileBase64, String> {
    crate::core::files::read_file_as_base64(path).await
}

#[tauri::command]
pub async fn file_exists(path: String) -> Result<bool, String> {
    crate::core::files::file_exists(path).await
}

#[tauri::command]
pub async fn get_file_modified_time(path: String) -> Result<u64, String> {
    crate::core::files::get_file_modified_time(path).await
}

#[tauri::command]
pub async fn get_file_size(path: String) -> Result<u64, String> {
    crate::core::files::get_file_size(path).await
}

#[tauri::command]
pub async fn get_file_md5(path: String) -> Result<String, String> {
    crate::core::files::get_file_md5(path).await
}

// ──────────────────────────────────────────────────────────────────────────
// Directory + preprocessing commands  (core::fs_ops)
// ──────────────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_directory(path: String) -> Result<Vec<FileNode>, String> {
    crate::core::fs_ops::list_directory(path).await
}

#[tauri::command]
pub async fn create_directory(path: String) -> Result<(), String> {
    crate::core::fs_ops::create_directory(path).await
}

#[tauri::command]
pub async fn preprocess_file(path: String) -> Result<String, String> {
    crate::core::fs_ops::preprocess_file(path).await
}

// ──────────────────────────────────────────────────────────────────────────
// Wiki-page-level commands  (core::wiki)
// ──────────────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn find_related_wiki_pages(
    project_path: String,
    source_name: String,
) -> Result<Vec<String>, String> {
    crate::core::wiki::find_related_wiki_pages(project_path, source_name).await
}
