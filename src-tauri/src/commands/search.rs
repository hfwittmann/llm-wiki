//! Tauri command wrapper for search_project. Real logic lives in core::search.

// Re-export public types and functions so any code that already imported from
// `commands::search` (e.g. api_server.rs) keeps working without changes.
#[allow(unused_imports)]
pub use crate::core::search::{
    build_snippet, extract_image_refs, extract_title, resolve_query_embedding,
    search_project_inner, tokenize_query, ProjectSearchResponse, ProjectSearchResult,
    SearchEmbeddingConfig, SearchImageRef,
};

#[tauri::command]
pub async fn search_project(
    project_path: String,
    query: String,
    top_k: Option<usize>,
    include_content: Option<bool>,
    query_embedding: Option<Vec<f32>>,
    embedding_config: Option<SearchEmbeddingConfig>,
) -> Result<ProjectSearchResponse, String> {
    crate::core::search::search_project(
        project_path,
        query,
        top_k,
        include_content,
        query_embedding,
        embedding_config,
    )
    .await
    .map_err(|e| e.to_string())
}
